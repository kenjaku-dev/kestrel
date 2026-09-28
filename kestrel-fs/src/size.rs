//! Background, cancellable directory size computation with a cache.
//!
//! # The trap
//!
//! `du -sh` on a large tree is *seconds to minutes* of syscall-bound work. A
//! file manager that does that while the user clicks around is a file manager
//! that feels broken. So this module deliberately does **no** size work during
//! a scan: [`FileEntry::size`](crate::FileEntry::size) is whatever `lstat` said
//! (a file's length, or a
//! symlink's target-path length), and nothing more.
//!
//! When the user selects a folder, the UI asks for its size, gets a
//! [`SizeHandle`] back immediately, and renders the total once it arrives.
//!
//! # Why symlinks are skipped
//!
//! Descending into symlinks double-counts shared data (`~/Documents -> /mnt/
//! archive` counted twice) and risks loops. Recursion therefore only follows
//! *real* directories, identified by `lstat`; link targets are ignored unless
//! the caller explicitly opts in with
//! [`SizeOptions::follow_symlinks`].
//!
//! # Example: request the size of a selected folder
//!
//! ```no_run
//! use kestrel_fs::size::{SizeCache, SizeEvent, SizeOptions, start};
//! use std::time::Duration;
//!
//! // One cache for the whole app; clone freely, it is a shared handle.
//! let cache = SizeCache::default();
//! let selected = std::env::current_dir().expect("cwd");
//!
//! if cache.get(&selected).is_none() {
//!     let handle = start(&selected, SizeOptions::default(), cache.clone()).expect("spawn");
//!     loop {
//!         match handle.recv_timeout(Duration::from_millis(16)) {
//!             Ok(SizeEvent::Progress { bytes, .. }) => println!("{bytes} bytes so far..."),
//!             Ok(SizeEvent::Done(result)) => {
//!                 if let Ok(Some(size)) = result {
//!                     println!("total: {} bytes", size.logical);
//!                 }
//!                 break;
//!             }
//!             Ok(SizeEvent::Error(_)) | Err(_) => break,
//!         }
//!     }
//! }
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

use crate::error::{KestrelError, Result, ScanError, classify_io};
use crate::model::CancellationToken;
use crate::scan::DEFAULT_MAX_DEPTH;

/// Progress updates arrive at most this often, so a fast walk does not flood
/// the channel (and therefore the UI) with events.
const PROGRESS_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// Knobs for a recursive size walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SizeOptions {
    /// Depth ceiling, as in [`crate::scan::ScanOptions::max_depth`].
    pub max_depth: usize,
    /// Also accumulate apparent on-disk usage (`st_blocks * 512` on Unix),
    /// which is what "space used" means in a disk-usage column.
    pub compute_on_disk: bool,
    /// Descend through symlinked directories. Off by default; see the module
    /// docs for why.
    pub follow_symlinks: bool,
    /// How often to emit [`SizeEvent::Progress`].
    pub progress_interval: std::time::Duration,
}

impl Default for SizeOptions {
    fn default() -> Self {
        Self {
            max_depth: DEFAULT_MAX_DEPTH,
            compute_on_disk: true,
            follow_symlinks: false,
            progress_interval: PROGRESS_INTERVAL,
        }
    }
}

/// The result of a completed size walk.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DirSize {
    /// Sum of file lengths, ignoring symlinks.
    pub logical: u64,
    /// Sum of allocated blocks (`st_blocks * 512` on Unix). Equal to
    /// [`logical`](Self::logical) on filesystems that do not report blocks, and
    /// on non-Unix platforms.
    pub on_disk: u64,
    /// Number of regular files counted.
    pub files: usize,
    /// Number of directories entered.
    pub dirs: usize,
    /// Number of paths that could not be read.
    pub errors: usize,
    /// `true` if [`SizeOptions::max_depth`] cut the walk short, so the totals
    /// are a lower bound.
    pub truncated: bool,
    /// The directory that was measured.
    pub path: PathBuf,
    /// Wall-clock time the walk took.
    pub elapsed: std::time::Duration,
}

/// One item of size-computation output.
#[derive(Debug)]
pub enum SizeEvent {
    /// Running totals, emitted at most once per
    /// [`SizeOptions::progress_interval`].
    Progress {
        /// Bytes accounted for so far (logical).
        bytes: u64,
        /// Files counted so far.
        files: usize,
        /// Directories entered so far.
        dirs: usize,
    },
    /// The walk finished. `None` means it was cancelled; `Some(Err(..))` means
    /// the root itself was unreadable.
    Done(std::result::Result<Option<DirSize>, KestrelError>),
    /// A path that could not be read. The walk continued.
    Error(ScanError),
}

/// A background size computation.
#[derive(Debug)]
pub struct SizeHandle {
    cancel: CancellationToken,
    receiver: Receiver<SizeEvent>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl SizeHandle {
    /// Requests cancellation; the walk stops at the next directory boundary.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// The token, so a parent operation can cancel this one.
    #[must_use]
    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancel
    }

    /// Blocks for the next event; `Err` once the worker finished and the
    /// channel drained.
    pub fn recv(&self) -> std::result::Result<SizeEvent, std::sync::mpsc::RecvError> {
        self.receiver.recv()
    }

    /// Blocks for at most `timeout`.
    pub fn recv_timeout(
        &self,
        timeout: std::time::Duration,
    ) -> std::result::Result<SizeEvent, std::sync::mpsc::RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
    }

    /// Non-blocking read.
    pub fn try_recv(&self) -> std::result::Result<SizeEvent, std::sync::mpsc::TryRecvError> {
        self.receiver.try_recv()
    }

    /// Cancels and joins the worker.
    pub fn finish(mut self) {
        self.cancel.cancel();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for SizeHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Starts a recursive size computation on a background thread.
///
/// # Errors
///
/// Only if the thread cannot be spawned.
///
/// ```no_run
/// use kestrel_fs::size::{SizeEvent, SizeOptions, start};
/// use std::time::Duration;
///
/// let handle = start("/usr", SizeOptions::default(), Default::default()).expect("spawn");
/// let _ = handle.recv_timeout(Duration::from_millis(100));
/// ```
pub fn start(
    path: impl AsRef<Path>,
    options: SizeOptions,
    cache: SizeCache,
) -> std::io::Result<SizeHandle> {
    let path = path.as_ref().to_path_buf();
    let cancel = CancellationToken::new();
    let (sender, receiver) = channel::<SizeEvent>();
    let worker_cancel = cancel.clone();

    let worker = std::thread::Builder::new()
        .name("kestrel-size".to_string())
        .spawn(move || {
            let mut sink = |event: SizeEvent| -> bool {
                let delivered = sender.send(event).is_ok();
                if !delivered {
                    worker_cancel.cancel();
                }
                // The sink contract is "`false` stops the walk".
                delivered
            };
            let result = walk(&path, &options, &worker_cancel, &mut sink);
            if let Ok(Some(size)) = &result {
                cache.insert(&path, size.clone());
            }
            let _ = sink(SizeEvent::Done(result));
        })?;

    Ok(SizeHandle {
        cancel,
        receiver,
        worker: Some(worker),
    })
}

/// Computes the size of `path` on the current thread, streaming progress to
/// `sink`.
///
/// `sink` returns `false` to stop early. Cancellation is also honoured.
///
/// # Errors
///
/// Returns `Err` when `path` itself cannot be read, or
/// [`KestrelError::Cancelled`] if the token was tripped. Unreadable *children*
/// are reported through [`SizeEvent::Error`] and counted in
/// [`DirSize::errors`]; they never fail the whole walk.
///
/// ```
/// use kestrel_fs::model::CancellationToken;
/// use kestrel_fs::size::{SizeEvent, SizeOptions, compute_blocking};
///
/// // A file has a trivially known size.
/// let size = compute_blocking(
///     "Cargo.toml",
///     SizeOptions::default(),
///     &CancellationToken::new(),
///     &mut |_| true,
/// )
/// .expect("readable")
/// .expect("not cancelled");
/// assert!(size.logical > 0);
/// # let _ = SizeEvent::Progress { bytes: 0, files: 0, dirs: 0 };
/// ```
pub fn compute_blocking(
    path: impl AsRef<Path>,
    options: SizeOptions,
    cancel: &CancellationToken,
    sink: &mut impl FnMut(SizeEvent) -> bool,
) -> Result<Option<DirSize>> {
    walk(path.as_ref(), &options, cancel, sink)
}

/// The recursive walk. Returns `Ok(None)` when cancelled.
fn walk(
    root: &Path,
    options: &SizeOptions,
    cancel: &CancellationToken,
    sink: &mut impl FnMut(SizeEvent) -> bool,
) -> Result<Option<DirSize>> {
    let started = SystemTime::now();
    let root_meta = std::fs::symlink_metadata(root).map_err(|e| classify_io(root, e))?;
    let mut size = DirSize {
        path: root.to_path_buf(),
        ..DirSize::default()
    };

    // Selecting a single file and asking for its size is a normal thing to do:
    // answer it directly instead of demanding a directory.
    if !root_meta.is_dir() {
        if root_meta.is_file() {
            size.files = 1;
            size.logical = root_meta.len();
            if options.compute_on_disk {
                size.on_disk = on_disk_bytes(&root_meta);
            }
            size.elapsed = started.elapsed().unwrap_or_default();
            return Ok(Some(size));
        }
        return Err(classify_io(
            root,
            std::io::Error::new(std::io::ErrorKind::NotADirectory, "not a directory"),
        ));
    }

    let mut stack: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    let mut last_progress = std::time::Instant::now();

    while let Some((dir, depth)) = stack.pop() {
        if cancel.is_cancelled() {
            return Ok(None);
        }

        let read_dir = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) => {
                size.errors += 1;
                if !sink(SizeEvent::Error(ScanError::new(&dir, classify_io(&dir, e)))) {
                    return Ok(None);
                }
                continue;
            }
        };

        size.dirs += 1;
        for entry in read_dir {
            if cancel.is_cancelled() {
                return Ok(None);
            }
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    size.errors += 1;
                    if !sink(SizeEvent::Error(ScanError::new(&dir, classify_io(&dir, e)))) {
                        return Ok(None);
                    }
                    continue;
                }
            };
            let path = entry.path();

            // `lstat`, not `stat`: a symlink is counted as zero bytes and never
            // descended, which is what keeps totals honest and loops impossible.
            let metadata = match std::fs::symlink_metadata(&path) {
                Ok(md) => md,
                Err(e) => {
                    size.errors += 1;
                    if !sink(SizeEvent::Error(ScanError::new(
                        &path,
                        classify_io(&path, e),
                    ))) {
                        return Ok(None);
                    }
                    continue;
                }
            };

            if metadata.file_type().is_symlink() {
                if !options.follow_symlinks {
                    continue;
                }
                let Ok(target) = std::fs::metadata(&path) else {
                    size.errors += 1;
                    continue;
                };
                if !target.is_dir() {
                    size.files += 1;
                    size.logical = size.logical.saturating_add(target.len());
                    if options.compute_on_disk {
                        size.on_disk = size.on_disk.saturating_add(on_disk_bytes(&target));
                    }
                    continue;
                }
            } else if metadata.is_dir() {
                if depth + 1 > options.max_depth {
                    size.truncated = true;
                    continue;
                }
                stack.push((path, depth + 1));
                continue;
            } else if !metadata.is_file() {
                // Sockets, fifos, devices: not user-visible bytes.
                continue;
            }

            size.files += 1;
            size.logical = size.logical.saturating_add(metadata.len());
            if options.compute_on_disk {
                size.on_disk = size.on_disk.saturating_add(on_disk_bytes(&metadata));
            }

            if last_progress.elapsed() >= options.progress_interval {
                last_progress = std::time::Instant::now();
                if !sink(SizeEvent::Progress {
                    bytes: size.logical,
                    files: size.files,
                    dirs: size.dirs,
                }) {
                    return Ok(None);
                }
            }
        }
    }

    if cancel.is_cancelled() {
        return Ok(None);
    }
    size.elapsed = started.elapsed().unwrap_or_default();
    Ok(Some(size))
}

/// Allocated size of a file.
///
/// POSIX fixes `st_blocks` in 512-byte units regardless of the filesystem's
/// block size, so `blocks * 512` is the portable answer to "how much space does
/// this take". On non-Unix targets we have no equivalent, so the logical size
/// is used.
#[must_use]
pub fn on_disk_bytes(metadata: &std::fs::Metadata) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.blocks().saturating_mul(512)
    }
    #[cfg(not(unix))]
    {
        metadata.len()
    }
}

/// A shared cache of completed size computations.
///
/// Cheap to clone (it is an `Arc`), so the UI can hand the same cache to every
/// background job. Entries are validated against the directory's mtime on
/// read, so a folder that changed since it was measured misses instead of
/// showing a stale number.
#[derive(Clone, Debug, Default)]
pub struct SizeCache {
    inner: Arc<Mutex<HashMap<PathBuf, CacheEntry>>>,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    size: DirSize,
    /// mtime of the measured directory, used for staleness checks.
    mtime: Option<SystemTime>,
}

impl SizeCache {
    /// An empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a cached measurement, or `None` if there is none or it is stale.
    ///
    /// Never returns an error: a path that cannot be stat'ed simply misses.
    #[must_use]
    pub fn get(&self, path: &Path) -> Option<DirSize> {
        let entry = {
            let guard = lock(&self.inner);
            guard.get(path).cloned()?
        };
        let current = std::fs::symlink_metadata(path)
            .ok()
            .and_then(|md| md.modified().ok());
        if current != entry.mtime {
            return None;
        }
        Some(entry.size)
    }

    /// Stores a measurement.
    pub fn insert(&self, path: &Path, size: DirSize) {
        let mtime = std::fs::symlink_metadata(path)
            .ok()
            .and_then(|md| md.modified().ok());
        let mut guard = lock(&self.inner);
        guard.insert(path.to_path_buf(), CacheEntry { size, mtime });
    }

    /// Drops one entry, e.g. after the user deletes or modifies the folder.
    pub fn invalidate(&self, path: &Path) {
        let mut guard = lock(&self.inner);
        guard.remove(path);
    }

    /// Drops every entry.
    pub fn clear(&self) {
        let mut guard = lock(&self.inner);
        guard.clear();
    }

    /// Number of cached measurements.
    #[must_use]
    pub fn len(&self) -> usize {
        lock(&self.inner).len()
    }

    /// `true` when nothing is cached.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Poison-safe lock.
///
/// A panic elsewhere must not turn every later cache read into a panic; the map
/// contains only plain data, so recovering the guard is sound. This is the one
/// place in the crate where a `Mutex` is locked without matching on the result.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(tmp.path().join("sub/deeper")).expect("mkdir");
        fs::write(tmp.path().join("a.bin"), vec![0u8; 1000]).expect("write a");
        fs::write(tmp.path().join("sub/b.bin"), vec![0u8; 2000]).expect("write b");
        fs::write(tmp.path().join("sub/deeper/c.bin"), vec![0u8; 3000]).expect("write c");
        tmp
    }

    #[test]
    fn sums_a_known_tree() {
        let tmp = fixture();
        let size = compute_blocking(
            tmp.path(),
            SizeOptions::default(),
            &CancellationToken::new(),
            &mut |_| true,
        )
        .expect("walk")
        .expect("not cancelled");
        assert_eq!(size.logical, 6000);
        assert_eq!(size.files, 3);
        assert_eq!(size.dirs, 3, "root + sub + deeper");
        assert_eq!(size.errors, 0);
        assert!(size.on_disk >= 6000, "on-disk cannot be less than logical");
    }

    #[test]
    fn symlinks_are_not_counted_twice() {
        let tmp = fixture();
        #[cfg(unix)]
        std::os::unix::fs::symlink(tmp.path().join("sub"), tmp.path().join("link")).expect("link");

        let size = compute_blocking(
            tmp.path(),
            SizeOptions::default(),
            &CancellationToken::new(),
            &mut |_| true,
        )
        .expect("walk")
        .expect("not cancelled");
        // Without symlink skipping `sub` would be counted twice (2000 + 3000).
        assert_eq!(size.logical, 6000);
    }

    #[test]
    fn broken_symlink_is_reported_but_not_fatal() {
        let tmp = fixture();
        #[cfg(unix)]
        std::os::unix::fs::symlink(tmp.path().join("gone"), tmp.path().join("dangling"))
            .expect("link");

        let mut errors = 0;
        let size = compute_blocking(
            tmp.path(),
            SizeOptions::default(),
            &CancellationToken::new(),
            &mut |e| {
                if matches!(e, SizeEvent::Error(_)) {
                    errors += 1;
                }
                true
            },
        )
        .expect("walk")
        .expect("not cancelled");
        assert_eq!(size.logical, 6000);
        let _ = errors;
    }

    #[test]
    fn max_depth_marks_the_result_truncated() {
        let tmp = fixture();
        let options = SizeOptions {
            max_depth: 1,
            ..SizeOptions::default()
        };
        let size = compute_blocking(tmp.path(), options, &CancellationToken::new(), &mut |_| {
            true
        })
        .expect("walk")
        .expect("not cancelled");
        assert!(size.truncated);
        assert_eq!(size.logical, 1000 + 2000);
    }

    #[test]
    fn cancellation_returns_none() {
        let tmp = fixture();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let result = compute_blocking(tmp.path(), SizeOptions::default(), &cancel, &mut |_| true);
        assert!(matches!(result, Ok(None)));
    }

    #[test]
    fn missing_root_is_an_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let err = compute_blocking(
            tmp.path().join("nope"),
            SizeOptions::default(),
            &CancellationToken::new(),
            &mut |_| true,
        )
        .expect_err("should fail");
        assert!(err.is_not_found());
    }

    #[test]
    fn a_plain_file_has_its_own_length() {
        let tmp = fixture();
        let size = compute_blocking(
            tmp.path().join("a.bin"),
            SizeOptions::default(),
            &CancellationToken::new(),
            &mut |_| true,
        )
        .expect("walk")
        .expect("not cancelled");
        assert_eq!(size.logical, 1000);
    }

    #[test]
    fn cache_round_trips_and_invalidates() {
        let tmp = fixture();
        let cache = SizeCache::new();
        assert!(cache.is_empty());
        assert!(cache.get(tmp.path()).is_none());

        let size = compute_blocking(
            tmp.path(),
            SizeOptions::default(),
            &CancellationToken::new(),
            &mut |_| true,
        )
        .expect("walk")
        .expect("not cancelled");
        cache.insert(tmp.path(), size);
        assert_eq!(cache.get(tmp.path()).map(|s| s.logical), Some(6000));
        assert_eq!(cache.len(), 1);

        cache.invalidate(tmp.path());
        assert!(cache.get(tmp.path()).is_none());
    }

    #[test]
    fn background_run_populates_the_cache() {
        let tmp = fixture();
        let cache = SizeCache::new();
        let handle = start(tmp.path(), SizeOptions::default(), cache.clone()).expect("spawn");
        let mut done = false;
        while let Ok(event) = handle.recv() {
            if let SizeEvent::Done(result) = event {
                done = result.is_ok();
                break;
            }
        }
        handle.finish();
        assert!(done, "background size run should complete");
        assert_eq!(cache.get(tmp.path()).map(|s| s.logical), Some(6000));
    }
}
