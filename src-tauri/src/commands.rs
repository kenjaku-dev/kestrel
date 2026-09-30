//! Phase 1 IPC commands: `scan_start`, `scan_cancel`, `stat`, `open_path`.
//!
//! Final signatures (Tauri-injected `AppHandle` / `State` params are invisible
//! to JS; the JS-visible contract is the remaining params, all camelCase):
//!
//! ```ts
//! scan_start(path: string, options: ScanOptionsDto, events: Channel<ScanEventDto>): Promise<number>
//! scan_cancel(id: number): Promise<void>
//! stat(path: string): Promise<FileEntryDto>
//! open_path(path: string): Promise<{ enteredDir: boolean }>
//! ```
//!
//! No blocking `list_dir`: the streaming `scan_start` with
//! `recursive: false` **is** the listing, so the two UIs share one code path.
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

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use kestrel_fs::error::classify_io;
use kestrel_fs::model::FileEntry;
use kestrel_fs::scan::{self, ScanEvent, ScanHandle};
use tauri::{AppHandle, Manager, Runtime, async_runtime, ipc::Channel};

use crate::dto::{CmdError, FileEntryDto, OpenResultDto, ScanEventDto, ScanOptionsDto};
use crate::state::{Backend, JobId};

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
}
