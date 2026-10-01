/* Kestrel Phase 1 — IPC layer. THE one-file abstraction.
 *
 * Everything UI-side talks to the backend through `KestrelIpc`. If the Rust
 * command names, argument keys, or DTO shapes ever change, edit this file
 * only: no other module imports @tauri-apps/api directly.
 *
 * Contract (MIGRATION.md §3):
 *   scan_start(path, options, events: Channel<ScanEventDto>) -> JobId (u64)
 *   scan_cancel(id: JobId) -> ()
 *   stat(path: string) -> FileEntryDto
 *   open_path(path: string) -> { entered_dir: boolean }
 *   op_start(req: OpRequestDto, events: Channel<OpEventDto>) -> JobId (u64)
 *   op_cancel(id: JobId) -> ()
 *   op_collision_answer(id: JobId, dst: string, decision: Decision) -> ()   [3b]
 *
 * Backpressure (MIGRATION.md §3, load-bearing): the backend pump task drains
 * with try_recv and never blocks on the frontend. Our side of that deal is
 * that the Channel callback must be CHEAP and SYNC — push into a buffer and
 * return. All accumulation / DOM work happens on BatchEnd / Complete via
 * requestAnimationFrame in browser.ts. Never await, never touch the DOM,
 * never allocate big structures inside onEvent.
 */

import type {
  CmdError,
  CollisionDecision,
  CollisionKindDto,
  FileEntryDto,
  JobId,
  OpenPathResult,
  OpEventDto,
  OpPhaseDto,
  OpRequestDto,
  ScanEventDto,
  ScanOptionsDto,
  SortSpecDto,
  WatchEventDto,
} from "./types";

export type ScanEventHandler = (event: ScanEventDto) => void;
export type WatchEventHandler = (event: WatchEventDto) => void;
export type OpEventHandler = (event: OpEventDto) => void;

/** One answered collision, for the mock's introspection list. */
export interface CollisionAnswer {
  id: JobId;
  dst: string;
  decision: CollisionDecision;
}

/** Mock-side state for one in-flight op. See `MockIpc.opJobs`. */
interface MockOpJob {
  timers: number[];
  cancelled: boolean;
  /** Set while the mock is blocked awaiting an answer for exactly one dst. */
  pending: { dst: string; kind: CollisionKindDto } | null;
  /** Runs when the pending collision is answered; null when not parked. */
  resume: ((decision: CollisionDecision) => void) | null;
  req: OpRequestDto;
  onEvent: OpEventHandler;
}

export interface KestrelIpc {
  readonly backend: "tauri" | "mock";
  scanStart(
    path: string,
    options: ScanOptionsDto,
    onEvent: ScanEventHandler
  ): Promise<JobId>;
  scanCancel(id: JobId): Promise<void>;
  stat(path: string): Promise<FileEntryDto>;
  openPath(path: string): Promise<OpenPathResult>;
  /** Frozen Phase 2 contract: subscribe to change events for `path`. */
  watchSubscribe(path: string, onEvent: WatchEventHandler): Promise<JobId>;
  /** Frozen Phase 2 contract: drop a subscription. Never rejects. */
  watchUnsubscribe(id: JobId): Promise<void>;
  /** Frozen Phase 3a contract: start a copy/move/trash/delete op, streaming
   * progress over a Channel. Returns the backend-minted JobId (u64). */
  opStart(req: OpRequestDto, onEvent: OpEventHandler): Promise<JobId>;
  /** Frozen Phase 3a contract: cancel an op. Never rejects. The engine
   * leaves a partial destination behind on cancel by design. */
  opCancel(id: JobId): Promise<void>;
  /** Frozen Phase 3b contract: every collision an in-flight op is currently
   *  blocked on, as `{id, dst, kind}` rows. The RESYNCHRONISATION point: a
   *  `collision` event is fire-and-forget over a Channel, so a UI that mounted
   *  or reloaded while an op was paused never saw the question and has no other
   *  way to learn one is waiting. Empty is the normal answer.
   *
   *  Added because the Rust command of this name exists; it is optional in the
   *  interface sense only in that an older backend simply has no rows to
   *  report. Never rejects: a missing command is "nothing pending". */
  opPendingCollisions(): Promise<Array<{
    id: JobId;
    dst: string;
    kind: CollisionKindDto;
  }>>;
  /** Frozen Phase 3b contract: settle ONE colliding destination.
   *
   * `dst` is not decoration. The engine keys `AnsweredCollisions` by path, so
   * answering the wrong `dst` leaves the real one blocking forever. Rejects
   * only if the command itself is missing; the caller treats that as fatal to
   * the dialog but not to the op. */
  opCollisionAnswer(
    id: JobId,
    dst: string,
    decision: CollisionDecision
  ): Promise<void>;
}

/* ------------------------------------------------------------------ */
/* Wire normalisation                                                  */
/* ------------------------------------------------------------------ */

/**
 * Canonical wire entry → frontend form.
 *
 * The backend (`src-tauri/src/dto.rs`, frozen) sends lowercase `kind`
 * (`"directory"|"file"|"symlink"|"other"`), camelCase `isDirTarget`, and no
 * `descendable` at all; the mock/fixture path uses capitalised kinds,
 * snake_case `is_dir_target`, and a real `descendable`. Both are accepted:
 * kinds are canonicalised, `isDirTarget`/`is_dir_target` are merged, and a
 * missing `descendable` is derived the way the engine does
 * (`FileEntry::is_descendable`: a real directory, or a symlink whose target
 * is a directory). Returns null when the payload is not an entry at all.
 */
export function normalizeEntry(raw: unknown): FileEntryDto | null {
  if (!raw || typeof raw !== "object") return null;
  const o = raw as Record<string, unknown>;
  if (typeof o["name"] !== "string" || typeof o["path"] !== "string")
    return null;
  const kind = canonicalKind(o["kind"]);
  const idtRaw =
    "isDirTarget" in o ? o["isDirTarget"] : o["is_dir_target"];
  const is_dir_target = typeof idtRaw === "boolean" ? idtRaw : null;
  const descRaw = o["descendable"];
  const descendable =
    typeof descRaw === "boolean"
      ? descRaw
      : kind === "Directory" ||
        (kind === "Symlink" && is_dir_target === true);
  const size = typeof o["size"] === "number" ? o["size"] : null;
  const modified = typeof o["modified"] === "number" ? o["modified"] : null;
  return {
    name: o["name"],
    path: o["path"],
    kind,
    size,
    modified,
    hidden: o["hidden"] === true,
    is_dir_target,
    descendable,
  };
}

function canonicalKind(raw: unknown): string {
  const lower =
    typeof raw === "string" ? raw.toLowerCase() : "";
  if (lower === "directory") return "Directory";
  if (lower === "file") return "File";
  if (lower === "symlink") return "Symlink";
  if (lower === "other") return "Other";
  // Tolerate future kinds verbatim rather than dropping the row.
  return typeof raw === "string" ? raw : "";
}

/** Flat backend error or nested mock error → one CmdError. */
function normalizeError(obj: Record<string, unknown>): CmdError | null {
  const nested = obj["error"];
  const src =
    nested && typeof nested === "object"
      ? (nested as Record<string, unknown>)
      : obj;
  if (typeof src["message"] !== "string") return null;
  const pathRaw = src["path"];
  return {
    kind: typeof src["kind"] === "string" ? (src["kind"] as string) : "unknown",
    path: typeof pathRaw === "string" ? pathRaw : null,
    message: src["message"] as string,
  };
}

function normalizeEntriesArray(raw: unknown): FileEntryDto[] | null {
  if (!Array.isArray(raw)) return null;
  const out: FileEntryDto[] = [];
  for (let i = 0; i < raw.length; i++) {
    const e = normalizeEntry(raw[i]);
    if (e) out.push(e);
  }
  return out;
}

/**
 * Backend wire form (authoritative — `src-tauri/src/dto.rs`, frozen):
 *   {"type":"entries","entries":[…]} | {"type":"error",path,kind,message} |
 *   {"type":"complete","total":n}
 * plus, for the mock/fixture path and older shapes, every variant below:
 *   {"Entry": {...}} | {"Entries": […]} | {"BatchEnd": …} | {"Error": {...}} |
 *   {"Complete": {"total": n}} | "BatchEnd" (bare unit variant) |
 *   already-normalised {type:"Entry",…} / {type:"Entries",…} / {type:"BatchEnd"}
 *   / {type:"Error",…} / {type:"Complete",…} (mock path / tests).
 * `type` matches case-insensitively; entry payloads always go through
 * `normalizeEntry`, so wire renames never drop a row silently.
 */
export function normalizeScanEvent(raw: unknown): ScanEventDto | null {
  if (!raw || typeof raw !== "object") {
    if (raw === "BatchEnd") return { type: "BatchEnd" };
    return null;
  }
  const obj = raw as Record<string, unknown>;
  if (typeof obj["type"] === "string") {
    const t = (obj["type"] as string).toLowerCase();
    if (t === "entries") {
      const entries = normalizeEntriesArray(obj["entries"]);
      return entries ? { type: "Entries", entries } : null;
    }
    if (t === "entry" && obj["entry"]) {
      const entry = normalizeEntry(obj["entry"]);
      return entry ? { type: "Entry", entry } : null;
    }
    if (t === "batchend" || t === "batch_end") return { type: "BatchEnd" };
    if (t === "error") {
      const error = normalizeError(obj);
      return error ? { type: "Error", error } : null;
    }
    if (t === "complete") {
      return {
        type: "Complete",
        total: typeof obj["total"] === "number" ? obj["total"] : -1,
      };
    }
    return null;
  }
  if (obj["Entry"] && typeof obj["Entry"] === "object") {
    const entry = normalizeEntry(obj["Entry"]);
    return entry ? { type: "Entry", entry } : null;
  }
  if (obj["Entries"] && Array.isArray(obj["Entries"])) {
    const entries = normalizeEntriesArray(obj["Entries"]);
    return entries ? { type: "Entries", entries } : null;
  }
  if (obj["Error"] && typeof obj["Error"] === "object") {
    const error = normalizeError(obj);
    return error ? { type: "Error", error } : null;
  }
  if (obj["Complete"] && typeof obj["Complete"] === "object") {
    const inner = obj["Complete"] as Record<string, unknown>;
    return {
      type: "Complete",
      total: typeof inner["total"] === "number" ? inner["total"] : -1,
    };
  }
  if ("BatchEnd" in obj) return { type: "BatchEnd" };
  return null;
}

/**
 * Watch wire form (frozen Phase 2 contract, internally tagged like scan):
 *   {"type":"changed","dirs":[…]} | {"type":"error",path,kind,message}
 * (flat error, same shape as scan errors). Tolerates the mock/fixture
 * capitalised variants the way normalizeScanEvent does.
 */
export function normalizeWatchEvent(raw: unknown): WatchEventDto | null {
  if (!raw || typeof raw !== "object") return null;
  const obj = raw as Record<string, unknown>;
  // Tolerate a nested mock naming ({Changed: […]}), mirroring scan shapes.
  if (obj["Changed"] && Array.isArray(obj["Changed"])) {
    return { type: "changed", dirs: stringsOf(obj["Changed"]) };
  }
  if (typeof obj["type"] !== "string") return null;
  const t = (obj["type"] as string).toLowerCase();
  if (t === "changed") {
    const rawDirs = obj["dirs"];
    if (!Array.isArray(rawDirs)) return null;
    return { type: "changed", dirs: stringsOf(rawDirs) };
  }
  if (t === "error") {
    const error = normalizeError(obj);
    return error ? { type: "error", error } : null;
  }
  return null;
}

function stringsOf(raw: unknown[]): string[] {
  const out: string[] = [];
  for (let i = 0; i < raw.length; i++)
    if (typeof raw[i] === "string") out.push(raw[i] as string);
  return out;
}

/**
 * Op wire form (frozen Phase 3a/3b contract, internally tagged with "type",
 * matching the scan/watch normalisers):
 *   {"type":"progress",id,phase,doneBytes,totalBytes,doneItems,totalItems} |
 *   {"type":"done",id,summary} |
 *   {"type":"error",id,error:{kind,path,message}} (nested, like watch) —
 *   a flat error shape is also accepted via normalizeError |
 *   {"type":"collision",id,dst,kind} (Phase 3b)
 * `type` and `phase` match case-insensitively; `phase` falls back to
 * "copying" when the backend sends an unknown value rather than dropping
 * the event. Numbers that arrive missing/not-a-number become 0 (an unknown
 * total renders as indeterminate — never a guessed percentage).
 *
 * `collision` is the one variant that REQUIRES a field: a collision with no
 * `dst` string is dropped (returns null) rather than surfaced with an empty
 * destination. That is deliberate and load-bearing. `op_collision_answer` is
 * keyed by (id, dst); showing a dialog for a path we cannot name produces an
 * answer that settles nothing, the engine stays blocked, and the UI would be
 * showing a choice that does nothing. Dropping the event keeps the frozen
 * 3a behaviour — the op ends with `already_exists` and nothing is overwritten.
 * `kind` degrades to "other" rather than dropping the event: the kind only
 * chooses the wording, never whether the answer can be routed.
 */
export function normalizeOpEvent(raw: unknown): OpEventDto | null {
  if (!raw || typeof raw !== "object") return null;
  const obj = raw as Record<string, unknown>;
  if (typeof obj["type"] !== "string") return null;
  const t = (obj["type"] as string).toLowerCase();
  const id = numOf(obj["id"]);
  if (t === "progress") {
    return {
      type: "progress",
      id,
      phase: canonicalPhase(obj["phase"]),
      doneBytes: numOf(obj["doneBytes"]),
      totalBytes: numOf(obj["totalBytes"]),
      doneItems: numOf(obj["doneItems"]),
      totalItems: numOf(obj["totalItems"]),
    };
  }
  if (t === "done") {
    const s = obj["summary"];
    return { type: "done", id, summary: typeof s === "string" ? s : "" };
  }
  if (t === "collision") {
    const dst = obj["dst"];
    // The one hard requirement. See the note above.
    if (typeof dst !== "string" || dst === "") return null;
    return {
      type: "collision",
      id,
      dst,
      kind: canonicalCollisionKind(obj["kind"]),
    };
  }
  if (t === "error") {
    const error = normalizeError(obj);
    return error ? { type: "error", id, error } : null;
  }
  return null;
}

/** `kind` on the wire is lowercase in the frozen contract; accept the
 *  capitalised spellings the mock path uses and the ones a Rust `Kind` enum
 *  might serialise, and fall back to "other" rather than dropping the event. */
function canonicalCollisionKind(raw: unknown): CollisionKindDto {
  const k = typeof raw === "string" ? raw.toLowerCase().split("_").join("") : "";
  if (k === "file") return "file";
  if (k === "dir" || k === "directory") return "dir";
  if (k === "symlink" || k === "link") return "symlink";
  return "other";
}

function numOf(v: unknown): number {
  return typeof v === "number" && !isNaN(v) ? v : 0;
}

function canonicalPhase(raw: unknown): OpPhaseDto {
  const p = typeof raw === "string" ? raw.toLowerCase() : "";
  if (p === "measuring") return "measuring";
  if (p === "deleting") return "deleting";
  return "copying";
}

/* ------------------------------------------------------------------ */
/* Tauri backend                                                       */
/* ------------------------------------------------------------------ */

interface TauriCore {
  invoke: <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;
  Channel: new (onmessage?: (raw: unknown) => void) => unknown;
}

class TauriIpc implements KestrelIpc {
  readonly backend = "tauri" as const;

  // Dynamic import: the module must also run in a plain browser (Vite
  // preview, fixture mode `?rows=N`) where __TAURI__ does not exist.
  private api: Promise<TauriCore> | null = null;

  private load(): Promise<TauriCore> {
    if (!this.api)
      this.api = import("@tauri-apps/api/core") as Promise<TauriCore>;
    return this.api;
  }

  async scanStart(
    path: string,
    options: ScanOptionsDto,
    onEvent: ScanEventHandler
  ): Promise<JobId> {
    const { invoke, Channel } = await this.load();
    // Channel callback: keep it minimal per the backpressure contract.
    const events = new Channel((raw: unknown) => {
      const ev = normalizeScanEvent(raw);
      if (ev) onEvent(ev);
    });
    // Argument keys must match the Rust parameter names exactly.
    const id = await invoke<JobId>("scan_start", { path, options, events });
    return typeof id === "number" ? id : Number(id);
  }

  async scanCancel(id: JobId): Promise<void> {
    const { invoke } = await this.load();
    await invoke("scan_cancel", { id });
  }

  async stat(path: string): Promise<FileEntryDto> {
    const { invoke } = await this.load();
    const raw = await invoke<unknown>("stat", { path });
    const entry = normalizeEntry(raw);
    if (!entry) throw new Error("stat: unexpected entry shape for " + path);
    return entry;
  }

  async openPath(path: string): Promise<OpenPathResult> {
    const { invoke } = await this.load();
    // The backend serialises camelCase `enteredDir` (dto.rs OpenResultDto);
    // accept snake_case too so the shape rename can never invert navigation.
    const res = await invoke<Record<string, unknown>>("open_path", { path });
    const entered = res["enteredDir"] !== undefined ? res["enteredDir"] : res["entered_dir"];
    return { entered_dir: entered === true };
  }

  async watchSubscribe(
    path: string,
    onEvent: WatchEventHandler
  ): Promise<JobId> {
    const { invoke, Channel } = await this.load();
    // Same backpressure deal as scan: the Channel callback stays cheap and
    // sync — the browser re-scans on a later task, never here.
    const events = new Channel((raw: unknown) => {
      const ev = normalizeWatchEvent(raw);
      if (ev) onEvent(ev);
    });
    // Argument keys must match the Rust parameter names exactly.
    const id = await invoke<JobId>("watch_subscribe", { path, events });
    return typeof id === "number" ? id : Number(id);
  }

  async watchUnsubscribe(id: JobId): Promise<void> {
    const { invoke } = await this.load();
    await invoke("watch_unsubscribe", { id });
  }

  async opStart(req: OpRequestDto, onEvent: OpEventHandler): Promise<JobId> {
    const { invoke, Channel } = await this.load();
    // Same backpressure deal as scan/watch: the Channel callback stays cheap
    // and sync — the ops controller buffers in a plain record, DOM work
    // happens on later tasks, never here.
    const progress = new Channel((raw: unknown) => {
      const ev = normalizeOpEvent(raw);
      if (ev) onEvent(ev);
    });
    // Argument keys must match the Rust parameter names exactly:
    // op_start(req, progress) -> JobId.
    const id = await invoke<JobId>("op_start", { req, progress });
    return typeof id === "number" ? id : Number(id);
  }

  async opCancel(id: JobId): Promise<void> {
    const { invoke } = await this.load();
    await invoke("op_cancel", { id });
  }

  async opCollisionAnswer(
    id: JobId,
    dst: string,
    decision: CollisionDecision
  ): Promise<void> {
    const { invoke } = await this.load();
    // Argument keys must match the Rust parameter names exactly (commands.rs
    // `op_collision_answer(app, id, dst, decision)`). `dst` travels with `id`
    // because the engine's `AnsweredCollisions` map is keyed by destination
    // path — an answer without it could not be matched to the collision it
    // settles. The decision is sent as the frozen lowercase wire string.
    await invoke("op_collision_answer", { id, dst, decision });
  }

  async opPendingCollisions(): Promise<
    Array<{ id: JobId; dst: string; kind: CollisionKindDto }>
  > {
    const { invoke } = await this.load();
    try {
      const raw = await invoke<unknown>("op_pending_collisions");
      return normalizePendingCollisions(raw);
    } catch {
      // A backend without the command has nothing to report. An empty list is
      // the documented normal answer, so a failure here must not be an error
      // the UI has to explain.
      return [];
    }
  }
}

/**
 * Pending-collision rows straight off the wire. Same tolerance as the other
 * normalisers: a row without a `dst` is dropped, because a question we cannot
 * name is one we cannot answer (the same rule as `normalizeOpEvent`'s
 * collision branch, and for the same reason).
 */
export function normalizePendingCollisions(raw: unknown): Array<{
  id: JobId;
  dst: string;
  kind: CollisionKindDto;
}> {
  if (!Array.isArray(raw)) return [];
  const out: Array<{ id: JobId; dst: string; kind: CollisionKindDto }> = [];
  for (let i = 0; i < raw.length; i++) {
    const row = raw[i] as Record<string, unknown> | null;
    if (!row || typeof row !== "object") continue;
    const dst = row["dst"];
    if (typeof dst !== "string" || dst === "") continue;
    out.push({
      id: numOf(row["id"]),
      dst,
      kind: canonicalCollisionKind(row["kind"]),
    });
  }
  return out;
}

/* ------------------------------------------------------------------ */
/* Mock backend (no Tauri runtime: fixture mode, Vite preview)          */
/* ------------------------------------------------------------------ */

function mockEntry(dir: string, name: string, i: number): FileEntryDto {
  const isDir = i % 50 === 49;
  return {
    name,
    path: dir + "/" + name,
    kind: isDir ? "Directory" : "File",
    size: isDir ? null : (i * 7919) % 1048576,
    modified: Date.now() - i * 86400000,
    hidden: name.charAt(0) === ".",
    is_dir_target: null,
    descendable: isDir,
  };
}

class MockIpc implements KestrelIpc {
  readonly backend = "mock" as const;
  private nextId = 1;
  private cancelled = new Set<JobId>();
  private watches = new Map<JobId, { path: string; onEvent: WatchEventHandler }>();

  async scanStart(
    path: string,
    options: ScanOptionsDto,
    onEvent: ScanEventHandler
  ): Promise<JobId> {
    const id = this.nextId++;
    // Honour the same query parameters the real backend takes, so the UI is
    // testable standalone: hidden filtering and backend-side sorting happen
    // here exactly where scan.rs does them in production.
    const showHidden =
      options.showHidden === undefined ? true : options.showHidden;
    const sort: SortSpecDto =
      options.sort === undefined || options.sort === null
        ? { key: "name", ascending: true, dirsFirst: true }
        : options.sort;
    const names = [".hidden-note", "README.md", "src", "docs", "assets"];
    const all: FileEntryDto[] = [];
    for (let i = 0; i < names.length + 40; i++) {
      const name = i < names.length ? names[i] : "file-" + i + ".txt";
      const e = mockEntry(path, name, i);
      if (!showHidden && e.hidden) continue;
      all.push(e);
    }
    mockSort(all, sort);
    // Emit asynchronously in chunks so the streaming path (Entries…
    // Complete) is exercised exactly like the real backend. Each timer
    // callback stays cheap: at most one small batch per macrotask.
    let i = 0;
    let errorSent = false;
    const step = () => {
      if (this.cancelled.has(id)) return;
      const BATCH = 200;
      const slice: FileEntryDto[] = [];
      for (let k = 0; k < BATCH && i < all.length; k++, i++) slice.push(all[i]);
      if (slice.length > 0) onEvent({ type: "Entries", entries: slice });
      // One synthetic permission error proves error rows render.
      if (!errorSent && i >= 10) {
        errorSent = true;
        onEvent({
          type: "Error",
          error: {
            kind: "PermissionDenied",
            path: path + "/denied-dir",
            message: "Permission denied (mock)",
          },
        });
      }
      onEvent({ type: "BatchEnd" });
      if (i < all.length) {
        setTimeout(step, 0);
      } else {
        onEvent({ type: "Complete", total: all.length });
      }
    };
    setTimeout(step, 0);
    return id;
  }

  async scanCancel(id: JobId): Promise<void> {
    this.cancelled.add(id);
  }

  async stat(path: string): Promise<FileEntryDto> {
    const name = path.split("/").pop() || path;
    return {
      name,
      path,
      kind: "File",
      size: 1234,
      modified: Date.now(),
      hidden: false,
      is_dir_target: null,
      descendable: false,
    };
  }

  async openPath(path: string): Promise<OpenPathResult> {
    void path;
    return { entered_dir: false };
  }

  async watchSubscribe(
    path: string,
    onEvent: WatchEventHandler
  ): Promise<JobId> {
    const id = this.nextId++;
    this.watches.set(id, { path, onEvent });
    return id;
  }

  async watchUnsubscribe(id: JobId): Promise<void> {
    this.watches.delete(id);
  }

/* ---- Mock ops: progress stream, cancel, and the 3b collision handshake ---- */

  /**
   * A live mock op. `pending`/`resume` are the collision handshake: the mock
   * parks with `pending` set and `resume` waiting, and `op_collision_answer`
   * is the only thing that lets the stream continue. That is what makes the
   * "sequential, not batched" rule testable — a second collision parks again
   * rather than inheriting the first answer.
   */
  private opJobs = new Map<JobId, MockOpJob>();
  /** Every answer the mock was asked to settle. Introspection only. */
  private answerLog: CollisionAnswer[] = [];

  /**
   * Mock `op_start`: streams a realistic event sequence without a backend —
   * `measuring` progress events with `totalBytes: 0` (unknown total, must
   * render indeterminate), then `copying`/`deleting` progress with a real
   * total, then `done`.
   *
   * A `copy`/`move` whose destination basename collides with a mock listing
   * name BLOCKS: it emits a `collision` event and parks until
   * `op_collision_answer` names that exact `dst`. `skip` and `overwrite` both
   * resume the stream (the mock honours exactly the one path it was asked
   * about and does not model the rest of the engine's `AnsweredCollisions`
   * map); `abort` ends the op with a terminal `cancelled` error.
   *
   * The 3a `already_exists` policy is deliberately not simulated here: under
   * 3b the engine asks rather than fails, and the seam suite still drives the
   * real `already_exists` bytes through the real normalizer and the real
   * OpManager, so that path stays covered.
   *
   * Dev-only `?opslow=1` stretches every delay 8× so the measuring and
   * collision windows are wide enough to observe. Mock only; zero production
   * impact.
   */
  async opStart(req: OpRequestDto, onEvent: OpEventHandler): Promise<JobId> {
    const id = this.nextId++;
    const slow = mockOpSlow() ? 8 : 1;
    const job: MockOpJob = {
      timers: [],
      cancelled: false,
      pending: null,
      resume: null,
      req,
      onEvent,
    };
    this.opJobs.set(id, job);
    const later = (ms: number, fn: () => void): void => {
      const t = setTimeout(() => {
        if (!job.cancelled) fn();
      }, ms * slow) as unknown as number;
      job.timers.push(t);
    };

    /** Emit progress up to `done`, then the terminal `done` event. */
    const stream = (): void => {
      const phase =
        req.op === "delete" || req.op === "trash" ? "deleting" : "copying";
      const totalBytes = 8 * 1048576;
      const totalItems = 24;
      // Measuring window: no total yet (totalBytes 0) — the UI must show an
      // indeterminate state here, not a guessed percentage.
      later(60, () =>
        job.onEvent({
          type: "progress",
          id,
          phase: "measuring",
          doneBytes: 0,
          totalBytes: 0,
          doneItems: 0,
          totalItems: 0,
        })
      );
      later(220, () =>
        job.onEvent({
          type: "progress",
          id,
          phase: "measuring",
          doneBytes: 0,
          totalBytes: 0,
          doneItems: 3,
          totalItems: 0,
        })
      );
      // Measured phase: monotonic progress toward a real total.
      for (let step = 1; step <= 8; step++) {
        const s = step;
        later(220 + s * 160, () =>
          job.onEvent({
            type: "progress",
            id,
            phase: phase as "copying" | "deleting",
            doneBytes: Math.floor((totalBytes * s) / 8),
            totalBytes,
            doneItems: Math.floor((totalItems * s) / 8),
            totalItems,
          })
        );
      }
      later(220 + 9 * 160, () => {
        job.onEvent({ type: "done", id, summary: mockSummary(req) });
        this.opJobs.delete(id);
      });
    };

    if ((req.op === "copy" || req.op === "move") && mockCollides(req.dst)) {
      later(150, () =>
        this.parkOnCollision(id, req.dst, "file", (decision) => {
          if (decision === "abort") {
            // The engine unblocks and reports the abort terminally, exactly as
            // it reports a user cancel: one `cancelled` error per op.
            job.onEvent({
              type: "error",
              id,
              error: {
                kind: "cancelled",
                path: req.dst,
                message: "aborted at a collision on " + req.dst + " (mock)",
              },
            });
            this.opJobs.delete(id);
            return;
          }
          stream();
        })
      );
      return id;
    }
    stream();
    return id;
  }

  /** Mock `op_cancel`: stop the stream, report the by-design partial.
   *  A parked mock is blocked inside the collision handshake, so cancel has to
   *  release it — otherwise the op would hang with no terminal event. */
  async opCancel(id: JobId): Promise<void> {
    const job = this.opJobs.get(id);
    if (!job || job.cancelled) return;
    job.cancelled = true;
    const resume = job.resume;
    job.pending = null;
    job.resume = null;
    if (resume) resume("abort");
    for (let i = 0; i < job.timers.length; i++) clearTimeout(job.timers[i]);
    this.opJobs.delete(id);
  }

  /**
   * Mock `op_collision_answer`: settle exactly one parked collision.
   *
   * Every way this can be wrong is dropped rather than guessed:
   *   * no such job (the op already settled),
   *   * nothing parked (the answer arrived late, or the same one twice),
   *   * a `dst` that is not the parked one — the engine keys its answers by
   *     destination path, so an answer naming another path settles nothing,
   *     and applying it anyway would be the data-loss bug this seam exists to
   *     prevent,
   *   * the job was cancelled.
   */
  async opCollisionAnswer(
    id: JobId,
    dst: string,
    decision: CollisionDecision
  ): Promise<void> {
    this.answerLog.push({ id, dst, decision });
    const job = this.opJobs.get(id);
    if (!job || job.cancelled) return;
    if (!job.pending) return;
    if (!samePath(job.pending.dst, dst)) return;
    const resume = job.resume;
    job.pending = null;
    job.resume = null;
    if (resume) resume(decision);
  }

  /** Real collisions parked right now, as `{id, dst, kind}` rows — the shape
   *  `op_pending_collisions` returns on the Tauri backend. The discovery half
   *  of the round-trip: a `collision` event is fire-and-forget over a
   *  Channel, so a UI that mounted (or reloaded) while an op was paused never
   *  saw the question. The dialog re-syncs from here; the mock answers it
   *  from its own parked set so the two paths agree. */
  async opPendingCollisions(): Promise<
    Array<{ id: JobId; dst: string; kind: CollisionKindDto }>
  > {
    const out: Array<{ id: JobId; dst: string; kind: CollisionKindDto }> = [];
    this.opJobs.forEach((job, id) => {
      if (job.cancelled || !job.pending) return;
      out.push({ id, dst: job.pending.dst, kind: job.pending.kind });
    });
    return out;
  }

  /** Emit a `collision` event and park until `settle` is called with the
   *  answer. Shared by the dst-name heuristic and `simulateCollision`. */
  private parkOnCollision(
    id: JobId,
    dst: string,
    kind: CollisionKindDto,
    settle: (decision: CollisionDecision) => void
  ): void {
    const job = this.opJobs.get(id);
    if (!job || job.cancelled || job.pending) return;
    job.pending = { dst, kind };
    job.resume = settle;
    job.onEvent({ type: "collision", id, dst, kind });
  }

  /**
   * Dev-only: make a live op hit a collision right now, whatever its
   * destination is named. Lets the 3b dialog be exercised from devtools or
   * automation on any op, and with any `kind`, so the folder and symlink
   * wordings are reachable too. Returns false when no op is waiting to ask.
   */
  simulateCollision(dst: string, kind: CollisionKindDto = "file"): boolean {
    const ids = Array.from(this.opJobs.keys());
    for (let i = 0; i < ids.length; i++) {
      const job = this.opJobs.get(ids[i]);
      if (!job || job.cancelled || job.pending) continue;
      const id = ids[i];
      const req = job.req;
      this.parkOnCollision(id, dst, kind, (decision) => {
        if (decision === "abort") {
          job.onEvent({
            type: "error",
            id,
            error: {
              kind: "cancelled",
              path: dst,
              message: "aborted at a collision on " + dst + " (mock)",
            },
          });
          this.opJobs.delete(id);
          return;
        }
        job.onEvent({ type: "done", id, summary: mockSummary(req) });
        this.opJobs.delete(id);
      });
      return true;
    }
    return false;
  }

  /** Introspection for devtools / tests: the answers the mock was given. */
  collisionAnswers(): CollisionAnswer[] {
    return this.answerLog.slice();
  }

  /**
   * Dev-only liveness hook: simulate a backend `changed` event, delivered to
   * every live subscription whose path is listed (the browser re-scans only
   * when the viewed directory is among them — the same rule as production).
   * No backend needed; wire it to a button or `__kestrel` in main.ts.
   */
  simulateChanged(dirs: string[]): void {
    const live = Array.from(this.watches.entries());
    for (let i = 0; i < live.length; i++) {
      const sub = live[i][1];
      let hit = false;
      for (let k = 0; k < dirs.length; k++)
        if (sameDir(dirs[k], sub.path)) {
          hit = true;
          break;
        }
      if (hit) sub.onEvent({ type: "changed", dirs: dirs.slice() });
    }
  }

  /** Introspection for tests / devtools: live subscription count. */
  watchCount(): number {
    return this.watches.size;
  }
}

/** Trailing-slash-insensitive path comparison. Used for watch dirs AND for
 *  collision answers: the engine keys `AnsweredCollisions` by path, so an
 *  answer whose `dst` differs only by a trailing slash must still match. */
function samePath(a: string, b: string): boolean {
  return stripSlash(a) === stripSlash(b);
}

/** Trailing-slash-insensitive directory comparison. */
function sameDir(a: string, b: string): boolean {
  return samePath(a, b);
}

/**
 * Mock collision rule: the mock listing holds README.md, src, docs, assets
 * (plus file-N.txt). A copy/move onto one of those basenames fails with
 * AlreadyExists — deterministic and demonstrable from the destination
 * prompt without a backend.
 */
function mockCollides(dst: string): boolean {
  const base = dst.split("/").pop() || "";
  const known = ["README.md", "src", "docs", "assets", ".hidden-note"];
  for (let i = 0; i < known.length; i++)
    if (base === known[i]) return true;
  return false;
}

function mockSummary(req: OpRequestDto): string {
  if (req.op === "copy") return "Copied " + req.src + " to " + req.dst + " (mock)";
  if (req.op === "move") return "Moved " + req.src + " to " + req.dst + " (mock)";
  if (req.op === "trash") return "Trashed " + req.src + " (mock)";
  return "Deleted " + req.src + " (mock)";
}

/** Dev-only mock timing control (`?opslow=1`): mock seam only. */
function mockOpSlow(): boolean {
  try {
    return new URLSearchParams(window.location.search).get("opslow") === "1";
  } catch {
    return false;
  }
}

function stripSlash(p: string): string {
  let s = p;
  while (s.length > 1 && s.charAt(s.length - 1) === "/") s = s.slice(0, -1);
  return s;
}

/**
 * Backend-side sort for the mock path only: mirrors scan.rs semantics —
 * directories first, then the column, nulls (unknown size/date) last,
 * ascending flag applied to the column comparison only.
 */
export function mockSort(rows: FileEntryDto[], sort: SortSpecDto): void {
  rows.sort((a, b) => {
    if (sort.dirsFirst) {
      const ad = a.descendable ? 1 : 0;
      const bd = b.descendable ? 1 : 0;
      if (ad !== bd) return bd - ad;
    }
    let c = 0;
    if (sort.key === "name") c = cmpStr(a.name, b.name);
    else if (sort.key === "size") c = cmpNullNum(a.size, b.size);
    else if (sort.key === "modified") c = cmpNullNum(a.modified, b.modified);
    else c = cmpStr(a.kind, b.kind);
    return sort.ascending ? c : -c;
  });
}

function cmpStr(a: string, b: string): number {
  const al = a.toLowerCase();
  const bl = b.toLowerCase();
  if (al < bl) return -1;
  if (al > bl) return 1;
  return 0;
}

/** Nulls sort last regardless of direction (matches the engine). */
function cmpNullNum(a: number | null, b: number | null): number {
  if (a === null || a === undefined) return b === null || b === undefined ? 0 : 1;
  if (b === null || b === undefined) return -1;
  return a - b;
}

/* ------------------------------------------------------------------ */
/* Factory                                                             */
/* ------------------------------------------------------------------ */

let singleton: KestrelIpc | null = null;

/** True only inside the Tauri webview. Anything else gets the mock.
 *
 * `window.__TAURI__` must NOT be the probe: it exists only when
 * `app.withGlobalTauri` is true (default false — see tauri-utils
 * `config.rs`). `window.isTauri` and `window.__TAURI_INTERNALS__` are
 * installed unconditionally by the backend's webview init scripts (tauri
 * `manager/webview.rs`), and `isTauri` is exactly what
 * `@tauri-apps/api/core`'s own `isTauri()` checks
 * (`!!(globalThis || window).isTauri`). The `__TAURI__` disjunct is kept
 * only for forward-compat with configs that do enable the global.
 */
export function isTauriRuntime(): boolean {
  if (typeof window === "undefined") return false;
  const w = window as unknown as Record<string, unknown>;
  return (
    w["isTauri"] === true ||
    "__TAURI_INTERNALS__" in window ||
    "__TAURI__" in window
  );
}

export function createIpc(force?: "tauri" | "mock"): KestrelIpc {
  if (force === "tauri") return new TauriIpc();
  if (force === "mock") return new MockIpc();
  if (singleton) return singleton;
  singleton = isTauriRuntime() ? new TauriIpc() : new MockIpc();
  return singleton;
}
