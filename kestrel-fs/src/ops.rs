//! User-initiated filesystem mutations: copy, move, delete, trash.
//!
//! These are the operations where a file manager is judged. The rules this
//! module enforces:
//!
//! * **Delete means trash.** [`trash()`] is the user-facing "delete";
//!   [`delete`] is the explicit, permanent one (Shift+Delete).
//! * **Never clobber by accident.** A copy or move whose destination exists
//!   fails with [`KestrelError::AlreadyExists`] unless the caller passed
//!   [`Collision::Overwrite`](CopyOptions::collision). The UI resolves collisions,
//!   this module does not guess.
//! * **Cross-device moves work.** `fs::rename` fails with `EXDEV` across mount
//!   points, which is exactly what happens when a user drags a folder from
//!   `/home` to a USB stick. [`move_`] detects that and falls back to
//!   copy-then-delete.
//! * **No runaway recursion.** Copying a directory into its own subtree is
//!   rejected before a single byte is written.
//!
//! Symlinks are copied *as symlinks* (never followed) on Unix, and moved
//! wholesale, so a link in a copied tree keeps pointing where it pointed.
//!
//! # Example
//!
//! ```
//! use kestrel_fs::ops::{CopyOptions, MoveStrategy, copy, delete, move_, trash};
//! # use std::path::Path;
//! # fn demo(src: &Path, dst: &Path) -> Result<(), kestrel_fs::KestrelError> {
//! // Copy, refusing to overwrite whatever is already at `dst`.
//! copy(src, dst, CopyOptions::default())?;
//!
//! // Move, with automatic copy+delete across filesystems.
//! move_(src, dst, CopyOptions::default(), MoveStrategy::Auto)?;
//!
//! // "Delete" in a file manager: recoverable.
//! trash(dst)?;
//!
//! // Permanent removal, for the explicit "delete forever" action.
//! delete(src)?;
//! # Ok(())
//! # }
//! ```

use std::path::{Path, PathBuf};

use crate::error::{KestrelError, Result, classify_io};
use crate::model::CancellationToken;

/// Buffer size for file copies. 64 KiB is a good balance between syscall
/// overhead and cache pressure.
const COPY_BUFFER: usize = 64 * 1024;

/// What to do when the destination already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Collision {
    /// Fail with [`KestrelError::AlreadyExists`]. The default: the UI asks the
    /// user, then retries with [`Overwrite`](Self::Overwrite) or
    /// [`Skip`](Self::Skip).
    #[default]
    Fail,
    /// Replace the destination.
    Overwrite,
    /// Leave the destination alone and report success. Useful for "merge these
    /// folders" semantics where existing files are kept.
    Skip,
}

/// Options for [`copy`] and [`move_`].
#[derive(Debug, Clone, Default)]
pub struct CopyOptions {
    /// Collision behaviour at the destination.
    pub collision: Collision,
    /// Collisions the caller has already answered, by destination path.
    ///
    /// A single [`Collision`] applies to *every* colliding path in a call, so
    /// without this there is no way to say "overwrite this one and ask me about
    /// the next": answering a directory's collision answers all of its
    /// children's too, silently. See [`AnsweredCollisions`].
    pub answered: AnsweredCollisions,
    /// Copy permission bits and (on Unix) timestamps to the destination.
    pub preserve_permissions: bool,
    /// Stop as soon as this token is cancelled, leaving a partial destination
    /// behind (which the caller is expected to clean up or report).
    pub cancel: Option<CancellationToken>,
}

/// Collisions the caller has already answered, keyed by destination path.
///
/// This exists to make a *scoped* answer expressible. [`Collision::Overwrite`]
/// passed to [`copy`] applies to every remaining colliding file in the tree, so
/// a caller that asks the user about one collision and then retries with
/// `Overwrite` has silently answered all the others — which is the opposite of
/// what "this one" means.
///
/// A path recorded here is resolved from the map instead of from
/// [`CopyOptions::collision`], and is never raised as a new collision. A caller
/// that wants the next collision asked about records the answer, then retries
/// with [`Collision::Fail`]: the settled path no longer raises, and the next
/// unrecorded one does.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnsweredCollisions(std::collections::BTreeMap<PathBuf, Collision>);

impl AnsweredCollisions {
    /// Nothing answered yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records the answer for one destination.
    pub fn answer(&mut self, dst: impl Into<PathBuf>, collision: Collision) {
        self.0.insert(dst.into(), collision);
    }

    /// The answer for `dst`, if the caller gave one.
    #[must_use]
    pub fn get(&self, dst: &Path) -> Option<Collision> {
        self.0.get(dst).copied()
    }

    /// How many collisions have been answered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// `true` when nothing has been answered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The policy in force for one destination: an answer recorded for this exact
/// path wins over the blanket [`CopyOptions::collision`].
fn collision_for(options: &CopyOptions, dst: &Path) -> Collision {
    options.answered.get(dst).unwrap_or(options.collision)
}

/// How [`move_`] should attempt the move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MoveStrategy {
    /// `rename` first, falling back to copy-then-delete on `EXDEV`. The default
    /// and the only correct option for directories.
    #[default]
    Auto,
    /// Skip the `rename` attempt and always copy-then-delete.
    ///
    /// Exposed mainly so the fallback path can be tested on a single-filesystem
    /// CI box, and for callers that know the destination is on another device.
    CopyThenDelete,
}

/// Copies `src` to `dst` recursively.
///
/// # Errors
///
/// * [`KestrelError::AlreadyExists`] — `dst` exists and
///   [`Collision::Fail`] was in effect.
/// * [`KestrelError::InvalidInput`] — `dst` is inside `src` (copying a folder
///   into itself would grow without bound).
/// * [`KestrelError::PermissionDenied`], [`KestrelError::NotFound`], ... —
///   whatever the filesystem reports, classified.
pub fn copy(src: impl AsRef<Path>, dst: impl AsRef<Path>, options: CopyOptions) -> Result<()> {
    let (src, dst) = (src.as_ref(), dst.as_ref());
    let cancel = options.cancel.clone().unwrap_or_default();

    let metadata = std::fs::symlink_metadata(src).map_err(|e| classify_io(src, e))?;
    // Before the collision check, and regardless of the policy: `Overwrite` is
    // an answer to "replace what is at the destination", and the source is not
    // something the user was ever offered as a thing to replace.
    guard_against_self_destination(src, dst, metadata.is_dir())?;
    check_collision(dst, collision_for(&options, dst))?;

    let mut counter = CopyCounter { cancelled: false };
    copy_recursive(src, dst, &metadata, &options, &cancel, 0, &mut counter)?;
    if counter.cancelled {
        return Err(KestrelError::Cancelled);
    }
    Ok(())
}

/// Marker used to report cancellation that happened inside the recursion.
struct CopyCounter {
    cancelled: bool,
}

/// Moves `src` to `dst`, falling back to copy-then-delete across devices.
///
/// # Errors
///
/// Same as [`copy`], plus [`KestrelError::Cancelled`] if the copy half was
/// interrupted (in which case the source is deliberately *not* deleted, so a
/// cancelled move never loses data).
///
/// ```
/// use kestrel_fs::ops::{CopyOptions, MoveStrategy, move_};
/// # use std::path::Path;
/// # fn demo(a: &Path, b: &Path) -> Result<(), kestrel_fs::KestrelError> {
/// // Force the slow path — the one that runs when the two paths live on
/// // different filesystems and `rename` returns EXDEV.
/// move_(a, b, CopyOptions::default(), MoveStrategy::CopyThenDelete)?;
/// # Ok(())
/// # }
/// ```
pub fn move_(
    src: impl AsRef<Path>,
    dst: impl AsRef<Path>,
    options: CopyOptions,
    strategy: MoveStrategy,
) -> Result<()> {
    let (src, dst) = (src.as_ref(), dst.as_ref());
    let metadata = std::fs::symlink_metadata(src).map_err(|e| classify_io(src, e))?;
    guard_against_self_destination(src, dst, metadata.is_dir())?;

    // `Skip` means the same thing here as it does for `copy`: the destination is
    // left exactly as it was. It has to be decided *before* either side is
    // touched, because a move is two operations and "skip" has to mean that
    // neither of them ran.
    //
    // This is the check the rename below would otherwise undo. `fs::rename`
    // replaces the destination unconditionally, so a `Skip` that let it proceed
    // overwrote the destination and reported success; and the copy-then-delete
    // path deleted the source after a copy that had written nothing at all. One
    // question, one answer, no data loss.
    if collision_for(&options, dst) == Collision::Skip && path_entry_exists(dst) {
        return Ok(());
    }

    if matches!(strategy, MoveStrategy::Auto) {
        // A rename is atomic and free; always try it first. Pre-checks first so
        // we never destroy a file with a blind rename.
        check_collision(dst, collision_for(&options, dst))?;
        match std::fs::rename(src, dst) {
            Ok(()) => return Ok(()),
            Err(e) if is_cross_device(&e) => {
                // Expected: different mounts. Fall through to copy+delete.
            }
            Err(e) => return Err(classify_io(src, e)),
        }
    } else {
        check_collision(dst, collision_for(&options, dst))?;
    }

    // The cross-device path. The source is removed only once the destination is
    // known to exist — never on the strength of a `Result` alone, because a
    // `Skip` deeper in the tree makes `copy` succeed having written nothing, and
    // a successful `copy` is not the same claim as "the bytes are there".
    if !path_entry_exists(dst) {
        copy(src, dst, options)?;
        if !path_entry_exists(dst) {
            return Err(KestrelError::InvalidInput {
                path: dst.to_path_buf(),
                reason: format!(
                    "nothing was copied to {}, so {} was left where it is",
                    dst.display(),
                    src.display()
                ),
            });
        }
    }
    delete_recursive(src, None)?;
    Ok(())
}

/// Sends `path` to the platform recycle bin.
///
/// This is the "Delete" action in the UI. On Linux it implements the freedesktop
/// trash spec, on macOS it uses `.Trash` (and the volume's `.Trashes` for
/// external drives), on Windows the Shell API — the behaviour users expect, and
/// far safer than `unlink`.
///
/// # Errors
///
/// [`KestrelError::Trash`] if the platform implementation refuses, plus the
/// usual classified errors when `path` does not exist.
pub fn trash(path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    if let Err(e) = std::fs::symlink_metadata(path) {
        return Err(classify_io(path, e));
    }
    trash::delete(path).map_err(|source| KestrelError::Trash {
        path: path.to_path_buf(),
        source: Box::new(source),
    })
}

/// Permanently removes a single file, symlink, or **empty** directory.
///
/// Use [`delete_recursive`] for a non-empty directory; the separation exists so
/// a Shift+Delete on a folder is never one keystroke from losing a tree.
///
/// # Errors
///
/// [`KestrelError::DirectoryNotEmpty`] for a populated directory.
pub fn delete(path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    let metadata = std::fs::symlink_metadata(path).map_err(|e| classify_io(path, e))?;
    // Note: `is_dir` is false for a symlink to a directory, so deleting a link
    // never reaches into its target.
    if metadata.is_dir() {
        std::fs::remove_dir(path).map_err(|e| classify_io(path, e))
    } else {
        std::fs::remove_file(path).map_err(|e| classify_io(path, e))
    }
}

/// Permanently removes a file, or a directory and everything under it.
///
/// # Errors
///
/// Any classified error from the recursive walk; a partial deletion may remain
/// and is reported per path through the same error type.
pub fn delete_recursive(path: impl AsRef<Path>, cancel: Option<&CancellationToken>) -> Result<()> {
    let path = path.as_ref();
    let metadata = std::fs::symlink_metadata(path).map_err(|e| classify_io(path, e))?;
    if !metadata.is_dir() {
        return delete(path);
    }

    // Collect first: mutating a directory while iterating its `read_dir` handle
    // is undefined behaviour on some platforms.
    let mut children = Vec::new();
    for entry in std::fs::read_dir(path).map_err(|e| classify_io(path, e))? {
        let entry = entry.map_err(|e| classify_io(path, e))?;
        children.push(entry.path());
    }
    for child in children {
        if let Some(token) = cancel
            && token.is_cancelled()
        {
            return Err(KestrelError::Cancelled);
        }
        delete_recursive(&child, cancel)?;
    }
    std::fs::remove_dir(path).map_err(|e| classify_io(path, e))
}

/// `true` when the error is the "these are on different filesystems" case.
#[must_use]
pub fn is_cross_device(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::CrossesDevices || error.raw_os_error() == Some(18)
}

/// Fails if `dst` exists and the policy is [`Collision::Fail`].
fn check_collision(dst: &Path, collision: Collision) -> Result<()> {
    if collision != Collision::Fail {
        return Ok(());
    }
    if path_entry_exists(dst) {
        return Err(KestrelError::AlreadyExists {
            path: dst.to_path_buf(),
        });
    }
    Ok(())
}

/// `true` if *something* occupies `path`, **without following a final symlink**.
///
/// [`Path::exists`] answers for the *target* of a symlink, so a dangling link
/// reads as absent. That is the wrong answer wherever the question is really
/// "is this name taken": a dangling link at the destination occupies the name,
/// `symlink()` on it fails `EEXIST`, and a collision at the top level is not a
/// collision. `symlink_metadata` answers for the name itself.
///
/// Public because the job layer has to make the same decision before it hands
/// the work over — "was this destination here before I started?" is the
/// question that decides whether a cancelled copy may clean up after itself.
#[must_use]
pub fn path_entry_exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// Rejects the two destinations that destroy data instead of producing it.
///
/// 1. `src` and `dst` are the same file. `copy_file_contents` opens the source
///    and *then* creates the destination, so an identical pair truncates the
///    source to zero bytes; the copy-then-delete move path then removes it
///    outright. The reachable route is mundane: pasting a file into its own
///    directory computes `dst = dir.join(file_name)`, which is `src`.
/// 2. `dst` is inside `src` (a directory copy that would grow without bound).
///
/// Both are refused **before** [`check_collision`] and regardless of the
/// policy. `Collision::Overwrite` is the user's answer to "replace what is at
/// the destination", and the source was never offered as a thing to replace;
/// letting the policy decide here is what made the bug invisible.
///
/// Symlinks are compared by resolved path, so a `dst` that reaches `src`
/// *through* a symlink is caught too. That is deliberately the conservative
/// direction: a link copied over a different link to the same target is refused
/// rather than recreated, which cannot lose anything.
fn guard_against_self_destination(src: &Path, dst: &Path, src_is_dir: bool) -> Result<()> {
    // The lexical test first: it is exact, needs no filesystem round-trip, and
    // is the case a paste into the file's own directory actually produces.
    if src == dst {
        return Err(same_destination_error(src));
    }
    // `dst` usually does not exist yet, so canonicalise the deepest ancestor
    // that does and re-attach the rest. `src` always exists.
    let Some(src_canon) = canonicalize_with_missing_tail(src) else {
        return Ok(());
    };
    let Some(dst_canon) = canonicalize_with_missing_tail(dst) else {
        return Ok(());
    };
    if src_canon == dst_canon {
        return Err(same_destination_error(src));
    }
    if src_is_dir && dst_canon.starts_with(&src_canon) {
        return Err(KestrelError::InvalidInput {
            path: dst.to_path_buf(),
            reason: format!("cannot copy {} into its own subdirectory", src.display()),
        });
    }
    Ok(())
}

/// The error for a destination that is the source.
fn same_destination_error(src: &Path) -> KestrelError {
    KestrelError::InvalidInput {
        path: src.to_path_buf(),
        reason: "the source and the destination are the same file".to_string(),
    }
}

/// Canonicalises as much of `path` as exists, then appends the missing tail.
///
/// Returns `None` if not even the first component could be resolved (relative
/// path with no existing prefix, in which case the check is best-effort).
fn canonicalize_with_missing_tail(path: &Path) -> Option<PathBuf> {
    if let Ok(canon) = path.canonicalize() {
        return Some(canon);
    }
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut cursor = path;
    while let Some(parent) = cursor.parent() {
        let Some(name) = cursor.file_name() else {
            break;
        };
        tail.push(name.to_os_string());
        if let Ok(canon) = parent.canonicalize() {
            let mut result = canon;
            for part in tail.iter().rev() {
                result.push(part);
            }
            return Some(result);
        }
        cursor = parent;
    }
    None
}

/// Recursive copy worker. `metadata` describes `src` (from `lstat`).
fn copy_recursive(
    src: &Path,
    dst: &Path,
    metadata: &std::fs::Metadata,
    options: &CopyOptions,
    cancel: &CancellationToken,
    depth: usize,
    counter: &mut CopyCounter,
) -> Result<()> {
    if cancel.is_cancelled() {
        counter.cancelled = true;
        return Err(KestrelError::Cancelled);
    }
    // A defensive ceiling: a bind-mounted tree that canonicalises to distinct
    // paths on every level would otherwise recurse until the stack gives out.
    const MAX_COPY_DEPTH: usize = 512;
    if depth > MAX_COPY_DEPTH {
        return Err(KestrelError::InvalidInput {
            path: dst.to_path_buf(),
            reason: "directory nesting exceeds the supported depth".to_string(),
        });
    }

    let file_type = metadata.file_type();

    if file_type.is_symlink() {
        return copy_symlink(src, dst, collision_for(options, dst));
    }

    let collision = collision_for(options, dst);
    if file_type.is_dir() {
        if path_entry_exists(dst) && collision == Collision::Skip {
            return Ok(());
        }
        match std::fs::create_dir(dst) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(classify_io(dst, e)),
        }
        if options.preserve_permissions {
            let _ = std::fs::set_permissions(dst, metadata.permissions());
        }

        let mut children = Vec::new();
        for entry in std::fs::read_dir(src).map_err(|e| classify_io(src, e))? {
            let entry = entry.map_err(|e| classify_io(src, e))?;
            children.push(entry.path());
        }
        // Stable order keeps progress reporting and error messages
        // reproducible across runs.
        children.sort();

        for child in children {
            let child_metadata =
                std::fs::symlink_metadata(&child).map_err(|e| classify_io(&child, e))?;
            let target = dst.join(child.file_name().unwrap_or(child.as_os_str()));
            if child_metadata.is_dir() && path_entry_exists(&target) {
                check_collision(&target, collision_for(options, &target))?;
            }
            copy_recursive(
                &child,
                &target,
                &child_metadata,
                options,
                cancel,
                depth + 1,
                counter,
            )?;
        }
        return Ok(());
    }

    if !file_type.is_file() {
        // Sockets, fifos and devices are skipped rather than faked.
        return Ok(());
    }

    if path_entry_exists(dst) {
        match collision {
            Collision::Skip => return Ok(()),
            Collision::Fail => {
                return Err(KestrelError::AlreadyExists {
                    path: dst.to_path_buf(),
                });
            }
            Collision::Overwrite => {}
        }
    }

    copy_file_contents(src, dst)?;
    if options.preserve_permissions {
        let _ = std::fs::set_permissions(dst, metadata.permissions());
    }
    Ok(())
}

/// Copies bytes, then best-effort timestamps.
fn copy_file_contents(src: &Path, dst: &Path) -> Result<()> {
    let mut reader = std::fs::File::open(src).map_err(|e| classify_io(src, e))?;
    let mut writer = std::fs::File::create(dst).map_err(|e| classify_io(dst, e))?;
    let mut buffer = vec![0u8; COPY_BUFFER];
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(classify_io(src, e)),
        };
        if let Err(e) = writer.write_all(&buffer[..read]) {
            return Err(classify_io(dst, e));
        }
    }
    // A crash between `write_all` and here leaves a truncated destination.
    if let Err(e) = writer.sync_data() {
        return Err(classify_io(dst, e));
    }
    let _ = copy_times(src, dst);
    Ok(())
}

/// Recreates a symlink at `dst` pointing wherever `src` pointed.
///
/// Copying the *link* rather than its target is what stops a copy of `~/` from
/// dragging in whatever the link happens to reach, and what keeps cycles
/// impossible.
///
/// The collision policy is honoured here, which needs saying because this
/// function used to be reached *before* the caller's collision match: a
/// symlink never consulted the policy at all, and unlinked the destination
/// unconditionally. Under [`Collision::Skip`] that destroyed the very file the
/// policy exists to protect.
///
/// The link is created under a temporary name and then renamed into place, so
/// the destination is never in a state where it has been removed but not yet
/// replaced. Unlink-then-link has a window — a crash, a cancel, another reader
/// of the tree — in which the link is simply gone.
#[cfg(unix)]
fn copy_symlink(src: &Path, dst: &Path, collision: Collision) -> Result<()> {
    // Consult the policy on the *name*, not on what it points at: a dangling
    // link occupies the destination even though `Path::exists` says otherwise,
    // and `symlink` on that name fails `EEXIST`.
    if path_entry_exists(dst) {
        match collision {
            Collision::Skip => return Ok(()),
            Collision::Fail => {
                return Err(KestrelError::AlreadyExists {
                    path: dst.to_path_buf(),
                });
            }
            Collision::Overwrite => {}
        }
    }

    let target = std::fs::read_link(src).map_err(|e| classify_io(src, e))?;
    let temp = temporary_link_name(dst);
    std::os::unix::fs::symlink(&target, &temp).map_err(|e| classify_io(dst, e))?;
    // `rename` replaces an existing entry — file, link, or dangling link — in
    // one step, so `Overwrite` no longer needs the destination destroyed first.
    if let Err(e) = std::fs::rename(&temp, dst) {
        // The temporary link is ours and nobody else can want it; leaving it
        // would be litter in the user's directory.
        let _ = std::fs::remove_file(&temp);
        return Err(classify_io(dst, e));
    }
    Ok(())
}

/// A sibling name for [`copy_symlink`]'s create-then-rename step.
///
/// In the destination's own directory so the rename stays within one
/// filesystem, and prefixed so it cannot collide with a name the user made.
#[cfg(unix)]
fn temporary_link_name(dst: &Path) -> PathBuf {
    let mut name = std::ffi::OsString::from(".kestrel-link-");
    if let Some(stem) = dst.file_name() {
        name.push(stem);
    }
    dst.with_file_name(name)
}

/// On platforms without a stable symlink-creation API in `std`, copying a link
/// copies what it points at. Slightly different semantics, documented in the
/// module docs.
#[cfg(not(unix))]
fn copy_symlink(src: &Path, dst: &Path, collision: Collision) -> Result<()> {
    let metadata = std::fs::metadata(src).map_err(|e| classify_io(src, e))?;
    let counter = &mut CopyCounter { cancelled: false };
    let options = CopyOptions {
        collision,
        preserve_permissions: false,
        cancel: None,
    };
    copy_recursive(
        src,
        dst,
        &metadata,
        &options,
        &CancellationToken::new(),
        0,
        counter,
    )
}

/// Best-effort timestamp preservation (mtime matters to build systems and
/// to rsync-style syncs; atime is copied when the platform reports it).
#[cfg(unix)]
fn copy_times(src: &Path, dst: &Path) -> std::io::Result<()> {
    let metadata = src.metadata()?;
    let mut times = std::fs::FileTimes::new();
    if let Ok(accessed) = metadata.accessed() {
        times = times.set_accessed(accessed);
    }
    if let Ok(modified) = metadata.modified() {
        times = times.set_modified(modified);
    }
    let _ = std::fs::File::open(dst)?.set_times(times);
    Ok(())
}

/// Timestamps are not copied on non-Unix platforms: `std` offers no portable
/// way to set them. Permission bits are still copied, via
/// [`fs::set_permissions`].
#[cfg(not(unix))]
fn copy_times(_src: &Path, _dst: &Path) -> std::io::Result<()> {
    Ok(())
}

use std::io::{Read, Write};

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tree(dir: &Path) {
        fs::create_dir_all(dir.join("sub")).expect("mkdir");
        fs::write(dir.join("a.txt"), b"alpha").expect("write");
        fs::write(dir.join("sub/b.txt"), b"beta").expect("write");
    }

    #[test]
    fn copies_a_tree() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir_all(src.join("sub")).expect("mkdir");
        fs::write(src.join("a.txt"), b"alpha").expect("write");
        fs::write(src.join("sub/b.txt"), b"beta").expect("write");

        copy(&src, &dst, CopyOptions::default()).expect("copy");

        assert_eq!(fs::read(dst.join("a.txt")).expect("read"), b"alpha");
        assert_eq!(fs::read(dst.join("sub/b.txt")).expect("read"), b"beta");
        assert!(dst.join("sub").is_dir(), "subtree should be copied");
        // The source must be untouched.
        assert!(src.join("a.txt").exists());
        assert!(src.join("sub/b.txt").exists());
    }

    #[test]
    fn refuses_to_overwrite_without_explicit_permission() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("src.txt");
        let dst = tmp.path().join("dst.txt");
        fs::write(&src, b"new").expect("write");
        fs::write(&dst, b"old").expect("write");

        let err = copy(&src, &dst, CopyOptions::default()).expect_err("should refuse");
        assert!(
            matches!(err, KestrelError::AlreadyExists { .. }),
            "got {err}"
        );
        assert_eq!(
            fs::read(&dst).expect("read"),
            b"old",
            "destination untouched"
        );

        copy(
            &src,
            &dst,
            CopyOptions {
                collision: Collision::Overwrite,
                ..CopyOptions::default()
            },
        )
        .expect("overwrite is explicit");
        assert_eq!(fs::read(&dst).expect("read"), b"new");
    }

    #[test]
    fn skip_collision_keeps_the_existing_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir(&src).expect("mkdir");
        fs::create_dir(&dst).expect("mkdir");
        fs::write(src.join("same.txt"), b"new").expect("write");
        fs::write(dst.join("same.txt"), b"old").expect("write");

        copy(
            &src,
            &dst,
            CopyOptions {
                collision: Collision::Skip,
                ..CopyOptions::default()
            },
        )
        .expect("merge");
        assert_eq!(fs::read(dst.join("same.txt")).expect("read"), b"old");
    }

    #[test]
    fn merge_refuses_when_a_child_collides() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir(&src).expect("mkdir");
        fs::create_dir(&dst).expect("mkdir");
        fs::write(src.join("same.txt"), b"new").expect("write");
        fs::write(dst.join("same.txt"), b"old").expect("write");

        // Directory-level collision is allowed (merge), but the colliding file
        // must be reported rather than clobbered.
        let err = copy(&src, &dst, CopyOptions::default()).expect_err("child collision");
        assert!(
            matches!(err, KestrelError::AlreadyExists { .. }),
            "got {err}"
        );
    }

    #[test]
    fn refuses_to_copy_a_directory_into_its_own_subtree() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("src");
        fs::create_dir_all(src.join("inner")).expect("mkdir");
        let dst = src.join("inner/src");

        let err = copy(&src, &dst, CopyOptions::default()).expect_err("should refuse");
        assert!(
            matches!(err, KestrelError::InvalidInput { .. }),
            "got {err}"
        );
    }

    #[test]
    fn move_uses_rename_when_possible() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("a.txt");
        let dst = tmp.path().join("b.txt");
        fs::write(&src, b"data").expect("write");

        move_(&src, &dst, CopyOptions::default(), MoveStrategy::Auto).expect("move");
        assert!(!src.exists());
        assert_eq!(fs::read(&dst).expect("read"), b"data");
    }

    #[test]
    fn move_falls_back_to_copy_then_delete() {
        // This is the EXDEV path. We cannot mount a second filesystem in a
        // test, so we drive the fallback directly: it must produce exactly the
        // result of a successful rename, with the source gone.
        let tmp = tempfile::tempdir().expect("tempdir");
        tree(tmp.path());
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir_all(src.join("sub")).expect("mkdir");
        fs::write(src.join("a.txt"), b"alpha").expect("write");
        fs::write(src.join("sub/b.txt"), b"beta").expect("write");

        move_(
            &src,
            &dst,
            CopyOptions::default(),
            MoveStrategy::CopyThenDelete,
        )
        .expect("fallback move");

        assert!(!src.exists(), "source must be removed after the copy");
        assert_eq!(fs::read(dst.join("a.txt")).expect("read"), b"alpha");
        assert_eq!(fs::read(dst.join("sub/b.txt")).expect("read"), b"beta");
    }

    #[test]
    fn move_refuses_to_clobber() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("a.txt");
        let dst = tmp.path().join("b.txt");
        fs::write(&src, b"new").expect("write");
        fs::write(&dst, b"old").expect("write");

        let err = move_(&src, &dst, CopyOptions::default(), MoveStrategy::Auto)
            .expect_err("should refuse");
        assert!(
            matches!(err, KestrelError::AlreadyExists { .. }),
            "got {err}"
        );
        assert!(src.exists(), "source must survive a refused move");
    }

    #[test]
    fn copying_a_symlink_copies_the_link() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let target = tmp.path().join("target.txt");
        let link = tmp.path().join("link.txt");
        let copy_path = tmp.path().join("copy.txt");
        fs::write(&target, b"payload").expect("write");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).expect("link");

        copy(&link, &copy_path, CopyOptions::default()).expect("copy");
        #[cfg(unix)]
        {
            let meta = fs::symlink_metadata(&copy_path).expect("lstat");
            assert!(meta.file_type().is_symlink(), "must stay a symlink");
            assert_eq!(fs::read(&copy_path).expect("read"), b"payload");
        }
    }

    #[test]
    fn cancelled_copy_writes_nothing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("src.txt");
        let dst = tmp.path().join("dst.txt");
        fs::write(&src, b"data").expect("write");
        let token = CancellationToken::new();
        token.cancel();

        let err = copy(
            &src,
            &dst,
            CopyOptions {
                cancel: Some(token),
                ..CopyOptions::default()
            },
        )
        .expect_err("cancelled");
        assert!(matches!(err, KestrelError::Cancelled), "got {err}");
        assert!(!dst.exists(), "nothing should have been written");
    }

    #[test]
    fn delete_removes_files_and_empty_dirs_only() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file = tmp.path().join("f.txt");
        fs::write(&file, b"x").expect("write");
        delete(&file).expect("delete file");
        assert!(!file.exists());

        let dir = tmp.path().join("d");
        fs::create_dir(&dir).expect("mkdir");
        fs::write(dir.join("child"), b"x").expect("write");
        let err = delete(&dir).expect_err("non-empty");
        assert!(
            matches!(err, KestrelError::DirectoryNotEmpty { .. }),
            "got {err}"
        );

        delete_recursive(&dir, None).expect("recursive delete");
        assert!(!dir.exists());
    }

    #[test]
    fn missing_paths_are_reported_as_not_found() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let missing = tmp.path().join("nope");
        assert!(
            copy(&missing, tmp.path().join("x"), CopyOptions::default())
                .expect_err("copy")
                .is_not_found()
        );
        assert!(delete(&missing).expect_err("delete").is_not_found());
        assert!(
            move_(
                &missing,
                tmp.path().join("x"),
                CopyOptions::default(),
                MoveStrategy::Auto
            )
            .expect_err("move")
            .is_not_found()
        );
    }

    #[test]
    fn cross_device_detection() {
        assert!(is_cross_device(&std::io::Error::from_raw_os_error(18)));
        assert!(is_cross_device(&std::io::Error::from(
            std::io::ErrorKind::CrossesDevices
        )));
        assert!(!is_cross_device(&std::io::Error::from(
            std::io::ErrorKind::PermissionDenied
        )));
    }
    #[test]
    fn copying_a_file_onto_itself_is_refused() {
        for collision in [Collision::Fail, Collision::Overwrite] {
            let tmp = tempfile::tempdir().expect("tempdir");
            let src = tmp.path().join("a.txt");
            fs::write(&src, b"precious").expect("write");

            let err = copy(
                &src,
                &src,
                CopyOptions {
                    collision,
                    ..CopyOptions::default()
                },
            )
            .expect_err("a file cannot be copied onto itself");
            assert!(
                matches!(err, KestrelError::InvalidInput { .. }),
                "got {err}"
            );
            assert_eq!(
                fs::read(&src).expect("read"),
                b"precious",
                "the source must survive {collision:?}"
            );
        }
    }

    /// The same refusal for a move, across both strategies: `Auto` reaches
    /// `rename` and `CopyThenDelete` reaches the copy-then-delete path, and the
    /// second one used to destroy the file outright.
    #[test]
    fn moving_a_file_onto_itself_is_refused() {
        for collision in [Collision::Fail, Collision::Overwrite] {
            for strategy in [MoveStrategy::Auto, MoveStrategy::CopyThenDelete] {
                let tmp = tempfile::tempdir().expect("tempdir");
                let src = tmp.path().join("a.txt");
                fs::write(&src, b"precious").expect("write");

                let err = move_(
                    &src,
                    &src,
                    CopyOptions {
                        collision,
                        ..CopyOptions::default()
                    },
                    strategy,
                )
                .expect_err("a file cannot be moved onto itself");
                assert!(
                    matches!(err, KestrelError::InvalidInput { .. }),
                    "got {err}"
                );
                assert_eq!(
                    fs::read(&src).expect("read"),
                    b"precious",
                    "the source must survive {collision:?}/{strategy:?}"
                );
            }
        }
    }

    /// A directory onto itself is the same bug one level down: the engine
    /// "merged" the tree into itself and truncated every file it found. Into a
    /// descendant is the `cp -r a a/a` case, which the existing guard covers —
    /// both are asserted here together, under both policies, because the guard
    /// is a precondition and a precondition cannot depend on the policy.
    #[test]
    fn a_directory_is_never_copied_into_itself_or_a_descendant() {
        for collision in [Collision::Fail, Collision::Overwrite] {
            let tmp = tempfile::tempdir().expect("tempdir");
            let src = tmp.path().join("src");
            fs::create_dir_all(src.join("inner")).expect("mkdir");
            fs::write(src.join("a.txt"), b"precious").expect("write");

            for dst in [src.clone(), src.join("inner/src")] {
                let err = copy(
                    &src,
                    &dst,
                    CopyOptions {
                        collision,
                        ..CopyOptions::default()
                    },
                )
                .expect_err("a directory cannot be copied into itself");
                assert!(
                    matches!(err, KestrelError::InvalidInput { .. }),
                    "got {err}"
                );
            }
            assert_eq!(
                fs::read(src.join("a.txt")).expect("read"),
                b"precious",
                "the tree must survive {collision:?}"
            );
        }
    }

    #[test]
    fn move_with_skip_leaves_both_source_and_destination() {
        for strategy in [MoveStrategy::Auto, MoveStrategy::CopyThenDelete] {
            let tmp = tempfile::tempdir().expect("tempdir");
            let src = tmp.path().join("src");
            let dst = tmp.path().join("dst");
            fs::create_dir_all(src.join("sub")).expect("mkdir");
            fs::write(src.join("a.txt"), b"new").expect("write");
            fs::write(src.join("sub/b.txt"), b"new").expect("write");
            fs::create_dir_all(&dst).expect("mkdir");
            fs::write(dst.join("a.txt"), b"old").expect("write");

            move_(
                &src,
                &dst,
                CopyOptions {
                    collision: Collision::Skip,
                    ..CopyOptions::default()
                },
                strategy,
            )
            .expect("skip is a valid answer, not an error");

            assert_eq!(
                fs::read(dst.join("a.txt")).expect("read"),
                b"old",
                "the destination must be untouched ({strategy:?})"
            );
            assert!(
                src.exists(),
                "a skipped move must not delete the source ({strategy:?})"
            );
            assert_eq!(
                fs::read(src.join("a.txt")).expect("read"),
                b"new",
                "the source must be intact ({strategy:?})"
            );
            assert_eq!(
                fs::read(src.join("sub/b.txt")).expect("read"),
                b"new",
                "the whole source tree must be intact ({strategy:?})"
            );
        }
    }

    /// The cross-device path is where the destruction happened: the copy
    /// returned early having written nothing, and the source was deleted
    /// anyway. This drives that fallback directly, because a single-filesystem
    /// test box cannot produce a real `EXDEV`.
    #[test]
    fn move_across_devices_with_skip_does_not_delete_the_source() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir_all(src.join("sub")).expect("mkdir");
        fs::write(src.join("a.txt"), b"new").expect("write");
        fs::write(src.join("sub/b.txt"), b"new").expect("write");
        fs::create_dir_all(&dst).expect("mkdir");
        fs::write(dst.join("a.txt"), b"old").expect("write");

        move_(
            &src,
            &dst,
            CopyOptions {
                collision: Collision::Skip,
                ..CopyOptions::default()
            },
            MoveStrategy::CopyThenDelete,
        )
        .expect("skip");

        assert!(
            src.join("sub/b.txt").exists(),
            "the source must survive a skipped cross-device move"
        );
        assert_eq!(fs::read(dst.join("a.txt")).expect("read"), b"old");
    }

    /// A dangling link at the destination occupies the name, and `Skip` has to
    /// respect that. `Path::exists` follows the link and reports `false`, so
    /// this is the case that used to reach `symlink()` and fail `EEXIST` — or,
    /// worse, unlink the destination on the way.
    #[test]
    #[cfg(unix)]
    fn a_dangling_link_at_the_destination_is_still_a_collision() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let target = tmp.path().join("target.txt");
        fs::write(&target, b"payload").expect("write");
        let link = tmp.path().join("link.txt");
        std::os::unix::fs::symlink(&target, &link).expect("link");
        let dst = tmp.path().join("dst.txt");
        std::os::unix::fs::symlink(tmp.path().join("gone"), &dst).expect("dangling");

        copy(
            &link,
            &dst,
            CopyOptions {
                collision: Collision::Skip,
                ..CopyOptions::default()
            },
        )
        .expect("skip");
        assert!(
            fs::symlink_metadata(&dst)
                .expect("lstat")
                .file_type()
                .is_symlink(),
            "the existing link must still be a link"
        );
        assert_eq!(
            fs::read_link(&dst).expect("readlink"),
            tmp.path().join("gone"),
            "Skip must leave the destination link alone"
        );
    }

    /// A symlink at the top of a copy must consult the collision policy like
    /// any other file.
    ///
    /// `copy_recursive` returns to `copy_symlink` *before* the caller's
    /// collision match, so the link never saw the policy and unlinked the
    /// destination unconditionally — including under [`Collision::Skip`], whose
    /// entire purpose is to leave the destination alone. The destination here
    /// is a real file with real bytes, and the assertion is that it is still
    /// there.
    #[test]
    #[cfg(unix)]
    fn a_symlink_copy_obeys_the_collision_policy() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let target = tmp.path().join("t.txt");
        fs::write(&target, b"payload").expect("write");
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&target, &link).expect("link");
        let dst = tmp.path().join("dst");
        fs::write(&dst, b"in the way").expect("write");

        copy(
            &link,
            &dst,
            CopyOptions {
                collision: Collision::Skip,
                ..CopyOptions::default()
            },
        )
        .expect("skip");
        assert_eq!(
            fs::read(&dst).expect("read"),
            b"in the way",
            "Skip must not remove the destination of a symlink copy"
        );
        assert!(
            !fs::symlink_metadata(&dst)
                .expect("lstat")
                .file_type()
                .is_symlink(),
            "Skip must not have replaced the destination with a link"
        );

        // `Overwrite` is the answer that *does* replace the destination — and it
        // must still work, now via create-then-rename.
        copy(
            &link,
            &dst,
            CopyOptions {
                collision: Collision::Overwrite,
                ..CopyOptions::default()
            },
        )
        .expect("overwrite");
        assert!(
            fs::symlink_metadata(&dst)
                .expect("lstat")
                .file_type()
                .is_symlink(),
            "Overwrite must replace the file with a link"
        );
        assert_eq!(fs::read(&dst).expect("read"), b"payload");

        let leftovers: Vec<String> = fs::read_dir(tmp.path())
            .expect("readdir")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".kestrel-link-"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "the create-then-rename step left litter behind: {leftovers:?}"
        );
    }

    /// A symlink *inside* a copied tree, replacing a colliding file.
    ///
    /// Note what this does and does not cover. `Skip` on a colliding symlink
    /// child is unreachable: `copy_recursive` skips a whole pre-existing
    /// directory subtree before it reaches any child, so the only way a child
    /// link meets an occupied name is `Overwrite`. That is what this asserts.
    /// The `Skip` data-loss case is
    /// [`a_symlink_copy_obeys_the_collision_policy`], where the link is the
    /// root of the copy and no directory sits above it.
    #[test]
    #[cfg(unix)]
    fn a_symlink_child_overwrites_its_colliding_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir_all(src.join("sub")).expect("mkdir");
        fs::write(tmp.path().join("t.txt"), b"payload").expect("write");
        std::os::unix::fs::symlink(tmp.path().join("t.txt"), src.join("sub/l")).expect("link");
        fs::create_dir_all(dst.join("sub")).expect("mkdir");
        fs::write(dst.join("sub/l"), b"in the way").expect("write");

        copy(
            &src,
            &dst,
            CopyOptions {
                collision: Collision::Overwrite,
                ..CopyOptions::default()
            },
        )
        .expect("overwrite");
        assert!(
            fs::symlink_metadata(dst.join("sub/l"))
                .expect("lstat")
                .file_type()
                .is_symlink(),
            "Overwrite must replace the file with a link"
        );
        assert_eq!(fs::read(dst.join("sub/l")).expect("read"), b"payload");

        // The create-then-rename step must not leave a temporary link behind,
        // here or in the directory the link was created in.
        let leftovers: Vec<String> = fs::read_dir(dst.join("sub"))
            .expect("readdir")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".kestrel-link-"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "the create-then-rename step left litter behind: {leftovers:?}"
        );
    }
}
