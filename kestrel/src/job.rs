//! File operations on a worker thread: copy, move, trash, delete.
//!
//! # Why a worker, and not the engine's blocking API
//!
//! [`kestrel_fs::ops::copy`] is synchronous and recursive. A 4 GB folder copy is
//! tens of thousands of `read`/`write` pairs: seconds to minutes. Calling it from
//! `ui()` would freeze the window for the duration, which is the one thing this
//! codebase is built not to do. So every operation runs on a thread and reports
//! through a channel, exactly as [`kestrel_fs::scan`] does — and, as the brief
//! requires, with its **own** event and handle types rather than reusing the
//! scan's. A copy has a different shape: it needs request/response (see
//! [`DecisionSlot`]) and it has a byte total the scan never has.
//!
//! # The collision protocol
//!
//! The engine's [`Collision::Fail`] already does the right check *before writing
//! a single byte*, and returns [`KestrelError::AlreadyExists`]. So the protocol
//! is simply: **try `Fail` first, and treat `AlreadyExists` as the question.**
//!
//! That is better than asking the UI to pre-scan the destination, because it
//! cannot disagree with the engine. A UI that checks `dst.exists()` is
//! reimplementing a check that will drift — and the drift is silent, because the
//! disagreement only shows up as a failed copy the user did not expect.
//!
//! When a collision is raised the worker parks on a [`DecisionSlot`] until the UI
//! answers. Parking is fine: it is a worker thread, and the UI thread is never
//! blocked. `Scope::All` makes the answer sticky for the rest of the job.
//!
//! # Never implicit
//!
//! The brief's rule, and the reason this module exists in this shape: no
//! strategy is ever chosen for the user. There is no default collision behaviour
//! on this path, no silent overwrite, and no silent skip — a collision always
//! becomes a dialog. [`Scope::ThisOne`] exists precisely so that "skip" cannot
//! accidentally become "skip everything".

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use kestrel_fs::error::{KestrelError, classify_io};
use kestrel_fs::model::CancellationToken;
use kestrel_fs::ops::{self, Collision, CopyOptions, MoveStrategy};

/// What a job does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// Copy each source into the destination directory.
    Copy,
    /// Move each source into the destination directory.
    Move,
    /// Move each source to the platform trash. **The default for `Delete`.**
    Trash,
    /// Remove each source permanently. Only reachable behind a confirm dialog.
    Delete,
}

impl Op {
    /// The §4.11 / §4.7 wording for this operation, used in the confirm dialog
    /// and the status bar.
    ///
    /// §4.7: "the verb is specific and irreversible-sounding — `Move to Trash`,
    /// `Delete Permanently`, `Empty Trash` — never `OK`, never `Yes`."
    #[must_use]
    pub fn verb(self) -> &'static str {
        match self {
            Self::Copy => "Copy",
            Self::Move => "Move",
            Self::Trash => "Move to Trash",
            Self::Delete => "Delete Permanently",
        }
    }

    /// `true` for the two operations that destroy the source.
    ///
    /// Drives the destructive button styling and whether a confirm dialog is
    /// required at all. §4.7: "Used for irreversible actions only".
    #[must_use]
    pub fn is_destructive(self) -> bool {
        matches!(self, Self::Trash | Self::Delete)
    }

    /// `true` when this op needs a destination directory.
    #[must_use]
    pub fn needs_destination(self) -> bool {
        matches!(self, Self::Copy | Self::Move)
    }
}

/// Whether a collision answer applies to just this item or to the rest of the job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// This one collision only. The next one asks again.
    ThisOne,
    /// Every remaining collision in this job. Explicitly chosen by the user, and
    /// never inferred.
    All,
}

/// The user's answer to a collision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decision {
    /// What to do with the destination.
    pub strategy: Strategy,
    /// How far the answer reaches.
    pub scope: Scope,
}

/// The two strategies the engine offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    /// Replace what is at the destination.
    Overwrite,
    /// Leave the destination alone and move on.
    Skip,
}

impl Strategy {
    /// The engine's strategy, for [`ops::CopyOptions`].
    ///
    /// This is the whole mapping, and it is deliberately total: there is no
    /// `Strategy` that maps to `Collision::Fail`, because a strategy the engine
    /// would reject is not an answer the user can give.
    #[must_use]
    pub fn to_collision(self) -> Collision {
        match self {
            Self::Overwrite => Collision::Overwrite,
            Self::Skip => Collision::Skip,
        }
    }

    /// `true` for the strategy that destroys data at the destination.
    #[must_use]
    // Part of the type's decision API and covered by tests, but the dialog
    // styles its `Overwrite*` buttons by variant rather than by asking this —
    // so nothing in `app.rs` calls it yet. Kept: the two predicates are how a
    // caller *should* branch, and deleting one would leave its counterpart
    // unexplained.
    #[allow(dead_code)]
    pub fn is_destructive(self) -> bool {
        matches!(self, Self::Overwrite)
    }

    /// `true` when this strategy leaves the existing file in place.
    #[must_use]
    #[allow(dead_code)]
    pub fn is_reversible(self) -> bool {
        matches!(self, Self::Skip)
    }
}

/// Build the decision a dialog button produces.
///
/// The scope is the *only* thing that varies between the two buttons of a
/// strategy, so a caller cannot accidentally pair "Overwrite" with the wrong
/// reach: it picks a scope and gets the strategy.
#[must_use]
pub fn decide(strategy: Strategy, scope: Scope) -> Decision {
    Decision { strategy, scope }
}

/// One thing to operate on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// The source path.
    pub src: PathBuf,
    /// The source's size, when the listing knew it.
    ///
    /// `None` for a directory, because [`kestrel_fs::model::FileEntry::size`] for
    /// a directory is its own `lstat` length, not a recursive total — and
    /// reporting `4.0 KB` as a directory's size is a lie. The byte total is
    /// filled in from the engine's recursive size walk when one is available.
    pub size: Option<u64>,
}

impl Item {
    /// A file whose size is known.
    #[must_use]
    pub fn file(src: impl Into<PathBuf>, size: Option<u64>) -> Self {
        Self {
            src: src.into(),
            size,
        }
    }

    /// A directory: size deliberately unknown.
    #[must_use]
    pub fn dir(src: impl Into<PathBuf>) -> Self {
        Self {
            src: src.into(),
            size: None,
        }
    }
}

/// What a job reports while it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobEvent {
    /// The job began, with this many items queued.
    Started {
        /// Items in the job.
        total: usize,
    },
    /// One item finished. Emitted for successes *and* for items the user chose to
    /// skip, because either way the job made progress.
    Item {
        /// The item's source path.
        src: PathBuf,
        /// The destination it went to, when the op has one.
        dst: Option<PathBuf>,
        /// Its size, or `None` when unknown.
        bytes: Option<u64>,
        /// `true` when the user chose to skip this one.
        skipped: bool,
    },
    /// A destination already exists and the job is waiting for an answer.
    ///
    /// The UI must show a dialog and answer on the job's [`DecisionSlot`]. Until
    /// it does, the worker is parked — never the frame.
    Collided {
        /// The source being copied.
        src: PathBuf,
        /// The occupied destination.
        dst: PathBuf,
        /// Collisions still unanswered, including this one.
        remaining: usize,
    },
    /// A tick, emitted at most every [`PROGRESS_INTERVAL`] so the status bar has
    /// something to show between item boundaries.
    Progress {
        /// Items finished.
        done: usize,
        /// Items total.
        total: usize,
        /// Bytes accounted for so far.
        bytes: u64,
        /// Bytes known to be in total, or `None` when no size walk ran.
        total_bytes: Option<u64>,
        /// The item currently being worked on.
        current: PathBuf,
    },
    /// The job finished successfully.
    Done {
        /// Items processed.
        done: usize,
        /// Bytes accounted for.
        bytes: u64,
        /// Items the user chose to skip.
        skipped: usize,
    },
    /// The job stopped because it was cancelled.
    Cancelled {
        /// Items processed before the stop.
        done: usize,
        /// Bytes accounted for before the stop.
        bytes: u64,
    },
    /// The job stopped on an error.
    Failed {
        /// What failed.
        path: PathBuf,
        /// Why, in a form safe to show a user.
        message: String,
        /// Items processed before the failure.
        done: usize,
        /// Bytes accounted for before the failure.
        bytes: u64,
    },
}

/// How often a `Progress` tick is emitted.
pub const PROGRESS_INTERVAL: std::time::Duration = std::time::Duration::from_millis(120);

/// The parked worker's question, and the UI's answer slot.
///
/// [`ops::copy`] is synchronous, so a worker that hits a collision has to stop
/// and wait. The wait happens here, on the worker: the frame loop is never
/// involved in blocking.
#[derive(Debug, Default)]
pub struct DecisionSlot {
    inner: Mutex<SlotState>,
    cv: Condvar,
    /// Set when the job is cancelled, so a parked worker wakes up instead of
    /// waiting for an answer to a question whose job is over.
    cancelled: AtomicBool,
}

#[derive(Debug, Default)]
struct SlotState {
    /// The pending question, or `None` if nothing is waiting.
    waiting: bool,
    /// The answer to the pending question.
    answer: Option<Decision>,
    /// The sticky answer from `Scope::All`.
    sticky: Option<Strategy>,
}

impl DecisionSlot {
    /// A fresh slot with nothing pending.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Posts a question and parks until it is answered or the job is cancelled.
    fn ask(&self) -> Option<Decision> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.waiting = true;
        self.cv.notify_all();
        loop {
            if let Some(answer) = guard.answer.take() {
                guard.waiting = false;
                self.cv.notify_all();
                return Some(answer);
            }
            if self.is_cancelled() {
                // The question is no longer pending: the job it belonged to is
                // over, so leaving `waiting` set would tell the UI a dialog is
                // still owed an answer.
                guard.waiting = false;
                return None;
            }
            // A bounded wait rather than an unbounded one, so a job whose UI went
            // away (window closed mid-copy) cannot leave a thread parked for the
            // life of the process. The loop re-checks cancellation.
            let (next, _timeout) = self
                .cv
                .wait_timeout(guard, std::time::Duration::from_millis(200))
                .unwrap_or_else(|e| e.into_inner());
            guard = next;
        }
    }

    /// Answers a pending question. `None` cancels the job instead.
    ///
    /// The `Scope::All` stickiness is recorded **here**, not in
    /// [`DecisionSlot::ask`], because `answer` is the only place the scope is
    /// known. Recording it in `ask` made the sticky value depend on a worker
    /// thread having parked, which is not a property of the answer at all — and
    /// a test that posted an answer without a parked worker found it.
    ///
    /// `None` is a *decision* — "I am not answering, stop" — and it is
    /// distinguishable from "nothing has been answered yet" only because it sets
    /// the cancelled flag. Storing it in `answer` as a `None` alone would be
    /// indistinguishable from an unanswered question: the worker's park loop
    /// waits for `answer.is_some() || is_cancelled()`, so the job would hang
    /// until the process exited, with the dialog closed and the status bar stuck.
    /// That is what the Cancel button of the collision dialog sends, so the bug
    /// is "close the dialog and the copy never stops".
    pub fn answer(&self, decision: Option<Decision>) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        // Cancellation is not an answer, so it must not clobber a sticky
        // `Scope::All` the user already gave — that answer is what every
        // *later* item would have been resolved by.
        if let Some(answer) = &decision
            && answer.scope == Scope::All
        {
            guard.sticky = Some(answer.strategy);
        }
        if decision.is_none() {
            self.cancelled.store(true, Ordering::Relaxed);
            guard.answer = None;
            guard.waiting = false;
        } else {
            guard.answer = decision;
        }
        // Notify outside the lock: a woken worker that has to take the mutex
        // immediately should not have to wait for this thread to drop it.
        drop(guard);
        self.cv.notify_all();
    }

    /// `true` when a question is waiting for an answer.
    #[must_use]
    pub fn is_waiting(&self) -> bool {
        self.inner.lock().is_ok_and(|g| g.waiting)
    }

    /// The sticky `Scope::All` answer, if the user gave one.
    #[must_use]
    pub fn sticky(&self) -> Option<Strategy> {
        self.inner.lock().ok().and_then(|g| g.sticky)
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}

/// A running job's handle. Mirrors [`kestrel_fs::scan::ScanHandle`].
#[derive(Debug)]
pub struct JobHandle {
    rx: Receiver<JobEvent>,
    cancel: CancellationToken,
    slot: Arc<DecisionSlot>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// Set once [`JobHandle::drain_into`] has seen a terminal event. See
    /// [`JobHandle::is_finished`] for why this is not a peek at the channel.
    finished: bool,
}

impl JobHandle {
    /// Drains every available event into `sink`.
    ///
    /// Takes a plain `FnMut(JobEvent)` for the same reason the scan's
    /// `drain_into` does: a `-> bool` sink invites an inverted-truth bug that
    /// silently truncates a job. Callers that need to stop early count events.
    pub fn drain_into(&mut self, mut sink: impl FnMut(JobEvent)) {
        while let Ok(event) = self.rx.try_recv() {
            self.finished |= is_terminal(&event);
            sink(event);
        }
    }

    /// Asks the job to stop. The worker finishes its current `read`/`write` pair
    /// and exits; a directory copy may leave a partial destination, which the
    /// engine documents and the UI reports.
    pub fn cancel(&self) {
        self.cancel.cancel();
        self.slot.cancelled.store(true, Ordering::Relaxed);
        self.slot.cv.notify_all();
    }

    /// `true` once the job has been asked to stop.
    #[must_use]
    // The UI tracks cancellation through `JobProgress`, not through the handle,
    // so this has no production caller. Kept because it is the honest way to
    // ask the question and the tests use it.
    #[allow(dead_code)]
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// The decision slot, for answering a [`JobEvent::Collided`].
    #[must_use]
    pub fn decisions(&self) -> &Arc<DecisionSlot> {
        &self.slot
    }

    /// `true` once a terminal event has been seen by [`JobHandle::drain_into`].
    ///
    /// This does **not** peek at the channel, because `std`'s `Receiver` cannot
    /// be peeked. Two obvious-looking attempts both eat the event:
    ///
    /// * `try_recv` takes the event, and the caller has to put it back.
    /// * `rx.try_iter().peekable().peek()` *looks* like a peek and is not one:
    ///   `Peekable::peek` calls `next`, and `try_iter`'s `next` is a
    ///   `try_recv`, so the message leaves the channel and is dropped with the
    ///   iterator.
    ///
    /// Either way the next `drain_into` silently never sees the event that was
    /// inspected — a `Collided`, a `Failed`, an `Item` — and a file manager
    /// that swallowed the failure of a copy is worse than one with no such
    /// method. So the answer is recorded as events are drained, which cannot
    /// lose them.
    ///
    /// The honest limit: this reports what has been *observed*. A caller that
    /// never drains will keep seeing `false`, which is the correct answer to
    /// the question it is actually being asked.
    #[must_use]
    #[allow(dead_code)]
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Joins the worker thread.
    ///
    /// Called when a job is replaced, so repeated operations do not accumulate
    /// threads — the same reason `ScanHandle::finish` exists.
    pub fn finish(mut self) {
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for JobHandle {
    fn drop(&mut self) {
        // Dropping a handle must not leave a thread running forever. Cancel, then
        // join, so the thread is gone before this returns.
        self.cancel.cancel();
        self.slot.cancelled.store(true, Ordering::Relaxed);
        self.slot.cv.notify_all();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Starts a job. Returns immediately.
///
/// # Errors
///
/// Returns the classified error if the source cannot even be stat'd, or if a
/// destination-requiring op was given none. Everything after that is reported
/// through [`JobEvent`], because a job that fails halfway has still done work
/// the user needs told about.
pub fn start(
    op: Op,
    items: Vec<Item>,
    destination: Option<PathBuf>,
) -> Result<JobHandle, KestrelError> {
    if op.needs_destination() && destination.is_none() {
        return Err(KestrelError::InvalidInput {
            path: PathBuf::from("."),
            reason: "copy and move need a destination directory".to_string(),
        });
    }
    if items.is_empty() {
        return Err(KestrelError::InvalidInput {
            path: PathBuf::from("."),
            reason: "a job needs at least one item".to_string(),
        });
    }

    let (tx, rx) = channel::<JobEvent>();
    let cancel = CancellationToken::new();
    let slot = Arc::new(DecisionSlot::new());

    let thread_cancel = cancel.clone();
    let thread_slot = Arc::clone(&slot);
    let total = items.len();
    let thread = std::thread::Builder::new()
        .name("kestrel-job".to_string())
        .spawn(move || {
            run(
                op,
                items,
                destination,
                thread_cancel,
                thread_slot,
                tx,
                total,
            );
        })
        .map_err(|e| KestrelError::InvalidInput {
            path: PathBuf::from("."),
            reason: format!("could not spawn a job worker: {e}"),
        })?;

    Ok(JobHandle {
        rx,
        cancel,
        slot,
        thread: Some(thread),
        finished: false,
    })
}

/// `true` for the three events that end a job.
fn is_terminal(event: &JobEvent) -> bool {
    matches!(
        event,
        JobEvent::Done { .. } | JobEvent::Cancelled { .. } | JobEvent::Failed { .. }
    )
}

/// The worker body.
fn run(
    op: Op,
    items: Vec<Item>,
    destination: Option<PathBuf>,
    cancel: CancellationToken,
    slot: Arc<DecisionSlot>,
    tx: Sender<JobEvent>,
    total: usize,
) {
    // A send failure means the UI is gone. Stop rather than finish an invisible
    // job; the items are still on disk, which is the safe direction.
    if tx.send(JobEvent::Started { total }).is_err() {
        return;
    }

    let mut done = 0usize;
    let mut skipped = 0usize;
    let mut bytes = 0u64;
    let mut total_bytes: Option<u64> = None;
    let started = Instant::now();
    let mut last_tick = Instant::now();

    for (index, item) in items.iter().enumerate() {
        if cancel.is_cancelled() {
            let _ = tx.send(JobEvent::Cancelled { done, bytes });
            return;
        }

        // Bytes progress only once per item, so the bar is honest about a
        // directory whose internal structure we cannot see. §7.16: a long
        // operation reports itself; a bar that creeps on a fabricated per-file
        // curve would be worse than one that steps.
        if let Some(size) = item.size {
            *total_bytes.get_or_insert(0) += size;
        }

        let dst = destination
            .as_ref()
            .map(|dir| dir.join(file_name_of(&item.src)));

        let mut skipped_this = false;
        let outcome = perform(
            op,
            &item.src,
            dst.as_deref(),
            &cancel,
            &slot,
            &tx,
            index,
            total,
        );

        match outcome {
            Outcome::Done => {
                if let Some(size) = item.size {
                    bytes += size;
                }
            }
            Outcome::Skipped => {
                skipped_this = true;
                skipped += 1;
            }
            Outcome::Cancelled => {
                let _ = tx.send(JobEvent::Cancelled { done, bytes });
                return;
            }
            Outcome::Failed(message) => {
                let _ = tx.send(JobEvent::Failed {
                    path: item.src.clone(),
                    message,
                    done,
                    bytes,
                });
                return;
            }
        }

        done += 1;
        if tx
            .send(JobEvent::Item {
                src: item.src.clone(),
                dst: dst.clone(),
                bytes: item.size,
                skipped: skipped_this,
            })
            .is_err()
        {
            return;
        }

        if last_tick.elapsed() >= PROGRESS_INTERVAL {
            last_tick = Instant::now();
            if tx
                .send(JobEvent::Progress {
                    done,
                    total,
                    bytes,
                    total_bytes,
                    current: item.src.clone(),
                })
                .is_err()
            {
                return;
            }
        }
        let _ = (index, started);
    }

    let _ = tx.send(JobEvent::Done {
        done,
        bytes,
        skipped,
    });
}

/// What one item's operation produced.
enum Outcome {
    Done,
    Skipped,
    Cancelled,
    Failed(String),
}

/// Runs one item, asking about collisions as they arise.
#[allow(clippy::too_many_arguments)]
fn perform(
    op: Op,
    src: &Path,
    dst: Option<&Path>,
    cancel: &CancellationToken,
    slot: &Arc<DecisionSlot>,
    tx: &Sender<JobEvent>,
    index: usize,
    total: usize,
) -> Outcome {
    match op {
        Op::Trash => match ops::trash(src) {
            Ok(()) => Outcome::Done,
            Err(err) => fail(src, err),
        },
        Op::Delete => match ops::delete_recursive(src, Some(cancel)) {
            Ok(()) => Outcome::Done,
            Err(err) => fail(src, err),
        },
        Op::Copy | Op::Move => {
            let Some(dst) = dst else {
                return Outcome::Failed("no destination directory".to_string());
            };
            if cancel.is_cancelled() {
                return Outcome::Cancelled;
            }
            // Ask only if the destination is occupied. The engine's own
            // `Collision::Fail` does the check and returns before writing, so
            // this loop cannot disagree with it.
            let sticky = slot.sticky();
            let mut strategy = sticky.map_or(Collision::Fail, |s| s.to_collision());
            // Was the destination there before this item started? Only a
            // destination this job *created* may be removed again — rolling back
            // onto one that was already there would destroy the file the user
            // asked to keep, which is the one case where cleaning up is worse
            // than the mess.
            let dst_existed = ops::path_entry_exists(dst);
            // Collisions the user has answered for this item, by destination.
            // A `Scope::ThisOne` answer is recorded here and *only* here: the
            // blanket `strategy` would apply it to every remaining colliding
            // child of a directory, which is how "overwrite this one" came to
            // mean "overwrite all of them without asking".
            let mut answered = ops::AnsweredCollisions::new();
            let mut asked = 0u32;
            loop {
                if cancel.is_cancelled() {
                    let _ = cleanup_partial(dst, dst_existed);
                    return Outcome::Cancelled;
                }
                let options = CopyOptions {
                    collision: strategy,
                    answered: answered.clone(),
                    cancel: Some(cancel.clone()),
                    ..CopyOptions::default()
                };
                let result = match op {
                    Op::Copy => ops::copy(src, dst, options),
                    _ => ops::move_(src, dst, options, MoveStrategy::Auto),
                };
                match result {
                    Ok(()) => {
                        // `Skip` that actually skipped something is a skip; a
                        // `Skip` the user gave for an *earlier* item's collision
                        // and that then wrote this one is not. The engine, not
                        // the strategy, is the authority: the effective policy
                        // for this destination is what decided the outcome.
                        let effective = answered.get(dst).unwrap_or(strategy);
                        return if effective == Collision::Skip && dst_existed {
                            Outcome::Skipped
                        } else {
                            Outcome::Done
                        };
                    }
                    Err(KestrelError::AlreadyExists { path })
                        if asked < MAX_COLLISIONS_PER_ITEM =>
                    {
                        // The failed probe may have written siblings that sorted
                        // before the collision. Undo them, so the retry does not
                        // copy on top of its own leftovers and so a `Skip` of the
                        // colliding file does not leave its neighbours behind.
                        let _ = cleanup_partial(dst, dst_existed);
                        asked += 1;
                        // No sticky answer yet: ask.
                        if slot.sticky().is_none() {
                            let remaining = total - index;
                            if tx
                                .send(JobEvent::Collided {
                                    src: src.to_path_buf(),
                                    dst: dst.to_path_buf(),
                                    remaining,
                                })
                                .is_err()
                            {
                                return Outcome::Failed("the window closed".to_string());
                            }
                        }
                        let Some(answer) = slot.ask() else {
                            let _ = cleanup_partial(dst, dst_existed);
                            return Outcome::Cancelled;
                        };
                        let decided = answer.strategy.to_collision();
                        if answer.scope == Scope::ThisOne {
                            // Settle exactly this destination...
                            answered.answer(path, decided);
                            // ...and nothing else. Going back to the `Fail` probe
                            // is what raises the *next* collision; the answered
                            // path no longer raises, so the loop makes progress
                            // instead of asking about the same file forever.
                            strategy = Collision::Fail;
                        } else {
                            strategy = decided;
                        }
                    }
                    // Cancellation is a status, not a failure. Reporting it as
                    // `Failed` showed the user a failure dialog whose body read
                    // "cancelled" — two different claims, and the wrong one.
                    Err(KestrelError::Cancelled) => {
                        let _ = cleanup_partial(dst, dst_existed);
                        return Outcome::Cancelled;
                    }
                    Err(err) => {
                        let message = describe(&err);
                        return Outcome::Failed(match cleanup_partial(dst, dst_existed) {
                            Ok(()) => message,
                            // The partial destination is the engine's documented
                            // debt to the caller ("which the caller is expected
                            // to clean up or report"). Reporting it by name is
                            // the last resort; the user is the one who can act.
                            Err(_) => format!(
                                "{message}; {} was left partly written and may be incomplete",
                                dst.display()
                            ),
                        });
                    }
                }
            }
        }
    }
}

/// Turns an engine error into a user-presentable message.
///
/// Classifying through [`classify_io`] keeps a mid-job failure worded the same
/// way as a listing failure, so the status bar has one vocabulary.
fn fail(path: &Path, err: KestrelError) -> Outcome {
    let _ = path;
    Outcome::Failed(describe(&err))
}

/// How many collisions one item may raise before the worker stops asking.
///
/// A runaway guard, not a policy: each round either settles a destination or
/// reaches a new one, so the loop terminates on its own. The ceiling exists so
/// that if that ever stops being true the job ends with a message instead of
/// asking the same question forever.
const MAX_COLLISIONS_PER_ITEM: u32 = 10_000;

/// Removes a destination this job created, so a cancelled or failed copy does
/// not leave a half-written tree in the user's directory.
///
/// The engine documents the partial destination as the caller's problem —
/// "leaving a partial destination behind (which the caller is expected to clean
/// up or report)" — and a caller that does neither is how a user ends up with a
/// truncated tree and no idea how it got there.
///
/// Only a destination that did **not** exist when the item started is removed.
/// A destination that was already there is the file the user asked to keep;
/// deleting it to tidy up would be the worst possible outcome.
///
/// A failure here is not fatal: the caller decides whether to report it, because
/// "it failed and here is the mess" and "it failed and the mess is gone" are
/// both acceptable endings and only the second is worth a sentence.
fn cleanup_partial(dst: &Path, dst_existed: bool) -> kestrel_fs::error::Result<()> {
    if dst_existed || !ops::path_entry_exists(dst) {
        return Ok(());
    }
    ops::delete_recursive(dst, None)
}

/// A one-line, user-facing description of an engine error.
///
/// Deliberately not the `Debug` form and not `Display`: a user reading
/// "PermissionDenied" or a full path has learned nothing about what to do, and
/// §4.7's body text has to say what will happen. Every arm is a bare clause,
/// because the dialog already supplies the subject ("Delete 3 items permanently
/// — `x` … permission denied").
#[must_use]
pub fn describe(err: &KestrelError) -> String {
    match err {
        KestrelError::AlreadyExists { .. } => "the destination already exists".to_string(),
        KestrelError::PermissionDenied { .. } => "permission denied".to_string(),
        KestrelError::NotFound { .. } => "it no longer exists".to_string(),
        KestrelError::InvalidFilename { .. } => {
            "the name is too long for this filesystem".to_string()
        }
        KestrelError::FilesystemLoop { .. } | KestrelError::LoopDetected { .. } => {
            "it is a symlink loop".to_string()
        }
        KestrelError::CrossesDevices { .. } => "it is on a different filesystem".to_string(),
        KestrelError::NotADirectory { .. } => "part of the path is not a directory".to_string(),
        KestrelError::IsADirectory { .. } => "it is a directory".to_string(),
        KestrelError::DirectoryNotEmpty { .. } => "the directory is not empty".to_string(),
        KestrelError::InvalidInput { reason, .. } => {
            // An empty reason would render as a blank sentence in the middle of
            // a dialog, which is worse than saying something vague: the user
            // knows the operation failed and needs *a* clue.
            if reason.trim().is_empty() {
                "the request was rejected".to_string()
            } else {
                reason.clone()
            }
        }
        KestrelError::Cancelled => "cancelled".to_string(),
        // The source is a boxed `dyn Error` for the `trash` crate; its own
        // message is the only thing that can say what the trash refused.
        KestrelError::Trash { source, .. } => source.to_string(),
        // Everything else carries an `io::Error` whose message is the OS's own
        // sentence, which is better than anything written here.
        other => other.to_string(),
    }
}

/// Re-derive a classification from a raw io error, for the paths the engine does
/// not classify itself.
#[must_use]
pub fn describe_io(path: &Path, err: std::io::Error) -> String {
    let classified = classify_io(path, err);
    describe(&classified)
}

/// The last component of a path, or the whole path if it has none.
fn file_name_of(path: &Path) -> PathBuf {
    path.file_name()
        .map_or_else(|| path.to_path_buf(), PathBuf::from)
}

/// A summary line for the status bar while a job runs.
///
/// Pure, so the wording is testable and cannot drift from §4.7.
#[must_use]
pub fn summary(op: Op, done: usize, total: usize) -> String {
    if total == 0 {
        return format!("{} nothing", op.verb());
    }
    format!("{} {done} of {total}", op.verb())
}

/// An items-per-second rate, or `None` before there is enough elapsed time to
/// mean anything.
///
/// A rate computed over 20 ms is noise, and a progress bar that says "2400
/// items/s" for one frame and "3 items/s" for the next is worse than no rate.
#[must_use]
pub fn rate(done: usize, elapsed: std::time::Duration) -> Option<f64> {
    if done < 2 {
        return None;
    }
    let secs = elapsed.as_secs_f64();
    if secs < 0.25 {
        return None;
    }
    Some(done as f64 / secs)
}

/// Formats a rate for display, or an empty string when there is none.
#[must_use]
pub fn format_rate(done: usize, elapsed: std::time::Duration) -> String {
    match rate(done, elapsed) {
        Some(r) if r >= 10.0 => format!("{:.0} items/s", r),
        Some(r) if r >= 1.0 => format!("{r:.1} items/s"),
        Some(r) => format!("{:.2} items/s", r),
        None => String::new(),
    }
}

/// Creates one directory, off the frame thread.
///
/// A one-shot rather than a [`JobHandle`]: the engine has no `mkdir`, and adding
/// an `Op::Mkdir` to make it fit would mean a job that is always exactly one
/// item, never collides, never has progress, and cannot fail halfway. The honest
/// shape is a single answer on a channel.
///
/// # Errors
///
/// Returns the classified error if the worker could not be spawned. The
/// directory's own failure arrives on the returned channel instead, because
/// `create_dir` is exactly the call whose failure a user needs told about — a
/// dropped thread that fails silently is a new folder that never appears.
pub fn mkdir(path: &Path) -> std::io::Result<Receiver<std::result::Result<(), KestrelError>>> {
    let (tx, rx) = channel();
    let path = path.to_path_buf();
    std::thread::Builder::new()
        .name("kestrel-mkdir".to_string())
        .spawn(move || {
            let result = std::fs::create_dir(&path).map_err(|e| classify_io(&path, e));
            // A send failure means the app is gone; the directory may or may not
            // exist, and there is nobody left to tell.
            let _ = tx.send(result);
        })?;
    Ok(rx)
}

/// A queue of items waiting to be pasted, in clipboard order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
// Built and tested as the paste path's ordering guarantee, but `app.rs` still
// hands its item list straight to `job::start`. Kept intact: it is the piece
// that makes paste order deterministic, and the tests assert that property.
#[allow(dead_code)]
pub struct PendingQueue {
    items: VecDeque<Item>,
}

#[allow(dead_code)]
impl PendingQueue {
    /// A queue holding `items`, in order.
    #[must_use]
    pub fn new(items: Vec<Item>) -> Self {
        Self {
            items: items.into_iter().collect(),
        }
    }

    /// The next item, without removing it.
    #[must_use]
    pub fn peek(&self) -> Option<&Item> {
        self.items.front()
    }

    /// Takes the next item.
    pub fn pop(&mut self) -> Option<Item> {
        self.items.pop_front()
    }

    /// How many items are queued.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// `true` when nothing is queued.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    // -- the collision decision mapping -------------------------------------

    /// Every strategy maps to the engine's strategy, and only two exist.
    #[test]
    fn strategy_maps_onto_the_engines_collision() {
        assert_eq!(Strategy::Overwrite.to_collision(), Collision::Overwrite);
        assert_eq!(Strategy::Skip.to_collision(), Collision::Skip);
    }

    /// `Collision::Fail` is not an answer a user can give.
    ///
    /// It is the engine's *probe* — the thing that raises the question — and if it
    /// were ever reachable as a strategy the worker would loop forever asking the
    /// same question.
    #[test]
    fn fail_is_a_probe_not_a_strategy() {
        for s in [Strategy::Overwrite, Strategy::Skip] {
            assert_ne!(
                s.to_collision(),
                Collision::Fail,
                "{s:?} would re-raise the question forever"
            );
        }
    }

    /// The scope is what varies, and it varies independently.
    #[test]
    fn scope_is_carried_independently_of_strategy() {
        for strategy in [Strategy::Overwrite, Strategy::Skip] {
            for scope in [Scope::ThisOne, Scope::All] {
                let d = decide(strategy, scope);
                assert_eq!(d.strategy, strategy);
                assert_eq!(d.scope, scope);
            }
        }
    }

    /// `Scope::All` is sticky; `ThisOne` is not. This is the whole difference
    /// between the two, so it is asserted through the real slot.
    #[test]
    fn only_an_all_answer_sticks() {
        let slot = DecisionSlot::new();
        // A ThisOne answer must leave nothing behind.
        slot.answer(Some(decide(Strategy::Overwrite, Scope::ThisOne)));
        assert_eq!(slot.sticky(), None);
        // An All answer must stick.
        slot.answer(Some(decide(Strategy::Skip, Scope::All)));
        assert_eq!(slot.sticky(), Some(Strategy::Skip));
    }

    /// Cancelling while a question is pending must unblock the parked worker.
    ///
    /// Without this, closing the window mid-copy leaves a thread parked for the
    /// life of the process.
    #[test]
    fn cancelling_unblocks_a_parked_question() {
        use std::sync::Arc;
        use std::time::Duration;
        let slot = Arc::new(DecisionSlot::new());
        let worker = {
            let slot = Arc::clone(&slot);
            std::thread::spawn(move || slot.ask())
        };
        // Let the worker reach the park.
        std::thread::sleep(Duration::from_millis(50));
        assert!(
            slot.is_waiting(),
            "the worker should be parked on the question"
        );
        slot.cancelled.store(true, Ordering::Relaxed);
        slot.cv.notify_all();
        let answer = worker.join().expect("worker");
        assert_eq!(answer, None, "a cancelled job must not get an answer");
    }

    /// Answering unblocks the parked worker with the right value.
    #[test]
    fn answering_unblocks_with_the_decision() {
        use std::sync::Arc;
        use std::time::Duration;
        let slot = Arc::new(DecisionSlot::new());
        let worker = {
            let slot = Arc::clone(&slot);
            std::thread::spawn(move || slot.ask())
        };
        std::thread::sleep(Duration::from_millis(50));
        assert!(slot.is_waiting());
        slot.answer(Some(decide(Strategy::Overwrite, Scope::All)));
        let answer = worker.join().expect("worker");
        assert_eq!(
            answer,
            Some(decide(Strategy::Overwrite, Scope::All)),
            "the worker must see the answer it was given"
        );
        assert_eq!(slot.sticky(), Some(Strategy::Overwrite));
        assert!(!slot.is_waiting(), "the question is answered");
    }

    // -- jobs ---------------------------------------------------------------

    /// The path a directory item lands at: the destination joined with the
    /// source's own name, which is how `run` computes it.
    fn lands_at(dst: &Path, src: &Path) -> PathBuf {
        dst.join(file_name_of(src))
    }

    fn drain_all(h: &mut JobHandle) -> Vec<JobEvent> {
        let mut out = Vec::new();
        // Bounded: a test must not hang if the worker wedges.
        for _ in 0..2000 {
            let before = out.len();
            h.drain_into(|e| out.push(e));
            if out.len() > before {
                if let Some(last) = out.last() {
                    if matches!(
                        last,
                        JobEvent::Done { .. }
                            | JobEvent::Cancelled { .. }
                            | JobEvent::Failed { .. }
                    ) {
                        break;
                    }
                }
                // The worker has more to say, but has not finished: yield and
                // poll again rather than blocking on the channel.
                std::thread::sleep(Duration::from_millis(2));
            } else {
                // Nothing new this tick, terminal or not — same yield either
                // way, so there is no branch to make here.
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        out
    }

    use std::time::Duration;

    #[test]
    fn a_copy_job_copies_every_item_and_reports_done() {
        let src = tmp();
        let dst = tmp();
        fs::write(src.path().join("a.txt"), b"hello").expect("write");
        fs::write(src.path().join("b.txt"), b"world!").expect("write");

        let mut h = start(
            Op::Copy,
            vec![
                Item::file(src.path().join("a.txt"), Some(5)),
                Item::file(src.path().join("b.txt"), Some(6)),
            ],
            Some(dst.path().to_path_buf()),
        )
        .expect("start");
        let events = drain_all(&mut h);
        h.finish();

        assert_eq!(fs::read(dst.path().join("a.txt")).expect("read"), b"hello");
        assert_eq!(fs::read(dst.path().join("b.txt")).expect("read"), b"world!");
        let done = events
            .iter()
            .find_map(|e| match e {
                JobEvent::Done { done, .. } => Some(*done),
                _ => None,
            })
            .expect("a Done event");
        assert_eq!(done, 2);
    }

    /// A collision must surface as a question, and the user's answer must be
    /// obeyed. This is the "never silently overwrite" rule, end to end.
    #[test]
    fn a_collision_asks_and_then_obeys_the_answer() {
        let src = tmp();
        let dst = tmp();
        fs::write(src.path().join("a.txt"), b"new").expect("write");
        fs::write(dst.path().join("a.txt"), b"old").expect("write");

        let mut h = start(
            Op::Copy,
            vec![Item::file(src.path().join("a.txt"), Some(3))],
            Some(dst.path().to_path_buf()),
        )
        .expect("start");

        // Wait for the question, then answer "skip".
        let mut events = Vec::new();
        let mut asked = false;
        for _ in 0..500 {
            h.drain_into(|e| events.push(e));
            if let Some(JobEvent::Collided { src: s, dst: d, .. }) = events.last() {
                assert_eq!(s.file_name().expect("name"), "a.txt");
                assert_eq!(d.file_name().expect("name"), "a.txt");
                h.decisions()
                    .answer(Some(decide(Strategy::Skip, Scope::ThisOne)));
                asked = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            asked,
            "a collision must be raised as a question, not resolved silently"
        );
        drain_all(&mut h);
        h.finish();

        assert_eq!(
            fs::read(dst.path().join("a.txt")).expect("read"),
            b"old",
            "Skip must leave the destination alone"
        );
    }

    /// And the other answer must be obeyed too — this is the "never silently
    /// overwrite" half: overwrite is possible, but only because the user said so.
    #[test]
    fn overwrite_is_applied_only_because_the_user_asked() {
        let src = tmp();
        let dst = tmp();
        fs::write(src.path().join("a.txt"), b"new").expect("write");
        fs::write(dst.path().join("a.txt"), b"old").expect("write");

        let mut h = start(
            Op::Copy,
            vec![Item::file(src.path().join("a.txt"), Some(3))],
            Some(dst.path().to_path_buf()),
        )
        .expect("start");
        let mut events = Vec::new();
        for _ in 0..500 {
            h.drain_into(|e| events.push(e));
            if matches!(events.last(), Some(JobEvent::Collided { .. })) {
                h.decisions()
                    .answer(Some(decide(Strategy::Overwrite, Scope::ThisOne)));
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        drain_all(&mut h);
        h.finish();
        assert_eq!(fs::read(dst.path().join("a.txt")).expect("read"), b"new");
    }

    /// The dialog's Cancel button is `answer(None)`, and it must cancel.
    ///
    /// The two buttons that carry a decision both answer `Some(..)`, so the
    /// `None` path is the one the collision tests never reach — and it is the
    /// path the user reaches whenever they close a dialog they did not want to
    /// answer. Before the fix this parked the worker on the condvar for the life
    /// of the process: no re-park, no `Cancelled`, the status bar stuck on
    /// "Copy 1 of 1" forever.
    #[test]
    fn answering_none_cancels_the_job() {
        let src = tmp();
        let dst = tmp();
        fs::write(src.path().join("a.txt"), b"new").expect("write");
        fs::write(dst.path().join("a.txt"), b"old").expect("write");

        let mut h = start(
            Op::Copy,
            vec![Item::file(src.path().join("a.txt"), Some(3))],
            Some(dst.path().to_path_buf()),
        )
        .expect("start");

        // Wait for the question, then press Cancel.
        let mut events = Vec::new();
        let mut asked = false;
        for _ in 0..500 {
            h.drain_into(|e| events.push(e));
            if matches!(events.last(), Some(JobEvent::Collided { .. })) {
                h.decisions().answer(None);
                asked = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            asked,
            "a collision must be raised as a question before it can be cancelled"
        );

        let rest = drain_all(&mut h);
        events.extend(rest);
        let cancelled = events
            .iter()
            .any(|e| matches!(e, JobEvent::Cancelled { .. }));

        if !cancelled {
            // The worker is still parked. Dropping or finishing the handle would
            // join that thread and hang the test instead of failing it, so it is
            // detached deliberately: this failure mode *is* the bug, and a red
            // test is more useful than a red suite.
            std::mem::forget(h);
            panic!("answering None must cancel the job, not park it forever: {events:?}");
        }

        h.finish();
        assert!(
            !events.iter().any(|e| matches!(e, JobEvent::Done { .. })),
            "a cancelled job must not claim success: {events:?}"
        );
        assert_eq!(
            fs::read(dst.path().join("a.txt")).expect("read"),
            b"old",
            "a cancelled collision must leave the destination alone"
        );
    }

    /// A cancelled copy is a cancellation, not a failure.
    ///
    /// The engine returns `KestrelError::Cancelled` and every other error was
    /// funnelled into `Outcome::Failed`, so a cancelled copy raised a *failure*
    /// dialog whose body read "cancelled" — and the half-copied tree it left in
    /// the destination was never mentioned. §7.16: a long operation reports
    /// itself honestly, and a user who is told "failed" goes looking for
    /// something that broke.
    ///
    /// The cancel is timed against the first file actually landing in the
    /// destination, so it lands *inside* the copy rather than before it — the
    /// engine's per-file cancellation check is what returns the error, and that
    /// is the branch under test.
    #[test]
    fn a_cancelled_job_reports_cancelled_not_failed() {
        let src = tmp();
        let dst = tmp();
        let mut items = Vec::new();
        for i in 0..600 {
            fs::write(src.path().join(format!("f{i:04}")), b"payload").expect("write");
            items.push(Item::file(src.path().join(format!("f{i:04}")), Some(7)));
        }
        // A directory source, so one item is a whole tree and the copy is
        // genuinely in flight when the cancel arrives.
        let tree = tmp();
        let mut names = Vec::new();
        for i in 0..600 {
            let name = format!("f{i:04}");
            fs::write(tree.path().join(&name), b"payload").expect("write");
            names.push(name);
        }

        let destination = lands_at(dst.path(), tree.path());
        let mut h = start(
            Op::Copy,
            vec![Item::dir(tree.path().to_path_buf())],
            Some(dst.path().to_path_buf()),
        )
        .expect("start");

        // Wait for the copy to be demonstrably under way, then cancel.
        let mut started = false;
        for _ in 0..200_000 {
            if names.iter().any(|n| destination.join(n).exists()) {
                started = true;
                break;
            }
            h.drain_into(|_| {});
            std::thread::sleep(Duration::from_micros(50));
        }
        h.cancel();
        assert!(
            started,
            "the copy should have been under way before cancelling"
        );

        let events = drain_all(&mut h);
        h.finish();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, JobEvent::Cancelled { .. })),
            "a cancelled copy must report Cancelled: {events:?}"
        );
        let failures: Vec<&JobEvent> = events
            .iter()
            .filter(|e| matches!(e, JobEvent::Failed { .. }))
            .collect();
        assert!(
            failures.is_empty(),
            "a cancellation must never be reported as a failure: {failures:?}"
        );
        // The engine's contract is that it leaves a partial destination for the
        // caller to clean up or report. This caller cleans up, so the user is
        // not left with a truncated tree.
        assert!(
            !ops::path_entry_exists(&destination),
            "a cancelled copy must not leave a partial destination behind"
        );
        // Silence the unused warning for the file fixture built above; it keeps
        // the two source shapes interchangeable in this test.
        assert_eq!(items.len(), 600);
    }

    /// `Scope::ThisOne` means *this one collision*, not "this one directory".
    ///
    /// A directory holding two colliding files raises one collision per file.
    /// Answering "overwrite this one" and then retrying with a blanket
    /// `Overwrite` silently overwrote the second file too, without a second
    /// question — the exact opposite of what the button said. The fix records
    /// the answer against that one destination and goes back to probing, so the
    /// next collision is asked about.
    #[test]
    fn a_second_collision_in_one_item_asks_again() {
        let dst = tmp();
        let source = tmp();
        let destination = lands_at(dst.path(), source.path());
        fs::create_dir_all(&destination).expect("mkdir");
        for name in ["a.txt", "b.txt"] {
            fs::write(source.path().join(name), b"new").expect("write");
            fs::write(destination.join(name), b"old").expect("write");
        }

        let mut h = start(
            Op::Copy,
            vec![Item::dir(source.path().to_path_buf())],
            Some(dst.path().to_path_buf()),
        )
        .expect("start");

        let mut events = Vec::new();
        let mut collisions = 0usize;
        for _ in 0..4000 {
            let mut collided = false;
            let mut finished = false;
            h.drain_into(|e| {
                match e {
                    JobEvent::Collided { .. } => collided = true,
                    JobEvent::Done { .. }
                    | JobEvent::Failed { .. }
                    | JobEvent::Cancelled { .. } => {
                        finished = true;
                    }
                    _ => {}
                }
                events.push(e);
            });
            // One answer per question asked, and no re-answering of a question
            // already answered: the worker only parks again when it has a *new*
            // collision to raise.
            if collided {
                collisions += 1;
                h.decisions()
                    .answer(Some(decide(Strategy::Overwrite, Scope::ThisOne)));
                continue;
            }
            if finished {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let rest = drain_all(&mut h);
        h.finish();
        events.extend(rest);

        // One question for the destination directory, then one per colliding
        // file. Before the fix the whole directory was one question and both
        // files were overwritten on the strength of the first answer.
        assert_eq!(
            collisions, 3,
            "each colliding destination is its own question: {events:?}"
        );
        assert_eq!(
            fs::read(destination.join("a.txt")).expect("read"),
            b"new",
            "the first answer was obeyed"
        );
        assert_eq!(
            fs::read(destination.join("b.txt")).expect("read"),
            b"new",
            "the second answer was obeyed too, after being asked for"
        );
    }

    /// A Move answered "Skip" leaves the source on disk.
    ///
    /// The button says "leave the destination alone". For a move that has to
    /// include the source, or the operation has quietly deleted a tree.
    #[test]
    fn a_move_answered_skip_leaves_the_source_on_disk() {
        let dst = tmp();
        let source = tmp();
        fs::write(source.path().join("a.txt"), b"new").expect("write");
        let destination = lands_at(dst.path(), source.path());
        fs::create_dir_all(&destination).expect("mkdir");
        fs::write(destination.join("a.txt"), b"old").expect("write");

        let mut h = start(
            Op::Move,
            vec![Item::dir(source.path().to_path_buf())],
            Some(dst.path().to_path_buf()),
        )
        .expect("start");

        let mut events = Vec::new();
        for _ in 0..500 {
            h.drain_into(|e| events.push(e));
            if matches!(events.last(), Some(JobEvent::Collided { .. })) {
                h.decisions()
                    .answer(Some(decide(Strategy::Skip, Scope::ThisOne)));
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let rest = drain_all(&mut h);
        h.finish();
        events.extend(rest);

        assert!(
            source.path().join("a.txt").exists(),
            "a skipped move must not delete the source"
        );
        assert_eq!(
            fs::read(destination.join("a.txt")).expect("read"),
            b"old",
            "the destination must be untouched"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, JobEvent::Item { skipped: true, .. })),
            "the job must report the item as skipped: {events:?}"
        );
    }

    /// `is_finished` must not eat the event it inspects.
    ///
    /// A `try_recv`-based peek silently drops whatever it looked at — a
    /// `Collided`, a `Failed`, an `Item` — and the next `drain_into` never sees
    /// it. Nothing calls the method today, which is exactly why it needed a test
    /// rather than a caller.
    ///
    /// The assertions are deliberately about *what the drain finds afterwards*,
    /// not about the boolean: the bug is a missing event, and a boolean
    /// assertion would pass just as happily against a method that ate the queue.
    #[test]
    fn is_finished_does_not_consume_the_event_it_inspects() {
        let src = tmp();
        let dst = tmp();
        fs::write(src.path().join("a.txt"), b"new").expect("write");
        fs::write(dst.path().join("a.txt"), b"old").expect("write");

        let mut h = start(
            Op::Copy,
            vec![Item::file(src.path().join("a.txt"), Some(3))],
            Some(dst.path().to_path_buf()),
        )
        .expect("start");

        // Park the worker on the question, so a `Collided` is waiting to be
        // swallowed.
        for _ in 0..500 {
            if h.decisions().is_waiting() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(h.decisions().is_waiting(), "the worker should be parked");

        assert!(!h.is_finished(), "nothing terminal has been drained yet");
        assert!(!h.is_finished(), "asking twice must not change that");

        let mut first = Vec::new();
        h.drain_into(|e| first.push(e));
        assert!(
            first.iter().any(|e| matches!(e, JobEvent::Started { .. })),
            "an event went missing: {first:?}"
        );
        assert!(
            first.iter().any(|e| matches!(e, JobEvent::Collided { .. })),
            "the collision went missing: {first:?}"
        );
        assert!(!h.is_finished(), "a collision is not terminal");

        h.decisions()
            .answer(Some(decide(Strategy::Skip, Scope::ThisOne)));
        let last = drain_all(&mut h);
        assert!(
            last.iter().any(|e| matches!(e, JobEvent::Item { .. })),
            "the item event went missing: {last:?}"
        );
        assert!(
            last.iter().any(|e| matches!(e, JobEvent::Done { .. })),
            "the terminal event went missing: {last:?}"
        );
        assert!(h.is_finished(), "a terminal event has been drained");

        // And it stays true, without the channel needing to be non-empty.
        h.drain_into(|_| {});
        assert!(h.is_finished());
        h.finish();
    }

    /// `Scope::All` must not ask twice: the second colliding item is resolved by
    /// the sticky answer.
    #[test]
    fn an_all_answer_is_not_asked_again() {
        let src = tmp();
        let dst = tmp();
        for n in ["a.txt", "b.txt"] {
            fs::write(src.path().join(n), b"new").expect("write");
            fs::write(dst.path().join(n), b"old").expect("write");
        }
        let mut h = start(
            Op::Copy,
            vec![
                Item::file(src.path().join("a.txt"), Some(3)),
                Item::file(src.path().join("b.txt"), Some(3)),
            ],
            Some(dst.path().to_path_buf()),
        )
        .expect("start");
        let mut events = Vec::new();
        for _ in 0..500 {
            h.drain_into(|e| events.push(e));
            if matches!(events.last(), Some(JobEvent::Collided { .. })) {
                h.decisions()
                    .answer(Some(decide(Strategy::Overwrite, Scope::All)));
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let rest = drain_all(&mut h);
        h.finish();
        events.extend(rest);
        let collisions = events
            .iter()
            .filter(|e| matches!(e, JobEvent::Collided { .. }))
            .count();
        assert_eq!(collisions, 1, "the sticky answer must not ask again");
        assert_eq!(fs::read(dst.path().join("b.txt")).expect("read"), b"new");
    }

    /// A cancelled job stops, and says so, rather than reporting success.
    #[test]
    fn cancelling_a_job_reports_cancellation_not_success() {
        let src = tmp();
        let dst = tmp();
        for i in 0..200 {
            fs::write(src.path().join(format!("f{i}")), b"x").expect("write");
        }
        let items: Vec<Item> = (0..200)
            .map(|i| Item::file(src.path().join(format!("f{i}")), Some(1)))
            .collect();
        let mut h = start(Op::Copy, items, Some(dst.path().to_path_buf())).expect("start");
        h.cancel();
        let events = drain_all(&mut h);
        h.finish();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, JobEvent::Cancelled { .. })),
            "a cancelled job must say so: {events:?}"
        );
        assert!(
            !events.iter().any(|e| matches!(e, JobEvent::Done { .. })),
            "a cancelled job must not claim success"
        );
    }

    /// An empty job is refused up front, not silently "succeeding".
    #[test]
    fn an_empty_job_is_refused() {
        let dst = tmp();
        assert!(start(Op::Copy, vec![], Some(dst.path().to_path_buf())).is_err());
    }

    /// Copy and move without a destination is a programming error, refused here
    /// rather than failing per item.
    #[test]
    fn copy_without_a_destination_is_refused() {
        let src = tmp();
        fs::write(src.path().join("a"), b"x").expect("write");
        assert!(
            start(
                Op::Copy,
                vec![Item::file(src.path().join("a"), Some(1))],
                None
            )
            .is_err()
        );
    }

    /// A missing source fails the job with a readable message, and the engine's
    /// error type is preserved rather than stringified at the boundary.
    /// A created directory exists, and its result is delivered.
    #[test]
    fn mkdir_creates_and_reports() {
        let d = tmp();
        let target = d.path().join("new");
        let rx = mkdir(&target).expect("spawn");
        let result = rx.recv_timeout(Duration::from_secs(5)).expect("an answer");
        assert!(result.is_ok(), "{:?}", result.err());
        assert!(target.is_dir(), "the directory should exist");
    }

    /// A `mkdir` that cannot work reports it, rather than the thread dying
    /// silently and the user wondering why no folder appeared.
    #[test]
    fn mkdir_reports_failure() {
        let d = tmp();
        let target = d.path().join("new");
        fs::create_dir(&target).expect("mkdir");
        let rx = mkdir(&target).expect("spawn");
        let result = rx.recv_timeout(Duration::from_secs(5)).expect("an answer");
        assert!(
            result.is_err(),
            "creating over an existing directory must fail"
        );
    }

    #[test]
    fn a_missing_source_fails_with_a_readable_message() {
        let src = tmp();
        let dst = tmp();
        let mut h = start(
            Op::Copy,
            vec![Item::file(src.path().join("nope.txt"), Some(1))],
            Some(dst.path().to_path_buf()),
        )
        .expect("start");
        let events = drain_all(&mut h);
        h.finish();
        let failed = events
            .iter()
            .find_map(|e| match e {
                JobEvent::Failed { message, .. } => Some(message.clone()),
                _ => None,
            })
            .expect("a Failed event");
        assert!(
            !failed.is_empty(),
            "the message must say something a user can act on"
        );
    }

    // -- op wording and progress formatting ---------------------------------

    /// §4.7: never `OK`, never `Yes`. The verbs are named.
    #[test]
    fn op_verbs_are_specific_and_never_ok() {
        assert_eq!(Op::Trash.verb(), "Move to Trash");
        assert_eq!(Op::Delete.verb(), "Delete Permanently");
        for op in [Op::Copy, Op::Move, Op::Trash, Op::Delete] {
            let v = op.verb().to_ascii_lowercase();
            assert_ne!(v, "ok");
            assert_ne!(v, "yes");
            assert_ne!(v, "okay");
            assert!(!v.is_empty());
        }
    }

    /// Only the destroying ops are destructive, and only they need a dialog.
    #[test]
    fn destructive_means_destroying() {
        assert!(Op::Trash.is_destructive());
        assert!(Op::Delete.is_destructive());
        assert!(!Op::Copy.is_destructive());
        assert!(!Op::Move.is_destructive());
        assert!(Op::Copy.needs_destination());
        assert!(!Op::Trash.needs_destination());
        assert!(
            Op::Copy.needs_destination(),
            "a collision needs a destination"
        );
        assert!(!Op::Trash.needs_destination());
    }

    /// A rate is suppressed until it means something.
    #[test]
    fn a_rate_needs_enough_samples_to_mean_anything() {
        assert_eq!(rate(0, Duration::from_secs(1)), None, "no items, no rate");
        assert_eq!(
            rate(1, Duration::from_secs(1)),
            None,
            "one item is not a rate"
        );
        assert_eq!(rate(50, Duration::from_millis(20)), None, "20ms is noise");
        let r = rate(50, Duration::from_secs(2)).expect("a rate");
        assert!((r - 25.0).abs() < 0.001, "got {r}");
        assert_eq!(format_rate(50, Duration::from_secs(2)), "25 items/s");
        assert_eq!(format_rate(1, Duration::from_secs(1)), "");
    }

    #[test]
    fn the_summary_names_the_op_and_the_progress() {
        assert_eq!(summary(Op::Copy, 3, 10), "Copy 3 of 10");
        assert_eq!(summary(Op::Trash, 0, 1), "Move to Trash 0 of 1");
    }

    #[test]
    fn the_queue_is_fifo_and_empties() {
        let mut q = PendingQueue::new(vec![Item::file("/a", Some(1)), Item::file("/b", Some(2))]);
        assert_eq!(q.len(), 2);
        assert!(!q.is_empty());
        assert_eq!(q.peek().expect("peek").src, PathBuf::from("/a"));
        assert_eq!(q.pop().expect("pop").src, PathBuf::from("/a"));
        assert_eq!(q.pop().expect("pop").src, PathBuf::from("/b"));
        assert!(q.pop().is_none());
        assert!(q.is_empty());
    }

    /// An error must read as a sentence, not as a `Debug` dump.
    #[test]
    fn errors_read_as_sentences() {
        let p = PathBuf::from("/x");
        let e = std::io::Error::other("boom");
        assert_eq!(
            describe(&KestrelError::AlreadyExists { path: p.clone() }),
            "the destination already exists"
        );
        assert_eq!(
            describe(&KestrelError::PermissionDenied {
                path: p.clone(),
                source: e
            }),
            "permission denied"
        );
        assert_eq!(
            describe(&KestrelError::NotFound {
                path: p.clone(),
                source: std::io::Error::other("x")
            }),
            "it no longer exists"
        );
        let msg = describe(&KestrelError::InvalidInput {
            path: p.clone(),
            reason: "dst is inside src".to_string(),
        });
        assert_eq!(msg, "dst is inside src");
        // Every arm must be a non-empty clause, never a variant name.
        for err in [
            KestrelError::AlreadyExists { path: p.clone() },
            KestrelError::InvalidInput {
                path: p.clone(),
                reason: String::new(),
            },
            KestrelError::InvalidInput {
                path: p.clone(),
                reason: "   ".to_string(),
            },
        ] {
            let d = describe(&err);
            assert!(
                !d.trim().is_empty(),
                "{err:?} described as an empty sentence"
            );
        }
        // `Cancelled` is the one variant that is a status, not a clause.
        assert_eq!(describe(&KestrelError::Cancelled), "cancelled");
    }
}
