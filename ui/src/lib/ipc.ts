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
  FileEntryDto,
  JobId,
  OpenPathResult,
  ScanEventDto,
  ScanOptionsDto,
} from "./types";

export type ScanEventHandler = (event: ScanEventDto) => void;

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

  async scanStart(
    path: string,
    _options: ScanOptionsDto,
    onEvent: ScanEventHandler
  ): Promise<JobId> {
    const id = this.nextId++;
    // Emit asynchronously in chunks so the streaming path (Entry…BatchEnd…
    // Complete) is exercised exactly like the real backend. Each timer
    // callback stays cheap: at most one small batch per macrotask.
    const names = [".hidden-note", "README.md", "src", "docs", "assets"];
    let i = 0;
    const step = () => {
      if (this.cancelled.has(id)) return;
      const BATCH = 200;
      for (let k = 0; k < BATCH && i < names.length + 40; k++, i++) {
        const name = i < names.length ? names[i] : "file-" + i + ".txt";
        onEvent({ type: "Entry", entry: mockEntry(path, name, i) });
      }
      // One synthetic permission error proves error rows render.
      if (i >= 10 && i - BATCH < 10) {
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
      if (i < names.length + 40) {
        setTimeout(step, 0);
      } else {
        onEvent({ type: "Complete", total: i });
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
