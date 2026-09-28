//! Directory traversal with incremental, cancellable delivery.
//!
//! # Design
//!
//! A file manager has two very different traversal needs:
//!
//! * **Listing** one directory (what the user is looking at right now). Cheap,
//!   non-recursive, sorted, and it must appear on screen immediately.
//! * **Walking** a tree (size computation, search). Potentially millions of
//!   entries, and the user will change their mind.
//!
//! Both are expressed by the same engine, [`scan`], which pushes
//! [`ScanEvent`]s into a caller-supplied sink as it goes. Nothing is
//! collected internally "so we can sort it" across directory boundaries: each
//! directory's entries are sorted and flushed, so the UI paints within
//! milliseconds even on a cold NFS mount.
//!
//! # Loop safety
//!
//! Recursive traversal that follows symlinks can revisit a directory forever
//! (`a/b/link -> ../..`). Two independent brakes stop that:
//!
//! 1. every directory we descend into is [`std::fs::canonicalize`]d and pushed
//!    into a `HashSet`; a second visit is reported as
//!    [`KestrelError::LoopDetected`] and not descended again;
//! 2. a hard [`ScanOptions::max_depth`] ceiling.
//!
//! By default symlinks are **not** followed at all, so a plain listing can
//! never loop regardless.
//!
//! # Example: list a directory on a worker thread
//!
//! ```no_run
//! use kestrel_fs::scan::{ScanEvent, ScanOptions, start};
//! use kestrel_fs::model::SortSpec;
//! use std::time::Duration;
//!
//! let handle = start(
//!     "/home/achraf",
//!     ScanOptions {
//!         recursive: false,
//!         show_hidden: true,
//!         sort: Some(SortSpec::default()),
//!         ..ScanOptions::default()
//!     },
//! )
//! .expect("spawn worker");
//!
//! // The UI thread only ever touches the channel, never the filesystem.
//! while let Ok(event) = handle.recv_timeout(Duration::from_millis(50)) {
//!     match event {
//!         ScanEvent::Entry(e) => println!("{} ({})", e.name, e.kind),
//!         ScanEvent::Error(err) => eprintln!("skipped {}: {err}", err.path.display()),
//!         ScanEvent::BatchEnd => println!("--- batch flushed ---"),
//!         ScanEvent::Complete { total } => { println!("{total} entries"); break }
//!     }
//! }
//! ```

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::Duration;

use crate::error::{KestrelError, Result, ScanError, classify_io};
use crate::model::{CancellationToken, FileEntry, SortSpec};

/// Default depth ceiling for recursive traversal.
///
/// Deep enough for any sane tree, shallow enough that a pathological one still
/// terminates quickly.
pub const DEFAULT_MAX_DEPTH: usize = 16;

/// How many entries are emitted between [`ScanEvent::BatchEnd`] markers when
/// scanning recursively.
///
/// Batches are only emitted for the background handle, where the consumer is a
/// UI thread that wants a "grow the list" hint; the synchronous
/// [`scan`] entry point passes its own sink and never inserts one.
pub const DEFAULT_BATCH_SIZE: usize = 256;

/// One item of scan output.
#[derive(Debug)]
pub enum ScanEvent {
    /// A discovered entry. Delivered as soon as the entry's parent directory
    /// has been read.
    Entry(FileEntry),
    /// End of a delivery batch; a good place for the UI to stop and repaint.
    BatchEnd,
    /// A path that could not be read. The scan continued past it.
    Error(ScanError),
    /// Traversal finished. `total` counts successfully delivered entries.
    Complete {
        /// Number of [`Entry`](Self::Entry) events emitted.
        total: usize,
    },
}

/// Knobs for a traversal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanOptions {
    /// Descend into subdirectories. The file list wants `false`; size/search
    /// want `true`.
    pub recursive: bool,
    /// Descend through symlinked directories. Off by default: it is the only
    /// way a traversal can revisit a path.
    pub follow_symlinks: bool,
    /// Depth ceiling for [`recursive`](Self::recursive) scans, counted from the
    /// scan root (which is depth 0).
    pub max_depth: usize,
    /// Emit entries whose name starts with `.` (and, on Windows, those with
    /// the hidden attribute).
    pub show_hidden: bool,
    /// Sort each directory before emitting it. `None` emits in `read_dir`
    /// order, which is faster.
    pub sort: Option<SortSpec>,
    /// Entries per [`ScanEvent::BatchEnd`] (background scans only).
    pub batch_size: usize,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            recursive: false,
            follow_symlinks: false,
            max_depth: DEFAULT_MAX_DEPTH,
            show_hidden: true,
            sort: Some(SortSpec::default()),
            batch_size: DEFAULT_BATCH_SIZE,
        }
    }
}

impl ScanOptions {
    /// Options for a plain, sorted, single-level listing.
    #[must_use]
    pub fn listing() -> Self {
        Self::default()
    }

    /// Options for a recursive walk (used by size computation and search).
    #[must_use]
    pub fn recursive() -> Self {
        Self {
            recursive: true,
            ..Self::default()
        }
    }

    /// Options for a fast walk with no sorting and hidden files skipped.
    #[must_use]
    pub fn unsorted() -> Self {
        Self {
            sort: None,
            show_hidden: false,
            ..Self::default()
        }
    }
}

/// Totals returned by the synchronous [`scan`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScanSummary {
    /// Entries successfully emitted.
    pub total: usize,
    /// Paths that failed and were reported as [`ScanEvent::Error`].
    pub errors: usize,
    /// `true` if a [`CancellationToken`] stopped the walk early.
    pub cancelled: bool,
}

/// A background scan: a cancel handle plus the receiving end of its event
/// stream.
///
/// Dropping the handle does not stop the worker; call [`cancel`](Self::cancel)
/// (or [`finish`](Self::finish), which also joins) when the user navigates away.
#[derive(Debug)]
pub struct ScanHandle {
    cancel: CancellationToken,
    receiver: Receiver<ScanEvent>,
    worker: Option<std::thread::JoinHandle<()>>,
    summary: Arc<std::sync::Mutex<Option<ScanSummary>>>,
}

impl ScanHandle {
    /// Requests cancellation. Safe to call from any thread, repeatedly.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// The underlying token, e.g. to pass to a nested size computation.
    #[must_use]
    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancel
    }

    /// Blocks for the next event. Returns `Err` once the worker has finished
    /// and the channel is drained.
    pub fn recv(&self) -> std::result::Result<ScanEvent, std::sync::mpsc::RecvError> {
        self.receiver.recv()
    }

    /// Blocks for at most `timeout`; lets a UI frame end without stalling.
    pub fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> std::result::Result<ScanEvent, RecvTimeoutErr> {
        self.receiver.recv_timeout(timeout)
    }

    /// Non-blocking read, for draining a batch at the end of a frame.
    pub fn try_recv(&self) -> std::result::Result<ScanEvent, std::sync::mpsc::TryRecvError> {
        self.receiver.try_recv()
    }

    /// Drains every event already queued, calling `f` for each.
    ///
    /// The pattern a UI uses once per frame: take what is ready, keep the rest
    /// for the next one.
    pub fn drain_into(&self, mut f: impl FnMut(ScanEvent)) {
        while let Ok(event) = self.receiver.try_recv() {
            f(event);
        }
    }

    /// Cancels and joins the worker thread, returning the summary.
    ///
    /// Takes `self` so it cannot race with a caller still reading events. Any
    /// events still queued in the channel are discarded.
    #[must_use]
    pub fn finish(mut self) -> Option<ScanSummary> {
        self.cancel.cancel();
        if let Some(worker) = self.worker.take() {
            // A panicking worker is a bug, but it must not take the UI down.
            let _ = worker.join();
        }
        self.summary()
    }

    /// The summary of a finished scan, or `None` while it is still running.
    #[must_use]
    pub fn summary(&self) -> Option<ScanSummary> {
        match self.summary.lock() {
            Ok(slot) => *slot,
            Err(poisoned) => *poisoned.into_inner(),
        }
    }
}

impl Drop for ScanHandle {
    fn drop(&mut self) {
        // Detach the worker: the sender lives in the thread, so the channel
        // closes on its own. We only make sure the flag is set so a long walk
        // stops promptly.
        self.cancel.cancel();
    }
}

/// Alias so `handle.recv_timeout(..)` reads well in UI code.
pub type RecvTimeoutErr = RecvTimeoutError;

/// Starts a scan on a background thread.
///
/// The returned handle is the *only* thing the UI thread should touch; the
/// filesystem work happens on the worker and results arrive as
/// [`ScanEvent`]s.
///
/// # Errors
///
/// Only fails if the thread cannot be spawned (an OS resource failure).
///
/// ```
/// use kestrel_fs::scan::{ScanEvent, ScanOptions, start};
///
/// let handle = start(".", ScanOptions::listing()).expect("spawn worker");
/// loop {
///     match handle.recv() {
///         Ok(ScanEvent::Complete { total }) => { assert!(total > 0); break }
///         Ok(_) => {}
///         Err(_) => break,
///     }
/// }
/// ```
pub fn start(root: impl AsRef<Path>, options: ScanOptions) -> std::io::Result<ScanHandle> {
    let root = root.as_ref().to_path_buf();
    let cancel = CancellationToken::new();
    let (sender, receiver) = channel::<ScanEvent>();
    let summary_slot: Arc<std::sync::Mutex<Option<ScanSummary>>> =
        Arc::new(std::sync::Mutex::new(None));
    let worker_slot = Arc::clone(&summary_slot);
    let worker_cancel = cancel.clone();
    let batch_size = options.batch_size.max(1);

    let worker = std::thread::Builder::new()
        .name("kestrel-scan".to_string())
        .spawn(move || {
            // The sink forwards events and injects a `BatchEnd` every
            // `batch_size` entries so the UI can repaint in slices.
            let mut in_batch = 0usize;
            let mut sink = |event: ScanEvent| {
                let is_entry = matches!(event, ScanEvent::Entry(_));
                if is_entry {
                    in_batch += 1;
                }
                let delivered = sender.send(event).is_ok();
                if is_entry && in_batch >= batch_size {
                    in_batch = 0;
                    let _ = sender.send(ScanEvent::BatchEnd);
                }
                if !delivered {
                    // Nobody is listening any more; stop walking the disk.
                    worker_cancel.cancel();
                }
                // The sink contract is "`false` stops the walk", so a
                // successfully delivered event means "keep going".
                delivered
            };

            let summary = scan_inner(&root, &options, &worker_cancel, &mut sink);
            if in_batch > 0 {
                let _ = sender.send(ScanEvent::BatchEnd);
            }
            let total = match &summary {
                Ok(s) => s.total,
                // A root we could not read: still tell the consumer we are done.
                Err(_) => 0,
            };
            let _ = sender.send(ScanEvent::Complete { total });
            let summary = summary.unwrap_or_default();
            // Poisoning here would only mean another thread panicked while
            // storing an `Option<ScanSummary>`; recovering the value is correct
            // and keeps this path panic-free.
            match worker_slot.lock() {
                Ok(mut slot) => *slot = Some(summary),
                Err(poisoned) => *poisoned.into_inner() = Some(summary),
            }
        })?;

    Ok(ScanHandle {
        cancel,
        receiver,
        worker: Some(worker),
        summary: summary_slot,
    })
}

/// Runs a scan on the current thread, streaming events to `sink`.
///
/// This is the primitive: [`start`] is a thin wrapper that runs it on a worker
/// and forwards the events to a channel. Tests and the size/search code use it
/// directly because it needs no threads.
///
/// `sink` returns `false` to stop the traversal (an alternative to
/// cancelling), `true` to continue. Traversal stops promptly either way — the
/// flag is checked between entries and between directories.
///
/// # Errors
///
/// Returns [`KestrelError`] only if the *root* itself cannot be read (that is a
/// genuine failure). Everything else is reported through
/// [`ScanEvent::Error`] and traversal continues.
///
/// ```
/// use kestrel_fs::model::CancellationToken;
/// use kestrel_fs::scan::{ScanEvent, ScanOptions, scan};
///
/// let mut names = Vec::new();
/// let mut sink = |event: ScanEvent| {
///     if let ScanEvent::Entry(entry) = event {
///         names.push(entry.name);
///     }
///     true // keep going
/// };
/// // "scan" is the synchronous primitive: no threads, no channels, easy to test.
/// let summary = scan(".", ScanOptions::listing(), &CancellationToken::new(), &mut sink)
///     .expect("scan .");
/// assert_eq!(summary.total, names.len());
/// ```
pub fn scan(
    root: impl AsRef<Path>,
    options: ScanOptions,
    cancel: &CancellationToken,
    mut sink: impl FnMut(ScanEvent) -> bool,
) -> Result<ScanSummary> {
    scan_inner(root.as_ref(), &options, cancel, &mut sink)
}

fn scan_inner(
    root: &Path,
    options: &ScanOptions,
    cancel: &CancellationToken,
    sink: &mut impl FnMut(ScanEvent) -> bool,
) -> Result<ScanSummary> {
    let mut summary = ScanSummary::default();

    // Refuse to start on a non-directory. A plain `read_dir` failure here would
    // also work, but the dedicated error reads much better in the UI.
    let root_meta = match std::fs::symlink_metadata(root) {
        Ok(md) => md,
        Err(e) => return Err(classify_io(root, e)),
    };
    if root_meta.file_type().is_symlink() {
        let canonical = root.canonicalize().map_err(|e| classify_io(root, e))?;
        let target_meta = std::fs::metadata(&canonical).map_err(|e| classify_io(&canonical, e))?;
        if !target_meta.is_dir() {
            return Err(classify_io(
                root,
                std::io::Error::new(std::io::ErrorKind::NotADirectory, "not a directory"),
            ));
        }
    } else if !root_meta.is_dir() {
        return Err(classify_io(
            root,
            std::io::Error::new(std::io::ErrorKind::NotADirectory, "not a directory"),
        ));
    }

    // Canonical root, so symlinked roots ("/tmp" -> "/private/tmp" on macOS)
    // compare equal to themselves later.
    let mut visited: HashSet<PathBuf> = HashSet::new();
    match root.canonicalize() {
        Ok(canon) => {
            visited.insert(canon);
        }
        Err(e) => {
            // A root we cannot canonicalise is still worth walking.
            summary.errors += 1;
            if !report(
                sink,
                ScanEvent::Error(ScanError::new(root, classify_io(root, e))),
            ) {
                return Ok(ScanSummary {
                    cancelled: true,
                    ..summary
                });
            }
        }
    }

    let mut queue = Vec::new();
    queue.push((root.to_path_buf(), 0usize));

    while let Some((dir, depth)) = queue.pop() {
        if cancel.is_cancelled() {
            summary.cancelled = true;
            break;
        }
        if !walk_one_dir(
            &dir,
            depth,
            options,
            cancel,
            sink,
            &mut summary,
            &mut queue,
            &mut visited,
        ) {
            summary.cancelled = true;
            break;
        }
    }

    Ok(summary)
}

/// Emits one directory's entries; returns `false` to abort the whole scan.
#[allow(clippy::too_many_arguments)]
fn walk_one_dir(
    dir: &Path,
    depth: usize,
    options: &ScanOptions,
    cancel: &CancellationToken,
    sink: &mut impl FnMut(ScanEvent) -> bool,
    summary: &mut ScanSummary,
    queue: &mut Vec<(PathBuf, usize)>,
    visited: &mut HashSet<PathBuf>,
) -> bool {
    let read_dir = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) => {
            summary.errors += 1;
            return report(
                sink,
                ScanEvent::Error(ScanError::new(dir, classify_io(dir, e))),
            );
        }
    };

    // Collect the directory first so it can be sorted, then flush. Memory is
    // bounded by the widest single directory, which is unavoidable if the user
    // asked for a sorted listing — and the common case (a few hundred rows) is
    // nothing.
    let mut entries: Vec<FileEntry> = Vec::new();
    let mut subdirs: Vec<PathBuf> = Vec::new();

    for result in read_dir {
        if cancel.is_cancelled() {
            return false;
        }
        let entry = match result {
            Ok(e) => e,
            Err(e) => {
                // A single unstat-able name in the directory. Keep going.
                summary.errors += 1;
                if !report(
                    sink,
                    ScanEvent::Error(ScanError::new(dir, classify_io(dir, e))),
                ) {
                    return false;
                }
                continue;
            }
        };
        let path = entry.path();

        // `DirEntry::metadata` uses `lstat` on Unix, so a symlink stays a
        // symlink here. (The doc note about Windows is why we ask for
        // `symlink_metadata` explicitly below rather than trusting it.)
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(md) => md,
            Err(e) => {
                summary.errors += 1;
                if !report(
                    sink,
                    ScanEvent::Error(ScanError::new(&path, classify_io(&path, e))),
                ) {
                    return false;
                }
                continue;
            }
        };

        let is_symlink = metadata.file_type().is_symlink();
        let mut model = FileEntry::from_metadata(&path, &metadata);
        if is_symlink {
            // Resolve the link *here*, on the worker's thread, so the UI never
            // has to `stat` to decide whether `Enter` should descend. Costs one
            // extra `stat` per symlink and nothing at all for the common case.
            model = model.resolve_link_target();
        }
        if model.hidden && !options.show_hidden {
            continue;
        }
        entries.push(model);

        if options.recursive {
            let descend = if is_symlink {
                // Only worth resolving a link when explicitly asked to.
                options.follow_symlinks && metadata_dir(&path)
            } else {
                metadata.is_dir()
            };
            if descend {
                if depth + 1 > options.max_depth {
                    continue;
                }
                match path.canonicalize() {
                    Ok(canon) => {
                        if !visited.insert(canon.clone()) {
                            // Already seen this directory under another name:
                            // a symlink loop. Report and do not descend.
                            summary.errors += 1;
                            if !report(
                                sink,
                                ScanEvent::Error(ScanError::new(
                                    &path,
                                    KestrelError::LoopDetected { path: path.clone() },
                                )),
                            ) {
                                return false;
                            }
                            continue;
                        }
                        subdirs.push(path);
                    }
                    Err(e) => {
                        // A broken symlink target, or a vanished directory.
                        // Both are normal; report and move on.
                        summary.errors += 1;
                        if !report(
                            sink,
                            ScanEvent::Error(ScanError::new(&path, classify_io(&path, e))),
                        ) {
                            return false;
                        }
                    }
                }
            }
        }
    }

    if let Some(spec) = options.sort {
        entries.sort_by(spec.comparator());
    }

    for entry in entries {
        summary.total += 1;
        if !report(sink, ScanEvent::Entry(entry)) {
            return false;
        }
    }

    // Reverse so the depth-first order matches a naive recursive walk rather
    // than reversing it (LIFO stack).
    for sub in subdirs.into_iter().rev() {
        queue.push((sub, depth + 1));
    }
    true
}

/// `symlink_metadata` says dir-or-not; for a symlink we must ask the target.
fn metadata_dir(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|md| md.is_dir())
}

/// Sends an event; returns `false` if the sink asked to stop.
fn report(sink: &mut impl FnMut(ScanEvent) -> bool, event: ScanEvent) -> bool {
    sink(event)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::EntryKind;
    use std::fs;

    /// Collects a whole scan into a vector of events.
    fn collect(root: &Path, options: ScanOptions) -> (Vec<ScanEvent>, ScanSummary) {
        let cancel = CancellationToken::new();
        let mut events = Vec::new();
        let mut sink = |e: ScanEvent| {
            events.push(e);
            true
        };
        let summary = scan(root, options, &cancel, &mut sink).expect("scan");
        (events, summary)
    }

    fn names(events: &[ScanEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|e| match e {
                ScanEvent::Entry(entry) => Some(entry.name.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn non_recursive_listing_emits_children_only() {
        let tmp = tempfile::tempdir().expect("tempdir");
        fs::create_dir(tmp.path().join("sub")).expect("mkdir");
        fs::write(tmp.path().join("a.txt"), b"a").expect("write");
        fs::write(tmp.path().join("sub/b.txt"), b"b").expect("write");

        let (events, summary) = collect(tmp.path(), ScanOptions::listing());
        let mut got = names(&events);
        got.sort();
        assert_eq!(got, vec!["a.txt", "sub"]);
        assert_eq!(summary.total, 2);
        assert!(!summary.cancelled);
    }

    #[test]
    fn recursive_listing_walks_the_whole_tree() {
        let tmp = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(tmp.path().join("x/y/z")).expect("mkdir");
        fs::write(tmp.path().join("x/y/z/deep.txt"), b"deep").expect("write");

        let (events, summary) = collect(tmp.path(), ScanOptions::recursive());
        let got = names(&events);
        assert!(got.contains(&"deep.txt".to_string()));
        assert!(got.contains(&"x".to_string()));
        assert!(got.contains(&"z".to_string()));
        assert_eq!(summary.errors, 0);
    }

    #[test]
    fn hidden_files_can_be_filtered() {
        let tmp = tempfile::tempdir().expect("tempdir");
        fs::write(tmp.path().join(".secret"), b"s").expect("write");
        fs::write(tmp.path().join("open"), b"o").expect("write");

        let options = ScanOptions {
            show_hidden: false,
            ..ScanOptions::listing()
        };
        let (events, _) = collect(tmp.path(), options);
        assert_eq!(names(&events), vec!["open"]);
    }

    #[test]
    fn symlinks_are_never_followed_by_default() {
        let tmp = tempfile::tempdir().expect("tempdir");
        fs::create_dir(tmp.path().join("sub")).expect("mkdir");
        #[cfg(unix)]
        std::os::unix::fs::symlink(tmp.path().join("sub"), tmp.path().join("link")).expect("link");

        let (events, _) = collect(tmp.path(), ScanOptions::recursive());
        let links: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                ScanEvent::Entry(entry) if entry.name == "link" => Some(entry.kind),
                _ => None,
            })
            .collect();
        assert_eq!(links, vec![EntryKind::Symlink]);
    }

    #[test]
    fn symlink_loop_is_detected_and_bounded() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let a = tmp.path().join("a");
        fs::create_dir(&a).expect("mkdir");
        fs::create_dir(a.join("b")).expect("mkdir");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&a, a.join("b/loop")).expect("link");

        let options = ScanOptions {
            follow_symlinks: true,
            ..ScanOptions::recursive()
        };
        let cancel = CancellationToken::new();
        let mut events = Vec::new();
        let mut sink = |e: ScanEvent| {
            events.push(e);
            true
        };
        let summary = scan(tmp.path(), options, &cancel, &mut sink).expect("scan");

        assert!(!summary.cancelled, "scan should not need cancelling");
        let looped = events.iter().any(|e| {
            matches!(e, ScanEvent::Error(err) if matches!(err.error, KestrelError::LoopDetected { .. }))
        });
        assert!(looped, "expected a LoopDetected error event");
        // Without loop protection this would be an unbounded number of entries.
        assert!(summary.total < 10, "runaway traversal: {}", summary.total);
    }

    #[test]
    fn max_depth_stops_deep_trees() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let deep = tmp.path().join("a/b/c/d");
        fs::create_dir_all(&deep).expect("mkdir");
        fs::write(deep.join("leaf"), b"x").expect("write");

        let options = ScanOptions {
            max_depth: 1,
            ..ScanOptions::recursive()
        };
        let (events, _) = collect(tmp.path(), options);
        assert!(!names(&events).contains(&"leaf".to_string()));
    }

    #[test]
    fn listing_is_sorted_naturally() {
        let tmp = tempfile::tempdir().expect("tempdir");
        for n in ["file10", "file2", "file1"] {
            fs::write(tmp.path().join(n), b"x").expect("write");
        }
        let (events, _) = collect(tmp.path(), ScanOptions::listing());
        assert_eq!(names(&events), vec!["file1", "file2", "file10"]);
    }

    #[test]
    fn missing_root_is_an_error_not_a_panic() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let missing = tmp.path().join("nope");
        let err = scan(
            &missing,
            ScanOptions::listing(),
            &CancellationToken::new(),
            &mut |_| true,
        )
        .expect_err("should fail");
        assert!(err.is_not_found());
    }

    #[test]
    fn file_as_root_is_not_a_directory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file = tmp.path().join("file.txt");
        fs::write(&file, b"x").expect("write");
        let err = scan(
            &file,
            ScanOptions::listing(),
            &CancellationToken::new(),
            &mut |_| true,
        )
        .expect_err("should fail");
        assert!(
            matches!(err, KestrelError::NotADirectory { .. }),
            "got {err}"
        );
    }

    #[test]
    fn cancellation_stops_a_walk() {
        let tmp = tempfile::tempdir().expect("tempdir");
        for i in 0..50 {
            fs::create_dir(tmp.path().join(format!("d{i}"))).expect("mkdir");
        }
        let cancel = CancellationToken::new();
        cancel.cancel();
        let mut count = 0;
        let summary = scan(tmp.path(), ScanOptions::recursive(), &cancel, &mut |_| {
            count += 1;
            true
        })
        .expect("scan");
        assert!(summary.cancelled);
        assert_eq!(count, 0);
    }

    #[test]
    fn sink_can_stop_early() {
        let tmp = tempfile::tempdir().expect("tempdir");
        for i in 0..20 {
            fs::write(tmp.path().join(format!("f{i}")), b"x").expect("write");
        }
        let mut count = 0;
        let summary = scan(
            tmp.path(),
            ScanOptions::listing(),
            &CancellationToken::new(),
            &mut |_| {
                count += 1;
                count < 3
            },
        )
        .expect("scan");
        assert_eq!(count, 3);
        assert!(summary.cancelled, "stopping the sink is a cancellation");
    }

    #[test]
    fn background_handle_delivers_batches_and_completion() {
        let tmp = tempfile::tempdir().expect("tempdir");
        for i in 0..600 {
            fs::write(tmp.path().join(format!("f{i:04}")), b"x").expect("write");
        }
        let handle = start(tmp.path(), ScanOptions::listing()).expect("spawn");

        let mut batches = 0;
        let mut total = 0;
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            match handle.recv_timeout(remaining) {
                Ok(ScanEvent::Entry(_)) => total += 1,
                Ok(ScanEvent::BatchEnd) => batches += 1,
                Ok(ScanEvent::Complete { total: t }) => {
                    assert_eq!(t, total);
                    break;
                }
                Ok(ScanEvent::Error(_)) => {}
                Err(RecvTimeoutError::Timeout) => panic!("scan timed out"),
                Err(e) => panic!("channel failed: {e}"),
            }
        }
        assert_eq!(total, 600);
        assert!(batches >= 2, "expected several batches, got {batches}");
    }

    #[test]
    fn background_handle_can_be_cancelled() {
        let tmp = tempfile::tempdir().expect("tempdir");
        for i in 0..2000 {
            fs::create_dir(tmp.path().join(format!("d{i:04}"))).expect("mkdir");
        }
        let handle = start(tmp.path(), ScanOptions::recursive()).expect("spawn");
        handle.cancel();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            match handle.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            {
                Ok(ScanEvent::Complete { .. }) => break,
                Ok(_) => {}
                Err(_) => panic!("worker did not stop after cancel"),
            }
        }
    }
}
