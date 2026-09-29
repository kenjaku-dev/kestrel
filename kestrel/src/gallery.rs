//! `--gallery`: every §4 component at every state, in both themes.
//!
//! This is how the design system gets verified *before* the real UI is built on
//! top of it. A token file that has only been read is not a token file that has
//! been seen; the gallery is the screenshot.
//!
//! It covers §4.10's state coverage matrix cell by cell — every interactive
//! component, every state — and the §6 accessibility pairing decisions that were
//! corrections rather than choices.

use egui::{Align2, Color32, Rect, RichText, ScrollArea, Sense, Stroke, Ui, pos2, vec2};

use crate::filetype::Category;
use crate::format;
use crate::tokens::{
    self, Theme, ThemeMode, border, component, icon_placeholder, metric, radius, space, ty,
    with_alpha,
};
use crate::widgets;

/// Renders the gallery into the central panel.
pub fn show(ui: &mut Ui, theme: &mut Theme, mode: &mut ThemeMode) {
    let theme = *theme;
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(theme.surfaces.app))
        .show(ui, |ui| {
            ScrollArea::vertical()
                .id_salt("gallery")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // Pin the content to the viewport width, once, and record
                    // it: `section` re-applies this at every section boundary
                    // because egui's layout widens the Ui as it goes. See the
                    // note on `section`.
                    let content_w = ui.ctx().content_rect().width();
                    ui.set_max_width(content_w);

                    header(ui, &theme, mode);
                    trace("surfaces", ui);
                    surfaces(ui, &theme, content_w);
                    trace("text", ui);
                    text(ui, &theme, content_w);
                    trace("borders", ui);
                    borders(ui, &theme, content_w);
                    trace("states", ui);
                    states(ui, &theme, content_w);
                    trace("accent_and_status", ui);
                    accent_and_status(ui, &theme, content_w);
                    trace("icons", ui);
                    icons(ui, &theme, content_w);
                    trace("type_scale", ui);
                    type_scale(ui, &theme, content_w);
                    trace("sidebar_items", ui);
                    sidebar_items(ui, &theme, content_w);
                    trace("file_rows", ui);
                    file_rows(ui, &theme, content_w);
                    trace("breadcrumb", ui);
                    breadcrumb(ui, &theme, content_w);
                    trace("toolbar", ui);
                    toolbar(ui, &theme, content_w);
                    trace("status_bar", ui);
                    status_bar(ui, &theme, content_w);
                    trace("menu", ui);
                    menu(ui, &theme, content_w);
                    trace("dialog", ui);
                    dialog(ui, &theme, content_w);
                    trace("elevation_overlay", ui);
                    elevation_overlay(ui, &theme, content_w);
                    trace("inputs", ui);
                    inputs(ui, &theme, content_w);
                    trace("spacing_radii_motion", ui);
                    spacing_radii_motion(ui, &theme, content_w);
                    trace("provenance", ui);
                    provenance(ui, &theme, content_w);
                });
        });
}

/// The env var that turns [`trace`] on.
///
/// Named rather than inlined so the release stub below and the debug build
/// cannot disagree about what the switch is called.
const DEBUG_LAYOUT_ENV: &str = "KESTREL_DEBUG_LAYOUT";

/// Reports the content `Ui`'s available width at each section boundary.
///
/// # The gallery is a layout instrument, and the instrument is dev-only
///
/// Each §4 section is a `horizontal` that egui widens to its widest child, so
/// `available_width()` after a section says how wide that section *painted*,
/// not how wide the viewport is. That is how the gallery's own width regression
/// (1200 -> 1564) was found, and it is the reason the widths have to be
/// threaded through every section as `content_w` rather than re-read.
///
/// But an `eprintln!` on the frame path is a defect in a shipped binary: the
/// output is meaningless to a user, and the branch is a per-frame environment
/// lookup inside a scroll handler. The variable stays the documented switch and
/// the dev build keeps the tool; the whole call compiles to a no-op in a release
/// build, so it cannot ship.
#[cfg(debug_assertions)]
fn trace(label: &str, ui: &Ui) {
    if std::env::var_os(DEBUG_LAYOUT_ENV).is_some() {
        eprintln!("DBG before {label} avail={:.1}", ui.available_width());
    }
}

/// The release stub for [`trace`]. Present so every call site needs no `cfg`.
#[cfg(not(debug_assertions))]
fn trace(_label: &str, _ui: &Ui) {}

/// A section heading, and the gallery's width anchor.
///
/// `widgets::section_label` is the §4.1/§4.6 component; this is the gallery's own
/// frame around it, plus the one place the content width is re-established.
///
/// # Why the width has to be threaded through
///
/// Inside a vertical `ScrollArea`, egui's layout **grows** the content `Ui`'s
/// `max_rect` as sections run — measured here as 1200 -> 1266.9 -> 1563.9 in a
/// 1200px viewport — and everything laid out from `ui.available_width()` then
/// lands off-screen, silently clipped at paint time. The `set_max_width` in
/// [`show`] alone is therefore not enough; it has to be re-applied at the top of
/// every section, and the width is passed explicitly so a section can never read
/// a stale one.
fn section(ui: &mut Ui, theme: &Theme, content_w: f32, title: &str) {
    ui.set_max_width(content_w);
    ui.add_space(space::S4);
    ui.horizontal(|ui| {
        widgets::section_label(ui, theme, title);
    });
    ui.add_space(space::S1);
    let (rule_rect, _) = ui.allocate_exact_size(vec2(content_w, border::HAIRLINE), Sense::hover());
    widgets::rule(
        ui.painter(),
        rule_rect.x_range().min,
        rule_rect.x_range().max,
        rule_rect.center().y,
        component::hairline(theme),
    );
    ui.add_space(space::S2);
}

/// A labelled row of swatches.
fn swatch_row(ui: &mut Ui, theme: &Theme, pairs: &[(&str, Color32)]) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(space::S3, space::S2);
        for (name, color) in pairs {
            ui.vertical(|ui| {
                widgets::swatch(ui, theme, *color, metric::ICON_LG);
                ui.add_space(space::HALF);
                ui.label(
                    RichText::new(*name)
                        .font(tokens::font(ty::MICRO, theme))
                        .color(theme.text.tertiary),
                );
            });
            ui.add_space(space::S1);
        }
    });
}

/// Every colour, with its spec hex, so a screenshot can be diffed against §3
/// by eye.
fn hex(color: Color32) -> String {
    let [r, g, b, _] = color.to_array();
    format!("#{r:02X}{g:02X}{b:02X}")
}

fn header(ui: &mut Ui, theme: &Theme, mode: &mut ThemeMode) {
    ui.add_space(space::S3);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Kestrel design gallery")
                .font(tokens::font(ty::DISPLAY, theme))
                .color(theme.text.primary),
        );
        ui.label(
            RichText::new(if theme.is_dark { "dark" } else { "light" })
                .font(tokens::font(ty::LABEL, theme))
                .color(theme.text.tertiary),
        );
    });
    ui.add_space(space::S1);
    ui.horizontal(|ui| {
        // Both themes, side by side on one screen: §6.1/§6.2 are comparative
        // audits, and a contrast ratio cannot be checked from one theme alone.
        for (label, candidate) in [
            ("Light", ThemeMode::Light),
            ("Dark", ThemeMode::Dark),
            ("System", ThemeMode::System),
        ] {
            let active = *mode == candidate;
            if widgets::toolbar_button(ui, theme, label, active).clicked() {
                *mode = candidate;
            }
        }
        ui.add_space(space::S4);
        ui.label(
            RichText::new("Every value below is transcribed from the spec; the hex is shown next to each swatch.")
                .font(tokens::font(ty::CAPTION, theme))
                .color(theme.text.tertiary),
        );
    });
}

/// §3.1 Surfaces.
fn surfaces(ui: &mut Ui, theme: &Theme, content_w: f32) {
    section(ui, theme, content_w, "3.1 Surfaces");
    let s = &theme.surfaces;
    swatch_row(
        ui,
        theme,
        &[
            ("surface.app", s.app),
            ("surface.panel", s.panel),
            ("surface.chrome", s.chrome),
            ("surface.list", s.list),
            ("surface.raised", s.raised),
            ("surface.input", s.input),
            ("surface.input-disabled", s.input_disabled),
            ("surface.scrim", s.scrim),
        ],
    );
    ui.add_space(space::S1);
    ui.label(
        RichText::new(format!(
            "Light `surface.raised` is {} — pure white, for menus and dialogs only. Dark is {}.",
            hex(Theme::light().surfaces.raised),
            hex(Theme::dark().surfaces.raised)
        ))
        .font(tokens::font(ty::CAPTION, theme))
        .color(theme.text.tertiary),
    );
}

/// §3.2 Text roles, each on every surface it can legally appear on.
fn text(ui: &mut Ui, theme: &Theme, content_w: f32) {
    section(ui, theme, content_w, "3.2 Text");
    let t = &theme.text;
    let rows: [(&str, Color32); 8] = [
        ("text.primary", t.primary),
        ("text.secondary", t.secondary),
        ("text.tertiary", t.tertiary),
        ("text.disabled", t.disabled),
        ("text.on-accent", t.on_accent),
        ("text.on-danger", t.on_danger),
        ("text.link", t.link),
        ("text.inverse", t.inverse),
    ];

    // §6.1/§6.2's binding constraint is `state.selected-hover`, so the roles are
    // shown on it and on `surface.list`. That is the pair that produced audit
    // corrections 3, 4, 5 and 11, and it is the pair worth seeing.
    for (surface_name, surface) in [
        ("surface.list", theme.surfaces.list),
        ("state.selected-hover", theme.state.selected_hover),
    ] {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(surface_name)
                    .font(tokens::font(ty::LABEL, theme))
                    .color(theme.text.tertiary),
            );
        });
        let (rect, _) = ui.allocate_exact_size(
            vec2(content_w, 4.0 + rows.len() as f32 * 20.0),
            Sense::hover(),
        );
        ui.painter()
            .rect_filled(rect, radius::all(radius::MD), surface);
        let painter = ui.painter();
        for (i, (name, color)) in rows.iter().enumerate() {
            let y = rect.top() + 4.0 + i as f32 * 20.0 + 10.0;
            painter.text(
                pos2(rect.left() + space::S2, y),
                Align2::LEFT_CENTER,
                *name,
                tokens::font(ty::UI, theme),
                *color,
            );
            painter.text(
                pos2(rect.left() + 160.0, y),
                Align2::LEFT_CENTER,
                hex(*color),
                tokens::font(ty::META, theme),
                theme.text.tertiary,
            );
        }
        ui.add_space(space::S2);
    }
    ui.label(
        RichText::new("text.disabled never renders on a selected row (WCAG 1.4.3 exemption, documented not hidden).")
            .font(tokens::font(ty::CAPTION, theme))
            .color(theme.text.tertiary),
    );
}

/// §3.3 Borders and focus.
fn borders(ui: &mut Ui, theme: &Theme, content_w: f32) {
    section(ui, theme, content_w, "3.3 Borders and focus");
    let b = &theme.borders;
    swatch_row(
        ui,
        theme,
        &[
            ("border.subtle", b.subtle),
            ("border.default", b.default),
            ("border.strong", b.strong),
            ("border.accent", b.accent),
            ("border.danger", b.danger),
            ("focus.ring", b.focus_ring),
            ("focus.ring-inactive", b.focus_ring_inactive),
        ],
    );
    ui.add_space(space::S1);
    ui.label(
        RichText::new("focus.ring is 2px (border.thick); focus.ring-offset is 1px. focus.ring-inactive is the 40% window-unfocused variant and is WCAG-exempt.")
            .font(tokens::font(ty::CAPTION, theme))
            .color(theme.text.tertiary),
    );
}

/// §3.4 Interactive state backgrounds, in §4.10's priority order.
fn states(ui: &mut Ui, theme: &Theme, content_w: f32) {
    section(ui, theme, content_w, "3.4 Interactive state backgrounds");
    let s = &theme.state;
    swatch_row(
        ui,
        theme,
        &[
            ("state.hover", s.hover),
            ("state.hover-strong", s.hover_strong),
            ("state.pressed", s.pressed),
            ("state.selected", s.selected),
            ("state.selected-hover", s.selected_hover),
            ("state.selected-bar", s.selected_bar),
            ("state.cut", s.cut),
            ("state.focus-within", s.focus_within),
            ("state.drop-target", s.drop_target),
        ],
    );
}

/// §3.5 Accent and §3.6 Status.
fn accent_and_status(ui: &mut Ui, theme: &Theme, content_w: f32) {
    section(ui, theme, content_w, "3.5 Accent · 3.6 Status");
    let a = &theme.accent;
    swatch_row(
        ui,
        theme,
        &[
            ("accent.base", a.base),
            ("accent.hover", a.hover),
            ("accent.pressed", a.pressed),
            ("accent.subtle-bg", a.subtle_bg),
            ("accent.border", a.border),
            ("accent.text", a.text),
            ("accent.on", a.on),
        ],
    );
    ui.add_space(space::S2);
    let s = &theme.status;
    swatch_row(
        ui,
        theme,
        &[
            ("status.danger-text", s.danger_text),
            ("status.danger-solid", s.danger_solid),
            ("status.danger-bg", s.danger_bg),
            ("status.danger-border", s.danger_border),
            ("status.success-text", s.success_text),
            ("status.success-solid", s.success_solid),
            ("status.success-bg", s.success_bg),
            ("status.success-border", s.success_border),
            ("status.warning-text", s.warning_text),
            ("status.warning-solid", s.warning_solid),
            ("status.warning-bg", s.warning_bg),
            ("status.warning-border", s.warning_border),
            ("status.info-text", s.info_text),
            ("status.info-bg", s.info_bg),
        ],
    );
    ui.add_space(space::S1);
    ui.label(
        RichText::new("§3.6: a status colour never appears without its glyph. Every chip above is drawn with one below it.")
            .font(tokens::font(ty::CAPTION, theme))
            .color(theme.text.tertiary),
    );
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(space::S2, space::S1);
        for (name, text_color, bg, border_color, glyph) in [
            ("Danger", s.danger_text, s.danger_bg, s.danger_border, "x"),
            (
                "Success",
                s.success_text,
                s.success_bg,
                s.success_border,
                "\u{2713}",
            ),
            (
                "Warning",
                s.warning_text,
                s.warning_bg,
                s.warning_border,
                "!",
            ),
            ("Info", s.info_text, s.info_bg, s.info_text, "i"),
        ] {
            let (rect, _) = ui.allocate_exact_size(vec2(92.0, 24.0), Sense::hover());
            let painter = ui.painter();
            painter.rect_filled(rect, radius::all(radius::MD), bg);
            widgets::rect_stroke(
                painter,
                rect,
                radius::all(radius::MD),
                Stroke::new(border::HAIRLINE, border_color),
            );
            painter.text(
                pos2(rect.left() + space::S2, rect.center().y),
                Align2::LEFT_CENTER,
                glyph,
                tokens::font_sans(11.0),
                text_color,
            );
            painter.text(
                pos2(rect.left() + 20.0, rect.center().y),
                Align2::LEFT_CENTER,
                name,
                tokens::font(ty::CAPTION, theme),
                text_color,
            );
            ui.add_space(space::S1);
        }
    });
}

/// §3.7 / §2.4 Icon roles, with the §5.2 category each one serves.
fn icons(ui: &mut Ui, theme: &Theme, content_w: f32) {
    section(ui, theme, content_w, "3.7 Icon roles · 5.2 File types");
    let icon = &theme.icon;
    swatch_row(
        ui,
        theme,
        &[
            ("icon.folder", icon.folder),
            ("icon.text", icon.text),
            ("icon.code", icon.code),
            ("icon.image", icon.image),
            ("icon.video", icon.video),
            ("icon.audio", icon.audio),
            ("icon.archive", icon.archive),
            ("icon.binary", icon.binary),
            ("icon.executable", icon.executable),
            ("icon.symlink", icon.symlink),
            ("icon.hidden", icon.hidden),
            ("icon.error", icon.error),
            ("icon.chrome", icon.chrome),
            ("icon.chrome-active", icon.chrome_active),
        ],
    );

    ui.add_space(space::S2);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(space::S3, space::S2);
        for category in Category::ALL {
            ui.vertical(|ui| {
                let (rect, _) = ui.allocate_exact_size(
                    vec2(component::ROW_ICON_SIZE, component::ROW_ICON_SIZE),
                    Sense::hover(),
                );
                // The real glyph, at the §5.1 16px grid. `icon_placeholder` is
                // now only the fallback for a name this release lacks, and a
                // test asserts every category's name resolves — so on this
                // screen a square would mean a regression, not a placeholder.
                if let Some(g) = crate::icons::codepoint(category.glyph()) {
                    tokens::icon_glyph(ui.painter(), rect, g, category.icon_color(&theme.icon));
                } else {
                    icon_placeholder(ui.painter(), rect, category.icon_color(&theme.icon));
                }
                ui.add_space(space::HALF);
                ui.label(
                    RichText::new(category.glyph())
                        .font(tokens::font(ty::MICRO, theme))
                        .color(theme.text.tertiary),
                );
            });
            ui.add_space(space::S1);
        }
    });
    ui.add_space(space::S1);
    ui.label(
        RichText::new(
            "The glyphs above are the spec's Phosphor 2.x assignments, vendored from @phosphor-icons/web 2.1.2 into assets/fonts/. Every one is asserted present in the loaded font by icons::tests::every_glyph_is_in_the_vendored_font; a coloured square on this screen would be a regression, not a placeholder. Two names differ from the spec: file-exe renders as the spec's own documented `terminal` fallback, and dots-three-horizontal as v2's `dots-three`.",
        )
        .font(tokens::font(ty::CAPTION, theme))
        .color(theme.status.warning_text),
    );
}

/// §2.8 The type scale, at its real size, with the recorded weight.
fn type_scale(ui: &mut Ui, theme: &Theme, content_w: f32) {
    section(ui, theme, content_w, "2.8 Type scale");
    // A fixed cell width rather than a measured one: `horizontal_wrapped`
    // decides where to break against `ui.available_width()`, which inside a
    // `ScrollArea` is the value §`section` pins — but a 15-item wrapped row
    // whose break lands 8px off still clips the last cell. Fixed cells make the
    // break arithmetic exact and the row count a function of `content_w` alone.
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(space::S2, space::S3);
        for token in ty::ALL {
            ui.allocate_ui(vec2(180.0, ui.available_height()), |ui| {
                // `TypeToken` is not `Copy` (its `family` is a `FontFamily`,
                // which epaint 0.36 makes `Clone`-only), so the descriptive
                // fields are read out before the token is moved into `font`.
                let (name, size, line, weight, tracking, uppercase) = (
                    token.style_name,
                    token.size,
                    token.line,
                    token.weight,
                    token.tracking,
                    token.uppercase,
                );
                let font = tokens::font(token, theme);
                ui.label(RichText::new(name).font(font).color(theme.text.primary));
                ui.label(
                    RichText::new(format!(
                        "{}px/{}px · {weight} · tracking {tracking:+.2}em{}",
                        size as u32,
                        line as u32,
                        if uppercase { " · uppercase" } else { "" }
                    ))
                    .font(tokens::font(ty::MICRO, theme))
                    .color(theme.text.tertiary),
                );
            });
        }
    });
    ui.add_space(space::S1);
    ui.label(
        RichText::new(
            "Weights ARE applied: epaint resolves a glyph to the FIRST face in a family \
             that has it, so each weight is its own single-face FontFamily rather than a \
             position in a fallback list. Tracking is still NOT applied — epaint exposes no \
             tracking control — which is why several rows below read +0.00em.",
        )
        .font(tokens::font(ty::CAPTION, theme))
        .color(theme.text.tertiary),
    );
}

/// §4.1 Sidebar item, every state.
fn sidebar_items(ui: &mut Ui, theme: &Theme, content_w: f32) {
    section(ui, theme, content_w, "4.1 Sidebar item");
    // `glyph` is the §5.3 name for the state being shown. The active row uses
    // the Fill face, which §5.1 permits in exactly two places — this is one of
    // them, the other being a toggled-on toolbar button.
    for (label, state, glyph) in [
        ("default", "default", "house"),
        ("hover", "hover", "house"),
        ("active-place", "active", "house"),
        ("disabled", "disabled", "plug"),
    ] {
        let enabled = state != "disabled";
        let active = state == "active";
        let (bg, text_color, icon_color) = if !enabled {
            (
                component::SIDEBAR_ITEM_BG_DISABLED,
                theme.text.disabled,
                with_alpha(theme.icon.chrome, 0.4),
            )
        } else if active {
            (
                theme.accent.subtle_bg,
                theme.text.primary,
                theme.icon.chrome_active,
            )
        } else if state == "hover" {
            (theme.state.hover, theme.text.primary, theme.icon.chrome)
        } else {
            (
                component::SIDEBAR_ITEM_BG,
                theme.text.secondary,
                theme.icon.chrome,
            )
        };

        let (rect, _) =
            ui.allocate_exact_size(vec2(180.0, component::SIDEBAR_ITEM_HEIGHT), Sense::hover());
        // The row is drawn on `surface.panel`, which is what it lives on.
        let back =
            Rect::from_min_size(rect.left_top() - vec2(0.0, 0.0), vec2(200.0, rect.height()));
        ui.painter()
            .rect_filled(back, radius::all(radius::NONE), theme.surfaces.panel);
        if bg != Color32::TRANSPARENT {
            ui.painter()
                .rect_filled(rect, radius::all(component::SIDEBAR_ITEM_RADIUS), bg);
        }
        if active {
            ui.painter().rect_filled(
                Rect::from_min_size(
                    rect.left_top(),
                    vec2(component::SIDEBAR_ITEM_ACTIVE_BAR, rect.height()),
                ),
                radius::all(radius::NONE),
                theme.state.selected_bar,
            );
        }
        let icon_rect = Rect::from_center_size(
            rect.left_center()
                + vec2(
                    component::SIDEBAR_ITEM_PADDING_X + component::SIDEBAR_ICON_SIZE / 2.0,
                    0.0,
                ),
            vec2(component::SIDEBAR_ICON_SIZE, component::SIDEBAR_ICON_SIZE),
        );
        match crate::icons::codepoint(glyph) {
            Some(g) if active => tokens::icon_glyph_fill(ui.painter(), icon_rect, g, icon_color),
            Some(g) => tokens::icon_glyph(ui.painter(), icon_rect, g, icon_color),
            None => icon_placeholder(ui.painter(), icon_rect, icon_color),
        }
        ui.painter().text(
            pos2(
                icon_rect.right() + component::SIDEBAR_ITEM_GAP,
                rect.center().y,
            ),
            Align2::LEFT_CENTER,
            label,
            tokens::font(ty::UI, theme),
            text_color,
        );
        // State name, right-aligned in mono.
        ui.painter().text(
            pos2(rect.right() + space::S3, rect.center().y),
            Align2::LEFT_CENTER,
            state,
            tokens::font(ty::META, theme),
            theme.text.tertiary,
        );
    }
}

/// §4.2 File row, every state in the matrix.
fn file_rows(ui: &mut Ui, theme: &Theme, content_w: f32) {
    section(ui, theme, content_w, "4.2 File list row");

    let states = [
        (
            "default",
            component::RowState::Default,
            "notes.md",
            Category::Text,
            false,
        ),
        (
            "hover",
            component::RowState::Hover,
            "notes.md",
            Category::Text,
            false,
        ),
        (
            "selected",
            component::RowState::Selected,
            "notes.md",
            Category::Text,
            false,
        ),
        (
            "selected + hover",
            component::RowState::SelectedHover,
            "notes.md",
            Category::Text,
            false,
        ),
        (
            "selected + focused",
            component::RowState::SelectedFocus,
            "notes.md",
            Category::Text,
            false,
        ),
        (
            "focused, not selected",
            component::RowState::FocusVisible,
            "notes.md",
            Category::Text,
            false,
        ),
        (
            "cut",
            component::RowState::Cut,
            "archive.tar",
            Category::Archive,
            false,
        ),
        (
            "drop target",
            component::RowState::DropTarget,
            "Documents",
            Category::Folder,
            false,
        ),
        (
            "disabled",
            component::RowState::Disabled,
            "root-owned",
            Category::Binary,
            false,
        ),
        (
            "hidden file",
            component::RowState::Default,
            ".prettierrc.toml",
            Category::Text,
            true,
        ),
    ];

    // `Theme::style` sets the *list* band's item spacing to the 2px half-step
    // (§2.5), so consecutive allocated rects would be 28px apart. The row
    // height token is 26px and the gallery is where that is checked, so the
    // half-step is zeroed for the duration.
    let outer_spacing = ui.spacing().item_spacing;
    ui.spacing_mut().item_spacing.y = 0.0;

    for (label, state, name, category, hidden) in states {
        // Each row is drawn on a real `surface.list` backdrop, because a row's
        // background is meaningless without the surface it sits on (§6's whole
        // method is "tested against all ten surfaces").
        let (backdrop, _) =
            ui.allocate_exact_size(vec2(content_w, component::ROW_HEIGHT), Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(backdrop, radius::all(radius::NONE), theme.surfaces.list);
        painter.rect_filled(
            backdrop,
            component::row_corner_radius(),
            state.background(theme),
        );
        if let Some(bar) = state.selected_bar(theme) {
            painter.rect_filled(
                Rect::from_min_max(
                    pos2(
                        backdrop.left(),
                        backdrop.top() + component::ROW_SELECTED_BAR_INSET,
                    ),
                    pos2(
                        backdrop.left() + component::ROW_SELECTED_BAR_WIDTH,
                        backdrop.bottom() - component::ROW_SELECTED_BAR_INSET,
                    ),
                ),
                radius::all(1.0),
                bar,
            );
        }
        if let Some(ring) = state.focus_ring(theme, true) {
            widgets::rect_stroke(painter, backdrop, component::row_corner_radius(), ring);
        }
        if hidden {
            painter.circle_filled(
                pos2(backdrop.left() + metric::GUTTER_MARKER, backdrop.center().y),
                component::ROW_HIDDEN_DOT / 2.0,
                component::hidden_dot_color(theme),
            );
        }

        let icon_color = if hidden {
            theme.icon.hidden
        } else {
            category.icon_color(&theme.icon)
        };
        let icon_color = match state {
            component::RowState::Disabled => component::icon_at(icon_color, 0.5),
            component::RowState::Cut => component::icon_at(icon_color, 0.7),
            _ => icon_color,
        };
        let glyph_rect = Rect::from_center_size(
            pos2(
                backdrop.left() + metric::GUTTER + component::ROW_ICON_SIZE / 2.0,
                backdrop.center().y,
            ),
            vec2(component::ROW_ICON_SIZE, component::ROW_ICON_SIZE),
        );
        if let Some(g) = crate::icons::codepoint(category.glyph()) {
            tokens::icon_glyph(painter, glyph_rect, g, icon_color);
        } else {
            icon_placeholder(painter, glyph_rect, icon_color);
        }

        let name_color = match (hidden, state) {
            (_, component::RowState::Disabled) => theme.text.disabled,
            (true, _) => theme.text.tertiary,
            _ => theme.text.primary,
        };
        painter.text(
            pos2(
                backdrop.left()
                    + metric::GUTTER
                    + component::ROW_ICON_SIZE
                    + component::ROW_ICON_GAP,
                backdrop.center().y,
            ),
            Align2::LEFT_CENTER,
            name,
            tokens::font(state.name_token(), theme),
            name_color,
        );

        // The three machine columns, right-aligned, mono, `text.tertiary`.
        let size = if category == Category::Folder {
            format::NOT_APPLICABLE.to_string()
        } else {
            format::bytes(4096)
        };
        painter.text(
            pos2(backdrop.right() - 220.0, backdrop.center().y),
            Align2::RIGHT_CENTER,
            &size,
            tokens::font(ty::META, theme),
            theme.text.tertiary,
        );
        painter.text(
            pos2(backdrop.right() - 130.0, backdrop.center().y),
            Align2::RIGHT_CENTER,
            category.label(),
            tokens::font(ty::CAPTION, theme),
            theme.text.tertiary,
        );
        painter.text(
            pos2(backdrop.right(), backdrop.center().y),
            Align2::RIGHT_CENTER,
            "2026-09-28 18:21",
            tokens::font(ty::META, theme),
            theme.text.tertiary,
        );
        // The state name, outside the row, in mono.
        painter.text(
            pos2(backdrop.left() + 300.0, backdrop.center().y),
            Align2::LEFT_CENTER,
            label,
            tokens::font(ty::META, theme),
            theme.text.tertiary,
        );
    }

    ui.spacing_mut().item_spacing = outer_spacing;
    ui.add_space(space::S1);
    ui.label(
        RichText::new("Directory rows show an em dash for size: FileEntry::size for a directory is its own lstat length, not a recursive total.")
            .font(tokens::font(ty::CAPTION, theme))
            .color(theme.text.tertiary),
    );
}

/// §4.3 Breadcrumb segments.
fn breadcrumb(ui: &mut Ui, theme: &Theme, content_w: f32) {
    section(ui, theme, content_w, "4.3 Breadcrumb");
    let (rect, _) = ui.allocate_exact_size(
        vec2(content_w, component::BREADCRUMB_HEIGHT),
        Sense::hover(),
    );
    let painter = ui.painter();
    painter.rect_filled(rect, radius::all(radius::NONE), theme.surfaces.panel);
    widgets::rule(
        painter,
        rect.x_range().min,
        rect.x_range().max,
        rect.bottom() - 0.5,
        Stroke::new(border::HAIRLINE, theme.borders.subtle),
    );

    let segments: [(&str, bool); 4] = [
        ("/", false),
        ("home", false),
        ("Projects", false),
        ("kestrel", true),
    ];
    let sep_color = component::icon_at(theme.icon.chrome, component::BREADCRUMB_SEPARATOR_ALPHA);
    let mut x = rect.left() + component::BREADCRUMB_PADDING_X;
    let y = rect.center().y;
    for (i, (label, is_current)) in segments.iter().enumerate() {
        if i > 0 {
            painter.text(
                pos2(x, y),
                Align2::LEFT_CENTER,
                "\u{203A}",
                tokens::font_sans(component::BREADCRUMB_SEPARATOR_SIZE),
                sep_color,
            );
            x += component::BREADCRUMB_SEPARATOR_HIT_W;
        }
        let (color, token) = if *is_current {
            (
                theme.text.primary,
                component::BREADCRUMB_SEGMENT_TEXT_CURRENT,
            )
        } else {
            (theme.text.secondary, component::BREADCRUMB_SEGMENT_TEXT)
        };
        let font = tokens::font(token, theme);
        let w = widgets::text_width(ui, label, font.clone());
        let seg = Rect::from_center_size(
            pos2(x + w / 2.0, y),
            vec2(w + space::S2, component::BREADCRUMB_SEGMENT_HEIGHT),
        );
        if *is_current {
            // The current directory shows no hover state and is not clickable.
            painter.rect_filled(seg, radius::all(radius::SM), theme.surfaces.panel);
        } else {
            painter.rect_filled(seg, radius::all(radius::SM), theme.state.hover);
        }
        painter.text(pos2(x, y), Align2::LEFT_CENTER, *label, font, color);
        x = seg.right() + component::BREADCRUMB_SEGMENT_GAP;
    }
    painter.text(
        pos2(rect.left() + 220.0, y),
        Align2::LEFT_CENTER,
        "the final segment is 500 weight, not clickable, no hover",
        tokens::font(ty::CAPTION, theme),
        theme.text.tertiary,
    );
}

/// §4.4 Toolbar buttons, both variants and all four states.
fn toolbar(ui: &mut Ui, theme: &Theme, content_w: f32) {
    section(ui, theme, content_w, "4.4 Toolbar button");
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(space::S2, space::S1);
        for (label, active) in [
            ("Grid", false),
            ("List", true),
            ("Details", false),
            ("Sort", false),
            ("New folder", false),
        ] {
            widgets::toolbar_button(ui, theme, label, active);
        }
        ui.add_space(space::S4);
        // The one filled button: `accent.base` bg, `text.on-accent`,
        // `type.ui-strong`.
        let (rect, _) = ui.allocate_exact_size(
            vec2(
                widgets::text_width(
                    ui,
                    "New folder",
                    tokens::font(component::TOOLBAR_BTN_LABEL_ACTIVE, theme),
                ) + space::S2 * 2.0,
                component::TOOLBAR_BTN_HEIGHT,
            ),
            Sense::hover(),
        );
        let painter = ui.painter();
        painter.rect_filled(
            rect,
            radius::all(component::TOOLBAR_BTN_RADIUS),
            theme.accent.base,
        );
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "New folder",
            tokens::font(component::TOOLBAR_BTN_LABEL_ACTIVE, theme),
            theme.accent.on,
        );
    });
    ui.add_space(space::S1);
    ui.label(
        RichText::new(
            "Every icon-only button ships with a tooltip and an accessible name (§7.14).",
        )
        .font(tokens::font(ty::CAPTION, theme))
        .color(theme.text.tertiary),
    );
}

/// §4.5 Status bar sections.
fn status_bar(ui: &mut Ui, theme: &Theme, content_w: f32) {
    section(ui, theme, content_w, "4.5 Status bar");
    let (rect, _) =
        ui.allocate_exact_size(vec2(content_w, component::STATUSBAR_HEIGHT), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, radius::all(radius::NONE), theme.surfaces.panel);
    widgets::rule(
        painter,
        rect.x_range().min,
        rect.x_range().max,
        rect.top() + 0.5,
        Stroke::new(border::HAIRLINE, theme.borders.subtle),
    );
    let y = rect.center().y;
    let mut x = rect.left() + component::STATUSBAR_PADDING_X;
    for (text, font, color) in [
        (
            "1,204 items",
            tokens::font(component::STATUSBAR_LABEL, theme),
            theme.text.tertiary,
        ),
        (
            "3 items",
            tokens::font(component::STATUSBAR_SELECTION_COUNT, theme),
            theme.accent.text,
        ),
        (
            "/home/achraf/Projects/kestrel/kestre\u{2026}",
            tokens::font(component::STATUSBAR_VALUE, theme),
            theme.text.secondary,
        ),
    ] {
        let width = widgets::text_width(ui, text, font.clone());
        painter.text(pos2(x, y), Align2::LEFT_CENTER, text, font, color);
        x += width + component::STATUSBAR_SECTION_GAP;
    }
    // The free-space meter, right-aligned.
    let meter = Rect::from_min_size(
        pos2(
            rect.right() - component::STATUSBAR_METER_W - space::S2,
            y - component::STATUSBAR_METER_H / 2.0,
        ),
        vec2(component::STATUSBAR_METER_W, component::STATUSBAR_METER_H),
    );
    painter.rect_filled(meter, radius::all(radius::XS), theme.state.hover);
    painter.rect_filled(
        Rect::from_min_size(meter.min, vec2(meter.width() * 0.42, meter.height())),
        radius::all(radius::XS),
        theme.accent.base,
    );
    painter.text(
        pos2(
            rect.right() - component::STATUSBAR_METER_W - space::S2,
            y - 14.0,
        ),
        Align2::RIGHT_CENTER,
        "128 GiB free",
        tokens::font(component::STATUSBAR_LABEL, theme),
        theme.text.tertiary,
    );
}

/// §4.6 Context menu items, in a `surface.raised` frame.
fn menu(ui: &mut Ui, theme: &Theme, content_w: f32) {
    section(ui, theme, content_w, "4.6 Context menu");
    let width = component::MENU_MIN_WIDTH;
    let items: [(&str, &str, bool); 5] = [
        ("Open", "Enter", false),
        ("Open in new pane", "Ctrl+Enter", false),
        ("Rename", "F2", false),
        ("Delete Permanently", "Shift+Delete", true),
        ("Properties", "", false),
    ];
    let height = component::MENU_PADDING * 2.0
        + component::MENU_ITEM_HEIGHT * items.len() as f32
        + component::MENU_SEPARATOR_MARGIN_Y * 2.0
        + border::HAIRLINE;
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let painter = ui.painter();
    // `menu.elevation` = elev.1 + elev.inner-highlight (light only). With
    // `button_frame` off and no overlay here, the 1px `border.strong` outline is
    // the fallback that keeps the overlay boundary visible (§2.10).
    painter.rect_filled(
        rect,
        radius::all(component::MENU_RADIUS),
        theme.surfaces.raised,
    );
    widgets::rect_stroke(
        painter,
        rect,
        radius::all(component::MENU_RADIUS),
        Stroke::new(border::HAIRLINE, theme.borders.strong),
    );

    let mut y = rect.top() + component::MENU_PADDING;
    for (i, (label, shortcut, destructive)) in items.iter().enumerate() {
        if i == 3 {
            widgets::rule(
                painter,
                rect.left() + component::MENU_SEPARATOR_INSET_X,
                rect.right() - component::MENU_SEPARATOR_INSET_X,
                y + component::MENU_SEPARATOR_MARGIN_Y / 2.0,
                component::hairline(theme),
            );
            y += component::MENU_SEPARATOR_MARGIN_Y;
        }
        let item = Rect::from_min_size(
            pos2(rect.left() + component::MENU_PADDING, y),
            vec2(
                rect.width() - component::MENU_PADDING * 2.0,
                component::MENU_ITEM_HEIGHT,
            ),
        );
        // §4.6: destructive items are red *text and icon only*. "No red-filled
        // menu rows — a red block in a menu is the loudest possible signal and
        // it fires on every right-click, not on the click that matters."
        let color = if *destructive {
            theme.status.danger_text
        } else {
            theme.text.primary
        };
        painter.text(
            pos2(
                item.left() + component::MENU_ITEM_PADDING_X,
                item.center().y,
            ),
            Align2::LEFT_CENTER,
            *label,
            tokens::font(ty::UI, theme),
            color,
        );
        if !shortcut.is_empty() {
            painter.text(
                pos2(
                    item.right() - component::MENU_ITEM_PADDING_X,
                    item.center().y,
                ),
                Align2::RIGHT_CENTER,
                *shortcut,
                tokens::font(component::MENU_ITEM_SHORTCUT, theme),
                theme.text.tertiary,
            );
        }
        y += component::MENU_ITEM_HEIGHT;
    }
}

/// §2.10 Elevation, rendered rather than described.
fn elevation_overlay(ui: &mut Ui, theme: &Theme, content_w: f32) {
    section(ui, theme, content_w, "2.10 Elevation");
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(space::S3, space::S2);
        elevation_card(ui, theme);
    });
    ui.label(
        RichText::new(
            "Shadows are on overlays only. Chrome is flat and separated by hairlines (§7.8).",
        )
        .font(tokens::font(ty::CAPTION, theme))
        .color(theme.text.tertiary),
    );
}

/// §4.7 A confirmation dialog in a `surface.raised` frame over a scrim.
fn dialog(ui: &mut Ui, theme: &Theme, content_w: f32) {
    section(ui, theme, content_w, "4.7 Confirmation dialog");

    let block = component::DIALOG_BLOCK_HEIGHT;
    let (scrim_rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), block), Sense::hover());
    ui.painter()
        .rect_filled(scrim_rect, radius::all(radius::NONE), theme.surfaces.scrim);

    let box_rect = Rect::from_center_size(
        scrim_rect.center(),
        vec2(component::DIALOG_WIDTH, block - space::S10),
    );

    // The dialog *chrome* is drawn with the painter, and its *content* with
    // widgets. Splitting them like this is what keeps the stale-borrow problem
    // away: the painter is only alive for the chrome pass, and `checkbox` below
    // needs `&mut Ui` to allocate a real hit area.
    {
        let painter = ui.painter();
        painter.rect_filled(
            box_rect,
            radius::all(component::DIALOG_RADIUS),
            theme.surfaces.raised,
        );
        widgets::rect_stroke(
            painter,
            box_rect,
            radius::all(component::DIALOG_RADIUS),
            Stroke::new(border::HAIRLINE, theme.borders.strong),
        );
    }

    ui.scope_builder(
        egui::UiBuilder::new().max_rect(box_rect.shrink(component::DIALOG_PADDING)),
        |ui| {
            ui.spacing_mut().item_spacing.y = space::S2;
            ui.set_width(ui.available_width());

            // `dialog.icon` — 20px, `warning` in `status.danger-text` for a
            // destructive action.
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = vec2(space::S2, 0.0);
                ui.label(
                    RichText::new("!")
                        .font(tokens::font_sans(component::DIALOG_ICON_SIZE))
                        .color(theme.status.danger_text),
                );
                // `dialog.title` — `type.dialog-title`, `text.primary`.
                ui.label(
                    RichText::new("Delete 3 items permanently?")
                        .font(tokens::font(component::DIALOG_TITLE, theme))
                        .color(theme.text.primary),
                );
            });

            // `dialog.body` — `type.dialog-body`, `text.secondary`.
            ui.label(
                RichText::new("This cannot be undone. Move to Trash is reversible.")
                    .font(tokens::font(component::DIALOG_BODY, theme))
                    .color(theme.text.secondary),
            );

            // `dialog.path-quote` — `type.meta`, `state.hover` bg, radius 4,
            // padding 6/8, middle-truncate.
            let quote = Rect::from_min_size(
                ui.available_rect_before_wrap().min,
                vec2(ui.available_width(), 24.0),
            );
            let painter = ui.painter();
            painter.rect_filled(
                quote,
                radius::all(component::DIALOG_PATH_QUOTE_RADIUS),
                theme.state.hover,
            );
            painter.text(
                pos2(
                    quote.left() + component::DIALOG_PATH_QUOTE_PAD_X,
                    quote.center().y,
                ),
                Align2::LEFT_CENTER,
                format::middle_truncate("/home/achraf/Projects/kestrel/kestrel/src/tokens.rs", 46),
                tokens::font(ty::META, theme),
                theme.text.secondary,
            );
            ui.allocate_space(vec2(0.0, quote.height()));

            // `dialog.checkbox` — 14px painted inside a 24px hit area (§6.6).
            checkbox(ui, theme, true, "Also remove from Trash");

            // `dialog.footer-align` right, `dialog.footer-gap` 12px. `Cancel`
            // is leftmost and keeps focus on open; the destructive verb is never
            // the Enter default (§4.7 copy rules).
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = component::DIALOG_FOOTER_GAP;
                dialog_button(ui, theme, "Delete Permanently", true).clicked();
                dialog_button(ui, theme, "Cancel", false).clicked();
            });
        },
    );

    ui.label(
        RichText::new("Cancel is leftmost and holds focus on open; the destructive verb is never the Enter default.")
            .font(tokens::font(ty::CAPTION, theme))
            .color(theme.text.tertiary),
    );
}

/// A §4.7 footer button, in either the `confirm` or the `destructive` variant.
fn dialog_button(ui: &mut Ui, theme: &Theme, label: &str, destructive: bool) -> egui::Response {
    let font = tokens::font(ty::UI_STRONG, theme);
    let width =
        widgets::text_width(ui, label, font.clone()) + component::DIALOG_BTN_PADDING_X * 2.0;
    let (rect, response) =
        ui.allocate_exact_size(vec2(width, component::DIALOG_BTN_HEIGHT), Sense::click());

    let (bg, fg) = if response.is_pointer_button_down_on() {
        (theme.state.pressed, theme.text.primary)
    } else if response.hovered() {
        // `dialog.btn-cancel-bg-hover` is `state.hover-strong`; the destructive
        // variant uses `danger.600`/`danger.500`.
        if destructive {
            (theme.status.danger_solid, theme.text.on_danger)
        } else {
            (theme.state.hover_strong, theme.text.primary)
        }
    } else if destructive {
        (theme.status.danger_solid, theme.text.on_danger)
    } else {
        (theme.surfaces.input, theme.text.primary)
    };

    let painter = ui.painter();
    painter.rect_filled(rect, radius::all(component::DIALOG_BTN_RADIUS), bg);
    if !destructive {
        // `dialog.btn-cancel-border` — 1px `border.strong`. The destructive
        // button is solid, so it carries no separate outline.
        widgets::rect_stroke(
            painter,
            rect,
            radius::all(component::DIALOG_BTN_RADIUS),
            Stroke::new(border::HAIRLINE, theme.borders.strong),
        );
    }
    painter.text(rect.center(), Align2::CENTER_CENTER, label, font, fg);
    response
}

/// §4.8 Text input and the search field.
fn inputs(ui: &mut Ui, theme: &Theme, content_w: f32) {
    section(ui, theme, content_w, "4.8 Text input · search field");
    for (label, value, focused) in [
        ("default", "Filter\u{2026}", false),
        ("focused", "kestrel", true),
        ("invalid", "sh", false),
    ] {
        let (rect, _) =
            ui.allocate_exact_size(vec2(280.0, component::INPUT_HEIGHT), Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(
            rect,
            radius::all(component::INPUT_RADIUS),
            theme.surfaces.input,
        );
        let border_color = if label == "invalid" {
            theme.borders.danger
        } else if focused {
            theme.borders.accent
        } else {
            theme.borders.default
        };
        widgets::rect_stroke(
            painter,
            rect,
            radius::all(component::INPUT_RADIUS),
            Stroke::new(border::HAIRLINE, border_color),
        );
        // `input.ring` — 2px `focus.ring` at 1px offset.
        if focused {
            widgets::rect_stroke(
                painter,
                rect.expand(border::HAIRLINE),
                radius::all(component::INPUT_RADIUS + 1.0),
                Stroke::new(border::HAIRLINE, theme.borders.focus_ring),
            );
        }
        // The leading `magnifying-glass` 16px.
        tokens::icon_glyph(
            painter,
            Rect::from_center_size(
                pos2(
                    rect.left() + component::INPUT_PADDING_X + component::INPUT_ICON_SIZE / 2.0,
                    rect.center().y,
                ),
                vec2(component::INPUT_ICON_SIZE, component::INPUT_ICON_SIZE),
            ),
            crate::icons::MAGNIFYING_GLASS,
            theme.icon.chrome,
        );
        let is_placeholder = value == component::SEARCH_PLACEHOLDER;
        painter.text(
            pos2(
                rect.left()
                    + component::INPUT_PADDING_X
                    + component::INPUT_ICON_SIZE
                    + component::INPUT_ICON_GAP,
                rect.center().y,
            ),
            Align2::LEFT_CENTER,
            value,
            tokens::font(ty::UI, theme),
            if is_placeholder {
                theme.text.tertiary
            } else {
                theme.text.primary
            },
        );
        // The live match count sits *outside* the field, never inside it.
        painter.text(
            pos2(rect.right() + space::S2, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            tokens::font(ty::CAPTION, theme),
            theme.text.tertiary,
        );
    }
}

/// §2.5, §2.6, §2.7, §2.11: the numeric layers, on one grid.
fn spacing_radii_motion(ui: &mut Ui, theme: &Theme, content_w: f32) {
    section(
        ui,
        theme,
        content_w,
        "2.5–2.7, 2.11 Spacing · radii · borders · motion",
    );
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("space")
                .font(tokens::font(ty::LABEL, theme))
                .color(theme.text.tertiary),
        );
        ui.spacing_mut().item_spacing = vec2(space::S1, 0.0);
        for (name, value) in [
            ("half", space::HALF),
            ("1", space::S1),
            ("1.5", space::S1_5),
            ("2", space::S2),
            ("2.5", space::S2_5),
            ("3", space::S3),
            ("4", space::S4),
            ("6", space::S6),
            ("8", space::S8),
        ] {
            ui.vertical(|ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(value.max(1.0), 20.0), Sense::hover());
                ui.painter()
                    .rect_filled(rect, radius::all(radius::NONE), theme.accent.base);
                ui.label(
                    RichText::new(name)
                        .font(tokens::font(ty::MICRO, theme))
                        .color(theme.text.tertiary),
                );
            });
        }
    });
    ui.add_space(space::S2);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("radius")
                .font(tokens::font(ty::LABEL, theme))
                .color(theme.text.tertiary),
        );
        ui.spacing_mut().item_spacing = vec2(space::S1, 0.0);
        for (name, value) in [
            ("none", radius::NONE),
            ("xs", radius::XS),
            ("sm", radius::SM),
            ("md", radius::MD),
            ("lg", radius::LG),
            ("xl", radius::XL),
            ("xxl", radius::XXL),
        ] {
            ui.vertical(|ui| {
                ui.painter().rect_filled(
                    Rect::from_min_size(ui.available_rect_before_wrap().min, vec2(24.0, 24.0)),
                    radius::all(value),
                    theme.state.selected,
                );
                ui.label(
                    RichText::new(format!("{name} {value}"))
                        .font(tokens::font(ty::MICRO, theme))
                        .color(theme.text.tertiary),
                );
            });
        }
    });
    ui.add_space(space::S2);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(space::S3, 0.0);
        for (name, duration) in [
            ("motion.instant (selection)", tokens::motion::INSTANT),
            ("motion.fast", tokens::motion::FAST),
            ("motion.base", tokens::motion::BASE),
            ("motion.moderate", tokens::motion::MODERATE),
            ("motion.slow", tokens::motion::SLOW),
            ("motion.deliberate", tokens::motion::DELIBERATE),
            ("motion.loop.spinner", tokens::motion::SPINNER),
        ] {
            ui.label(
                RichText::new(format!("{name} · {}ms", duration.as_millis()))
                    .font(tokens::font(ty::META, theme))
                    .color(theme.text.secondary),
            );
        }
    });
    ui.label(
        RichText::new("Selection is 0ms. motion.base (130ms) is the ceiling for anything the pointer touches.")
            .font(tokens::font(ty::CAPTION, theme))
            .color(theme.text.tertiary),
    );
}

/// §4.7 `dialog.checkbox` — 14px painted, 24px hit.
fn checkbox(ui: &mut Ui, theme: &Theme, checked: bool, label: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = vec2(space::S2, 0.0);
        // §6.6: "checkbox 14px painted / 24px hit". The hit rect is the
        // 24px target; the box is 14px and centred inside it.
        let (hit, response) = ui.allocate_exact_size(
            widgets::hit_area(vec2(component::DIALOG_CHECKBOX, component::DIALOG_CHECKBOX)),
            Sense::click(),
        );
        let box_rect = Rect::from_center_size(
            hit.center(),
            vec2(component::DIALOG_CHECKBOX, component::DIALOG_CHECKBOX),
        );
        let painter = ui.painter();
        let (fill, border_color, glyph) = if checked {
            (theme.accent.base, theme.accent.base, theme.accent.on)
        } else {
            (
                Color32::TRANSPARENT,
                theme.borders.strong,
                theme.text.secondary,
            )
        };
        if fill != Color32::TRANSPARENT {
            painter.rect_filled(
                box_rect,
                radius::all(component::DIALOG_CHECKBOX_RADIUS),
                fill,
            );
        }
        widgets::rect_stroke(
            painter,
            box_rect,
            radius::all(component::DIALOG_CHECKBOX_RADIUS),
            Stroke::new(border::HAIRLINE, border_color),
        );
        if checked {
            painter.text(
                box_rect.center(),
                Align2::CENTER_CENTER,
                "\u{2713}",
                tokens::font_sans(12.0),
                glyph,
            );
        }
        ui.label(
            RichText::new(label)
                .font(tokens::font(ty::UI, theme))
                .color(theme.text.primary),
        );
        let _ = response;
    });
}

/// §2.10 `elev.1` + `elev.inner-highlight` on a real `surface.raised` card.
///
/// The gallery is the only place an overlay exists in this phase, and
/// elevation is a token rather than a vibe, so it gets rendered rather than
/// described.
fn elevation_card(ui: &mut Ui, theme: &Theme) {
    let size = vec2(220.0, 70.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    // `Frame::shadow` is egui's only shadow primitive — `RectShape` has no
    // `shadow` method, and `Painter` has no shadow entry point. Overlaying a
    // zero-inner-margin Frame is therefore the way to render a `Shadow` token
    // outside of a real egui window/menu.
    egui::Frame::new()
        .fill(theme.surfaces.raised)
        .corner_radius(radius::all(component::MENU_RADIUS))
        .shadow(tokens::elevation::menu_shadow(theme.is_dark))
        .show(ui, |ui| {
            ui.set_width(size.x);
            ui.set_height(size.y);
        });
    // `elev.inner-highlight` — light theme only, 1px `#FFFFFF` @ 60% on top.
    if !theme.is_dark {
        widgets::rule(
            ui.painter(),
            rect.left() + radius::LG,
            rect.right() - radius::LG,
            rect.top() + border::HAIRLINE,
            Stroke::new(border::HAIRLINE, tokens::elevation::inner_highlight()),
        );
    }
    widgets::rect_stroke(
        ui.painter(),
        rect,
        radius::all(component::MENU_RADIUS),
        Stroke::new(border::HAIRLINE, theme.borders.strong),
    );
    ui.painter().text(
        pos2(rect.left() + space::S3, rect.center().y - 8.0),
        Align2::LEFT_CENTER,
        "elev.1 + inner-highlight",
        tokens::font(ty::UI, theme),
        theme.text.primary,
    );
    // A machine value in mono, at an explicit size rather than a type token —
    // the case `font_mono` exists for.
    ui.painter().text(
        pos2(rect.left() + space::S3, rect.center().y + 8.0),
        Align2::LEFT_CENTER,
        "offset 6 / blur 18 / spread -4",
        tokens::font_mono(11.0),
        theme.text.secondary,
    );

    // `elev.3` — the drag ghost, the one other shadowed thing in the app.
    let ghost = Rect::from_min_size(
        pos2(rect.right() + space::S4, rect.top() + space::S2),
        vec2(90.0, 40.0),
    );
    egui::Frame::new()
        .fill(theme.accent.subtle_bg)
        .corner_radius(radius::all(component::ROW_RADIUS))
        .shadow(tokens::elevation::ghost_shadow(theme.is_dark))
        .show(ui, |ui| {
            ui.set_width(ghost.width());
            ui.set_height(ghost.height());
        });
    ui.painter().text(
        ghost.center(),
        Align2::CENTER_CENTER,
        "elev.3",
        tokens::font(ty::CAPTION, theme),
        theme.text.primary,
    );
}

/// What is real, and what is still a stand-in.
fn provenance(ui: &mut Ui, theme: &Theme, content_w: f32) {
    section(ui, theme, content_w, "Provenance \u{b7} vendored assets");
    for (role, family, faces) in [
        ("font.sans", tokens::fonts::SANS, "IBMPlexSans-Regular 400"),
        (
            "type.ui-strong / name-selected",
            tokens::fonts::SANS_500,
            "IBMPlexSans-Medium 500",
        ),
        (
            "type.label / ui-heading",
            tokens::fonts::SANS_600,
            "IBMPlexSans-SemiBold 600",
        ),
        ("font.mono", tokens::fonts::MONO, "IBMPlexMono-Regular 400"),
        (
            "type.meta-strong",
            tokens::fonts::MONO_500,
            "IBMPlexMono-Medium 500",
        ),
        ("icons (Regular)", tokens::fonts::ICONS, "Phosphor-Regular"),
        ("icons (Fill)", tokens::fonts::ICONS_FILL, "Phosphor-Fill"),
    ] {
        ui.label(
            RichText::new(format!("{role:28} {family:18} {faces}"))
                .font(tokens::font(ty::META, theme))
                .color(theme.text.secondary),
        );
    }
    ui.add_space(space::S1);
    ui.label(
        RichText::new(
            "One FontFamily per weight: epaint takes the FIRST face in a family that \
             has the glyph, so 400/500/600 in one list would render every run at 400.",
        )
        .font(tokens::font(ty::CAPTION, theme))
        .color(theme.text.tertiary),
    );

    ui.add_space(space::S2);
    let mut resolved = 0usize;
    let mut missing: Vec<&str> = Vec::new();
    for (name, glyph) in crate::icons::ALL {
        if glyph.is_available(ui.ctx()) {
            resolved += 1;
        } else {
            missing.push(name);
        }
    }
    ui.label(
        RichText::new(format!(
            "Phosphor glyphs present in the loaded font: {resolved}/{}",
            crate::icons::ALL.len()
        ))
        .font(tokens::font(ty::CAPTION, theme))
        .color(if missing.is_empty() {
            theme.status.success_text
        } else {
            theme.status.danger_text
        }),
    );
    if !missing.is_empty() {
        ui.label(
            RichText::new(format!("missing: {}", missing.join(", ")))
                .font(tokens::font(ty::MICRO, theme))
                .color(theme.status.danger_text),
        );
    }
    ui.add_space(space::S1);
    ui.label(
        RichText::new(
            "Names this Phosphor release spells differently: file-exe -> terminal (the \
             spec's own documented fallback), dots-three-horizontal -> dots-three (a \
             v1 to v2 rename, identical drawing). file-go / file-sh / file-json do not \
             exist in 2.x and fall back to file-code, per §5.2.",
        )
        .font(tokens::font(ty::CAPTION, theme))
        .color(theme.text.tertiary),
    );
}
