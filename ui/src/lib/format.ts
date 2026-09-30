/* Kestrel Phase 1 — display formatting. No dependencies, safari13-safe. */

const SIZE_UNITS = ["B", "KB", "MB", "GB", "TB"];

/** null (directories / unknown) renders as an em dash, never "0 B". */
export function formatSize(bytes: number | null): string {
  if (bytes === null || bytes === undefined) return "—";
  if (bytes === 0) return "0 B";
  let v = bytes;
  let u = 0;
  while (v >= 1024 && u < SIZE_UNITS.length - 1) {
    v /= 1024;
    u++;
  }
  return (u === 0 ? String(Math.round(v)) : v.toFixed(1)) + " " + SIZE_UNITS[u];
}

/**
 * EPOCH MILLIS in, local date out. The DTO is millis — do NOT divide.
 * Falls back to "—" for null/invalid rather than rendering 1970.
 */
export function formatDate(millis: number | null): string {
  if (millis === null || millis === undefined) return "—";
  const d = new Date(millis);
  if (isNaN(d.getTime())) return "—";
  const y = d.getFullYear();
  const m = pad(d.getMonth() + 1);
  const day = pad(d.getDate());
  const h = pad(d.getHours());
  const min = pad(d.getMinutes());
  return y + "-" + m + "-" + day + " " + h + ":" + min;
}

function pad(n: number): string {
  return n < 10 ? "0" + n : String(n);
}

/** Single-letter kind glyph (text, no icon font in Phase 1).
 * Compares case-insensitively: entries arrive canonicalised, but a wire
 * rename must degrade to a glyph, never a crash. */
export function kindGlyph(kind: string, descendable: boolean): string {
  const k = (kind || "").toLowerCase();
  if (k === "directory" || descendable) return "D";
  if (k === "symlink") return "L";
  if (k === "other") return "?";
  return "F";
}
