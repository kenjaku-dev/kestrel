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
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use kestrel_fs::KestrelError;
use kestrel_fs::model::CancellationToken;
use kestrel_fs::scan::ScanHandle;
use kestrel_fs::size::SizeHandle;
use kestrel_fs::watcher::{DEFAULT_DEBOUNCE, DirWatcher, WatchEvent};

use crate::dto::CollisionDecisionDto;

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
    /// Collisions this op is currently blocked on, keyed by destination path.
    ///
    /// One rendezvous per collision: the worker parks on the receiving end
    /// (`recv_timeout`, so it also watches `cancel` and the answer deadline),
    /// and `op_collision_answer` sends on this end. `SyncSender` is `Send` but
    /// **not** `Sync`, so this field needs the same mechanical `Mutex` adapter
    /// as every other engine handle in this module — and the lock is held only
    /// for the `insert`/`remove`, never across `send`/`recv`.
    ///
    /// Keyed by path as well as by job so an answer for a stale `dst` (already
    /// settled, or never asked about) is a silent no-op rather than an
    /// answer landing on somebody else's channel.
    pub pending: Mutex<HashMap<PathBuf, SyncSender<CollisionDecisionDto>>>,
}

impl OpJob {
    /// A fresh op with no pending collisions.
    #[must_use]
    pub fn new(cancel: CancellationToken, description: String) -> Self {
        Self {
            cancel,
            description,
            pending: Mutex::new(HashMap::new()),
        }
    }
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

    /// Delivers one collision answer to the op waiting on `dst`.
    ///
    /// A **silent no-op** in every case where there is nothing sensible to do,
    /// which is the same posture as [`cancel_op`](Self::cancel_op) and the
    /// scan/watch cancels:
    ///
    /// * unknown job id (the op finished, or the id never existed),
    /// * `dst` is not the collision currently pending (already answered, or a
    ///   path this op was never asked about),
    /// * the worker thread is gone (the channel receiver was dropped).
    ///
    /// Never errors and never panics: an answer racing its own op is normal
    /// traffic, not a failure, and the frontend must not have to distinguish
    /// "answered" from "nobody was listening".
    ///
    /// The channel is **removed** before sending and the send happens with no
    /// lock held. That ordering is what makes a second answer for the same
    /// `dst` a no-op rather than a queue-up: the first caller takes the sender
    /// out of the map, so the second finds nothing. It also means the op
    /// record's lock is never held across a `send` that could block on a
    /// full rendezvous.
    pub fn answer_collision(&self, id: JobId, dst: &Path, decision: CollisionDecisionDto) {
        let Some(job) = lock(&self.ops).get(&id).cloned() else {
            return;
        };
        let Some(sender) = lock(&job.pending).remove(dst) else {
            return;
        };
        // `SyncSender::send` on a rendezvous channel blocks until the worker
        // receives; the worker is either parked on it or gone. If it is gone
        // the receiver is dropped and `send` returns `Err` immediately, so this
        // cannot hang. Discarded either way: an answer nobody receives is a
        // no-op, not an error.
        let _ = sender.send(decision);
    }

    /// The collisions every in-flight op is currently blocked on, as
    /// `(id, dst)` pairs.
    ///
    /// This is the discovery half of the round-trip, and it exists because a
    /// frontend cannot know an op is paused by looking at its own state: the
    /// `collision` event is fire-and-forget over a `Channel`, so a window that
    /// reloads (or mounts late) mid-pause has missed it. Mirrors the scan
    /// pump's posture — the registry is the truth, and the UI resynchronises
    /// from it rather than trusting it caught every message.
    ///
    /// Order is unspecified (a hash map), and the frontend must not depend on
    /// it; it keys by `id` anyway.
    #[must_use]
    pub fn pending_collisions(&self) -> Vec<(JobId, PathBuf)> {
        let jobs: Vec<(JobId, Arc<OpJob>)> = lock(&self.ops)
            .iter()
            .map(|(id, job)| (*id, Arc::clone(job)))
            .collect();
        let mut out = Vec::new();
        for (id, job) in jobs {
            // The `ops` lock is released before each `pending` lock: two
            // different locks, never nested, so an answer in flight can never
            // wait on a registry walk.
            for dst in lock(&job.pending).keys() {
                out.push((id, dst.clone()));
            }
        }
        out
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

    /// The rendezvous registry is behind a `Mutex` like every other engine
    /// handle here, and the `Send + Sync` bound must survive it: a
    /// `SyncSender` is `Send` but **not** `Sync`, so this is exactly the
    /// `!Sync → Sync` adapter the module documents. Removing the `Mutex` — or
    /// letting the job hold an `mpsc::Sender` bare — must fail to compile.
    #[test]
    fn op_jobs_with_pending_channels_stay_send_sync() {
        assert_send_sync::<Arc<OpJob>>();
        let job = OpJob::new(CancellationToken::new(), "test".to_string());
        let (tx, _rx) = std::sync::mpsc::sync_channel(1);
        lock(&job.pending).insert(PathBuf::from("/tmp/x"), tx);
        assert_send_sync::<Mutex<HashMap<PathBuf, SyncSender<CollisionDecisionDto>>>>();
        // And the registry is readable from another thread while a worker
        // holds the job — the whole point of the adapter.
        let shared = Arc::new(job);
        let other = Arc::clone(&shared);
        assert!(
            std::thread::spawn(move || lock(&other.pending).len() == 1)
                .join()
                .expect("reader thread")
        );
    }

    /// One op's answers never reach another's channel: the same `dst` in a
    /// different op is a different question.
    #[test]
    fn an_answer_only_reaches_the_job_that_is_blocked_on_that_path() {
        let backend = Backend::try_new_watching(std::env::temp_dir(), Duration::from_millis(1))
            .expect("backend");
        let skip = serde_json::from_str("\"skip\"").expect("decision");
        let dst = PathBuf::from("/shared/path.txt");
        let one = Arc::new(OpJob::new(CancellationToken::new(), "one".to_string()));
        let two = Arc::new(OpJob::new(CancellationToken::new(), "two".to_string()));
        let (tx_one, rx_one) = std::sync::mpsc::sync_channel(1);
        let (tx_two, _rx_two) = std::sync::mpsc::sync_channel(1);
        lock(&one.pending).insert(dst.clone(), tx_one);
        lock(&two.pending).insert(dst.clone(), tx_two);
        backend.register_op(1, Arc::clone(&one));
        backend.register_op(2, Arc::clone(&two));

        backend.answer_collision(1, &dst, skip);

        assert_eq!(rx_one.try_recv().expect("job 1 received its answer"), skip);
        assert!(
            lock(&two.pending).contains_key(&dst),
            "job 2's question is untouched — the same path in a different op \
             is a different question"
        );
    }

    /// `cancel_op` on a paused op drops the registry entry. The answer then
    /// goes nowhere, which is the required silent no-op rather than a panic:
    /// the frontend races this against its own teardown routinely.
    #[test]
    fn cancel_op_drops_the_job_and_its_pending_answers_with_it() {
        let backend = Backend::try_new_watching(std::env::temp_dir(), Duration::from_millis(1))
            .expect("backend");
        let dst = PathBuf::from("/tmp/pending.txt");
        let job = Arc::new(OpJob::new(CancellationToken::new(), "test".to_string()));
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        lock(&job.pending).insert(dst.clone(), tx);
        backend.register_op(9, Arc::clone(&job));
        assert_eq!(backend.pending_collisions(), vec![(9, dst.clone())]);

        backend.cancel_op(9);

        assert!(
            backend.pending_collisions().is_empty(),
            "a cancelled op advertises no questions: the dialog would have \
             nothing to answer and no op to answer it for"
        );
        assert!(
            job.cancel.is_cancelled(),
            "the token must be set, not dropped — the pause watches the token"
        );
        backend.answer_collision(9, &dst, serde_json::from_str("\"abort\"").expect("d"));
        assert!(
            rx.try_recv().is_err(),
            "an answer for a cancelled op is a silent no-op"
        );
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
