/* Kestrel Phase 2 — browse controller.
 *
 * Stream handling (the accumulation contract, unchanged from Phase 1):
 * - Channel callback `onStreamEvent` is SYNC and cheap: Entry events are
 *   pushed onto `pending` (a plain array) and we return. No DOM, no await,
 *   no sorting — the backend must never block on us (backpressure note).
 * - BatchEnd flushes `pending` into `all` in one pass and schedules ONE
 *   rAF render. Bursts of thousands of entries therefore cost one render
 *   per batch, not one per entry.
 * - Error events append a visible error Row (never dropped silently).
 * - Complete sets `done=true`, records the total, renders once, updates
 *   the status line. A missing Complete (cancelled nav) simply never flips
 *   `done` — rows already shown stay shown.
 *
 * Phase 2 query routing (deliberate split):
 * - SORT is a backend query parameter (`SortSpecDto` → scan.rs). Toggling
 *   sort re-runs `scan_start` with the new spec; the client never re-sorts
 *   streamed rows. `dirsFirst` is always true and survives every toggle.
 * - FILTER is client-side over the already-streamed entries: one cheap scan
 *   over the flat `all` array per keystroke, reusing cached lowercase names
 *   and the existing view array (no per-row allocation). Never re-scans.
 * - HIDDEN is a re-scan parameter (`ScanOptionsDto.showHidden`).
 *
 * The windower pool is never recreated: sort/filter/hidden change only
 * which rows the window reads (`rows` view + setCount + invalidate), so the
 * DOM node count stays O(window) independent of N.
 *
 * Watch (frozen contract): one subscription per navigation. `navigate`
 * drops the old subscription best-effort (never awaited, never allowed to
 * fail navigation) and subscribes after the new scan starts; a
 * navigation-sequence guard means a slow subscribe racing a fast second
 * navigation unsubscribes its own orphan instead of leaking it. A `changed`
 * event whose dirs include the viewed directory re-runs the existing scan
 * path (`rescan`, which keeps the watch) — no incremental patching.
 */

import type { KestrelIpc } from "./ipc";
import type {
  CmdError,
  FileEntryDto,
  JobId,
  Row,
  ScanEventDto,
  ScanOptionsDto,
  SortKeyDto,
  SortSpecDto,
  WatchEventDto,
} from "./types";
import { defaultSort } from "./types";
import { formatDate, formatSize, kindGlyph } from "./format";
import { ROW_HEIGHT, Windower, type PooledRow } from "./windower";

export interface BrowserViews {
  crumbsEl: HTMLElement;
  upBtn: HTMLButtonElement;
  viewport: HTMLElement;
  statusEl: HTMLElement;
  stateEl: HTMLElement; // loading / empty overlay
  perfEl: HTMLElement | null;
}

export interface Crumb {
  label: string;
  path: string;
}

function noop(): void {
  /* best-effort promise sink */
}

export class Browser {
  /** The windower-facing VIEW (filtered). `all` holds everything streamed. */
  rows: Row[] = [];
  private all: Row[] = [];
  /** Lowercase names parallel to `all` ("" for error rows): computed once
   * per streamed entry so filtering never allocates per keystroke. */
  private keys: string[] = [];
  private pending: FileEntryDto[] = [];
  private jobId: JobId | null = null;
  private navSeq = 0;
  /** The navSeq whose data `all`/`rows` currently hold. A re-scan keeps the
   * old rows on screen until the new scan's first event arrives (no blank
   * flash on sort toggles / watch rescans); `ensureFresh` swaps then. */
  private freshSeq = 0;
  private done = false;
  private total = -1;
  private errorCount = 0;
  currentPath = "";
  private selected = -1;
  private renderQueued = false;

  /* ---- Phase 2 query state ---- */
  private sort: SortSpecDto = defaultSort();
  private showHidden = true; // matches the backend default (show_hidden=true)
  private filter = "";
  private filterLower = "";
  private fixtureMode = false;

  /* ---- Phase 2 watch state ---- */
  private watchSeq = 0;
  private watchId: JobId | null = null;
  private watchNote = "watch: none";

  /* ---- Perf observations for the HUD ---- */
  lastFilterMs = -1;
  lastSortMs = -1;
  private pendingSortT0 = 0;

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
    // One delegated listener for breadcrumb clicks (not one per segment).
    this.views.crumbsEl.addEventListener("click", (e) => this.onCrumbClick(e));
  }

  /* ---------------- navigation ---------------- */

  async navigate(path: string): Promise<void> {
    // New navigation epoch for BOTH the scan and the watch: a slow
    // watch_subscribe racing a fast second navigate() must orphan itself
    // (unsubscribe its own id) rather than leak or clobber the new watch.
    this.watchSeq++;
    const wseq = this.watchSeq;
    this.dropWatch();
    this.fixtureMode = false;
    this.currentPath = path;
    await this.startScan();
    // Subscribe after the listing is kicked off: watch liveness must never
    // delay first paint. Best effort — if the backend lane has not landed
    // the command yet, the listing still works and the status line says so.
    let wid: JobId | null = null;
    try {
      wid = await this.ipc.watchSubscribe(path, (ev) =>
        this.onWatchEvent(wseq, ev)
      );
    } catch {
      wid = null;
    }
    if (wid === null) {
      if (wseq === this.watchSeq) this.watchNote = "watch: unavailable";
    } else if (wseq === this.watchSeq && path === this.currentPath) {
      this.watchId = wid;
      this.watchNote = "watch: sub #" + wid + " on " + path;
    } else {
      // Superseded while subscribing: immediately release the orphan.
      this.ipc.watchUnsubscribe(wid).then(noop, noop);
    }
    this.paintChrome();
  }

  /** Fire-and-forget watch release: never awaited, never throws. */
  private dropWatch(): void {
    if (this.watchId !== null) {
      const old = this.watchId;
      this.watchId = null;
      this.ipc.watchUnsubscribe(old).then(noop, noop);
    }
    this.watchNote = "watch: none";
  }

  /** Cancel the in-flight scan and re-run `scan_start` on the current path
   * with the current sort/hidden options. Used by navigate() (after the
   * watch has been swapped) and by watch-triggered rescan (which keeps the
   * watch — resubscribing on every fs event would churn subscriptions). */
  private async startScan(): Promise<void> {
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
    this.pending = [];
    this.done = false;
    this.total = -1;
    this.errorCount = 0;
    // Deferred reset: the previous listing stays on screen (dimmed by the
    // loading overlay) until the new scan's first event swaps the arrays in
    // `ensureFresh`. Sort toggles and watch rescans therefore never flash
    // blank, and scrolling keeps working mid-rescan.
    this.paintChrome();
    this.showState("loading");

    // NOTE: Channel events may fire before scan_start resolves, so the
    // handler keys off the per-navigation sequence, not the JobId.
    const id = await this.ipc.scanStart(
      this.currentPath,
      this.scanOptions(),
      (ev) => this.onStreamEvent(seq, ev)
    );
    if (seq === this.navSeq) this.jobId = id;
  }

  private scanOptions(): ScanOptionsDto {
    return {
      recursive: false,
      showHidden: this.showHidden,
      sort: {
        key: this.sort.key,
        ascending: this.sort.ascending,
        dirsFirst: true,
      },
    };
  }

  private onStreamEvent(seq: number, ev: ScanEventDto): void {
    if (seq !== this.navSeq) return; // stale scan
    // First event of this navigation swaps in fresh arrays (see startScan).
    // One integer compare per event — the backpressure path stays cheap.
    this.ensureFresh(seq);
    // Cheap + sync from here on: buffer, never render except via BatchEnd.
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
      const row: Row = { kind: "error", error: ev.error };
      this.all.push(row);
      this.keys.push("");
      this.rows.push(row); // error rows always pass the filter
      this.errorCount++;
      this.queueRender();
    } else if (ev.type === "Complete") {
      this.flushPending();
      this.done = true;
      this.total = ev.total;
      this.queueRender();
      this.paintChrome();
      // Sort-to-paint: from toggle to the presented frame after Complete.
      if (this.pendingSortT0 > 0) {
        const t0 = this.pendingSortT0;
        this.pendingSortT0 = 0;
        const self = this;
        requestAnimationFrame(function () {
          requestAnimationFrame(function () {
            self.lastSortMs = performance.now() - t0;
            self.paintChrome();
          });
        });
      }
    }
  }

  /** Swap in empty arrays for `seq` on its first stream event. Called on
   * every event (one compare); the actual reset runs once per navigation. */
  private ensureFresh(seq: number): void {
    if (this.freshSeq === seq) return;
    this.freshSeq = seq;
    this.all = [];
    this.keys = [];
    this.rows = [];
    this.pending = [];
    this.selected = 0;
    this.windower.selectedIndex = -1;
    this.windower.setCount(0);
  }

  private flushPending(): void {
    if (this.pending.length === 0) {
      this.queueRender();
      return;
    }
    // One pass per batch. Per-entry work (one Row ref + one lowercase name)
    // happens once per streamed entry, never per keystroke.
    const q = this.filterLower;
    const hideHidden = !this.showHidden;
    for (let i = 0; i < this.pending.length; i++) {
      const e = this.pending[i];
      if (hideHidden && e.hidden) continue; // mock path may not filter
      const key = e.name.toLowerCase();
      const row: Row = { kind: "entry", entry: e };
      this.all.push(row);
      this.keys.push(key);
      if (q === "" || key.indexOf(q) >= 0) this.rows.push(row);
    }
    this.pending.length = 0;
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

  /* ---------------- Phase 2: sort / filter / hidden ---------------- */

  currentSort(): SortSpecDto {
    return {
      key: this.sort.key,
      ascending: this.sort.ascending,
      dirsFirst: true,
    };
  }

  hiddenShown(): boolean {
    return this.showHidden;
  }

  currentFilter(): string {
    return this.filter;
  }

  /** Sort is a BACKEND query parameter: re-run the scan with the new spec.
   * `dirsFirst` is forced true on every toggle. Returns when the re-scan
   * has been kicked off (paint completes later; see lastSortMs). */
  setSortKey(key: SortKeyDto): Promise<void> {
    if (key === this.sort.key) return Promise.resolve();
    this.sort.key = key;
    return this.applySort();
  }

  toggleSortDir(): Promise<void> {
    this.sort.ascending = !this.sort.ascending;
    return this.applySort();
  }

  private applySort(): Promise<void> {
    if (this.fixtureMode) {
      // No backend in fixture mode: same ordering semantics, client-side,
      // so sort-to-paint stays measurable on `?rows=N`.
      const t0 = performance.now();
      this.sortAllClient();
      this.rebuildView();
      this.clampSelection();
      this.windower.invalidate();
      this.windower.setCount(this.rows.length);
      this.windower.setSelected(this.selected);
      this.paintChrome();
      this.updateStateOverlay();
      this.lastSortMs = performance.now() - t0;
      this.paintChrome();
      return Promise.resolve();
    }
    this.pendingSortT0 = performance.now();
    return this.startScan();
  }

  /**
   * Filter-as-you-type: CLIENT-SIDE over the already-streamed entries.
   * Never re-invokes scan_start. One cheap scan over the flat `all` array
   * using cached lowercase names; the view array's backing store is reused
   * (length reset, refs pushed) so there is no per-row allocation per
   * keystroke. The windower pool is untouched — setCount + invalidate
   * repaints only the visible window. Returns keystroke-to-paint ms
   * (synchronous paint via setCount's update).
   */
  setFilter(q: string): number {
    const t0 = performance.now();
    const lower = q.toLowerCase();
    if (lower === this.filterLower) return 0;
    this.filter = q;
    this.filterLower = lower;
    this.rebuildView();
    this.clampSelection();
    // Invalidate BEFORE setCount: stamps still hold pre-filter indices and
    // would otherwise skip repainting slots that now show different rows.
    this.windower.invalidate();
    this.windower.setCount(this.rows.length);
    this.windower.setSelected(this.selected);
    this.paintChrome();
    this.updateStateOverlay();
    this.lastFilterMs = performance.now() - t0;
    this.paintChrome();
    return this.lastFilterMs;
  }

  /** Hidden maps to `ScanOptionsDto.showHidden`: a re-scan parameter. */
  setShowHidden(v: boolean): Promise<void> {
    if (v === this.showHidden) return Promise.resolve();
    this.showHidden = v;
    if (this.fixtureMode) {
      this.rebuildView();
      this.clampSelection();
      this.windower.invalidate();
      this.windower.setCount(this.rows.length);
      this.windower.setSelected(this.selected);
      this.paintChrome();
      this.updateStateOverlay();
      return Promise.resolve();
    }
    return this.startScan();
  }

  private rebuildView(): void {
    const q = this.filterLower;
    const hideHidden = !this.showHidden;
    this.rows.length = 0; // reuse backing store across keystrokes
    for (let i = 0; i < this.all.length; i++) {
      const r = this.all[i];
      if (r.kind === "error") {
        this.rows.push(r);
        continue;
      }
      if (hideHidden && r.entry.hidden) continue;
      if (q !== "" && this.keys[i].indexOf(q) < 0) continue;
      this.rows.push(r);
    }
  }

  private clampSelection(): void {
    if (this.selected >= this.rows.length)
      this.selected = Math.max(0, this.rows.length - 1);
    if (this.rows.length === 0) this.selected = -1;
    else if (this.selected < 0) this.selected = 0;
  }

  /** Client-side ordering for fixture mode only (mirrors scan.rs: dirs
   * first, column, nulls last, ascending flag on the column only). */
  private sortAllClient(): void {
    const sort = this.sort;
    const order: number[] = new Array(this.all.length);
    for (let i = 0; i < order.length; i++) order[i] = i;
    const all = this.all;
    const keys = this.keys;
    order.sort(function (ia, ib) {
      const a = all[ia];
      const b = all[ib];
      if (a.kind === "error") return b.kind === "error" ? 0 : 1;
      if (b.kind === "error") return -1;
      const ae = (a as { kind: "entry"; entry: FileEntryDto }).entry;
      const be = (b as { kind: "entry"; entry: FileEntryDto }).entry;
      if (sort.dirsFirst) {
        const ad = ae.descendable ? 1 : 0;
        const bd = be.descendable ? 1 : 0;
        if (ad !== bd) return bd - ad;
      }
      let c = 0;
      if (sort.key === "name") c = cmpStr(ae.name, be.name);
      else if (sort.key === "size") c = cmpNullNum(ae.size, be.size);
      else if (sort.key === "modified") c = cmpNullNum(ae.modified, be.modified);
      else c = cmpStr(ae.kind, be.kind);
      return sort.ascending ? c : -c;
    });
    const reAll: Row[] = new Array(all.length);
    const reKeys: string[] = new Array(keys.length);
    for (let i = 0; i < order.length; i++) {
      reAll[i] = all[order[i]];
      reKeys[i] = keys[order[i]];
    }
    this.all = reAll;
    this.keys = reKeys;
  }

  /* ---------------- watch events ---------------- */

  private onWatchEvent(wseq: number, ev: WatchEventDto): void {
    if (wseq !== this.watchSeq) return; // dead watcher from an older nav
    if (ev.type === "error") {
      this.watchNote = "watch: error " + ev.error.message;
      this.setStatus("Watch error: " + ev.error.message);
      return;
    }
    let hit = false;
    for (let i = 0; i < ev.dirs.length; i++) {
      if (sameDir(ev.dirs[i], this.currentPath)) {
        hit = true;
        break;
      }
    }
    this.watchNote =
      "watch: changed [" + ev.dirs.join(", ") + "]" + (hit ? " → rescan" : " (ignored)");
    this.paintChrome();
    if (!hit) return;
    if (this.fixtureMode) return; // no backend to re-scan in fixture mode
    // Re-run the existing scan path so both UIs share one code path and
    // cannot drift. Keeps the watch subscription (no churn per event).
    void this.startScan();
  }

  watchStatus(): string {
    return this.watchNote;
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

  private onCrumbClick(e: MouseEvent): void {
    let el = e.target as HTMLElement | null;
    while (el && el !== this.views.crumbsEl) {
      const path = el.dataset ? el.dataset["path"] : undefined;
      if (path !== undefined) {
        void this.navigate(path);
        return;
      }
      el = el.parentElement;
    }
  }

  private onKey(e: KeyboardEvent): void {
    // Phase 3a hook: op shortcuts (copy/move/trash/delete) live in main.ts.
    // Returning true consumes the key before list navigation sees it.
    if (this.opKeyHandler) {
      try {
        if (this.opKeyHandler(e)) return;
      } catch {
        /* a broken hook must never break arrow/enter navigation */
      }
    }
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

  /* ---------------- Phase 3a: ops surface ---------------- */

  /** Op shortcut hook (wired in main.ts). Return true to consume the key. */
  opKeyHandler: ((e: KeyboardEvent) => boolean) | null = null;

  /** The selected entry, or null when nothing (or an error row) is selected.
   * Single-item exit gate: there is no selection set this phase. */
  selectedEntry(): FileEntryDto | null {
    const row = this.rows[this.selected];
    if (!row || row.kind !== "entry") return null;
    return row.entry;
  }

  selectedIndex(): number {
    return this.selected;
  }

  /** Re-run the listing on the current path (op settled → contents changed).
   * Keeps the watch subscription — same path as watch-triggered rescans. */
  rescan(): void {
    if (this.fixtureMode) return;
    void this.startScan();
  }

  /**
   * Op failure sink. Reuses the scan error path exactly: the error becomes
   * a visible error row (never dropped silently) plus a status line — no
   * second error channel. `AlreadyExists` collisions land here as errors.
   */
  showOpError(error: CmdError): void {
    const row: Row = { kind: "error", error };
    this.all.push(row);
    this.keys.push("");
    this.rows.push(row); // error rows always pass the filter
    this.errorCount++;
    this.setStatus("Operation failed: " + error.message);
    this.queueRender();
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
    slot.root.setAttribute("aria-selected", selected ? "true" : "false");
    if (row.kind === "error") {
      const where = row.error.path ? row.error.path + ": " : "";
      slot.glyph.textContent = "!";
      slot.name.textContent = where + row.error.message;
      slot.meta.textContent = row.error.kind;
    } else {
      const e = row.entry;
      slot.glyph.textContent = kindGlyph(e.kind, e.descendable);
      slot.name.textContent = e.name;
      slot.meta.textContent = formatSize(e.size) + "   " + formatDate(e.modified);
    }
  }

  private paintRowSelection(): void {
    // Selection-only change: invalidate stamps so the window repaints once.
    this.windower.invalidate();
  }

  private paintChrome(): void {
    this.paintCrumbs();
    const n = this.rows.length;
    const total = this.all.length;
    let s: string;
    if (!this.done) s = "Loading… " + n + " items";
    else if (n === 0 && total === 0) s = "Empty";
    else if (this.filterLower !== "" || !this.showHidden)
      s = n + " of " + total + " items";
    else s = n + " items";
    if (this.errorCount) s += " · " + this.errorCount + " errors";
    if (this.done && this.total >= 0 && this.total !== total)
      s += " (total " + this.total + ")";
    if (this.lastFilterMs >= 0)
      s += " · filter " + this.lastFilterMs.toFixed(1) + " ms";
    if (this.lastSortMs >= 0) s += " · sort " + this.lastSortMs.toFixed(1) + " ms";
    s += " · " + this.watchNote;
    this.views.statusEl.textContent = s;
    this.updateStateOverlay();
  }

  /**
   * Breadcrumb. Root decision: the egui app renders a bare "/" segment that
   * reads like a stray character, so the root crumb is labelled "Root" (full
   * path "/" kept in the title tooltip). Drive roots ("C:") and relative
   * paths keep their own legible first segment.
   */
  private paintCrumbs(): void {
    const bar = this.views.crumbsEl;
    while (bar.firstChild) bar.removeChild(bar.firstChild);
    const crumbs = crumbsFor(this.currentPath || "/");
    for (let i = 0; i < crumbs.length; i++) {
      if (i > 0) {
        const sep = document.createElement("span");
        sep.className = "k-sep";
        sep.textContent = "/";
        bar.appendChild(sep);
      }
      const c = crumbs[i];
      const last = i === crumbs.length - 1;
      const b = document.createElement("button");
      b.type = "button";
      b.className = "k-crumb" + (last ? " is-current" : "");
      b.textContent = c.label;
      b.title = c.path;
      if (!last && c.path) b.dataset["path"] = c.path;
      if (last) b.setAttribute("aria-current", "page");
      bar.appendChild(b);
    }
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
    this.watchSeq++; // invalidate any in-flight watch subscribe
    this.dropWatch();
    this.fixtureMode = true;
    this.freshSeq = this.navSeq;
    this.jobId = null;
    this.currentPath = label;
    this.all = rows.slice();
    this.keys = new Array(rows.length);
    for (let i = 0; i < rows.length; i++) {
      const r = rows[i];
      this.keys[i] = r.kind === "error" ? "" : r.entry.name.toLowerCase();
    }
    this.sortAllClient();
    this.rebuildView();
    this.done = true;
    this.total = rows.length;
    this.errorCount = 0;
    this.selected = 0;
    this.windower.invalidate();
    this.windower.setCount(this.rows.length);
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

/** Split an absolute path into clickable segments. See paintCrumbs for the
 * root labelling decision. Pure function, unit-testable. */
export function crumbsFor(path: string): Crumb[] {
  let s = path;
  while (s.length > 1 && s.charAt(s.length - 1) === "/") s = s.slice(0, -1);
  // Windows drive root or drive-absolute, e.g. "C:", "C:/", "C:\x\y".
  const drive = s.match(/^([A-Za-z]:)([\/\\]|$)/);
  if (drive) {
    const root = drive[1];
    const rest = s.slice(root.length).replace(/\\/g, "/");
    const parts = rest.split("/").filter((p) => p !== "");
    const crumbs: Crumb[] = [{ label: root, path: root + "/" }];
    let acc = root;
    for (let i = 0; i < parts.length; i++) {
      acc += "/" + parts[i];
      crumbs.push({ label: parts[i], path: acc });
    }
    return crumbs;
  }
  if (s.charAt(0) === "/") {
    const parts = s.split("/").filter((p) => p !== "");
    const crumbs: Crumb[] = [{ label: "Root", path: "/" }];
    let acc = "";
    for (let i = 0; i < parts.length; i++) {
      acc += "/" + parts[i];
      crumbs.push({ label: parts[i], path: acc });
    }
    return crumbs;
  }
  // Relative path: no root segment, each part accumulates.
  const parts = s.split("/").filter((p) => p !== "");
  if (parts.length === 0) return [{ label: "Root", path: "/" }];
  const crumbs: Crumb[] = [];
  let acc = "";
  for (let i = 0; i < parts.length; i++) {
    acc = acc === "" ? parts[i] : acc + "/" + parts[i];
    crumbs.push({ label: parts[i], path: acc });
  }
  return crumbs;
}

function sameDir(a: string, b: string): boolean {
  return stripSlash(a) === stripSlash(b);
}

function stripSlash(p: string): string {
  let s = p;
  while (s.length > 1 && s.charAt(s.length - 1) === "/") s = s.slice(0, -1);
  return s;
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
  const an = a === null || a === undefined;
  const bn = b === null || b === undefined;
  if (an) return bn ? 0 : 1;
  if (bn) return -1;
  return (a as number) - (b as number);
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
