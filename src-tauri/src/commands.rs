//! Phase 1 IPC commands: `scan_start`, `scan_cancel`, `stat`, `open_path`.
//! Phase 2 adds `watch_subscribe` / `watch_unsubscribe` (live directory
//! watching over the one shared `DirWatcher`).
//!
//! Final signatures (Tauri-injected `AppHandle` / `State` params are invisible
//! to JS; the JS-visible contract is the remaining params, all camelCase):
//!
//! ```ts
//! scan_start(path: string, options: ScanOptionsDto, events: Channel<ScanEventDto>): Promise<number>
//! scan_cancel(id: number): Promise<void>
//! stat(path: string): Promise<FileEntryDto>
//! open_path(path: string): Promise<{ enteredDir: boolean }>
//! watch_subscribe(path: string, events: Channel<WatchEventDto>): Promise<number>
//! watch_unsubscribe(id: number): Promise<void>
//! ```
//!
//! No blocking `list_dir`: the streaming `scan_start` with
//! `recursive: false` **is** the listing, so the two UIs share one code path.
//! A `changed` watch event carries the stale directories and the frontend
//! answers by re-running `scan_start` for the directory it is viewing —
//! deliberately no incremental patching, for the same one-code-path reason.
//!
//! ## Backpressure (load-bearing)
//!
//! The engine cancels its worker when an `mpsc::send` fails — i.e. when the
//! `Receiver` is dropped. The pump below therefore holds the `ScanHandle`
//! (and its receiver) for the whole job and drains it promptly with
//! `recv_timeout`/`try_recv`; a slow or absent browser only ever fails
//! `Channel::send`, whose error the pump deliberately ignores. The engine can
//! never see a failed send until `scan_cancel`/completion drops the handle,
//! which is the intentional cancel — never a busy frontend.
//!
//! The watch relay is the same posture with the roles shifted: the engine
//! talks `mpsc` to the relay's receiver (broadcast to every subscriber), and
//! the relay drains that receiver with `recv_timeout` and forwards to the
//! `Channel`, discarding `Channel::send` results. A slow or absent frontend
//! therefore never stops the shared watcher and never spins the relay — the
//! relay idles in `recv_timeout` until the next engine event or cancel.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use kestrel_fs::error::classify_io;
use kestrel_fs::model::{CancellationToken, FileEntry};
use kestrel_fs::scan::{self, ScanEvent, ScanHandle};
use kestrel_fs::watcher::{ChangeSet, WatchEvent};
use tauri::{AppHandle, Manager, Runtime, async_runtime, ipc::Channel};

use crate::dto::{
    CmdError, FileEntryDto, OpenResultDto, ScanEventDto, ScanOptionsDto, WatchEventDto,
};
use crate::state::{Backend, JobId, WatchRelay};

/// How long one pump iteration waits for the next engine event. Short enough
/// that `scan_cancel` (which needs the same per-job lock) never waits long,
/// long enough not to spin.
const PUMP_WAIT: Duration = Duration::from_millis(50);

/// Upper bound on one `Entries` IPC message. The engine already inserts a
/// `BatchEnd` every `batch_size` (default 256); the cap only bounds a client
/// that raised `batch_size` without thinking.
const MAX_BATCH: usize = 512;

/// Give up waiting for the open worker after this long; the worker is
/// MIME-parse + spawn, i.e. milliseconds, so this only fires when something
/// is deeply wrong.
const OPEN_TIMEOUT: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// scan_start / scan_cancel
// ---------------------------------------------------------------------------

/// Starts a streaming scan. Returns the backend-minted [`JobId`]; events
/// arrive on `events` as [`ScanEventDto`] (`entries*`, then `complete`;
/// `error` rows may interleave). The caller must invoke `scan_cancel` once it
/// has `complete` or navigates away.
///
/// Fails fast (no job minted) when the root cannot even be `lstat`ed or the
/// worker thread cannot spawn — the engine would otherwise stream an empty
/// `complete` for a missing root, which reads as "empty directory".
#[tauri::command]
pub fn scan_start<R: Runtime>(
    app: AppHandle<R>,
    path: String,
    options: ScanOptionsDto,
    events: Channel<ScanEventDto>,
) -> Result<JobId, CmdError> {
    let engine_options = options.into_engine();
    if let Err(e) = std::fs::symlink_metadata(&path) {
        return Err(CmdError::from_kestrel(&classify_io(Path::new(&path), e)));
    }
    let backend = app.state::<Backend>();
    let id = backend.mint();
    let handle = scan::start(&path, engine_options)
        .map_err(|e| CmdError::from_kestrel(&classify_io(Path::new(&path), e)))?;
    let shared = Arc::new(Mutex::new(handle));
    backend.register_scan(id, Arc::clone(&shared));

    // `spawn_blocking`, not `spawn`: the pump is synchronous blocking code
    // (`recv_timeout` loop) and must not stall the async executor. `AppHandle`
    // is `'static`, so state re-resolves inside for the completion cleanup.
    async_runtime::spawn_blocking(move || {
        pump_scan_loop(&shared, &events);
        app.state::<Backend>().cancel_scan(id);
    });
    Ok(id)
}

/// Cancels a scan. Unknown ids are a silent no-op (the job already completed
/// and cleaned itself up, or the id never existed).
#[tauri::command]
pub fn scan_cancel<R: Runtime>(app: AppHandle<R>, id: JobId) {
    app.state::<Backend>().cancel_scan(id);
}

// ---------------------------------------------------------------------------
// the pump
// ---------------------------------------------------------------------------

/// Drains one scan job into its `Channel`, batching entry bursts at `BatchEnd`
/// boundaries, until the engine says `Complete` or its worker is gone.
///
/// Send failures (closed/slow webview) are **ignored on purpose**: the engine
/// talks `mpsc` to this pump, not `Channel` to the browser, so as long as this
/// loop keeps draining, the engine never sees a failed send and never cancels.
/// Stopping the drain — or propagating the `Channel` error back — is what
/// would let a busy browser silently kill a scan.
pub(crate) fn pump_scan_loop(handle: &Arc<Mutex<ScanHandle>>, events: &Channel<ScanEventDto>) {
    let mut batch: Vec<FileEntryDto> = Vec::new();
    // Rows drained from the engine in this pump run. Only used to close the
    // stream honestly if the worker dies without saying Complete.
    let mut drained: usize = 0;

    // Forward one batch. The result is discarded on purpose: a failed
    // `Channel::send` means the webview is gone or too busy to eval, and
    // that must never stop the drain — the engine is still producing into
    // our receiver, and stopping here would drop the handle and cancel it.
    let flush = |batch: &mut Vec<FileEntryDto>| {
        if batch.is_empty() {
            return;
        }
        let dto = ScanEventDto::Entries(std::mem::take(batch));
        let _ = events.send(dto);
    };

    loop {
        let event = lock(handle).recv_timeout(PUMP_WAIT);
        match event {
            Ok(ScanEvent::Entry(entry)) => {
                drained += 1;
                batch.push(FileEntryDto::from_entry(&entry));
                if batch.len() >= MAX_BATCH {
                    flush(&mut batch);
                }
            }
            Ok(ScanEvent::BatchEnd) => flush(&mut batch),
            Ok(ScanEvent::Error(scan_err)) => {
                // Flush first: the rows were produced before the error and
                // the frontend must see them in engine order.
                flush(&mut batch);
                let dto = ScanEventDto::Error {
                    path: scan_err.path.to_string_lossy().into_owned(),
                    kind: CmdError::kind_of_kestrel(&scan_err.error).to_string(),
                    message: scan_err.error.to_string(),
                };
                let _ = events.send(dto);
            }
            Ok(ScanEvent::Complete { total }) => {
                flush(&mut batch);
                let _ = events.send(ScanEventDto::Complete { total });
                break;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Keep the UI painting during slow disks: forward the partial
                // burst rather than holding it for the next BatchEnd.
                flush(&mut batch);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                // The worker is gone without saying Complete (panic, or a
                // cancel that raced completion). Close the stream with what
                // was drained so the frontend cannot wait forever; `total`
                // here counts drained rows, not the engine's own total, which
                // died with the worker.
                flush(&mut batch);
                let _ = events.send(ScanEventDto::Complete { total: drained });
                break;
            }
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

// ---------------------------------------------------------------------------
// watch_subscribe / watch_unsubscribe
// ---------------------------------------------------------------------------

/// Subscribes to live changes for one directory. Returns the backend-minted
/// [`JobId`]; changes arrive on `events` as [`WatchEventDto`] (`changed`
/// carrying the stale directories, `error` carrying a watcher failure). The
/// caller must invoke `watch_unsubscribe` once it navigates away — the relay
/// runs until cancelled, exactly like a scan runs until `scan_cancel`.
///
/// Fails fast (no job minted) when the directory cannot even be `lstat`ed or
/// the platform backend refuses the watch.
///
/// Every subscription shares the ONE long-lived watcher in [`Backend`]:
/// no per-subscription thread is ever spawned, and unsubscribing never drops
/// the watcher, so it never blocks (MIGRATION.md §3).
#[tauri::command]
pub fn watch_subscribe<R: Runtime>(
    app: AppHandle<R>,
    path: String,
    events: Channel<WatchEventDto>,
) -> Result<JobId, CmdError> {
    let dir = PathBuf::from(&path);
    if let Err(e) = std::fs::symlink_metadata(&dir) {
        return Err(CmdError::from_kestrel(&classify_io(&dir, e)));
    }
    let backend = app.state::<Backend>();
    let id = backend.mint();
    let rx = backend
        .add_watch(&dir)
        .map_err(|e| CmdError::from_kestrel(&e))?;
    let cancel = CancellationToken::new();
    let relay_events = Arc::new(Mutex::new(rx));
    backend.register_watch(
        id,
        WatchRelay {
            dir: dir.clone(),
            events: Arc::clone(&relay_events),
            cancel: cancel.clone(),
        },
    );

    // `spawn_blocking`, not `spawn`: the relay is synchronous blocking code
    // (`recv_timeout` loop) and must not stall the async executor.
    // `AppHandle` is `'static`, so state re-resolves inside for the
    // post-loop cleanup — a no-op when `watch_unsubscribe` already ran.
    async_runtime::spawn_blocking(move || {
        pump_watch_loop(&relay_events, &dir, &events, &cancel);
        app.state::<Backend>().cancel_watch(id);
    });
    Ok(id)
}

/// Unsubscribes a watch. Unknown ids are a silent no-op (the relay already
/// exited and cleaned itself up, or the id never existed). Never blocks: see
/// [`Backend::cancel_watch`].
#[tauri::command]
pub fn watch_unsubscribe<R: Runtime>(app: AppHandle<R>, id: JobId) {
    app.state::<Backend>().cancel_watch(id);
}

// ---------------------------------------------------------------------------
// the watch relay
// ---------------------------------------------------------------------------

/// How long one relay iteration waits for the next engine event. Short enough
/// that `watch_unsubscribe` (which sets the cancel token) takes effect
/// promptly, long enough not to spin: with no events the relay idles here.
const WATCH_PUMP_WAIT: Duration = Duration::from_millis(50);

/// Drains one watch relay into its `Channel` until cancelled or the engine's
/// senders go away (backend shutdown).
///
/// Send failures (closed/slow webview) are **ignored on purpose**, exactly
/// like the scan pump: the engine talks `mpsc` to this relay's receiver, not
/// `Channel` to the browser, so as long as this loop keeps draining, the
/// engine never sees a failed send. Stopping the drain — or propagating the
/// `Channel` error back — is what would let a busy browser silently kill the
/// shared watcher for every other subscriber.
pub(crate) fn pump_watch_loop(
    receiver: &Mutex<std::sync::mpsc::Receiver<WatchEvent>>,
    dir: &Path,
    events: &Channel<WatchEventDto>,
    cancel: &CancellationToken,
) {
    loop {
        if cancel.is_cancelled() {
            break;
        }
        let event = lock(receiver).recv_timeout(WATCH_PUMP_WAIT);
        match event {
            Ok(WatchEvent::Changed(changes)) => {
                // The engine broadcasts every change to every subscriber, so
                // most events are for directories this relay does not watch.
                // Forward only what touches our directory; the frontend
                // re-scans when any listed dir is the one it is viewing.
                if !watch_touches(&changes, dir) {
                    continue;
                }
                let mut dirs: Vec<String> = changes
                    .dirs
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect();
                // Guarantee the exact-match the frontend waits for: a fresh
                // `mkdir` may report only the new directory itself (it exists
                // by flush time, so it maps to itself rather than its
                // parent), while the frontend is viewing the parent.
                let dir_str = dir.to_string_lossy().into_owned();
                if !dirs.contains(&dir_str) {
                    dirs.push(dir_str);
                }
                // Discarded on purpose: a failed send means the webview is
                // gone or busy, never a reason to stop draining.
                let _ = events.send(WatchEventDto::Changed { dirs });
            }
            Ok(WatchEvent::Error(err)) => {
                let _ = events.send(WatchEventDto::Error {
                    error: CmdError::from_kestrel(&err),
                });
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Idle: no engine event this window, loop back and re-check
                // the cancel token. This is what keeps a quiet relay from
                // spinning.
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                // The shared watcher is gone (backend shutdown). Nothing left
                // to drain; the post-loop cleanup handles the map entry.
                break;
            }
        }
    }
}

/// Whether an engine change set concerns `dir`: it names the directory
/// itself, or any path inside it (a changed file's parent is `dir`, and a
/// freshly created subdirectory reports its own path, which starts with
/// `dir`). Component-wise [`Path::starts_with`], so `/foo` never matches
/// `/foobar`.
fn watch_touches(changes: &ChangeSet, dir: &Path) -> bool {
    if changes.touches(dir) {
        return true;
    }
    changes.paths.iter().any(|p| p.starts_with(dir))
        || changes.dirs.iter().any(|d| d.starts_with(dir))
}

// ---------------------------------------------------------------------------
// stat
// ---------------------------------------------------------------------------

/// Stats one path. Symlink targets are resolved (one extra `stat`, on this
/// command's thread, not the UI's) so `isDirTarget` matches what a scan row
/// carries.
#[tauri::command]
pub fn stat(path: String) -> Result<FileEntryDto, CmdError> {
    FileEntry::from_path(&path)
        .map(|entry| FileEntryDto::from_entry(&entry.resolve_link_target()))
        .map_err(|e| CmdError::from_kestrel(&e))
}

// ---------------------------------------------------------------------------
// open_path
// ---------------------------------------------------------------------------

/// Implements `Enter` on a row: a directory (or a symlink to one) answers
/// `{ entered_dir: true }` and the frontend navigates — nothing is launched.
/// Anything else is handed to its default application off-thread; success
/// answers `{ entered_dir: false }`, and every failure mode (no handler,
/// broken handler, spawn refused, lost worker) answers `Err(CmdError)`.
#[tauri::command]
pub async fn open_path(path: String) -> Result<OpenResultDto, CmdError> {
    let entry = FileEntry::from_path(&path)
        .map(|entry| entry.resolve_link_target())
        .map_err(|e| CmdError::from_kestrel(&e))?;
    if entry.is_descendable() {
        return Ok(OpenResultDto { entered_dir: true });
    }
    let worker_path = PathBuf::from(&path);
    async_runtime::spawn_blocking(move || wait_for_open(&worker_path))
        .await
        .unwrap_or_else(|_| {
            Err(CmdError::custom(
                "worker_lost",
                Some(Path::new(&path)),
                format!("opening {path} never reported back"),
            ))
        })
}

/// Polls an engine `Opening` to completion without blocking the async
/// executor (runs inside `spawn_blocking`).
fn wait_for_open(path: &Path) -> Result<OpenResultDto, CmdError> {
    let mut opening = kestrel_fs::open::open(path);
    let deadline = Instant::now() + OPEN_TIMEOUT;
    loop {
        if let Some(answer) = opening.poll() {
            return match answer {
                Ok(_) => Ok(OpenResultDto { entered_dir: false }),
                Err(e) => Err(CmdError::from_open(&e)),
            };
        }
        if Instant::now() >= deadline {
            return Err(CmdError::custom(
                "worker_lost",
                Some(path),
                format!("gave up working out how to open {}", path.to_string_lossy()),
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use kestrel_fs::scan::ScanOptions;
    use std::fs;
    use tauri::ipc::InvokeResponseBody;

    fn big_dir(files: usize) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        for i in 0..files {
            fs::write(tmp.path().join(format!("f{i:05}")), b"x").expect("write");
        }
        tmp
    }

    /// Runs the real [`pump_scan_loop`] over a real engine scan with a
    /// `Channel` whose delivery closure is scripted by the test — no Tauri
    /// runtime involved (`Channel::new` delivers into the closure directly).
    /// Returns the raw JSON messages, i.e. exactly what the frontend would
    /// receive over IPC.
    fn run_pump_collect(
        dir: &Path,
        options: ScanOptions,
        on_message: impl Fn(&serde_json::Value) -> tauri::Result<()> + Send + Sync + 'static,
    ) -> (Vec<serde_json::Value>, Arc<Mutex<ScanHandle>>) {
        let handle = scan::start(dir, options).expect("spawn");
        let shared = Arc::new(Mutex::new(handle));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_clone = Arc::clone(&seen);
        let events: Channel<ScanEventDto> = Channel::new(move |body| {
            let json = match &body {
                InvokeResponseBody::Json(s) => s.clone(),
                InvokeResponseBody::Raw(_) => panic!("DTOs must encode as JSON"),
            };
            let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
            seen_clone.lock().expect("lock").push(value.clone());
            on_message(&value)
        });
        pump_scan_loop(&shared, &events);
        drop(events); // releases the closure's `seen` clone
        let seen = Arc::try_unwrap(seen)
            .expect("seen")
            .into_inner()
            .expect("lock");
        (seen, shared)
    }

    fn totals(seen: &[serde_json::Value]) -> (usize, Option<u64>) {
        let entries = seen
            .iter()
            .filter(|v| v["type"] == serde_json::json!("entries"))
            .map(|v| v["entries"].as_array().map_or(0, Vec::len))
            .sum();
        let complete = seen
            .iter()
            .filter(|v| v["type"] == serde_json::json!("complete"))
            .find_map(|v| v["total"].as_u64());
        (entries, complete)
    }

    /// THE load-bearing test: the frontend is gone (every `Channel::send`
    /// fails, as it does when the webview is closed or too busy to eval),
    /// and the pump must still drain the engine to completion.
    ///
    /// Why the assertion is on the *stream*, not the worker summary: the
    /// engine only cancels on failed *mpsc* sends, i.e. a dropped receiver.
    /// The pump holds the receiver throughout, so while the test holds its
    /// `Arc` the worker always finishes — the production harm of a naive
    /// "stop when the frontend is slow" pump is that it returns early, the
    /// post-pump cleanup (`cancel_scan`) drops the last handle, the
    /// receiver dies, and the still-running worker cancels mid-listing.
    /// This test replicates that exactly: run the pump, then drop everything
    /// like the cleanup does, and require that the pump attempted the *whole*
    /// stream — every row and the `complete` — before returning. A pump that
    /// bails on the first failed send leaves a truncated stream and no
    /// `complete`, and fails here.
    #[test]
    fn absent_consumer_does_not_cancel_a_scan() {
        let tmp = big_dir(2000);
        let failing: fn(&serde_json::Value) -> tauri::Result<()> =
            |_| Err(tauri::Error::Io(io_error()));
        let (seen, shared) = run_pump_collect(tmp.path(), ScanOptions::listing(), failing);
        // Production post-pump cleanup: the job leaves the map and the last
        // handle drops, closing the receiver.
        drop(shared);
        let (entries, complete) = totals(&seen);
        assert_eq!(
            complete,
            Some(2000),
            "the pump must drain to the engine's complete even with nobody \
             listening — a truncated stream means a busy browser killed the scan"
        );
        assert_eq!(
            entries, 2000,
            "every row must be attempted even when every send fails"
        );
    }

    fn io_error() -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::NotConnected, "webview gone")
    }

    /// A slow browser (5 ms per message) must see every row and the completion.
    #[test]
    fn slow_consumer_sees_every_row_and_completion() {
        let tmp = big_dir(500);
        let (seen, _) = run_pump_collect(tmp.path(), ScanOptions::listing(), |_| {
            std::thread::sleep(Duration::from_millis(5));
            Ok(())
        });
        let (entries, complete) = totals(&seen);
        assert_eq!(complete, Some(500), "completion must arrive");
        assert_eq!(entries, 500, "no row may be lost to a slow consumer");
    }

    /// A healthy consumer gets batches in order, then exactly one completion
    /// whose total matches the rows delivered.
    #[test]
    fn healthy_consumer_gets_ordered_batches_then_complete() {
        let tmp = big_dir(600);
        let (seen, _) = run_pump_collect(tmp.path(), ScanOptions::listing(), |_| Ok(()));
        let (entries, complete) = totals(&seen);
        assert_eq!(entries, 600);
        assert_eq!(complete, Some(600));
        assert_eq!(
            seen.iter()
                .filter(|v| v["type"] == serde_json::json!("complete"))
                .count(),
            1,
            "exactly one completion"
        );
        // Batches, not one message per row: ~600 rows must not cost 600 IPC sends.
        let batches = seen
            .iter()
            .filter(|v| v["type"] == serde_json::json!("entries"))
            .count();
        assert!(
            batches < 20,
            "rows must be coalesced, got {batches} batches"
        );
    }

    /// `scan_cancel` on a missing id is silent; the `Backend` helper is what
    /// the command calls, so testing it covers the command logic.
    #[test]
    fn backend_cancel_scan_is_race_safe() {
        let backend = Backend::try_new_watching(std::env::temp_dir(), Duration::from_millis(1))
            .expect("backend");
        backend.cancel_scan(12345); // never existed: no panic, no error
    }

    #[test]
    fn stat_returns_a_row_for_a_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file = tmp.path().join("note.txt");
        fs::write(&file, b"hi").expect("write");
        let dto = stat(file.to_string_lossy().into_owned()).expect("stat");
        assert_eq!(dto.name, "note.txt");
        assert_eq!(dto.kind, "file");
        assert_eq!(dto.size, Some(2));
    }

    #[test]
    fn stat_reports_missing_as_not_found() {
        let err = stat("/definitely/not/here-ever".to_string()).expect_err("missing");
        assert_eq!(err.kind, "not_found");
        assert!(err.path.is_some());
    }

    #[test]
    fn open_path_enters_a_directory_without_launching() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let rt = tokio_test_runtime();
        let out = rt.block_on(open_path(tmp.path().to_string_lossy().into_owned()));
        assert!(
            out.expect("dir enters").entered_dir,
            "a directory must navigate, never launch"
        );
    }

    #[test]
    fn open_path_reports_missing_as_not_found() {
        let rt = tokio_test_runtime();
        let err = rt
            .block_on(open_path("/definitely/not/here-ever".to_string()))
            .expect_err("missing");
        assert_eq!(err.kind, "not_found");
    }

    /// Minimal single-thread Tokio runtime for driving the async `open_path`
    /// in tests without involving Tauri at all.
    fn tokio_test_runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("test runtime")
    }

    // ------------------------------------------------------------------
    // watch relay tests (Phase 2)
    // ------------------------------------------------------------------

    use crate::dto::WatchEventDto;
    use crate::state::WatchRelay;
    use kestrel_fs::model::CancellationToken;
    use kestrel_fs::watcher::WatchEvent;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn watch_backend() -> Backend {
        Backend::try_new_watching(std::env::temp_dir(), Duration::from_millis(50)).expect("backend")
    }

    fn register_watch_on(backend: &Backend, dir: &Path) -> (JobId, CancellationToken) {
        let rx = backend.add_watch(dir).expect("add_watch");
        let cancel = CancellationToken::new();
        let id = backend.mint();
        backend.register_watch(
            id,
            WatchRelay {
                dir: dir.to_path_buf(),
                events: Arc::new(Mutex::new(rx)),
                cancel: cancel.clone(),
            },
        );
        (id, cancel)
    }

    /// Builds a `Channel` that records every message as JSON (exactly what the
    /// frontend would receive) and answers each send with `respond`.
    fn recording_channel(
        seen: &Arc<Mutex<Vec<serde_json::Value>>>,
        respond: impl Fn() -> tauri::Result<()> + Send + Sync + 'static,
    ) -> Channel<WatchEventDto> {
        let seen_clone = Arc::clone(seen);
        Channel::new(move |body| {
            let json = match &body {
                InvokeResponseBody::Json(s) => s.clone(),
                InvokeResponseBody::Raw(_) => panic!("DTOs must encode as JSON"),
            };
            let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
            seen_clone.lock().expect("lock").push(value);
            respond()
        })
    }

    fn wait_for_changed(
        seen: &Arc<Mutex<Vec<serde_json::Value>>>,
        what: &str,
    ) -> serde_json::Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            {
                let guard = seen.lock().expect("lock");
                if let Some(v) = guard
                    .iter()
                    .find(|v| v["type"] == serde_json::json!("changed"))
                {
                    return v.clone();
                }
            }
            assert!(
                Instant::now() < deadline,
                "no changed event within the deadline ({what}): {:?}",
                seen.lock().expect("lock")
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Each subscription mints a distinct id; unsubscribing an unknown id (or
    /// twice) is a silent no-op, matching `scan_cancel`'s race safety.
    #[test]
    fn watch_ids_are_unique_and_unsubscribe_is_race_safe() {
        let backend = watch_backend();
        let a = tempfile::tempdir().expect("tempdir");
        let b = tempfile::tempdir().expect("tempdir");
        let (id_a, _) = register_watch_on(&backend, a.path());
        let (id_b, _) = register_watch_on(&backend, b.path());
        assert_ne!(id_a, id_b, "minted watch ids must be unique");
        assert_ne!(id_a, 0);
        assert_ne!(id_b, 0);
        backend.cancel_watch(id_a);
        // Racing cancels and unknown ids are silent no-ops, never panics.
        backend.cancel_watch(id_a);
        backend.cancel_watch(u64::MAX);
        // The sibling subscription survives its neighbour's cancel.
        assert!(lock(&backend.watches).contains_key(&id_b));
        backend.cancel_watch(id_b);
        assert!(lock(&backend.watches).is_empty());
    }

    /// THE load-bearing test, mirroring
    /// `absent_consumer_does_not_cancel_a_scan`: the frontend is gone (every
    /// `Channel::send` fails, as when the webview is closed or too busy to
    /// eval), and the relay must still forward attempts without stopping the
    /// shared watcher.
    #[test]
    fn absent_consumer_does_not_stop_the_watcher() {
        let backend = watch_backend();
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().to_path_buf();
        let rx = backend.add_watch(&dir).expect("add_watch");
        let relay_events = Arc::new(Mutex::new(rx));
        let cancel = CancellationToken::new();

        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = Arc::clone(&attempts);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let events = recording_channel(&seen, move || {
            attempts_clone.fetch_add(1, Ordering::SeqCst);
            Err(tauri::Error::Io(io_error()))
        });

        let cancel_clone = cancel.clone();
        let dir_clone = dir.clone();
        let pump = std::thread::spawn(move || {
            pump_watch_loop(&relay_events, &dir_clone, &events, &cancel_clone);
        });

        fs::write(tmp.path().join("newfile.txt"), b"x").expect("write");
        let deadline = Instant::now() + Duration::from_secs(10);
        while attempts.load(Ordering::SeqCst) == 0 {
            assert!(
                Instant::now() < deadline,
                "the relay never attempted a send, even with nobody listening"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        // The shared watcher is still alive: an independent subscriber sees
        // the next change. A relay that propagated the frontend failure into
        // the engine (e.g. by dropping the watcher) fails here.
        let probe = lock(&backend.watcher).subscribe();
        fs::write(tmp.path().join("second.txt"), b"y").expect("write");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match probe.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(WatchEvent::Changed(_)) => break,
                Ok(WatchEvent::Error(_)) => {}
                Err(e) => panic!("shared watcher stopped delivering: {e}"),
            }
        }

        cancel.cancel();
        let started = Instant::now();
        pump.join().expect("pump thread joins");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "a cancelled relay must exit promptly, not spin"
        );
    }

    /// A relay forwards an engine change as exactly one `changed` event whose
    /// `dirs` carries the watched directory.
    #[test]
    fn relay_forwards_a_change_as_one_changed_event() {
        let backend = watch_backend();
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().to_path_buf();
        let rx = backend.add_watch(&dir).expect("add_watch");
        let relay_events = Arc::new(Mutex::new(rx));
        let cancel = CancellationToken::new();

        let seen = Arc::new(Mutex::new(Vec::new()));
        let events = recording_channel(&seen, || Ok(()));
        let cancel_clone = cancel.clone();
        let dir_clone = dir.clone();
        let pump = std::thread::spawn(move || {
            pump_watch_loop(&relay_events, &dir_clone, &events, &cancel_clone);
        });

        fs::write(tmp.path().join("newfile.txt"), b"x").expect("write");
        let first = wait_for_changed(&seen, "write a file");
        let dirs = first["dirs"]
            .as_array()
            .expect("changed carries a dirs array");
        assert!(
            dirs.iter()
                .any(|d| d.as_str() == Some(dir.to_string_lossy().as_ref())),
            "changed must carry the watched directory, got {first}"
        );

        // One coalesced event, not one per inotify report: after several
        // debounce windows there must still be exactly one `changed`.
        std::thread::sleep(Duration::from_millis(500));
        cancel.cancel();
        pump.join().expect("pump thread joins");
        let guard = seen.lock().expect("lock");
        let changed = guard
            .iter()
            .filter(|v| v["type"] == serde_json::json!("changed"))
            .count();
        assert_eq!(changed, 1, "one change must forward as exactly one event");
    }

    /// A freshly created subdirectory may report only itself (it exists by
    /// flush time, so the engine maps it to itself rather than its parent).
    /// The relay must still carry the watched parent, or the frontend's
    /// exact-match rescan would miss every `mkdir`.
    #[test]
    fn relay_forwards_a_new_directory_carrying_the_watched_parent() {
        let backend = watch_backend();
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().to_path_buf();
        let rx = backend.add_watch(&dir).expect("add_watch");
        let relay_events = Arc::new(Mutex::new(rx));
        let cancel = CancellationToken::new();

        let seen = Arc::new(Mutex::new(Vec::new()));
        let events = recording_channel(&seen, || Ok(()));
        let cancel_clone = cancel.clone();
        let dir_clone = dir.clone();
        let pump = std::thread::spawn(move || {
            pump_watch_loop(&relay_events, &dir_clone, &events, &cancel_clone);
        });

        fs::create_dir(tmp.path().join("newdir")).expect("mkdir");
        let first = wait_for_changed(&seen, "mkdir");
        let dirs = first["dirs"]
            .as_array()
            .expect("changed carries a dirs array");
        assert!(
            dirs.iter()
                .any(|d| d.as_str() == Some(dir.to_string_lossy().as_ref())),
            "mkdir must still carry the watched parent, got {first}"
        );
        cancel.cancel();
        pump.join().expect("pump thread joins");
    }

    /// `DirWatcher::drop` joins its debouncer thread, so unsubscribing must
    /// never drop the shared watcher: it has to be quick on the calling task.
    #[test]
    fn watch_unsubscribe_does_not_block_the_calling_task() {
        let backend = watch_backend();
        let tmp = tempfile::tempdir().expect("tempdir");
        let (id, _) = register_watch_on(&backend, tmp.path());
        let started = Instant::now();
        backend.cancel_watch(id);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "unsubscribe must not block the caller"
        );
        assert!(!lock(&backend.watches).contains_key(&id));
    }
}
