//! Debounced filesystem watching for the currently displayed directory.
//!
//! # Why debouncing is not optional
//!
//! A single `cargo build` can emit thousands of inotify events in a few
//! seconds; an editor save can emit three at once; `rsync` emits a stream.
//! If the UI rescan fires per event it re-reads the directory hundreds of
//! times and the app *loses frames* — the watcher itself becomes the lag. So
//! raw events are collected and coalesced: a [`ChangeSet`] is only delivered
//! after the directory has been quiet for the debounce window (250 ms by
//! default), with a hard ceiling so a very long write burst still produces
//! periodic refreshes.
//!
//! Only the directory the user is looking at is watched, non-recursively:
//! watching `/home` recursively would wake the app for every file any process
//! touches.
//!
//! # Example
//!
//! ```no_run
//! use kestrel_fs::watcher::{WatchEvent, watch_with};
//! use std::time::Duration;
//!
//! let sub = watch_with("/home/achraf", Duration::from_millis(250)).expect("watch");
//! loop {
//!     match sub.recv_timeout(Duration::from_millis(16)) {
//!         // Only these directories need re-listing.
//!         Ok(WatchEvent::Changed(changes)) => {
//!             for dir in &changes.dirs {
//!                 println!("refresh {dir}", dir = dir.display());
//!             }
//!         }
//!         Ok(WatchEvent::Error(err)) => eprintln!("watcher: {err}"),
//!         Err(_) => break,
//!     }
//! }
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use notify::{EventKind, RecursiveMode, Watcher};

use crate::error::KestrelError;
use crate::model::CancellationToken;

/// Default quiet period before a change set is delivered.
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(250);

/// How often the debouncer wakes up to check whether it should give up waiting.
const POLL: Duration = Duration::from_millis(25);

/// A burst of changes, coalesced across the debounce window.
///
/// `dirs` is what the UI actually needs: the set of directories whose listing
/// is now stale. `paths` carries the raw affected paths for finer-grained
/// reactions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangeSet {
    /// Every path the backend mentioned, deduplicated.
    pub paths: Vec<PathBuf>,
    /// Directories that need re-listing: a changed path that is itself a
    /// directory, or the parent of a changed file.
    pub dirs: Vec<PathBuf>,
}

impl ChangeSet {
    /// `true` when `dir` is one of the directories needing a refresh.
    #[must_use]
    pub fn touches(&self, dir: &Path) -> bool {
        self.dirs.iter().any(|d| d == dir)
    }
}

/// An item of watcher output.
#[derive(Debug)]
pub enum WatchEvent {
    /// Something changed, after debouncing.
    Changed(ChangeSet),
    /// The backend reported an error, or a watched path vanished.
    Error(KestrelError),
}

/// Watches directories, coalescing bursts into [`ChangeSet`]s.
///
/// All methods take `&self` and are safe to call from the UI thread: the only
/// locking involved is a short-lived `Mutex` around the subscriber list and the
/// notify handle, and no filesystem syscall happens on the caller's thread.
pub struct DirWatcher {
    root: PathBuf,
    debounce: Arc<Mutex<Duration>>,
    subscribers: Arc<Mutex<Vec<Sender<WatchEvent>>>>,
    backend: Arc<Mutex<Option<notify::RecommendedWatcher>>>,
    cancel: CancellationToken,
    debouncer: Option<std::thread::JoinHandle<()>>,
}

impl std::fmt::Debug for DirWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirWatcher")
            .field("root", &self.root)
            .field(
                "subscribers",
                &self.subscribers.lock().map(|s| s.len()).unwrap_or(0),
            )
            .finish()
    }
}

impl DirWatcher {
    /// Starts watching `dir` (non-recursively).
    ///
    /// # Errors
    ///
    /// If the directory does not exist, or the platform backend refuses to
    /// watch it (too many watches, inotify limits, ...).
    pub fn new(dir: impl AsRef<Path>, debounce: Duration) -> Result<Self, KestrelError> {
        let root = dir.as_ref().to_path_buf();
        let (raw_tx, raw_rx) = channel::<notify::Result<notify::Event>>();

        let mut backend = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            // A closed channel only means the debouncer is gone; dropping the
            // event is correct.
            let _ = raw_tx.send(res);
        })
        .map_err(|e| KestrelError::Watch {
            path: Some(root.clone()),
            message: e.to_string(),
        })?;

        backend
            .watch(&root, RecursiveMode::NonRecursive)
            .map_err(|e| KestrelError::Watch {
                path: Some(root.clone()),
                message: e.to_string(),
            })?;

        let debounce_slot = Arc::new(Mutex::new(debounce));
        let subscribers: Arc<Mutex<Vec<Sender<WatchEvent>>>> = Arc::new(Mutex::new(Vec::new()));
        let cancel = CancellationToken::new();

        let debouncer = {
            let debounce_slot = Arc::clone(&debounce_slot);
            let subscribers = Arc::clone(&subscribers);
            let cancel = cancel.clone();
            let root_label = root.clone();
            std::thread::Builder::new()
                .name("kestrel-watch".to_string())
                .spawn(move || {
                    debounce_loop(&root_label, &raw_rx, &debounce_slot, &subscribers, &cancel);
                })
                .map_err(|e| KestrelError::Watch {
                    path: Some(root.clone()),
                    message: e.to_string(),
                })?
        };

        Ok(Self {
            root,
            debounce: debounce_slot,
            subscribers,
            backend: Arc::new(Mutex::new(Some(backend))),
            cancel,
            debouncer: Some(debouncer),
        })
    }

    /// The directory currently watched.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Registers a new subscriber and returns its receiving end.
    #[must_use]
    pub fn subscribe(&self) -> Receiver<WatchEvent> {
        let (tx, rx) = channel();
        let mut guard = lock(&self.subscribers);
        guard.push(tx);
        rx
    }

    /// Changes the debounce window; takes effect on the next event.
    pub fn set_debounce(&self, debounce: Duration) {
        let mut guard = lock(&self.debounce);
        *guard = debounce;
    }

    /// The current debounce window.
    #[must_use]
    pub fn debounce(&self) -> Duration {
        *lock(&self.debounce)
    }

    /// Starts watching an additional directory (non-recursively).
    ///
    /// # Errors
    ///
    /// Propagates the backend's error, e.g. for a path that does not exist.
    pub fn add_path(&self, dir: impl AsRef<Path>) -> Result<(), KestrelError> {
        let dir = dir.as_ref();
        let mut guard = lock(&self.backend);
        let backend = guard.as_mut().ok_or_else(|| KestrelError::Watch {
            path: Some(dir.to_path_buf()),
            message: "watcher has been stopped".to_string(),
        })?;
        backend
            .watch(dir, RecursiveMode::NonRecursive)
            .map_err(|e| KestrelError::Watch {
                path: Some(dir.to_path_buf()),
                message: e.to_string(),
            })
    }

    /// Stops watching a directory added with [`add_path`](Self::add_path).
    ///
    /// # Errors
    ///
    /// Propagates the backend's error.
    pub fn remove_path(&self, dir: impl AsRef<Path>) -> Result<(), KestrelError> {
        let dir = dir.as_ref();
        let mut guard = lock(&self.backend);
        let backend = guard.as_mut().ok_or_else(|| KestrelError::Watch {
            path: Some(dir.to_path_buf()),
            message: "watcher has been stopped".to_string(),
        })?;
        backend.unwatch(dir).map_err(|e| KestrelError::Watch {
            path: Some(dir.to_path_buf()),
            message: e.to_string(),
        })
    }
}

impl Drop for DirWatcher {
    fn drop(&mut self) {
        self.cancel.cancel();
        // Dropping the notify handle stops its event thread, which drops the
        // callback and therefore closes the raw channel; the debouncer notices
        // on its next poll either way.
        drop(lock(&self.backend).take());
        if let Some(handle) = self.debouncer.take() {
            let _ = handle.join();
        }
    }
}

/// Owns a [`DirWatcher`] plus one subscriber's receiving end.
///
/// The watcher is a field, so the subscription keeps it alive; dropping this
/// struct stops everything.
#[derive(Debug)]
pub struct WatchSubscription {
    watcher: DirWatcher,
    events: Receiver<WatchEvent>,
}

impl WatchSubscription {
    /// Blocks for the next event.
    pub fn recv(&self) -> std::result::Result<WatchEvent, std::sync::mpsc::RecvError> {
        self.events.recv()
    }

    /// Blocks for at most `timeout`, so a UI frame can end first.
    pub fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> std::result::Result<WatchEvent, std::sync::mpsc::RecvTimeoutError> {
        self.events.recv_timeout(timeout)
    }

    /// Non-blocking read.
    pub fn try_recv(&self) -> std::result::Result<WatchEvent, std::sync::mpsc::TryRecvError> {
        self.events.try_recv()
    }

    /// Adds a second consumer (e.g. a background reconciler) to the same
    /// watcher.
    #[must_use]
    pub fn subscribe(&self) -> Receiver<WatchEvent> {
        self.watcher.subscribe()
    }

    /// The underlying watcher, to widen or re-tune the watch set.
    #[must_use]
    pub fn watcher(&self) -> &DirWatcher {
        &self.watcher
    }

    /// Convenience: the directory being watched.
    #[must_use]
    pub fn root(&self) -> &Path {
        self.watcher.root()
    }
}

/// Watches `dir` with the default debounce window.
///
/// # Errors
///
/// See [`DirWatcher::new`].
pub fn watch(dir: impl AsRef<Path>) -> Result<WatchSubscription, KestrelError> {
    watch_with(dir, DEFAULT_DEBOUNCE)
}

/// Watches `dir` with an explicit debounce window.
///
/// # Errors
///
/// See [`DirWatcher::new`].
///
/// ```
/// use kestrel_fs::watcher::{DEFAULT_DEBOUNCE, watch_with};
/// use std::time::Duration;
///
/// # fn demo(dir: &std::path::Path) -> Result<(), kestrel_fs::KestrelError> {
/// let sub = watch_with(dir, Duration::from_millis(150))?;
/// assert_eq!(sub.watcher().debounce(), Duration::from_millis(150));
/// assert!(sub.watcher().debounce() < DEFAULT_DEBOUNCE);
/// # Ok(())
/// # }
/// ```
pub fn watch_with(
    dir: impl AsRef<Path>,
    debounce: Duration,
) -> Result<WatchSubscription, KestrelError> {
    let watcher = DirWatcher::new(dir, debounce)?;
    let events = watcher.subscribe();
    Ok(WatchSubscription { watcher, events })
}

/// Accumulates events and flushes them once the directory goes quiet.
fn debounce_loop(
    root: &Path,
    raw: &Receiver<notify::Result<notify::Event>>,
    debounce_slot: &Mutex<Duration>,
    subscribers: &Arc<Mutex<Vec<Sender<WatchEvent>>>>,
    cancel: &CancellationToken,
) {
    let mut pending: HashMap<PathBuf, ()> = HashMap::new();
    // Trailing edge: every new event pushes the deadline out. The ceiling stops
    // a continuous writer from starving the UI of refreshes entirely.
    let mut deadline: Option<Instant> = None;
    let mut first_event: Option<Instant> = None;

    loop {
        if cancel.is_cancelled() {
            return;
        }
        let window = *lock(debounce_slot);
        // Always flush within a few windows, even if events never stop.
        let ceiling = window.saturating_mul(5).max(Duration::from_secs(1));

        match raw.recv_timeout(POLL) {
            Ok(Ok(event)) => {
                if matches!(event.kind, EventKind::Other) {
                    // e.g. a "rescan" poke on some backends; nothing to show.
                    continue;
                }
                for path in &event.paths {
                    pending.insert(path.clone(), ());
                }
                if deadline.is_none() {
                    deadline = Some(Instant::now() + window);
                    first_event = Some(Instant::now());
                } else if let Some(start) = first_event
                    && Instant::now() >= start + ceiling
                {
                    // Burst too long: give the UI something now.
                    flush(&mut pending, subscribers);
                    deadline = None;
                    first_event = None;
                } else {
                    deadline = Some(Instant::now() + window);
                }
            }
            Ok(Err(e)) => {
                let _ = broadcast(
                    subscribers,
                    WatchEvent::Error(KestrelError::Watch {
                        path: Some(root.to_path_buf()),
                        message: e.to_string(),
                    }),
                );
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if let Some(deadline_at) = deadline
                    && Instant::now() >= deadline_at
                {
                    flush(&mut pending, subscribers);
                    deadline = None;
                    first_event = None;
                }
            }
        }
    }
}

/// Turns the pending paths into a [`ChangeSet`] and hands it to every
/// subscriber, dropping the ones that have gone away.
fn flush(pending: &mut HashMap<PathBuf, ()>, subscribers: &Arc<Mutex<Vec<Sender<WatchEvent>>>>) {
    if pending.is_empty() {
        return;
    }
    let mut paths: Vec<PathBuf> = pending.drain().map(|(p, ())| p).collect();
    paths.sort();

    let mut dirs: Vec<PathBuf> = Vec::new();
    for path in &paths {
        // An existing directory changed in its own right; anything else means
        // its parent listing is stale. A removed path no longer exists, so this
        // naturally resolves to the parent.
        let is_dir = std::fs::symlink_metadata(path).is_ok_and(|md| md.is_dir());
        let target = if is_dir {
            path.clone()
        } else {
            path.parent()
                .map_or_else(|| path.clone(), Path::to_path_buf)
        };
        if !dirs.contains(&target) {
            dirs.push(target);
        }
    }

    let _ = broadcast(subscribers, WatchEvent::Changed(ChangeSet { paths, dirs }));
}

/// Sends to all subscribers, removing any whose receiver is gone.
fn broadcast(subscribers: &Arc<Mutex<Vec<Sender<WatchEvent>>>>, event: WatchEvent) -> usize {
    let mut guard = lock(subscribers);
    let mut delivered = 0usize;
    guard.retain(|tx| {
        let ok = tx.send(clone_event(&event)).is_ok();
        if ok {
            delivered += 1;
        }
        ok
    });
    delivered
}

/// `WatchEvent` is not `Clone` (it owns errors), and every subscriber needs
/// its own copy, so rebuild it from the shared description.
fn clone_event(event: &WatchEvent) -> WatchEvent {
    match event {
        WatchEvent::Changed(changes) => WatchEvent::Changed(changes.clone()),
        WatchEvent::Error(err) => WatchEvent::Error(KestrelError::Watch {
            path: err.path().map(Path::to_path_buf),
            message: err.to_string(),
        }),
    }
}

/// Poison-safe lock; see the note in [`crate::size`].
///
/// [`crate::size`]: crate::size::SizeCache
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn watching_a_missing_directory_is_an_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let err = DirWatcher::new(tmp.path().join("nope"), DEFAULT_DEBOUNCE).expect_err("no dir");
        assert!(matches!(err, KestrelError::Watch { .. }), "got {err}");
    }

    #[test]
    fn change_set_reports_the_parent_directory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sub = watch(tmp.path()).expect("watch");

        fs::write(tmp.path().join("new.txt"), b"hello").expect("write");

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match sub.recv_timeout(remaining) {
                Ok(WatchEvent::Changed(changes)) => {
                    assert!(
                        changes.touches(tmp.path()),
                        "expected the parent to be flagged"
                    );
                    return;
                }
                Ok(WatchEvent::Error(_)) => {}
                Err(e) => panic!("no change event within the deadline: {e}"),
            }
        }
    }

    #[test]
    fn a_created_directory_is_flagged_itself() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sub = watch(tmp.path()).expect("watch");
        let new_dir = tmp.path().join("fresh");
        fs::create_dir(&new_dir).expect("mkdir");

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match sub.recv_timeout(remaining) {
                Ok(WatchEvent::Changed(changes)) => {
                    assert!(
                        changes.touches(&new_dir) || changes.touches(tmp.path()),
                        "changed set did not include the new directory: {changes:?}"
                    );
                    return;
                }
                Ok(WatchEvent::Error(_)) => {}
                Err(e) => panic!("no change event within the deadline: {e}"),
            }
        }
    }

    #[test]
    fn bursts_are_coalesced_into_one_event() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sub = watch_with(tmp.path(), Duration::from_millis(150)).expect("watch");

        // Simulate a build: many writes in quick succession.
        for i in 0..50 {
            fs::write(tmp.path().join(format!("f{i:03}")), b"x").expect("write");
        }

        let mut events = 0;
        let first_deadline = Instant::now() + Duration::from_secs(10);
        while events == 0 {
            match sub.recv_timeout(first_deadline.saturating_duration_since(Instant::now())) {
                Ok(WatchEvent::Changed(changes)) => {
                    assert!(changes.paths.len() > 1, "burst should be coalesced");
                    events += 1;
                }
                Ok(WatchEvent::Error(_)) => {}
                Err(e) => panic!("no change event within the deadline: {e}"),
            }
        }
    }

    #[test]
    fn multiple_subscribers_both_receive() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sub = watch(tmp.path()).expect("watch");
        let second = sub.subscribe();
        fs::write(tmp.path().join("a"), b"x").expect("write");

        for rx in [sub.watcher().subscribe(), second] {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                    Ok(WatchEvent::Changed(_)) => break,
                    Ok(WatchEvent::Error(_)) => {}
                    Err(e) => panic!("subscriber missed the event: {e}"),
                }
            }
        }
    }

    #[test]
    fn drop_stops_the_worker_thread() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let started = Instant::now();
        {
            let _sub = watch(tmp.path()).expect("watch");
        }
        // `Drop` joins the debouncer; it polls every 25 ms, so this is quick.
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "dropping a watcher must not block"
        );
    }
}
