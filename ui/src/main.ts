/* Kestrel Phase 1 — bootstrap.
 *
 * - Builds the IPC layer (Tauri when inside the webview, mock otherwise —
 *   see lib/ipc.ts), the windower, and the browse controller.
 * - `?rows=N` renders N synthetic rows with no backend (windower proof).
 * - `&perf=1` shows the measurement HUD (first paint + scroll frame p50/p95).
 * - `?path=/some/dir` sets the starting directory (default "/").
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

function percentile(sorted: number[], q: number): number {
  if (sorted.length === 0) return 0;
  const i = Math.min(sorted.length - 1, Math.floor(q * sorted.length));
  return sorted[i];
}

async function main(): Promise<void> {
  const viewport = el<HTMLElement>("viewport");
  const spacer = el<HTMLElement>("spacer");
  const rowsEl = el<HTMLElement>("rows");
  const pathEl = el<HTMLElement>("path");
  const upBtn = el<HTMLButtonElement>("up-btn");
  const statusEl = el<HTMLElement>("status");
  const stateEl = el<HTMLElement>("state");
  const perfEl = el<HTMLElement>("perf");
  const backendTag = el<HTMLElement>("backend-tag");

  const ipc = createIpc();
  backendTag.textContent = ipc.backend === "tauri" ? "tauri" : "mock";

  const windower = new Windower(viewport, spacer, rowsEl, ROW_HEIGHT, OVERSCAN);
  const browser = new Browser(
    ipc,
    { pathEl, upBtn, viewport, statusEl, stateEl, perfEl },
    windower
  );

  const fixtureN = parseFixtureParam();
  const wantPerf = perfHudWanted();

  // Dev-only verification hook (see lib.rs `fixture_query --open`): after the
  // initial listing completes, activate the named row — the exact code path
  // as Enter/double-click — so `open_path` can be exercised in a window
  // where synthetic input is unavailable. Never present in normal use.
  const autoOpen = autoOpenName();

  if (fixtureN !== null) {
    // Fixture path: measure build + first synchronous paint.
    const t0 = performance.now();
    const rows = makeSyntheticRows(fixtureN);
    const t1 = performance.now();
    browser.showFixture(rows, "/tmp/kestrel-perf/fixture (" + fixtureN + " rows)");
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
          windower
        );
      });
    });
    if (wantPerf) startPerfSampler(perfEl, windower);
    // Dev-only: exercise the real scroll path (passive listener → rAF →
    // windower.update) on a loop so the HUD's frame costs are genuine
    // WebKitGTK measurements, not idle zeros.
    if (autoScrollWanted()) startAutoScroll(viewport);
  } else {
    if (wantPerf) startPerfSampler(perfEl, windower);
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
      else statusEl.textContent = "auto-open: no row named " + autoOpen;
    }
  }

  viewport.focus();

  // Exposed for automation / manual probing in devtools.
  (window as unknown as Record<string, unknown>)["__kestrel"] = {
    browser,
    windower,
    ipc,
  };
}

function reportPerf(
  perfEl: HTMLElement,
  _show: boolean,
  n: number,
  buildMs: number,
  paintMs: number,
  totalMs: number,
  windower: Windower
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
    windowerPoolSize(windower);
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

/** While ?perf=1, refresh scroll frame stats twice a second. */
function startPerfSampler(perfEl: HTMLElement, windower: Windower): void {
  perfEl.style.display = "";
  setInterval(() => {
    const costs = windower.frameCosts.slice().sort((a, b) => a - b);
    const p50 = percentile(costs, 0.5);
    const p95 = percentile(costs, 0.95);
    const scroll =
      "scroll frames: " +
      costs.length +
      " · update p50 " +
      p50.toFixed(2) +
      " ms · p95 " +
      p95.toFixed(2) +
      " ms";
    // Re-read the first line every tick: the fixture summary is written on a
    // later rAF than sampler setup, and a stale snapshot would clobber it.
    const cur = perfEl.textContent || "";
    const first = cur.split("\n")[0];
    const base = first.indexOf("fixture:") === 0 ? first : "";
    perfEl.textContent = base ? base + "\n" + scroll : scroll;
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
