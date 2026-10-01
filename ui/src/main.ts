/* Kestrel Phase 2 — bootstrap.
 *
 * - Builds the IPC layer (Tauri when inside the webview, mock otherwise —
 *   see lib/ipc.ts), the windower, and the browse controller.
 * - `?rows=N` renders N synthetic rows with no backend (windower proof).
 * - `&perf=1` shows the measurement HUD (first paint + scroll frame p50/p95
 *   + filter/sort timings + watch status + DOM pool size).
 * - `?path=/some/dir` sets the starting directory (default "/").
 * - Toolbar: breadcrumb (in Browser), filter-as-you-type, sort key +
 *   direction, hidden toggle — wired here, owned by Browser.
 */

import "./styles.css";
import { Browser } from "./lib/browser";
import {
  autoScrollWanted,
  makeSyntheticRows,
  parseFixtureParam,
  perfHudWanted,
} from "./lib/fixture";
import { createIpc, type KestrelIpc } from "./lib/ipc";
import type {
  CollisionDecision,
  FileEntryDto,
  OpRequestDto,
  SortKeyDto,
} from "./lib/types";
import { OpManager, isAlreadyExistsKind, type OpJob } from "./lib/ops";
import { CollisionDialog } from "./lib/collision";
import { formatSize } from "./lib/format";
import { OVERSCAN, ROW_HEIGHT, Windower } from "./lib/windower";

function el<T extends HTMLElement>(id: string): T {
  const node = document.getElementById(id);
  if (!node) throw new Error("missing element #" + id);
  return node as T;
}

function startPath(): string {
  try {
    const p = new URLSearchParams(window.location.search).get("path");
    if (p) return p;
  } catch {
    /* ignore */
  }
  return "/";
}

/** Dev-only `?open=<name>`: row to auto-activate after first listing. */
function autoOpenName(): string | null {
  try {
    return new URLSearchParams(window.location.search).get("open");
  } catch {
    return null;
  }
}

/**
 * Dev-only `?ipc=mock`: force the mock seam inside the real binary, so the
 * Phase 3a op progress stream is exercisable on WebKitGTK before the backend
 * lane lands `op_start`. Same category as `rows`/`perf`: never the default,
 * always explicit, and the status backend tag keeps reading honestly.
 */
function ipcForceParam(): "mock" | null {
  try {
    return new URLSearchParams(window.location.search).get("ipc") === "mock"
      ? "mock"
      : null;
  } catch {
    return null;
  }
}

/**
 * Dev-only `?opdemo=copy|move|trash|delete|copy-collide`: auto-start one op
 * on the selected (or first) entry after the listing completes, so progress
 * + keystroke-to-paint-with-an-op-running are screenshot-measurable without
 * synthetic input. Copy/move target `<src>-opdemo` in the same directory
 * unless `?opdst=` overrides it. `copy-collide` targets an existing
 * `README.md` to demo the `already_exists` error path.
 */
function opDemoParam(): string | null {
  try {
    const v = new URLSearchParams(window.location.search).get("opdemo");
    return v === "copy" ||
      v === "move" ||
      v === "trash" ||
      v === "delete" ||
      v === "copy-collide" ||
      v === "copy-ask"
      ? v
      : null;
  } catch {
    return null;
  }
}

/**
 * Dev-only `?collide=<file|dir|symlink|other>`: after an op starts, make the
 * mock hit a collision with THAT kind at that destination. Separated from
 * `?opdemo=copy-ask` so the wording for a folder collision — the one that
 * merges rather than replaces, and is the easiest thing to get wrong — is
 * reachable without editing code. No-op on the real backend.
 */
function collideParam(): string | null {
  try {
    const v = new URLSearchParams(window.location.search).get("collide");
    if (v === "file" || v === "dir" || v === "symlink" || v === "other")
      return v;
    return null;
  } catch {
    return null;
  }
}

/**
 * Dev-only `?opprompt=copy|move|delete`: open the destination prompt (or
 * delete confirm) for the selected/first entry without starting an op —
 * the prompt UI is screenshot-measurable without synthetic input.
 */
function opPromptParam(): string | null {
  try {
    const v = new URLSearchParams(window.location.search).get("opprompt");
    return v === "copy" || v === "move" || v === "delete" ? v : null;
  } catch {
    return null;
  }
}

/**
 * Dev-only `?opcancelms=N`: cancel the `?opdemo=` op N ms after it starts,
 * so the cancelling/cancelled rows (and the partial-destination note) are
 * screenshot-measurable without synthetic input.
 */
function opCancelMsParam(): number | null {
  try {
    const raw = new URLSearchParams(window.location.search).get("opcancelms");
    if (!raw) return null;
    const n = parseInt(raw, 10);
    return isNaN(n) || n < 0 ? null : Math.min(n, 10000);
  } catch {
    return null;
  }
}

/**
 * Dev-only `?opdst=<path>`: override the `<src>-opdemo` destination of a
 * `?opdemo=copy|move` run. Needed for cross-filesystem moves (the only slow
 * ones — same-dir moves are atomic renames and finish instantly).
 */
function opDstParam(): string | null {
  try {
    const v = new URLSearchParams(window.location.search).get("opdst");
    return v !== null && v !== "" ? v : null;
  } catch {
    return null;
  }
}

/**
 * Dev-only `?opstats=1`: append live per-job progress counters (events seen,
 * backwards jumps) to the job notes — the monotonic-progress evidence for
 * the exit gate, screenshot-readable.
 */
function opStatsParam(): boolean {
  try {
    return new URLSearchParams(window.location.search).get("opstats") === "1";
  } catch {
    return false;
  }
}

/**
 * Dev-only measurement params (same category as `rows`/`perf`/`open`):
 * `&filter=<q>`, `&sort=name|size|modified|kind`, `&dir=asc|desc`,
 * `&hidden=0|1`. Applied after the initial listing completes so timings
 * (keystroke-to-paint, sort-to-paint) land in the HUD/status line and are
 * screenshot-readable in the real binary, where synthetic input is
 * unavailable. No effect unless the params are present.
 */
async function applyDevParams(
  browser: Browser,
  ui: {
    filterEl: HTMLInputElement;
    sortKeyEl: HTMLSelectElement;
    sortDirEl: HTMLButtonElement;
    hiddenEl: HTMLInputElement;
  }
): Promise<void> {
  let q: URLSearchParams | null = null;
  try {
    q = new URLSearchParams(window.location.search);
  } catch {
    return;
  }
  const hidden = q.get("hidden");
  const sort = q.get("sort");
  const dir = q.get("dir");
  const filter = q.get("filter");
  if (hidden === null && sort === null && dir === null && filter === null)
    return;
  if (hidden === "0" || hidden === "1") {
    await browser.setShowHidden(hidden === "1");
    ui.hiddenEl.checked = browser.hiddenShown();
    await waitScan(browser);
  }
  if (sort !== null && isSortKey(sort)) {
    await browser.setSortKey(sort);
    ui.sortKeyEl.value = browser.currentSort().key;
    await waitScan(browser);
  }
  if (dir === "desc" && browser.currentSort().ascending) {
    await browser.toggleSortDir();
    await waitScan(browser);
  } else if (dir === "asc" && !browser.currentSort().ascending) {
    await browser.toggleSortDir();
    await waitScan(browser);
  }
  ui.sortDirEl.textContent = browser.currentSort().ascending ? "↑" : "↓";
  if (filter !== null) {
    browser.setFilter(filter);
    ui.filterEl.value = filter;
  }
}

async function waitScan(browser: Browser): Promise<void> {
  const t0 = Date.now();
  while (!browser.doneScanning && Date.now() - t0 < 15000) {
    await new Promise((r) => setTimeout(r, 100));
  }
  // Let the sort-to-paint double-rAF stamp land before a screenshot.
  await new Promise((r) =>
    requestAnimationFrame(() => requestAnimationFrame(() => r(null)))
  );
}

function percentile(sorted: number[], q: number): number {
  if (sorted.length === 0) return 0;
  const i = Math.min(sorted.length - 1, Math.floor(q * sorted.length));
  return sorted[i];
}

function isSortKey(v: string): v is SortKeyDto {
  return v === "name" || v === "size" || v === "modified" || v === "kind";
}

async function main(): Promise<void> {
  const viewport = el<HTMLElement>("viewport");
  const spacer = el<HTMLElement>("spacer");
  const rowsEl = el<HTMLElement>("rows");
  const crumbsEl = el<HTMLElement>("crumbs");
  const upBtn = el<HTMLButtonElement>("up-btn");
  const statusEl = el<HTMLElement>("status");
  const stateEl = el<HTMLElement>("state");
  const perfEl = el<HTMLElement>("perf");
  const backendTag = el<HTMLElement>("backend-tag");
  const filterEl = el<HTMLInputElement>("filter");
  const sortKeyEl = el<HTMLSelectElement>("sort-key");
  const sortDirEl = el<HTMLButtonElement>("sort-dir");
  const hiddenEl = el<HTMLInputElement>("hidden-toggle");

  const ipc = createIpc(ipcForceParam() === "mock" ? "mock" : undefined);
  backendTag.textContent = ipc.backend === "tauri" ? "tauri" : "mock";

  const windower = new Windower(viewport, spacer, rowsEl, ROW_HEIGHT, OVERSCAN);
  const browser = new Browser(
    ipc,
    { crumbsEl, upBtn, viewport, statusEl, stateEl, perfEl },
    windower
  );

  const fixtureN = parseFixtureParam();
  const wantPerf = perfHudWanted();

  // --- Phase 2 toolbar wiring (all state lives in Browser) ---
  sortKeyEl.value = browser.currentSort().key;
  sortDirEl.textContent = browser.currentSort().ascending ? "↑" : "↓";
  hiddenEl.checked = browser.hiddenShown();

  filterEl.addEventListener("input", () => {
    browser.setFilter(filterEl.value);
    refreshPerfLine();
  });
  filterEl.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      filterEl.value = "";
      browser.setFilter("");
      refreshPerfLine();
    }
    // Let Up/Down move selection without leaving the filter box.
    if (e.key === "ArrowDown" || e.key === "ArrowUp" || e.key === "Enter") {
      e.preventDefault();
      viewport.focus();
    }
  });
  sortKeyEl.addEventListener("change", () => {
    const v = sortKeyEl.value;
    if (isSortKey(v)) void browser.setSortKey(v).then(refreshPerfLine);
  });
  sortDirEl.addEventListener("click", () => {
    void browser.toggleSortDir().then(() => {
      sortDirEl.textContent = browser.currentSort().ascending ? "↑" : "↓";
      refreshPerfLine();
    });
  });
  hiddenEl.addEventListener("change", () => {
    void browser.setShowHidden(hiddenEl.checked).then(refreshPerfLine);
  });

  // --- Phase 3a ops wiring (selection lives in Browser; jobs live here) ---
  const appEl = el<HTMLElement>("app");
  const opsUi = setupOps({
    browser,
    ipc,
    statusEl,
    viewport,
    appEl,
    panelEl: el<HTMLElement>("op-panel"),
    jobsEl: el<HTMLElement>("jobs"),
    copyBtn: el<HTMLButtonElement>("op-copy"),
    moveBtn: el<HTMLButtonElement>("op-move"),
    trashBtn: el<HTMLButtonElement>("op-trash"),
    deleteBtn: el<HTMLButtonElement>("op-delete"),
    onHud: refreshPerfLine,
    showOpStats: opStatsParam(),
  });

  const autoOpen = autoOpenName();

  if (fixtureN !== null) {
    // Fixture path: measure build + first synchronous paint.
    const t0 = performance.now();
    const rows = makeSyntheticRows(fixtureN);
    const t1 = performance.now();
    browser.showFixture(rows, "/tmp/kestrel-perf/fixture (" + fixtureN + " rows)");
    // Dev-only measurement params (no-op unless present in the query).
    await applyDevParams(browser, { filterEl, sortKeyEl, sortDirEl, hiddenEl });
    const t2 = performance.now();
    // Two rAFs ≈ presented frame for the "first paint" number.
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        const t3 = performance.now();
        reportPerf(
          perfEl,
          wantPerf || true,
          fixtureN,
          t1 - t0,
          t2 - t1,
          t3 - t0,
          windower,
          browser
        );
        // Dev-only auto probe: keystroke-to-paint with no synthetic input,
        // so `--rows 10000 --perf` in the real binary still yields a genuine
        // filter number. Probe a wide query, record, then clear. (Clear
        // first: a `&filter=` dev param may already equal the probe query,
        // and setFilter early-returns 0 on no-change — that 0 is not a
        // measurement.)
        const pq = "file-000";
        browser.setFilter("");
        const probeMs = browser.setFilter(pq);
        browser.setFilter("");
        setExtraHudLine(
          'filter-probe "' + pq + '": ' + probeMs.toFixed(1) + " ms"
        );
        (window as unknown as Record<string, unknown>)[
          "__kestrelFilterProbeMs"
        ] = probeMs;
        appendPhase2Line(perfEl, windower, browser, opsUi.ops.activeCount());
      });
    });
    if (wantPerf) startPerfSampler(perfEl, windower, browser, () => opsUi.ops.activeCount());
    // Dev-only: exercise the real scroll path (passive listener → rAF →
    // windower.update) on a loop so the HUD's frame costs are genuine
    // WebKitGTK measurements, not idle zeros.
    if (autoScrollWanted()) startAutoScroll(viewport);
  } else {
    if (wantPerf) startPerfSampler(perfEl, windower, browser, () => opsUi.ops.activeCount());
    await browser.navigate(startPath());
    if (autoOpen !== null) {
      // The scan streams in after scan_start resolves; wait for completion
      // (max ~15 s) so the row exists before activating.
      const t0 = Date.now();
      while (!browser.doneScanning && Date.now() - t0 < 15000) {
        await new Promise((r) => setTimeout(r, 100));
      }
      const idx = browser.rows.findIndex(
        (r) => r.kind === "entry" && r.entry.name === autoOpen
      );
      if (idx >= 0) await browser.activate(idx);
      else {
        // A dev-measurement run may smuggle `&filter=`/`&sort=` after an
        // `&open=` value (the binary only forwards fixed flags); a miss
        // here is then expected and dev params repaint the status after.
        const hasDevParams = (() => {
          try {
            const qq = new URLSearchParams(window.location.search);
            return (
              qq.get("filter") !== null ||
              qq.get("sort") !== null ||
              qq.get("dir") !== null ||
              qq.get("hidden") !== null
            );
          } catch {
            return false;
          }
        })();
        if (!hasDevParams)
          statusEl.textContent = "auto-open: no row named " + autoOpen;
      }
    }
    // Dev-only measurement params (no-op unless present in the query).
    await applyDevParams(browser, { filterEl, sortKeyEl, sortDirEl, hiddenEl });
  }

  // Dev-only op demo (no-op unless `?opdemo=` is present in the query).
  await maybeStartOpDemo(browser, opsUi);
  // Dev-only prompt demo (no-op unless `?opprompt=` is present).
  maybeOpenOpPrompt(browser, opsUi);

  viewport.focus();

  function refreshPerfLine(): void {
    if (!wantPerf && fixtureN === null) return;
    appendPhase2Line(perfEl, windower, browser, opsUi.ops.activeCount());
  }

  // Exposed for automation / manual probing in devtools.
  const kestrel = {
    browser,
    windower,
    ipc,
    /** Keystroke-to-paint for a filter query (ms, synchronous paint). */
    setFilter: (q: string): number => {
      filterEl.value = q;
      const ms = browser.setFilter(q);
      refreshPerfLine();
      return ms;
    },
    /** Sort-to-paint: resolves after the re-scan completes and paints. */
    setSort: (key: SortKeyDto): Promise<void> => {
      sortKeyEl.value = key;
      return browser.setSortKey(key).then(() => {
        sortDirEl.textContent = browser.currentSort().ascending ? "↑" : "↓";
        refreshPerfLine();
      });
    },
    /** Liveness demo without the backend: deliver a mock `changed` event. */
    simulateWatchChange: (dirs: string[]): boolean => {
      const mock = ipc as unknown as Record<string, unknown>;
      const fn = mock["simulateChanged"];
      if (typeof fn === "function") {
        (fn as (dirs: string[]) => void).call(mock, dirs);
        return true;
      }
      return false;
    },
    /** Phase 3a: start an op on the selected entry (automation/devtools). */
    startOp: (kind: string): Promise<number | null> => opsUi.startForTest(kind),
    /** Phase 3a: cancel every in-flight op. */
    cancelAllOps: (): void => opsUi.cancelAll(),
    /** Phase 3a: in-flight op count (HUD + measurement). */
    opsActive: (): number => opsUi.ops.activeCount(),
    /** Phase 3a: DOM pool size after the job list was added (must stay flat). */
    domPoolSize: (): number => windowerPoolSize(windower),
    /* ---- Phase 3b: the collision dialog ------------------------------- */
    /** Is the collision dialog on screen right now? */
    collisionOpen: (): boolean => opsUi.dialog.isOpen,
    /** The collision currently on screen, or null. Read-only introspection. */
    collisionPending: (): { id: number; dst: string; kind: string } | null => {
      const jobs = opsUi.ops.liveJobs();
      for (let i = 0; i < jobs.length; i++) {
        const c = jobs[i].collision;
        if (c) return { id: c.id, dst: c.dst, kind: c.kind };
      }
      return null;
    },
    /** Answer the pending collision. Mirrors a click on a dialog button, and
     *  reports whether the answer was actually routed — a false here means it
     *  was stale, which is the guard doing its job rather than a failure. */
    answerCollision: (decision: string): boolean => {
      const pending = kestrel.collisionPending();
      if (!pending) return false;
      const coll = opsUi.ops.collisionFor(pending.id);
      if (!coll) return false;
      return opsUi.ops.answer(coll, decision as CollisionDecision);
    },
    /** Every answer sent, as the IPC seam recorded it. */
    collisionAnswers: () => CollisionDialog.answersOf(ipc),
    /** Re-sync: ask the backend what it is blocked on and open the dialog for
     *  each. This is the path that matters after a reload — a `collision`
     *  event is fire-and-forget over a Channel, so a window that mounted while
     *  an op was paused never saw the question. Returns how many it opened. */
    syncCollisions: async (): Promise<number> => {
      const rows = await ipc.opPendingCollisions();
      let opened = 0;
      for (let i = 0; i < rows.length; i++) {
        const row = rows[i];
        const known = opsUi.ops.collisionFor(row.id);
        // Already showing it: the event did arrive, or this call already ran.
        if (known && known.dst === row.dst) continue;
        const job = opsUi.ops
          .liveJobs()
          .find((j) => j.id === row.id);
        if (!job) continue; // we have no job row for it; nothing to show
        opsUi.ops.adoptCollision(job, { id: row.id, dst: row.dst, kind: row.kind, seq: 0 });
        opened++;
      }
      return opened;
    },
  };
  (window as unknown as Record<string, unknown>)["__kestrel"] = kestrel;
}

function reportPerf(
  perfEl: HTMLElement,
  _show: boolean,
  n: number,
  buildMs: number,
  paintMs: number,
  totalMs: number,
  windower: Windower,
  browser: Browser
): void {
  perfEl.style.display = "";
  perfEl.textContent =
    "fixture: " +
    n +
    " rows · build " +
    buildMs.toFixed(1) +
    " ms · window paint " +
    paintMs.toFixed(1) +
    " ms · to presented " +
    totalMs.toFixed(1) +
    " ms · DOM nodes in pool: " +
    windowerPoolSize(windower) +
    phase2Line(browser) +
    (extraHudLine !== "" ? "\n" + extraHudLine : "");
  (window as unknown as Record<string, unknown>)["__kestrelPerf"] = {
    rows: n,
    buildMs,
    paintMs,
    presentedMs: totalMs,
  };
}

function windowerPoolSize(w: Windower): number {
  return (w as unknown as { pool: unknown[] }).pool.length;
}

/**
 * Phase 3a HUD extension: one sticky extra line (filter probe, with-op
 * filter timing) that survives the 500 ms sampler rewrites. Without this,
 * anything appended to the HUD is wiped on the next tick and unmeasurable
 * in screenshots.
 */
let extraHudLine = "";

function setExtraHudLine(s: string): void {
  extraHudLine = extraHudLine === "" ? s : extraHudLine + "\n" + s;
}

function phase2Line(browser: Browser, opsActive?: number): string {
  const f = browser.lastFilterMs >= 0 ? browser.lastFilterMs.toFixed(1) + " ms" : "—";
  const s = browser.lastSortMs >= 0 ? browser.lastSortMs.toFixed(1) + " ms" : "—";
  const ops = opsActive !== undefined ? " · ops active " + opsActive : "";
  return "\nfilter-to-paint " + f + " · sort-to-paint " + s + " · " + browser.watchStatus() + ops;
}

function appendPhase2Line(
  perfEl: HTMLElement,
  windower: Windower,
  browser: Browser,
  opsActive?: number
): void {
  perfEl.style.display = "";
  const costs = windower.frameCosts.slice().sort((a, b) => a - b);
  const p50 = percentile(costs, 0.5);
  const p95 = percentile(costs, 0.95);
  const cur = perfEl.textContent || "";
  const first = cur.split("\n")[0];
  const base = first.indexOf("fixture:") === 0 ? first : "";
  const scroll =
    "scroll frames: " +
    costs.length +
    " · update p50 " +
    p50.toFixed(2) +
    " ms · p95 " +
    p95.toFixed(2) +
    " ms";
  perfEl.textContent =
    (base ? base + "\n" : "") +
    scroll +
    phase2Line(browser, opsActive) +
    (extraHudLine !== "" ? "\n" + extraHudLine : "");
}

/** While ?perf=1, refresh scroll frame stats twice a second. */
function startPerfSampler(
  perfEl: HTMLElement,
  windower: Windower,
  browser: Browser,
  opsCount?: () => number
): void {
  perfEl.style.display = "";
  setInterval(() => {
    appendPhase2Line(
      perfEl,
      windower,
      browser,
      opsCount ? opsCount() : undefined
    );
  }, 500);
}

/**
 * Dev-only scroll driver for `&autoscroll=1`: steps the viewport down ~6 s
 * on a loop (wrapping to top), so the HUD records genuine scroll-frame
 * costs. Each step goes through the production path — a real `scrollTop`
 * assignment, the passive scroll listener, one rAF, `windower.update()`.
 */
function startAutoScroll(viewport: HTMLElement): void {
  const t0 = performance.now();
  const DURATION_MS = 6000;
  const step = () => {
    const vh = viewport.clientHeight || 600;
    viewport.scrollTop += Math.max(200, Math.floor(vh * 0.75));
    if (viewport.scrollTop + vh >= viewport.scrollHeight) viewport.scrollTop = 0;
    if (performance.now() - t0 < DURATION_MS) setTimeout(step, 50);
  };
  // Start after first paint settles so scroll costs don't include setup.
  setTimeout(step, 1500);
}

/**
 * Dev-only `?opdemo=`: auto-start one op after the listing completes.
 * Copy/move target `<src>-opdemo` in the same directory. When `&filter=`
 * is also present, the filter is re-applied mid-stream (measuring window)
 * so the HUD's filter-to-paint number is genuinely "with an op running".
 */
async function maybeStartOpDemo(
  browser: Browser,
  opsUi: OpsHandle
): Promise<void> {
  const kind = opDemoParam();
  if (!kind) return;
  const t0 = Date.now();
  while (!browser.doneScanning && Date.now() - t0 < 15000) {
    await new Promise((r) => setTimeout(r, 25));
  }
  // One frame past listing paint: the job row must exist as early as
  // possible so headless captures (which fire ~130 ms after load) still
  // land inside the measuring window.
  await new Promise((r) => requestAnimationFrame(() => r(null)));
  const id = await opsUi.startForTest(kind);
  if (id === null) return;
  const cancelMs = opCancelMsParam();
  if (cancelMs !== null) {
    await new Promise((r) => setTimeout(r, cancelMs));
    opsUi.ops.cancel(id);
    // Let the cancelling→cancelled flip land before a screenshot.
    await new Promise((r) => setTimeout(r, 600));
    return;
  }
  let filterQ: string | null = null;
  try {
    filterQ = new URLSearchParams(window.location.search).get("filter");
  } catch {
    filterQ = null;
  }
  if (filterQ !== null) {
    // Mid-measuring window: progress events are streaming, total unknown.
    // With `?opslow=1` the window is wide, so measure early enough for a
    // headless capture to see the sticky HUD line.
    let slow = false;
    try {
      slow = new URLSearchParams(window.location.search).get("opslow") === "1";
    } catch {
      slow = false;
    }
    await new Promise((r) => setTimeout(r, slow ? 100 : 700));
    browser.setFilter("");
    const ms = browser.setFilter(filterQ);
    (window as unknown as Record<string, unknown>)[
      "__kestrelFilterWithOpMs"
    ] = ms;
    setExtraHudLine(
      'filter-with-op "' + filterQ + '": ' + ms.toFixed(1) + " ms"
    );
  }
}

/* ------------------------------------------------------------------ */
/* Phase 3a — ops UI: buttons + shortcuts, destination prompt, jobs     */
/* ------------------------------------------------------------------ */

/** Dev-only `?opprompt=`: open the prompt/confirm with no synthetic input. */
function maybeOpenOpPrompt(browser: Browser, opsUi: OpsHandle): void {
  const kind = opPromptParam();
  if (!kind) return;
  // The demo runner is skipped when a prompt is requested, so poll for the
  // listing here instead of reusing its wait.
  const t0 = Date.now();
  const poll = (): void => {
    if (!browser.doneScanning && Date.now() - t0 < 15000) {
      setTimeout(poll, 25);
      return;
    }
    opsUi.openPrompt(kind);
  };
  poll();
}

interface OpsContext {
  browser: Browser;
  ipc: KestrelIpc;
  statusEl: HTMLElement;
  viewport: HTMLElement;
  panelEl: HTMLElement;
  jobsEl: HTMLElement;
  appEl: HTMLElement;
  copyBtn: HTMLButtonElement;
  moveBtn: HTMLButtonElement;
  trashBtn: HTMLButtonElement;
  deleteBtn: HTMLButtonElement;
  onHud: () => void;
  /** Dev-only `?opstats=1`: show live per-job progress counters in notes. */
  showOpStats: boolean;
}

interface OpsHandle {
  ops: OpManager;
  dialog: CollisionDialog;
  startForTest(kind: string): Promise<number | null>;
  cancelAll(): void;
  openPrompt(kind: string): void;
}

interface JobNodes {
  root: HTMLDivElement;
  label: HTMLSpanElement;
  phase: HTMLSpanElement;
  bytes: HTMLSpanElement;
  bar: HTMLProgressElement;
  note: HTMLSpanElement;
  cancelBtn: HTMLButtonElement;
  dismissBtn: HTMLButtonElement;
}

/**
 * Wire the four operations. Every op is reachable by a visible button AND
 * a keyboard shortcut (a hidden keyboard-only feature is not shipped):
 * Ctrl+C copy, Ctrl+M move, Delete trash, Shift+Delete permanent delete.
 * Shortcuts fire only when the list itself has focus — never while typing
 * in the filter, the sort box, or the destination prompt.
 */
function setupOps(ctx: OpsContext): OpsHandle {
  // The dialog is constructed first: it needs the manager, and the manager's
  // collision sinks need the dialog. Neither constructor talks to the other,
  // so the ordering is a wiring convenience and nothing more.
  let dialog: CollisionDialog;
  const ops = new OpManager(ctx.ipc, {
    onJobsChanged: (jobs) => renderJobs(ctx, ops, jobs),
    onOpError: (error) => ctx.browser.showOpError(error),
    onSettled: () => {
      ctx.browser.rescan();
      ctx.onHud();
    },
    // Phase 3b. Both sinks are the OpManager telling the view the truth: a
    // job is now blocked, or no longer blocked. The view never guesses from
    // progress events, because "blocked" is not derivable from them.
    onCollision: (job) => {
      if (job.collision) dialog.enqueue(job, job.collision);
    },
    onCollisionClosed: (id) => dialog.release(id),
  });
  dialog = new CollisionDialog(
    document.body,
    ctx.appEl,
    ops,
    ctx.ipc,
    () => ctx.viewport
  );

  const selectedOrHint = (): FileEntryDto | null => {
    const e = ctx.browser.selectedEntry();
    if (!e)
      ctx.statusEl.textContent =
        "Select a file or folder first (click a row or use arrow keys).";
    return e;
  };

  ctx.copyBtn.addEventListener("click", () => {
    const e = selectedOrHint();
    if (e) openCopyMove(ctx, ops, "copy", e);
  });
  ctx.moveBtn.addEventListener("click", () => {
    const e = selectedOrHint();
    if (e) openCopyMove(ctx, ops, "move", e);
  });
  ctx.trashBtn.addEventListener("click", () => {
    const e = selectedOrHint();
    if (e) void startOp(ctx, ops, { op: "trash", src: e.path });
  });
  ctx.deleteBtn.addEventListener("click", () => {
    const e = selectedOrHint();
    if (e) openDeleteConfirm(ctx, ops, e);
  });

  ctx.browser.opKeyHandler = (e) => {
    // The collision dialog is modal: while it is up, the list is not
    // answering keys at all. Without this the shortcuts below would start a
    // SECOND op from under an open dialog.
    if (dialog.isOpen) return true;
    if (isTypingTarget(ctx)) return false;
    const mod = e.ctrlKey && !e.shiftKey && !e.altKey && !e.metaKey;
    if (mod && (e.key === "c" || e.key === "C")) {
      const s = selectedOrHint();
      if (s) openCopyMove(ctx, ops, "copy", s);
      e.preventDefault();
      return true;
    }
    if (mod && (e.key === "m" || e.key === "M")) {
      const s = selectedOrHint();
      if (s) openCopyMove(ctx, ops, "move", s);
      e.preventDefault();
      return true;
    }
    if (!e.ctrlKey && !e.altKey && !e.metaKey && e.key === "Delete") {
      const s = selectedOrHint();
      if (s) {
        if (e.shiftKey) openDeleteConfirm(ctx, ops, s);
        else void startOp(ctx, ops, { op: "trash", src: s.path });
      }
      e.preventDefault();
      return true;
    }
    return false;
  };

  const handle: OpsHandle = {
    ops,
    dialog,
    cancelAll: () => {
      const jobs = ops.liveJobs();
      for (let i = 0; i < jobs.length; i++) ops.cancel(jobs[i].id);
    },
    openPrompt: (kind: string): void => {
      const e = firstEntry(ctx);
      if (!e) return;
      if (kind === "copy" || kind === "move") openCopyMove(ctx, ops, kind, e);
      else if (kind === "delete") openDeleteConfirm(ctx, ops, e);
    },
    startForTest: (kind: string): Promise<number | null> => {
      const e = firstEntry(ctx);
      if (!e) return Promise.resolve(null);
      if (kind === "copy" || kind === "move") {
        const dst = opDstParam();
        return startOp(ctx, ops, {
          op: kind,
          src: e.path,
          dst: dst !== null ? dst : e.path + "-opdemo",
        });
      }
      if (kind === "copy-collide")
        // Targets an existing README.md in the viewed directory: the mock
        // parks there and the 3b dialog opens (3b asks; `already_exists` is
        // still covered by the seam suite from the committed Rust bytes).
        return startOp(ctx, ops, {
          op: "copy",
          src: e.path,
          dst: dirName(e.path) + "/README.md",
        });
      if (kind === "copy-ask")
        // Forces a collision at a destination the mock's name heuristic would
        // not recognise, so the dialog is reachable for any op and any `kind`
        // via `&collide=`. Waits for the op to exist before asking.
        return startForcedCollision(ctx, ops, e, "file");
      if (kind === "trash") return startOp(ctx, ops, { op: "trash", src: e.path });
      if (kind === "delete")
        return startOp(ctx, ops, {
          op: "delete",
          src: e.path,
          recursive: e.descendable,
        });
      return Promise.resolve(null);
    },
  };
  return handle;
}

/**
 * Dev-only: start a copy and make the mock collide on it at a destination the
 * name heuristic would not catch. Polls briefly for the job to exist rather
 * than assuming a JobId, because `op_start` returns before the stream begins.
 */
async function startForcedCollision(
  ctx: OpsContext,
  ops: OpManager,
  entry: FileEntryDto,
  kind: "file" | "dir" | "symlink" | "other"
): Promise<number | null> {
  const dst = dirName(entry.path) + "/" + entry.name + "-ask";
  const id = await startOp(ctx, ops, { op: "copy", src: entry.path, dst });
  if (id === null) return null;
  const mock = ctx.ipc as unknown as Record<string, unknown>;
  const sim = mock["simulateCollision"];
  if (typeof sim !== "function") {
    ctx.statusEl.textContent =
      "simulateCollision is mock-only; the real backend sends collisions itself.";
    return id;
  }
  const want = collideParam() || kind;
  const t0 = Date.now();
  while (Date.now() - t0 < 2000) {
    if ((sim as (d: string, k: string) => boolean).call(mock, dst, want))
      return id;
    await new Promise((r) => setTimeout(r, 25));
  }
  ctx.statusEl.textContent = "no live op to collide with";
  return id;
}

/** Selected entry, else the first entry row (dev/automation entry point). */
function firstEntry(ctx: OpsContext): FileEntryDto | null {
  const sel = ctx.browser.selectedEntry();
  if (sel) return sel;
  const rows = ctx.browser.rows;
  for (let i = 0; i < rows.length; i++) {
    const r = rows[i];
    if (r.kind === "entry") return r.entry;
  }
  return null;
}

function dirName(p: string): string {
  const i = Math.max(p.lastIndexOf("/"), p.lastIndexOf("\\"));
  if (i <= 0) return "/";
  return p.slice(0, i);
}

/** True while the user is typing somewhere ops shortcuts must not fire. */
function isTypingTarget(ctx: OpsContext): boolean {
  const a = document.activeElement;
  if (!a || a === ctx.viewport) return false;
  const tag = (a.tagName || "").toUpperCase();
  if (tag === "INPUT" || tag === "SELECT" || tag === "TEXTAREA") return true;
  try {
    if (ctx.panelEl.contains(a)) return true;
  } catch {
    /* ignore */
  }
  return false;
}

function startOp(
  ctx: OpsContext,
  ops: OpManager,
  req: OpRequestDto
): Promise<number | null> {
  return ops
    .start(req)
    .then((id) => {
      ctx.viewport.focus();
      return id as number;
    })
    .catch((err) => {
      // The backend lane may not have landed op_start yet: say so honestly
      // on the status line instead of failing silently.
      ctx.statusEl.textContent =
        "Operation failed to start: " + messageOf(err);
      return null;
    });
}

function clearEl(node: HTMLElement): void {
  while (node.firstChild) node.removeChild(node.firstChild);
}

function closePanel(ctx: OpsContext): void {
  clearEl(ctx.panelEl);
  ctx.panelEl.style.display = "none";
}

/**
 * Destination prompt for copy/move. There is no meaningful default target,
 * so the user names one: the directory to copy INTO (default: the viewed
 * directory, always shown verbatim) plus the new name (default: the source
 * name). A live preview shows the full destination path. Inline form, not
 * a modal dialog — 3a ships no dialogs.
 */
function openCopyMove(
  ctx: OpsContext,
  ops: OpManager,
  kind: "copy" | "move",
  src: FileEntryDto
): void {
  const verb = kind === "copy" ? "Copy" : "Move";
  clearEl(ctx.panelEl);
  ctx.panelEl.style.display = "";

  const title = document.createElement("p");
  title.className = "k-op-title";
  title.textContent = verb + " “" + src.name + "”";
  ctx.panelEl.appendChild(title);

  const dirLabel = document.createElement("label");
  dirLabel.textContent = "Into directory:";
  const dirInput = document.createElement("input");
  dirInput.type = "text";
  dirInput.value = ctx.browser.currentPath || "/";
  dirInput.setAttribute("aria-label", "Destination directory");
  dirLabel.appendChild(dirInput);
  ctx.panelEl.appendChild(dirLabel);

  const nameLabel = document.createElement("label");
  nameLabel.textContent = "New name:";
  const nameInput = document.createElement("input");
  nameInput.type = "text";
  nameInput.value = src.name;
  nameInput.setAttribute("aria-label", "Destination file name");
  nameLabel.appendChild(nameInput);
  ctx.panelEl.appendChild(nameLabel);

  const preview = document.createElement("p");
  preview.className = "k-op-target";
  ctx.panelEl.appendChild(preview);
  const repaint = (): void => {
    preview.textContent = "Destination: " + joinDst(dirInput.value, nameInput.value);
  };
  dirInput.addEventListener("input", repaint);
  nameInput.addEventListener("input", repaint);
  repaint();

  const hint = document.createElement("p");
  hint.className = "k-op-hint";
  hint.textContent =
    "If that destination already exists the op fails and leaves it " +
    "untouched — nothing is overwritten, and there is no collision " +
    "dialog in Phase 3a (a later phase adds it).";
  ctx.panelEl.appendChild(hint);

  const go = document.createElement("button");
  go.type = "button";
  go.textContent = kind === "copy" ? "Copy here" : "Move here";
  const cancel = document.createElement("button");
  cancel.type = "button";
  cancel.textContent = "Cancel";
  ctx.panelEl.appendChild(go);
  ctx.panelEl.appendChild(cancel);

  const confirm = (): void => {
    const dst = joinDst(dirInput.value, nameInput.value);
    if (!nameInput.value) {
      preview.textContent = "Destination: give the copy a name.";
      nameInput.focus();
      return;
    }
    closePanel(ctx);
    const req: OpRequestDto =
      kind === "copy"
        ? { op: "copy", src: src.path, dst }
        : { op: "move", src: src.path, dst };
    void startOp(ctx, ops, req);
  };
  go.addEventListener("click", confirm);
  cancel.addEventListener("click", () => {
    closePanel(ctx);
    ctx.viewport.focus();
  });
  nameInput.addEventListener("keydown", (e) => {
    if (e.key === "Enter") confirm();
    else if (e.key === "Escape") {
      closePanel(ctx);
      ctx.viewport.focus();
    }
  });
  dirInput.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      closePanel(ctx);
      ctx.viewport.focus();
    }
  });
  nameInput.focus();
  try {
    nameInput.select();
  } catch {
    /* older webviews may lack select */
  }
}

/** Permanent delete always confirms inline: it cannot be undone. */
function openDeleteConfirm(
  ctx: OpsContext,
  ops: OpManager,
  src: FileEntryDto
): void {
  clearEl(ctx.panelEl);
  ctx.panelEl.style.display = "";

  const title = document.createElement("p");
  title.className = "k-op-title";
  title.textContent = "Delete “" + src.name + "” permanently?";
  ctx.panelEl.appendChild(title);

  const warn = document.createElement("p");
  warn.className = "k-op-warn";
  warn.textContent =
    "This cannot be undone (unlike Trash)." +
    (src.descendable ? " It is a directory: its contents are deleted too." : "");
  ctx.panelEl.appendChild(warn);

  const del = document.createElement("button");
  del.type = "button";
  del.textContent = "Delete forever";
  const cancel = document.createElement("button");
  cancel.type = "button";
  cancel.textContent = "Cancel";
  ctx.panelEl.appendChild(del);
  ctx.panelEl.appendChild(cancel);

  del.addEventListener("click", () => {
    closePanel(ctx);
    void startOp(ctx, ops, {
      op: "delete",
      src: src.path,
      recursive: src.descendable,
    });
  });
  cancel.addEventListener("click", () => {
    closePanel(ctx);
    ctx.viewport.focus();
  });
  del.focus();
}

/* ---------------- job list (never the windower) ---------------- */

const jobNodes = new Map<number, JobNodes>();

/**
 * Keyed, in-place job rows: one DOM row per JobId, created once and updated
 * per event (textContent + <progress> value only). Progress traffic never
 * allocates list rows and never invalidates the windower pool.
 */
function renderJobs(ctx: OpsContext, ops: OpManager, jobs: OpJob[]): void {
  const seen = new Map<number, boolean>();
  for (let i = 0; i < jobs.length; i++) seen.set(jobs[i].id, true);
  const dead: number[] = [];
  jobNodes.forEach((_v, id) => {
    if (!seen.has(id)) dead.push(id);
  });
  for (let i = 0; i < dead.length; i++) {
    const n = jobNodes.get(dead[i]);
    if (n && n.root.parentNode === ctx.jobsEl) ctx.jobsEl.removeChild(n.root);
    jobNodes.delete(dead[i]);
  }
  for (let i = 0; i < jobs.length; i++) {
    const job = jobs[i];
    let n = jobNodes.get(job.id);
    if (!n) {
      n = makeJobRow(ctx, ops, job.id);
      ctx.jobsEl.appendChild(n.root);
      jobNodes.set(job.id, n);
    }
    paintJobRow(n, job, ctx.showOpStats);
  }
}

function makeJobRow(ctx: OpsContext, ops: OpManager, id: number): JobNodes {
  const root = document.createElement("div");
  root.className = "k-job";
  const top = document.createElement("span");
  top.className = "k-job-top";
  const label = document.createElement("span");
  label.className = "k-job-label";
  const phase = document.createElement("span");
  phase.className = "k-job-phase";
  const bytes = document.createElement("span");
  bytes.className = "k-job-bytes";
  const bar = document.createElement("progress");
  bar.setAttribute("aria-label", "Operation progress");
  const cancelBtn = document.createElement("button");
  cancelBtn.type = "button";
  cancelBtn.textContent = "Cancel";
  cancelBtn.addEventListener("click", () => {
    // One route to "stop this op": a paused job answers its pending collision
    // with `abort` rather than sending op_cancel, because the engine is
    // blocked inside the handshake and only the answer releases it.
    const coll = ops.collisionFor(id);
    if (coll) ops.abortForCollision(coll);
    else ops.cancel(id);
  });
  const dismissBtn = document.createElement("button");
  dismissBtn.type = "button";
  dismissBtn.textContent = "Dismiss";
  dismissBtn.addEventListener("click", () => {
    ops.dismiss(id);
    ctx.viewport.focus();
  });
  top.appendChild(label);
  top.appendChild(phase);
  top.appendChild(bytes);
  top.appendChild(bar);
  top.appendChild(cancelBtn);
  top.appendChild(dismissBtn);
  const note = document.createElement("span");
  note.className = "k-job-note";
  root.appendChild(top);
  root.appendChild(note);
  return { root, label, phase, bytes, bar, note, cancelBtn, dismissBtn };
}

function paintJobRow(n: JobNodes, job: OpJob, showStats: boolean): void {
  n.label.textContent = job.label;
  n.phase.textContent = jobPhaseText(job);
  n.root.className = "k-job" + (job.status === "failed" ? " is-failed" : "");
  if (job.unknownTotal && job.status === "running") {
    // Honest indeterminate: measuring has no total yet. The <progress>
    // element with NO value attribute is the semantic indeterminate state
    // (modern-web-guidance spinner); a guessed percentage would be a bug.
    n.bar.removeAttribute("value");
    n.bar.removeAttribute("max");
    n.bytes.textContent = job.doneItems > 0 ? job.doneItems + " items seen" : "";
  } else if (!job.unknownTotal && job.totalBytes > 0) {
    n.bar.setAttribute("max", String(job.totalBytes));
    n.bar.setAttribute("value", String(job.doneBytes));
    let t =
      formatSize(job.doneBytes) + " / " + formatSize(job.totalBytes);
    if (job.totalItems > 0)
      t += " · " + job.doneItems + "/" + job.totalItems + " items";
    n.bytes.textContent = t;
  } else {
    n.bar.removeAttribute("value");
    n.bar.removeAttribute("max");
    n.bytes.textContent = "";
  }
  n.note.textContent = jobNoteText(job, showStats);
  // A paused job keeps its Cancel button, because stopping the operation is
  // still available — it is one of the three answers. The button does not
  // work by dismissing the dialog; it routes through the same `answer` path
  // so there is exactly one implementation of "stop this op".
  const running = job.status === "running" || job.status === "paused";
  const terminal =
    job.status === "done" ||
    job.status === "failed" ||
    job.status === "cancelled";
  n.root.className =
    "k-job" +
    (job.status === "failed" ? " is-failed" : "") +
    (job.status === "paused" ? " is-paused" : "");
  n.cancelBtn.style.display = running || job.status === "cancelling" ? "" : "none";
  n.cancelBtn.textContent = job.status === "cancelling" ? "Cancelling…" : "Cancel";
  n.cancelBtn.disabled = job.status !== "running" && job.status !== "paused";
  n.dismissBtn.style.display = terminal ? "" : "none";
}

function jobPhaseText(job: OpJob): string {
  if (job.status === "done") return "done";
  if (job.status === "failed") return "failed";
  if (job.status === "cancelled") return "cancelled";
  if (job.status === "cancelling") return "cancelling";
  // "paused", not the copy phase. The engine is not copying; it is waiting on
  // a human, and a row that says "copying" during a blocking question is a
  // progress lie.
  if (job.status === "paused") return "paused";
  return job.phase;
}

/**
 * The cancel contract, stated in the UI: the engine leaves a partial
 * destination behind on cancel by design, so the user is told to check it
 * rather than discovering a half-copied file. The note also says WHO
 * settled the job — a backend confirmation with its measured latency, or
 * the local 400 ms fallback — so the settle source is never asserted, only
 * reported (Task 3).
 */
function jobNoteText(job: OpJob, showStats: boolean): string {
  const stats =
    showStats && (job.status === "running" || job.status === "done")
      ? " [events " +
        job.progressEvents +
        ", backwards " +
        job.backwardsJumps +
        "]"
      : "";
  if (job.status === "cancelling") return "Cancelling…";
  if (job.status === "paused") return pausedNote(job);
  if (job.status === "cancelled") return cancelNote(job);
  if (job.status === "done") return job.summary + stats;
  if (job.status === "failed" && job.error) {
    let t = job.error.kind + ": " + job.error.message;
    if (isAlreadyExistsKind(job.error.kind))
      t += " The destination was left untouched — the collision dialog arrives in a later phase.";
    return t;
  }
  if (job.status === "running" && job.unknownTotal)
    return "Measuring size — no total yet, so no percentage is shown." + stats;
  if (job.status === "running") return stats !== "" ? stats.slice(1) : "";
  return "";
}

/**
 * The paused note. Says the op is stopped and WHY, names the destination the
 * decision is about, and says how many collisions this op has already had —
 * so "one at a time" is visible rather than assumed. A paused job keeps
 * whatever byte count it had reached: that number is true as of the pause, and
 * it does not move, which is exactly why the phase reads "paused".
 */
function pausedNote(job: OpJob): string {
  if (!job.collision) return "Paused.";
  const bytes =
    !job.unknownTotal && job.totalBytes > 0
      ? " " + formatSize(job.doneBytes) + " of " + formatSize(job.totalBytes) +
        " written before the pause."
      : "";
  const more =
    job.collisionCount > 1
      ? " This is collision " + job.collisionCount + " in this operation; " +
        "each one is answered on its own."
      : "";
  return (
    "Paused, waiting for you to decide about " +
    job.collision.dst +
    ". Nothing is being written until you do." +
    bytes +
    more
  );
}

function cancelNote(job: OpJob): string {
  const during =
    job.phaseAtCancel !== "" ? " (during " + job.phaseAtCancel + ")" : "";
  if (
    job.settleSource === "backend" &&
    job.terminalEventAt > job.cancelRequestedAt
  ) {
    const ms = Math.round(job.terminalEventAt - job.cancelRequestedAt);
    return (
      job.summary +
      during +
      " Backend confirmed cancel " +
      ms +
      " ms after the request."
    );
  }
  return (
    job.summary +
    during +
    " No backend confirmation arrived within 400 ms; settled locally."
  );
}

function joinDst(dir: string, name: string): string {
  let d = dir;
  while (d.length > 1 && d.charAt(d.length - 1) === "/") d = d.slice(0, -1);
  if (d === "") d = "/";
  const sep = d.charAt(d.length - 1) === "/" ? "" : "/";
  return d + sep + name;
}

function messageOf(err: unknown): string {
  if (err instanceof Error) return err.message;
  try {
    return JSON.stringify(err);
  } catch {
    return String(err);
  }
}

void main();
