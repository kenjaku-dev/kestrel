//! Core data types: entry model, hidden-file detection, sorting, cancellation.
//!
//! This module is deliberately free of any I/O *policy* — it describes a single
//! filesystem entry, how entries are ordered, and how in-flight work is
//! cancelled. [`crate::scan`], [`crate::size`] and [`crate::ops`] all speak
//! these types.
//!
//! # Example: build and sort a small listing
//!
//! ```
//! use kestrel_fs::model::{EntryKind, FileEntry, SortSpec};
//! use std::time::{Duration, SystemTime};
//!
//! let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
//! let entries = vec![
//!     FileEntry {
//!         name: "file10.txt".into(),
//!         path: "/tmp/file10.txt".into(),
//!         kind: EntryKind::File,
//!         size: Some(10),
//!         modified: Some(now),
//!         hidden: false,
//!         is_dir_target: None,
//!     },
//!     FileEntry {
//!         name: "file2.txt".into(),
//!         path: "/tmp/file2.txt".into(),
//!         kind: EntryKind::File,
//!         size: Some(2),
//!         modified: Some(now),
//!         hidden: false,
//!         is_dir_target: None,
//!     },
//! ];
//!
//! let mut sorted = entries;
//! sorted.sort_by(SortSpec::default().comparator());
//! assert_eq!(sorted[0].name, "file2.txt");
//! ```

use std::cmp::Ordering;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::time::SystemTime;

use crate::error::{Result, classify_io};

/// What a directory entry is, *without* following symlinks.
///
/// Classification always comes from [`std::fs::symlink_metadata`] so a symlink
/// is always [`Symlink`](Self::Symlink), never silently masquerading as a
/// [`File`](Self::File). A UI can then show a "link" badge and offer
/// "open target" / "delete link only" actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EntryKind {
    /// A real directory.
    Directory,
    /// A real file (regular file, or something file-like such as a fifo).
    File,
    /// A symbolic link. The target was *not* resolved.
    Symlink,
    /// Anything else: sockets, block/char devices, FIFOs, ...
    Other,
}

impl EntryKind {
    /// Classifies a `symlink_metadata` result.
    #[must_use]
    pub fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        if metadata.file_type().is_symlink() {
            Self::Symlink
        } else if metadata.is_dir() {
            Self::Directory
        } else if metadata.is_file() {
            Self::File
        } else {
            Self::Other
        }
    }

    /// Stable lowercase name, handy for UI labels and `serde`-ish round-trips.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Directory => "directory",
            Self::File => "file",
            Self::Symlink => "symlink",
            Self::Other => "other",
        }
    }

    /// True only for a real directory (not a symlink pointing at one).
    #[must_use]
    pub fn is_directory(self) -> bool {
        matches!(self, Self::Directory)
    }

    /// True for a symlink, regardless of what it points to.
    #[must_use]
    pub fn is_symlink(self) -> bool {
        matches!(self, Self::Symlink)
    }
}

impl std::fmt::Display for EntryKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A single row in a file manager listing.
///
/// `size` is the length reported by `lstat`. For a symlink that is the length
/// of the target *path string*, which is rarely what a user wants to see — the
/// UI can show it muted, or resolve the link lazily on demand. `None` means
/// "unknown/unreadable", which the UI renders as `-` instead of `0`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    /// File name only (not the full path). Lossy-converted for non-UTF-8 names.
    pub name: String,
    /// Absolute or relative path this entry was read from.
    pub path: PathBuf,
    /// Classification from `symlink_metadata`.
    pub kind: EntryKind,
    /// Length in bytes, or `None` if the OS would not tell us.
    pub size: Option<u64>,
    /// Last modification time, or `None` on filesystems that do not track it.
    pub modified: Option<SystemTime>,
    /// Whether this entry counts as hidden (see [`is_hidden`]).
    pub hidden: bool,
    /// For a symlink, whether its **target** is a directory.
    ///
    /// `Some(true)`/`Some(false)` for a symlink the scanner could resolve;
    /// `None` for a non-symlink (the question does not apply) *and* for a
    /// symlink whose target could not be stat'd — a dangling link, a permission
    /// error, an `ELOOP` chain. The three cases are deliberately not collapsed:
    /// "not a link" and "a link I could not follow" are different facts, and a
    /// UI that treats both as "don't descend" silently swallows broken links.
    ///
    /// # Why this field exists
    ///
    /// A file manager must decide, on `Enter`, whether the focused row is a
    /// directory. For a symlink the only way to know is to follow it, and doing
    /// that in the UI thread puts a `stat(2)` in the frame loop. Resolving it
    /// here moves the work onto the scanner's thread, where it belongs, and
    /// makes the answer free at render time.
    pub is_dir_target: Option<bool>,
}

impl FileEntry {
    /// Reads one entry from the filesystem, **not** following symlinks.
    ///
    /// # Errors
    ///
    /// Returns the classified [`KestrelError`](crate::KestrelError) for the
    /// path if `lstat` fails
    /// (missing entry, permission denied, `ELOOP`, `ENAMETOOLONG`, ...).
    ///
    /// ```
    /// use kestrel_fs::model::EntryKind;
    /// use kestrel_fs::model::FileEntry;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let entry = FileEntry::from_path(".")?;
    /// assert!(entry.path.is_absolute() || !entry.path.as_os_str().is_empty());
    /// let _ = entry.kind == EntryKind::Directory;
    /// # Ok(())
    /// # }
    /// ```
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        // `symlink_metadata` == `lstat`: describes the link itself.
        let metadata = std::fs::symlink_metadata(path).map_err(|e| classify_io(path, e))?;
        Ok(Self::from_metadata(path, &metadata))
    }

    /// Builds an entry from metadata that was fetched elsewhere (e.g. by a
    /// recursive walk that already had it in hand).
    ///
    /// `is_dir_target` is left `None`: this constructor has not followed the
    /// link, and guessing would be worse than saying "unknown". The scanner
    /// fills it in with [`FileEntry::resolve_link_target`].
    #[must_use]
    pub fn from_metadata(path: &Path, metadata: &std::fs::Metadata) -> Self {
        let kind = EntryKind::from_metadata(metadata);
        let name = entry_name(path);
        Self {
            name,
            path: path.to_path_buf(),
            kind,
            size: Some(metadata.len()),
            modified: metadata.modified().ok(),
            hidden: is_hidden_path(path),
            is_dir_target: None,
        }
    }

    /// Follows a symlink and records whether its target is a directory.
    ///
    /// A no-op for anything that is not a symlink. Returns `self` unchanged
    /// when the target cannot be stat'd, leaving `is_dir_target` as `None` —
    /// the caller has already been told the link exists, and a failure to
    /// resolve it is not a reason to discard the row.
    ///
    /// **This performs I/O.** Call it from a worker thread, never from a render
    /// callback.
    #[must_use]
    pub fn resolve_link_target(mut self) -> Self {
        if self.kind != EntryKind::Symlink {
            return self;
        }
        // `metadata` follows the link; `symlink_metadata` would not, and would
        // just re-report the link itself.
        self.is_dir_target = std::fs::metadata(&self.path).ok().map(|md| md.is_dir());
        self
    }

    /// `true` when `Enter` on this row should descend into a directory.
    ///
    /// The UI's whole answer to "is this descendable", with no I/O: a real
    /// directory is always descendable, and a symlink is only if the scanner
    /// proved its target is one. A symlink whose target could not be resolved
    /// is not descendable, which is the correct outcome for a broken link.
    #[must_use]
    pub fn is_descendable(&self) -> bool {
        match self.kind {
            EntryKind::Directory => true,
            EntryKind::Symlink => self.is_dir_target == Some(true),
            EntryKind::File | EntryKind::Other => false,
        }
    }

    /// The file name as raw OS bytes, for callers that must not lossy-convert.
    #[must_use]
    pub fn name_os(&self) -> &OsStr {
        self.path
            .file_name()
            .unwrap_or_else(|| self.path.as_os_str())
    }
}

/// The name used for a listing row.
///
/// Root paths (`/`, `C:\`) have no `file_name()`, so they fall back to the
/// whole path rendered lossily.
#[must_use]
pub fn entry_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.to_string_lossy().into_owned(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// Cross-platform "is this entry hidden" test.
///
/// * **Unix (Linux, macOS, BSD)** — a leading `.`. macOS additionally hides
///   items with the `UF_HIDDEN` flag, which needs an extra `stat`; we do not
///   perform it because the extra syscall per row is measurable on large
///   listings, and the `.` rule is the one users actually rely on.
/// * **Windows** — the `FILE_ATTRIBUTE_HIDDEN` bit, read from the metadata we
///   already fetched, so this is free.
/// * **Everything else** — the leading-dot rule.
///
/// The OS may *also* apply its own hiding rules (e.g. GNOME's `.hidden`
/// file); those are environment policy rather than filesystem facts, and are
/// the GUI layer's business.
///
/// ```
/// use kestrel_fs::model::is_hidden;
///
/// assert!(is_hidden(".config"));
/// assert!(!is_hidden("Documents"));
/// ```
#[must_use]
pub fn is_hidden(name: impl AsRef<Path>) -> bool {
    is_hidden_path(name.as_ref())
}

/// Path-based form of [`is_hidden`]; only the final component is considered.
///
/// ```
/// use kestrel_fs::model::is_hidden_path;
/// use std::path::Path;
///
/// assert!(is_hidden_path(Path::new("/home/achraf/.bashrc")));
/// assert!(!is_hidden_path(Path::new("/home/achraf/Documents")));
/// // Only the final component matters: everything under a dot-directory is
/// // reached through that directory, it is not itself hidden.
/// assert!(!is_hidden_path(Path::new("/home/achraf/.config/nvim/init.lua")));
/// ```
#[must_use]
pub fn is_hidden_path(path: &Path) -> bool {
    // The component that actually carries the name. For "/" this is "/",
    // which is not hidden, and we return early below.
    let name = path
        .file_name()
        .map_or_else(|| path.as_os_str(), OsStr::new);

    #[cfg(windows)]
    {
        // Cheap: read the attribute bit from metadata we may already have.
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        if let Ok(md) = path.symlink_metadata() {
            if md.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0 {
                return true;
            }
        }
        return dot_leading(name);
    }

    #[cfg(not(windows))]
    {
        dot_leading(name)
    }
}

#[cfg_attr(windows, allow(dead_code))]
fn dot_leading(name: &OsStr) -> bool {
    name.to_string_lossy().starts_with('.')
}

#[cfg(windows)]
use std::os::windows::fs::MetadataExt as _;

/// Which column the user sorted by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SortKey {
    /// File name, compared case-insensitively and "naturally" (see
    /// [`natural_cmp`]), so `file2` sorts before `file10`.
    #[default]
    Name,
    /// Size in bytes; entries with an unknown size sort last.
    Size,
    /// Last modification time; unknown times sort last.
    Modified,
    /// Entry kind (directories, then symlinks, then files, then other).
    Kind,
}

/// A complete ordering for a listing.
///
/// Kept as data rather than a bare closure so the UI can round-trip the
/// user's choice through settings and show the arrow in the right column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortSpec {
    /// Column to sort by.
    pub key: SortKey,
    /// Ascending (true) or descending (false).
    pub ascending: bool,
    /// Keep directories above files regardless of the chosen column.
    pub dirs_first: bool,
}

impl Default for SortSpec {
    fn default() -> Self {
        Self {
            key: SortKey::Name,
            ascending: true,
            dirs_first: true,
        }
    }
}

impl SortSpec {
    /// A spec that sorts by `key`, descending, with directories first.
    #[must_use]
    pub fn by_descending(key: SortKey) -> Self {
        Self {
            key,
            ascending: false,
            ..Self::default()
        }
    }

    /// Returns a comparator usable with `Iterator::sort_by` / `Vec::sort_by`.
    ///
    /// Total and deterministic: equal rows fall back to a case-insensitive name
    /// comparison and then to the raw path, so listings never jitter between
    /// two passes.
    #[must_use = "the comparator is meant to be handed straight to sort_by"]
    pub fn comparator(self) -> impl Fn(&FileEntry, &FileEntry) -> Ordering {
        move |a: &FileEntry, b: &FileEntry| {
            if self.dirs_first {
                let ga = kind_group(a.kind);
                let gb = kind_group(b.kind);
                if ga != gb {
                    return ga.cmp(&gb);
                }
            }
            let primary = match self.key {
                SortKey::Name => natural_cmp(&a.name, &b.name),
                SortKey::Size => cmp_optional(a.size, b.size),
                SortKey::Modified => cmp_optional(a.modified, b.modified),
                SortKey::Kind => a.kind.cmp(&b.kind),
            };
            let primary = if self.ascending {
                primary
            } else {
                primary.reverse()
            };
            if primary != Ordering::Equal {
                return primary;
            }
            natural_cmp(&a.name, &b.name).then_with(|| a.path.cmp(&b.path))
        }
    }
}

/// Directories first, then symlinks, then files, then everything else.
fn kind_group(kind: EntryKind) -> u8 {
    match kind {
        EntryKind::Directory => 0,
        EntryKind::Symlink => 1,
        EntryKind::File => 2,
        EntryKind::Other => 3,
    }
}

/// Orders `Some` before `None` and then by value.
fn cmp_optional<T: Ord>(a: Option<T>, b: Option<T>) -> Ordering {
    match (a, b) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// Case-insensitive **natural** string comparison.
///
/// Runs of ASCII digits compare as numbers (leading zeros ignored), everything
/// else compares by lowercased character, so `file2` < `file10` and
/// `img9.png` < `img10.png`. Strings that are equal under those rules fall back
/// to byte order, which makes the comparator *total* — useful when the UI needs
/// a stable sort. Implemented here rather than pulled in from a crate so the
/// ordering is exactly the behaviour the file list depends on, and so the crate
/// keeps two runtime dependencies in total.
///
/// ```
/// use kestrel_fs::model::natural_cmp;
/// use std::cmp::Ordering;
///
/// assert_eq!(natural_cmp("file2", "file10"), Ordering::Less);
/// // Case is ignored for the decision, with byte order as the tie-break.
/// assert_eq!(natural_cmp("file2", "File2"), Ordering::Greater);
/// assert_eq!(natural_cmp("a", "b"), Ordering::Less);
/// ```
#[must_use]
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut ai = a.chars();
    let mut bi = b.chars();

    loop {
        match (ai.next(), bi.next()) {
            (None, None) => break,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) => {
                if ca.is_ascii_digit() && cb.is_ascii_digit() {
                    // Restart both iterators at the digit run that started at
                    // this position.
                    let num_a = take_number(&mut ai, ca);
                    let num_b = take_number(&mut bi, cb);
                    let ord = compare_numbers(&num_a, &num_b);
                    if ord != Ordering::Equal {
                        return ord;
                    }
                } else {
                    let la = lower(ca);
                    let lb = lower(cb);
                    if la != lb {
                        return la.cmp(&lb);
                    }
                }
            }
        }
    }
    // Equal ignoring case and digit structure: keep a stable, total order.
    a.cmp(b)
}

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Consumes the rest of a digit run (the first digit having been consumed by
/// the caller) and returns it with leading zeros stripped, so `007` and `7`
/// compare as the same number.
fn take_number(iter: &mut impl Iterator<Item = char>, first: char) -> String {
    let mut out = String::new();
    let mut started = first != '0';
    if started {
        out.push(first);
    }
    for c in iter.by_ref() {
        if !c.is_ascii_digit() {
            break;
        }
        if c != '0' {
            started = true;
        }
        if started {
            out.push(c);
        }
    }
    out
}

/// Compares two zero-stripped digit runs by (length, lexicographic).
fn compare_numbers(a: &str, b: &str) -> Ordering {
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

/// A cheap, cloneable, thread-safe cancel flag.
///
/// Used to interrupt a scan of `/`, a recursive size computation, or a
/// multi-gigabyte copy that the user has navigated away from. Cloning shares
/// the same flag, so the worker holds a clone and the UI holds the original.
///
/// ```
/// use kestrel_fs::model::CancellationToken;
///
/// let token = CancellationToken::new();
/// let worker_token = token.clone();
/// assert!(!worker_token.is_cancelled());
/// token.cancel();
/// assert!(worker_token.is_cancelled());
///
/// // A child inherits cancellation from its parent (no thread is needed to
/// // poll a parent, so a background scan can be stopped from anywhere).
/// let child = token.child();
/// assert!(child.is_cancelled());
/// ```
#[derive(Clone, Debug, Default)]
pub struct CancellationToken {
    inner: Arc<TokenInner>,
}

#[derive(Debug, Default)]
struct TokenInner {
    flag: AtomicBool,
    /// Weak so a child never keeps its parent alive.
    parent: std::sync::Mutex<Option<std::sync::Weak<TokenInner>>>,
}

impl CancellationToken {
    /// Creates a fresh, uncancelled token.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation. Idempotent, and safe to call from any thread.
    pub fn cancel(&self) {
        self.inner.flag.store(true, AtomicOrdering::SeqCst);
    }

    /// `true` once [`cancel`](Self::cancel) has been called on this token or on
    /// any of its ancestors.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        if self.inner.flag.load(AtomicOrdering::SeqCst) {
            return true;
        }
        // Poisoning can only happen if a thread panicked while holding this
        // mutex; the stored data is a single `Weak` we do not mutate, so
        // recovering the guard is correct and keeps the API panic-free.
        let guard = match self.inner.parent.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        match guard.as_ref().and_then(std::sync::Weak::upgrade) {
            Some(parent) => parent.flag.load(AtomicOrdering::SeqCst),
            None => false,
        }
    }

    /// A token that is cancelled when this one is, and can be cancelled
    /// independently of it.
    #[must_use]
    pub fn child(&self) -> Self {
        let child = Self::new();
        {
            // The link lives on the child; the parent only holds a `Weak` so a
            // short-lived child cannot keep a long-lived parent alive.
            let mut guard = match child.inner.parent.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            *guard = Some(Arc::downgrade(&self.inner));
        }
        child
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn entry(name: &str, kind: EntryKind, size: Option<u64>) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            path: PathBuf::from(name),
            kind,
            size,
            modified: None,
            hidden: is_hidden(name),
            is_dir_target: None,
        }
    }

    #[test]
    fn natural_order_beats_lexicographic() {
        let mut names = vec!["file10", "file2", "file1", "File20", "file3"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, vec!["file1", "file2", "file3", "file10", "File20"]);
    }

    #[test]
    fn natural_order_handles_leading_zeros_and_prefixes() {
        // "a007" and "a7" are the same number, so the byte-order tie-break
        // decides: '0' sorts before '7'.
        assert_eq!(natural_cmp("a007", "a7"), Ordering::Less);
        assert_eq!(natural_cmp("a7", "a007"), Ordering::Greater);
        assert_eq!(natural_cmp("a07", "a7b"), Ordering::Less);
        assert_eq!(natural_cmp("", "a"), Ordering::Less);
        assert_eq!(natural_cmp("a", "a"), Ordering::Equal);
        assert_eq!(natural_cmp("a", "b"), Ordering::Less);
    }

    #[test]
    fn natural_order_is_total() {
        // Any two strings must compare one of exactly three ways, never panic
        // and never claim equality while also comparing Less on a tie-break.
        for a in ["", "0", "1", "10", "x1", "x1y", "X2", "é", "9é"] {
            for b in ["", "0", "1", "10", "x1", "x1y", "X2", "é", "9é"] {
                assert!(matches!(
                    natural_cmp(a, b),
                    Ordering::Less | Ordering::Equal | Ordering::Greater
                ));
            }
        }
    }

    #[test]
    fn dirs_sort_first_regardless_of_key() {
        let mut v = [
            entry("zeta.txt", EntryKind::File, Some(1)),
            entry("alpha", EntryKind::Directory, Some(999_999)),
        ];
        v.sort_by(SortSpec::default().comparator());
        assert_eq!(v[0].name, "alpha");
    }

    #[test]
    fn unknown_sizes_and_times_sort_last() {
        let mut v = [
            entry("a", EntryKind::File, None),
            entry("b", EntryKind::File, Some(1)),
        ];
        v.sort_by(
            SortSpec {
                key: SortKey::Size,
                ascending: true,
                dirs_first: false,
            }
            .comparator(),
        );
        assert_eq!(v[0].name, "b");

        let mut d = [
            entry("a", EntryKind::File, None),
            entry("b", EntryKind::File, None),
        ];
        d[0].modified = None;
        d[1].modified = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1));
        d.sort_by(
            SortSpec {
                key: SortKey::Modified,
                ascending: true,
                dirs_first: false,
            }
            .comparator(),
        );
        assert_eq!(d[0].name, "b");
    }

    #[test]
    fn descending_reverses_the_primary_key() {
        let mut v = [
            entry("file2", EntryKind::File, None),
            entry("file10", EntryKind::File, None),
        ];
        v.sort_by(SortSpec::by_descending(SortKey::Name).comparator());
        assert_eq!(v[0].name, "file10");
    }

    #[test]
    fn kind_sort_and_group_order() {
        assert!(kind_group(EntryKind::Directory) < kind_group(EntryKind::Symlink));
        assert!(kind_group(EntryKind::Symlink) < kind_group(EntryKind::File));
        assert!(kind_group(EntryKind::File) < kind_group(EntryKind::Other));
    }

    #[test]
    fn hidden_detection() {
        assert!(is_hidden(".git"));
        assert!(is_hidden_path(Path::new("/a/b/.hidden")));
        assert!(!is_hidden_path(Path::new("/a/b/.hidden/visible")));
        assert!(!is_hidden_path(Path::new("/a/b/visible")));
        assert!(!is_hidden_path(Path::new("/")));
    }

    /// The exact top-of-listing sequence a live `$HOME` scan produced, pinned as
    /// a regression test.
    ///
    /// It was reported as a mis-sort — `.android` appearing to come "before
    /// `.astrobot`". It does, and it should: comparing the second character,
    /// `n` < `s`. The test exists to record that this ordering is *intended*,
    /// so a future change that "fixes" it in the other direction is caught.
    ///
    /// It also pins the harder properties `sort_by` silently depends on:
    /// antisymmetry, transitivity and idempotence. A comparator that breaks
    /// transitivity does not mis-sort — it panics inside `sort_by`, or emits an
    /// order that changes between two runs of the same directory, which is the
    /// worst possible failure for a file list.
    #[test]
    fn home_dotfile_sequence_sorts_as_observed() {
        const OBSERVED: [&str; 24] = [
            ".adal",
            ".agents",
            ".aider-desk",
            ".android",
            ".astrobot",
            ".autohand",
            ".cache",
            ".cargo",
            ".claude",
            ".cline",
            ".codeartsdoer",
            ".codebuddy",
            ".codeium",
            ".codemaker",
            ".codestudio",
            ".codex",
            ".commandcode",
            ".config",
            ".config-krince",
            ".continue",
            ".dart-tool",
            ".dartServer",
            ".dbus",
            ".elementary",
        ];

        // 1. The sequence is already in ascending order.
        for w in OBSERVED.windows(2) {
            assert_ne!(
                natural_cmp(w[0], w[1]),
                Ordering::Greater,
                "{:?} must not sort before {:?}",
                w[0],
                w[1]
            );
        }

        // 2. Sorting it is a no-op, through the engine's own `SortSpec`.
        let mut entries: Vec<FileEntry> = OBSERVED
            .iter()
            .map(|n| FileEntry {
                name: (*n).to_string(),
                path: PathBuf::from(*n),
                kind: EntryKind::Directory,
                size: Some(4096),
                modified: None,
                hidden: true,
                is_dir_target: None,
            })
            .collect();
        entries.sort_by(SortSpec::default().comparator());
        let got: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(got, OBSERVED, "SortSpec reordered the listing");

        // 3. Antisymmetry: cmp(a,b) == cmp(b,a).reverse() unless equal.
        for a in OBSERVED {
            for b in OBSERVED {
                if natural_cmp(a, b) != Ordering::Equal {
                    assert_eq!(
                        natural_cmp(a, b),
                        natural_cmp(b, a).reverse(),
                        "not antisymmetric: {a:?} / {b:?}"
                    );
                }
            }
        }

        // 4. Transitivity over a corpus of dotfiles, bare names, digit runs and
        //    mixed case — the inputs a real home directory actually produces.
        const CORPUS: [&str; 24] = [
            ".adal",
            ".agents",
            ".android",
            ".astrobot",
            ".autohand",
            ".dart-tool",
            ".dartServer",
            ".config-krince",
            "file1",
            "file2",
            "file3",
            "file10",
            "File20",
            "a007",
            "a7",
            "a07",
            "a7b",
            "a",
            "b",
            "0",
            "00",
            "000",
            "1",
            "10",
        ];
        for a in CORPUS {
            for b in CORPUS {
                for c in CORPUS {
                    if natural_cmp(a, b) == Ordering::Less && natural_cmp(b, c) == Ordering::Less {
                        assert_eq!(
                            natural_cmp(a, c),
                            Ordering::Less,
                            "transitivity broken: {a:?} < {b:?} < {c:?}"
                        );
                    }
                }
            }
        }

        // 5. Idempotence: sorting twice changes nothing.
        let mut twice: Vec<&str> = CORPUS.to_vec();
        twice.sort_by(|a, b| natural_cmp(a, b));
        let once = twice.clone();
        twice.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(once, twice, "sort is not idempotent");
    }

    #[test]
    fn a_directory_is_descendable_without_any_io() {
        let dir = FileEntry {
            name: "src".into(),
            path: PathBuf::from("src"),
            kind: EntryKind::Directory,
            size: Some(4096),
            modified: None,
            hidden: false,
            is_dir_target: None,
        };
        assert!(dir.is_descendable());
        assert_eq!(
            dir.is_dir_target, None,
            "a real dir never needs a link target"
        );
    }

    #[test]
    fn a_symlink_is_only_descendable_when_its_target_was_resolved() {
        let base = FileEntry {
            name: "link".into(),
            path: PathBuf::from("/definitely/not/here/link"),
            kind: EntryKind::Symlink,
            size: Some(4),
            modified: None,
            hidden: false,
            is_dir_target: None,
        };
        // Unresolved: not descendable, and `resolve_link_target` cannot say
        // more because the target does not exist.
        assert!(!base.is_descendable());
        let still_unknown = base.clone().resolve_link_target();
        assert_eq!(still_unknown.is_dir_target, None);
        assert!(!still_unknown.is_descendable());

        let mut to_dir = base.clone();
        to_dir.is_dir_target = Some(true);
        assert!(to_dir.is_descendable());
        let mut to_file = base;
        to_file.is_dir_target = Some(false);
        assert!(!to_file.is_descendable());
    }

    #[test]
    fn entry_name_falls_back_for_roots() {
        assert_eq!(entry_name(Path::new("/")), "/");
        assert_eq!(entry_name(Path::new("/tmp/x")), "x");
    }

    #[test]
    fn cancellation_propagates_to_children() {
        let parent = CancellationToken::new();
        let child = parent.child();
        assert!(!child.is_cancelled());
        parent.cancel();
        assert!(child.is_cancelled());
    }
}
