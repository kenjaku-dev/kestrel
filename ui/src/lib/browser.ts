/* Kestrel Phase 1 — browse controller.
 *
 * Stream handling (the accumulation contract):
 * - Channel callback `onStreamEvent` is SYNC and cheap: Entry events are
 *   pushed onto `pending` (a plain array) and we return. No DOM, no await,
 *   no sorting — the backend must never block on us (backpressure note).
 * - BatchEnd flushes `pending` into `rows` in one splice and schedules ONE
 *   rAF render. Bursts of thousands of entries therefore cost one render
 *   per batch, not one per entry.
 * - Error events append a visible error Row (never dropped silently).
 * - Complete sets `done=true`, records the total, renders once, updates
 *   the status line. A missing Complete (cancelled nav) simply never flips
 *   `done` — rows already shown stay shown.
 *
 * Interaction: single delegated click/dblclick on the row content plus
 * keyboard on the viewport. Zero per-row listeners. Selection is one
 * integer; rendering reads it during the window pass.
 */

import type { KestrelIpc } from "./ipc";
import type { JobId, Row, ScanEventDto } from "./types";
import { formatDate, formatSize, kindGlyph } from "./format";
import { ROW_HEIGHT, Windower, type PooledRow } from "./windower";

export interface BrowserViews {
  pathEl: HTMLElement;
  upBtn: HTMLButtonElement;
  viewport: HTMLElement;
  statusEl: HTMLElement;
  stateEl: HTMLElement; // loading / empty overlay
  perfEl: HTMLElement | null;
}

export class Browser {
  rows: Row[] = [];
  private pending: import("./types").FileEntryDto[] = [];
  private jobId: JobId | null = null;
  private navSeq = 0;
  private done = false;
  private total = -1;
  private errorCount = 0;
  currentPath = "";
  private selected = -1;
  private renderQueued = false;

  constructor(
    private ipc: KestrelIpc,
    private views: BrowserViews,
    private windower: Windower
  ) {
    this.windower.setRenderRow((slot, index, selected) =>
      this.paintRow(slot, index, selected)
    );
    // Delegated pointer handling — the only click listeners in the list.
    this.views.viewport.addEventListener("click", (e) => this.onClick(e));
    this.views.viewport.addEventListener("dblclick", (e) => this.onDblClick(e));
    this.views.viewport.addEventListener("keydown", (e) => this.onKey(e));
    this.views.upBtn.addEventListener("click", () => this.up());
  }

  /* ---------------- navigation ---------------- */

  async navigate(path: string): Promise<void> {
    // Cancel the in-flight scan so a late BatchEnd can't paint into the
    // new directory. Stale events are also guarded by navigation seq.
    if (this.jobId !== null) {
      const old = this.jobId;
      this.jobId = null;
      try {
        await this.ipc.scanCancel(old);
      } catch {
        /* best effort; a failed cancel just means the stream ends soon */
      }
    }
    this.navSeq++;
    const seq = this.navSeq;
    this.currentPath = path;
    this.rows = [];
    this.pending = [];
    this.done = false;
    this.total = -1;
    this.errorCount = 0;
    this.selected = 0;
    this.windower.selectedIndex = -1;
    this.windower.setCount(0);
    this.paintChrome();
    this.showState("loading");

    // NOTE: Channel events may fire before scan_start resolves, so the
    // handler keys off the per-navigation sequence, not the JobId.
    const id = await this.ipc.scanStart(
      path,
      { recursive: false },
      (ev) => this.onStreamEvent(seq, ev)
    );
    if (seq === this.navSeq) this.jobId = id;
  }

  private onStreamEvent(seq: number, ev: ScanEventDto): void {
    if (seq !== this.navSeq) return; // stale scan
    // Cheap + sync: buffer, never render here except via the BatchEnd path.
    if (ev.type === "Entry") {
      this.pending.push(ev.entry);
    } else if (ev.type === "Entries") {
      // One coalesced batch straight off the wire (the pump fuses each
      // burst; BatchEnd itself is never forwarded). A batch *is* the
      // boundary, so flush exactly like the BatchEnd path: one render.
      for (let i = 0; i < ev.entries.length; i++)
        this.pending.push(ev.entries[i]);
      this.flushPending();
    } else if (ev.type === "BatchEnd") {
      this.flushPending();
    } else if (ev.type === "Error") {
      this.rows.push({ kind: "error", error: ev.error });
      this.errorCount++;
      this.queueRender();
    } else if (ev.type === "Complete") {
      this.flushPending();
      this.done = true;
      this.total = ev.total;
      this.queueRender();
      this.paintChrome();
    }
  }

  private flushPending(): void {
    if (this.pending.length === 0) {
      this.queueRender();
      return;
    }
    // One splice per batch — amortised O(1) per entry.
    const batch: Row[] = new Array(this.pending.length);
    for (let i = 0; i < this.pending.length; i++)
      batch[i] = { kind: "entry", entry: this.pending[i] };
    this.pending.length = 0;
    for (let i = 0; i < batch.length; i++) this.rows.push(batch[i]);
    this.queueRender();
  }

  private queueRender(): void {
    this.windower.setCount(this.rows.length);
    this.windower.setSelected(this.selected);
    this.paintChrome();
    if (this.renderQueued) return;
    this.renderQueued = true;
    requestAnimationFrame(() => {
      this.renderQueued = false;
      this.updateStateOverlay();
    });
  }

  /* ---------------- opening ---------------- */

  /** Enter / double-click behaviour. Directories navigate; files open_path. */
  async activate(index: number): Promise<void> {
    const row = this.rows[index];
    if (!row) return;
    if (row.kind === "error") return; // error rows are informational
    const e = row.entry;
    // `descendable` is derived by the normaliser, but match kind
    // case-insensitively anyway so a wire rename can never strand navigation.
    const kind = (e.kind || "").toLowerCase();
    if (e.descendable || kind === "directory") {
      await this.navigate(e.path);
      return;
    }
    try {
      const res = await this.ipc.openPath(e.path);
      if (res.entered_dir) await this.navigate(e.path);
      else this.setStatus("Opened " + e.name);
    } catch (err) {
      this.setStatus("Open failed: " + messageOf(err));
    }
  }

  up(): void {
    const parent = parentPath(this.currentPath);
    if (parent !== null) void this.navigate(parent);
  }

  /* ---------------- input (delegated) ---------------- */

  private rowIndexFromEvent(e: Event): number {
    let el = e.target as HTMLElement | null;
    while (el && el !== this.views.viewport) {
      const idx = el.dataset ? el.dataset["index"] : undefined;
      if (idx !== undefined) return parseInt(idx, 10);
      el = el.parentElement;
    }
    return -1;
  }

  private onClick(e: MouseEvent): void {
    const i = this.rowIndexFromEvent(e);
    if (i >= 0) this.select(i, false);
    this.views.viewport.focus();
  }

  private onDblClick(e: MouseEvent): void {
    const i = this.rowIndexFromEvent(e);
    if (i >= 0) void this.activate(i);
  }

  private onKey(e: KeyboardEvent): void {
    const n = this.rows.length;
    if (e.key === "ArrowDown") {
      e.preventDefault();
      this.select(Math.min(n - 1, this.selected + 1), true);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      this.select(Math.max(0, this.selected - 1), true);
    } else if (e.key === "Enter") {
      e.preventDefault();
      void this.activate(this.selected);
    } else if (e.key === "Backspace") {
      e.preventDefault();
      this.up();
    } else if (e.key === "Home") {
      e.preventDefault();
      this.select(0, true);
    } else if (e.key === "End") {
      e.preventDefault();
      this.select(Math.max(0, n - 1), true);
    }
  }

  select(i: number, scroll: boolean): void {
    if (i < 0 || i >= this.rows.length) return;
    this.selected = i;
    this.windower.setSelected(i);
    if (scroll) this.windower.scrollToIndex(i);
    this.paintRowSelection();
  }

  /* ---------------- painting ---------------- */

  /** Per-visible-row paint. Called only for rows in the window. No listeners. */
  private paintRow(slot: PooledRow, index: number, selected: boolean): void {
    const row = this.rows[index];
    if (!row) return;
    const cls =
      "k-row" +
      (selected ? " is-selected" : "") +
      (row.kind === "error" ? " is-error" : "");
    if (slot.root.className !== cls) slot.root.className = cls;
    slot.root.setAttribute(
      "aria-selected",
      selected ? "true" : "false"
    );
    if (row.kind === "error") {
      const where = row.error.path ? row.error.path + ": " : "";
      slot.glyph.textContent = "!";
      slot.name.textContent = where + row.error.message;
      slot.meta.textContent = row.error.kind;
    } else {
      const e = row.entry;
      slot.glyph.textContent = kindGlyph(e.kind, e.descendable);
      slot.name.textContent = e.name;
      slot.meta.textContent =
        formatSize(e.size) + "   " + formatDate(e.modified);
    }
  }

  private paintRowSelection(): void {
    // Selection-only change: invalidate stamps so the window repaints once.
    this.windower.invalidate();
  }

  private paintChrome(): void {
    this.views.pathEl.textContent = this.currentPath || "(no path)";
    const n = this.rows.length;
    let s: string;
    if (!this.done) s = "Loading… " + n + " items";
    else if (n === 0) s = "Empty";
    else s = n + " items" + (this.errorCount ? " · " + this.errorCount + " errors" : "");
    if (this.done && this.total >= 0 && this.total !== n)
      s += " (total " + this.total + ")";
    this.views.statusEl.textContent = s;
    this.updateStateOverlay();
  }

  private showState(mode: "loading" | "empty" | "none"): void {
    const el = this.views.stateEl;
    if (mode === "none") {
      el.style.display = "none";
      return;
    }
    el.style.display = "";
    el.textContent = mode === "loading" ? "Loading…" : "Empty directory";
  }

  private updateStateOverlay(): void {
    if (!this.done) this.showState("loading");
    else if (this.rows.length === 0) this.showState("empty");
    else this.showState("none");
  }

  private setStatus(s: string): void {
    this.views.statusEl.textContent = s;
  }

  /* ---------------- fixture hook ---------------- */

  /** Dev-only: bypass the backend and show N synthetic rows. */
  showFixture(rows: Row[], label: string): void {
    this.navSeq++; // invalidate any in-flight real scan
    this.jobId = null;
    this.currentPath = label;
    this.rows = rows;
    this.done = true;
    this.total = rows.length;
    this.errorCount = 0;
    this.selected = 0;
    this.windower.setCount(rows.length);
    this.windower.setSelected(0);
    this.paintChrome();
  }

  get doneScanning(): boolean {
    return this.done;
  }
}

export function parentPath(p: string): string | null {
  // Normalise trailing slashes (but keep root "/").
  let s = p;
  while (s.length > 1 && s.charAt(s.length - 1) === "/") s = s.slice(0, -1);
  if (s === "/" || s === "") return null;
  // Windows drive root, e.g. "C:" or "C:/".
  if (/^[A-Za-z]:$/.test(s) || /^[A-Za-z]:\/$/.test(s)) return null;
  const i = Math.max(s.lastIndexOf("/"), s.lastIndexOf("\\"));
  if (i <= 0) return "/";
  return s.slice(0, i);
}

function messageOf(err: unknown): string {
  if (err instanceof Error) return err.message;
  try {
    return JSON.stringify(err);
  } catch {
    return String(err);
  }
}

export { ROW_HEIGHT };
