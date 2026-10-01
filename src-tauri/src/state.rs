//! Phase 1 IPC backend state.
//!
//! The single [`Backend`] value is created once at startup
//! ([`Backend::try_new`]) and shared with every command through
//! `tauri::State`. The frontend never mints ids: [`JobId`]s come from
//! [`Backend::mint`] (`AtomicU64::fetch_add`), so untrusted input can
//! neither collide with nor guess another job's id.
//!
//! ## The `!Sync` crux (MIGRATION.md §3)
//!
//! `tauri::State<T>` requires `T: Send + Sync + 'static`, but the engine's
//! `ScanHandle`, `SizeHandle`, `WatchSubscription` receiver half and
//! `DirWatcher` each own an `mpsc::Receiver` or a `JoinHandle`, so they are
//! `Send` but **not** `Sync`. The `Mutex` around each of them is the
//! mechanical `!Sync → Sync` adapter — it is not taken to guard `recv()`
//! (those take `&self`); it only makes the type `Sync`. Per-job
//! `Arc<Mutex<Handle>>` values sit inside the outer maps so cancelling one
//! job never serialises, or blocks, any other. **Do not "fix" this by
//! changing the engine.**

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use kestrel_fs::KestrelError;
use kestrel_fs::model::CancellationToken;
use kestrel_fs::scan::ScanHandle;
use kestrel_fs::size::SizeHandle;
use kestrel_fs::watcher::{DEFAULT_DEBOUNCE, DirWatcher, WatchEvent};

/// A backend-minted job id. The frontend never invents one: ids come from
/// [`Backend::mint`] only.
pub type JobId = u64;

/// First id handed out. `0` is reserved as "no job" for the frontend.
pub const FIRST_JOB_ID: JobId = 1;

/// Phase 3 placeholder: one tracked mutating operation (copy / move / trash /
/// delete with progress). Phase 1 only reserves the map slot so the [`Backend`]
/// shape is final; later phases fill this in.
#[derive(Debug)]
pub struct OpJob {
    /// Cancels the in-flight operation. `CancellationToken` is `Send + Sync`,
    /// so this needs no adapter.
    pub cancel: CancellationToken,
    /// Human-readable description for progress UI ("Copying foo → bar").
    pub description: String,
}

/// Phase 2 placeholder: one subscribed directory relay on the long-lived
/// [`DirWatcher`]. Phase 1 only reserves the map slot; Phase 2 pumps the
/// receiver into a `Channel`.
pub struct WatchRelay {
    /// The directory being relayed, exactly as subscribed (the frontend
    /// compares these strings against the viewed directory, so the raw
    /// subscribed path is kept — never canonicalised).
    pub dir: PathBuf,
    /// The subscriber's receiving end on the shared watcher. `Receiver` is
    /// `Send` but **not** `Sync`, so the `Mutex` is the same mechanical
    /// adapter as everywhere else in this module; the `Arc` lets the pump
    /// task hold the receiver while the map owns the relay, mirroring the
    /// per-job `Arc<Mutex<ScanHandle>>` pattern.
    pub events: Arc<Mutex<Receiver<WatchEvent>>>,
    /// Stops the pump task. Set by [`Backend::cancel_watch`]; the pump also
    /// exits when the engine's senders go away (backend shutdown).
    pub cancel: CancellationToken,
}

/// Shared backend state, managed once at startup. See the module docs for why
/// every engine handle sits behind a `Mutex`.
pub struct Backend {
    /// Monotonic id source. See [`Backend::mint`].
    pub next_id: AtomicU64,
    /// In-flight scans by id. Removed by the pump task when the scan
    /// completes and by `scan_cancel` when the user navigates away.
    pub scans: Mutex<HashMap<JobId, Arc<Mutex<ScanHandle>>>>,
    /// Phase 2+: in-flight size computations. Reserved now so the shape is final.
    pub sizes: Mutex<HashMap<JobId, Arc<Mutex<SizeHandle>>>>,
    /// Phase 3+: in-flight mutating operations. Reserved now.
    pub ops: Mutex<HashMap<JobId, Arc<OpJob>>>,
    /// The ONE long-lived watcher (MIGRATION.md §3). `DirWatcher::drop`
    /// joins its debouncer thread, so one shared instance — never one per
    /// subscription — is what keeps thread count flat while navigating.
    ///
    /// `DirWatcher` owns a `JoinHandle` and is therefore `Send` but **not**
    /// `Sync`; the `Mutex` adapts it for `tauri::State`. This is a deliberate
    /// deviation from the literal struct sketch in MIGRATION.md §3 (which
    /// shows the field bare): the sketch's own `Send + Sync + 'static`
    /// requirement forces it, for exactly the reason the section gives.
    pub watcher: Mutex<DirWatcher>,
    /// Phase 2+: watch relays by id. Reserved now.
    pub watches: Mutex<HashMap<JobId, WatchRelay>>,
}

impl Backend {
    /// Creates the backend and its one long-lived [`DirWatcher`].
    ///
    /// The watcher is homed on `$HOME`: the file manager opens there, so the
    /// root watch is the one subscription the app always needs and every
    /// other viewed directory is added with [`add_path`](DirWatcher::add_path)
    /// as navigation happens. When `$HOME` is unset or missing (CI, minimal
    /// containers), it falls back to the system temp directory, which always
    /// exists; either way the home is inert until subscribed — nothing reads
    /// from a root nobody subscribed to, so its debouncer thread idles.
    ///
    /// There is exactly one watcher for the whole process: each
    /// `WatchSubscription` would spawn its own `DirWatcher` and thread, and
    /// `DirWatcher::drop` joins that thread, so per-subscription watchers
    /// would mean N threads and blocking drops (MIGRATION.md §3).
    ///
    /// # Errors
    ///
    /// Returns the engine's [`KestrelError::Watch`] if the platform backend
    /// refuses even the home watch (exhausted inotify limits, ...).
    pub fn try_new() -> Result<Self, KestrelError> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_dir());
        let root = home.unwrap_or_else(std::env::temp_dir);
        Self::try_new_watching(root, DEFAULT_DEBOUNCE)
    }

    /// Same as [`try_new`](Self::try_new) but watching `dir`, for tests.
    pub fn try_new_watching(
        dir: impl AsRef<std::path::Path>,
        debounce: Duration,
    ) -> Result<Self, KestrelError> {
        Ok(Self {
            next_id: AtomicU64::new(FIRST_JOB_ID),
            scans: Mutex::new(HashMap::new()),
            sizes: Mutex::new(HashMap::new()),
            ops: Mutex::new(HashMap::new()),
            watcher: Mutex::new(DirWatcher::new(dir, debounce)?),
            watches: Mutex::new(HashMap::new()),
        })
    }

    /// Mints the next [`JobId`]. Lock-free; ids are unique per process even
    /// under concurrent `scan_start` calls.
    pub fn mint(&self) -> JobId {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Registers a freshly spawned scan handle under `id`.
    pub fn register_scan(&self, id: JobId, handle: Arc<Mutex<ScanHandle>>) {
        lock(&self.scans).insert(id, handle);
    }

    /// Removes a scan job and cancels its worker, if it is still present.
    /// Unknown ids are a no-op so a racing frontend (Complete arriving while
    /// the user navigates away) cannot error.
    pub fn cancel_scan(&self, id: JobId) {
        if let Some(handle) = lock(&self.scans).remove(&id) {
            lock(&handle).cancel();
        }
    }

    /// Removes an op job and cancels its worker, if it is still present.
    /// Unknown ids are a no-op so a racing frontend cannot error — the same
    /// posture as [`cancel_scan`](Self::cancel_scan) and
    /// [`cancel_watch`](Self::cancel_watch).
    ///
    /// Never blocks: cancelling is an atomic store on the token, and no thread
    /// is joined here. The pump task exits on its own once it observes the
    /// token (or finishes the op and removes the entry itself).
    pub fn cancel_op(&self, id: JobId) {
        if let Some(job) = lock(&self.ops).remove(&id) {
            job.cancel.cancel();
        }
    }

    /// Registers a freshly started op job under `id`.
    pub fn register_op(&self, id: JobId, job: Arc<OpJob>) {
        lock(&self.ops).insert(id, job);
    }

    /// Subscribes `dir` on the ONE shared watcher and returns the receiving
    /// end for a [`WatchRelay`].
    ///
    /// The receiver is created *before* `add_path` so no change can slip
    /// through between starting the inotify watch and listening. When `dir`
    /// is the watcher's own root it is already watched from birth and only
    /// the subscription is needed. A failed `add_path` (vanished directory,
    /// exhausted watches) drops the fresh receiver — the next engine
    /// broadcast prunes its orphaned sender — and reports the engine error,
    /// so the caller mints no job for a directory that cannot be watched.
    ///
    /// # Errors
    ///
    /// Propagates the engine's [`KestrelError::Watch`] when the path cannot
    /// be watched.
    pub fn add_watch(&self, dir: &std::path::Path) -> Result<Receiver<WatchEvent>, KestrelError> {
        let is_root = lock(&self.watcher).root().to_path_buf() == *dir;
        let rx = lock(&self.watcher).subscribe();
        if is_root {
            return Ok(rx);
        }
        if let Err(e) = lock(&self.watcher).add_path(dir) {
            drop(rx);
            return Err(e);
        }
        Ok(rx)
    }

    /// Registers a freshly subscribed watch relay under `id`.
    pub fn register_watch(&self, id: JobId, relay: WatchRelay) {
        lock(&self.watches).insert(id, relay);
    }

    /// Removes a watch relay and stops its pump, if it is still present.
    /// Unknown ids are a no-op so a racing frontend (a change arriving while
    /// the user navigates away) cannot error — the same posture as
    /// [`cancel_scan`](Self::cancel_scan).
    ///
    /// Never blocks: cancelling is an atomic store, and neither the shared
    /// `DirWatcher` (which stays alive in `self`) nor any thread is dropped
    /// here — `DirWatcher::drop` joins its debouncer thread, which is why the
    /// watcher is shared rather than per-subscription. The inotify watch for
    /// `dir` is released only when no remaining relay uses it (and never for
    /// the watcher's own root, which is permanent); the release is a quick
    /// `unwatch` syscall under a short-held lock, and failures are ignored —
    /// the directory may already be gone, which needs no action.
    pub fn cancel_watch(&self, id: JobId) {
        let Some(relay) = lock(&self.watches).remove(&id) else {
            return;
        };
        relay.cancel.cancel();
        let still_used = lock(&self.watches)
            .values()
            .any(|other| other.dir == relay.dir);
        if still_used {
            return;
        }
        let is_root = lock(&self.watcher).root().to_path_buf() == relay.dir;
        if !is_root {
            let _ = lock(&self.watcher).remove_path(&relay.dir);
        }
    }
}

/// Poison-safe lock: a panic elsewhere must not wedge the backend forever.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync + 'static>() {}

    /// The `!Sync` problem must not silently return: if any engine handle
    /// ever becomes `!Send`, or the adapters are removed, this fails to
    /// compile — which is the point.
    #[test]
    fn backend_is_send_sync_and_static() {
        assert_send_sync::<Backend>();
    }

    #[test]
    fn per_job_wrappers_are_send_sync() {
        assert_send_sync::<Arc<Mutex<ScanHandle>>>();
        assert_send_sync::<Arc<Mutex<SizeHandle>>>();
        assert_send_sync::<Arc<OpJob>>();
        assert_send_sync::<WatchRelay>();
        assert_send_sync::<Mutex<DirWatcher>>();
    }

    #[test]
    fn job_ids_are_unique_and_never_zero() {
        let backend = Backend::try_new_watching(std::env::temp_dir(), Duration::from_millis(1))
            .expect("backend");
        let ids: Vec<JobId> = (0..1000).map(|_| backend.mint()).collect();
        assert!(ids.iter().all(|&id| id != 0), "0 is reserved as no-job");
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "minted ids must be unique");
        assert_eq!(ids[0], FIRST_JOB_ID);
    }

    #[test]
    fn cancel_scan_is_idempotent_and_cancels_the_worker() {
        use kestrel_fs::scan::{ScanOptions, start};

        let backend = Backend::try_new_watching(std::env::temp_dir(), Duration::from_millis(1))
            .expect("backend");
        let dir = tempfile::tempdir().expect("tempdir");
        let handle = start(dir.path(), ScanOptions::listing()).expect("spawn");
        let id = backend.mint();
        backend.register_scan(id, Arc::new(Mutex::new(handle)));
        assert!(lock(&backend.scans).contains_key(&id));
        backend.cancel_scan(id);
        assert!(!lock(&backend.scans).contains_key(&id));
        // Racing cancels and unknown ids are silent no-ops, never panics.
        backend.cancel_scan(id);
        backend.cancel_scan(u64::MAX);
    }
}
