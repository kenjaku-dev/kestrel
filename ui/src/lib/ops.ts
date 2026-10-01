/* Kestrel Phase 3a — op controller (copy / move / trash / delete).
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
 * - `AlreadyExists` is an error, not a dialog: 3a has no collision event by
 *   design. Show it honestly; the blocking dialog is a later phase.
 * - Cancel never blocks: the job is marked `cancelling` synchronously and
 *   `ipc.opCancel` is fired best-effort. A stopped mock stream simply sends
 *   no more events; a late event for a finished job is ignored by id guard.
 * - Multiple ops may be in flight: jobs keyed by backend-minted JobId, each
 *   with its own row in the job list.
 *
 * Safari13-safe: no optional chaining, no Array.at/replaceAll/structuredClone.
 */

import type { KestrelIpc } from "./ipc";
import type {
  CmdError,
  JobId,
  OpEventDto,
  OpPhaseDto,
  OpRequestDto,
} from "./types";

export type OpStatus =
  | "running"
  | "cancelling"
  | "cancelled"
  | "done"
  | "failed";

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

  constructor(private ipc: KestrelIpc, private view: OpsView) {}

  liveJobs(): OpJob[] {
    return Array.from(this.jobs.values());
  }

  activeCount(): number {
    let n = 0;
    const all = Array.from(this.jobs.values());
    for (let i = 0; i < all.length; i++) {
      const s = all[i].status;
      if (s === "running" || s === "cancelling") n++;
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
      progressEvents: 0,
      backwardsJumps: 0,
      lastDoneBytes: 0,
      lastPhase: "",
    };
    this.jobs.set(id, job);
    this.emit();
    return id;
  }

  /** Cancel an op. Synchronous state flip; the IPC call never blocks the UI. */
  cancel(id: JobId): void {
    const job = this.jobs.get(id);
    if (!job) return;
    if (job.status !== "running") return;
    job.status = "cancelling";
    job.cancelRequestedAt = now();
    job.phaseAtCancel = job.phase;
    this.cancelled.add(id);
    this.emit();
    // Fire-and-forget by design: cancel must be reachable while an op runs
    // and must not itself block on the backend.
    this.ipc.opCancel(id).then(noop, noop);
    // The engine leaves a partial destination behind on cancel by design.
    // Say so now, not after the user discovers a half-copied file: the job
    // settles locally if the backend sends nothing more (its Channel sends
    // are discarded when the webview is busy, so a terminal event can be
    // genuinely lost — the timer is the safety net for that case, and the
    // note says which source settled the job).
    const self = this;
    setTimeout(function () {
      const j = self.jobs.get(id);
      if (j && j.status === "cancelling") {
        j.status = "cancelled";
        j.settleSource = "local";
        j.summary =
          "Cancelled — the destination may be partially written; " +
          "check it and clean up what you do not need.";
        self.emit();
        self.view.onSettled();
      }
    }, 400);
  }

  /** Drop a terminal job row from the list (dismiss button). */
  dismiss(id: JobId): void {
    const job = this.jobs.get(id);
    if (!job) return;
    if (job.status === "running" || job.status === "cancelling") return;
    this.jobs.delete(id);
    this.cancelled.delete(id);
    this.emit();
  }

  /** Cheap + sync: stamp counters, notify. No DOM, no await. */
  private onOpEvent(ev: OpEventDto): void {
    const job = this.jobs.get(ev.id);
    if (!job) return; // stale: superseded scan-era job or unknown id
    if (job.status === "done" || job.status === "failed") return;
    if (this.cancelled.has(ev.id) && ev.type !== "error") {
      // A late done/progress after the user cancelled stays cancelled —
      // never flip a cancelled job back to running.
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
    } else if (ev.type === "done") {
      if (this.cancelled.has(ev.id)) return;
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
      // note stands, and nothing goes to the error sink.
      if (this.cancelled.has(ev.id) && isCancelKind(ev.error.kind)) {
        job.status = "cancelled";
        job.settleSource = "backend";
        job.terminalEventAt = now();
        job.summary =
          ev.error.message ||
          "Cancelled — the destination may be partially written; " +
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

/**
 * Backend error kinds are frozen snake_case (`already_exists`, `cancelled`,
 * `permission_denied` — dto.rs). Match case- and underscore-insensitively so
 * a capitalisation drift (mock or future backend) degrades to the same UI,
 * never to a missed branch.
 */
function flatKind(kind: string): string {
  return (kind || "").toLowerCase().split("_").join("");
}

/** Collision error: the op failed, the destination was left untouched. */
export function isAlreadyExistsKind(kind: string): boolean {
  return flatKind(kind) === "alreadyexists";
}

/** Cancel report: terminal, but not a failure — see onOpEvent. */
function isCancelKind(kind: string): boolean {
  const f = flatKind(kind);
  return f === "cancelled" || f === "canceled";
}
