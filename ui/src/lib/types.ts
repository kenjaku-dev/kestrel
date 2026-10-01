/* Kestrel Phase 1 — DTO and row types.
 *
 * Mirrors the frozen IPC contract from MIGRATION.md §3. Tauri commands are
 * invoked by name; arguments and return values are JSON. If the Rust side
 * ever renames a field, only `ipc.ts` (normalisation) and this file change.
 *
 * Wire notes (serde, internally-tagged enum — see `ipc.ts normalizeScanEvent`):
 * - FileEntryDto.modified is EPOCH MILLIS, not seconds. Rendering it with
 *   `new Date(modified)` is correct; multiplying/dividing by 1000 is the
 *   classic bug (dates render as 1970 or 50000 AD).
 * - ScanEventDto arrives over a Tauri Channel as serde_json with an
 *   INTERNALLY tagged shape: `{"type":"entries","entries":[...]}`,
 *   `{"type":"error","path":…,"kind":…,"message":…}` (flat),
 *   `{"type":"complete","total":n}`. `kind` is lowercase
 *   (`"directory"|"file"|"symlink"|"other"`) and the symlink-target flag is
 *   camelCase `isDirTarget`; there is no `descendable` on the wire.
 * - `ipc.ts` normalises every accepted shape into the union below: kinds
 *   are canonicalised to `"Directory"|"File"|"Symlink"|"Other"`,
 *   `isDirTarget` → `is_dir_target`, and `descendable` is derived the way
 *   the engine does (real dir, or symlink whose target is a dir).
 */

export type EntryKind = "Directory" | "File" | "Symlink" | "Other";

export interface FileEntryDto {
  name: string;
  path: string;
  kind: string; // "Directory" | "File" | "Symlink" | "Other" (string, not enum: tolerant of future kinds)
  size: number | null; // bytes, null for directories
  modified: number | null; // EPOCH MILLIS, null when unknown
  hidden: boolean;
  is_dir_target: boolean | null;
  descendable: boolean;
}

export interface CmdError {
  kind: string;
  path: string | null;
  message: string;
}

/** Normalised stream events. See ipc.ts normalizeScanEvent for wire shapes.
 * `Entries` is a coalesced batch straight off the wire (the pump fuses each
 * burst at `BatchEnd` boundaries, and `BatchEnd` itself is never forwarded);
 * the other three variants are shared with the mock/fixture path. */
export type ScanEventDto =
  | { type: "Entry"; entry: FileEntryDto }
  | { type: "Entries"; entries: FileEntryDto[] }
  | { type: "BatchEnd" }
  | { type: "Error"; error: CmdError }
  | { type: "Complete"; total: number };

export interface ScanOptionsDto {
  recursive: boolean;
  /** Emit dotfiles. Backend default `true`; the UI default matches. */
  showHidden?: boolean;
  /** Sort order, or `null`/`undefined` for readdir order. Backend default:
   *  engine default (name, ascending, dirs first). */
  sort?: SortSpecDto | null;
}

/** Sort column as the backend expects it (lowercase wire form). */
export type SortKeyDto = "name" | "size" | "modified" | "kind";

/** Full sort spec. Mirrors `SortSpecDto` (dto.rs): camelCase `dirsFirst`. */
export interface SortSpecDto {
  key: SortKeyDto;
  ascending: boolean;
  dirsFirst: boolean;
}

/** Default sort: name, ascending, directories first. Survives every toggle. */
export function defaultSort(): SortSpecDto {
  return { key: "name", ascending: true, dirsFirst: true };
}

/** Watch event over the `watch_subscribe` channel (frozen contract,
 * internally tagged with "type", like ScanEventDto). */
export type WatchEventDto =
  | { type: "changed"; dirs: string[] }
  | { type: "error"; error: CmdError };

export type JobId = number; // u64 on the Rust side; always backend-minted

/** One rendered row: either a listing entry or a visible backend error. */
export type Row =
  | { kind: "entry"; entry: FileEntryDto }
  | { kind: "error"; error: CmdError };

export interface OpenPathResult {
  entered_dir: boolean;
}

/* Kestrel Phase 3a — file operations (frozen contract, internally tagged
 * with "op" on the request and "type" on events, matching the scan/watch
 * normalisers in ipc.ts).
 *
 * There is deliberately no `collision` event in 3a: a collision arrives as
 * an `error` event with kind `already_exists` and the op fails, leaving the
 * destination untouched. The collision dialog is a later phase.
 */
export type OpRequestDto =
  | { op: "copy"; src: string; dst: string }
  | { op: "move"; src: string; dst: string }
  | { op: "trash"; src: string }
  | { op: "delete"; src: string; recursive: boolean };

export type OpPhaseDto = "measuring" | "copying" | "deleting";

/** Normalised op stream events. See ipc.ts normalizeOpEvent for wire shapes.
 * During `measuring` there is no total yet: the backend may send
 * totalBytes 0 and the UI must render an indeterminate state, never a
 * guessed percentage. */
export type OpEventDto =
  | {
      type: "progress";
      id: number;
      phase: OpPhaseDto;
      doneBytes: number;
      totalBytes: number;
      doneItems: number;
      totalItems: number;
    }
  | { type: "done"; id: number; summary: string }
  | { type: "error"; id: number; error: CmdError };
