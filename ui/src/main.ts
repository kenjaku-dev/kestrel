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
import { createIpc } from "./lib/ipc";
import type { SortKeyDto } from "./lib/types";
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

  const ipc = createIpc();
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
      });
    });
    if (wantPerf) startPerfSampler(perfEl, windower, browser);
    // Dev-only: exercise the real scroll path (passive listener → rAF →
    // windower.update) on a loop so the HUD's frame costs are genuine
    // WebKitGTK measurements, not idle zeros.
    if (autoScrollWanted()) startAutoScroll(viewport);
  } else {
    if (wantPerf) startPerfSampler(perfEl, windower, browser);
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

  viewport.focus();

  function refreshPerfLine(): void {
    if (!wantPerf && fixtureN === null) return;
    appendPhase2Line(perfEl, windower, browser);
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
    phase2Line(browser);
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

function phase2Line(browser: Browser): string {
  const f = browser.lastFilterMs >= 0 ? browser.lastFilterMs.toFixed(1) + " ms" : "—";
  const s = browser.lastSortMs >= 0 ? browser.lastSortMs.toFixed(1) + " ms" : "—";
  return "\nfilter-to-paint " + f + " · sort-to-paint " + s + " · " + browser.watchStatus();
}

function appendPhase2Line(
  perfEl: HTMLElement,
  windower: Windower,
  browser: Browser
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
    (base ? base + "\n" : "") + scroll + phase2Line(browser);
}

/** While ?perf=1, refresh scroll frame stats twice a second. */
function startPerfSampler(
  perfEl: HTMLElement,
  windower: Windower,
  browser: Browser
): void {
  perfEl.style.display = "";
  setInterval(() => {
    appendPhase2Line(perfEl, windower, browser);
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

void main();
