/* Kestrel Phase 1 — synthetic fixture rows.
 *
 * Dev-only way to prove the windower on a large list without a backend:
 * open `?rows=10000` (optionally `&perf=1` for the measurement HUD).
 * Deterministic (arithmetic PRNG, no Math.random) so runs are comparable.
 * Safari13-safe: no Array.at, no structuredClone, no String.replaceAll.
 */

import type { FileEntryDto, Row } from "./types";

export function parseFixtureParam(): number | null {
  try {
    const q = new URLSearchParams(window.location.search);
    const raw = q.get("rows") || q.get("fixture");
    if (!raw) return null;
    const n = parseInt(raw, 10);
    if (isNaN(n) || n <= 0) return null;
    return Math.min(n, 100000);
  } catch {
    return null;
  }
}

export function perfHudWanted(): boolean {
  try {
    return new URLSearchParams(window.location.search).get("perf") === "1";
  } catch {
    return false;
  }
}

/** Dev-only `&autoscroll=1`: drive the viewport top-to-bottom on a loop so
 * scroll-frame costs are measurable without synthetic input. */
export function autoScrollWanted(): boolean {
  try {
    return new URLSearchParams(window.location.search).get("autoscroll") === "1";
  } catch {
    return false;
  }
}

/** Deterministic pseudo-random from an index (mulberry-ish, inline). */
function prand(i: number): number {
  let x = (i * 2654435761) >>> 0;
  x ^= x >>> 15;
  x = (x * 2246822519) >>> 0;
  x ^= x >>> 13;
  return (x >>> 0) / 4294967296;
}

const EXT = ["txt", "md", "png", "jpg", "rs", "json", "log", "pdf"];

export function makeSyntheticRows(n: number, base = "/tmp/kestrel-perf/10k"): Row[] {
  const rows: Row[] = new Array(n);
  for (let i = 0; i < n; i++) {
    const isDir = i % 97 === 96;
    const ext = EXT[i % EXT.length];
    const name =
      (isDir ? "dir-" : "file-") + zeroPad(i, 6) + (isDir ? "" : "." + ext);
    const entry: FileEntryDto = {
      name,
      path: base + "/" + name,
      kind: isDir ? "Directory" : "File",
      size: isDir ? null : Math.floor(prand(i) * 8 * 1048576),
      // EPOCH MILLIS, staggered one hour apart into the past.
      modified: 1759270000000 - i * 3600000,
      hidden: false,
      is_dir_target: null,
      descendable: isDir,
    };
    rows[i] = { kind: "entry", entry };
  }
  return rows;
}

function zeroPad(n: number, width: number): string {
  let s = String(n);
  while (s.length < width) s = "0" + s;
  return s;
}
