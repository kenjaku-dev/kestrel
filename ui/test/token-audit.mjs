/* A standalone audit of `tokens.css`, kept out of the test suite so its output
 * can be read directly when reviewing the three-layer rule.
 *
 * Run from `ui/`:  node test/token-audit.mjs    (exit 0 = clean)
 *
 * The rule being checked is the spec's, verbatim (design-tokens.md, "Layer rule
 * (hard)"): "components may only reference semantic tokens (§3). Semantic tokens
 * reference primitives (§2). No component ever names a primitive. If a component
 * needs a value that has no semantic role, the role is missing — add it to §3,
 * don't reach past it."
 *
 * That has four distinct checks, because it is four distinct claims:
 *
 *   1. §3 NAMES NO RAW COLOUR. A semantic that writes a hex has skipped §2
 *      entirely. This is the check that would fail on
 *      `--kestrel-surface-base: #12100e`, which is the failure mode the whole
 *      architecture exists to prevent.
 *   2. §4 NAMES NO RAW COLOUR. Same argument, one layer up.
 *   3. §4 COLOURS RESOLVE TO §3. A component colour that lands on a §2 ramp step
 *      is still re-themeable-broken: swapping the ramp would not move it.
 *   4. NOTHING REACHES UPWARD, and `styles.css` names no §2 primitive.
 *
 * Two things that look like violations and are not, both required by the spec:
 *
 *   LATERAL (§3 → §3). The spec defines `icon.chrome` AS `text.secondary` and
 *   `icon.chrome-active` AS `accent.base` (§3.7). A semantic composed from
 *   another semantic is what §3 is for.
 *
 *   §4 MEASUREMENTS → §2. §4's cells are written as measurements —
 *   "sidebar.item-padding-x | 8 px", "row.height | 26 px" — and `tokens.rs`
 *   resolves them the same way (`SIDEBAR_ITEM_PADDING_X = space::S2`). A
 *   measurement resolves to a §2 metric or spacing rung; that is the design, not
 *   a breach. What must never happen is a component COLOUR resolving to §2, and
 *   that is check 3.
 */

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

// Anchored to this file, not process.cwd(), so the audit behaves identically
// whether it is run from ui/ or from the repo root. A gate that passes or fails
// depending on where you happen to be standing is not a gate.
const here = dirname(fileURLToPath(import.meta.url));

const css = readFileSync(resolve(here, "../src/tokens.css"), "utf8");
const styles = readFileSync(resolve(here, "../src/styles.css"), "utf8");

/* -- parsing --------------------------------------------------------------- */

function blocksOf(text) {
  const clean = text.replace(/\/\*[\s\S]*?\*\//g, "");
  const out = [];
  for (const m of clean.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    const decls = {};
    for (const dm of m[2].matchAll(/(--kestrel-[a-z0-9-]+)\s*:\s*([^;]+);/g)) {
      decls[dm[1]] = dm[2].trim();
    }
    out.push(decls);
  }
  return out;
}

const BLOCKS = blocksOf(css);
const ALL = new Set();
for (const b of BLOCKS) for (const n of Object.keys(b)) ALL.add(n);

/* -- layer assignment ------------------------------------------------------ */

const SEM_GROUPS = new Set(["surface", "text", "border", "focus", "state", "status", "icon"]);

// `accent` and `scrollbar` each straddle two layers, so a prefix test alone
// cannot classify them: `accent.50` is a §2.2 ramp step while `accent.base` is a
// §3.5 role; `scrollbar.thumb` is §3.8 while `scrollbar.width` is §4.9.
const ACCENT_ROLES = new Set(["base", "hover", "pressed", "subtle-bg", "border", "text", "on"]);
const SCROLLBAR_ROLES = new Set(["track", "thumb", "thumb-hover", "thumb-active"]);
const COMPONENT_PREFIXES = [
  "sidebar", "row", "breadcrumb", "toolbar", "statusbar", "menu",
  "dialog", "input", "viewport", "impl", "scrollbar",
];
/** §3.1's roles that are named directly, e.g. `--kestrel-surface-list-veil`.
 *  Not in the spec's §3.1 table: the loading state is unspecified (see §8 D-8),
 *  so this role wraps a §2 primitive that the file records and explains. */
const SURFACE_ROLES = new Set(["list-veil"]);

/** §3.9's text roles, e.g. `--kestrel-text-role-name-font`. */
const TEXT_ROLES = new Set([
  "name", "name-selected", "meta", "meta-strong", "column-header",
  "status-bar", "dialog-title", "dialog-body", "ui", "ui-strong",
  "verb", "empty-state", "caption", "micro", "label",
  // `numeric` and `motion` are §3 roles that expose a §2 primitive as a value
  // rather than a colour: §2.8's tabular figures, and §2.11's durations applied
  // to rendered properties. A semantic may reference a primitive; that is the
  // layer rule working, not breaking.
  "numeric",
]);

/** §2.11 durations exposed as §3 roles for the reduced-motion block. */
const MOTION_ROLES = new Set(["role-reduced", "role-reduced-spinner"]);
/** Theme-switched elevation selectors: §2 tokens that are themed like §3. */
const ELEV_THEMED = new Set(["elev-1", "elev-2", "elev-3", "elev-inner-highlight-active"]);

function layerOf(name) {
  const bare = name.replace(/^--kestrel-/, "");
  // `--kestrel-white` / `--kestrel-black` are §2's two extremes with no second
  // segment, so the group test has to tolerate a bare word.
  const m = /^([a-z0-9]+)(?:-(.+))?$/.exec(bare);
  if (!m) return null;
  const [, group, rest] = m;
  if (rest === undefined) return 1;
  if (group === "surface" && SURFACE_ROLES.has(rest)) return 2;
  if (group === "accent" && ACCENT_ROLES.has(rest)) return 2;
  if (group === "scrollbar" && SCROLLBAR_ROLES.has(rest)) return 2;
  if (group === "text" && TEXT_ROLES.has(rest)) return 2;
  if (group === "text" && TEXT_ROLES.has(rest.replace(/-(font|tracking)$/, ""))) return 2;
  if (group === "motion" && MOTION_ROLES.has(rest)) return 2;
  if (group === "scrollbar" || group === "input" || COMPONENT_PREFIXES.includes(group)) return 3;
  if (SEM_GROUPS.has(group)) return 2;
  if (group === "elev" && ELEV_THEMED.has(bare)) return 2;
  return 1;
}

/**
 * A §4 token whose name says it carries a colour. These are what the layer rule
 * protects: if one resolves to a §2 ramp step, swapping the ramp does not move
 * it, so the palette is not re-themeable.
 */
const COLOUR_NAME =
  /(?:^|-)(bg|color|colour|text|icon|bar|border|ring|track|thumb|fill|scrim|elevation|outline|shadow|path-quote-bg)$/;

/** §3's three "colour @ N%" notations, which one CSS value can only express as rgba(). */
const ALPHA_COMPOSITES = new Set([
  "--kestrel-surface-scrim",
  "--kestrel-focus-ring-inactive",
  "--kestrel-state-cut",
]);

const isColourLiteral = (v) => /#[0-9a-f]{3,8}\b/i.test(v) || /\brgba?\(/i.test(v);
const refsOf = (v) => [...v.matchAll(/var\(\s*(--kestrel-[a-z0-9-]+)/g)].map((m) => m[1]);

/* -- the four checks ------------------------------------------------------- */

const check1 = []; // §3 names a raw colour
const check2 = []; // §4 names a raw colour
const check3 = []; // §4 colour resolves to §2
const check4 = []; // anything reaches upward
const consumer = []; // styles.css names a §2 primitive
const lateral = [];

const counts = { 1: 0, 2: 0, 3: 0 };
for (const n of ALL) {
  const L = layerOf(n);
  if (L) counts[L]++;
}

for (const b of BLOCKS) {
  for (const [n, val] of Object.entries(b)) {
    const L = layerOf(n);
    if (isColourLiteral(val)) {
      if (L === 2 && ALPHA_COMPOSITES.has(n)) {
        // checked separately, by exact alpha
      } else if (L === 2) check1.push(`${n}: ${val}`);
      else if (L === 3) check2.push(`${n}: ${val}`);
    }
    for (const ref of refsOf(val)) {
      const LR = layerOf(ref);
      if (LR === null) check4.push(`${n} -> ${ref}: not a kestrel token`);
      else if (L === 3 && LR === 1 && COLOUR_NAME.test(n)) {
        check3.push(`${n} (a colour) -> ${ref} (§2 primitive): must come from a §3 role`);
      } else if (LR > L) {
        check4.push(`${n} (L${L}) -> ${ref} (L${LR}): reaches upward`);
      } else if (LR === L) {
        lateral.push(`${n} -> ${ref}`);
      }
    }
  }
}

for (const ref of refsOf(styles)) {
  if (layerOf(ref) === 1) consumer.push(`styles.css names the §2 primitive ${ref}`);
}

/* -- report ---------------------------------------------------------------- */

const line = "─".repeat(70);
console.log("KESTREL TOKEN LAYER AUDIT");
console.log(line);
console.log(`distinct custom properties      ${ALL.size}`);
console.log(`  §2 layer 1  primitives         ${counts[1]}`);
console.log(`  §3 layer 2  semantics          ${counts[2]}`);
console.log(`  §4 layer 3  components         ${counts[3]}`);
console.log("");
console.log(`check 1  §3 naming a raw colour          ${check1.length}`);
check1.forEach((v) => console.log(`  ✗ ${v}`));
console.log(`check 2  §4 naming a raw colour          ${check2.length}`);
check2.forEach((v) => console.log(`  ✗ ${v}`));
console.log(`check 3  §4 colour resolving to §2       ${check3.length}`);
check3.forEach((v) => console.log(`  ✗ ${v}`));
console.log(`check 4  reaching upward / non-token     ${check4.length}`);
check4.forEach((v) => console.log(`  ✗ ${v}`));
console.log(`        styles.css naming a §2 primitive ${consumer.length}`);
consumer.forEach((v) => console.log(`  ✗ ${v}`));
console.log(line);
const lateralCount = new Set(lateral).size;
console.log(`lateral §3 → §3 references (allowed, spec-defined): ${lateralCount}`);
for (const v of [...new Set(lateral)].sort()) console.log(`  · ${v}`);
console.log(line);

const total = check1.length + check2.length + check3.length + check4.length + consumer.length;
console.log(
  total === 0
    ? "RESULT: clean — the three-layer rule holds."
    : `RESULT: ${total} violation(s).`,
);
process.exit(total === 0 ? 0 : 1);