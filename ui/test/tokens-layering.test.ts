/* Token-layering check for `tokens.css`.
 *
 * The spec's layer rule is hard: §3 semantics reference §2 primitives, §4
 * components reference §3 semantics, and nothing reaches past its neighbour. A
 * component that names a hex is a defect. So is a semantic that names a hex
 * instead of a primitive — it severs the palette from the ramps underneath it,
 * and the whole point of the three-layer design is that swapping a ramp
 * re-themes the app.
 *
 * This test parses the token file as text (no CSS parser needed — the token
 * layer is a flat, regular set of declarations) and asserts the invariants that
 * a compile-time type would otherwise give the Rust side: `Theme` is a struct of
 * concrete fields precisely so a missing role is a compile error.
 *
 * Run: npm test
 */

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { strict as assert } from "node:assert";
import test from "node:test";

// Resolved from `process.cwd()` rather than `import.meta.url`: esbuild bundles
// this file to a path under /tmp, so `import.meta.url` would point at the build
// output's directory, not the source tree. Same convention as op-seam.test.ts,
// and it means the test is run from `ui/`, which is what `npm test` does.
const src = (file: string): string => readFileSync(resolve(process.cwd(), file), "utf8");
const css = src("src/tokens.css");
const styles = src("src/styles.css");

/* The §3 semantic roles, by group. Listed explicitly rather than inferred from
 * a prefix, because two groups collide on a prefix: `--kestrel-accent-50` is a
 * §2.2 *ramp step* and `--kestrel-accent-base` is a §3 *role*. Guessing by
 * prefix would classify the ramp as semantic and then flag every hex in it. */
const SEMANTIC_GROUPS: Record<string, string[]> = {
  surface: ["app", "panel", "chrome", "list", "raised", "input", "input-disabled", "scrim", "list-veil"],
  text: ["primary", "secondary", "tertiary", "disabled", "on-accent", "on-danger", "link", "inverse"],
  border: ["subtle", "default", "strong", "accent", "danger"],
  focus: ["ring", "ring-inactive", "ring-width", "ring-offset"],
  state: ["hover", "hover-strong", "pressed", "selected", "selected-hover",
          "selected-bar", "cut", "focus-within", "drop-target"],
  // `accent.base` … `accent.on` are §3.5 roles; `accent.50` … `accent.900` are
  // §2.2 ramp steps and are NOT in this list.
  accent: ["base", "hover", "pressed", "subtle-bg", "border", "text", "on"],
  status: ["danger-text", "danger-solid", "danger-hover", "danger-bg", "danger-border",
           "success-text", "success-solid", "success-bg", "success-border",
           "warning-text", "warning-solid", "warning-bg", "warning-border",
           "info-text", "info-bg"],
  icon: ["folder", "text", "code", "image", "video", "audio", "archive", "binary",
         "executable", "symlink", "hidden", "error", "chrome", "chrome-active"],
  scrollbar: ["track", "thumb", "thumb-hover", "thumb-active"],
};

/** Every §3 semantic token name, e.g. `--kestrel-surface-base`. */
const SEMANTIC_TOKENS = new Set<string>();
for (const [group, roles] of Object.entries(SEMANTIC_GROUPS)) {
  for (const role of roles) SEMANTIC_TOKENS.add(`--kestrel-${group}-${role}`);
}

/** Every `--kestrel-*` declaration in the file, with its raw value. */
function declarations(): Map<string, string[]> {
  const found = new Map<string, string[]>();
  const re = /(--kestrel-[a-z0-9-]+)\s*:\s*([^;]+);/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(css)) !== null) {
    const list = found.get(m[1]);
    if (list) list.push(m[2].trim());
    else found.set(m[1], [m[2].trim()]);
  }
  return found;
}

const DECLS = declarations();

/** The `var(--x)` tokens referenced by a declaration value. */
function refsTo(value: string): string[] {
  const out: string[] = [];
  const re = /var\(\s*(--kestrel-[a-z0-9-]+)/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(value)) !== null) out.push(m[1]);
  return out;
}

/** Strip comments so prose never counts as a value. */
function withoutComments(value: string): string {
  return value.replace(/\/\*[\s\S]*?\*\//g, "");
}

test("every referenced token is defined somewhere in the file", () => {
  const missing: string[] = [];
  for (const [name, values] of DECLS) {
    for (const value of values) {
      for (const ref of refsTo(withoutComments(value))) {
        if (!DECLS.has(ref)) missing.push(`${name} references undefined ${ref}`);
      }
    }
  }
  assert.deepEqual(missing, [], missing.join("\n"));
});

test("no token name is defined twice with conflicting values", () => {
  // The dark column intentionally redefines the whole §3 semantic set, and the
  // reduced-motion block redefines the durations. Both are allowed; a token
  // redefined in the SAME selector block is a copy-paste mistake.
  const byBlock = new Map<string, Map<string, string>>();
  const blockRe = /([^{}]+)\{([^{}]*)\}/g;
  let m: RegExpExecArray | null;
  while ((m = blockRe.exec(css)) !== null) {
    const body = m[2];
    const declRe = /(--kestrel-[a-z0-9-]+)\s*:\s*([^;]+);/g;
    let d: RegExpExecArray | null;
    while ((d = declRe.exec(body)) !== null) {
      const bucket = byBlock.get(d[1]) ?? new Map<string, string>();
      const value = d[2].trim();
      const prev = bucket.get(m[1].trim());
      if (prev !== undefined && prev !== value) {
        assert.fail(
          `${d[1]} has two different values in the same block ${m[1].trim()}:\n  ${prev}\n  ${value}`,
        );
      }
      bucket.set(m[1].trim(), value);
      byBlock.set(d[1], bucket);
    }
  }
});

test("§3 semantics reference §2 primitives only — never a raw hex", () => {
  // §3 semantic roles are the colours the components consume. Every one of them
  // must resolve to a primitive or to another semantic, and none may be a
  // literal colour: a raw hex here is what makes the palette un-rethemeable.
  //
  // Three groups are exempt, and the exemption is per-token, not per-group:
  //   * `--kestrel-focus-ring-width` / `-offset` are §3.3 roles that resolve to
  //     §2.7 border widths — already checked above, no hex.
  //   * `surface.scrim`, `focus.ring-inactive` and `state.cut` are the spec's
  //     three "colour @ N%" notations (§3.1, §3.3, §3.4). A CSS custom property
  //     holds one value and cannot compose an alpha over a hex the way the spec
  //     writes it, so each is a single `rgba()`. That is a faithful
  //     transcription of a composite, not a collapsed one — and each of them is
  //     asserted below to carry the exact alpha the spec specifies.
  const ALPHA_COMPOSITES = new Set([
    "--kestrel-surface-scrim",
    "--kestrel-focus-ring-inactive",
    "--kestrel-state-cut",
    // The loading veil is unspecified by the spec (§8 D-8 — the loading state has
    // no §4 block), so the raw composites are §2 primitives (`--kestrel-veil-*`)
    // and this is the §3 role wrapping them.
    "--kestrel-surface-list-veil",
  ]);
  const offenders: string[] = [];
  for (const [name, values] of DECLS) {
    if (!SEMANTIC_TOKENS.has(name)) continue;
    if (ALPHA_COMPOSITES.has(name)) continue;
    for (const value of values) {
      const bare = withoutComments(value);
      if (/#[0-9a-f]{3,8}\b/i.test(bare) || /\brgba?\(/.test(bare)) {
        offenders.push(`${name}: ${bare}`);
      }
    }
  }
  assert.deepEqual(offenders, [], offenders.join("\n"));
});

test("the three alpha composites carry the spec's exact alpha", () => {
  // §3.1 `surface.scrim` — #1C1A17 @ 38% light, #000000 @ 58% dark.
  // §3.3 `focus.ring-inactive` — accent @ 40% in both columns.
  // §3.4 `state.cut` — `state.selected` @ 55% over `surface.list`.
  assert.deepEqual(DECLS.get("--kestrel-surface-scrim"), [
    "rgba(28, 26, 23, 0.38)",
    "rgba(0, 0, 0, 0.58)",
    "rgba(0, 0, 0, 0.58)",
  ]);
  assert.deepEqual(DECLS.get("--kestrel-focus-ring-inactive"), [
    "rgba(14, 107, 95, 0.4)",   // accent.600 @ 40%
    "rgba(79, 199, 177, 0.4)",  // accent.400-dark @ 40%
    "rgba(79, 199, 177, 0.4)",
  ]);
  assert.deepEqual(DECLS.get("--kestrel-state-cut"), [
    "rgba(201, 230, 223, 0.55)", // accent.200 @ 55% over surface.list (#FCFBF9)
    "rgba(22, 51, 47, 0.55)",    // #16332F @ 55% over surface.list (#1E1C1A)
    "rgba(22, 51, 47, 0.55)",
  ]);
});

test("§4 components reference §3 semantics only — never a raw hex", () => {
  const offenders: string[] = [];
  for (const [name, values] of DECLS) {
    if (!/^--kestrel-(sidebar|row|breadcrumb|toolbar|statusbar|menu|dialog|input|viewport|impl)-/.test(name)) {
      continue;
    }
    for (const value of values) {
      const bare = withoutComments(value);
      if (/#[0-9a-f]{3,8}\b/i.test(bare)) {
        offenders.push(`${name}: ${bare}`);
      }
    }
  }
  assert.deepEqual(offenders, [], offenders.join("\n"));
});

test("a semantic token's transitive closure ends in primitives, with no cycle", () => {
  // Resolve every §3 token down to its terminal values and confirm (a) it
  // terminates and (b) every terminal is a raw value — a colour, a number, a
  // font stack, or a keyword. A cycle shows up as an unresolved name.

  const resolve = (name: string, seen: Set<string>): string[] => {
    if (seen.has(name)) return [`CYCLE:${name}`];
    const nextSeen = new Set(seen).add(name);
    const values = DECLS.get(name);
    if (!values) return [`UNDEFINED:${name}`];
    const out: string[] = [];
    for (const value of values) {
      const bare = withoutComments(value);
      const refs = refsTo(bare);
      if (refs.length === 0) out.push(bare);
      else for (const ref of refs) out.push(...resolve(ref, nextSeen));
    }
    return out;
  };

  // A terminal is acceptable if it is a raw colour, a bare number with an
  // optional unit, a font-family stack, a keyword, or a built-in function. The
  // `text-role-*-font` tokens terminate at a quoted family list, which is
  // correct: §3.9 maps a role to a §2.8 type token, and the family IS the token.
  const isTerminal = (v: string): boolean => {
    const s = v.trim();
    if (!s) return false;
    return (
      /^#[0-9a-f]{3,8}$/i.test(s) ||
      /^(?:rgba?|hsla?|cubic-bezier|var)\(/i.test(s) ||
      /^(?:transparent|none|inherit|initial|unset)$/i.test(s) ||
      /^-?[\d.]+(?:px|ms|em|rem|%|ch|vh|vw)?$/i.test(s) ||
      /^["'].*["'](?:\s*,\s*["'].*["'|,a-z-]+)*$/i.test(s) ||
      /^[a-z-]+$/i.test(s)
    );
  };

  const problems: string[] = [];
  for (const name of SEMANTIC_TOKENS) {
    if (!DECLS.has(name)) {
      problems.push(`UNDEFINED:${name}`);
      continue;
    }
    for (const terminal of resolve(name, new Set())) {
      if (terminal.startsWith("CYCLE:") || terminal.startsWith("UNDEFINED:")) {
        problems.push(`${name} -> ${terminal}`);
      } else if (!isTerminal(terminal)) {
        problems.push(`${name} terminates at an unexpected value: ${terminal}`);
      }
    }
  }
  assert.deepEqual(problems, [], problems.join("\n"));
});

test("every §3 role is defined in both the light and the dark column", () => {
  // §3's preamble: "Every role is defined for light and dark. A role with no
  // definition in a theme is a bug, not a fallback to a primitive." The dark
  // column must therefore redefine the WHOLE set, not just the few that differ
  // — a role that silently kept its light value is exactly that bug.
  const darkBlock = /@media \(prefers-color-scheme: dark\) \{[\s\S]*?\n\}/.exec(css);
  assert.ok(darkBlock, "no prefers-color-scheme: dark block found");

  const missing: string[] = [];
  for (const token of SEMANTIC_TOKENS) {
    if (!DECLS.has(token)) continue; // a role the file does not define at all
    if (!darkBlock[0].includes(token + ":")) missing.push(token);
  }
  assert.deepEqual(missing, [], `dark column is missing: ${missing.join(", ")}`);
});

test("every §3 role in the spec's own table is present", () => {
  // The roles above are the ones this transcription uses. Assert the count so a
  // future edit that drops a role fails loudly rather than quietly narrowing the
  // surface the other checks cover.
  // 75 = 9 surfaces + 8 text + 5 borders + 4 focus + 9 state + 7 accent
  //    + 14 status + 14 icon + 4 scrollbar
  assert.equal(SEMANTIC_TOKENS.size, 75);
  for (const token of SEMANTIC_TOKENS) {
    assert.ok(DECLS.has(token), `${token} is listed as a §3 role but not defined`);
  }
});

test("the accent is pine teal, and no token in the file is a default blue", () => {
  // `--kestrel-accent-base` is defined once per theme column: light `:root`, the
  // dark media query, and the `data-theme="dark"` block. Three is correct.
  assert.ok((DECLS.get("--kestrel-accent-base")?.length ?? 0) >= 2,
    "accent.base must be defined in both columns");
  const forbidden = [
    "#3b82f6", "#2563eb", "#0a84ff", "#4a90d9", "#0b5cc0", "#1a73e8", "#007aff",
  ];
  const hits: string[] = [];
  for (const [name, values] of DECLS) {
    for (const value of values) {
      const bare = withoutComments(value).toLowerCase();
      for (const bad of forbidden) {
        if (bare.includes(bad)) hits.push(`${name} contains ${bad}`);
      }
    }
  }
  assert.deepEqual(hits, [], hits.join("\n"));
});

test("the neutral ramp carries warm chroma, not a pure gray", () => {
  // Anti-goal 2. A pure-gray ramp is what makes an interface look like a
  // screenshot of a template. Every ramp step must have R > B.
  const ramp = [
    "25", "50", "100", "150", "200", "300", "400", "500",
    "600", "700", "800", "850", "900", "950", "1000",
  ];
  const offenders: string[] = [];
  for (const step of ramp) {
    const value = (DECLS.get(`--kestrel-neutral-${step}`) ?? [""])[0];
    const m = /^#([0-9a-f]{6})$/i.exec(value.trim());
    if (!m) { offenders.push(`neutral-${step}: not a 6-digit hex (${value})`); continue; }
    const n = parseInt(m[1], 16);
    const r = (n >> 16) & 0xff;
    const b = n & 0xff;
    if (r <= b) offenders.push(`neutral-${step} = #${m[1]} (R=${r} B=${b}) is not warm`);
  }
  assert.deepEqual(offenders, [], offenders.join("\n"));
});

test("the font stack is IBM Plex, not system-ui or Inter", () => {
  // Anti-goal 3. `system-ui` is refused specifically because it silently changes
  // meaning on every platform the app runs on.
  const sans = (DECLS.get("--kestrel-font-sans") ?? [""])[0];
  const mono = (DECLS.get("--kestrel-font-mono") ?? [""])[0];
  assert.match(sans, /IBM Plex Sans/);
  assert.match(mono, /IBM Plex Mono/);
  for (const stack of [sans, mono]) {
    assert.doesNotMatch(stack, /system-ui/, `refuses system-ui: ${stack}`);
    assert.doesNotMatch(stack, /\bInter\b/);
    assert.doesNotMatch(stack, /\bRoboto\b/);
  }
});

test("selection is instant and motion.base is the direct-manipulation ceiling", () => {
  // §2.11 rules 1 and 3. A fading selection reads as lag in a file list.
  assert.equal(DECLS.get("--kestrel-motion-instant")?.[0], "0ms");
  assert.equal(DECLS.get("--kestrel-motion-base")?.[0], "130ms");
  assert.equal(DECLS.get("--kestrel-motion-fast")?.[0], "90ms");
  const spinner = DECLS.get("--kestrel-motion-spinner")?.[0];
  assert.equal(spinner, "900ms");
});

test("the row height token matches the JS ROW_HEIGHT it is bound to", () => {
  // `lib/windower.ts` hard-codes `ROW_HEIGHT = 32` and derives both the spacer
  // height and every row's `translateY` from it. `--kestrel-row-height`
  // resolves through `metric.row-comfortable`. If the two ever drift, the row
  // window desynchronises from the scroll position — a real, visible bug that
  // no type checker would catch, because the coupling is across a CSS/TS seam.
  const windower = src("src/lib/windower.ts");
  const m = /export const ROW_HEIGHT = (\d+);/.exec(windower);
  assert.ok(m, "ROW_HEIGHT not found in lib/windower.ts");
  const jsHeight = Number(m[1]);
  const token = DECLS.get("--kestrel-row-height")?.[0] ?? "";
  const metric = DECLS.get("--kestrel-metric-row-comfortable")?.[0] ?? "";
  assert.match(token, /var\(--kestrel-metric-row-comfortable\)/,
    "--kestrel-row-height should resolve through metric.row-comfortable");
  assert.equal(metric, `${jsHeight}px`,
    `CSS row height (${metric}) does not match JS ROW_HEIGHT (${jsHeight})`);
});

test("elevation exists for all three levels plus the inner highlight", () => {
  // §2.10. Defined even though nothing is elevated yet: a token file that only
  // holds what today's UI uses stops being a spec and becomes a cache.
  for (const name of [
    "--kestrel-elev-0",
    "--kestrel-elev-1-light", "--kestrel-elev-1-dark",
    "--kestrel-elev-2-light", "--kestrel-elev-2-dark",
    "--kestrel-elev-3-light", "--kestrel-elev-3-dark",
    "--kestrel-elev-inner-highlight",
  ]) {
    assert.ok(DECLS.has(name), `missing ${name}`);
  }
});

test("reduced motion zeroes every duration except the spinner", () => {
  // §2.11 rule 5. A state the user never sees is not a state.
  const block = /@media \(prefers-reduced-motion: reduce\) \{([\s\S]*?)\n\}/.exec(css);
  assert.ok(block, "no prefers-reduced-motion block found");
  const body = block[1];
  for (const token of [
    "--kestrel-motion-fast", "--kestrel-motion-base", "--kestrel-motion-moderate",
    "--kestrel-motion-slow", "--kestrel-motion-deliberate",
  ]) {
    assert.match(body, new RegExp(`${token}: 0ms`), `${token} should be 0ms`);
  }
  assert.doesNotMatch(body, /--kestrel-motion-spinner: 0ms/,
    "the spinner must keep its duration; a frozen ring reads as a stuck UI");
});

test("no Safari-14.1+ features are used in either stylesheet", () => {
  // The renderer is WebKitGTK, which tracks Safari 13. These are the specific
  // features that would silently break rather than error.
  //
  // Comments are stripped FIRST, deliberately. The header of both files names
  // the forbidden features in prose, to record that they are not used — so
  // scanning raw text would flag the documentation of the constraint as a
  // violation of it. Only real CSS is scanned.
  const strip = (s: string): string => s.replace(/\/\*[\s\S]*?\*\//g, "");
  const both = strip(css) + "\n" + strip(styles);
  const forbidden: Array<[RegExp, string]> = [
    [/@property\b/, "@property (Safari 16.4)"],
    [/:has\(/, ":has() (Safari 15.4)"],
    [/@container\b/, "container queries (Safari 16)"],
    [/content-visibility\s*:/, "content-visibility (Safari 18)"],
    [/:focus-visible\b/, ":focus-visible (Safari 15.4)"],
    [/\bconic-gradient\(/, "conic-gradient (Safari 12.1+ but @property-dependent)"],
    [/\bfont-size-adjust\s*:/, "font-size-adjust (Safari 16.4)"],
    [/\binset\s*:\s*[-\d]/, "inset shorthand (Safari 14.1)"],
    [/\baspect-ratio\s*:/, "aspect-ratio (Safari 15)"],
    [/\bdvh\b|\bdvw\b/, "dynamic viewport units (Safari 15.4)"],
  ];
  const hits: string[] = [];
  for (const [re, what] of forbidden) {
    if (re.test(both)) hits.push(what);
  }
  assert.deepEqual(hits, [], hits.join("\n"));
});

test("no flex gap: every flex row spaces its children with margins", () => {
  // Flex `gap` needs Safari 14.1. The existing stylesheet established margins
  // and this must not regress.
  const styles = src("src/styles.css");
  const offenders: string[] = [];
  // Find every rule whose body mentions `gap:` and check whether it is a
  // grid/flex `gap` or the legitimately-named token.
  for (const m of styles.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    const body = m[2];
    if (/\bgap\s*:\s*(var\(--kestrel-[a-z-]+\)|[0-9])/.test(body)) {
      // Allowed only on non-flex containers. #state and #jobs are flex/grid? The
      // ones in this file that use gap use it for grid, not flex — flag flex.
      if (/display:\s*flex/.test(body) || /display:\s*inline-flex/.test(body)) {
        offenders.push(m[1].trim().replace(/\s+/g, " "));
      }
    }
  }
  assert.deepEqual(offenders, [], `flex gap used in: ${offenders.join(", ")}`);
});

test("styles.css contains no raw hex colours and no hard-coded pixel values", () => {
  // The brief: prove the layout-critical rules are fully tokenised.
  const styles = src("src/styles.css");
  // Strip comments and the @import line before inspecting values.
  const body = styles
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/@import[^;]+;/, "");

  const hexes = [...body.matchAll(/#[0-9a-f]{3,8}\b/gi)].map((m) => m[0]);
  assert.deepEqual(hexes, [], `raw hex colours in styles.css: ${hexes.join(", ")}`);

  const rawFns = [...body.matchAll(/\b(?:rgba?|hsla?)\(/g)].map((m) => m[0]);
  assert.deepEqual(rawFns, [], `raw colour functions in styles.css: ${rawFns.join(", ")}`);

  // Every remaining `px` occurrence, checked one at a time. Two are legitimate:
  //
  //   * `width: 1px` on `#spacer` — the intrinsic width that gives the scroll
  //     container something to scroll. Not a design value; its height is set
  //     from JS (`count * ROW_HEIGHT`), so the width is a layout necessity.
  //   * `calc(0px - <token>)` — a negated token for `outline-offset`. The `0px`
  //     is a unit-zero so the subtraction resolves to a length; the value being
  //     subtracted IS the token, so nothing is hard-coded.
  const pxValues = [...body.matchAll(/[\w-]+\s*:\s*([^;{}]*?\b\d+(?:\.\d+)?px\b[^;{}]*)/g)]
    .map((m) => m[0].trim());
  const isJustified = (v: string): boolean =>
    /^#?[\w-]*\s*width:\s*1px$/.test(v) ||
    /calc\(\s*0px\s*-\s*var\(--kestrel-[a-z-]+\)\s*\)$/.test(v);
  const unaccounted = pxValues.filter((v) => !isJustified(v));
  assert.deepEqual(unaccounted, [],
    `un-tokenised px values in styles.css:\n${unaccounted.join("\n")}`);
  assert.ok(
    pxValues.some((v) => /width:\s*1px/.test(v)),
    "the scroll spacer's intrinsic 1px should still be present",
  );
});

test("the implementation constants block is labelled as such", () => {
  // The web build had values §4 does not describe (§8 D-8 records the job panel,
  // the op panel and the perf HUD as unspecified). They are collected under an
  // explicit `impl-` prefix so each is visible as a decision to make rather
  // than a number nobody owns.
  const impls = [...DECLS.keys()].filter((n) => n.startsWith("--kestrel-impl-"));
  assert.ok(impls.length > 0, "no impl constants found");
  // Each must be either a reference to another layer or an explicitly-labelled
  // intrinsic size.
  const LITERAL_OK = /^(#[0-9a-f]{3,8}|rgba?\(|[\d.]+(px|%|ch)$|transparent$|none$)/i;
  for (const name of impls) {
    for (const value of DECLS.get(name) ?? []) {
      const bare = withoutComments(value);
      const hasRefs = refsTo(bare).length > 0;
      assert.ok(hasRefs || LITERAL_OK.test(bare),
        `${name} = ${bare} is neither a token reference nor an intrinsic size`);
    }
  }
});