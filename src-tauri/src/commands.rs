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

use kestrel_fs::KestrelError;
use kestrel_fs::error::classify_io;
use kestrel_fs::model::{CancellationToken, FileEntry};
use kestrel_fs::ops::{self, Collision, CopyOptions, MoveStrategy, ProgressEvent, ProgressSink};
use kestrel_fs::scan::{self, ScanEvent, ScanHandle};
use kestrel_fs::size::{self, SizeEvent, SizeOptions};
use kestrel_fs::watcher::{ChangeSet, WatchEvent};
use tauri::{AppHandle, Manager, Runtime, async_runtime, ipc::Channel};

use crate::dto::{
    CmdError, FileEntryDto, OpEventDto, OpPhase, OpRequestDto, OpenResultDto, ScanEventDto,
    ScanOptionsDto, WatchEventDto,
};
use crate::state::{Backend, JobId, OpJob, WatchRelay};

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
// op_start / op_cancel (Phase 3a)
//
// ```ts
// op_start(req: OpRequestDto, progress: Channel<OpEventDto>): Promise<number>
// op_cancel(id: number): Promise<void>
// ```
//
// Delivery posture is the scan/watch pumps': the worker drains the engine and
// forwards to the `Channel` **without ever blocking on the frontend**, and
// every `Channel::send` result is discarded with a comment saying why. A busy
// or gone frontend must not cancel a file operation midway — a dropped send
// during a half-finished copy is exactly the case where the op keeps going.
// ---------------------------------------------------------------------------

/// How often the op worker forwards engine progress to the frontend. The
/// engine already throttles its own sink; this second throttle bounds IPC when
/// an op touches thousands of files (one engine emit per file) — without it a
/// big tree would cost one `webview.eval` per file.
const OP_FORWARD_INTERVAL: Duration = Duration::from_millis(100);

/// Starts a mutating operation (copy / move / trash / delete). Returns the
/// backend-minted [`JobId`]; progress and exactly one terminal event (`done`
/// or `error`) arrive on `progress` as [`OpEventDto`]. The caller must invoke
/// `op_cancel` once it no longer cares — cancelling stops the worker at the
/// next chunk boundary, leaving a partial destination and reporting
/// `Cancelled` (a cancelled move never deletes the source).
///
/// There is no fail-fast validation: every outcome, including a missing source
/// or a collision, arrives as an event, so the frontend has exactly one error
/// path to handle. A collision under the Phase 3a policy
/// ([`Collision::Fail`]) is an `error` event with kind `"already_exists"`.
#[tauri::command]
pub fn op_start<R: Runtime>(
    app: AppHandle<R>,
    req: OpRequestDto,
    progress: Channel<OpEventDto>,
) -> Result<JobId, CmdError> {
    let backend = app.state::<Backend>();
    let id = backend.mint();
    let cancel = CancellationToken::new();
    backend.register_op(
        id,
        Arc::new(OpJob {
            cancel: cancel.clone(),
            description: describe_op(&req),
        }),
    );

    // `spawn_blocking`, not `spawn`: the worker is synchronous blocking code
    // and must not stall the async executor. `AppHandle` is `'static`, so
    // state re-resolves inside for the completion cleanup — a no-op when
    // `op_cancel` already ran.
    async_runtime::spawn_blocking(move || {
        run_op_blocking(id, &req, &cancel, &progress);
        app.state::<Backend>().cancel_op(id);
    });
    Ok(id)
}

/// Cancels an op. Unknown ids are a silent no-op (the op already finished and
/// cleaned itself up, or the id never existed) — the same posture as
/// `scan_cancel` and `watch_unsubscribe`. Never blocks: see
/// [`Backend::cancel_op`].
#[tauri::command]
pub fn op_cancel<R: Runtime>(app: AppHandle<R>, id: JobId) {
    app.state::<Backend>().cancel_op(id);
}

/// Runs one op to exactly one terminal event (`done` or `error`) on `events`.
///
/// This is the whole body of [`op_start`]'s worker, factored out so tests can
/// drive it with a [`Channel`] recorder and no Tauri runtime — exactly like
/// `pump_scan_loop`. Public so out-of-tree probes can exercise the same path
/// the command spawns.
pub fn run_op_blocking(
    id: JobId,
    req: &OpRequestDto,
    cancel: &CancellationToken,
    events: &Channel<OpEventDto>,
) {
    match req {
        OpRequestDto::Copy { src, dst } => run_copylike(id, src, dst, cancel, events, false),
        OpRequestDto::Move { src, dst } => run_copylike(id, src, dst, cancel, events, true),
        OpRequestDto::Trash { src } => match ops::trash(src) {
            Ok(()) => send_done(id, events, format!("trashed {}", short_name(src))),
            Err(e) => send_error(id, events, &e),
        },
        OpRequestDto::Delete { src, recursive } => {
            if *recursive {
                let path = PathBuf::from(src);
                let forward = Forwarder::new(events.clone(), id, OpPhase::Deleting, 0, 0);
                let sink: ProgressSink = Arc::new(move |ev| forward.push(&ev));
                match ops::delete_recursive_with_progress(&path, Some(cancel), Some(&sink)) {
                    Ok(()) => send_done(id, events, format!("deleted {}", short_name(src))),
                    Err(e) => send_error(id, events, &e),
                }
            } else {
                match ops::delete(src) {
                    Ok(()) => send_done(id, events, format!("deleted {}", short_name(src))),
                    Err(e) => send_error(id, events, &e),
                }
            }
        }
    }
}

/// Shared body for copy and move: measure, then run, then one terminal event.
fn run_copylike(
    id: JobId,
    src: &str,
    dst: &str,
    cancel: &CancellationToken,
    events: &Channel<OpEventDto>,
    is_move: bool,
) {
    let src_path = PathBuf::from(src);
    let dst_path = PathBuf::from(dst);
    let Some((total_bytes, total_items)) = measure_source(&src_path, id, cancel, events) else {
        send_error(id, events, &KestrelError::Cancelled);
        return;
    };
    let forward = Forwarder::new(
        events.clone(),
        id,
        OpPhase::Copying,
        total_bytes,
        total_items,
    );
    let sink: ProgressSink = Arc::new(move |ev| forward.push(&ev));
    // Phase 3a has no collision dialog yet, so the policy is `Fail`: a
    // collision surfaces as `AlreadyExists` and the op fails without writing.
    // Phase 3b adds the question round-trip; nothing here changes shape for it.
    let options = CopyOptions {
        collision: Collision::Fail,
        answered: ops::AnsweredCollisions::new(),
        preserve_permissions: false,
        cancel: Some(cancel.clone()),
        progress: Some(sink),
    };
    let verb = if is_move { "moved" } else { "copied" };
    let result = if is_move {
        ops::move_(&src_path, &dst_path, options, MoveStrategy::Auto)
    } else {
        ops::copy(&src_path, &dst_path, options)
    };
    match result {
        Ok(()) => send_done(id, events, format!("{verb} {}", short_name(src))),
        Err(e) => send_error(id, events, &e),
    }
}

/// Best-effort pre-measurement of `src` for the `totalBytes`/`totalItems` of
/// copy/move progress, mirroring the egui `job.rs` approach: measure with
/// [`size::compute_blocking`](size::compute_blocking), then copy.
///
/// Returns `None` when cancelled (the caller reports `Cancelled`). Any other
/// failure returns `(0, 0)` and lets the op itself surface the real error —
/// the measure must never fail an op the copy would have explained better.
/// Unknown shapes (symlinks, special files) are `(0, 0)`: indeterminate, and
/// the frontend shows them as such rather than as a guessed 90%.
fn measure_source(
    src: &Path,
    id: JobId,
    cancel: &CancellationToken,
    events: &Channel<OpEventDto>,
) -> Option<(u64, usize)> {
    if cancel.is_cancelled() {
        return None;
    }
    let send = |done_bytes: u64, total_bytes: u64, done_items: usize, total_items: usize| {
        // Discarded on purpose, like every other pump: a failed send means the
        // webview is gone or busy, never a reason to stop measuring.
        let _ = events.send(OpEventDto::Progress {
            id,
            phase: OpPhase::Measuring,
            done_bytes,
            total_bytes,
            done_items,
            total_items,
        });
    };
    let metadata = match std::fs::symlink_metadata(src) {
        Ok(md) => md,
        Err(_) => return Some((0, 0)),
    };
    if metadata.is_file() {
        let total = metadata.len();
        send(total, total, 1, 1);
        return Some((total, 1));
    }
    if !metadata.is_dir() {
        return Some((0, 0));
    }
    // A directory: walk it with the engine's own measurer, forwarding its
    // (already throttled) progress as measuring-phase events.
    let mut sink = |event: SizeEvent| -> bool {
        if cancel.is_cancelled() {
            return false;
        }
        if let SizeEvent::Progress { bytes, files, .. } = event {
            send(bytes, 0, files, 0);
        }
        // `true` unconditionally: a busy frontend must not stop the walk (the
        // `send` above already discarded its result for exactly this reason).
        true
    };
    match size::compute_blocking(src, SizeOptions::default(), cancel, &mut sink) {
        Ok(Some(size)) => {
            send(size.logical, size.logical, size.files, size.files);
            Some((size.logical, size.files))
        }
        Ok(None) => None,
        Err(_) => Some((0, 0)),
    }
}

/// Forwards engine [`ProgressEvent`]s to the frontend channel.
///
/// Throttled to [`OP_FORWARD_INTERVAL`], except the first event for the phase,
/// which always goes through: a phase transition (`measuring` → `copying`) is
/// itself information, and without the exception a fast op's only `copying`
/// event could be swallowed by the throttle. `Clone` so the engine sink can
/// own one. (No `Debug`: `Channel` has none, and there is nothing to log
/// about a forwarder anyway.)
#[derive(Clone)]
struct Forwarder {
    events: Channel<OpEventDto>,
    id: JobId,
    phase: OpPhase,
    total_bytes: u64,
    total_items: usize,
    last: Arc<Mutex<(Instant, Option<OpPhase>)>>,
}

impl Forwarder {
    fn new(
        events: Channel<OpEventDto>,
        id: JobId,
        phase: OpPhase,
        total_bytes: u64,
        total_items: usize,
    ) -> Self {
        Self {
            events,
            id,
            phase,
            total_bytes,
            total_items,
            last: Arc::new(Mutex::new((Instant::now(), None))),
        }
    }

    fn push(&self, ev: &ProgressEvent) {
        let mut guard = lock(&self.last);
        if guard.1 != Some(self.phase) || guard.0.elapsed() >= OP_FORWARD_INTERVAL {
            *guard = (Instant::now(), Some(self.phase));
            // Discarded on purpose: a failed send means the webview is gone or
            // busy, never a reason to stop the op — the engine keeps producing
            // into our sink regardless.
            let _ = self.events.send(OpEventDto::Progress {
                id: self.id,
                phase: self.phase,
                done_bytes: ev.bytes_copied,
                total_bytes: self.total_bytes,
                done_items: ev.files_done,
                total_items: self.total_items,
            });
        }
    }
}

/// Sends the success terminal. The result is discarded: see [`pump_scan_loop`].
fn send_done(id: JobId, events: &Channel<OpEventDto>, summary: String) {
    let _ = events.send(OpEventDto::Done { id, summary });
}

/// Sends the failure/cancel terminal. The result is discarded: see
/// [`pump_scan_loop`].
fn send_error(id: JobId, events: &Channel<OpEventDto>, err: &KestrelError) {
    let _ = events.send(OpEventDto::Error {
        id,
        error: CmdError::from_kestrel(err),
    });
}

/// A human sentence for the op registry ("Copy a → b").
fn describe_op(req: &OpRequestDto) -> String {
    match req {
        OpRequestDto::Copy { src, dst } => {
            format!("Copy {} → {}", short_name(src), short_name(dst))
        }
        OpRequestDto::Move { src, dst } => {
            format!("Move {} → {}", short_name(src), short_name(dst))
        }
        OpRequestDto::Trash { src } => format!("Trash {}", short_name(src)),
        OpRequestDto::Delete { src, .. } => format!("Delete {}", short_name(src)),
    }
}

/// The file name, or the full path when there is none.
fn short_name(path: &str) -> &str {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
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

    // ------------------------------------------------------------------
    // op tests (Phase 3a)
    // ------------------------------------------------------------------

    use crate::dto::{OpEventDto, OpRequestDto};
    use crate::state::OpJob;

    /// Scratch root for the tests that need gigabytes.
    ///
    /// `/tmp` is a 4 GB tmpfs on this host, so a 1 GB source *plus* its 1 GB
    /// destination would live in RAM and evict the page cache out from under
    /// every other test running in parallel. Prefer a real-disk root; let CI
    /// override with `KESTREL_TEST_TMPDIR`.
    ///
    /// Every path built from this is still a [`tempfile::TempDir`]: nothing
    /// outside one is ever written, and each test cleans up after itself.
    fn big_test_root() -> PathBuf {
        if let Some(dir) = std::env::var_os("KESTREL_TEST_TMPDIR") {
            return PathBuf::from(dir);
        }
        ["/var/tmp", "/home"]
            .into_iter()
            .map(PathBuf::from)
            .find(|p| p.is_dir())
            .unwrap_or_else(std::env::temp_dir)
    }

    /// A gigabyte-scale tempdir on real disk (see [`big_test_root`]).
    fn big_tempdir() -> tempfile::TempDir {
        let root = big_test_root();
        std::fs::create_dir_all(&root).expect("scratch root must be creatable");
        tempfile::Builder::new()
            .prefix("kestrel-op-")
            .tempdir_in(&root)
            .unwrap_or_else(|e| panic!("cannot make a tempdir under {}: {e}", root.display()))
    }

    /// Writes `len` bytes to `path` in 1 MiB chunks (a single `vec!` of a
    /// gigabyte would spike RSS by a gigabyte for no reason).
    fn write_mb_file(path: &Path, megabytes: usize) -> u64 {
        use std::io::Write;
        let chunk = vec![0x5Au8; 1024 * 1024];
        let mut f = fs::File::create(path).expect("create");
        for _ in 0..megabytes {
            f.write_all(&chunk).expect("write");
        }
        drop(f);
        megabytes as u64 * 1024 * 1024
    }

    /// Runs `run_op_blocking` on a worker thread with a cancel token the test
    /// holds, so the cancel can be delivered *mid-flight* rather than before
    /// the op starts. `on_first_destination_byte` runs on the main thread and
    /// is expected to cancel as soon as `probe` shows the destination
    /// starting to land — that is what makes the partial real.
    fn run_op_cancel_midflight(
        req: OpRequestDto,
        probe: PathBuf,
        min_bytes: u64,
    ) -> Vec<serde_json::Value> {
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let events = op_recording_channel(&seen, || Ok(()));
        let worker = std::thread::spawn(move || {
            run_op_blocking(7, &req, &worker_cancel, &events);
            // `events` drops here, releasing the recorder's `seen` clone.
        });
        let started = Instant::now();
        loop {
            if fs::metadata(&probe).is_ok_and(|m| m.len() >= min_bytes) {
                cancel.cancel();
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(60),
                "the destination never reached {min_bytes} bytes: a test that cannot \
                 land a cancel mid-flight is not testing what it claims"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        worker.join().expect("op worker joins");
        Arc::try_unwrap(seen)
            .expect("seen")
            .into_inner()
            .expect("lock")
    }

    /// The one terminal event of a stream, with its kind.
    fn terminal(seen: &[serde_json::Value]) -> (&str, serde_json::Value) {
        let terminals: Vec<&serde_json::Value> = seen
            .iter()
            .filter(|v| {
                v["type"] == serde_json::json!("done") || v["type"] == serde_json::json!("error")
            })
            .collect();
        assert_eq!(
            terminals.len(),
            1,
            "exactly one terminal event per op (MIGRATION.md §3): {seen:?}"
        );
        let last = seen.last().expect("a non-empty stream");
        assert_eq!(
            last, terminals[0],
            "the terminal event must be last: a progress event after it means the \
             relay emitted past the end of the op"
        );
        (
            terminals[0]["type"].as_str().expect("type"),
            terminals[0].clone(),
        )
    }

    /// Every `progress` event, in delivery order.
    fn progress_events(seen: &[serde_json::Value]) -> Vec<&serde_json::Value> {
        seen.iter()
            .filter(|v| v["type"] == serde_json::json!("progress"))
            .collect()
    }

    /// Builds a `Channel` that records every op message as JSON (exactly what
    /// the frontend would receive) and answers each send with `respond`.
    fn op_recording_channel(
        seen: &Arc<Mutex<Vec<serde_json::Value>>>,
        respond: impl Fn() -> tauri::Result<()> + Send + Sync + 'static,
    ) -> Channel<OpEventDto> {
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

    /// Runs the real [`run_op_blocking`] with a healthy consumer and returns
    /// the raw JSON messages, i.e. exactly what the frontend would receive.
    fn run_op_collect(req: OpRequestDto) -> Vec<serde_json::Value> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let events = op_recording_channel(&seen, || Ok(()));
        run_op_blocking(7, &req, &CancellationToken::new(), &events);
        drop(events);
        Arc::try_unwrap(seen)
            .expect("seen")
            .into_inner()
            .expect("lock")
    }

    fn copy_req(src: &Path, dst: &Path) -> OpRequestDto {
        OpRequestDto::Copy {
            src: src.to_string_lossy().into_owned(),
            dst: dst.to_string_lossy().into_owned(),
        }
    }

    /// Each op mints a distinct id; cancelling an unknown id (or twice) is a
    /// silent no-op, matching `scan_cancel` / `watch_unsubscribe`.
    #[test]
    fn op_ids_are_unique_and_cancel_is_race_safe() {
        let backend = watch_backend();
        let new_job = || {
            Arc::new(OpJob {
                cancel: CancellationToken::new(),
                description: "test op".to_string(),
            })
        };
        let a = backend.mint();
        let b = backend.mint();
        assert_ne!(a, b, "minted op ids must be unique");
        assert_ne!(a, 0);
        backend.register_op(a, new_job());
        backend.register_op(b, new_job());
        backend.cancel_op(a);
        assert!(!lock(&backend.ops).contains_key(&a));
        // Racing cancels and unknown ids are silent no-ops, never panics.
        backend.cancel_op(a);
        backend.cancel_op(u64::MAX);
        // The sibling op survives its neighbour's cancel.
        assert!(lock(&backend.ops).contains_key(&b));
        backend.cancel_op(b);
        assert!(lock(&backend.ops).is_empty());
    }

    /// A copy streams `measuring` → `copying` progress with non-decreasing
    /// byte counts, then exactly one `done` naming the file.
    #[test]
    fn copy_reports_progress_then_done() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("report.pdf");
        fs::write(&src, vec![3u8; 512 * 1024]).expect("write");
        let dst = tmp.path().join("report-copy.pdf");

        let seen = run_op_collect(copy_req(&src, &dst));

        assert_eq!(fs::read(&dst).expect("read").len(), 512 * 1024);
        let phases: Vec<&str> = seen
            .iter()
            .filter(|v| v["type"] == serde_json::json!("progress"))
            .filter_map(|v| v["phase"].as_str())
            .collect();
        assert!(
            phases.contains(&"measuring"),
            "must report a measuring phase, got {seen:?}"
        );
        assert!(
            phases.contains(&"copying"),
            "must report a copying phase, got {seen:?}"
        );
        let mut prev = 0u64;
        for v in seen
            .iter()
            .filter(|v| v["phase"] == serde_json::json!("copying"))
        {
            let done = v["doneBytes"].as_u64().expect("doneBytes is a number");
            assert!(done >= prev, "doneBytes must never go backwards");
            prev = done;
            assert!(v["totalBytes"].as_u64().expect("total") > 0);
            assert_eq!(v["id"], serde_json::json!(7));
        }
        let terminals: Vec<&serde_json::Value> = seen
            .iter()
            .filter(|v| {
                v["type"] == serde_json::json!("done") || v["type"] == serde_json::json!("error")
            })
            .collect();
        assert_eq!(terminals.len(), 1, "exactly one terminal event: {seen:?}");
        assert_eq!(terminals[0]["type"], serde_json::json!("done"));
        assert!(
            terminals[0]["summary"]
                .as_str()
                .is_some_and(|s| s.contains("report.pdf")),
            "the summary must name the file, got {seen:?}"
        );
    }

    /// THE data-safety test: a 3a collision is an `error` event with kind
    /// `already_exists`, and the destination is byte-for-byte unchanged.
    #[test]
    fn collision_surfaces_already_exists_and_keeps_destination() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("new.txt");
        let dst = tmp.path().join("old.txt");
        fs::write(&src, b"new bytes").expect("write");
        fs::write(&dst, b"old bytes stay").expect("write");

        let seen = run_op_collect(copy_req(&src, &dst));

        let terminals: Vec<&serde_json::Value> = seen
            .iter()
            .filter(|v| v["type"] == serde_json::json!("error"))
            .collect();
        assert_eq!(
            terminals.len(),
            1,
            "a collision ends in one error: {seen:?}"
        );
        assert_eq!(
            terminals[0]["error"]["kind"],
            serde_json::json!("already_exists")
        );
        assert_eq!(
            fs::read(&dst).expect("read"),
            b"old bytes stay",
            "the destination must be byte-for-byte unchanged"
        );
        assert!(
            !seen.iter().any(|v| v["type"] == serde_json::json!("done")),
            "a failed op must not claim done"
        );
    }

    /// The engine's self-destination guard must reach the wire as
    /// `invalid_input`, not as a silent success or a panic.
    #[test]
    fn dst_inside_src_reaches_wire_as_invalid_input() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("tree");
        fs::create_dir_all(src.join("inner")).expect("mkdir");
        fs::write(src.join("a.txt"), b"precious").expect("write");
        let dst = src.join("inner/tree");

        let seen = run_op_collect(copy_req(&src, &dst));

        let errors: Vec<&serde_json::Value> = seen
            .iter()
            .filter(|v| v["type"] == serde_json::json!("error"))
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "the guard must surface one error: {seen:?}"
        );
        assert_eq!(
            errors[0]["error"]["kind"],
            serde_json::json!("invalid_input")
        );
        assert_eq!(
            fs::read(src.join("a.txt")).expect("read"),
            b"precious",
            "the source tree must survive"
        );
    }

    /// Mirrors `absent_consumer_does_not_stop_the_watcher`: every
    /// `Channel::send` fails (webview gone or busy) and the op must still run
    /// to completion on disk.
    #[test]
    fn absent_consumer_does_not_cancel_an_op() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("src");
        fs::create_dir_all(src.join("sub")).expect("mkdir");
        fs::write(src.join("a.txt"), b"alpha").expect("write");
        fs::write(src.join("sub/b.txt"), vec![9u8; 1024 * 1024]).expect("write");
        let dst = tmp.path().join("dst");

        let seen = Arc::new(Mutex::new(Vec::new()));
        let events = op_recording_channel(&seen, || Err(tauri::Error::Io(io_error())));
        run_op_blocking(7, &copy_req(&src, &dst), &CancellationToken::new(), &events);

        assert_eq!(fs::read(dst.join("a.txt")).expect("read"), b"alpha");
        assert_eq!(
            fs::read(dst.join("sub/b.txt")).expect("read").len(),
            1024 * 1024,
            "the whole tree must land even with nobody listening"
        );
        assert!(src.join("a.txt").exists(), "the source must be untouched");
    }

    /// A cancelled move reports `cancelled` as an `error` event and never
    /// deletes the source. (The mid-copy proof — source intact beside a
    /// partial destination — is the engine test
    /// `cancelled_move_keeps_the_source_and_leaves_a_partial_destination`;
    /// this is the same path driven through the command layer.)
    #[test]
    fn cancelled_move_reports_cancelled_and_keeps_the_source() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("big.bin");
        fs::write(&src, vec![5u8; 1024 * 1024]).expect("write");
        let dst = tmp.path().join("big.bin.moved");

        let cancel = CancellationToken::new();
        cancel.cancel();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let events = op_recording_channel(&seen, || Ok(()));
        run_op_blocking(
            7,
            &OpRequestDto::Move {
                src: src.to_string_lossy().into_owned(),
                dst: dst.to_string_lossy().into_owned(),
            },
            &cancel,
            &events,
        );
        drop(events);
        let seen = Arc::try_unwrap(seen)
            .expect("seen")
            .into_inner()
            .expect("lock");

        let errors: Vec<&serde_json::Value> = seen
            .iter()
            .filter(|v| v["type"] == serde_json::json!("error"))
            .collect();
        assert_eq!(errors.len(), 1, "a cancel ends in one error: {seen:?}");
        assert_eq!(errors[0]["error"]["kind"], serde_json::json!("cancelled"));
        assert!(
            src.exists(),
            "a cancelled move must never delete the source"
        );
        assert_eq!(fs::read(&src).expect("read").len(), 1024 * 1024);
        assert!(
            !dst.exists(),
            "a pre-cancelled move must not write anything"
        );
    }

    /// A recursive delete streams `deleting` progress (with `totalBytes` 0 —
    /// a delete has no byte total) then one `done`.
    #[test]
    fn recursive_delete_reports_deleting_progress_then_done() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("gone");
        fs::create_dir_all(dir.join("sub")).expect("mkdir");
        fs::write(dir.join("a.txt"), b"a").expect("write");
        fs::write(dir.join("sub/b.txt"), b"b").expect("write");

        let seen = run_op_collect(OpRequestDto::Delete {
            src: dir.to_string_lossy().into_owned(),
            recursive: true,
        });

        assert!(!dir.exists(), "the tree must be gone");
        let progress: Vec<&serde_json::Value> = seen
            .iter()
            .filter(|v| v["type"] == serde_json::json!("progress"))
            .collect();
        assert!(
            progress
                .iter()
                .any(|v| v["phase"] == serde_json::json!("deleting")),
            "must report a deleting phase, got {seen:?}"
        );
        for v in &progress {
            assert_eq!(
                v["totalBytes"],
                serde_json::json!(0),
                "a delete must not invent a byte total"
            );
        }
        let terminals: Vec<&serde_json::Value> = seen
            .iter()
            .filter(|v| v["type"] == serde_json::json!("done"))
            .collect();
        assert_eq!(terminals.len(), 1, "exactly one done: {seen:?}");
    }

    /// Trashing a path that does not exist reports `not_found` — no job, no
    /// progress, one honest error.
    #[test]
    fn trash_of_a_missing_path_reports_not_found() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let seen = run_op_collect(OpRequestDto::Trash {
            src: tmp.path().join("nope").to_string_lossy().into_owned(),
        });
        let errors: Vec<&serde_json::Value> = seen
            .iter()
            .filter(|v| v["type"] == serde_json::json!("error"))
            .collect();
        assert_eq!(errors.len(), 1, "one error: {seen:?}");
        assert_eq!(errors[0]["error"]["kind"], serde_json::json!("not_found"));
    }

    /// EXIT-GATE CLAUSE 1 — "a 1 GB copy shows monotonic progress" — proved
    /// through the **command relay**, not just the engine.
    ///
    /// The engine hook is covered at `kestrel-fs/src/ops.rs`, and
    /// `copy_reports_progress_then_done` covers a 512 KiB copy. Neither
    /// exercises the seam this clause is really about: whether the relay
    /// forwards a gigabyte of progress faithfully on the way to the frontend.
    /// Two throttles sit in that path (the engine's 100 ms and
    /// [`OP_FORWARD_INTERVAL`]'s 100 ms), so the emitted sequence is a
    /// subsample of the engine's — and a throttle is exactly the kind of code
    /// that can reorder, duplicate, or truncate.
    ///
    /// So this asserts the four properties the frontend actually depends on,
    /// over the real [`run_op_blocking`] with a real [`Channel`]:
    ///
    /// 1. **Monotonic** — `doneBytes` never decreases within a phase.
    /// 2. **Not reordered** — the phase sequence only ever advances
    ///    `measuring → copying`, never back.
    /// 3. **Not duplicated** — consecutive progress events are distinct, so
    ///    the relay is not re-sending a stale event.
    /// 4. **Not truncated** — the last `copying` event reaches the real byte
    ///    total, so the frontend's bar can actually complete.
    #[test]
    fn one_gigabyte_copy_streams_monotonic_progress_through_the_relay() {
        const MEGABYTES: usize = 1024;
        let tmp = big_tempdir();
        let src = tmp.path().join("gigabyte.bin");
        let src_len = write_mb_file(&src, MEGABYTES);
        assert_eq!(
            src_len,
            1024 * 1024 * 1024,
            "the clause says 1 GB, not less"
        );
        let dst = tmp.path().join("gigabyte-copy.bin");

        let seen = run_op_collect(copy_req(&src, &dst));

        assert_eq!(
            fs::metadata(&dst).expect("destination").len(),
            src_len,
            "the whole gigabyte must land"
        );

        let progress = progress_events(&seen);
        assert!(
            progress.len() >= 3,
            "a gigabyte must produce a real stream, not one or two events \
             (throttling has gone wrong): {} events",
            progress.len()
        );

        // (1) Monotonic within a phase, and (3) no duplicate event.
        // A phase change legitimately restarts the byte count (measuring
        // hands a known total to copying), so the baseline resets there —
        // exactly the rule `ui/src/lib/ops.ts` applies in `onOpEvent`.
        let mut prev: Option<(&str, u64, u64)> = None;
        for v in &progress {
            let phase = v["phase"].as_str().expect("phase is a string");
            let done = v["doneBytes"].as_u64().expect("doneBytes is a number");
            let items = v["doneItems"].as_u64().expect("doneItems is a number");
            assert_eq!(
                v["id"],
                serde_json::json!(7),
                "every event must carry the one minted id: {v}"
            );
            if let Some((_, pbytes, pitems)) = prev.filter(|(p, ..)| *p == phase) {
                // Same phase: the byte and item counters must both advance.
                assert!(
                    done >= pbytes,
                    "doneBytes went backwards within phase {phase}: \
                     {pbytes} then {done} — {seen:?}"
                );
                assert!(
                    done != pbytes || items != pitems,
                    "a byte-for-byte duplicate progress event reached the wire: {v}"
                );
                assert!(
                    items >= pitems,
                    "doneItems went backwards within phase {phase}: \
                     {pitems} then {items} — {seen:?}"
                );
            }
            prev = Some((phase, done, items));
        }

        // (2) Phases only ever advance. A `measuring` event arriving after a
        // `copying` one is a relay that reordered its own output.
        let order: [&str; 3] = ["measuring", "copying", "deleting"];
        let mut highest = 0usize;
        for v in &progress {
            let phase = v["phase"].as_str().expect("phase");
            let rank = order
                .iter()
                .position(|p| *p == phase)
                .unwrap_or_else(|| panic!("unknown phase {phase} on the wire"));
            assert!(
                rank >= highest,
                "the relay reordered phases: {phase} arrived after rank {highest}"
            );
            highest = rank;
        }

        // (4) Not truncated: the final `copying` event must reach the *exact*
        // byte total, not merely be past halfway. "Past halfway" is what a
        // truncated stream still satisfies — a relay that dropped the last few
        // events would leave the frontend bar stuck at 96% forever, which is
        // precisely the "monotonic but wrong" failure this clause exists to
        // catch. The engine's trailing `finish()` emit guarantees the final
        // event carries the true total, so the relay must forward it.
        let last_copying = progress
            .iter()
            .rev()
            .find(|v| v["phase"] == serde_json::json!("copying"))
            .expect("a copying phase");
        assert_eq!(
            last_copying["totalBytes"],
            serde_json::json!(src_len),
            "totalBytes must be the measured byte total: {last_copying}"
        );
        assert_eq!(
            last_copying["doneBytes"],
            serde_json::json!(src_len),
            "the final copying event must reach the full byte total, so the \
             frontend's bar can actually complete. A truncated relay stops \
             early and the bar stalls short forever: {last_copying}"
        );

        let (kind, done) = terminal(&seen);
        assert_eq!(kind, "done");
        assert!(
            done["summary"]
                .as_str()
                .is_some_and(|s| s.contains("gigabyte.bin")),
            "the summary must name the file: {done}"
        );
    }

    /// EXIT-GATE CLAUSE 1, the "does not drop or duplicate under load" half.
    ///
    /// A throttle that emits *more* than the engine produced is as broken as
    /// one that emits less: the frontend would animate a stall that never
    /// happened. This drives a many-file tree — where the engine emits once
    /// per *file*, unconditionally, bypassing its own time throttle — and
    /// requires the relay's event count to be bounded by elapsed time rather
    /// than by file count. A tree of 4 000 files copied in well under a second
    /// must not cost 4 000 IPC messages.
    #[test]
    fn the_relay_throttles_a_many_file_tree_instead_of_forwarding_every_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("many");
        fs::create_dir(&src).expect("mkdir");
        for i in 0..4_000 {
            fs::write(src.join(format!("f{i:05}")), b"x").expect("write");
        }
        let dst = tmp.path().join("many-copy");

        let started = Instant::now();
        let seen = run_op_collect(copy_req(&src, &dst));
        let elapsed = started.elapsed();

        assert_eq!(
            fs::read_dir(&dst).expect("dst readable").count(),
            4_000,
            "every file must land"
        );
        let progress = progress_events(&seen);
        assert!(
            !progress.is_empty(),
            "a 4 000-file copy must report something: {seen:?}"
        );
        // Count only the `copying` events: the `measuring` walk is throttled
        // by the engine's own `SizeOptions::progress_interval` and is a
        // different code path entirely (see `measure_source`).
        let copying = progress
            .iter()
            .filter(|v| v["phase"] == serde_json::json!("copying"))
            .count();
        assert!(
            copying > 0,
            "a 4 000-file copy must report copying progress: {seen:?}"
        );
        // The engine emits once per *completed file*, unconditionally —
        // bypassing its own time throttle — precisely so a many-small-file
        // copy is visible. The relay's `OP_FORWARD_INTERVAL` is the only
        // thing standing between 4 000 emits and 4 000 `webview.eval` calls.
        //
        // Ceiling: one event per interval that actually elapsed, plus the
        // always-through first event of the phase. Compared against the file
        // count (4 000), which is what a broken throttle converges to.
        let intervals = (elapsed.as_millis() / OP_FORWARD_INTERVAL.as_millis().max(1)) as usize;
        let ceiling = intervals + 4;
        assert!(
            copying <= ceiling,
            "the relay forwarded {copying} copying events for a copy that took \
             {elapsed:?}; the forward interval alone allows {ceiling}. \
             4 000 files emitted 4 000 engine events and the throttle let {} \
             through — one IPC message per file would stall a real webview.",
            copying - ceiling
        );
        assert!(
            copying < 4_000,
            "every single file reached the wire: the forward throttle is dead"
        );
    }

    /// EXIT-GATE CLAUSE 2 — "cancel leaves a partial and reports `Cancelled`"
    /// — proved through the **command layer**.
    ///
    /// The engine guarantees the partial
    /// (`kestrel-fs/src/ops.rs::cancelling_mid_file_stops_the_copy_and_leaves_a_partial_destination`),
    /// and `cancelled_move_reports_cancelled_and_keeps_the_source` covers the
    /// *pre*-cancelled move. What neither covers is the case the clause is
    /// actually about: a cancel that lands **mid-copy**, so the op has written
    /// real bytes, and the command layer must still surface it as a terminal
    /// `cancelled` event rather than a generic failure or silence.
    ///
    /// Both halves are asserted: the partial destination on disk, and the
    /// terminal event's `kind`. A command layer that reported the cancel as
    /// `io` or `unknown` would leave the user with no idea their file is
    /// half-copied.
    #[test]
    fn cancelling_a_copy_mid_flight_leaves_a_partial_and_reports_cancelled() {
        let tmp = big_tempdir();
        let src = tmp.path().join("big.bin");
        // Large enough that the cancel provably lands mid-copy rather than
        // after it: a gigabyte takes ~1s to copy on this disk.
        let src_len = write_mb_file(&src, 768);
        let dst = tmp.path().join("big.bin.copy");

        let seen = run_op_cancel_midflight(copy_req(&src, &dst), dst.clone(), 64 * 1024 * 1024);

        let (kind, terminal) = terminal(&seen);
        assert_eq!(
            kind, "error",
            "a cancelled copy is a terminal error event: {seen:?}"
        );
        assert_eq!(
            terminal["error"]["kind"],
            serde_json::json!("cancelled"),
            "the frontend routes `cancelled` to a cancelled job and everything \
             else to a failure; reporting the wrong kind makes a cancel look \
             like a broken disk: {terminal}"
        );
        assert_eq!(
            terminal["error"]["path"],
            serde_json::Value::Null,
            "KestrelError::Cancelled carries no path; the wire must say null, \
             not a bogus one"
        );

        let partial = fs::metadata(&dst)
            .expect("the partial destination must exist")
            .len();
        assert!(
            partial > 0 && partial < src_len,
            "expected a partial destination, got {partial} of {src_len} bytes — \
             a cancel that landed before or after the copy is not this test"
        );
        assert_eq!(
            fs::metadata(&src).expect("source").len(),
            src_len,
            "the source must be untouched by a cancelled copy"
        );
    }

    /// EXIT-GATE CLAUSE 3 — "a cancelled move never deletes the source" —
    /// proved through the **command layer** on the *copy-then-delete* path.
    ///
    /// The engine clause is covered at
    /// `ops.rs::cancelled_move_keeps_the_source_and_leaves_a_partial_destination`,
    /// but that test drives `MoveStrategy::CopyThenDelete` directly. The
    /// command layer always uses [`MoveStrategy::Auto`], which on one
    /// filesystem is an atomic `rename` — instant, uncancellable, and therefore
    /// *not the code path that could ever delete the source*. The dangerous
    /// path is the `EXDEV` fallback: copy into place, then
    /// `delete_recursive(src)`. If a cancel were mishandled there, the source
    /// would be deleted with no copy behind it.
    ///
    /// So this forces the fallback with two genuinely different filesystems
    /// (`/dev/shm` and `/tmp` are separate mounts here) rather than a test
    /// hook, and cancels mid-copy.
    #[test]
    fn cancelling_a_cross_device_move_never_deletes_the_source() {
        // The source goes on a filesystem other than TMPDIR's, so `fs::rename`
        // fails with EXDEV and `move_` takes its copy-then-delete fallback.
        let src_dir = foreign_fs_tempdir("src");
        let dst_dir = tempfile::tempdir().expect("destination tempdir on TMPDIR");
        assert_ne!(
            device_id(src_dir.path()),
            device_id(dst_dir.path()),
            "source and destination must be on different mounts, or this is the \
             atomic-rename path and tests nothing about copy-then-delete \
             (src dev {} vs dst dev {})",
            device_id(src_dir.path()),
            device_id(dst_dir.path())
        );
        let src = src_dir.path().join("big.bin");
        let src_len = write_mb_file(&src, 512);
        let dst = dst_dir.path().join("big.bin.moved");

        let seen = run_op_cancel_midflight(
            OpRequestDto::Move {
                src: src.to_string_lossy().into_owned(),
                dst: dst.to_string_lossy().into_owned(),
            },
            dst.clone(),
            64 * 1024 * 1024,
        );

        let (kind, terminal) = terminal(&seen);
        assert_eq!(
            kind, "error",
            "a cancelled move is a terminal error: {seen:?}"
        );
        assert_eq!(
            terminal["error"]["kind"],
            serde_json::json!("cancelled"),
            "the cancel must be reported as `cancelled` over IPC: {terminal}"
        );

        // The whole point of the clause. `move_`'s copy-then-delete removes the
        // source once the destination exists; a cancel between the two would
        // delete the user's only copy.
        assert!(
            src.exists(),
            "a cancelled move must never delete the source — the copy half \
             did not finish, so deleting the source would lose data"
        );
        assert_eq!(
            fs::metadata(&src).expect("source stat").len(),
            src_len,
            "the source must be byte-for-byte intact"
        );
        if let Ok(partial) = fs::metadata(&dst) {
            assert!(
                partial.len() < src_len,
                "the destination must be partial, not a completed copy"
            );
        }
    }

    /// A tempdir on a filesystem *other* than the default one, so the move
    /// under test takes the `EXDEV` copy-then-delete fallback.
    ///
    /// Scans for a mount whose device differs from `/tmp`'s and is big enough
    /// for a few hundred megabytes. Panics rather than skipping: a silent
    /// skip would turn "the cancelled-move path was never exercised" into a
    /// green suite, which is precisely the failure mode this suite exists to
    /// prevent.
    fn foreign_fs_tempdir(tag: &str) -> tempfile::TempDir {
        let base = device_id(&std::env::temp_dir());
        let mut tried = Vec::new();
        for candidate in ["/dev/shm", "/run", "/var/tmp", "/home"] {
            let path = Path::new(candidate);
            if !path.is_dir() {
                tried.push(format!("{candidate} (absent)"));
                continue;
            }
            let dev = device_id(path);
            if dev == base {
                // Same filesystem: a rename would succeed and the copy half
                // would never run. Not a candidate.
                tried.push(format!("{candidate} (same device as TMPDIR)"));
                continue;
            }
            match tempfile::Builder::new()
                .prefix(&format!("kestrel-{tag}-"))
                .tempdir_in(path)
            {
                Ok(dir) => return dir,
                Err(e) => tried.push(format!("{candidate} ({e})")),
            }
        }
        panic!(
            "no second writable filesystem found for the cross-device move test. \
             Searched: {}. A cancel must be exercised on move's EXDEV \
             copy-then-delete path — the one that could ever delete the source. \
             Set TMPDIR to a filesystem other than these, or mount a tmpfs.",
            tried.join(", ")
        )
    }

    /// `st_dev` of a path's filesystem, via `MetadataExt`.
    fn device_id(path: &Path) -> u64 {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(path).expect("stat").dev()
    }
}
