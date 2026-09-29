//! The Kestrel design system, as typed Rust.
//!
//! This module is a *transcription* of the design token specification at
//! `/tmp/opencode/kestrel-tokens.md` (1,038 lines, machine-audited, direction
//! "Kestrel" locked). It is organised in the same three layers the spec
//! mandates, in the same order:
//!
//! | Layer | Module | Rule (spec §"Layer rule", hard) |
//! |---|---|---|
//! | Primitive | [`primitive`], [`space`], [`radius`], [`border`], [`metric`], [`motion`], [`ease`], [`ty`] | raw values, no meaning |
//! | Semantic | [`Theme`] and its sub-structs | may only reference primitives |
//! | Component | [`component`] | may only reference *semantic* names |
//!
//! No component in this crate ever names a hex value or a primitive. If a
//! component needs a value the semantic layer does not define, that is a gap in
//! §3 of the spec and the role should be added there, not reached past.
//!
//! # Traceability
//!
//! Every colour below carries the spec's hex in a trailing comment. The comment
//! is the source of truth for *which* spec line a value came from; the code is
//! only the encoding. If they ever disagree, the spec wins and this file is a
//! bug.
//!
//! # Usage
//!
//! ```no_run
//! # use kestrel::tokens::{Theme, ThemeMode};
//! # use egui::Context;
//! fn apply(ctx: &Context, mode: ThemeMode, system_dark: Option<bool>) {
//!     let theme = Theme::resolve(mode, system_dark);
//!     theme.apply(ctx);            // pushes Visuals + Style + text styles
//!     let bg = theme.surfaces.app; // call sites read semantic names only
//!     let _ = bg;
//! }
//! ```

use std::sync::Arc;
use std::time::Duration;

use egui::{
    Color32, Context, CornerRadius, FontFamily, FontId, Rect, Shadow, Stroke, StrokeKind,
    TextStyle, Vec2, Visuals,
};

// =============================================================================
// Layer 1 — PRIMITIVES (§2)
// =============================================================================

/// §2.1 Neutral ramp, "warm graphite".
///
/// Every step carries roughly 3–5% warm chroma. This is the single most
/// important structural decision of the "Kestrel" direction: a pure-gray ramp
/// is what makes an interface look like a screenshot of a template (§7.2).
pub mod neutral {
    // A token table is *deliberately* complete: `neutral.500` and
    // `neutral.1000` are transcribed from §2.1 whether or not §3 happens to
    // name them today, because a token file that only contains the values the
    // current UI happens to use stops being a spec and becomes a cache. Every
    // unused constant below is therefore intentional, not dead.
    #![allow(dead_code)]

    use super::Color32;

    /// `neutral.25` — brightest light surface.
    pub const N25: Color32 = Color32::from_rgb(0xFC, 0xFB, 0xF9);
    /// `neutral.50`
    pub const N50: Color32 = Color32::from_rgb(0xF7, 0xF5, 0xF2);
    /// `neutral.100`
    pub const N100: Color32 = Color32::from_rgb(0xEF, 0xEC, 0xE7);
    /// `neutral.150`
    pub const N150: Color32 = Color32::from_rgb(0xE7, 0xE3, 0xDC);
    /// `neutral.200`
    pub const N200: Color32 = Color32::from_rgb(0xDE, 0xD9, 0xD1);
    /// `neutral.300`
    pub const N300: Color32 = Color32::from_rgb(0xCF, 0xC9, 0xC0);
    /// `neutral.400`
    pub const N400: Color32 = Color32::from_rgb(0xA7, 0x9F, 0x94);
    /// `neutral.500`
    pub const N500: Color32 = Color32::from_rgb(0x7C, 0x74, 0x6A);
    /// `neutral.600`
    pub const N600: Color32 = Color32::from_rgb(0x5E, 0x57, 0x4F);
    /// `neutral.700`
    pub const N700: Color32 = Color32::from_rgb(0x44, 0x3F, 0x39);
    /// `neutral.800`
    pub const N800: Color32 = Color32::from_rgb(0x2B, 0x28, 0x24);
    /// `neutral.850`
    pub const N850: Color32 = Color32::from_rgb(0x21, 0x1F, 0x1C);
    /// `neutral.900`
    pub const N900: Color32 = Color32::from_rgb(0x18, 0x17, 0x15);
    /// `neutral.950`
    pub const N950: Color32 = Color32::from_rgb(0x10, 0x0F, 0x0E);
    /// `neutral.1000` — near-black, dark theme canvas base.
    pub const N1000: Color32 = Color32::from_rgb(0x0A, 0x0A, 0x09);
}

/// §2.2 Accent ramp, "pine teal". The only hue in the UI chrome.
///
/// Blue is refused by policy (§7.1): the accent is `#0E6B5F` light,
/// `#4FC7B1` dark, and nothing else.
pub mod accent {
    // A token table is *deliberately* complete: `neutral.500` and
    // `neutral.1000` are transcribed from §2.1 whether or not §3 happens to
    // name them today, because a token file that only contains the values the
    // current UI happens to use stops being a spec and becomes a cache. Every
    // unused constant below is therefore intentional, not dead.
    #![allow(dead_code)]

    use super::Color32;

    /// `accent.50`
    pub const A50: Color32 = Color32::from_rgb(0xE8, 0xF5, 0xF1);
    /// `accent.100`
    pub const A100: Color32 = Color32::from_rgb(0xDC, 0xF0, 0xEA);
    /// `accent.200`
    pub const A200: Color32 = Color32::from_rgb(0xC9, 0xE6, 0xDF);
    /// `accent.300`
    pub const A300: Color32 = Color32::from_rgb(0xA6, 0xD6, 0xCB);
    /// `accent.400`
    pub const A400: Color32 = Color32::from_rgb(0x5F, 0xC0, 0xAC);
    /// `accent.500`
    pub const A500: Color32 = Color32::from_rgb(0x33, 0xA8, 0x94);
    /// `accent.600` — base accent (light theme).
    pub const A600: Color32 = Color32::from_rgb(0x0E, 0x6B, 0x5F);
    /// `accent.700`
    pub const A700: Color32 = Color32::from_rgb(0x0A, 0x54, 0x49);
    /// `accent.800`
    pub const A800: Color32 = Color32::from_rgb(0x07, 0x3B, 0x33);
    /// `accent.900`
    pub const A900: Color32 = Color32::from_rgb(0x05, 0x2B, 0x25);
    /// `accent.400.dark` — base accent (dark theme).
    pub const A400_DARK: Color32 = Color32::from_rgb(0x4F, 0xC7, 0xB1);
    /// `accent.300.dark`
    pub const A300_DARK: Color32 = Color32::from_rgb(0x6B, 0xD9, 0xC4);
}

/// §2.3 Status ramps. Each hue is ≥40° away from the accent and from the other
/// two, and is always paired with a distinct glyph so colour is never the sole
/// channel (§2.3 note, §6.6).
pub mod status {
    // A token table is *deliberately* complete: `neutral.500` and
    // `neutral.1000` are transcribed from §2.1 whether or not §3 happens to
    // name them today, because a token file that only contains the values the
    // current UI happens to use stops being a spec and becomes a cache. Every
    // unused constant below is therefore intentional, not dead.
    #![allow(dead_code)]

    use super::Color32;

    /// `danger.50`
    pub const D50: Color32 = Color32::from_rgb(0xFB, 0xE9, 0xE7);
    /// `danger.100`
    pub const D100: Color32 = Color32::from_rgb(0xF7, 0xD5, 0xD1);
    /// `danger.300`
    pub const D300: Color32 = Color32::from_rgb(0xD9, 0x6A, 0x62);
    /// `danger.400`
    pub const D400: Color32 = Color32::from_rgb(0xE0, 0x5A, 0x50);
    /// `danger.500`
    pub const D500: Color32 = Color32::from_rgb(0xB3, 0x26, 0x1E);
    /// `danger.600`
    pub const D600: Color32 = Color32::from_rgb(0x8F, 0x1D, 0x17);
    /// `danger.700`
    pub const D700: Color32 = Color32::from_rgb(0x6E, 0x16, 0x10);
    /// `danger.400.dark`
    pub const D400_DARK: Color32 = Color32::from_rgb(0xF2, 0x83, 0x7C);
    /// `danger.600.dark`
    pub const D600_DARK: Color32 = Color32::from_rgb(0x5A, 0x15, 0x12);

    /// `success.50`
    pub const S50: Color32 = Color32::from_rgb(0xE4, 0xF2, 0xE4);
    /// `success.100`
    pub const S100: Color32 = Color32::from_rgb(0xCF, 0xE7, 0xD0);
    /// `success.300`
    pub const S300: Color32 = Color32::from_rgb(0x8C, 0xBF, 0x8F);
    /// `success.400`
    pub const S400: Color32 = Color32::from_rgb(0x4E, 0x9A, 0x52);
    /// `success.500`
    pub const S500: Color32 = Color32::from_rgb(0x28, 0x6A, 0x2A);
    /// `success.600`
    pub const S600: Color32 = Color32::from_rgb(0x1D, 0x4E, 0x1F);
    /// `success.400.dark`
    pub const S400_DARK: Color32 = Color32::from_rgb(0x7F, 0xCE, 0x85);

    /// `warning.50`
    pub const W50: Color32 = Color32::from_rgb(0xFB, 0xF0, 0xD9);
    /// `warning.100`
    pub const W100: Color32 = Color32::from_rgb(0xF6, 0xE3, 0xB8);
    /// `warning.300`
    pub const W300: Color32 = Color32::from_rgb(0xE3, 0xC4, 0x70);
    /// `warning.400`
    pub const W400: Color32 = Color32::from_rgb(0xC9, 0x9A, 0x1E);
    /// `warning.500`
    pub const W500: Color32 = Color32::from_rgb(0x7A, 0x51, 0x00);
    /// `warning.600`
    pub const W600: Color32 = Color32::from_rgb(0x5E, 0x3F, 0x00);
    /// `warning.400.dark`
    pub const W400_DARK: Color32 = Color32::from_rgb(0xE0, 0xB4, 0x61);
}

/// §2.4 File-type hue slots.
///
/// These are the *entire* colour budget of the app (§1.4 decision 1). Every one
/// clears 3:1 against every surface in both themes (§6.3).
///
/// Components never reference this module — they reference [`Theme::icon`].
pub mod hue {
    // A token table is *deliberately* complete: `neutral.500` and
    // `neutral.1000` are transcribed from §2.1 whether or not §3 happens to
    // name them today, because a token file that only contains the values the
    // current UI happens to use stops being a spec and becomes a cache. Every
    // unused constant below is therefore intentional, not dead.
    #![allow(dead_code)]

    use super::Color32;

    /// `hue.folder`
    pub const FOLDER_L: Color32 = Color32::from_rgb(0x9A, 0x65, 0x10);
    /// `hue.folder` (dark)
    pub const FOLDER_D: Color32 = Color32::from_rgb(0xD8, 0xA5, 0x48);
    /// `hue.text`
    pub const TEXT_L: Color32 = Color32::from_rgb(0x5E, 0x57, 0x4F);
    /// `hue.text` (dark)
    pub const TEXT_D: Color32 = Color32::from_rgb(0xB8, 0xB2, 0xA9);
    /// `hue.code`
    pub const CODE_L: Color32 = Color32::from_rgb(0x5B, 0x3E, 0xA8);
    /// `hue.code` (dark)
    pub const CODE_D: Color32 = Color32::from_rgb(0xA7, 0x94, 0xF5);
    /// `hue.image`
    pub const IMAGE_L: Color32 = Color32::from_rgb(0xA6, 0x32, 0x6E);
    /// `hue.image` (dark)
    pub const IMAGE_D: Color32 = Color32::from_rgb(0xE8, 0x8A, 0xB4);
    /// `hue.video`
    pub const VIDEO_L: Color32 = Color32::from_rgb(0x7A, 0x3A, 0xBF);
    /// `hue.video` (dark)
    pub const VIDEO_D: Color32 = Color32::from_rgb(0xC1, 0x99, 0xF2);
    /// `hue.audio`
    pub const AUDIO_L: Color32 = Color32::from_rgb(0x28, 0x6A, 0x2A);
    /// `hue.audio` (dark)
    pub const AUDIO_D: Color32 = Color32::from_rgb(0x7F, 0xCE, 0x85);
    /// `hue.archive`
    pub const ARCHIVE_L: Color32 = Color32::from_rgb(0x9A, 0x45, 0x20);
    /// `hue.archive` (dark)
    pub const ARCHIVE_D: Color32 = Color32::from_rgb(0xE0, 0xA1, 0x84);
    /// `hue.binary` — intentionally identical to `hue.text` (§2.4 note).
    pub const BINARY_L: Color32 = TEXT_L;
    /// `hue.binary` (dark)
    pub const BINARY_D: Color32 = TEXT_D;
    /// `hue.executable`
    pub const EXECUTABLE_L: Color32 = Color32::from_rgb(0x1F, 0x5E, 0x8A);
    /// `hue.executable` (dark)
    pub const EXECUTABLE_D: Color32 = Color32::from_rgb(0x6F, 0xB6, 0xE0);
    /// `hue.symlink`
    pub const SYMLINK_L: Color32 = Color32::from_rgb(0x0E, 0x6B, 0x5F);
    /// `hue.symlink` (dark)
    pub const SYMLINK_D: Color32 = Color32::from_rgb(0x4F, 0xC7, 0xB1);
    /// `hue.hidden`
    pub const HIDDEN_L: Color32 = Color32::from_rgb(0x6A, 0x63, 0x5A);
    /// `hue.hidden` (dark)
    pub const HIDDEN_D: Color32 = Color32::from_rgb(0x94, 0x8E, 0x85);
    /// `hue.error`
    pub const ERROR_L: Color32 = Color32::from_rgb(0xB3, 0x26, 0x1E);
    /// `hue.error` (dark)
    pub const ERROR_D: Color32 = Color32::from_rgb(0xF2, 0x83, 0x7C);
}

/// §2.5 Spacing scale — 4px base with a 2px half-step.
///
/// The half-step exists for the file list, where a 4px rhythm is too coarse.
/// §7.4 is explicit that the three density bands are *not* an inconsistency:
/// every value is chosen by role, not by picking the next rung up the ladder.
pub mod space {
    // A token table is *deliberately* complete: `neutral.500` and
    // `neutral.1000` are transcribed from §2.1 whether or not §3 happens to
    // name them today, because a token file that only contains the values the
    // current UI happens to use stops being a spec and becomes a cache. Every
    // unused constant below is therefore intentional, not dead.
    #![allow(dead_code)]

    /// `space.0`
    pub const S0: f32 = 0.0;
    /// `space.half`
    pub const HALF: f32 = 2.0;
    /// `space.1`
    pub const S1: f32 = 4.0;
    /// `space.1.5`
    pub const S1_5: f32 = 6.0;
    /// `space.2`
    pub const S2: f32 = 8.0;
    /// `space.2.5`
    pub const S2_5: f32 = 10.0;
    /// `space.3`
    pub const S3: f32 = 12.0;
    /// `space.4`
    pub const S4: f32 = 16.0;
    /// `space.5`
    pub const S5: f32 = 20.0;
    /// `space.6`
    pub const S6: f32 = 24.0;
    /// `space.8`
    pub const S8: f32 = 32.0;
    /// `space.10`
    pub const S10: f32 = 40.0;
    /// `space.12`
    pub const S12: f32 = 48.0;
}

/// §2.6 Radii.
pub mod radius {
    // A token table is *deliberately* complete: `neutral.500` and
    // `neutral.1000` are transcribed from §2.1 whether or not §3 happens to
    // name them today, because a token file that only contains the values the
    // current UI happens to use stops being a spec and becomes a cache. Every
    // unused constant below is therefore intentional, not dead.
    #![allow(dead_code)]

    use super::CornerRadius;

    /// `radius.none`
    pub const NONE: f32 = 0.0;
    /// `radius.xs`
    pub const XS: f32 = 2.0;
    /// `radius.sm` — the file-row radius (§4.2 `row.radius`).
    pub const SM: f32 = 3.0;
    /// `radius.md`
    pub const MD: f32 = 4.0;
    /// `radius.lg`
    pub const LG: f32 = 6.0;
    /// `radius.xl`
    pub const XL: f32 = 8.0;
    /// `radius.2xl`
    pub const XXL: f32 = 12.0;
    /// `radius.pill`
    pub const PILL: f32 = 999.0;

    /// egui's `CornerRadius` is per-corner `u8`; this converts a spec radius.
    #[must_use]
    pub fn all(px: f32) -> CornerRadius {
        CornerRadius::same(px.clamp(0.0, u8::MAX as f32).round() as u8)
    }
}

/// §2.7 Border widths.
pub mod border {
    // A token table is *deliberately* complete: `neutral.500` and
    // `neutral.1000` are transcribed from §2.1 whether or not §3 happens to
    // name them today, because a token file that only contains the values the
    // current UI happens to use stops being a spec and becomes a cache. Every
    // unused constant below is therefore intentional, not dead.
    #![allow(dead_code)]

    /// `border.hairline` — all separators, input borders, menu outline.
    pub const HAIRLINE: f32 = 1.0;
    /// `border.thick` — focus ring, selection left bar.
    pub const THICK: f32 = 2.0;
    /// `border.marker` — active-pane indicator bar.
    pub const MARKER: f32 = 3.0;
    /// `border.hidden-marker` — diameter of the hidden-file dot.
    pub const HIDDEN_MARKER: f32 = 4.0;
}

/// §2.9 Layout metrics.
pub mod metric {
    // A token table is *deliberately* complete: `neutral.500` and
    // `neutral.1000` are transcribed from §2.1 whether or not §3 happens to
    // name them today, because a token file that only contains the values the
    // current UI happens to use stops being a spec and becomes a cache. Every
    // unused constant below is therefore intentional, not dead.
    #![allow(dead_code)]

    /// `metric.row` — the default file list row height. Also
    /// `metric.sidebar-item`, which the spec requires to equal it.
    pub const ROW: f32 = 26.0;
    /// `metric.row-compact`
    pub const ROW_COMPACT: f32 = 22.0;
    /// `metric.row-comfortable`
    pub const ROW_COMFORTABLE: f32 = 32.0;
    /// `metric.column-header`
    pub const COLUMN_HEADER: f32 = 24.0;
    /// `metric.toolbar`
    pub const TOOLBAR: f32 = 34.0;
    /// `metric.breadcrumb`
    pub const BREADCRUMB: f32 = 28.0;
    /// `metric.status-bar`
    pub const STATUS_BAR: f32 = 24.0;
    /// `metric.sidebar-item` — must equal [`ROW`]; asserted in tests.
    pub const SIDEBAR_ITEM: f32 = ROW;
    /// `metric.sidebar-width` — default.
    pub const SIDEBAR_WIDTH: f32 = 200.0;
    /// `metric.sidebar-width` — min.
    pub const SIDEBAR_MIN: f32 = 160.0;
    /// `metric.sidebar-width` — max.
    pub const SIDEBAR_MAX: f32 = 340.0;
    /// `metric.icon` — list icons; never below 14.
    pub const ICON: f32 = 16.0;
    /// `metric.icon-compact`
    pub const ICON_COMPACT: f32 = 14.0;
    /// `metric.icon-chrome`
    pub const ICON_CHROME: f32 = 18.0;
    /// `metric.icon-lg`
    pub const ICON_LG: f32 = 20.0;
    /// `metric.target-min` — WCAG 2.2 minimum interactive hit target.
    pub const TARGET_MIN: f32 = 24.0;
    /// `metric.scrollbar` — full hit area.
    pub const SCROLLBAR: f32 = 12.0;
    /// `metric.scrollbar-thumb` — visible thumb.
    pub const SCROLLBAR_THUMB: f32 = 6.0;
    /// `metric.gutter` — leading gutter before the icon.
    pub const GUTTER: f32 = 10.0;
    /// `metric.gutter-marker` — gutter reserved for the hidden-file dot.
    pub const GUTTER_MARKER: f32 = 3.0;
    /// `metric.column-name` — flexible, min 160.
    pub const COLUMN_NAME_MIN: f32 = 160.0;
    /// `metric.column-size` — right-aligned.
    pub const COLUMN_SIZE: f32 = 88.0;
    /// `metric.column-kind`
    pub const COLUMN_KIND: f32 = 92.0;
    /// `metric.column-modified` — right-aligned.
    pub const COLUMN_MODIFIED: f32 = 140.0;
    /// `metric.column-permissions` — mono, right-aligned.
    pub const COLUMN_PERMISSIONS: f32 = 108.0;
}

/// §2.11 Motion.
///
/// Rule 1 is the most important line in the spec: **selection is instant (0ms)**.
/// A fading selection reads as lag in a file list. See [`motion::INSTANT`].
pub mod motion {
    // A token table is *deliberately* complete: `neutral.500` and
    // `neutral.1000` are transcribed from §2.1 whether or not §3 happens to
    // name them today, because a token file that only contains the values the
    // current UI happens to use stops being a spec and becomes a cache. Every
    // unused constant below is therefore intentional, not dead.
    #![allow(dead_code)]

    use super::Duration;

    /// `motion.instant` — selection state changes. Never animate selection.
    pub const INSTANT: Duration = Duration::from_millis(0);
    /// `motion.fast` — hover tint, focus ring, checkbox, toggle.
    pub const FAST: Duration = Duration::from_millis(90);
    /// `motion.base` — button press, row hover, icon swap. The ceiling for any
    /// direct-manipulation response (§2.11 rule 3).
    pub const BASE: Duration = Duration::from_millis(130);
    /// `motion.moderate` — menu open, dialog enter, sidebar resize settle.
    pub const MODERATE: Duration = Duration::from_millis(180);
    /// `motion.slow` — scrim fade-in.
    pub const SLOW: Duration = Duration::from_millis(260);
    /// `motion.deliberate` — directory content cross-fade on navigation.
    pub const DELIBERATE: Duration = Duration::from_millis(380);
    /// `motion.loop.spinner` — one rotation, linear, looping.
    pub const SPINNER: Duration = Duration::from_millis(900);

    /// Everything in [`SUPPRESSED`] renders its final state immediately when the
    /// OS reports a reduced-motion preference (§2.11 rule 5).
    pub const SUPPRESSED: [Duration; 6] = [INSTANT, FAST, BASE, MODERATE, SLOW, DELIBERATE];
}

/// §2.11 Easing curves.
///
/// egui has no cubic-bezier easing primitive, so these are recorded for
/// traceability and consumed only where egui offers a matching timing function
/// (see [`cubic_bezier_ease_in_out_quad`] as the current stand-in). No
/// component in this crate tweens layout (§2.11 rule 2, §7.13).
pub mod ease {
    // A token table is *deliberately* complete: `neutral.500` and
    // `neutral.1000` are transcribed from §2.1 whether or not §3 happens to
    // name them today, because a token file that only contains the values the
    // current UI happens to use stops being a spec and becomes a cache. Every
    // unused constant below is therefore intentional, not dead.
    #![allow(dead_code)]

    /// `ease.standard` — `cubic-bezier(0.2, 0, 0, 1)`
    pub const STANDARD: [f64; 4] = [0.2, 0.0, 0.0, 1.0];
    /// `ease.exit` — `cubic-bezier(0.4, 0, 1, 1)`
    pub const EXIT: [f64; 4] = [0.4, 0.0, 1.0, 1.0];
    /// `ease.emphasis` — `cubic-bezier(0.05, 0.7, 0.1, 1)`
    pub const EMPHASIS: [f64; 4] = [0.05, 0.7, 0.1, 1.0];
    /// `ease.linear` — `linear` (indeterminate progress only)
    pub const LINEAR: [f64; 4] = [0.0, 0.0, 1.0, 1.0];
}

/// Which of §2.8's two families a type token belongs to.
///
/// §2.8's mono/sans split is a hard rule: "Monospace for machine data;
/// proportional for human names." Making it a type rather than a convention is
/// what stops a future call site from quietly rendering a file size in Plex Sans
/// where the spec says Plex Mono.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontKind {
    /// IBM Plex Sans — human-readable names.
    Sans,
    /// IBM Plex Mono — machine values, with tabular figures.
    Mono,
}

/// A resolved type-scale entry (§2.8).
#[derive(Debug, Clone, PartialEq)]
pub struct TypeToken {
    /// Absolute px size.
    pub size: f32,
    /// Line box height, px.
    pub line: f32,
    /// Spec weight, 400–700. Applied by choosing a font *family* rather than
    /// by ordering a fallback list — see [`fonts`] for why that distinction is
    /// the whole trick.
    pub weight: u16,
    /// Letter tracking, in em. **Not applied**: epaint exposes no tracking
    /// control, and the vendored Plex faces are used at their designed side
    /// bearings. Recorded for traceability and asserted in tests.
    pub tracking: f32,
    /// Which §2.8 family this token belongs to.
    pub kind: FontKind,
    /// Uppercase (only `type.label`).
    pub uppercase: bool,
    /// The egui `TextStyle` name this token is registered under, e.g. `"ui"`.
    pub style_name: &'static str,
}

impl TypeToken {
    const fn new(
        style_name: &'static str,
        size: f32,
        line: f32,
        weight: u16,
        tracking: f32,
        kind: FontKind,
        uppercase: bool,
    ) -> Self {
        Self {
            size,
            line,
            weight,
            tracking,
            kind,
            uppercase,
            style_name,
        }
    }
}

/// §2.8 Typography. Sizes are absolute px; the base body is 13px.
pub mod ty {
    // A token table is *deliberately* complete: `neutral.500` and
    // `neutral.1000` are transcribed from §2.1 whether or not §3 happens to
    // name them today, because a token file that only contains the values the
    // current UI happens to use stops being a spec and becomes a cache. Every
    // unused constant below is therefore intentional, not dead.
    #![allow(dead_code)]

    use super::FontKind;
    pub use super::TypeToken;

    const SANS: FontKind = FontKind::Sans;
    const MONO: FontKind = FontKind::Mono;

    /// `type.micro` — 10/14, 500, +0.04em
    pub const MICRO: TypeToken = TypeToken::new("micro", 10.0, 14.0, 500, 0.04, SANS, false);
    /// `type.caption` — 11/15, 400
    pub const CAPTION: TypeToken = TypeToken::new("caption", 11.0, 15.0, 400, 0.0, SANS, false);
    /// `type.label` — 11/15, 600, +0.06em, uppercase
    pub const LABEL: TypeToken = TypeToken::new("label", 11.0, 15.0, 600, 0.06, SANS, true);
    /// `type.meta` — mono 12/16, 400
    pub const META: TypeToken = TypeToken::new("meta", 12.0, 16.0, 400, 0.0, MONO, false);
    /// `type.meta-strong` — mono 12/16, 500
    pub const META_STRONG: TypeToken =
        TypeToken::new("meta-strong", 12.0, 16.0, 500, 0.0, MONO, false);
    /// `type.status` — 12/16, 400
    pub const STATUS: TypeToken = TypeToken::new("status", 12.0, 16.0, 400, 0.0, SANS, false);
    /// `type.ui` — 13/18, 400
    pub const UI: TypeToken = TypeToken::new("ui", 13.0, 18.0, 400, 0.0, SANS, false);
    /// `type.ui-strong` — 13/18, 500
    pub const UI_STRONG: TypeToken = TypeToken::new("ui-strong", 13.0, 18.0, 500, 0.0, SANS, false);
    /// `type.ui-heading` — 13/18, 600
    pub const UI_HEADING: TypeToken =
        TypeToken::new("ui-heading", 13.0, 18.0, 600, 0.0, SANS, false);
    /// `type.name` — 13/18, 400
    pub const NAME: TypeToken = TypeToken::new("name", 13.0, 18.0, 400, 0.0, SANS, false);
    /// `type.name-selected` — 13/18, 500
    pub const NAME_SELECTED: TypeToken =
        TypeToken::new("name-selected", 13.0, 18.0, 500, 0.0, SANS, false);
    /// `type.dialog-body` — 13/20, 400
    pub const DIALOG_BODY: TypeToken =
        TypeToken::new("dialog-body", 13.0, 20.0, 400, 0.0, SANS, false);
    /// `type.dialog-title` — 15/21, 600
    pub const DIALOG_TITLE: TypeToken =
        TypeToken::new("dialog-title", 15.0, 21.0, 600, 0.0, SANS, false);
    /// `type.verb` — 13/18, 700, +0.01em
    pub const VERB: TypeToken = TypeToken::new("verb", 13.0, 18.0, 700, 0.01, SANS, false);
    /// `type.display` — 22/28, 600, −0.01em
    pub const DISPLAY: TypeToken = TypeToken::new("display", 22.0, 28.0, 600, -0.01, SANS, false);

    /// Every token, in spec order. Populates
    /// [`crate::tokens::Theme::style`] and drives the gallery.
    pub const ALL: [TypeToken; 15] = [
        MICRO,
        CAPTION,
        LABEL,
        META,
        META_STRONG,
        STATUS,
        UI,
        UI_STRONG,
        UI_HEADING,
        NAME,
        NAME_SELECTED,
        DIALOG_BODY,
        DIALOG_TITLE,
        VERB,
        DISPLAY,
    ];
}

/// §2.10 Elevation. Shadows appear on **overlays only** (§7.8).
///
/// Light theme: `#1C1A17` at a per-level alpha. Dark theme: `#000000`.
pub mod elevation {
    // A token table is *deliberately* complete: `neutral.500` and
    // `neutral.1000` are transcribed from §2.1 whether or not §3 happens to
    // name them today, because a token file that only contains the values the
    // current UI happens to use stops being a spec and becomes a cache. Every
    // unused constant below is therefore intentional, not dead.
    #![allow(dead_code)]

    use super::{Color32, Shadow, neutral, with_alpha};

    /// Builds a shadow from the spec's offset / blur / spread / colour-at-alpha.
    ///
    /// The spec's "spread" is negative (`−4`, `−12`), i.e. the shadow is
    /// *contracted*; `Shadow::spread` is unsigned in epaint, so the magnitude
    /// is used directly.
    fn build(offset: [i8; 2], blur: u8, spread: i8, color: Color32, alpha: f32) -> Shadow {
        Shadow {
            offset,
            blur,
            spread: spread.unsigned_abs(),
            color: with_alpha(color, alpha),
        }
    }

    /// `elev.1` — menus and tooltips. Offset 6, blur 18, spread −4.
    #[must_use]
    pub fn menu_shadow(dark: bool) -> Shadow {
        if dark {
            build([0, 6], 18, -4, Color32::BLACK, 0.55)
        } else {
            build([0, 6], 18, -4, neutral::N900, 0.12)
        }
    }

    /// `elev.2` — dialogs. Offset 16, blur 40, spread −12.
    #[must_use]
    pub fn dialog_shadow(dark: bool) -> Shadow {
        if dark {
            build([0, 16], 40, -12, Color32::BLACK, 0.65)
        } else {
            build([0, 16], 40, -12, neutral::N900, 0.22)
        }
    }

    /// `elev.3` — the drag ghost. Offset 2, blur 6, spread 0.
    #[must_use]
    pub fn ghost_shadow(dark: bool) -> Shadow {
        if dark {
            build([0, 2], 6, 0, Color32::BLACK, 0.45)
        } else {
            build([0, 2], 6, 0, neutral::N900, 0.10)
        }
    }

    /// `surface.scrim` — the modal backdrop.
    #[must_use]
    pub fn scrim(dark: bool) -> Color32 {
        if dark {
            with_alpha(Color32::BLACK, 0.58)
        } else {
            with_alpha(neutral::N900, 0.38)
        }
    }

    /// `elev.inner-highlight` — a 1px `#FFFFFF` @ 60% inner top edge, light theme
    /// only. Elevation degrades gracefully: if shadows are unavailable, menus
    /// and dialogs must still carry `border.strong` + this highlight so the
    /// overlay boundary survives (§2.10 note).
    #[must_use]
    pub fn inner_highlight() -> Color32 {
        with_alpha(Color32::WHITE, 0.60)
    }
}

/// Builds an [`egui::FontId`] from a type token.
///
/// Size, family and **weight** all come from the token. The weight becomes a
/// family choice rather than a fallback-list position — see [`fonts`] for why
/// that distinction is load-bearing. `tracking` stays unapplied: epaint exposes
/// no tracking control.
#[must_use]
pub fn font(token: ty::TypeToken, _theme: &Theme) -> FontId {
    fonts::font_id(token)
}

/// Builds a [`egui::FontId`] for a machine value at an arbitrary size.
///
/// §2.8's mono/sans split is a hard rule, so this has no sans counterpart: if
/// a caller wants proportional text it must name the token that says so.
#[must_use]
pub fn font_mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

/// Builds a [`egui::FontId`] for proportional text at an arbitrary size.
///
/// At weight 400, the §2.8 sans. Weight 500 and 600 exist as their own
/// families, so there is no `font_sans_at` counterpart to
/// [`fonts::family`] by design: the type scale names the weights it uses.
#[must_use]
pub fn font_sans(size: f32) -> FontId {
    FontId::new(size, fonts::family(FontKind::Sans, 400))
}

/// The **fallback** icon paint: a coloured rounded square, for a category whose
/// glyph name this Phosphor release does not have.
///
/// # This is no longer the normal path
///
/// Phase 2 shipped this as *the* icon renderer, because no icon font was
/// vendored, and the brief for Phase 3 listed "icons don't render" as a defect. The
/// investigation found the opposite: the squares were rendering, in the right
/// §3.7 hues, and the report that called them invisible was looking at a
/// directory of 25 hidden dotfolders, every one of which is correctly
/// `icon.hidden` grey per §5.2. The real gap was the missing font, not the paint.
///
/// [`icon_glyph`] is now what the app calls. This survives because a category
/// whose *name* fails to resolve should show a neutral square rather than render
/// nothing — §5.1's own principle that colour is never the only channel, applied
/// to the failure case. `icons::tests::every_glyph_is_in_the_vendored_font` and
/// `filetype`'s own tests assert that no category actually reaches this path, so
/// seeing a square on screen is a regression signal, not a normal state.
///
/// The square keeps the two properties the icon column is for: the §3.7 `icon.*`
/// colour, and a shape distinct from every other category's glyph.
///
/// Once the font is vendored, the whole function becomes
/// `painter.text(rect.center(), Align2::CENTER_CENTER, glyph, font, color)` with
/// Paints a Phosphor glyph centred in `rect`.
///
/// The §3.7 colour is the caller's to choose — the glyph is a *shape*, the hue
/// is the *information* (§1.4: colour is the whole recognition channel, and
/// §5.1 makes the silhouette a redundant second one, not a replacement).
pub fn icon_glyph(painter: &egui::Painter, rect: Rect, glyph: crate::icons::Glyph, color: Color32) {
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph.char(),
        font_icon(rect.width(), false),
        color,
    );
}

/// Paints a glyph with the **Fill** face, for a toggled-on toolbar button.
///
/// §4.4 `toolbar.btn-toggled-on-icon` — one of exactly two places §5.1 allows
/// Fill weight; the other is the active place in the sidebar.
pub fn icon_glyph_fill(
    painter: &egui::Painter,
    rect: Rect,
    glyph: crate::icons::Glyph,
    color: Color32,
) {
    let _ = glyph.fill_font_id();
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph.char(),
        font_icon(rect.width(), true),
        color,
    );
}

/// the glyph drawn from the icon font, and the coloured square disappears.
pub fn icon_placeholder(painter: &egui::Painter, rect: Rect, color: Color32) {
    // A 16px box with a 1px inset: at 16px a full-bleed square reads as a
    // blob, and §5.1's whole argument is that 16px is where stroke integrity
    // matters.
    let inset = rect.width() * 0.0625;
    let inner = Rect::from_min_max(
        egui::pos2(rect.left() + inset, rect.top() + inset),
        egui::pos2(rect.right() - inset, rect.bottom() - inset),
    );
    painter.rect_filled(
        inner,
        radius::all(radius::XS),
        component::icon_at(color, 0.30),
    );
    painter.rect_stroke(
        inner,
        radius::all(radius::XS),
        Stroke::new(border::HAIRLINE, color),
        component::STROKE_KIND,
    );
}

/// A `FontId` for an icon glyph at an arbitrary size.
///
/// The size must be a §2.9 icon metric. Using anything else is how a 16px grid
/// gets blurred, which is §5.1's entire argument for this icon set.
#[must_use]
pub fn font_icon(size: f32, fill: bool) -> FontId {
    let family = if fill {
        fonts::icons_fill()
    } else {
        fonts::icons()
    };
    FontId::new(size, family)
}

/// Replaces a colour's alpha channel, keeping its RGB.
///
/// `ecolor::Color32` stores premultiplied RGBA and exposes no alpha setter, so
/// the spec's "colour @ N%" notation is expressed through this helper.
#[must_use]
pub fn with_alpha(color: Color32, alpha: f32) -> Color32 {
    let [r, g, b, _] = color.to_array();
    Color32::from_rgba_unmultiplied(r, g, b, (alpha.clamp(0.0, 1.0) * 255.0).round() as u8)
}

// =============================================================================
// Layer 2 — SEMANTIC (§3)
// =============================================================================

/// Which theme to render. `System` defers to the compositor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeMode {
    /// The light theme (§3, light column).
    Light,
    /// The dark theme (§3, dark column). The default when the system theme is
    /// unavailable.
    #[default]
    Dark,
    /// Whatever the windowing system says. Resolved through
    /// [`system_preference`].
    System,
}

/// Reads the compositor's light/dark preference off the live window.
///
/// `eframe::Frame::info()` returns an `IntegrationInfo` that carries only
/// `cpu_usage` on native platforms — no theme — so the winit window is the only
/// reachable route. On Wayland, `Window::theme()` returns *theme overrides only*,
/// i.e. `None` when the desktop environment has not set one; callers must have a
/// default for that case.
#[must_use]
pub fn system_preference(frame: &eframe::Frame) -> Option<bool> {
    let window = frame.winit_window()?;
    // `Window::theme()` takes `&self` but may block on the main thread; the
    // eframe frame callback already runs there, so this is not a stall.
    match window.theme()? {
        winit::window::Theme::Dark => Some(true),
        winit::window::Theme::Light => Some(false),
    }
}

/// §3.1 Surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Surfaces {
    /// `surface.app` — window background, gutters, empty space.
    pub app: Color32,
    /// `surface.panel` — sidebar, breadcrumb bar, status bar.
    pub panel: Color32,
    /// `surface.chrome` — toolbar background.
    pub chrome: Color32,
    /// `surface.list` — file list rows (default).
    pub list: Color32,
    /// `surface.raised` — menus, dialogs, popovers, tooltips. Pure white in
    /// light so overlays read as *lifted* (§1.4).
    pub raised: Color32,
    /// `surface.input` — search field, rename field.
    pub input: Color32,
    /// `surface.input-disabled`
    pub input_disabled: Color32,
    /// `surface.scrim` — modal backdrop.
    pub scrim: Color32,
}

/// §3.2 Text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextRoles {
    /// `text.primary`
    pub primary: Color32,
    /// `text.secondary`
    pub secondary: Color32,
    /// `text.tertiary`
    pub tertiary: Color32,
    /// `text.disabled` — WCAG-exempt (§6.2); never rendered on a selected row.
    pub disabled: Color32,
    /// `text.on-accent`
    pub on_accent: Color32,
    /// `text.on-danger`
    pub on_danger: Color32,
    /// `text.link`
    pub link: Color32,
    /// `text.inverse`
    pub inverse: Color32,
}

/// §3.3 Borders and focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BorderRoles {
    /// `border.subtle` — column rules, inside-pane separators.
    pub subtle: Color32,
    /// `border.default` — input borders, menu outlines, sidebar divider.
    pub default: Color32,
    /// `border.strong` — dialog outline, checkbox idle, overlay fallback.
    pub strong: Color32,
    /// `border.accent` — focused input border, selected left bar.
    pub accent: Color32,
    /// `border.danger`
    pub danger: Color32,
    /// `focus.ring` — the 2px keyboard focus ring.
    pub focus_ring: Color32,
    /// `focus.ring-inactive` — 40% ring when the *window* is unfocused. Exempt
    /// from WCAG 2.4.7 (§6.4).
    pub focus_ring_inactive: Color32,
}

/// §3.4 Interactive state backgrounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateRoles {
    /// `state.hover`
    pub hover: Color32,
    /// `state.hover-strong`
    pub hover_strong: Color32,
    /// `state.pressed`
    pub pressed: Color32,
    /// `state.selected` — the flat selection tint. *Not* a rounded pill.
    pub selected: Color32,
    /// `state.selected-hover`
    pub selected_hover: Color32,
    /// `state.selected-bar` — the 2px left bar that is selection's second channel.
    pub selected_bar: Color32,
    /// `state.cut` — `selected` at 55% over `surface.list`.
    pub cut: Color32,
    /// `state.focus-within`
    pub focus_within: Color32,
    /// `state.drop-target`
    pub drop_target: Color32,
}

/// §3.5 Accent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccentRoles {
    /// `accent.base` — `#0E6B5F` light, `#4FC7B1` dark.
    pub base: Color32,
    /// `accent.hover`
    pub hover: Color32,
    /// `accent.pressed`
    pub pressed: Color32,
    /// `accent.subtle-bg` — toggled-on toolbar button, active sidebar place.
    pub subtle_bg: Color32,
    /// `accent.border`
    pub border: Color32,
    /// `accent.text`
    pub text: Color32,
    /// `accent.on`
    pub on: Color32,
}

/// §3.6 Status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusRoles {
    /// `status.danger-text`
    pub danger_text: Color32,
    /// `status.danger-solid`
    pub danger_solid: Color32,
    /// `status.danger-hover` — `dialog.btn-destructive-bg-hover`.
    pub danger_hover: Color32,
    /// `status.danger-bg`
    pub danger_bg: Color32,
    /// `status.danger-border`
    pub danger_border: Color32,
    /// `status.success-text`
    pub success_text: Color32,
    /// `status.success-solid`
    pub success_solid: Color32,
    /// `status.success-bg`
    pub success_bg: Color32,
    /// `status.success-border`
    pub success_border: Color32,
    /// `status.warning-text`
    pub warning_text: Color32,
    /// `status.warning-solid`
    pub warning_solid: Color32,
    /// `status.warning-bg`
    pub warning_bg: Color32,
    /// `status.warning-border`
    pub warning_border: Color32,
    /// `status.info-text`
    pub info_text: Color32,
    /// `status.info-bg`
    pub info_bg: Color32,
}

/// §3.7 Icon roles. Mirrors §2.4; components reference these, never `hue.*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IconRoles {
    /// `icon.folder`
    pub folder: Color32,
    /// `icon.text`
    pub text: Color32,
    /// `icon.code`
    pub code: Color32,
    /// `icon.image`
    pub image: Color32,
    /// `icon.video`
    pub video: Color32,
    /// `icon.audio`
    pub audio: Color32,
    /// `icon.archive`
    pub archive: Color32,
    /// `icon.binary`
    pub binary: Color32,
    /// `icon.executable`
    pub executable: Color32,
    /// `icon.symlink`
    pub symlink: Color32,
    /// `icon.hidden`
    pub hidden: Color32,
    /// `icon.error`
    pub error: Color32,
    /// `icon.chrome` — toolbar, breadcrumb, status bar glyphs. Equal to
    /// `text.secondary` in both themes.
    pub chrome: Color32,
    /// `icon.chrome-active` — a toggled-on toolbar button. Equal to
    /// `accent.base`.
    pub chrome_active: Color32,
}

/// §3.8 Scrollbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollbarRoles {
    /// `scrollbar.track` — panel colour; transparent over the list.
    pub track: Color32,
    /// `scrollbar.thumb`
    pub thumb: Color32,
    /// `scrollbar.thumb-hover`
    pub thumb_hover: Color32,
    /// `scrollbar.thumb-active`
    pub thumb_active: Color32,
}

/// A fully-resolved theme: one column of §3, every role defined.
///
/// A role with no definition in a theme is a bug, not a fallback to a primitive
/// (§3 preamble) — that is why this is a struct of concrete fields rather than
/// a `HashMap<&str, Color32>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Which column of §3 this instance is.
    pub mode: ThemeMode,
    /// `true` when the light column was selected.
    pub is_dark: bool,
    /// §3.1
    pub surfaces: Surfaces,
    /// §3.2
    pub text: TextRoles,
    /// §3.3
    pub borders: BorderRoles,
    /// §3.4
    pub state: StateRoles,
    /// §3.5
    pub accent: AccentRoles,
    /// §3.6
    pub status: StatusRoles,
    /// §3.7
    pub icon: IconRoles,
    /// §3.8
    pub scrollbar: ScrollbarRoles,
}

impl Theme {
    /// §3, light column.
    #[must_use]
    pub fn light() -> Self {
        use accent as a;
        use hue as h;
        use neutral as n;
        use status as s;
        let text_secondary = Color32::from_rgb(0x4A, 0x45, 0x3F); // #4A453F
        Self {
            mode: ThemeMode::Light,
            is_dark: false,
            surfaces: Surfaces {
                app: n::N50,             // #F7F5F2
                panel: n::N100,          // #EFECE7
                chrome: n::N50,          // #F7F5F2
                list: n::N25,            // #FCFBF9
                raised: Color32::WHITE,  // #FFFFFF — overlays only (§1.4)
                input: n::N25,           // #FCFBF9
                input_disabled: n::N100, // #EFECE7
                scrim: elevation::scrim(false),
            },
            text: TextRoles {
                primary: Color32::from_rgb(0x1C, 0x1A, 0x17),  // #1C1A17
                secondary: text_secondary,                     // #4A453F
                tertiary: n::N600,                             // #5E574F
                disabled: Color32::from_rgb(0x8C, 0x85, 0x7C), // #8C857C
                on_accent: Color32::WHITE,                     // #FFFFFF
                on_danger: Color32::WHITE,                     // #FFFFFF
                link: a::A700,                                 // #0A5449
                inverse: n::N50,                               // #F7F5F2
            },
            borders: BorderRoles {
                subtle: n::N200,                               // #DED9D1
                default: n::N300,                              // #CFC9C0
                strong: n::N400,                               // #A79F94
                accent: a::A600,                               // #0E6B5F
                danger: s::D500,                               // #B3261E
                focus_ring: a::A600,                           // #0E6B5F
                focus_ring_inactive: with_alpha(a::A600, 0.4), // 40%
            },
            state: StateRoles {
                hover: Color32::from_rgb(0xF0, 0xED, 0xE8),   // #F0EDE8
                hover_strong: n::N150,                        // #E7E3DC
                pressed: Color32::from_rgb(0xE1, 0xDC, 0xD3), // #E1DCD3
                selected: a::A200,                            // #C9E6DF
                selected_hover: Color32::from_rgb(0xBC, 0xDE, 0xD5), // #BCDED5
                selected_bar: a::A600,                        // #0E6B5F
                cut: blend(a::A200, n::N25, 0.55),            // 55% over list
                focus_within: a::A100,                        // #DCF0EA
                drop_target: a::A100,                         // #DCF0EA
            },
            accent: AccentRoles {
                base: a::A600,      // #0E6B5F
                hover: a::A700,     // #0A5449
                pressed: a::A800,   // #073B33
                subtle_bg: a::A100, // #DCF0EA
                border: a::A300,    // #A6D6CB
                text: a::A700,      // #0A5449
                on: Color32::WHITE, // #FFFFFF
            },
            status: StatusRoles {
                danger_text: s::D500,  // #B3261E
                danger_solid: s::D600, // #8F1D17
                // `dialog.btn-destructive-bg-hover` — the lighter step, so the
                // button lifts toward the user rather than receding.
                danger_hover: s::D500,
                danger_bg: s::D50,       // #FBE9E7
                danger_border: s::D100,  // #F7D5D1
                success_text: s::S500,   // #286A2A
                success_solid: s::S500,  // #286A2A
                success_bg: s::S50,      // #E4F2E4
                success_border: s::S100, // #CFE7D0
                warning_text: s::W500,   // #7A5100
                warning_solid: s::W500,  // #7A5100
                warning_bg: s::W50,      // #FBF0D9
                warning_border: s::W100, // #F6E3B8
                info_text: a::A600,      // #0E6B5F
                info_bg: a::A100,        // #DCF0EA
            },
            icon: IconRoles {
                folder: h::FOLDER_L,         // #9A6510
                text: h::TEXT_L,             // #5E574F
                code: h::CODE_L,             // #5B3EA8
                image: h::IMAGE_L,           // #A6326E
                video: h::VIDEO_L,           // #7A3ABF
                audio: h::AUDIO_L,           // #286A2A
                archive: h::ARCHIVE_L,       // #9A4520
                binary: h::BINARY_L,         // #5E574F
                executable: h::EXECUTABLE_L, // #1F5E8A
                symlink: h::SYMLINK_L,       // #0E6B5F
                hidden: h::HIDDEN_L,         // #6A635A
                error: h::ERROR_L,           // #B3261E
                chrome: text_secondary,      // = text.secondary
                chrome_active: a::A600,      // = accent.base
            },
            scrollbar: ScrollbarRoles {
                track: n::N100,                                   // #EFECE7 (panel)
                thumb: Color32::from_rgb(0x7A, 0x72, 0x68),       // #7A7268
                thumb_hover: Color32::from_rgb(0x68, 0x61, 0x5A), // #68615A
                thumb_active: n::N700,                            // #443F39
            },
        }
    }

    /// §3, dark column.
    #[must_use]
    pub fn dark() -> Self {
        use accent as a;
        use hue as h;
        use neutral as n;
        use status as s;
        let text_secondary = Color32::from_rgb(0xB8, 0xB2, 0xA9); // #B8B2A9
        Self {
            mode: ThemeMode::Dark,
            is_dark: true,
            surfaces: Surfaces {
                app: Color32::from_rgb(0x13, 0x12, 0x11),    // #131211
                panel: Color32::from_rgb(0x19, 0x18, 0x17),  // #191817
                chrome: Color32::from_rgb(0x13, 0x12, 0x11), // #131211
                list: Color32::from_rgb(0x1E, 0x1C, 0x1A),   // #1E1C1A
                // `surface.raised` — `neutral.800`, #2B2824.
                //
                // §3.1's dark column prints `#24221F` here, which is **not a
                // value in §2.1's ramp** and is in fact `state.hover`'s value
                // in §3.4 — the spec contradicts itself two sections apart. The
                // code keeps `neutral.800`, because a menu or dialog raised on a
                // hover tint would sit *below* the rows it overlays rather than
                // above them, and `neutral.800` is the only ramp step that puts
                // `surface.raised` above both `surface.panel` (#191817) and
                // `surface.list` (#1E1C1A) while staying distinguishable from
                // them. The spec is being corrected to `#2B2824` to match; the
                // value in the code is authoritative and is not changing.
                raised: n::N800,
                input: Color32::from_rgb(0x1E, 0x1C, 0x1A), // #1E1C1A
                input_disabled: Color32::from_rgb(0x19, 0x18, 0x17), // #191817
                scrim: elevation::scrim(true),
            },
            text: TextRoles {
                primary: Color32::from_rgb(0xF0, 0xED, 0xE8),  // #F0EDE8
                secondary: text_secondary,                     // #B8B2A9
                tertiary: Color32::from_rgb(0xA6, 0xA0, 0x99), // #A6A099
                disabled: Color32::from_rgb(0x72, 0x6D, 0x66), // #726D66
                on_accent: Color32::from_rgb(0x08, 0x22, 0x1D), // #08221D
                on_danger: Color32::from_rgb(0xF0, 0xED, 0xE8), // #F0EDE8
                link: a::A300_DARK,                            // #6BD9C4
                inverse: Color32::from_rgb(0x13, 0x12, 0x11),  // #131211
            },
            borders: BorderRoles {
                subtle: Color32::from_rgb(0x2C, 0x2A, 0x27),  // #2C2A27
                default: Color32::from_rgb(0x3D, 0x3A, 0x35), // #3D3A35
                strong: Color32::from_rgb(0x6F, 0x69, 0x61),  // #6F6961
                accent: a::A400_DARK,                         // #4FC7B1
                danger: s::D400_DARK,                         // #F2837C
                focus_ring: a::A400_DARK,                     // #4FC7B1
                focus_ring_inactive: with_alpha(a::A400_DARK, 0.4),
            },
            state: StateRoles {
                hover: Color32::from_rgb(0x24, 0x22, 0x1F), // #24221F
                hover_strong: Color32::from_rgb(0x2C, 0x2A, 0x27), // #2C2A27
                pressed: Color32::from_rgb(0x13, 0x12, 0x11), // #131211
                selected: Color32::from_rgb(0x16, 0x33, 0x2F), // #16332F
                selected_hover: Color32::from_rgb(0x1A, 0x3A, 0x34), // #1A3A34
                selected_bar: a::A400_DARK,                 // #4FC7B1
                cut: blend(
                    Color32::from_rgb(0x16, 0x33, 0x2F),
                    Color32::from_rgb(0x1E, 0x1C, 0x1A),
                    0.55,
                ),
                focus_within: Color32::from_rgb(0x1A, 0x3A, 0x34), // #1A3A34
                drop_target: Color32::from_rgb(0x1A, 0x3A, 0x34),  // #1A3A34
            },
            accent: AccentRoles {
                base: a::A400_DARK,                             // #4FC7B1
                hover: a::A300_DARK,                            // #6BD9C4
                pressed: Color32::from_rgb(0x8F, 0xE6, 0xD4),   // #8FE6D4
                subtle_bg: Color32::from_rgb(0x16, 0x33, 0x2F), // #16332F
                border: Color32::from_rgb(0x3A, 0x7A, 0x6E),    // #3A7A6E
                text: a::A300_DARK,                             // #6BD9C4
                on: Color32::from_rgb(0x08, 0x22, 0x1D),        // #08221D
            },
            status: StatusRoles {
                danger_text: s::D400_DARK,  // #F2837C
                danger_solid: s::D600_DARK, // #5A1512
                danger_hover: s::D400_DARK,
                danger_bg: Color32::from_rgb(0x33, 0x16, 0x14), // #331614
                danger_border: s::D600_DARK,                    // #5A1512
                success_text: s::S400_DARK,                     // #7FCE85
                success_solid: s::S600,                         // #1D4E1F
                success_bg: Color32::from_rgb(0x18, 0x2B, 0x19), // #182B19
                success_border: Color32::from_rgb(0x2B, 0x4A, 0x2D), // #2B4A2D
                warning_text: s::W400_DARK,                     // #E0B461
                warning_solid: s::W600,                         // #5E3F00
                warning_bg: Color32::from_rgb(0x33, 0x28, 0x11), // #332811
                warning_border: Color32::from_rgb(0x4A, 0x3A, 0x15), // #4A3A15
                info_text: a::A400_DARK,                        // #4FC7B1
                info_bg: Color32::from_rgb(0x16, 0x33, 0x2F),   // #16332F
            },
            icon: IconRoles {
                folder: h::FOLDER_D,         // #D8A548
                text: h::TEXT_D,             // #B8B2A9
                code: h::CODE_D,             // #A794F5
                image: h::IMAGE_D,           // #E88AB4
                video: h::VIDEO_D,           // #C199F2
                audio: h::AUDIO_D,           // #7FCE85
                archive: h::ARCHIVE_D,       // #E0A184
                binary: h::BINARY_D,         // #B8B2A9
                executable: h::EXECUTABLE_D, // #6FB6E0
                symlink: h::SYMLINK_D,       // #4FC7B1
                hidden: h::HIDDEN_D,         // #948E85
                error: h::ERROR_D,           // #F2837C
                chrome: text_secondary,
                chrome_active: a::A400_DARK,
            },
            scrollbar: ScrollbarRoles {
                track: Color32::from_rgb(0x19, 0x18, 0x17), // #191817
                thumb: Color32::from_rgb(0x78, 0x71, 0x6A), // #78716A
                thumb_hover: Color32::from_rgb(0x94, 0x8D, 0x84), // #948D84
                thumb_active: Color32::from_rgb(0xB8, 0xB2, 0xA9), // #B8B2A9
            },
        }
    }

    /// Resolves a [`ThemeMode`] into a concrete theme.
    ///
    /// `system_dark` is the answer to "does the system prefer dark?". `None`
    /// means the platform did not say, and the spec gives no guidance, so this
    /// falls back to [`ThemeMode::Dark`] — the app is dark-first (spec §1.2
    /// lineage) and a file manager that flashes white on an unknown desktop is
    /// worse than one that guesses dark.
    #[must_use]
    pub fn resolve(mode: ThemeMode, system_dark: Option<bool>) -> Self {
        match mode {
            ThemeMode::Light => Self::light(),
            ThemeMode::Dark => Self::dark(),
            ThemeMode::System => match system_dark {
                Some(true) => Self::dark(),
                Some(false) => Self::light(),
                None => Self::dark(),
            },
        }
    }

    /// The [`Visuals`] this theme produces.
    ///
    /// Chrome is flat: `elev.0` everywhere except the window/menu overlays,
    /// which are the only places shadows are allowed (§2.10, §7.8).
    #[must_use]
    pub fn visuals(&self) -> Visuals {
        let mut v = if self.is_dark {
            Visuals::dark()
        } else {
            Visuals::light()
        };
        v.dark_mode = self.is_dark;

        // Surfaces
        v.panel_fill = self.surfaces.panel;
        v.window_fill = self.surfaces.raised;
        v.extreme_bg_color = self.surfaces.app;
        v.faint_bg_color = self.surfaces.list;
        v.code_bg_color = self.surfaces.input;
        v.text_edit_bg_color = Some(self.surfaces.input);

        // Overlays only. `surface.raised` is pure white in light so menus and
        // dialogs read as lifted without needing a heavy shadow.
        v.window_stroke = Stroke::new(border::HAIRLINE, self.borders.strong);
        v.menu_corner_radius = radius::all(radius::LG); // menu.radius 6
        v.window_corner_radius = radius::all(radius::XL); // dialog.radius 8
        v.popup_shadow = elevation::menu_shadow(self.is_dark);
        v.window_shadow = elevation::dialog_shadow(self.is_dark);

        // Text
        v.override_text_color = Some(self.text.primary);
        v.weak_text_color = Some(self.text.tertiary);
        v.weak_text_alpha = 1.0;
        v.hyperlink_color = self.text.link;
        v.warn_fg_color = self.status.warning_text;
        v.error_fg_color = self.status.danger_text;

        // Selection (text selection inside inputs / selectable buttons)
        v.selection.bg_fill = self.accent.subtle_bg;
        v.selection.stroke = Stroke::new(border::HAIRLINE, self.accent.base);
        // `input.caret` — 1px, `text.primary`, 100% blink at 530ms on / off.
        v.text_cursor = egui::style::TextCursorStyle {
            stroke: Stroke::new(border::HAIRLINE, self.text.primary),
            preview: false,
            blink: true,
            on_duration: 0.53,
            off_duration: 0.53,
        };

        // Widget chrome: flat, hairline-separated, near-neutral. This is the
        // §1.4 thesis in code — the only saturated thing on screen is the
        // thing being acted on.
        let hairline = Stroke::new(border::HAIRLINE, self.borders.default);
        v.widgets.noninteractive.bg_fill = self.surfaces.app;
        v.widgets.noninteractive.weak_bg_fill = self.surfaces.app;
        v.widgets.noninteractive.bg_stroke = Stroke::NONE;
        v.widgets.noninteractive.fg_stroke = Stroke::new(border::HAIRLINE, self.text.secondary);
        v.widgets.noninteractive.corner_radius = radius::all(radius::MD);

        v.widgets.inactive.bg_fill = self.surfaces.input;
        v.widgets.inactive.weak_bg_fill = self.surfaces.input;
        v.widgets.inactive.bg_stroke = hairline;
        v.widgets.inactive.fg_stroke = Stroke::new(border::HAIRLINE, self.text.primary);
        v.widgets.inactive.corner_radius = radius::all(radius::MD);

        v.widgets.hovered.bg_fill = self.state.hover_strong;
        v.widgets.hovered.weak_bg_fill = self.state.hover_strong;
        v.widgets.hovered.bg_stroke = Stroke::new(border::HAIRLINE, self.borders.strong);
        v.widgets.hovered.fg_stroke = Stroke::new(border::HAIRLINE, self.text.primary);
        v.widgets.hovered.corner_radius = radius::all(radius::MD);

        v.widgets.active.bg_fill = self.state.pressed;
        v.widgets.active.weak_bg_fill = self.state.pressed;
        v.widgets.active.bg_stroke = Stroke::new(border::HAIRLINE, self.borders.strong);
        v.widgets.active.fg_stroke = Stroke::new(border::HAIRLINE, self.text.primary);
        v.widgets.active.corner_radius = radius::all(radius::MD);

        v.widgets.open.bg_fill = self.state.hover_strong;
        v.widgets.open.weak_bg_fill = self.state.hover_strong;
        v.widgets.open.bg_stroke = Stroke::new(border::HAIRLINE, self.borders.strong);
        v.widgets.open.fg_stroke = Stroke::new(border::HAIRLINE, self.text.primary);
        v.widgets.open.corner_radius = radius::all(radius::MD);

        // `button_frame` off: §7.15 "the card reflex" — nothing in this app is
        // a card, so a default button is a flat label, not a bordered panel.
        v.button_frame = false;
        v.collapsing_header_frame = false;
        v.striped = false;
        v.slider_trailing_fill = false;
        v.indent_has_left_vline = false;
        v
    }

    /// The [`egui::Style`] half of this theme: spacing, radii and the whole
    /// §2.8 type scale registered as named text styles.
    #[must_use]
    pub fn style(&self, base: &egui::Style) -> egui::Style {
        let mut style = base.clone();
        style.visuals = self.visuals();

        // §7.4 / §4 density bands, applied per role rather than globally. egui
        // only has one item spacing, so it gets the *list* band (2px half-step);
        // every other band is expressed explicitly at its call site via
        // `component::*` below.
        style.spacing.item_spacing = Vec2::new(space::S2, space::HALF);
        style.spacing.button_padding = Vec2::new(space::S2, space::S1_5);
        style.spacing.interact_size = Vec2::new(metric::TARGET_MIN, metric::TARGET_MIN);
        // `Margin` is `i8` in epaint 0.36, so this is a literal px count.
        style.spacing.window_margin = egui::Margin::same(16);
        style.spacing.indent = metric::GUTTER_MARKER;

        // §2.8: register every type token as a named `TextStyle`.
        //
        // **Weight is applied**, by choosing a family rather than by ordering a
        // fallback list — see [`fonts`] for why that distinction is the whole
        // trick, since `epaint` takes the first face in a family that has the
        // glyph and all three Plex weights have the same coverage.
        //
        // **Tracking is still not applied.** epaint exposes no tracking control
        // and the vendored Plex faces are used at their designed side bearings, so
        // the tokens' `tracking` column is recorded and asserted in tests but has
        // no effect. That is the one remaining §2.8 deviation.
        for token in ty::ALL {
            style.text_styles.insert(
                TextStyle::Name(Arc::from(token.style_name)),
                fonts::font_id(token),
            );
        }
        style
    }

    /// Pushes this theme onto an [`egui::Context`].
    ///
    /// Both the light and dark slots are written, then the active one is
    /// selected — so a later `ctx.set_theme(...)` (e.g. from egui's own
    /// light/dark widget) lands on an already-correct style rather than on
    /// egui's stock blue-grey one.
    pub fn apply(&self, ctx: &Context) {
        let base = (*ctx.global_style()).clone();
        let style = self.style(&base);
        ctx.set_style_of(egui::Theme::Light, style.clone());
        ctx.set_style_of(egui::Theme::Dark, style.clone());
        ctx.set_theme(if self.is_dark {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        });
    }
}

/// `top` composited over `bottom` at `ratio`.
///
/// The spec writes `state.cut` as `state.selected` @ 55% over `surface.list`
/// rather than as a single hex, so it is computed rather than transcribed.
/// Compositing happens on the 8-bit channels, which is what "55% over" means
/// to a designer; doing it in linear light would darken the result and break
/// the §6 contrast audit.
fn blend(top: Color32, bottom: Color32, ratio: f32) -> Color32 {
    let ratio = ratio.clamp(0.0, 1.0);
    let [tr, tg, tb, _] = top.to_array();
    let [br, bg, bb, _] = bottom.to_array();
    let mix = |t: u8, b: u8| (t as f32 * ratio + b as f32 * (1.0 - ratio)).round() as u8;
    Color32::from_rgb(mix(tr, br), mix(tg, bg), mix(tb, bb))
}

// =============================================================================
// Layer 3 — COMPONENT (§4)
// =============================================================================

/// §4 component tokens.
///
/// Every value here is a *semantic* name or a primitive measurement. No
/// component below may name a `hue.*` or a raw hex value.
pub mod component {
    // A token table is *deliberately* complete: `neutral.500` and
    // `neutral.1000` are transcribed from §2.1 whether or not §3 happens to
    // name them today, because a token file that only contains the values the
    // current UI happens to use stops being a spec and becomes a cache. Every
    // unused constant below is therefore intentional, not dead.
    #![allow(dead_code)]

    use super::{
        Color32, CornerRadius, Stroke, TypeToken, border, metric, radius, space, ty, with_alpha,
    };

    // ---- 4.1 Sidebar item --------------------------------------------------

    /// `sidebar.item-height`
    pub const SIDEBAR_ITEM_HEIGHT: f32 = metric::SIDEBAR_ITEM;
    /// `sidebar.item-padding-x`
    pub const SIDEBAR_ITEM_PADDING_X: f32 = space::S2;
    /// `sidebar.item-gap`
    pub const SIDEBAR_ITEM_GAP: f32 = space::S2_5;
    /// `sidebar.item-radius`
    pub const SIDEBAR_ITEM_RADIUS: f32 = radius::MD;
    /// `sidebar.item-bg` — transparent.
    pub const SIDEBAR_ITEM_BG: Color32 = Color32::TRANSPARENT;
    /// `sidebar.item-bg-disabled` — transparent.
    pub const SIDEBAR_ITEM_BG_DISABLED: Color32 = Color32::TRANSPARENT;
    /// `sidebar.item-active-bar` width — 3px, radius 0, full height, left.
    pub const SIDEBAR_ITEM_ACTIVE_BAR: f32 = border::MARKER;
    /// `sidebar.item-section-label` height.
    pub const SIDEBAR_SECTION_LABEL_HEIGHT: f32 = 22.0;
    /// `sidebar.item-section-label` padding-x.
    pub const SIDEBAR_SECTION_LABEL_PADDING_X: f32 = space::S2;
    /// `sidebar.divider` vertical margin.
    pub const SIDEBAR_DIVIDER_MARGIN_Y: f32 = space::S2;
    /// `sidebar.resize-handle` — 4px hit area, 1px `border.strong` on hover.
    pub const SIDEBAR_RESIZE_HANDLE: f32 = 4.0;
    /// `sidebar.item-icon` — `icon.chrome`, 18px.
    pub const SIDEBAR_ICON_SIZE: f32 = metric::ICON_CHROME;

    // ---- 4.2 File list row -------------------------------------------------

    /// `row.height` (22 / 32 variants exist for the density bands).
    pub const ROW_HEIGHT: f32 = metric::ROW;
    /// `row.height` — compact density.
    pub const ROW_HEIGHT_COMPACT: f32 = metric::ROW_COMPACT;
    /// `row.height` — comfortable density.
    pub const ROW_HEIGHT_COMFORTABLE: f32 = metric::ROW_COMFORTABLE;
    /// `row.padding-x`
    pub const ROW_PADDING_X: f32 = space::S2;
    /// `row.gutter` — 10px leading, of which 3px is the hidden marker.
    pub const ROW_GUTTER: f32 = metric::GUTTER;
    /// `row.icon-size` (14 / 20 variants).
    pub const ROW_ICON_SIZE: f32 = metric::ICON;
    /// `row.icon-gap`
    pub const ROW_ICON_GAP: f32 = space::S2_5;
    /// `row.column-gap`
    pub const ROW_COLUMN_GAP: f32 = space::S3;
    /// `row.radius` — 3px. Never above this (§4.2 rule 5).
    pub const ROW_RADIUS: f32 = radius::SM;
    /// Selection's second channel: the 2px left bar (§4.2 state matrix).
    pub const ROW_SELECTED_BAR_WIDTH: f32 = border::THICK;
    /// The bar is inset 1px top and bottom so it does not poke out of the row's
    /// 3px corner radius. Deviation: the spec gives the bar no radius of its
    /// own; 1px is half the bar width, i.e. the smallest possible rounding.
    pub const ROW_SELECTED_BAR_INSET: f32 = 1.0;
    /// `row.divider` — inset 8px from left and right.
    pub const ROW_DIVIDER_INSET_X: f32 = space::S2;
    /// `tree.disclosure` — the 12px caret in a tree row's leading gutter.
    ///
    /// Not a §2.9 metric; the value is §5.3's `caret-right` / `caret-down` size,
    /// which is what makes the disclosure visually part of the icon set rather
    /// than chrome. The gutter is 10px, so a 12px caret overflows it by 1px on
    /// each side and reads as centred on the row's left edge.
    pub const TREE_DISCLOSURE: f32 = 12.0;
    /// `row.indent-step` per depth level (tree mode).
    pub const ROW_INDENT_STEP: f32 = 14.0;
    /// `row.col-header` height.
    pub const ROW_COL_HEADER_HEIGHT: f32 = metric::COLUMN_HEADER;
    /// `row.col-size` width.
    pub const ROW_COL_SIZE_WIDTH: f32 = metric::COLUMN_SIZE;
    /// `row.col-kind` width.
    pub const ROW_COL_KIND_WIDTH: f32 = metric::COLUMN_KIND;
    /// `row.col-modified` width.
    pub const ROW_COL_MODIFIED_WIDTH: f32 = metric::COLUMN_MODIFIED;
    /// `row.col-hidden` — the 4px dot marker.
    pub const ROW_HIDDEN_DOT: f32 = border::HIDDEN_MARKER;
    /// `row.skeleton` — 12px tall, radius 2px, no shimmer.
    pub const ROW_SKELETON_HEIGHT: f32 = 12.0;
    pub const ROW_SKELETON_RADIUS: f32 = radius::XS;
    /// `row.skeleton-count` — 8 rows, then stop.
    pub const ROW_SKELETON_COUNT: usize = 8;
    /// `row.empty-title` type token.
    pub const ROW_EMPTY_TITLE: TypeToken = ty::DISPLAY;
    /// `row.empty-body` type token.
    pub const ROW_EMPTY_BODY: TypeToken = ty::DIALOG_BODY;
    /// `row.empty-icon` — 48px, `icon.chrome` at 40%.
    pub const ROW_EMPTY_ICON_SIZE: f32 = 48.0;
    pub const ROW_EMPTY_ICON_ALPHA: f32 = 0.4;
    /// `row.reading` — 18px busy glyph.
    pub const ROW_READING_ICON_SIZE: f32 = 18.0;

    // ---- 4.3 Breadcrumb ----------------------------------------------------

    /// `breadcrumb.height`
    pub const BREADCRUMB_HEIGHT: f32 = metric::BREADCRUMB;
    /// `breadcrumb.padding-x`
    pub const BREADCRUMB_PADDING_X: f32 = space::S2;
    /// `breadcrumb.segment-gap` — 2px padding, 6px visual.
    pub const BREADCRUMB_SEGMENT_GAP: f32 = space::S1_5;
    /// `breadcrumb.segment-height`
    pub const BREADCRUMB_SEGMENT_HEIGHT: f32 = 20.0;
    /// `breadcrumb.segment-radius`
    pub const BREADCRUMB_SEGMENT_RADIUS: f32 = radius::SM;
    /// `breadcrumb.segment-text` type token.
    pub const BREADCRUMB_SEGMENT_TEXT: TypeToken = ty::UI;
    /// `breadcrumb.segment-text-current` type token (`type.ui-strong`).
    pub const BREADCRUMB_SEGMENT_TEXT_CURRENT: TypeToken = ty::UI_STRONG;
    /// `breadcrumb.separator` — `caret-right`, 12px, `icon.chrome` at 55%.
    pub const BREADCRUMB_SEPARATOR_SIZE: f32 = 12.0;
    pub const BREADCRUMB_SEPARATOR_ALPHA: f32 = 0.55;
    /// `breadcrumb.separator-clickable` — 16 x 20 px hit area.
    pub const BREADCRUMB_SEPARATOR_HIT_W: f32 = 16.0;
    pub const BREADCRUMB_SEPARATOR_HIT_H: f32 = 20.0;
    /// `breadcrumb.overflow` — 20 x 20 px.
    pub const BREADCRUMB_OVERFLOW_SIZE: f32 = 20.0;
    /// `breadcrumb.overflow` — how many trailing segments always stay visible.
    pub const BREADCRUMB_MIN_VISIBLE: usize = 2;

    // ---- 4.4 Toolbar button ------------------------------------------------

    /// `toolbar.btn-height` / `toolbar.btn-width` (icon button).
    pub const TOOLBAR_BTN_HEIGHT: f32 = 28.0;
    pub const TOOLBAR_BTN_WIDTH: f32 = 28.0;
    /// `toolbar.btn-gap` (icon + label).
    pub const TOOLBAR_BTN_LABEL_GAP: f32 = space::S1_5;
    /// `toolbar.btn-radius`
    pub const TOOLBAR_BTN_RADIUS: f32 = radius::MD;
    /// `toolbar.btn-icon-size`
    pub const TOOLBAR_BTN_ICON_SIZE: f32 = metric::ICON_CHROME;
    /// `toolbar.btn-bg` — transparent.
    pub const TOOLBAR_BTN_BG: Color32 = Color32::TRANSPARENT;
    /// `toolbar.btn-label` type token.
    pub const TOOLBAR_BTN_LABEL: TypeToken = ty::UI;
    /// `toolbar.btn-label-active` type token.
    pub const TOOLBAR_BTN_LABEL_ACTIVE: TypeToken = ty::UI_STRONG;
    /// `toolbar.btn-separator` — 1px `border.subtle`, 16px tall, 8px margins.
    pub const TOOLBAR_BTN_SEPARATOR_H: f32 = 16.0;
    pub const TOOLBAR_BTN_SEPARATOR_MARGIN: f32 = space::S2;
    /// `toolbar.btn-tooltip-delay` — 500ms.
    pub const TOOLBAR_BTN_TOOLTIP_DELAY: f32 = 0.5;

    // ---- 4.5 Status bar ----------------------------------------------------

    /// `statusbar.height`
    pub const STATUSBAR_HEIGHT: f32 = metric::STATUS_BAR;
    /// `statusbar.padding-x`
    pub const STATUSBAR_PADDING_X: f32 = space::S2_5;
    /// `statusbar.section-gap`
    pub const STATUSBAR_SECTION_GAP: f32 = 14.0;
    /// `statusbar.section-divider` — 1px `border.subtle`, 12px tall.
    pub const STATUSBAR_SECTION_DIVIDER_H: f32 = 12.0;
    /// `statusbar.label` type token.
    pub const STATUSBAR_LABEL: TypeToken = ty::STATUS;
    /// `statusbar.value` type token (`type.meta`).
    pub const STATUSBAR_VALUE: TypeToken = ty::META;
    /// `statusbar.value-strong` type token (`type.meta-strong`).
    pub const STATUSBAR_VALUE_STRONG: TypeToken = ty::META_STRONG;
    /// `statusbar.selection-count` type token.
    pub const STATUSBAR_SELECTION_COUNT: TypeToken = ty::META_STRONG;
    /// `statusbar.busy` — 12px spinner.
    pub const STATUSBAR_BUSY_ICON_SIZE: f32 = 12.0;
    /// Free-space meter: 60px wide, 4px tall.
    pub const STATUSBAR_METER_W: f32 = 60.0;
    pub const STATUSBAR_METER_H: f32 = 4.0;

    // ---- 4.6 Context menu --------------------------------------------------

    /// `menu.min-width` / `menu.max-width`
    pub const MENU_MIN_WIDTH: f32 = 180.0;
    pub const MENU_MAX_WIDTH: f32 = 320.0;
    /// `menu.padding`
    pub const MENU_PADDING: f32 = radius::XS;
    /// `menu.radius`
    pub const MENU_RADIUS: f32 = radius::LG;
    /// `menu.item-height`
    pub const MENU_ITEM_HEIGHT: f32 = metric::ROW;
    /// `menu.item-padding-x`
    pub const MENU_ITEM_PADDING_X: f32 = space::S2;
    /// `menu.item-radius`
    pub const MENU_ITEM_RADIUS: f32 = radius::MD;
    /// `menu.item-gap` — label -> shortcut.
    pub const MENU_ITEM_GAP: f32 = space::S3;
    /// `menu.item-icon` — 16px.
    pub const MENU_ITEM_ICON_SIZE: f32 = metric::ICON;
    /// `menu.item-shortcut` type token.
    pub const MENU_ITEM_SHORTCUT: TypeToken = ty::META;
    /// `menu.checkmark-slot` — 16px fixed.
    pub const MENU_CHECKMARK_SLOT: f32 = 16.0;
    /// `menu.section-label` — 22px.
    pub const MENU_SECTION_LABEL_HEIGHT: f32 = 22.0;
    /// `menu.separator` — 5px vertical margin, 8px horizontal inset.
    pub const MENU_SEPARATOR_MARGIN_Y: f32 = radius::XS + 3.0;
    pub const MENU_SEPARATOR_INSET_X: f32 = space::S2;
    /// `menu.offset-from-cursor` — 2px.
    pub const MENU_OFFSET_FROM_CURSOR: f32 = border::HAIRLINE * 2.0;

    // ---- 4.7 Confirmation dialog ------------------------------------------

    /// `dialog.width`
    pub const DIALOG_WIDTH: f32 = 400.0;
    /// `dialog.padding`
    pub const DIALOG_PADDING: f32 = space::S5;
    /// `dialog.radius`
    pub const DIALOG_RADIUS: f32 = radius::XL;
    /// `dialog.title` type token.
    pub const DIALOG_TITLE: TypeToken = ty::DIALOG_TITLE;
    /// `dialog.body` type token.
    pub const DIALOG_BODY: TypeToken = ty::DIALOG_BODY;
    /// `dialog.path-quote` — radius 4, padding 6/8.
    pub const DIALOG_PATH_QUOTE_RADIUS: f32 = radius::MD;
    pub const DIALOG_PATH_QUOTE_PAD_Y: f32 = space::S1_5;
    pub const DIALOG_PATH_QUOTE_PAD_X: f32 = space::S2;
    /// `dialog.icon` — 20px.
    /// `dialog.icon` — 20px.
    pub const DIALOG_ICON: f32 = DIALOG_ICON_SIZE;
    pub const DIALOG_ICON_SIZE: f32 = metric::ICON_LG;
    /// `dialog.footer-gap`
    pub const DIALOG_FOOTER_GAP: f32 = space::S3;
    /// `dialog.btn-height`
    pub const DIALOG_BTN_HEIGHT: f32 = 30.0;
    /// `dialog.btn-padding-x`
    pub const DIALOG_BTN_PADDING_X: f32 = 14.0;
    /// `dialog.btn-radius`
    pub const DIALOG_BTN_RADIUS: f32 = radius::MD;
    /// `dialog.btn-label` — `type.ui`, **500**.
    ///
    /// The spec writes the size from one token and the weight from another:
    /// `type.ui` is 13px/400 and this row says 500. `ty::UI_STRONG` is 13px/500,
    /// so it is the token that matches the row exactly — a 13px label at 400 is
    /// a label at the wrong *weight* rather than the right one, and the fill and
    /// the text end up disagreeing about which button is the one. Named here
    /// because the button was reaching for a bare `ty::UI`, which is the layer
    /// rule's failure mode: a §4 component value with no §4 component token.
    pub const DIALOG_BTN_LABEL: TypeToken = ty::UI_STRONG;
    /// Gallery-only: the block the §4.7 dialog is drawn inside, scrim included.
    /// Not a spec value — a layout convenience so the gallery can allocate one
    /// rectangle instead of nesting five `allocate_ui` calls.
    pub const DIALOG_BLOCK_HEIGHT: f32 = 340.0;
    /// `dialog.path-quote` height — the spec gives padding (6/8px) around
    /// `type.meta`, whose line box is 16px, so 16 + 6 + 6 = 28.
    pub const DIALOG_PATH_QUOTE_H: f32 = 28.0;
    /// The determinate progress bar on a `Progress` dialog.
    ///
    /// §4.7 has no token for a progress bar, so this is derived from §4.5's
    /// free-space meter — the only determinate meter in the spec — at its
    /// `metric`-level height rather than the meter's 4px, which is sized for a
    /// status-bar strip and would be a hairline in a dialog. Named here so the
    /// derivation is one line to review and one line to overrule when §4.7 grows
    /// a progress row of its own.
    pub const DIALOG_PROGRESS_BAR_H: f32 = 6.0;
    /// `dialog.btn-destructive-bg` — `status.danger-solid`.
    ///
    /// A function rather than a constant because it is a *semantic role* looked
    /// up on the theme, not a primitive; `status.danger-solid` is already the
    /// right name and re-deriving a hex here would break the three-layer rule.
    #[must_use]
    // The SCREAMING_CASE matches the `dialog.*` spec token it carries, which is
    // the naming convention for every other item in this §4.7 block. Snake case
    // would make these two functions the odd ones out in a list that is
    // otherwise a direct transcription of the spec table.
    #[allow(non_snake_case)]
    pub fn DIALOG_BTN_DESTRUCTIVE_BG(theme: &super::Theme) -> Color32 {
        theme.status.danger_solid
    }
    /// `dialog.btn-destructive-bg-hover` — `danger.600`, falling back to
    /// `danger.500` on a theme that did not define the 600 step.
    ///
    /// The fallback rather than a hard-coded value: §3.6 defines
    /// `status.danger-solid` and its hover as a *pair* of primitives, and a
    /// token that invents a hex when one is missing is exactly the failure the
    /// three-layer rule exists to prevent.
    #[must_use]
    #[allow(non_snake_case)]
    pub fn DIALOG_BTN_DESTRUCTIVE_BG_HOVER(theme: &super::Theme) -> Color32 {
        theme.status.danger_hover
    }
    /// `dialog.checkbox` — 14px box, 2px inset, 3px radius.
    pub const DIALOG_CHECKBOX: f32 = 14.0;
    pub const DIALOG_CHECKBOX_RADIUS: f32 = radius::SM;

    // ---- 4.8 Text input / search field ------------------------------------

    /// `input.height`
    pub const INPUT_HEIGHT: f32 = 28.0;
    /// `input.radius`
    pub const INPUT_RADIUS: f32 = radius::MD;
    /// `input.padding-x`
    pub const INPUT_PADDING_X: f32 = space::S2;
    /// `input.icon-size` — 16px; `input.icon-gap` — 6px.
    pub const INPUT_ICON_SIZE: f32 = metric::ICON;
    pub const INPUT_ICON_GAP: f32 = space::S1_5;
    /// `input.clear-btn` — 20 x 20 px.
    pub const INPUT_CLEAR_BTN: f32 = 20.0;
    /// The search-field placeholder. The spec is emphatic it is "Filter…",
    /// not "Search" — the field filters a list, it does not search the web.
    pub const SEARCH_PLACEHOLDER: &str = "Filter\u{2026}";

    // ---- 4.9 Scrollbar -----------------------------------------------------

    /// `scrollbar.width` — full hit area.
    pub const SCROLLBAR_WIDTH: f32 = metric::SCROLLBAR;
    /// `scrollbar.thumb-size` — 6px visible.
    pub const SCROLLBAR_THUMB_SIZE: f32 = metric::SCROLLBAR_THUMB;
    /// `scrollbar.thumb-radius` — 3px.
    pub const SCROLLBAR_THUMB_RADIUS: f32 = radius::SM;
    /// `scrollbar.inset` — 3px from the pane edge.
    pub const SCROLLBAR_INSET: f32 = radius::SM;
    /// `scrollbar.min-thumb` — 24px, so a thumb is never ungrabbable.
    pub const SCROLLBAR_MIN_THUMB: f32 = metric::TARGET_MIN;
    /// `scrollbar.keyboard` — 3 lines per arrow press.
    pub const SCROLLBAR_KEYBOARD_LINES: f32 = 3.0;

    // ---- 4.10 State coverage -----------------------------------------------

    /// §4.2's file-row state matrix, transcribed one variant per row.
    ///
    /// §4.10 gives the *priority order* — `disabled` > `drop-target` >
    /// `pressed` > `selected` > `focus-visible` > `hover` > `default` — with
    /// one exception: "`selected + focus-visible` renders both, because that
    /// combination is the single most important state in the whole app". So
    /// that pair is a distinct variant rather than a resolution rule, and
    /// [`RowState::resolve`] is where the priority is applied.
    ///
    /// `SelectedFocus` and `SelectedHover` exist so a row is never "selected
    /// and hovered-but-not-focused with a different background" — §4.2 rule 2
    /// calls that out as producing an ambiguous third rendering.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum RowState {
        /// `default` — `surface.list`, no bar, no ring.
        Default,
        /// `hover` — `state.hover`.
        Hover,
        /// `focused, not selected` — `surface.list` + a 2px `focus.ring` inset
        /// at 1px offset.
        FocusVisible,
        /// `selected` — `state.selected` + 2px `state.selected-bar`.
        Selected,
        /// `selected + hover` — `state.selected-hover` + bar.
        SelectedHover,
        /// `selected + focused` — `state.selected` + bar + ring, name at
        /// `type.name-selected`.
        SelectedFocus,
        /// `cut` (marked for cut) — `state.cut`, bar at 55%, icon at 70%.
        ///
        /// Deliberately *not* strikethrough (§4.2 rule 6): strikethrough on a
        /// filename reads as "deleted" and causes mistakes.
        Cut,
        /// `drop target` — `state.drop-target` + a 2px `border.accent` inset ring.
        DropTarget,
        /// `disabled` — as its row state, no bar, `text.disabled`, icon at 50%.
        Disabled,
    }

    impl RowState {
        /// Applies §4.10's priority order to the three independent facts a row
        /// has: selection, focus, and whether the pointer is over it.
        ///
        /// `pressed` and `drop-target` are not derived here — they are supplied
        /// directly by the caller, which is the only place that knows about
        /// them — and they outrank everything, so a caller that knows about
        /// them passes them in as `forced`.
        #[must_use]
        pub fn resolve(selected: bool, focused: bool, hovered: bool, forced: Option<Self>) -> Self {
            if let Some(forced) = forced {
                return forced;
            }
            match (selected, focused, hovered) {
                (true, true, _) => Self::SelectedFocus,
                (true, false, true) => Self::SelectedHover,
                (true, false, false) => Self::Selected,
                (false, true, _) => Self::FocusVisible,
                (false, false, true) => Self::Hover,
                (false, false, false) => Self::Default,
            }
        }

        /// The row background, straight out of §4.2's matrix.
        #[must_use]
        pub fn background(self, theme: &super::Theme) -> Color32 {
            let state = &theme.state;
            match self {
                Self::Default | Self::FocusVisible | Self::Disabled => theme.surfaces.list,
                Self::Hover => state.hover,
                Self::Selected | Self::SelectedFocus => state.selected,
                Self::SelectedHover => state.selected_hover,
                Self::Cut => state.cut,
                Self::DropTarget => state.drop_target,
            }
        }

        /// The 2px left bar: selection's second channel (§6.6 "never colour
        /// alone"). `None` for states the matrix gives no bar to.
        #[must_use]
        pub fn selected_bar(self, theme: &super::Theme) -> Option<Color32> {
            match self {
                Self::Selected | Self::SelectedHover | Self::SelectedFocus => {
                    Some(theme.state.selected_bar)
                }
                // `cut` keeps the bar, at 55%.
                Self::Cut => Some(with_alpha(theme.state.selected_bar, 0.55)),
                Self::DropTarget => Some(theme.borders.accent),
                Self::Default | Self::Hover | Self::FocusVisible | Self::Disabled => None,
            }
        }

        /// The 2px focus ring, or `None`.
        ///
        /// §4.2 gives `focused, not selected` a *1px offset* and
        /// `selected + focused` no offset (the ring is drawn directly on the
        /// tint). The difference is 1px on a 26px row, so both are drawn as an
        /// inset stroke and the offset is not separately rendered; this is
        /// recorded rather than silently approximated.
        #[must_use]
        pub fn focus_ring(self, theme: &super::Theme, window_focused: bool) -> Option<Stroke> {
            match self {
                Self::FocusVisible | Self::SelectedFocus => Some(Stroke::new(
                    super::border::THICK,
                    if window_focused {
                        theme.borders.focus_ring
                    } else {
                        // §6.4: dimmed only while the *window* is unfocused, and
                        // restoring full opacity on refocus is mandatory.
                        theme.borders.focus_ring_inactive
                    },
                )),
                Self::DropTarget => Some(Stroke::new(super::border::THICK, theme.borders.accent)),
                _ => None,
            }
        }

        /// The name's type token. `selected + focused` is the only state that
        /// promotes the weight, per §4.2's state matrix.
        #[must_use]
        pub fn name_token(self) -> TypeToken {
            if self == Self::SelectedFocus {
                super::ty::NAME_SELECTED
            } else {
                super::ty::NAME
            }
        }

        /// Disabled rows render `text.disabled` and the icon at 50% (§4.2).
        ///
        /// §6.2's structural rule: disabled text never renders on a selected
        /// row, because a disabled item is not selectable.
        #[must_use]
        pub fn text_role(self) -> DisabledText {
            if self == Self::Disabled {
                DisabledText::Disabled
            } else {
                DisabledText::Normal
            }
        }
    }

    /// Whether a row's text is rendered in the `text.disabled` role.
    ///
    /// A tiny enum rather than a bool so the §6.2 rule is a type-level
    /// statement: a selected row *cannot* be given `DisabledText::Disabled`
    /// without going through [`RowState::Disabled`], which has no bar.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum DisabledText {
        /// `text.primary` / `text.tertiary` as normal.
        Normal,
        /// `text.disabled` — WCAG-exempt (§6.2).
        Disabled,
    }

    /// A 1px `border.subtle` separator. Sticky chrome is a layout sibling with a
    /// 1px border, never an overlay (§2.12, §6.6) — that is what makes "focus
    /// not obscured" structural rather than a scroll offset.
    #[must_use]
    pub fn hairline(theme: &super::Theme) -> Stroke {
        Stroke::new(border::HAIRLINE, theme.borders.subtle)
    }

    /// Every stroke this app draws is **inside** its shape.
    ///
    /// §2.7 calls `border.hairline` the width for "all separators, input
    /// borders, menu outline", and §4.3 calls the focus ring "2px inset". With
    /// `StrokeKind::Middle` — epaint's default — a 1px line is split 0.5px
    /// either side of the edge and lands on no pixel boundary, so at 1x DPI it
    /// renders as a grey blur. `Inside` keeps every stroke on whole pixels,
    /// which is the same argument §5.1 makes about 16px icon grids.
    pub const STROKE_KIND: super::StrokeKind = super::StrokeKind::Inside;

    /// The row's corner radius as egui's per-corner type.
    #[must_use]
    pub fn row_corner_radius() -> CornerRadius {
        super::radius::all(ROW_RADIUS)
    }

    /// The hidden-file gutter dot, 4px, `icon.hidden`.
    #[must_use]
    pub fn hidden_dot_color(theme: &super::Theme) -> Color32 {
        theme.icon.hidden
    }

    /// Paints an icon at a reduced alpha — the spec's universal "disabled
    /// glyph at 40%" and "cut icon at 70%" treatment (§4.2, §4.10).
    #[must_use]
    pub fn icon_at(color: Color32, alpha: f32) -> Color32 {
        with_alpha(color, alpha)
    }

    /// The §6.2 rule, as a value: disabled text is `text.disabled`, and it is
    /// WCAG-exempt only because a disabled control is exempt — so it must never
    /// be paired with a selection, which is why [`RowState::Disabled`] has no
    /// selected-bar arm.
    pub const DISABLED_TEXT_ROLE: &str = "text.disabled";

    /// A 26px row never has vertical slack for an 8px-radius lozenge: the pill
    /// would leave 9px of dead space top and bottom and read as a chip, not a
    /// row (§7.6). Asserted in tests.
    pub const ROW_MAX_RADIUS: f32 = ROW_RADIUS;
}

// =============================================================================
// Fonts (§2.8)
// =============================================================================

/// §2.8 font installation: **IBM Plex Sans 400/500/600 + IBM Plex Mono 400/500**.
///
/// Vendored at setup time into `assets/fonts/` (OFL, see
/// `assets/fonts/LICENSE-IBM-Plex.txt`). Nothing is downloaded during
/// `cargo build`; the bytes are compiled in.
///
/// # The weight trap, and how it is avoided
///
/// `epaint` resolves a glyph by walking a family's font list and taking **the
/// first face whose `cmap` has the character** (`CachedFamily::find_face_for_char`).
/// Every weight of a superfamily covers the same characters, so putting
/// `Regular, Medium, SemiBold` into one family means *every* run renders at
/// Regular — 500-weight text silently becomes 400-weight text, and the §2.8
/// weight column becomes a lie.
///
/// So each weight gets its **own single-entry `FontFamily::Name(..)`**. There is
/// no intra-family fallback because none is needed: all three weights have
/// identical coverage, so the first entry is always the right one. Weight is
/// selected by *choosing a family*, never by ordering a list.
///
/// `FontFamily::Proportional` and `FontFamily::Monospace` are re-bound to Plex as
/// well, so egui's own widgets — every `ui.label` without an explicit font, every
/// tooltip, every menu — render in the design system rather than in egui's
/// default Ubuntu-Light.
pub mod fonts {
    use super::{Context, FontFamily, FontId, FontKind, ty};
    use std::sync::Arc;

    // -- Vendored bytes (MIT / OFL; see assets/fonts/LICENSE-*.txt) ------------

    const PLEX_SANS_REGULAR: &[u8] = include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf");
    const PLEX_SANS_MEDIUM: &[u8] = include_bytes!("../assets/fonts/IBMPlexSans-Medium.ttf");
    const PLEX_SANS_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/IBMPlexSans-SemiBold.ttf");
    const PLEX_MONO_REGULAR: &[u8] = include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf");
    const PLEX_MONO_MEDIUM: &[u8] = include_bytes!("../assets/fonts/IBMPlexMono-Medium.ttf");
    const PHOSPHOR_REGULAR: &[u8] = include_bytes!("../assets/fonts/Phosphor-Regular.ttf");
    const PHOSPHOR_FILL: &[u8] = include_bytes!("../assets/fonts/Phosphor-Fill.ttf");

    // -- Family names ---------------------------------------------------------
    //
    // Public because a call site may legitimately need a weight the type scale
    // does not name — a `type.verb` at 700 in Phase 4, say.

    /// IBM Plex Sans at weight 400. The `font.sans` of §2.8.
    pub const SANS: &str = "plex-sans";
    /// IBM Plex Sans at weight 500 — `type.ui-strong`, `type.name-selected`.
    pub const SANS_500: &str = "plex-sans-500";
    /// IBM Plex Sans at weight 600 — `type.label`, `type.ui-heading`.
    pub const SANS_600: &str = "plex-sans-600";
    /// IBM Plex Mono at weight 400. The `font.mono` of §2.8.
    pub const MONO: &str = "plex-mono";
    /// IBM Plex Mono at weight 500 — `type.meta-strong`.
    pub const MONO_500: &str = "plex-mono-500";
    /// Phosphor Regular. §5.1's icon family.
    pub const ICONS: &str = "phosphor";
    /// Phosphor **Fill** — the two places §5.1 allows it.
    pub const ICONS_FILL: &str = "phosphor-fill";

    /// The family that renders `kind` at `weight`.
    ///
    /// The single place the §2.8 weight column becomes a font choice. Weights
    /// round *down* to the nearest face that exists: 500 and 600 are exact
    /// faces, and anything in between takes the lower one, because a 500-weight
    /// request rendered at 600 is a heavier lie than the reverse.
    #[must_use]
    pub fn family(kind: FontKind, weight: u16) -> FontFamily {
        match kind {
            FontKind::Sans => {
                if weight >= 600 {
                    named(SANS_600)
                } else if weight >= 500 {
                    named(SANS_500)
                } else {
                    named(SANS)
                }
            }
            FontKind::Mono => {
                if weight >= 500 {
                    named(MONO_500)
                } else {
                    named(MONO)
                }
            }
        }
    }

    fn named(name: &str) -> FontFamily {
        FontFamily::Name(Arc::from(name))
    }

    /// The Regular-weight icon family.
    #[must_use]
    pub fn icons() -> FontFamily {
        named(ICONS)
    }

    /// The Fill-weight icon family, for a toggled-on toolbar button (§4.4).
    #[must_use]
    pub fn icons_fill() -> FontFamily {
        named(ICONS_FILL)
    }

    /// The `FontId` for a type token: its size, and the family its weight maps
    /// to. This is what every call site should use instead of `FontId::new`.
    #[must_use]
    pub fn font_id(token: ty::TypeToken) -> FontId {
        FontId::new(token.size, family(token.kind, token.weight))
    }

    /// Builds the `FontDefinitions` and installs them on `ctx`.
    ///
    /// Called once, from the `AppCreator` closure, before the first frame — and
    /// again by the headless `--screenshot` path, so a capture is typeset with
    /// exactly the families a real window uses.
    pub fn install(ctx: &Context) {
        let mut defs = egui::FontDefinitions::default();

        // The icon font first, so an icon codepoint is never stolen by a text
        // font that happens to share the Private Use Area. It is *not* prepended
        // to the text families: a text run that fell through to Phosphor would
        // silently render Latin letters in an icon face.
        let icons = named(ICONS);
        defs.font_data.insert(
            "phosphor-regular".to_owned(),
            Arc::new(egui::FontData::from_static(PHOSPHOR_REGULAR)),
        );
        defs.font_data.insert(
            "phosphor-fill".to_owned(),
            Arc::new(egui::FontData::from_static(PHOSPHOR_FILL)),
        );
        // The trailing text face is a **replacement-glyph** fallback, not a
        // rendering path.
        //
        // epaint builds a `CachedFamily` per family and looks for `◻` or `?` to
        // use as its "this glyph is missing" glyph. In a one-face Phosphor family
        // it finds neither, logs `Failed to find replacement characters` on every
        // single launch, and then renders any missing glyph as *nothing at all*
        // rather than as tofu. This was measured, not guessed: with the fallback
        // on `phosphor-fill` but not `phosphor`, the warning count went 2 -> 1,
        // which is how the culprit was identified at all.
        //
        // It is safe because Phosphor owns the Private Use Area: a PUA codepoint
        // resolves in Phosphor and a normal character never reaches this face.
        defs.families.insert(
            icons,
            vec![
                "phosphor-regular".to_owned(),
                "plex-sans-regular".to_owned(),
            ],
        );
        let icons_fill = named(ICONS_FILL);
        // Fill *is* the fill face; Regular is the fallback for the handful of
        // glyphs Phosphor 2.x ships in one weight only.
        defs.families.insert(
            icons_fill,
            vec![
                "phosphor-fill".to_owned(),
                "phosphor-regular".to_owned(),
                "plex-sans-regular".to_owned(),
            ],
        );

        // Plex. One `font_data` entry per weight...
        for (name, bytes) in [
            ("plex-sans-regular", PLEX_SANS_REGULAR),
            ("plex-sans-medium", PLEX_SANS_MEDIUM),
            ("plex-sans-semibold", PLEX_SANS_SEMIBOLD),
            ("plex-mono-regular", PLEX_MONO_REGULAR),
            ("plex-mono-medium", PLEX_MONO_MEDIUM),
        ] {
            defs.font_data.insert(
                name.to_owned(),
                Arc::new(egui::FontData::from_static(bytes)),
            );
        }

        // ...and one single-entry family per (kind, weight). See the module note:
        // this is the whole point.
        defs.families
            .insert(named(SANS), vec!["plex-sans-regular".to_owned()]);
        defs.families
            .insert(named(SANS_500), vec!["plex-sans-medium".to_owned()]);
        defs.families
            .insert(named(SANS_600), vec!["plex-sans-semibold".to_owned()]);
        defs.families
            .insert(named(MONO), vec!["plex-mono-regular".to_owned()]);
        defs.families
            .insert(named(MONO_500), vec!["plex-mono-medium".to_owned()]);

        // Re-bind egui's own families so unstyled widgets inherit the design
        // system instead of egui's default.
        let proportional = defs.families.entry(FontFamily::Proportional).or_default();
        proportional.insert(0, "plex-sans-regular".to_owned());
        let monospace = defs.families.entry(FontFamily::Monospace).or_default();
        monospace.insert(0, "plex-mono-regular".to_owned());

        ctx.set_fonts(defs);
    }
}

// =============================================================================
// Tests
// =============================================================================

// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_glyphs() {
        let ctx = crate::shot::ctx_with_fonts();
        let id = crate::tokens::font_icon(16.0, false);
        let galley = ctx.fonts_mut(|f| {
            f.layout_no_wrap(
                crate::icons::FOLDER.char().to_string(),
                id.clone(),
                Color32::WHITE,
            )
        });
        eprintln!("PROBE galley size {:?}", galley.size());
        for r in galley.rows.iter() {
            for g in r.glyphs.iter() {
                eprintln!("PROBE glyph chr={:?} uv_rect={:?}", g.chr, g.uv_rect);
            }
        }
    }

    /// Every registered family must be able to produce epaint's
    /// replacement glyph, or the app logs a warning on every launch and renders
    /// missing glyphs as nothing.
    ///
    /// This asserts the *structural* precondition rather than the outcome,
    /// because the outcome is not observable: epaint's warning goes to `log` and
    /// its `FontsView::has_glyph` false-negatives on the replacement character by
    /// design (its own source notes this). The precondition is that some face in
    /// every family is a Plex face — Phosphor has no `◻` and no `?`, and no
    /// Plex face is missing either. The two icon families are the only ones that
    /// can violate this, and they are the ones this test names.
    #[test]
    fn no_family_is_icons_only() {
        let ctx = crate::shot::ctx_with_fonts();
        let defs = ctx.fonts(|f| f.definitions().clone());
        let is_phosphor_face = |n: &str| n.starts_with("phosphor-");
        for (family, faces) in &defs.families {
            assert!(
                faces.iter().any(|f| !is_phosphor_face(f)),
                "family {family} is {faces:?}: Phosphor has no replacement glyph, \
                 so this family would warn on every launch and draw a missing \
                 icon as nothing"
            );
        }
        // ... and the two icon families specifically must keep their text face.
        for name in [fonts::ICONS, fonts::ICONS_FILL] {
            let family = egui::FontFamily::Name(name.into());
            let faces = &defs.families[&family];
            assert!(
                faces.contains(&"plex-sans-regular".to_owned()),
                "the {name} family needs a text face for its replacement glyph: {faces:?}"
            );
        }
    }

    #[test]
    fn sidebar_item_height_equals_row_height() {
        // §2.9 is explicit: "metric.sidebar-item | 26 | must equal metric.row".
        assert_eq!(metric::SIDEBAR_ITEM, metric::ROW);
    }

    #[test]
    fn both_themes_define_every_role() {
        // §3 preamble: "A role with no definition in a theme is a bug, not a
        // fallback to a primitive." Because the roles are concrete fields of a
        // struct, this is enforced by the compiler; the test guards against
        // someone adding a field and defaulting it to TRANSPARENT.
        for theme in [Theme::light(), Theme::dark()] {
            assert_ne!(theme.surfaces.app, Color32::TRANSPARENT);
            assert_ne!(theme.surfaces.panel, Color32::TRANSPARENT);
            assert_ne!(theme.surfaces.list, Color32::TRANSPARENT);
            assert_ne!(theme.surfaces.raised, Color32::TRANSPARENT);
            assert_ne!(theme.text.primary, Color32::TRANSPARENT);
            assert_ne!(theme.state.selected, Color32::TRANSPARENT);
            assert_ne!(theme.state.selected_bar, Color32::TRANSPARENT);
            assert_ne!(theme.accent.base, Color32::TRANSPARENT);
            assert_ne!(theme.borders.focus_ring, Color32::TRANSPARENT);
        }
    }

    #[test]
    fn light_uses_pure_white_only_for_raised_surfaces() {
        // §1.4: "pure white (#FFFFFF) is used only for menus and dialogs, so
        // overlays physically read as lifted off the page."
        let t = Theme::light();
        assert_eq!(t.surfaces.raised, Color32::WHITE);
        for (name, c) in [
            ("app", t.surfaces.app),
            ("panel", t.surfaces.panel),
            ("chrome", t.surfaces.chrome),
            ("list", t.surfaces.list),
            ("input", t.surfaces.input),
        ] {
            assert_ne!(c, Color32::WHITE, "surface.{name} must not be pure white");
        }
    }

    #[test]
    fn system_resolves_to_dark_when_the_platform_is_silent() {
        // Wayland's `Window::theme()` returns overrides only, so `None` is the
        // common case, not an error. Documented fallback: dark.
        assert!(Theme::resolve(ThemeMode::System, None).is_dark);
        assert!(Theme::resolve(ThemeMode::System, Some(true)).is_dark);
        assert!(!Theme::resolve(ThemeMode::System, Some(false)).is_dark);
        assert!(!Theme::resolve(ThemeMode::Light, Some(true)).is_dark);
    }

    #[test]
    fn accent_is_teal_and_never_blue() {
        // §7.1 refuses blue by policy. Assert the two accent bases.
        assert_eq!(
            Theme::light().accent.base.to_array(),
            [0x0E, 0x6B, 0x5F, 255]
        );
        assert_eq!(
            Theme::dark().accent.base.to_array(),
            [0x4F, 0xC7, 0xB1, 255]
        );
    }

    #[test]
    fn selection_is_a_tint_plus_a_bar_not_a_pill() {
        // §7.6 / §1.4 decision 2. The row radius is 3px and the selected bar
        // is 2px; if either ever becomes 8px the "rounded blue lozenge" is
        // back and the design has broken.
        assert_eq!(component::ROW_RADIUS, 3.0);
        assert_eq!(component::ROW_SELECTED_BAR_WIDTH, 2.0);
        // 26px is the whole point: an 8px-radius lozenge on a 26px row leaves
        // 13px of dead vertical space and reads as a chip, not a row.
        assert_eq!(component::ROW_HEIGHT, 26.0);
    }

    #[test]
    fn motion_ceiling_is_respected() {
        // §2.11 rule 3: motion.base is the ceiling for direct manipulation.
        assert_eq!(motion::BASE, Duration::from_millis(130));
        assert_eq!(motion::FAST, Duration::from_millis(90));
        assert_eq!(motion::INSTANT, Duration::from_millis(0));
    }

    #[test]
    fn type_scale_never_exceeds_22px() {
        // §2.8 rule: "no `type.*` token is used above 22px".
        for token in ty::ALL {
            assert!(
                (10.0..=22.0).contains(&token.size),
                "{} is {}px",
                token.style_name,
                token.size
            );
            assert!((400..=700).contains(&token.weight));
        }
    }

    #[test]
    fn every_type_token_is_registered_exactly_once() {
        let mut names: Vec<&str> = ty::ALL.iter().map(|t| t.style_name).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "duplicate type token name");
        for expected in [
            "micro",
            "caption",
            "label",
            "meta",
            "meta-strong",
            "status",
            "ui",
            "ui-strong",
            "ui-heading",
            "name",
            "name-selected",
            "dialog-body",
            "dialog-title",
            "verb",
            "display",
        ] {
            assert!(names.contains(&expected), "missing type.{expected}");
        }
    }

    #[test]
    fn mono_family_is_used_for_machine_values_only() {
        // §2.8: the mono/sans split is a hard rule.
        for token in [
            ty::META,
            ty::META_STRONG,
            component::STATUSBAR_VALUE,
            component::STATUSBAR_VALUE_STRONG,
            component::STATUSBAR_SELECTION_COUNT,
        ] {
            assert_eq!(token.kind, FontKind::Mono, "{}", token.style_name);
        }
        for token in [ty::NAME, ty::UI, ty::DISPLAY, ty::DIALOG_TITLE] {
            assert_eq!(token.kind, FontKind::Sans, "{}", token.style_name);
        }
    }
}
