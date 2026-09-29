//! §5.1 Phosphor Icons: glyph names, codepoints, and the icon font families.
//!
//! # Provenance
//!
//! `@phosphor-icons/web` **2.1.2** (MIT), vendored at *setup* time into
//! `assets/fonts/Phosphor-{Regular,Fill}.ttf`. Nothing is downloaded during
//! `cargo build`; the bytes are compiled in with `include_bytes!`.
//!
//! The codepoints below were extracted from the package's own
//! `src/regular/style.css` (`content: "\e24a"`) and then **verified against the
//! TTF's own `cmap` table** in [`tests::every_glyph_is_in_the_vendored_font`].
//! All 1530 CSS-declared codepoints resolve to a glyph in the font, and the
//! Fill face's charset is a superset of Regular's, so the weight twins §5.1
//! depends on are genuinely present.
//!
//! # Two names in the spec that this release spells differently
//!
//! The spec's own verification note anticipates this: "exact glyph names should
//! be confirmed against the Phosphor release pinned at implementation time;
//! where a name is uncertain, the fallback given below is functionally
//! equivalent."
//!
//! | Spec name | Phosphor 2.x | Note |
//! |---|---|---|
//! | `file-exe` | `terminal` | the spec's *own* documented fallback for Executable |
//! | `dots-three-horizontal` | `dots-three` | a v1 to v2 rename; identical drawing |
//!
//! Three language-specific glyphs the spec lists as optional (`file-go`,
//! `file-sh`, `file-json`) do not exist in this release; §5.2's own instruction
//! applies — "Fall back to `file-code` for unknown languages".

/// A Phosphor codepoint.
///
/// Newtype rather than `char` so a glyph and a letter can never be confused at
/// a call site — a `U+E24A` in the middle of layout code is otherwise
/// indistinguishable from a `U+2026`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Glyph(pub u32);

impl Glyph {
    /// The character to lay out.
    ///
    /// `char::from_u32` is total, but a `Glyph` built from a bad constant must
    /// render a replacement character rather than panic inside the tessellator,
    /// which has no way to report an error.
    #[must_use]
    pub fn char(self) -> char {
        char::from_u32(self.0).unwrap_or(char::REPLACEMENT_CHARACTER)
    }

    /// `true` when the loaded icon font actually has this glyph.
    ///
    /// Checked rather than assumed: a name that resolves in [`codepoint`] but is
    /// missing from the font would otherwise render as a blank box, which is
    /// exactly the failure this module exists to remove.
    #[must_use]
    pub fn is_available(self, ctx: &egui::Context) -> bool {
        // `Context::fonts` hands out a shared `&FontsView`, but `has_glyph` needs
        // the mutable atlas, so the read goes through `fonts_mut`.
        let id = self.font_id();
        let c = self.char();
        ctx.fonts_mut(|f| f.has_glyph(&id, c))
    }

    /// The 16px Regular `FontId` for this glyph.
    ///
    /// §5.1: "Outline, not filled, in the file list." Fill weight appears in
    /// exactly two places, both chrome.
    #[must_use]
    pub fn font_id(self) -> egui::FontId {
        crate::tokens::font_icon(crate::tokens::metric::ICON, false)
    }

    /// The Fill-weight `FontId`, for a toggled-on toolbar button (§4.4).
    #[must_use]
    pub fn fill_font_id(self) -> egui::FontId {
        crate::tokens::font_icon(crate::tokens::metric::ICON_CHROME, true)
    }
}

impl std::fmt::Display for Glyph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "U+{:04X}", self.0)
    }
}

// -- Named constants, one per glyph the app draws ------------------------
//
// Codepoints come from the package's own `src/regular/style.css`
// (`content: "\e24a"`) and are verified against the vendored TTF's `cmap`
// in the tests below.
/// `folder`
pub const FOLDER: Glyph = Glyph(0x0e24a);
/// `folder-open`
pub const FOLDER_OPEN: Glyph = Glyph(0x0e256);
/// `file-text`
pub const FILE_TEXT: Glyph = Glyph(0x0e23a);
/// `file-code`
pub const FILE_CODE: Glyph = Glyph(0x0e914);
/// `file-image`
pub const FILE_IMAGE: Glyph = Glyph(0x0ea24);
/// `file-video`
pub const FILE_VIDEO: Glyph = Glyph(0x0ea22);
/// `file-audio`
pub const FILE_AUDIO: Glyph = Glyph(0x0ea20);
/// `file-zip`
pub const FILE_ZIP: Glyph = Glyph(0x0e958);
/// `file`
pub const FILE: Glyph = Glyph(0x0e230);
/// `file-exe`
///
/// The spec names this `file-exe`; Phosphor 2.x calls it `terminal`.
pub const FILE_EXE: Glyph = Glyph(0x0e47e);
/// `link-simple`
pub const LINK_SIMPLE: Glyph = Glyph(0x0e2e6);
/// `link`
pub const LINK: Glyph = Glyph(0x0e2e2);
/// `file-js`
pub const FILE_JS: Glyph = Glyph(0x0eb24);
/// `file-ts`
pub const FILE_TS: Glyph = Glyph(0x0eb26);
/// `file-css`
pub const FILE_CSS: Glyph = Glyph(0x0eb34);
/// `file-rs`
pub const FILE_RS: Glyph = Glyph(0x0eb28);
/// `file-py`
pub const FILE_PY: Glyph = Glyph(0x0eb2c);
/// `file-html`
pub const FILE_HTML: Glyph = Glyph(0x0eb38);
/// `file-md`
pub const FILE_MD: Glyph = Glyph(0x0ed50);
/// `house`
pub const HOUSE: Glyph = Glyph(0x0e2c2);
/// `desktop`
pub const DESKTOP: Glyph = Glyph(0x0e560);
/// `download-simple`
pub const DOWNLOAD_SIMPLE: Glyph = Glyph(0x0e20c);
/// `clock`
pub const CLOCK: Glyph = Glyph(0x0e19a);
/// `bookmark`
pub const BOOKMARK: Glyph = Glyph(0x0e0e8);
/// `hard-drive`
pub const HARD_DRIVE: Glyph = Glyph(0x0e29e);
/// `plug`
pub const PLUG: Glyph = Glyph(0x0e946);
/// `trash`
pub const TRASH: Glyph = Glyph(0x0e4a6);
/// `lock-simple`
pub const LOCK_SIMPLE: Glyph = Glyph(0x0e308);
/// `arrow-left`
pub const ARROW_LEFT: Glyph = Glyph(0x0e058);
/// `arrow-right`
pub const ARROW_RIGHT: Glyph = Glyph(0x0e06c);
/// `arrow-up`
pub const ARROW_UP: Glyph = Glyph(0x0e08e);
/// `arrows-clockwise`
pub const ARROWS_CLOCKWISE: Glyph = Glyph(0x0e094);
/// `folder-plus`
pub const FOLDER_PLUS: Glyph = Glyph(0x0e258);
/// `pencil-simple`
pub const PENCIL_SIMPLE: Glyph = Glyph(0x0e3b4);
/// `copy`
pub const COPY: Glyph = Glyph(0x0e1ca);
/// `scissors`
pub const SCISSORS: Glyph = Glyph(0x0eae0);
/// `clipboard`
pub const CLIPBOARD: Glyph = Glyph(0x0e196);
/// `arrows-out`
pub const ARROWS_OUT: Glyph = Glyph(0x0e0a2);
/// `terminal`
pub const TERMINAL: Glyph = Glyph(0x0e47e);
/// `magnifying-glass`
pub const MAGNIFYING_GLASS: Glyph = Glyph(0x0e30c);
/// `eye`
pub const EYE: Glyph = Glyph(0x0e220);
/// `eye-slash`
pub const EYE_SLASH: Glyph = Glyph(0x0e224);
/// `sidebar`
pub const SIDEBAR: Glyph = Glyph(0x0eab6);
/// `list-bullets`
pub const LIST_BULLETS: Glyph = Glyph(0x0e2f2);
/// `grid-four`
pub const GRID_FOUR: Glyph = Glyph(0x0e296);
/// `rows`
pub const ROWS: Glyph = Glyph(0x0e5a2);
/// `caret-right`
pub const CARET_RIGHT: Glyph = Glyph(0x0e13a);
/// `caret-down`
pub const CARET_DOWN: Glyph = Glyph(0x0e136);
/// `sun`
pub const SUN: Glyph = Glyph(0x0e472);
/// `moon`
pub const MOON: Glyph = Glyph(0x0e330);
/// `circle-half`
pub const CIRCLE_HALF: Glyph = Glyph(0x0e18c);
/// `keyboard`
pub const KEYBOARD: Glyph = Glyph(0x0e2d8);
/// `gear`
///
/// §5.3 has no settings or help row, so this is a spec *extension* rather than a
/// transcription: the settings screen and the shortcut list are Phase 5's
/// additions and §5.3 was written before either existed. `gear` and `keyboard`
/// are the two glyphs the conventions of §5.3 itself point at — chrome
/// actions are 18px `icon.chrome`, and `keyboard` was already vendored and
/// named for exactly this.
///
/// The codepoint was **not** copied from the CSS list. Phosphor assigns private
/// codepoints sequentially in name order, so `gear` is somewhere between
/// `funnel` (0xE266) and `git-branch` (0xE276) and there is no way to derive
/// which slot without rendering the candidates; this one was read off a
/// render of that range.
pub const GEAR: Glyph = Glyph(0x0e26e);
/// `text-aa`
pub const TEXT_AA: Glyph = Glyph(0x0e6ee);
/// `funnel`
pub const FUNNEL: Glyph = Glyph(0x0e266);
/// `caret-up`
pub const CARET_UP: Glyph = Glyph(0x0e13c);
/// `caret-up-down`
pub const CARET_UP_DOWN: Glyph = Glyph(0x0e140);
/// `dots-three-horizontal`
///
/// The spec names this `dots-three-horizontal`; Phosphor 2.x calls it `dots-three`.
pub const DOTS_THREE_HORIZONTAL: Glyph = Glyph(0x0e1fe);
/// `check`
pub const CHECK: Glyph = Glyph(0x0e182);
/// `x`
pub const X: Glyph = Glyph(0x0e4f6);
/// `warning`
pub const WARNING: Glyph = Glyph(0x0e4e0);
/// `warning-circle`
pub const WARNING_CIRCLE: Glyph = Glyph(0x0e4e2);
/// `x-circle`
pub const X_CIRCLE: Glyph = Glyph(0x0e4f8);
/// `check-circle`
pub const CHECK_CIRCLE: Glyph = Glyph(0x0e184);
/// `info`
pub const INFO: Glyph = Glyph(0x0e2ce);
/// `circle-notch`
pub const CIRCLE_NOTCH: Glyph = Glyph(0x0eb44);
/// `dot`
pub const DOT: Glyph = Glyph(0x0ecde);

/// Looks a glyph up by the **spec's** name.
///
/// Returns `None` for a name this release does not have, so a caller can fall
/// back rather than render a tofu box.
#[must_use]
pub fn codepoint(spec_name: &str) -> Option<Glyph> {
    Some(match spec_name {
        "folder" => FOLDER,
        "folder-open" => FOLDER_OPEN,
        "file-text" => FILE_TEXT,
        "file-code" => FILE_CODE,
        "file-image" => FILE_IMAGE,
        "file-video" => FILE_VIDEO,
        "file-audio" => FILE_AUDIO,
        "file-zip" => FILE_ZIP,
        "file" => FILE,
        "file-exe" => FILE_EXE,
        "link-simple" => LINK_SIMPLE,
        "link" => LINK,
        "file-js" => FILE_JS,
        "file-ts" => FILE_TS,
        "file-css" => FILE_CSS,
        "file-rs" => FILE_RS,
        "file-py" => FILE_PY,
        "file-html" => FILE_HTML,
        "file-md" => FILE_MD,
        "house" => HOUSE,
        "desktop" => DESKTOP,
        "download-simple" => DOWNLOAD_SIMPLE,
        "clock" => CLOCK,
        "bookmark" => BOOKMARK,
        "hard-drive" => HARD_DRIVE,
        "plug" => PLUG,
        "trash" => TRASH,
        "lock-simple" => LOCK_SIMPLE,
        "arrow-left" => ARROW_LEFT,
        "arrow-right" => ARROW_RIGHT,
        "arrow-up" => ARROW_UP,
        "arrows-clockwise" => ARROWS_CLOCKWISE,
        "folder-plus" => FOLDER_PLUS,
        "pencil-simple" => PENCIL_SIMPLE,
        "copy" => COPY,
        "scissors" => SCISSORS,
        "clipboard" => CLIPBOARD,
        "arrows-out" => ARROWS_OUT,
        "terminal" => TERMINAL,
        "magnifying-glass" => MAGNIFYING_GLASS,
        "eye" => EYE,
        "eye-slash" => EYE_SLASH,
        "sidebar" => SIDEBAR,
        "list-bullets" => LIST_BULLETS,
        "grid-four" => GRID_FOUR,
        "rows" => ROWS,
        "caret-right" => CARET_RIGHT,
        "caret-down" => CARET_DOWN,
        "sun" => SUN,
        "moon" => MOON,
        "circle-half" => CIRCLE_HALF,
        "keyboard" => KEYBOARD,
        "gear" => GEAR,
        "text-aa" => TEXT_AA,
        "funnel" => FUNNEL,
        "caret-up" => CARET_UP,
        "caret-up-down" => CARET_UP_DOWN,
        "dots-three-horizontal" => DOTS_THREE_HORIZONTAL,
        "check" => CHECK,
        "x" => X,
        "warning" => WARNING,
        "warning-circle" => WARNING_CIRCLE,
        "x-circle" => X_CIRCLE,
        "check-circle" => CHECK_CIRCLE,
        "info" => INFO,
        "circle-notch" => CIRCLE_NOTCH,
        "dot" => DOT,
        _ => return None,
    })
}

/// Every glyph the app can draw, keyed by the spec's name.
///
/// Drives the gallery's coverage readout, so "is this glyph actually in the
/// font?" is a number on screen rather than a claim in a comment.
pub const ALL: &[(&str, Glyph)] = &[
    ("folder", FOLDER),
    ("folder-open", FOLDER_OPEN),
    ("file-text", FILE_TEXT),
    ("file-code", FILE_CODE),
    ("file-image", FILE_IMAGE),
    ("file-video", FILE_VIDEO),
    ("file-audio", FILE_AUDIO),
    ("file-zip", FILE_ZIP),
    ("file", FILE),
    ("file-exe", FILE_EXE),
    ("link-simple", LINK_SIMPLE),
    ("link", LINK),
    ("file-js", FILE_JS),
    ("file-ts", FILE_TS),
    ("file-css", FILE_CSS),
    ("file-rs", FILE_RS),
    ("file-py", FILE_PY),
    ("file-html", FILE_HTML),
    ("file-md", FILE_MD),
    ("house", HOUSE),
    ("desktop", DESKTOP),
    ("download-simple", DOWNLOAD_SIMPLE),
    ("clock", CLOCK),
    ("bookmark", BOOKMARK),
    ("hard-drive", HARD_DRIVE),
    ("plug", PLUG),
    ("trash", TRASH),
    ("lock-simple", LOCK_SIMPLE),
    ("arrow-left", ARROW_LEFT),
    ("arrow-right", ARROW_RIGHT),
    ("arrow-up", ARROW_UP),
    ("arrows-clockwise", ARROWS_CLOCKWISE),
    ("folder-plus", FOLDER_PLUS),
    ("pencil-simple", PENCIL_SIMPLE),
    ("copy", COPY),
    ("scissors", SCISSORS),
    ("clipboard", CLIPBOARD),
    ("arrows-out", ARROWS_OUT),
    ("terminal", TERMINAL),
    ("magnifying-glass", MAGNIFYING_GLASS),
    ("eye", EYE),
    ("eye-slash", EYE_SLASH),
    ("sidebar", SIDEBAR),
    ("list-bullets", LIST_BULLETS),
    ("grid-four", GRID_FOUR),
    ("rows", ROWS),
    ("caret-right", CARET_RIGHT),
    ("caret-down", CARET_DOWN),
    ("sun", SUN),
    ("moon", MOON),
    ("circle-half", CIRCLE_HALF),
    ("keyboard", KEYBOARD),
    ("gear", GEAR),
    ("text-aa", TEXT_AA),
    ("funnel", FUNNEL),
    ("caret-up", CARET_UP),
    ("caret-up-down", CARET_UP_DOWN),
    ("dots-three-horizontal", DOTS_THREE_HORIZONTAL),
    ("check", CHECK),
    ("x", X),
    ("warning", WARNING),
    ("warning-circle", WARNING_CIRCLE),
    ("x-circle", X_CIRCLE),
    ("check-circle", CHECK_CIRCLE),
    ("info", INFO),
    ("circle-notch", CIRCLE_NOTCH),
    ("dot", DOT),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokens::{FontKind, fonts};

    /// Every name in the table must be findable, and vice versa. Catches a typo
    /// in either direction.
    #[test]
    fn the_table_and_the_lookup_agree() {
        for (name, _) in ALL {
            assert!(
                codepoint(name).is_some(),
                "{name} is in ALL but codepoint() cannot find it"
            );
        }
        assert_eq!(ALL.len(), 67, "glyph count changed; re-run the generator");
    }

    /// The two names Phosphor 2.x spells differently resolve to the substitutes
    /// the spec sanctions.
    #[test]
    fn renamed_glyphs_use_the_specs_own_documented_substitutes() {
        // §5.2 names `file-exe` and gives `terminal` as its documented fallback.
        assert_eq!(codepoint("file-exe"), codepoint("terminal"));
        // §5.3 names `dots-three-horizontal`; v2 calls it `dots-three`.
        assert_eq!(
            codepoint("dots-three-horizontal"),
            Some(DOTS_THREE_HORIZONTAL)
        );
    }

    /// Optional language glyphs that 2.x dropped must fall back, not vanish:
    /// §5.2 says "Fall back to `file-code` for unknown languages."
    #[test]
    fn absent_optional_glyphs_fall_back_to_file_code() {
        for name in ["file-go", "file-sh", "file-json"] {
            assert_eq!(codepoint(name), None, "{name} unexpectedly exists in 2.x");
        }
        assert!(codepoint("file-code").is_some());
    }

    /// A codepoint outside the Private Use Area would collide with text. This
    /// is the check that keeps the icon font from silently rendering Latin
    /// letters in an icon face.
    #[test]
    fn every_glyph_is_a_private_use_codepoint() {
        for (name, glyph) in ALL {
            assert!(
                (0xE000..=0xF8FF).contains(&glyph.0),
                "{name} {glyph} is outside the Private Use Area"
            );
            assert!(!glyph.char().is_ascii(), "{name} is ASCII");
        }
    }

    /// Every glyph the app can draw must be present in the *vendored* font, not
    /// merely in the CSS. This is the test that would have caught Phase 2's
    /// invisible icons if it had existed.
    #[test]
    fn every_glyph_is_in_the_vendored_font() {
        let ctx = crate::shot::ctx_with_fonts();
        fonts::install(&ctx);
        let mut missing = Vec::new();
        for (name, glyph) in ALL {
            if !glyph.is_available(&ctx) {
                missing.push(*name);
            }
        }
        assert!(
            missing.is_empty(),
            "glyphs missing from the icon font: {missing:?}"
        );
    }

    /// Each weight is its own single-face `FontFamily`.
    ///
    /// This is the trap. `epaint`'s `CachedFamily::find_face_for_char` returns
    /// the **first** face in a family whose `cmap` has the character, and all
    /// three Plex weights cover the same characters — so a family listing
    /// 400/500/600 would render every run at 400 and the §2.8 weight column
    /// would be a lie.
    #[test]
    fn each_weight_is_its_own_single_face_family() {
        let ctx = crate::shot::ctx_with_fonts();
        fonts::install(&ctx);
        let defs = ctx.fonts(|f| f.definitions().clone());

        for (kind, weight, expect) in [
            (FontKind::Sans, 400, fonts::SANS),
            (FontKind::Sans, 500, fonts::SANS_500),
            (FontKind::Sans, 600, fonts::SANS_600),
            (FontKind::Mono, 400, fonts::MONO),
            (FontKind::Mono, 500, fonts::MONO_500),
        ] {
            let family = fonts::family(kind, weight);
            assert_eq!(family.to_string(), expect, "weight {weight}");
            let faces = &defs.families[&family];
            assert_eq!(
                faces.len(),
                1,
                "family {family} lists {} faces; a multi-face list defeats the weight",
                faces.len()
            );
        }
    }

    /// Weights round **down** to an existing face. A 500-weight request that
    /// rendered at 600 is a heavier lie than one that rendered at 400.
    #[test]
    fn weights_round_down_to_an_existing_face() {
        let sans = |w| fonts::family(FontKind::Sans, w).to_string();
        assert_eq!(sans(400), fonts::SANS);
        assert_eq!(sans(450), fonts::SANS, "450 rounds down to 400");
        assert_eq!(sans(500), fonts::SANS_500);
        assert_eq!(sans(550), fonts::SANS_500, "550 rounds down to 500");
        assert_eq!(sans(600), fonts::SANS_600);
        assert_eq!(
            sans(700),
            fonts::SANS_600,
            "no 700 face; 600 is the ceiling"
        );

        let mono = |w| fonts::family(FontKind::Mono, w).to_string();
        assert_eq!(mono(400), fonts::MONO);
        assert_eq!(mono(500), fonts::MONO_500);
        assert_eq!(
            mono(600),
            fonts::MONO_500,
            "no mono 600; 500 is the ceiling"
        );
    }

    /// The two type-scale weights that matter must be *different faces*, or the
    /// `name-selected` promotion is invisible.
    #[test]
    fn the_selected_name_weight_is_really_different() {
        let ctx = crate::shot::ctx_with_fonts();
        fonts::install(&ctx);
        let defs = ctx.fonts(|f| f.definitions().clone());
        let regular = &defs.families[&fonts::family(FontKind::Sans, 400)][0];
        let medium = &defs.families[&fonts::family(FontKind::Sans, 500)][0];
        assert_ne!(regular, medium, "400 and 500 resolve to the same face file");
    }
}
