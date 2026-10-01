/* Kestrel Phase 3a/3b — op controller (copy / move / trash / delete, plus the
 * collision handshake).
 *
 * Owns the in-flight job table and routes `OpEventDto` traffic to a plain
 * view-model the DOM layer reads. Deliberate boundaries:
 * - The Channel callback (`onOpEvent`) is SYNC and cheap: look up the job
 *   by id, stamp the latest counters, notify the view. No DOM, no await —
 *   the same backpressure deal as scan/watch in ipc.ts.
 * - Progress rendering never touches the windower: the job list is separate
 *   DOM updated per event (textContent + <progress> value), so the 39-node
 *   row pool is untouched by op traffic.
 * - Errors reuse the scan error path: the owner passes an `onOpError` sink
 *   (wired to Browser.showOpError), never a second error channel.
 * - Cancel never blocks: the job is marked `cancelling` synchronously and
 *   `ipc.opCancel` is fired best-effort. A stopped mock stream simply sends
 *   no more events; a late event for a finished job is ignored by id guard.
 * - Multiple ops may be in flight: jobs keyed by backend-minted JobId, each
 *   with its own row in the job list.
 *
 * Phase 3b — the collision handshake (the frozen contract):
 * - `collision` is neither progress nor a terminal event. The backend emits it
 *   and then BLOCKS on that one `dst` until `op_collision_answer` arrives, so
 *   the job moves to a distinct `paused` status rather than pretending to be
 *   `running`. A paused job is still active; it is not finished, and saying
 *   "running" would be a lie about work that is not happening.
 * - Exactly ONE collision is owed per job, and it is owed for ONE `dst`. The
 *   engine keys `AnsweredCollisions` by destination path (kestrel-fs/src/ops.rs)
 *   precisely so an answer can be scoped, so there is no "apply to all" and
 *   none is offered. A second collision arrives as its own event and parks
 *   again.
 * - An answer is routed only when the job is live, is the one currently owed,
 *   and names that exact `dst`. Anything else is dropped, so a stale answer
 *   (late, doubled, or aimed at a finished op) can never settle the wrong
 *   collision — the data-loss bug the whole feature is built to avoid.
 * - `abort` is answered through `op_collision_answer`, NOT `op_cancel`: the
 *   engine is blocked inside the handshake, not running, and the answer is
 *   what releases it. The local status flip (`cancelling`) and the 400 ms
 *   fallback timer are shared with `cancel` so a lost terminal event still
 *   settles the job honestly.
 *
 * Safari13-safe: no optional chaining, no Array.at/replaceAll/structuredClone.
 */

import type { KestrelIpc } from "./ipc";
import type {
  CmdError,
  CollisionDecision,
  CollisionKindDto,
  JobId,
  OpEventDto,
  OpPhaseDto,
  OpRequestDto,
} from "./types";

export type OpStatus =
  | "running"
  /** Phase 3b: blocked on a collision decision. Not running, not finished. */
  | "paused"
  | "cancelling"
  | "cancelled"
  | "done"
  | "failed";

/** The single collision a job is blocked on. Identity is (id, dst, seq):
 *  `dst` because that is the engine's own key, `seq` because two collisions
 *  for one job must never be confused for each other. */
export interface CollisionState {
  id: JobId;
  dst: string;
  kind: CollisionKindDto;
  seq: number;
}

export interface OpJob {
  id: JobId;
  req: OpRequestDto;
  label: string;
  status: OpStatus;
  phase: OpPhaseDto;
  doneBytes: number;
  totalBytes: number;
  doneItems: number;
  totalItems: number;
  /** True while totalBytes is 0: the UI must render indeterminate. */
  unknownTotal: boolean;
  summary: string;
  error: CmdError | null;
  /* ---- Cancel/settle evidence (Task 3: measured, not asserted) ---- */
  /** performance.now() when the user requested cancel (0 when never). */
  cancelRequestedAt: number;
  /** performance.now() when the backend's terminal event arrived (0 if none). */
  terminalEventAt: number;
  /** Who settled the job: "" while in flight, then "backend" or "local". */
  settleSource: "" | "backend" | "local";
  /** Phase label at the moment cancel was requested ("" when never). */
  phaseAtCancel: string;
  /* ---- Phase 3b: the collision handshake ---- */
  /** The one collision this job is blocked on, or null when none is owed. */
  collision: CollisionState | null;
  /** Collision parks seen for this job (monotonic; the exit-gate evidence
   *  that collisions arrive one at a time and are never batched). */
  collisionCount: number;
  /* ---- Monotonic-progress evidence (exit gate 1) ---- */
  /** Progress events seen for this job (any phase). */
  progressEvents: number;
  /** doneBytes drops within the same phase (phase changes reset the baseline). */
  backwardsJumps: number;
  lastDoneBytes: number;
  lastPhase: string;
}

export interface OpsView {
  /** Called after every job mutation (progress included). Keep it cheap. */
  onJobsChanged(jobs: OpJob[]): void;
  /** Same sink the app uses for scan errors. Never a second channel. */
  onOpError(error: CmdError): void;
  /** A job reached a terminal state; the listing may have changed. */
  onSettled(): void;
  /** Phase 3b: this job is now blocked on a collision decision. Open the
   *  dialog. Optional so an embedder that renders no dialog (fixture mode,
   *  a headless harness) does not have to implement it. */
  onCollision?(job: OpJob): void;
  /** Phase 3b: this job no longer owes an answer — answered, cancelled, or
   *  settled some other way. The dialog must close. Optional, as above. */
  onCollisionClosed?(id: JobId): void;
}

function noop(): void {
  /* best-effort promise sink */
}

function now(): number {
  try {
    return performance.now();
  } catch {
    return Date.now();
  }
}

export function opLabel(req: OpRequestDto): string {
  if (req.op === "copy") return "Copy " + req.src + " → " + req.dst;
  if (req.op === "move") return "Move " + req.src + " → " + req.dst;
  if (req.op === "trash") return "Trash " + req.src;
  return "Delete " + req.src;
}

export class OpManager {
  private jobs = new Map<JobId, OpJob>();
  /** Ids the user cancelled: a late done/error for one stays cancelled. */
  private cancelled = new Set<JobId>();
  /** Monotonic collision counter. Makes two collisions for one job
   *  distinguishable so a late answer cannot settle the newer one. */
  private collSeq = 0;

  constructor(private ipc: KestrelIpc, private view: OpsView) {}

  liveJobs(): OpJob[] {
    return Array.from(this.jobs.values());
  }

  activeCount(): number {
    let n = 0;
    const all = Array.from(this.jobs.values());
    for (let i = 0; i < all.length; i++) {
      const s = all[i].status;
      // `paused` counts: a job blocked on a decision is unfinished, and
      // reporting it as idle would understate what the window is doing.
      if (s === "running" || s === "cancelling" || s === "paused") n++;
    }
    return n;
  }

  /** Start an op. Resolves with the backend-minted id once accepted. */
  async start(req: OpRequestDto): Promise<JobId> {
    const self = this;
    const id = await this.ipc.opStart(req, function (ev) {
      self.onOpEvent(ev);
    });
    const job: OpJob = {
      id,
      req,
      label: opLabel(req),
      status: "running",
      phase: "measuring",
      doneBytes: 0,
      totalBytes: 0,
      doneItems: 0,
      totalItems: 0,
      unknownTotal: true,
      summary: "",
      error: null,
      cancelRequestedAt: 0,
      terminalEventAt: 0,
      settleSource: "",
      phaseAtCancel: "",
      collision: null,
      collisionCount: 0,
      progressEvents: 0,
      backwardsJumps: 0,
      lastDoneBytes: 0,
      lastPhase: "",
    };
    this.jobs.set(id, job);
    this.emit();
    return id;
  }

  /**
   * Cancel an op. Synchronous state flip; the IPC call never blocks the UI.
   *
   * Two entry points, deliberately one implementation: the job row's Cancel
   * button, and a job that is paused on a collision being aborted from the
   * dialog. In the paused case the answer still goes out over
   * `op_collision_answer` (the engine is blocked in the handshake and
   * `op_cancel` is not what releases it), but every piece of local state —
   * `cancelling`, the cancel timestamp, the 400 ms fallback — is identical.
   */
  cancel(id: JobId): void {
    const job = this.jobs.get(id);
    if (!job) return;
    if (job.status !== "running" && job.status !== "paused") return;
    this.markStopping(job);
    this.emit();
    // Fire-and-forget by design: cancel must be reachable while an op runs
    // and must not itself block on the backend.
    this.ipc.opCancel(id).then(noop, noop);
    this.armCancelFallback(id);
  }

  /** Release a parked collision dialog by aborting the whole operation.
   *  Routed through `answer`, so there is one code path for "stop this op"
   *  whether or not a dialog happened to be open. */
  abortForCollision(coll: CollisionState): boolean {
    return this.answer(coll, "abort");
  }

  /** The collision this job is blocked on, or null. */
  collisionFor(id: JobId): CollisionState | null {
    const job = this.jobs.get(id);
    return job ? job.collision : null;
  }

  /**
   * Adopt a collision discovered by asking rather than by event.
   *
   * The `collision` event is fire-and-forget over a Channel, so a window that
   * mounted (or reloaded) while an op was already paused never saw the
   * question. `op_pending_collisions` is the resynchronisation point, and this
   * is where its rows enter the same state machine the event path uses — one
   * implementation, so the guards in `answer` apply to an adopted collision
   * exactly as they do to a streamed one.
   *
   * Refused for a job that is not ours or not live: an answer routed at a
   * finished op is a decision with no referent.
   *
   * `seq` 0 is reserved for adopted collisions and sorts BELOW every
   * streamed one (the counter starts at 1). It does not need to be unique —
   * uniqueness is only load-bearing between two collisions that are live at the
   * same time, and there is at most one owed per job by construction. What
   * matters is that an answer carrying a streamed `seq` can never match an
   * adopted collision and vice versa, which the inequality gives in both
   * directions.
   */
  adoptCollision(job: OpJob, coll: CollisionState): boolean {
    if (!this.jobs.has(job.id)) return false;
    if (isTerminal(job.status)) return false;
    if (job.collision) return false; // one owed per job; never stack them
    job.collision = coll;
    job.collisionCount++;
    job.status = "paused";
    this.emit();
    if (this.view.onCollision) this.view.onCollision(job);
    return true;
  }

  /**
   * Answer the pending collision. Returns true only when the answer was
   * actually routed.
   *
   * Every guard here is a data-safety guard, not a tidiness guard. `dst` is
   * what the engine matches on; routing a different one settles nothing while
   * leaving the real collision blocking, and `seq` stops a late answer for an
   * earlier collision from being applied to a later one. A settled job owes
   * nothing, so an answer that arrives after `done`/`failed`/`cancelled` is
   * dropped rather than sent.
   */
  answer(coll: CollisionState, decision: CollisionDecision): boolean {
    const job = this.jobs.get(coll.id);
    if (!job) return false;
    if (isTerminal(job.status)) return false;
    const cur = job.collision;
    if (!cur) return false;
    if (cur.seq !== coll.seq) return false;
    if (cur.dst !== coll.dst) return false;

    // Take the debt before the IPC call: the dialog closes now, and the event
    // that finally settles the op is not what releases it — the answer is.
    this.clearCollision(job);
    if (decision === "abort") {
      this.markStopping(job);
    } else if (job.status === "paused") {
      job.status = "running";
    }
    this.emit();
    if (decision === "abort") this.armCancelFallback(coll.id);
    const self = this;
    this.ipc
      .opCollisionAnswer(coll.id, coll.dst, decision)
      .then(noop, function (err) {
        // The command is not there yet (the backend lane is still landing it).
        // Say so on the status line rather than letting the dialog look
        // answered while the engine is still blocked.
        self.view.onOpError({
          kind: "collision_answer_failed",
          path: coll.dst,
          message:
            "The answer could not be sent (" +
            messageOf(err) +
            "). The operation is still waiting at " +
            coll.dst +
            ".",
        });
      });
    return true;
  }

  /** Local stop-state flip, shared by `cancel` and an abort answer. */
  private markStopping(job: OpJob): void {
    // Read the phase BEFORE the status flip: `phaseAtCancel` is evidence, and
    // a job stopped while paused reports "paused", not the stale copy phase.
    const wasPaused = job.status === "paused";
    job.status = "cancelling";
    job.cancelRequestedAt = now();
    job.phaseAtCancel = wasPaused ? "paused" : job.phase;
    this.cancelled.add(job.id);
    // A pending decision is void once the user stops the op: leaving the
    // dialog up would offer a choice that can no longer change anything.
    this.clearCollision(job);
  }

  /**
   * The engine leaves a partial destination behind on cancel/abort by design,
   * so say so now rather than letting the user discover a half-copied file.
   * The job settles locally if the backend sends nothing more (its Channel
   * sends are discarded when the webview is busy, so a terminal event can be
   * genuinely lost — the timer is the safety net for that case, and the note
   * says which source settled the job).
   */
  private armCancelFallback(id: JobId): void {
    const self = this;
    setTimeout(function () {
      const j = self.jobs.get(id);
      if (j && j.status === "cancelling") {
        j.status = "cancelled";
        j.settleSource = "local";
        j.summary =
          "Stopped — the destination may be partially written; " +
          "check it and clean up what you do not need.";
        self.emit();
        self.view.onSettled();
      }
    }, 400);
  }

  private clearCollision(job: OpJob): void {
    if (!job.collision) return;
    job.collision = null;
    if (this.view.onCollisionClosed) this.view.onCollisionClosed(job.id);
  }

  /** Drop a terminal job row from the list (dismiss button). */
  dismiss(id: JobId): void {
    const job = this.jobs.get(id);
    if (!job) return;
    if (!isTerminal(job.status)) return;
    this.jobs.delete(id);
    this.cancelled.delete(id);
    this.emit();
  }

  /** Cheap + sync: stamp counters, notify. No DOM, no await. */
  private onOpEvent(ev: OpEventDto): void {
    const job = this.jobs.get(ev.id);
    if (!job) return; // stale: superseded scan-era job or unknown id
    if (isTerminal(job.status)) return;
    if (this.cancelled.has(ev.id) && ev.type !== "error") {
      // A late done/progress after the user cancelled stays cancelled —
      // never flip a cancelled job back to running. A `collision` is in the
      // same class: the user already stopped this op, so there is nothing
      // left to decide and no dialog is raised for it.
      return;
    }
    if (ev.type === "progress") {
      job.phase = ev.phase;
      job.doneBytes = ev.doneBytes;
      job.totalBytes = ev.totalBytes;
      job.doneItems = ev.doneItems;
      job.totalItems = ev.totalItems;
      // Exit-gate-1 evidence: same-phase monotonicity. A phase change
      // (measuring hands a known total to copying, which restarts from 0)
      // resets the baseline instead of counting as a backwards jump.
      job.progressEvents++;
      if (job.lastPhase === ev.phase && ev.doneBytes < job.lastDoneBytes)
        job.backwardsJumps++;
      job.lastDoneBytes = ev.doneBytes;
      job.lastPhase = ev.phase;
      // The spec's own wording: during measuring there is no total yet and
      // the backend may send totalBytes 0. Indeterminate is honest; a
      // guessed percentage is a bug. Any non-positive total is unknown.
      job.unknownTotal = ev.totalBytes <= 0;
      this.emit();
    } else if (ev.type === "collision") {
      // Phase 3b: the backend has stopped on this one destination and is
      // waiting for an answer naming it. Not progress, not terminal — the
      // job goes to `paused` so nothing claims work is happening.
      this.collSeq++;
      job.collisionCount++;
      job.collision = {
        id: ev.id,
        dst: ev.dst,
        kind: ev.kind,
        seq: this.collSeq,
      };
      job.status = "paused";
      this.emit();
      if (this.view.onCollision) this.view.onCollision(job);
    } else if (ev.type === "done") {
      if (this.cancelled.has(ev.id)) return;
      this.clearCollision(job);
      job.status = "done";
      job.summary = ev.summary;
      // A finished op fills the bar to its total so the row reads complete.
      if (!job.unknownTotal) job.doneBytes = job.totalBytes;
      this.emit();
      this.view.onSettled();
    } else {
      // The real backend reports a user cancel as a terminal `error` event
      // with kind `cancelled` (exactly one terminal event per op). That is
      // not a failure: the job stays cancelled, the partial-destination
      // note stands, and nothing goes to the error sink. An abort answered
      // at a collision arrives the same way.
      this.clearCollision(job);
      if (this.cancelled.has(ev.id) && isCancelKind(ev.error.kind)) {
        job.status = "cancelled";
        job.settleSource = "backend";
        job.terminalEventAt = now();
        job.summary =
          ev.error.message ||
          "Stopped — the destination may be partially written; " +
            "check it and clean up what you do not need.";
        this.emit();
        this.view.onSettled();
        return;
      }
      job.status = "failed";
      job.error = ev.error;
      this.emit();
      // Reuse the scan error path — no second error channel.
      this.view.onOpError(ev.error);
      this.view.onSettled();
    }
  }

  private emit(): void {
    this.view.onJobsChanged(this.liveJobs());
  }
}

/** Backend error kinds are frozen snake_case (`already_exists`, `cancelled`,
 * `permission_denied` — dto.rs). Match case- and underscore-insensitively so
 * a capitalisation drift (mock or future backend) degrades to the same UI,
 * never to a missed branch. */
function flatKind(kind: string): string {
  return (kind || "").toLowerCase().split("_").join("");
}

/** No more events will settle this job on their own. */
function isTerminal(status: OpStatus): boolean {
  return (
    status === "done" || status === "failed" || status === "cancelled"
  );
}

/** Collision error: the op failed, the destination was left untouched. Still
 *  reachable in 3b — the backend uses it when a collision cannot be turned
 *  into a question (no answer channel, a non-collidable race). */
export function isAlreadyExistsKind(kind: string): boolean {
  return flatKind(kind) === "alreadyexists";
}

/** Cancel/abort report: terminal, but not a failure — see onOpEvent. */
function isCancelKind(kind: string): boolean {
  const f = flatKind(kind);
  return f === "cancelled" || f === "canceled";
}

function messageOf(err: unknown): string {
  if (err instanceof Error) return err.message;
  try {
    return JSON.stringify(err);
  } catch {
    return String(err);
  }
}
