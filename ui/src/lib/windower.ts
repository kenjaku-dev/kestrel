/* Kestrel Phase 1 — hand-rolled fixed-height windower.
 *
 * Approach (MIGRATION.md §6: fixed-height rows, overscan ~10, no TanStack):
 * - The scroll container holds one spacer div sized count × rowHeight, so
 *   the scrollbar reflects the full list while only a slice of rows exists.
 * - A pool of absolutely-positioned row divs (visible + 2×overscan, ~30 for
 *   a 600px viewport at 32px rows) is created ONCE and reused. Rendering a
 *   new window only rewrites textContent / transform on existing nodes —
 *   no createElement, no innerHTML, no per-row listeners in the steady
 *   scroll path.
 * - ONE delegated scroll listener (passive) + ONE rAF per frame at most.
 *   Reads (scrollTop, clientHeight) happen once per frame, then all writes.
 *   No layout thrash: we never interleave read/write per row.
 * - No allocation per frame: pool nodes and their cached span refs persist;
 *   the update loop indexes into the row array directly (no slice/map).
 * - Rows unchanged since the last frame are skipped via a per-node index
 *   stamp (dataset), so a frame with no scroll movement costs ~nothing.
 * - Deliberately NOT content-visibility: that leaves thousands of DOM nodes
 *   alive. Here the DOM node count is O(window), independent of N.
 *
 * Safari13 notes: transform translateY (not `translate` property), absolute
 * positioning, flexbox without `gap` (flex gap needs Safari 14.1+).
 */

export const ROW_HEIGHT = 32;
export const OVERSCAN = 10;

export interface PooledRow {
  root: HTMLElement;
  glyph: HTMLElement;
  name: HTMLElement;
  meta: HTMLElement;
  lastIndex: number; // stamped rendered index, -1 = empty, -2 = hidden
  lastSelected: boolean;
}

export type RenderRowFn = (
  pool: PooledRow,
  index: number,
  selected: boolean
) => void;

export class Windower {
  private pool: PooledRow[] = [];
  private raf = 0;
  private count = 0;
  private generation = 0; // bumped on setRows: forces full re-render
  /** Recent per-frame update costs in ms (ring, capped) for the perf HUD. */
  frameCosts: number[] = [];
  /** Scroll events coalesced counter (diagnostic for HUD). */
  frames = 0;

  constructor(
    private viewport: HTMLElement,
    private spacer: HTMLElement,
    private content: HTMLElement,
    private rowHeight: number = ROW_HEIGHT,
    private overscan: number = OVERSCAN,
    private renderRow: RenderRowFn = () => {}
  ) {
    // One listener total. Passive so scrolling never blocks on JS.
    this.viewport.addEventListener("scroll", () => this.schedule(), {
      passive: true,
    });
  }

  setRenderRow(fn: RenderRowFn): void {
    this.renderRow = fn;
    this.generation++;
    this.schedule();
  }

  setCount(n: number): void {
    this.count = n;
    // Single write; height change does not force sync layout here because
    // we never read layout in the same task after writing.
    this.spacer.style.height = n * this.rowHeight + "px";
    this.generation++;
    // Render synchronously on count change so first paint is measurable
    // (loading → list transition isn't delayed a frame for no reason).
    this.update();
  }

  getCount(): number {
    return this.count;
  }

  /** Ask for a re-render on next frame (cheap, coalesced). */
  schedule(): void {
    if (this.raf) return;
    this.raf = requestAnimationFrame(() => {
      this.raf = 0;
      this.update();
    });
  }

  /** Ensure the selected index is visible (arrow-key navigation). */
  scrollToIndex(index: number): void {
    const top = index * this.rowHeight;
    const vh = this.viewport.clientHeight;
    const st = this.viewport.scrollTop;
    if (top < st) this.viewport.scrollTop = top;
    else if (top + this.rowHeight > st + vh)
      this.viewport.scrollTop = top + this.rowHeight - vh;
  }

  private makeRow(): PooledRow {
    const root = document.createElement("div");
    root.className = "k-row";
    root.setAttribute("role", "option");
    const glyph = document.createElement("span");
    glyph.className = "k-glyph";
    const name = document.createElement("span");
    name.className = "k-name";
    const meta = document.createElement("span");
    meta.className = "k-meta";
    root.appendChild(glyph);
    root.appendChild(name);
    root.appendChild(meta);
    this.content.appendChild(root);
    return { root, glyph, name, meta, lastIndex: -2, lastSelected: false };
  }

  /** The only method that touches the DOM per frame. No allocation here. */
  update(): void {
    const t0 = performance.now();
    const st = this.viewport.scrollTop;
    const vh = this.viewport.clientHeight || 600;
    const n = this.count;

    let start = Math.floor(st / this.rowHeight) - this.overscan;
    if (start < 0) start = 0;
    let end = Math.ceil((st + vh) / this.rowHeight) + this.overscan;
    if (end > n) end = n;
    if (start > end) start = end;

    const need = end - start;
    // Grow the pool once; never shrink (reuse, don't churn the DOM).
    while (this.pool.length < need) this.pool.push(this.makeRow());

    const gen = this.generation;
    void gen;
    for (let p = 0; p < this.pool.length; p++) {
      const slot = this.pool[p];
      if (p < need) {
        const index = start + p;
        const selected = this.selectedIndex === index;
        if (slot.lastIndex === index && slot.lastSelected === selected) {
          // Already correct — ensure visible (may have been hidden) then skip.
          if (slot.root.style.display === "none") slot.root.style.display = "";
          continue;
        }
        // Writes only, batched after the two reads above.
        slot.root.style.display = "";
        slot.root.style.transform =
          "translateY(" + index * this.rowHeight + "px)";
        slot.root.dataset["index"] = String(index);
        this.renderRow(slot, index, selected);
        slot.lastIndex = index;
        slot.lastSelected = selected;
      } else {
        if (slot.lastIndex !== -2) {
          slot.root.style.display = "none";
          slot.lastIndex = -2;
        }
      }
    }

    const dt = performance.now() - t0;
    this.frames++;
    if (this.frameCosts.length < 600) this.frameCosts.push(dt);
    else this.frameCosts[this.frames % 600] = dt;
  }

  selectedIndex = -1;

  setSelected(index: number): void {
    if (this.selectedIndex === index) return;
    this.selectedIndex = index;
    this.schedule();
  }

  /** Force full re-render (e.g. rows mutated in place on BatchEnd). */
  invalidate(): void {
    for (let p = 0; p < this.pool.length; p++) this.pool[p].lastIndex = -1;
    this.schedule();
  }

  /** Synchronous re-render — used right after setRows for first-paint timing. */
  flush(): void {
    if (this.raf) {
      cancelAnimationFrame(this.raf);
      this.raf = 0;
    }
    this.update();
  }
}
