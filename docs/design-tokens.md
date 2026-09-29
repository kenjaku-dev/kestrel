> ## ⚑ How to read this document
>
> This file is the **single source of truth** for Kestrel's design system. Every colour,
> spacing value, type token and component contract in `kestrel/src/tokens.rs` and the
> widget modules is transcribed from it, and the transcription is meant to be
> mechanically checkable.
>
> * **The direction is LOCKED — "Kestrel" (§1.4).** Directions A, B and C are
>   *rationale only*. They are retained so the reasoning behind the choice stays
>   legible. **None of them are to be implemented.** Do not "explore" them.
> * **§7's 17 anti-goals are binding constraints on all future work**, not a style
>   guide or a list of preferences. A change that violates one of them is a defect in
>   the change, and the anti-goal is the thing that has to give — deliberately, in
>   writing, with a reason. Anti-goals 12 and 13 (motion, and layout-animating
>   properties) are also the two that a future contributor is most likely to break by
>   accident, so they are enforced in code rather than by good intentions.
> * **The contrast audit in §6 is machine-verified and the numbers are load-bearing.**
>   Every ratio in §6.1–§6.4 was computed with the WCAG 2.x relative-luminance
>   formula and matched to 2 decimal places, including the 13 documented failures in
>   §6.5. They are not estimates and they are not decoration: they are the reason
>   particular hexes are the hexes they are. If you change a colour, you have
>   invalidated whichever rows mention it — recompute them and write the method into
>   the row. A stale audit is worse than no audit, because it looks like evidence.
>   §6.5 records the 13 failures on purpose; a spec with no failures in it was not
>   actually measured.
> * **Where this document and the code disagree, this document is the intent and the
>   code is the defect** — with exactly one recorded exception, below. Fix the code,
>   not the spec, and say so in a commit.
>
> ### The one exception: `surface.raised` (dark)
>
> `surface.raised`'s dark value was **the code's** before it was the spec's. The spec
> originally said `#24221F`, which is not a rung in §2.1's neutral ramp; the code used
> `#2B2824`, which is `neutral.800`. `#2B2824` was adopted, the spec was corrected to
> match, and **§6.2 and §6.5 were re-verified against it** — one row re-opened as a
> result. The full ruling, the reasoning, and the recomputation are in **§8, decision
> D-1**, at the end of this document. §8 is the changelog: read it before you
> implement against any value, because it is the only place where the code is right
> and the text above is behind.
>
> **Provenance:** authored 2026-09 against `/tmp/opencode/kestrel-tokens.md`. That
> path is tmpfs and does not survive a reboot, which is the reason this copy exists.
> `docs/design-tokens.md` is now the durable artifact.

# Kestrel — Design Token Specification

> **Direction: LOCKED — "Kestrel" (§1.4)**, approved by the user. Directions A, B and C
> are retained above for rationale only and are not to be implemented. The 17 anti-goals
> in §7 are binding on all future UI work.

**Project:** Kestrel, a lightweight two-pane file manager (egui/eframe)
**Document type:** framework-agnostic design tokens + aesthetic direction
**Status:** reference spec. No code. Values are hex / px / ms / weights, mapped onto framework types at implementation time.

**How to read this:** §1 aesthetic directions · §2 primitives · §3 semantics · §4 components · §5 icon mapping · §6 accessibility audit · §7 anti-goals.

**Layer rule (hard):** components may only reference **semantic** tokens (§3). Semantic tokens reference **primitives** (§2). No component ever names a primitive. If a component needs a value that has no semantic role, the role is missing — add it to §3, don't reach past it.

---

## 1. Aesthetic directions

### 1.1 Direction A — "Paper Calm"

Calm, near-neutral desktop chrome in the tradition of macOS Finder: `#F5F5F7` window canvas, pure-white rows, `#ECECEE` sidebar, `#1D1D1F` text, a single system-blue accent (`#0A84FF`) used only for selection. 28px rows, 13px system sans, 8px corner radius, soft two-layer shadows, 12–16px padding throughout, 220px sidebar, no visible dividers — separation is done with background lightness alone.

- **Mood:** invisible, safe, familiar. The UI apologises for existing.
- **Optimises for:** glanceability, first-time users, screenshot-ability, zero learning cost, a feeling of polish without opinion.
- **Would annoy:** anyone with 5,000+ files in a directory (28px rows throw away ~25% of vertical space, turning a list into a scroll marathon); keyboard-first users, because airy UIs usually ship weak focus affordances; anyone who has actually used a power tool and finds the whole thing slow even when it is not.

### 1.2 Direction B — "Instrument Panel"

Dense, keyboard-first, dark-first. 20–22px rows, 11–12px type, 0–2px radius, 1px hairline rules between every column and section, monospaced right-aligned numeric columns with tabular figures, an always-visible 2px focus ring, and near-zero use of the accent — it appears only for focus, selection, and "this is on". Selection is a flat rectangular wash plus a 2px left bar, never a rounded pill. Surfaces are separated by 2–4% lightness steps, not by shadow.

- **Mood:** instrument, not application. Precise, cold, slightly severe.
- **Optimises for:** information density, long sessions, low-end hardware, mouse-free operation, rendering a 10,000-row list with no perceptible frame cost.
- **Would annoy:** anyone previewing images or wanting visual warmth; people with low vision, who are already pushed by 11px type; anyone who believes colour is wasted. Treating "monochrome = serious" throws away the fastest recognition channel a file manager has — file-type colour — in exchange for a look that reads as a spreadsheet.

### 1.3 Direction C — "Ember"

Warm and personal: cream paper surfaces (`#F5F0E1`), terracotta accent (`#C67B5C`), a humanist serif (Source Serif 4 / Fraunces) for folder and file names, 10px radius, long soft shadows, 32px rows, generous whitespace, quiet personality in empty states.

- **Mood:** a desk object. Tactile, unhurried, one's own.
- **Optimises for:** delight, browsing as pleasure, low-stress everyday use, making a personal tool feel personal.
- **Would annoy:** precision work. A serif fights long filenames and destroys column alignment; +6px per row is a real cost at scale; and warmth applied to 4,000 items turns a directory listing into something you scroll through rather than read.

### 1.4 Recommendation — "Kestrel"

**Thesis: the chrome is near-neutral, near-flat, and dense; all of the colour in the app is spent on information (file-type hue), and the single saturated thing on screen is always the thing you are acting on.**

This borrows B's density budget (26px rows, 13px base, 90ms hovers, hairline rules) and A's restraint (no decorative colour, no shadow on chrome, generous-but-not-slack chrome sizing), and rejects B's monochrome austerity. Three decisions make it its own thing rather than a compromise:

1. **The colour budget is file-type hues and nothing else.** Headers, toolbars, sidebars and buttons are neutral. Saturated colour appears only where it *means* something. This is what separates "designed" from "assembled", and it means the icon column becomes a scannable colour index that a monochrome design throws away for free.
2. **Selection is a flat tint plus a 2px accent left bar, at 3px radius** — not a rounded blue pill. Precise, aligned to the grid, and it survives at 26px row height.
3. **The neutral ramp is warm graphite, not gray.** Every neutral carries roughly 3–5% warm chroma. This is what keeps the app from reading as a terminal or a spreadsheet, and it costs nothing.

Density is the "lightweight" constraint made visible. 26px rows, 13px type, 90ms hovers, flat chrome, and motion capped at 130ms mean the window never appears to be waiting on you.

**Rolled into the final palette:** pure white (`#FFFFFF`) is used *only* for menus and dialogs, so overlays physically read as "lifted off the page" without needing heavy shadows.

---

## 2. Primitive layer

Raw values only. No meaning attached. Hex, px, ms, unitless weights.

### 2.1 Neutral ramp — "warm graphite"

Chroma rises toward the mid-tones and falls off at both ends, which is what keeps a warm ramp from looking muddy.

| Token | Hex | Note |
|---|---|---|
| `neutral.25` | `#FCFBF9` | brightest light surface |
| `neutral.50` | `#F7F5F2` | |
| `neutral.100` | `#EFECE7` | |
| `neutral.150` | `#E7E3DC` | |
| `neutral.200` | `#DED9D1` | |
| `neutral.300` | `#CFC9C0` | |
| `neutral.400` | `#A79F94` | |
| `neutral.500` | `#7C746A` | |
| `neutral.600` | `#5E574F` | |
| `neutral.700` | `#443F39` | |
| `neutral.800` | `#2B2824` | |
| `neutral.850` | `#211F1C` | |
| `neutral.900` | `#181715` | |
| `neutral.950` | `#100F0E` | |
| `neutral.1000` | `#0A0A09` | near-black, dark theme canvas base |

### 2.2 Accent ramp — "pine teal"

The only hue in the UI chrome. Blue is refused by policy (see §7).

| Token | Hex |
|---|---|
| `accent.50` | `#E8F5F1` |
| `accent.100` | `#DCF0EA` |
| `accent.200` | `#C9E6DF` |
| `accent.300` | `#A6D6CB` |
| `accent.400` | `#5FC0AC` |
| `accent.500` | `#33A894` |
| `accent.600` | `#0E6B5F` | base accent (light theme) |
| `accent.700` | `#0A5449` | |
| `accent.800` | `#073B33` | |
| `accent.900` | `#052B25` | |
| `accent.400.dark` | `#4FC7B1` | base accent (dark theme) |
| `accent.300.dark` | `#6BD9C4` | |

### 2.3 Status ramps

| Token | Hex | | Token | Hex |
|---|---|---|---|---|
| `danger.50` | `#FBE9E7` | | `danger.400` | `#E05A50` |
| `danger.100` | `#F7D5D1` | | `danger.500` | `#B3261E` |
| `danger.300` | `#D96A62` | | `danger.600` | `#8F1D17` |
| `danger.700` | `#6E1610` | | `danger.400.dark` | `#F2837C` |
| | | | `danger.600.dark` | `#5A1512` |
| `success.50` | `#E4F2E4` | | `success.400` | `#4E9A52` |
| `success.100` | `#CFE7D0` | | `success.500` | `#286A2A` |
| `success.300` | `#8CBF8F` | | `success.600` | `#1D4E1F` |
| `success.400.dark` | `#7FCE85` | | | |
| `warning.50` | `#FBF0D9` | | `warning.400` | `#C99A1E` |
| `warning.100` | `#F6E3B8` | | `warning.500` | `#7A5100` |
| `warning.300` | `#E3C470` | | `warning.600` | `#5E3F00` |
| `warning.400.dark` | `#E0B461` | | | |

**Note on hue collision:** the accent is teal (`hue ~172°`), success is green (`~130°`), warning amber (`~42°`), danger red (`~4°`). The three status hues are separated by ≥40° from each other and from the accent, and every status is *always* paired with a distinct glyph (`warning`, `check-circle`, `x-circle`). Colour is never the sole channel.

### 2.4 File-type hue slots

Desaturated to sit on warm graphite without shouting. Each is used as an icon fill, never as a background. All pass ≥3:1 against every surface in both themes (see §6.3).

| Slot | Light | Dark |
|---|---|---|
| `hue.folder` | `#9A6510` | `#D8A548` |
| `hue.text` | `#5E574F` | `#B8B2A9` |
| `hue.code` | `#5B3EA8` | `#A794F5` |
| `hue.image` | `#A6326E` | `#E88AB4` |
| `hue.video` | `#7A3ABF` | `#C199F2` |
| `hue.audio` | `#286A2A` | `#7FCE85` |
| `hue.archive` | `#9A4520` | `#E0A184` |
| `hue.binary` | `#5E574F` | `#B8B2A9` |
| `hue.executable` | `#1F5E8A` | `#6FB6E0` |
| `hue.symlink` | `#0E6B5F` | `#4FC7B1` |
| `hue.hidden` | `#6A635A` | `#948E85` |
| `hue.error` | `#B3261E` | `#F2837C` |

`hue.text` and `hue.binary` are intentionally identical: an unclassifiable binary and a plain document are both "no hue", and inventing a colour for "unknown" would make the colour column meaningless.

### 2.5 Spacing scale — 4px base with a 2px half-step

The half-step exists for the file list, where a 4px rhythm is too coarse. Chrome and list use *different* densities; that is intentional and is a spacing decision, not an inconsistency.

| Token | px |
|---|---|
| `space.0` | 0 |
| `space.half` | 2 |
| `space.1` | 4 |
| `space.1.5` | 6 |
| `space.2` | 8 |
| `space.2.5` | 10 |
| `space.3` | 12 |
| `space.4` | 16 |
| `space.5` | 20 |
| `space.6` | 24 |
| `space.8` | 32 |
| `space.10` | 40 |
| `space.12` | 48 |

### 2.6 Radii

| Token | px |
|---|---|
| `radius.none` | 0 |
| `radius.xs` | 2 |
| `radius.sm` | 3 |
| `radius.md` | 4 |
| `radius.lg` | 6 |
| `radius.xl` | 8 |
| `radius.2xl` | 12 |
| `radius.pill` | 999 |

### 2.7 Border widths

| Token | px | Use |
|---|---|---|
| `border.hairline` | 1 | all separators, input borders, menu outline |
| `border.thick` | 2 | focus ring, selection left bar |
| `border.marker` | 3 | active-pane indicator bar |
| `border.hidden-marker` | 4 | diameter of the hidden-file dot |

### 2.8 Typography

**Families**

| Token | Family | Rationale |
|---|---|---|
| `font.sans` | IBM Plex Sans (400, 500, 600) | One superfamily for the whole UI. It was drawn for technical products, has genuine character at 13px (flat-topped `a`, angled `l`, open-tailed `g`) without being decorative, and its metrics are stable enough to align a column of filenames. It is not Inter, not Roboto, not `system-ui`. |
| `font.mono` | IBM Plex Mono (400, 500) | Used **only for machine values** — sizes, dates, counts, permissions, and the status-bar path. Monospace for machine data (tabular alignment, digit comparison); proportional for human names (scannability, space efficiency). This split is a hard rule, not a preference. |

Fallback chain: `IBM Plex Sans` → `IBM Plex Sans` webfont-bundled → `DejaVu Sans` → `sans-serif`. Mono: `IBM Plex Mono` → `DejaVu Sans Mono` → `monospace`.

**Type scale** — sizes are absolute px. Base body is 13px: this is a desktop tool, not a web page, and 13px is the largest size that still fits ~34 file rows in a 900px-tall list without scrolling.

| Token | Family | Size px | Line px | Weight | Tracking |
|---|---|---|---|---|---|
| `type.micro` | sans | 10 | 14 | 500 | +0.04em |
| `type.caption` | sans | 11 | 15 | 400 | 0 |
| `type.label` | sans | 11 | 15 | 600 | +0.06em, uppercase |
| `type.meta` | mono | 12 | 16 | 400 | 0 |
| `type.meta-strong` | mono | 12 | 16 | 500 | 0 |
| `type.status` | sans | 12 | 16 | 400 | 0 |
| `type.ui` | sans | 13 | 18 | 400 | 0 |
| `type.ui-strong` | sans | 13 | 18 | 500 | 0 |
| `type.ui-heading` | sans | 13 | 18 | 600 | 0 |
| `type.name` | sans | 13 | 18 | 400 | 0 |
| `type.name-selected` | sans | 13 | 18 | 500 | 0 |
| `type.dialog-body` | sans | 13 | 20 | 400 | 0 |
| `type.dialog-title` | sans | 15 | 21 | 600 | 0 |
| `type.verb` | sans | 13 | 18 | 700 | +0.01em |
| `type.display` | sans | 22 | 28 | 600 | -0.01em |

Rules: monospace values use tabular figures; no `type.*` token is used above 22px; no weight below 400; no weight above 700; all list text is single-line and vertically centred in its row (never wrapped, never clamped mid-word — middle-truncate with ellipsis at 60% width if needed).

### 2.9 Layout metrics

| Token | px | Note |
|---|---|---|
| `metric.row` | 26 | default file list row height |
| `metric.row-compact` | 22 | density: compact |
| `metric.row-comfortable` | 32 | density: comfortable, icon 20px |
| `metric.column-header` | 24 | |
| `metric.toolbar` | 34 | |
| `metric.breadcrumb` | 28 | |
| `metric.status-bar` | 24 | |
| `metric.sidebar-item` | 26 | must equal `metric.row` |
| `metric.sidebar-width` | 200 | default; min 160, max 340 |
| `metric.icon` | 16 | list icons; never below 14 |
| `metric.icon-compact` | 14 | compact density only, Regular weight only |
| `metric.icon-chrome` | 18 | toolbar / sidebar |
| `metric.icon-lg` | 20 | comfortable density |
| `metric.target-min` | 24 | minimum interactive hit target (WCAG 2.2 target size, minimum) |
| `metric.scrollbar` | 12 | full hit area |
| `metric.scrollbar-thumb` | 6 | visible thumb |
| `metric.gutter` | 10 | leading gutter before the icon |
| `metric.gutter-marker` | 3 | leading gutter reserved for the hidden-file dot |
| `metric.column-name` | auto | flexible, min 160 |
| `metric.column-size` | 88 | right-aligned |
| `metric.column-kind` | 92 | |
| `metric.column-modified` | 140 | right-aligned |
| `metric.column-permissions` | 108 | mono, right-aligned |

### 2.10 Elevation

Shadows appear on **overlays only** — menus, dialogs, tooltips, and the drag ghost. Chrome is flat and separated by hairlines. This is deliberate: shadows on every surface is the single strongest "generic dashboard" tell.

| Token | Offset Y | Blur | Spread | Light colour | Dark colour |
|---|---|---|---|---|---|
| `elev.0` | — | — | — | none | none |
| `elev.1` (menu, tooltip) | 6 | 18 | −4 | `#1C1A17` @ 12% | `#000000` @ 55% |
| `elev.2` (dialog) | 16 | 40 | −12 | `#1C1A17` @ 22% | `#000000` @ 65% |
| `elev.3` (drag ghost) | 2 | 6 | 0 | `#1C1A17` @ 10% | `#000000` @ 45% |
| `elev.inner-highlight` | −1 | 0 | 0 | `#FFFFFF` @ 60% (1px top inner) | not used |

Elevation degrades gracefully: if shadows are unavailable in the host, menus and dialogs must still carry `border.strong` + the light-mode `elev.inner-highlight`, so the overlay boundary survives.

### 2.11 Motion

| Token | ms | Use |
|---|---|---|
| `motion.instant` | 0 | **selection state changes** (see rule below) |
| `motion.fast` | 90 | hover tint, focus ring, checkbox, toggle |
| `motion.base` | 130 | button press, row hover on the list, icon swap |
| `motion.moderate` | 180 | menu open, dialog enter, sidebar resize settle |
| `motion.slow` | 260 | scrim fade-in |
| `motion.deliberate` | 380 | directory content cross-fade on navigation |
| `motion.loop.spinner` | 900 | one rotation, linear, looping |

| Token | Curve |
|---|---|
| `ease.standard` | `cubic-bezier(0.2, 0, 0, 1)` |
| `ease.exit` | `cubic-bezier(0.4, 0, 1, 1)` |
| `ease.emphasis` | `cubic-bezier(0.05, 0.7, 0.1, 1)` |
| `ease.linear` | `linear` (indeterminate progress only) |

**Hard motion rules**

1. **Selection is instant (0 ms).** A fading selection reads as lag in a file list. Change it in one frame. This is the most important motion decision in the app.
2. Nothing animates layout. No `width`, `height`, `margin`, or `top/left` transitions. Position and size change instantly; only opacity and colour animate.
3. `motion.base` (130 ms) is the ceiling for any direct manipulation response. Nothing the pointer touches takes longer.
4. No bounce, no spring, no overshoot, no stagger, no parallax, no skeleton shimmer. This app reads a local filesystem; a shimmer implies a network wait that does not exist.
5. **Reduced motion:** every duration becomes 0 except the spinner, and every animated property renders its final state immediately. No intermediate keyframes are skipped-then-settled.

### 2.12 Layering order

| Token | Value | |
|---|---|---|
| `layer.chrome` | 0 | toolbar, breadcrumb, status bar, sidebar |
| `layer.list` | 10 | file rows, in-pane overlays |
| `layer.ghost` | 100 | drag ghost |
| `layer.menu` | 200 | context menus, dropdowns |
| `layer.scrim` | 300 | modal backdrop |
| `layer.dialog` | 310 | confirmation dialogs |
| `layer.tooltip` | 400 | |
| `layer.drag-cursor` | 500 | cursor-attached affordances |

Sticky chrome (toolbar, breadcrumb, column header, status bar) is a **layout sibling with a 1px border**, never an overlay. This guarantees WCAG 2.2 "Focus Not Obscured (Minimum)" is satisfied structurally rather than by scrolling offsets.

---

## 3. Semantic layer

Components consume only these. Every role is defined for **light** and **dark**. A role with no definition in a theme is a bug, not a fallback to a primitive.

### 3.1 Surfaces

| Role | Light | Dark | Used by |
|---|---|---|---|
| `surface.app` | `#F7F5F2` | `#131211` | window background, gutters, empty space |
| `surface.panel` | `#EFECE7` | `#191817` | sidebar background, breadcrumb bar, status bar |
| `surface.chrome` | `#F7F5F2` | `#131211` | toolbar background |
| `surface.list` | `#FCFBF9` | `#1E1C1A` | file list rows (default) |
| `surface.raised` | `#FFFFFF` | `#24221F` | menus, dialogs, popovers, tooltips |
| `surface.input` | `#FCFBF9` | `#1E1C1A` | search field, rename field |
| `surface.input-disabled` | `#EFECE7` | `#191817` | |
| `surface.scrim` | `#1C1A17` @ 38% | `#000000` @ 58% | modal backdrop |

### 3.2 Text

| Role | Light | Dark | Used by |
|---|---|---|---|
| `text.primary` | `#1C1A17` | `#F0EDE8` | file names, sidebar labels, dialog body |
| `text.secondary` | `#4A453F` | `#B8B2A9` | path segments, secondary labels, help text |
| `text.tertiary` | `#5E574F` | `#A6A099` | column headers, size/date, status bar labels |
| `text.disabled` | `#8C857C` | `#726D66` | disabled controls (WCAG-exempt; see §6.2) |
| `text.on-accent` | `#FFFFFF` | `#08221D` | primary button label, accent-filled chips |
| `text.on-danger` | `#FFFFFF` | `#F0EDE8` | destructive button label |
| `text.link` | `#0A5449` | `#6BD9C4` | paths and hyperlinks |
| `text.inverse` | `#F7F5F2` | `#131211` | text on a dark toolbelt / status strip |

### 3.3 Borders and focus

| Role | Light | Dark | Used by |
|---|---|---|---|
| `border.subtle` | `#DED9D1` | `#2C2A27` | column rules, inside-pane separators |
| `border.default` | `#CFC9C0` | `#3D3A35` | input borders, menu outlines, sidebar divider |
| `border.strong` | `#A79F94` | `#6F6961` | dialog outline, checkbox idle border, overlay fallback |
| `border.accent` | `#0E6B5F` | `#4FC7B1` | focused input border, selected left bar |
| `border.danger` | `#B3261E` | `#F2837C` | destructive input border, error outline |
| `focus.ring` | `#0E6B5F` | `#4FC7B1` | 2px keyboard focus ring |
| `focus.ring-inactive` | `#0E6B5F` @ 40% | `#4FC7B1` @ 40% | focus ring when the window is not focused |
| `focus.ring-width` | 2 px | 2 px | |
| `focus.ring-offset` | 1 px | 1 px | |

### 3.4 Interactive state backgrounds

| Role | Light | Dark |
|---|---|---|
| `state.hover` | `#F0EDE8` | `#24221F` |
| `state.hover-strong` | `#E7E3DC` | `#2C2A27` |
| `state.pressed` | `#E1DCD3` | `#131211` |
| `state.selected` | `#C9E6DF` | `#16332F` |
| `state.selected-hover` | `#BCDED5` | `#1A3A34` |
| `state.selected-bar` | `#0E6B5F` | `#4FC7B1` |
| `state.cut` | `#C9E6DF` @ 55% over `surface.list` | `#16332F` @ 55% over `surface.list` |
| `state.focus-within` | `#DCF0EA` | `#1A3A34` |
| `state.drop-target` | `#DCF0EA` | `#1A3A34` |

### 3.5 Accent

| Role | Light | Dark |
|---|---|---|
| `accent.base` | `#0E6B5F` | `#4FC7B1` |
| `accent.hover` | `#0A5449` | `#6BD9C4` |
| `accent.pressed` | `#073B33` | `#8FE6D4` |
| `accent.subtle-bg` | `#DCF0EA` | `#16332F` |
| `accent.border` | `#A6D6CB` | `#3A7A6E` |
| `accent.text` | `#0A5449` | `#6BD9C4` |
| `accent.on` | `#FFFFFF` | `#08221D` |

### 3.6 Status

| Role | Light | Dark |
|---|---|---|
| `status.danger-text` | `#B3261E` | `#F2837C` |
| `status.danger-solid` | `#8F1D17` | `#5A1512` |
| `status.danger-bg` | `#FBE9E7` | `#331614` |
| `status.danger-border` | `#F7D5D1` | `#5A1512` |
| `status.success-text` | `#286A2A` | `#7FCE85` |
| `status.success-solid` | `#286A2A` | `#1D4E1F` |
| `status.success-bg` | `#E4F2E4` | `#182B19` |
| `status.success-border` | `#CFE7D0` | `#2B4A2D` |
| `status.warning-text` | `#7A5100` | `#E0B461` |
| `status.warning-solid` | `#7A5100` | `#5E3F00` |
| `status.warning-bg` | `#FBF0D9` | `#332811` |
| `status.warning-border` | `#F6E3B8` | `#4A3A15` |
| `status.info-text` | `#0E6B5F` | `#4FC7B1` |
| `status.info-bg` | `#DCF0EA` | `#16332F` |

Status text is always accompanied by a glyph. A status colour never appears without `warning` / `check-circle` / `x-circle` / `info` next to it.

### 3.7 Icon roles

Mirrors §2.4. Components reference these, never `hue.*`.

| Role | Light | Dark |
|---|---|---|
| `icon.folder` | `#9A6510` | `#D8A548` |
| `icon.text` | `#5E574F` | `#B8B2A9` |
| `icon.code` | `#5B3EA8` | `#A794F5` |
| `icon.image` | `#A6326E` | `#E88AB4` |
| `icon.video` | `#7A3ABF` | `#C199F2` |
| `icon.audio` | `#286A2A` | `#7FCE85` |
| `icon.archive` | `#9A4520` | `#E0A184` |
| `icon.binary` | `#5E574F` | `#B8B2A9` |
| `icon.executable` | `#1F5E8A` | `#6FB6E0` |
| `icon.symlink` | `#0E6B5F` | `#4FC7B1` |
| `icon.hidden` | `#6A635A` | `#948E85` |
| `icon.error` | `#B3261E` | `#F2837C` |
| `icon.chrome` | `text.secondary` | `text.secondary` | toolbar, breadcrumb, status bar glyphs |
| `icon.chrome-active` | `accent.base` | `accent.base` | a toggled-on toolbar button |

### 3.8 Scrollbar

| Role | Light | Dark |
|---|---|---|
| `scrollbar.track` | `#EFECE7` (panel) / transparent (list) | `#191817` / transparent |
| `scrollbar.thumb` | `#7A7268` | `#78716A` |
| `scrollbar.thumb-hover` | `#68615A` | `#948D84` |
| `scrollbar.thumb-active` | `#443F39` | `#B8B2A9` |

Thumbs meet 3:1 against their track in both themes (§6.4). Track is transparent over the list, so the thumb must clear 3:1 against `surface.list` too.

### 3.9 Type roles

| Role | Token |
|---|---|
| `text.role.name` | `type.name` |
| `text.role.name-selected` | `type.name-selected` |
| `text.role.meta` | `type.meta` |
| `text.role.column-header` | `type.label` |
| `text.role.status-bar` | `type.status` |
| `text.role.dialog-title` | `type.dialog-title` |
| `text.role.dialog-body` | `type.dialog-body` |
| `text.role.verb` | `type.verb` |
| `text.role.empty-state` | `type.display` |
| `text.role.caption` | `type.caption` |

---

## 4. Component layer

One block per component. Every value resolves to a §3 semantic role.

### 4.1 Sidebar item

Covers: places, bookmarks, volumes, recent.

| Token | Value |
|---|---|
| `sidebar.item-height` | 26 px |
| `sidebar.item-padding-x` | 8 px |
| `sidebar.item-gap` | 10 px |
| `sidebar.item-radius` | 4 px |
| `sidebar.item-bg` | transparent |
| `sidebar.item-bg-hover` | `state.hover` |
| `sidebar.item-bg-active` | `accent.subtle-bg` |
| `sidebar.item-bg-disabled` | transparent |
| `sidebar.item-text` | `text.secondary` |
| `sidebar.item-text-active` | `text.primary` |
| `sidebar.item-text-disabled` | `text.disabled` |
| `sidebar.item-icon` | `icon.chrome`, 18 px |
| `sidebar.item-icon-active` | `accent.base` |
| `sidebar.item-active-bar` | 3 px, `state.selected-bar`, left, full height, radius 0 |
| `sidebar.item-section-label` | `type.label`, `text.tertiary`, padding-x 8 px, height 22 px |
| `sidebar.item-count` | `type.meta`, `text.tertiary`, right-aligned |
| `sidebar.divider` | 1 px `border.subtle`, 8 px vertical margin |
| `sidebar.resize-handle` | 4 px wide hit area, 1 px `border.strong` on hover |
| `sidebar.item-transition` | `motion.fast`, `ease.standard`, colour only |

States: `default` (transparent, `text.secondary`) · `hover` (`state.hover`) · `active-place` (`accent.subtle-bg` + 3px bar + `text.primary` + `accent.base` icon) · `focus-visible` (2px `focus.ring` inset, 1px offset) · `disabled` (`text.disabled`, no hover) · `drop-target` (`state.drop-target` + 2px `border.accent` dashed-equivalent outline).

The active place is the **only** sidebar item that uses the accent. If more than one item is ever accented, the rule has broken.

### 4.2 File list row

The densest and most important component. Height 26 px, single line, no wrapping.

**Geometry**

| Token | Value |
|---|---|
| `row.height` | 26 px (22 / 32 variants) |
| `row.padding-x` | 8 px |
| `row.gutter` | 10 px leading, of which 3 px is reserved for the hidden marker |
| `row.icon-size` | 16 px (14 / 20 variants) |
| `row.icon-gap` | 10 px |
| `row.column-gap` | 12 px |
| `row.radius` | 3 px |
| `row.divider` | 1 px `border.subtle`, inset 8 px from left and right |
| `row.indent-step` | 14 px per depth level (tree mode) |

**State matrix — this is the core of the component**

| State | Background | Left bar | Text | Icon | Ring |
|---|---|---|---|---|---|
| `default` | `surface.list` | — | `text.primary` | `icon.<type>` | — |
| `hover` | `state.hover` | — | `text.primary` | `icon.<type>` | — |
| `selected` | `state.selected` | 2 px `state.selected-bar` | `text.primary` | `icon.<type>` | — |
| `selected + hover` | `state.selected-hover` | 2 px `state.selected-bar` | `text.primary` | `icon.<type>` | — |
| `selected + focused` | `state.selected` | 2 px `state.selected-bar` | `text.primary` (`type.name-selected`) | `icon.<type>` | 2 px `focus.ring` inset |
| `focused, not selected` | `surface.list` | — | `text.primary` | `icon.<type>` | 2 px `focus.ring` inset, 1 px offset |
| `cut` (marked for cut) | `state.cut` | 2 px `state.selected-bar` at 55% | `text.primary` | `icon.<type>` at 70% | — |
| `drop target` | `state.drop-target` | 2 px `border.accent` | `text.primary` | — | 2 px `border.accent` inset |
| `window inactive + focused` | `surface.list` | — | `text.primary` | `icon.<type>` | 2 px `focus.ring-inactive` |
| `hidden file` | as row state | as row state | `text.tertiary` | `icon.hidden` + 4 px `dot` marker in the gutter | as row state |
| `error` (unreadable / broken link) | as row state | as row state | `text.primary` | `icon.error` + `warning` glyph | as row state |
| `disabled` (e.g. permission) | as row state | — | `text.disabled` | `icon.<type>` at 50% | — |

**Rules**

1. Selection changes at `motion.instant` (0 ms). Hover and focus animate at `motion.fast` (90 ms).
2. A row is never both selected and hovered-but-not-focused with a different background — the two are combined into one state so there is no ambiguous third rendering.
3. **The focused row is always visible and never obscured.** Chrome is a layout sibling, so a focused row at the top or bottom of the list scrolls fully into view with 1 px to spare.
4. Disabled text never appears on a selected row. If an item is both disabled and selected, it is unselected; disabled items are not selectable.
5. Rows are flat. No shadow, no radius above 3 px, no gradient.
6. Strikethrough is **not** used for "cut" — the 55% tint plus the 55% icon alpha is the cut marker, and the status bar states the count explicitly. Strikethrough on a filename reads as "deleted" and causes mistakes.

**Column tokens**

| Token | Value |
|---|---|
| `row.col-header` | `type.label`, `text.tertiary`, height 24 px, `state.hover` on hover, `state.pressed` while dragging |
| `row.col-header-sortable` | trailing 12 px sort glyph, `icon.chrome`; `caret-up` / `caret-down` / `caret-up-down` |
| `row.col-size` | `type.meta`, `text.tertiary`, right-aligned, tabular figures, width 88 px |
| `row.col-kind` | `type.caption`, `text.tertiary`, left, width 92 px |
| `row.col-modified` | `type.meta`, `text.tertiary`, right-aligned, width 140 px, absolute UTC shown on hover |
| `row.col-permissions` | `type.meta`, `text.secondary`, right-aligned, width 108 px, octal + symbolic |
| `row.col-hidden` | `text.tertiary` on the row; marker dot `icon.hidden`, 4 px, vertically centred in the 3 px gutter |

**Selection model:** single-select and multi-select share this matrix. In multi-select, `selected` and `selected + focused` are distinct — only the latter carries the ring. This is the difference between a file manager that feels precise and one that does not.

**Empty and loading states**

| Token | Value |
|---|---|
| `row.empty-title` | `type.display`, `text.primary` |
| `row.empty-body` | `type.dialog-body`, `text.secondary`, max 44ch |
| `row.empty-icon` | `folder-open`, 48 px, `icon.chrome` at 40% |
| `row.skeleton` | `state.hover` bar, 12 px tall, radius 2 px, **no shimmer** (see §2.11 rule 4) |
| `row.skeleton-count` | 8 rows, then stop |
| `row.reading` | single 900 ms linear rotating `circle-notch`, 18 px, `icon.chrome` |

The empty state is a real design moment, not an afterthought: a 48px `folder-open` at 40% opacity, the folder name, and one sentence. No illustration, no illustration-adjacent illustration.

### 4.3 Breadcrumb segment

| Token | Value |
|---|---|
| `breadcrumb.height` | 28 px |
| `breadcrumb.padding-x` | 8 px |
| `breadcrumb.segment-gap` | 2 px (padding), 6 px (visual) |
| `breadcrumb.segment-height` | 20 px |
| `breadcrumb.segment-radius` | 3 px |
| `breadcrumb.segment-text` | `type.ui`, `text.secondary` |
| `breadcrumb.segment-text-current` | `type.ui-strong`, `text.primary` |
| `breadcrumb.segment-text-ellipsis` | `text.tertiary` |
| `breadcrumb.segment-bg-hover` | `state.hover` |
| `breadcrumb.segment-bg-active` | `state.pressed` |
| `breadcrumb.segment-ring` | 2 px `focus.ring` inset |
| `breadcrumb.separator` | `caret-right`, 12 px, `icon.chrome` at 55% |
| `breadcrumb.separator-clickable` | hit area 16 × 20 px |
| `breadcrumb.overflow` | leading `dots-three-horizontal` button, 20 × 20 px, opens a menu of collapsed ancestors |
| `breadcrumb.background` | `surface.panel` |
| `breadcrumb.bottom-border` | 1 px `border.subtle` |
| `breadcrumb.drop-indicator` | 2 px `border.accent` inset at the segment, top and bottom only |

The final segment is the current directory, rendered at 500 weight, and is **not** clickable — clicking it does nothing and it must not show a hover state. Middle segments are drop targets. Overflow (`/home/…/src/…/kestrel`) collapses from the left; the last two segments always stay visible, because the user's actual question is always "where am I" and "what's in here".

### 4.4 Toolbar button

Two variants: **icon button** (default) and **icon + label** (view switcher, sort, filter).

| Token | Icon button | Icon + label |
|---|---|---|
| `toolbar.btn-height` | 28 px | 28 px |
| `toolbar.btn-width` | 28 px | auto, padding-x 8 px |
| `toolbar.btn-gap` | — | 6 px |
| `toolbar.btn-radius` | 4 px | 4 px |
| `toolbar.btn-icon-size` | 18 px | 18 px |
| `toolbar.btn-label` | — | `type.ui`, `text.secondary` |
| `toolbar.btn-label-active` | — | `type.ui-strong`, `text.primary` |
| `toolbar.btn-bg` | transparent | transparent |
| `toolbar.btn-bg-hover` | `state.hover` | `state.hover` |
| `toolbar.btn-bg-pressed` | `state.pressed` | `state.pressed` |
| `toolbar.btn-icon` | `icon.chrome` | `icon.chrome` |
| `toolbar.btn-icon-active` | `accent.base` | `accent.base` |
| `toolbar.btn-ring` | 2 px `focus.ring` inset, 1 px offset | same |
| `toolbar.btn-disabled` | `icon.chrome` at 40% | `text.disabled` + 40% icon |
| `toolbar.btn-toggled-on-bg` | `accent.subtle-bg` | `accent.subtle-bg` |
| `toolbar.btn-toggled-on-icon` | `accent.base`, **Fill weight** | `accent.base`, Fill weight |
| `toolbar.btn-separator` | 1 px `border.subtle`, 16 px tall, 8 px margins | |
| `toolbar.btn-tooltip-delay` | 500 ms | 500 ms |
| `toolbar.transition` | `motion.fast`, `ease.standard`, colour only | |

**Rules**

- Every icon-only button has a tooltip (name + the keyboard shortcut) and an accessible name. No icon-only button ships without both.
- Toggled-on state uses a **Fill-weight glyph** on `accent.subtle-bg`. This is the only place Fill weight is used in the toolbar, which is what makes "on" readable in peripheral vision.
- `btn-height` 28 px against a 34 px toolbar gives 3 px above and below — the visual grouping of adjacent buttons is a 6 px gap, which is larger than the 3 px to the toolbar edge. Proximity does the grouping; no separators are needed except between functional groups.
- The primary action (New Folder) is the one filled button: `accent.base` bg, `text.on-accent`, `type.ui-strong`.

### 4.5 Status bar

| Token | Value |
|---|---|
| `statusbar.height` | 24 px |
| `statusbar.background` | `surface.panel` |
| `statusbar.top-border` | 1 px `border.subtle` |
| `statusbar.padding-x` | 10 px |
| `statusbar.section-gap` | 14 px |
| `statusbar.section-divider` | 1 px `border.subtle`, 12 px tall |
| `statusbar.label` | `type.status`, `text.tertiary` |
| `statusbar.value` | `type.meta`, `text.secondary` |
| `statusbar.value-strong` | `type.meta-strong`, `text.primary` |
| `statusbar.path` | `type.meta`, `text.secondary`, left, middle-truncate, flex |
| `statusbar.path-focus` | 2 px `focus.ring` inset, click-to-edit, `surface.input` while editing |
| `statusbar.selection-count` | `type.meta-strong`, `accent.text` |
| `statusbar.selection-count-idle` | `text.tertiary` |
| `statusbar.busy` | 900 ms linear `circle-notch`, 12 px, `icon.chrome` + `text.secondary` label |
| `statusbar.error` | `warning` glyph 12 px, `status.danger-text`, `type.status` |
| `statusbar.success-flash` | `check-circle` 12 px, `status.success-text`, 2000 ms hold, then fade at `motion.slow` |
| `statusbar.hidden-toggle` | 14 px hit target, `eye` / `eye-slash` |

**Sections, left to right:** selection count (or item count) · path · spacer · free-space indicator (a 60 px 4 px-tall meter, `state.hover` track, `accent.base` fill, `text.tertiary` label) · active-operation status. The status bar is a **layout sibling** with a border, never an overlay, so it can never obscure a focused row.

### 4.6 Context menu

| Token | Value |
|---|---|
| `menu.min-width` | 180 px |
| `menu.max-width` | 320 px |
| `menu.padding` | 4 px |
| `menu.radius` | 6 px |
| `menu.background` | `surface.raised` |
| `menu.border` | 1 px `border.strong` |
| `menu.elevation` | `elev.1` + `elev.inner-highlight` (light theme only) |
| `menu.offset-from-cursor` | 2 px, flips at viewport edge |
| `menu.item-height` | 26 px |
| `menu.item-padding-x` | 8 px |
| `menu.item-radius` | 4 px |
| `menu.item-gap` | 12 px (label → shortcut) |
| `menu.item-text` | `type.ui`, `text.primary` |
| `menu.item-icon` | 16 px, `icon.chrome` |
| `menu.item-shortcut` | `type.meta`, `text.tertiary`, right-aligned |
| `menu.item-bg-hover` | `state.hover` |
| `menu.item-bg-focus` | `accent.subtle-bg`, `text.primary` |
| `menu.item-bg-disabled` | transparent |
| `menu.item-text-disabled` | `text.disabled`, icon at 40% |
| `menu.item-destructive` | `status.danger-text` text + icon; hover bg stays `state.hover`, never a red wash |
| `menu.item-ring` | 2 px `focus.ring` inset |
| `menu.separator` | 1 px `border.subtle`, 5 px vertical margin, 8 px horizontal inset |
| `menu.section-label` | `type.label`, `text.tertiary`, 22 px |
| `menu.checkmark-slot` | 16 px fixed, keeps labels aligned when some items are checkable |
| `menu.enter` | `motion.moderate`, `ease.emphasis`, 2 px offset + opacity, 120 ms |
| `menu.exit` | `motion.fast`, `ease.exit`, opacity only, 90 ms |
| `menu.submenu-delay` | 250 ms |

Destructive items (`Delete Permanently`, `Empty Trash`) are red **text and icon only**. No red-filled menu rows — a red block in a menu is the loudest possible signal and it fires on every right-click, not on the click that matters.

### 4.7 Confirmation dialog

Used for irreversible actions only (delete, overwrite, empty trash, quit with unsaved work). Nothing else gets a dialog.

| Token | Value |
|---|---|
| `dialog.width` | 400 px |
| `dialog.padding` | 20 px |
| `dialog.radius` | 8 px |
| `dialog.background` | `surface.raised` |
| `dialog.border` | 1 px `border.strong` |
| `dialog.elevation` | `elev.2` + `elev.inner-highlight` |
| `dialog.scrim` | `surface.scrim` |
| `dialog.scrim-enter` | `motion.slow`, `ease.standard`, opacity only |
| `dialog.enter` | `motion.moderate`, `ease.emphasis`, 4 px rise + opacity, 140 ms |
| `dialog.exit` | `motion.fast`, `ease.exit`, opacity only, 90 ms |
| `dialog.title` | `type.dialog-title`, `text.primary` |
| `dialog.body` | `type.dialog-body`, `text.secondary` |
| `dialog.body-strong` | `type.dialog-body`, `text.primary` |
| `dialog.path-quote` | `type.meta`, `text.secondary`, `state.hover` background, radius 4 px, padding 6/8 px, middle-truncate |
| `dialog.icon` | 20 px, `icon.chrome`; `warning` glyph in `status.danger-text` for destructive |
| `dialog.footer-gap` | 12 px |
| `dialog.footer-align` | right |
| `dialog.btn-height` | 30 px |
| `dialog.btn-padding-x` | 14 px |
| `dialog.btn-radius` | 4 px |
| `dialog.btn-label` | `type.ui`, 500 |
| `dialog.btn-cancel-bg` | `surface.input` |
| `dialog.btn-cancel-border` | 1 px `border.strong` |
| `dialog.btn-cancel-text` | `text.primary` |
| `dialog.btn-cancel-bg-hover` | `state.hover-strong` |
| `dialog.btn-confirm-bg` | `accent.base` |
| `dialog.btn-confirm-bg-hover` | `accent.hover` |
| `dialog.btn-confirm-text` | `text.on-accent` |
| `dialog.btn-destructive-bg` | `status.danger-solid` |
| `dialog.btn-destructive-bg-hover` | `danger.600` / `danger.500` |
| `dialog.btn-destructive-text` | `text.on-danger` |
| `dialog.btn-focus-ring` | 2 px `focus.ring`, 2 px offset |
| `dialog.checkbox` | 14 px box, 2 px inset, 3 px radius, `border.strong` idle, `accent.base` checked with `check` glyph at 12 px |

**Copy rules for the destructive button:** the verb is specific and irreversible-sounding — `Move to Trash`, `Delete Permanently`, `Empty Trash` — never `OK`, never `Yes`. `Cancel` is always the leftmost button and keeps focus on open, so the safe action is where the keyboard already is. The `default` action is never the destructive one for Enter, unless the destructive action is reversible (`Move to Trash` is Enter-default; `Delete Permanently` is not).

### 4.8 Text input / search field

| Token | Value |
|---|---|
| `input.height` | 28 px |
| `input.radius` | 4 px |
| `input.background` | `surface.input` |
| `input.border` | 1 px `border.default` |
| `input.border-hover` | 1 px `border.strong` |
| `input.border-focus` | 1 px `border.accent` |
| `input.ring` | 2 px `focus.ring` at 1 px offset |
| `input.ring-danger` | 2 px `border.danger` at 1 px offset |
| `input.text` | `type.ui`, `text.primary` |
| `input.placeholder` | `type.ui`, `text.tertiary` |
| `input.caret` | 1 px, `text.primary`, 100% blink at 530 ms on / 530 ms off |
| `input.selection` | `accent.subtle-bg` behind, `text.primary` in front |
| `input.padding-x` | 8 px |
| `input.icon-size` | 16 px, `icon.chrome` |
| `input.icon-gap` | 6 px |
| `input.clear-btn` | 20 × 20 px, `x` at 12 px, `state.hover` on hover, 40% `icon.chrome` when idle |
| `input.disabled-bg` | `surface.input-disabled` |
| `input.disabled-text` | `text.disabled` |
| `input.invalid-border` | 1 px `border.danger` |
| `input.helper` | `type.caption`, `text.tertiary`, 4 px below |
| `input.helper-danger` | `type.caption`, `status.danger-text` |
| `input.transition` | `motion.fast`, `ease.standard`, border colour and ring only |

**Search field variant:** `height` 28 px, leading `magnifying-glass` 16 px, placeholder `Filter…` (not "Search" — the field filters a list, it does not search the web), trailing result count as `type.meta` / `text.tertiary`, trailing `x` clear button that appears only when the query is non-empty. `/` from anywhere focuses it. `Escape` clears the query and returns focus to the list. A live match count (`142 of 3,208`) sits to the right of the field, never inside it, so the text never reflows as you type.

Type-ahead in the file list (typing jumps to a matching filename) is a **separate, invisible** feature that shares no visual with the search field. The search field is an explicit, escapable, labelled control; type-ahead is a mode. Conflating them is how file managers end up with an invisible focus trap.

### 4.9 Scrollbar

| Token | Value |
|---|---|
| `scrollbar.width` | 12 px (full hit area) |
| `scrollbar.thumb-size` | 6 px (visible) |
| `scrollbar.thumb-radius` | 3 px |
| `scrollbar.track` | `scrollbar.track` role; transparent over the file list |
| `scrollbar.thumb` | `scrollbar.thumb` role |
| `scrollbar.thumb-hover` | `scrollbar.thumb-hover` role |
| `scrollbar.thumb-active` | `scrollbar.thumb-active` role (while dragging) |
| `scrollbar.inset` | 3 px from the pane edge |
| `scrollbar.min-thumb` | 24 px, so a thumb is never ungrabbable |
| `scrollbar.track-fade-in` | `motion.fast` on hover over the pane, 120 ms |
| `scrollbar.track-fade-out` | `motion.moderate`, 180 ms, 400 ms delay |
| `scrollbar.keyboard` | 3 lines per arrow press, 1 page per PageUp/PageDown, `motion.instant` — no smooth scroll animation |
| `scrollbar.overlay` | the thumb floats over the list; the list is never inset to make room |

Scrollbars **fade in on hover and fade out when idle**, because a permanent 12px gutter costs horizontal space in a name column that already competes with four other columns. While visible, the thumb meets 3:1 against the surface beneath it in both themes (§6.4). The minimum thumb is 24 px so it can always be grabbed — a thumb that shrinks to 6 px in a 4,000-row directory is a pointer-target failure regardless of its contrast.

### 4.10 State coverage matrix

Every interactive component, every state, both themes. Nothing in this table may be implemented as "it'll probably look fine".

| | default | hover | pressed | focus-visible | selected / on | disabled |
|---|---|---|---|---|---|---|
| **Sidebar item** | transparent, `text.secondary` | `state.hover` | `state.pressed` | 2px `focus.ring` inset | `accent.subtle-bg` + 3px bar | `text.disabled`, no hover |
| **File row** | `surface.list` | `state.hover` | `state.pressed` | 2px `focus.ring` inset | `state.selected` + 2px bar | `text.disabled`, icon 50% |
| **Breadcrumb segment** | `surface.panel`, `text.secondary` | `state.hover` | `state.pressed` | 2px `focus.ring` inset | current dir: 500 weight, no hover | — |
| **Toolbar button** | transparent, `icon.chrome` | `state.hover` | `state.pressed` | 2px `focus.ring` inset | `accent.subtle-bg` + Fill glyph | 40% icon, no hover |
| **Menu item** | `surface.raised` | `state.hover` | `state.hover` | `accent.subtle-bg` | `check` glyph in `accent.base` | `text.disabled`, no hover |
| **Dialog button** | per variant | per variant | `state.pressed` | 2px `focus.ring`, 2px offset | — | 40% label, no hover |
| **Text input** | `border.default` | `border.strong` | — | `border.accent` + 2px ring | text selected: `accent.subtle-bg` | `surface.input-disabled` |
| **Scrollbar thumb** | 6px `scrollbar.thumb` | `scrollbar.thumb-hover` | `scrollbar.thumb-active` | = hover | — | — |
| **Checkbox** | 14px `border.strong` | `border.accent` | `accent.pressed` | 2px `focus.ring` | `accent.base` + `check` | `border.subtle`, no fill |

**State priority when several apply** (highest first): `disabled` → `drop-target` → `pressed` → `selected` → `focus-visible` → `hover` → `default`. Exception: `selected + focus-visible` renders both, because that combination is the single most important state in the whole app.

### 4.11 Interaction contracts

States are only half the design. These are the gestures each state responds to.

| Action | Binding |
|---|---|
| Navigate into folder | `Enter`, double-click |
| Go up | `Alt+Up`, `Backspace` |
| Back / forward | `Alt+Left`, `Alt+Right` |
| Rename | `F2` (inline edit in the row) |
| Delete | `Delete` (to trash) |
| Permanent delete | `Shift+Delete` (confirm dialog) |
| Context menu | `Shift+F10`, right-click, `Menu` key |
| Select all | `Ctrl+A` |
| Copy / cut / paste | `Ctrl+C`, `Ctrl+X`, `Ctrl+V` |
| New folder | `Ctrl+Shift+N` |
| Refresh | `F5` |
| Search field | `Ctrl+F`, or `/` |
| Quick look | `Space` |
| View mode | `Ctrl+1` list, `Ctrl+2` grid, `Ctrl+3` details |
| Cycle panes | `Tab` / `Shift+Tab` |
| Type-ahead | any unhandled character |
| Escape | clear search → close menu → cancel rename, in that order |

Every one of these appears in its tooltip as text, in the form `Rename · F2`. The tooltip is the only place in the app where the shortcut is discoverable, which is why no icon-only button ships without one.

---

## 5. File-type icon mapping

### 5.1 Icon set: **Phosphor Icons**

One set, no mixing. Rationale:

- **It is drawn on a 16px grid.** This is the whole argument. A 24px-grid set with a 2px stroke (Lucide, Tabler, Feather) scaled down to a 16px cell becomes a 1.33px stroke — blurred and uneven at 1× DPI, and unintentionally fat at 2× DPI. Phosphor's Regular weight is exactly 1px on its native grid, so at 16px every stroke lands on an integer device pixel. A file list is a wall of 16px icons; stroke integrity is the whole game.
- **Weight twins exist for nearly every glyph.** `folder` and `folder-open`, `caret-right` and `caret-down`, `eye` and `eye-slash` are the *same drawing* at two weights, not two unrelated glyphs. This is what lets the toolbar signal "on" with a Fill glyph without introducing a second icon style.
- **It has a real `file-*` family.** `file-text`, `file-code`, `file-image`, `file-video`, `file-audio`, `file-zip`, `file-js`, `file-css` and ~40 more share one silhouette, so a directory of mixed files reads as one family rather than a ransom note.
- **It is licence-clean** (MIT) and ships as a single small SVG/PNG atlas, which matters for a "lightweight" binary.

**Outline, not filled, in the file list.** Filled glyphs at 16px become dark blobs and destroy the scan. Fill weight appears in exactly two places, both documented above: a toggled-on toolbar button, and the active place in the sidebar. That is the entire Fill budget.

**Stroke weight rules**

| Context | Size | Weight | Effective stroke |
|---|---|---|---|
| File list, default density | 16 px | Regular | 1.0 px |
| File list, compact density | 14 px | Regular | 0.875 px — **round up to 1.0 px, never render thinner** |
| File list, comfortable density | 20 px | Regular | 1.25 px |
| Toolbar / sidebar | 18–20 px | Regular, Bold when active | 1.125–1.5 px |
| Toggled-on state | 18 px | **Fill** | solid |
| Menu item | 16 px | Regular | 1.0 px |

**At 16px specifically:** no glyph in the set uses more than 11px of the 16px box, so there is always ≥2px of clear space between adjacent icons; cap and joint rounding stays at 1px or greater; and the minimum clear gap between two neighbouring strokes within one glyph is 2px. Icons are never rendered below 14px, and never below 16px in comfortable density.

**Colour is a redundant second channel, never the only one.** Every category below has a distinct *silhouette*. Colour reinforces; it does not encode. This is both a colourblind requirement and the reason the colour column can be muted without becoming ambiguous.

*Verification note: exact glyph names should be confirmed against the Phosphor release pinned at implementation time; where a name is uncertain, the fallback given below is functionally equivalent.*

### 5.2 Category mapping

| Category | Icon | Light | Dark | Detected by | Notes |
|---|---|---|---|---|---|
| **Folder** | `folder` | `icon.folder` | `icon.folder` | is a directory | Closed. |
| Folder, contents shown | `folder-open` | `icon.folder` | `icon.folder` | tree row expanded, or hovered while expandable | Same drawing as `folder`, different weight — this is why Phosphor was chosen. |
| **Text / document** | `file-text` | `icon.text` | `icon.text` | `.txt .md .pdf .rtf .doc .docx .odt .tex` | `icon.text` is neutral, deliberately. Documents are the majority of a directory and must not compete with code for attention. |
| **Code** | `file-code` | `icon.code` | `icon.code` | by extension → language map | Where the language is known, use the specific glyph: `file-js`, `file-ts`, `file-css`, `file-rs`, `file-py`, `file-go`, `file-sh`, `file-toml`, `file-html`, `file-json`, `file-yaml`, `file-sql`, `file-md`. Fall back to `file-code` for unknown languages. `file-json` and `file-toml` stay neutral (`icon.text`) — they are data, not code. |
| **Image** | `file-image` | `icon.image` | `icon.image` | `.png .jpg .jpeg .gif .webp .avif .svg .bmp .tif .tiff .heic` | |
| **Video** | `file-video` | `icon.video` | `icon.video` | `.mp4 .mkv .mov .webm .avi .m4v .mpg .wmv` | |
| **Audio** | `file-audio` | `icon.audio` | `icon.audio` | `.mp3 .flac .wav .ogg .m4a .aac .opus .wma` | Fallback: `waveform`. |
| **Archive / compressed** | `file-zip` | `icon.archive` | `icon.archive` | `.zip .tar .gz .bz2 .xz .7z .rar .zst .tgz` | **One glyph for all archive formats.** No per-format variants — nobody distinguishes a `.xz` from a `.7z` by shape, and inventing that distinction is noise. |
| **Binary / unknown** | `file` | `icon.binary` | `icon.binary` | extension not mapped, or none | Neutral. No mystery box, no question mark. |
| **Executable / binary program** | `file-exe` | `icon.executable` | `icon.executable` | executable bit set, or `.exe .appimage .bin .run .AppImage` | Fallback: `terminal`. |
| **Symlink** | `link-simple` | `icon.symlink` | `icon.symlink` | `is_symlink()` | The link glyph **replaces** the category glyph, because the most important fact about a symlink is that it is one — its target is shown as secondary text, not as an icon. Fallback: `link`. |
| **Hidden** | *(no icon change)* | `icon.hidden` | `icon.hidden` | leading `.` in the name | **Deliberate decision — see below.** The category glyph is kept and recoloured to `icon.hidden`, plus a 4px `dot` marker in the 3px leading gutter, plus `text.tertiary` on the name. |
| **Error / unreadable** | category glyph + `warning` | `icon.error` | `icon.error` | stat/permission/read failure | The category glyph is **retained and recoloured** to `icon.error`, the extension dot is replaced by a `warning` glyph, and the Kind column reads `Unreadable` in `status.danger-text`. The silhouette still tells you what kind of file it was. Fallback for the glyph: `warning-circle`. |

**Why hidden files get no special icon.** A hidden `.gitignore` is still a text file. Giving it a lock, an eye-slash, or a mystery box is a small lie, and it costs real legibility the moment you enable "show hidden" — which is exactly when you are trying to find a specific file and need the icons to be *more* consistent, not less. The `dot` marker plus muted text gives you the "these are hidden" read at the row's left edge, where your eye already is, without destroying the shape grammar. The `eye` / `eye-slash` toggle in the status bar is where the hidden state is controlled, and *that* is where the icon earns its place.

**Why symlinks override but hidden files do not.** The distinction is intent. A symlink's category is genuinely ambiguous — `notes.md -> ../docs/notes.md` — and resolving it is the file manager's job, so the link glyph carries the higher-priority fact. A hidden file's category is not ambiguous at all; its only new fact is "you normally don't see me," which is a fact about the *row*, not the file.

### 5.3 Chrome and action icons

| Purpose | Icon | Size | Colour |
|---|---|---|---|
| Home | `house` | 18 | `icon.chrome` |
| Desktop | `desktop` | 18 | `icon.chrome` |
| Documents | `file-text` | 18 | `icon.chrome` |
| Downloads | `download-simple` | 18 | `icon.chrome` |
| Recent | `clock` | 18 | `icon.chrome` |
| Bookmarks | `bookmark` | 18 | `icon.chrome` |
| Volume / drive | `hard-drive` | 18 | `icon.chrome` |
| Volume disconnected | `plug` | 18 | `icon.error` |
| Trash | `trash` | 18 | `icon.chrome` |
| Permission denied | `lock-simple` | 16 | `icon.error` |
| Back / forward | `arrow-left` / `arrow-right` | 18 | `icon.chrome` |
| Up one level | `arrow-up` | 18 | `icon.chrome` |
| Refresh | `arrows-clockwise` | 18 | `icon.chrome` |
| New folder | `folder-plus` | 18 | `icon.chrome` |
| Rename | `pencil-simple` | 18 | `icon.chrome` |
| Delete | `trash` | 18 | `icon.chrome` |
| Copy | `copy` | 18 | `icon.chrome` |
| Cut | `scissors` | 18 | `icon.chrome` |
| Paste | `clipboard` | 18 | `icon.chrome` |
| Extract here | `arrows-out` | 18 | `icon.chrome` |
| Open in terminal | `terminal` | 18 | `icon.chrome` |
| Search | `magnifying-glass` | 16 | `icon.chrome` |
| Show / hide hidden files | `eye` / `eye-slash` | 14 | `icon.chrome` |
| Toggle sidebar | `sidebar` | 18 | `icon.chrome` |
| View: list / grid / details | `list-bullets` / `grid-four` / `rows` | 18 | `icon.chrome` / Fill when active |
| Sort | `caret-up` / `caret-down` / `caret-up-down` | 12 | `icon.chrome` |
| Overflow | `dots-three-horizontal` / `dots-three-vertical` | 16 | `icon.chrome` |
| Confirm | `check` | 14 | `text.on-accent` |
| Clear | `x` | 12 | `icon.chrome` |
| Expand / collapse tree | `caret-right` / `caret-down` | 12 | `icon.chrome` |
| Hidden marker | `dot` | 4 | `icon.hidden` |
| Busy | `circle-notch` | 12–18 | `icon.chrome` |
| Warning | `warning` | 12–20 | `status.warning-text` |
| Error | `x-circle` | 12–20 | `status.danger-text` |
| Success | `check-circle` | 12 | `status.success-text` |

---

## 6. Accessibility audit

All ratios computed with the WCAG 2.x relative-luminance formula on the sRGB values as specified. No rounding in the token definitions; ratios reported to 2dp.

**Thresholds applied:** 4.5:1 for body and UI text under 18px/24px (`text.primary`, `text.secondary`, `text.tertiary`, all status text, all numeric metadata). 3:1 for non-text UI and meaningful graphics (`icon.*` hues, focus rings, scrollbar thumbs, input and checkbox borders, control outlines). Focus indicators: 3:1 against both the component and the surface behind it.

### 6.1 Text roles — LIGHT

Tested against all ten light surfaces: `app`, `panel`, `list`, `raised`, `row-hover`, `row-pressed`, `selected`, `selected-hover`, `input`, `focus-within`. The binding constraint in every case is the **selected-hover** background.

| Role | Value | app | panel | list | raised | row-hover | selected | sel-hover | input | **min** | |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `text.primary` | `#1C1A17` | 15.96 | 14.74 | 16.79 | 17.36 | 14.87 | 13.12 | 12.03 | 16.79 | **12.03** | PASS |
| `text.secondary` | `#4A453F` | 8.72 | 8.05 | 9.17 | 9.48 | 8.12 | 7.17 | 6.57 | 9.17 | **6.57** | PASS |
| `text.tertiary` | `#5E574F` | 6.54 | 6.04 | 6.88 | 7.11 | 6.09 | 5.38 | 4.93 | 6.88 | **4.93** | PASS |
| `text.disabled` | `#8C857C` | 3.35 | 3.09 | 3.52 | 3.65 | 3.12 | 2.75 | 2.53 | 3.52 | **2.53** | EXEMPT — §6.2 |
| `accent.text` | `#0A5449` | 8.12 | 7.50 | 8.54 | 8.83 | 7.57 | 6.67 | 6.12 | 8.54 | **6.12** | PASS |
| `status.danger-text` | `#B3261E` | 6.01 | 5.55 | 6.32 | 6.54 | 5.60 | 4.94 | 4.53 | 6.32 | **4.53** | PASS |
| `status.success-text` | `#286A2A` | 6.06 | 5.60 | 6.37 | 6.59 | 5.65 | 4.98 | 4.57 | 6.37 | **4.57** | PASS |
| `status.warning-text` | `#7A5100` | 6.70 | 6.22 | 7.06 | 7.32 | 6.29 | 5.28 | 4.84 | 7.06 | **4.84** | PASS |

`text.on-accent` `#FFFFFF` on `accent.base` `#0E6B5F` = **6.39** PASS. `text.on-danger` `#FFFFFF` on `status.danger-solid` `#8F1D17` = **8.91** PASS.

Status chips: danger `#8F1D17` on `#FBE9E7` = 7.60 · success `#286A2A` on `#E4F2E4` = 5.69 · warning `#7A5100` on `#FBF0D9` = 6.18 · accent `#0A5449` on `#DCF0EA` = 7.44. All PASS.

### 6.2 Text roles — DARK

| Role | Value | app | panel | list | raised | row-hover | selected | sel-hover | input | **min** | |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `text.primary` | `#F0EDE8` | 16.02 | 15.18 | 14.55 | 13.59 | 13.59 | 11.61 | 10.59 | 14.55 | **10.59** | PASS |
| `text.secondary` | `#B8B2A9` | 8.89 | 8.43 | 8.07 | 7.54 | 7.54 | 6.44 | 5.87 | 8.07 | **5.87** | PASS |
| `text.tertiary` | `#A6A099` | 7.23 | 6.85 | 6.56 | 6.13 | 6.13 | 5.24 | 4.77 | 6.56 | **4.77** | PASS |
| `text.disabled` | `#726D66` | 3.65 | 3.46 | 3.31 | 3.09 | 3.09 | 2.64 | 2.41 | 3.31 | **2.41** | EXEMPT — §6.2 |
| `accent.text` | `#6BD9C4` | 11.00 | 10.43 | 9.99 | 9.33 | 9.33 | 7.97 | 7.27 | 9.99 | **7.27** | PASS |
| `status.danger-text` | `#F2837C` | 7.41 | 7.02 | 6.72 | 6.28 | 6.28 | 5.37 | 4.89 | 6.72 | **4.89** | PASS |
| `status.success-text` | `#7FCE85` | 9.86 | 9.35 | 8.95 | 8.36 | 8.36 | 7.15 | 6.52 | 8.95 | **6.52** | PASS |
| `status.warning-text` | `#E0B461` | 9.69 | 9.18 | 8.79 | 8.21 | 8.21 | 7.02 | 6.40 | 8.79 | **6.40** | PASS |

`text.on-accent` `#08221D` on `accent.base` `#4FC7B1` = **8.06** PASS. `text.on-danger` `#F0EDE8` on `status.danger-solid` `#5A1512` = **11.60** PASS (this pairing was a real failure until corrected — see §6.5 entry 9).

Status chips: danger `#F2837C` on `#331614` = 6.57 · success `#7FCE85` on `#182B19` = 7.92 · warning `#E0B461` on `#332811` = 7.48 · accent `#6BD9C4` on `#16332F` = 7.97. All PASS.

**On `text.disabled`:** WCAG 1.4.3 explicitly exempts inactive/disabled user-interface components from contrast minimums, so 2.53 / 2.41 against a *selected-hover* row is not a failure. It is documented rather than hidden, and one structural rule removes the worst case: **disabled text never renders on a selected or selected-hover row.** Disabled items are not selectable, and if an item is both, the selection is dropped. On every surface where a disabled label can actually appear, the ratio is 3.09 or better, which clears 3:1 anyway.

### 6.3 Icon hues — 3:1 in both themes

Tested against all ten surfaces in each theme. The binding constraint for every slot is the **selected-hover** background, which is the closest a coloured icon ever sits to the surface it is drawn on.

| Slot | Light | min | Dark | min | |
|---|---|---|---|---|---|
| `folder` | `#9A6510` | 3.43 | `#D8A548` | 5.53 | PASS |
| `text` | `#5E574F` | 4.93 | `#B8B2A9` | 5.87 | PASS |
| `code` | `#5B3EA8` | 5.39 | `#A794F5` | 4.83 | PASS |
| `image` | `#A6326E` | 4.40 | `#E88AB4` | 5.14 | PASS |
| `video` | `#7A3ABF` | 4.57 | `#C199F2` | 5.36 | PASS |
| `audio` | `#286A2A` | 4.57 | `#7FCE85` | 6.52 | PASS |
| `archive` | `#9A4520` | 4.48 | `#E0A184` | 5.65 | PASS |
| `binary` | `#5E574F` | 4.93 | `#B8B2A9` | 5.87 | PASS |
| `executable` | `#1F5E8A` | 4.81 | `#6FB6E0` | 5.56 | PASS |
| `symlink` | `#0E6B5F` | 4.43 | `#4FC7B1` | 5.97 | PASS |
| `hidden` | `#6A635A` | 4.10 | `#948E85` | 3.81 | PASS |
| `error` | `#B3261E` | 4.53 | `#F2837C` | 4.89 | PASS |

**Adjacent-hue note:** the two lowest-contrast slots (`folder` 3.43 light, `hidden` 4.10 light) are the ones a designer is most tempted to push further, and they are left where they are — 3.43 is a comfortable margin over the 3.0 floor for a 16px glyph, and pushing the amber darker would cost the one colour in the app that carries real urgency. Both still clear 3:1 on the surface where they are least visible, and every slot clears it by ≥0.4 even in its worst case.

The three deliberately neutral slots (`text`, `binary`, and by extension `hidden`) are indistinguishable from each other by colour, and are distinguished by silhouette only — `file-text` vs `file` vs the row-level `dot` marker. That is the intended behaviour: "no hue" is a meaningful state, and inventing a colour for it would make the colour column unreliable.

### 6.4 Non-text UI — 3:1

| Element | Theme | Value | Worst surface | Ratio | |
|---|---|---|---|---|---|
| Focus ring | light | `#0E6B5F` | `state.selected-hover` `#BCDED5` | 4.43 | PASS |
| Focus ring | dark | `#4FC7B1` | `state.selected-hover` `#1A3A34` | 5.97 | PASS |
| Scrollbar thumb | light | `#7A7268` | `surface.panel` `#EFECE7` | 4.02 | PASS |
| Scrollbar thumb | dark | `#78716A` | `surface.list` `#1E1C1A` | 3.53 | PASS |
| Scrollbar thumb hover | light | `#68615A` | `surface.panel` | 5.17 | PASS |
| Scrollbar thumb hover | dark | `#948D84` | `surface.panel` | 5.41 | PASS |
| Checkbox idle border | light | `#7A7268` | `surface.input` | 4.58 | PASS |
| Checkbox idle border | dark | `#78716A` | `surface.list` | 3.53 | PASS |
| Input border | light | `#8C857C` | `surface.input` | 3.52 | PASS |
| Input border | dark | `#6F6961` | `surface.list` | 3.13 | PASS |

`focus.ring-inactive` at 40% alpha is a deliberate reduction and is **exempt**: the ring is only dimmed when the *window* is unfocused, at which point WCAG 2.4.7 does not require a visible focus indicator. Restoring full opacity on refocus is mandatory.

**Borders `subtle` (1.36 light / 1.19 dark) and `default` (1.59 / 1.50) are intentionally below 3:1.** They are decorative separators between already-distinguishable regions, not the sole means of identifying a control. Every *control boundary* — input border, checkbox, dialog outline, menu outline, focus ring, scrollbar thumb — clears 3:1 independently. This is the distinction the 3:1 requirement actually draws, and it is why separators can be hairlines without becoming invisible structure.

### 6.5 Corrections made during the audit

Every one of these was a real failure found by measurement, then fixed. Recording them because a spec with no failures in it was not actually measured. "Background" is the surface the pairing was measured against.

| # | Pairing | Found | Problem | Corrected to | New ratio |
|---|---|---|---|---|---|
| 1 | light `success-text` on `state.selected` | `#2C722E` | **4.47** — under 4.5 | `#286A2A` | 4.98 |
| 2 | light `warning-text` on `state.selected` | `#8A5A00` | **4.48** — under 4.5 | `#7A5100` | 5.28 |
| 3 | dark `text.tertiary` on `state.selected` | `#9A948B` on `#1E4640` | **3.48** — badly under 4.5 | selection tint → `#16332F` | 4.51 |
| 4 | dark `text.tertiary` on `state.selected-hover` | `#9A948B` on `#1B423C` | **4.11** — under 4.5 | tertiary → `#A6A099`, sel-hover → `#1A3A34` | 4.77 |
| 5 | light `warning-text` on `state.selected-hover` | `#825600` on `#BCDED5` | **4.43** — under 4.5 | `#7A5100` | 4.84 |
| 6 | dark `text.on-danger` on `status.danger-solid` | `#08221D` on `#5A1512` | **1.23** — near-invisible ink on near-black red | `#F0EDE8` | 11.60 |
| 7 | light scrollbar thumb on `surface.panel` | `#A79F94` | **2.22** — a scrollbar with no visible thumb | `#7A7268` | 4.02 |
| 8 | dark scrollbar thumb on `surface.list` | `#4A453F` | **1.87** — effectively invisible | `#78716A` | 3.53 |
| 9 | dark scrollbar thumb hover on `surface.panel` | `#6A635A` | **2.99** — missed by 0.01 | `#948D84` | 5.41 |
| 10 | dark `text.disabled` on `state.hover` | `#6B6660` on `#24221F` | **2.79** — missed by 0.21 | `#726D66` | 3.09 |
| 11 | dark `icon.hidden` on `state.selected` | `#8C8579` on `#1E4640` | **2.87** — under 3:1 | `#948E85` (and the tint fix in #3) | 3.81 |
| 12 | light `hue.folder` on `state.selected-hover` | `#A0741A` on `#BCDED5` | **2.94** — under 3:1 | `#9A6510` | 3.43 |
| 13 | dark `accent.border` on `surface.raised` | `#2A5A51` on `#24221F` | **2.02** — under 3:1 | `#3A7A6E` | 3.16 |

Entries 3, 4, 5 and 11 are all consequences of the same root cause: the dark selection tints were originally too light, which compressed the available contrast range for everything drawn on top of a selected row. Fixing the tints (3) exposed a second failure (4), which fixed the tint differently. The `selected-hover` state is the worst surface in the entire system — darker than `selected` in light mode is a mistake, lighter is a mistake in dark mode, and every value here was chosen by measurement rather than by eye.

### 6.6 Beyond contrast

- **Never colour alone.** Every state has a second channel: selection has a 2px bar, hidden has a `dot` marker, error has a glyph swap, disabled has reduced icon alpha *and* muted text, cut has reduced alpha on both bg and icon, focus has a ring, active-place has a 3px bar.
- **Focus Not Obscured (WCAG 2.2, minimum):** satisfied structurally. Toolbar, breadcrumb, column header, and status bar are layout siblings with 1px borders, not overlays. No sticky element ever covers a focused row. Floating menus close on blur before focus can move behind them.
- **Target size (WCAG 2.2, minimum 24×24):** every interactive element's hit area is ≥24px even when the painted element is smaller. Checkbox 14px painted / 24px hit. Scrollbar thumb 6px painted / 12px hit. Breadcrumb separator 12px painted / 16px hit. Hidden-file toggle 14px painted / 24px hit.
- **Keyboard completeness:** every action in §4.11 has a binding; focus order is sidebar → toolbar → breadcrumb → list → status bar; menus trap focus and restore it on close; `Escape` always has a defined single effect by priority.
- **Reduced motion:** honoured per §2.11 rule 5. No information is conveyed by motion alone anywhere in the app.
- **Type size:** 13px base is below the 16px comfortable reading target, which is an accepted trade for a desktop density tool. It is mitigated by a 1:1.38 line ratio (18px), a 12px mono with tabular figures for all machine values, and by allowing the OS text-scale factor to raise the whole scale (every size is a token, so a 1.25× scale is a single multiplier, not a redesign).

---

## 7. Anti-goals

What this design deliberately refuses to do. Each entry names a specific, recognisable failure mode.

1. **Default blue.** No `#3B82F6`, no `#2563EB`, no `#0A84FF`, and no "blue but slightly different" as primary, brand, link, focus, or selection colour. The accent is `#0E6B5F` (light) / `#4FC7B1` (dark). Blue is the colour of every AI-generated interface ever shipped; using it would be indistinguishable from not designing at all.
2. **Default gray.** No `#F3F4F6`, no `#9CA3AF`, no `#E5E7EB`. The neutral ramp runs `#FCFBF9` → `#0A0A09` and every step carries 3–5% warm chroma. A pure-gray ramp is what makes an interface look like a screenshot of a template.
3. **Roboto, Inter, or `system-ui`.** The type is IBM Plex Sans and IBM Plex Mono. Not because they're fashionable — because a single superfamily with real character at 13px, plus a strict mono-for-machine-values / sans-for-human-names split, is a decision. `system-ui` is refused specifically because it silently changes meaning on every platform the app is run on, and this app is developed by one person who will open it on more than one machine.
4. **One spacing scale for everything.** The app runs three density bands — chrome (8/12/16), list (4/8/10), icon gutter (0/2/10) — and every spacing value is chosen by role, not by picking the next rung. Uniform 8px padding everywhere is the layout equivalent of a default font.
5. **Evenly-spaced, evenly-weighted everything.** Section labels are 11px/600/uppercase/tracked +0.06em while file names are 13px/400. Weights and sizes carry meaning. If two elements are the same size and weight, they are the same kind of thing; if not, they are not.
6. **Rounded-pill selection.** Rows are 3px. Selection is a flat tint plus a 2px left bar, not an 8px-radius blue lozenge. A pill on a 26px row leaves 13px of dead vertical space and reads as a chip, not as a row.
7. **Decorative colour.** No coloured section headers, no coloured toolbar, no gradient buttons, no accent-coloured dividers, no rainbow folder tabs. Saturated colour appears only where it carries information.
8. **Shadows on chrome.** Toolbar, sidebar, breadcrumb, and status bar are flat, separated by 1px hairlines and 2–4% lightness steps. Shadows are for menus, dialogs, and the drag ghost only. Shadow-on-everything is the strongest "generic dashboard" tell there is.
9. **Three icon styles.** One set (Phosphor), one weight per context, one optical size. No mixing Lucide with Phosphor with hand-drawn SVGs, and no mixing outline and filled glyphs in the same column.
10. **Emoji as icons.** No 📁 📄 🎵 in a file list. They render differently on every platform, they cannot be recoloured by theme, and they break at 16px.
11. **Mystery-box icons.** An unclassified file is a neutral `file`, not a question mark. A hidden file keeps its category icon (§5.2).
12. **Motion as personality.** No bounce, no spring, no overshoot, no stagger, no parallax, no page transitions, no skeleton shimmer, no animated gradients. A local filesystem read is not a network wait, and nothing in this app takes longer than 130ms to respond to the pointer. Selection is **0ms** (§2.11).
13. **Layout-animating properties.** Nothing tweens `width`, `height`, `top`, `left`, `margin`, or padding. A file manager that animates row height while you scroll is a file manager that drops frames on a weak machine, which directly violates the "lightweight" constraint.
14. **Hidden affordances.** No icon-only toolbar button without a tooltip *and* an accessible name. No hover-only state that reveals information available nowhere else. No keyboard action discoverable only by accident.
15. **The card reflex.** Nothing in this app is a card. The sidebar, the list, the toolbar, and the status bar are regions divided by hairlines. Floating rounded panels with drop shadows and internal padding are a web-dashboard idiom that has no meaning in a two-pane file manager.
16. **Silent status changes.** A long operation always reports itself in the status bar with a spinner and a text label. The app never blocks input and never shows a spinner with no words.
17. **Generic destructive actions.** No `OK`. No red-filled menu rows. The verb is specific (`Delete Permanently`), the safe action holds focus on open, and irreversible actions require a dialog whose Enter is not bound to them.

---

*End of specification. Every hex, px, ms and weight above is final and directly mappable. The only value requiring a decision at implementation time is the exact Phosphor glyph name for a handful of file-type variants (§5.2 verification note); the silhouette and colour slot are already fixed.*
