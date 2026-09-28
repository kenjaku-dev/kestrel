//! The few reusable widgets the shell and the gallery share.
//!
//! Everything here paints with tokens and *only* with tokens. There is no raw
//! hex in this file, and no value that is not a §3 semantic role or a §4
//! component token — that is the layer rule (§"Layer rule", hard) and it is
//! mechanically checkable by reading this file, which is why these helpers exist
//! rather than each call site hand-rolling its own `rect_filled`.

use egui::{Align2, Color32, FontId, Rangef, Rect, RichText, Sense, Stroke, Ui, vec2};

use crate::motion::Motion;
use crate::tokens::{self, Theme, border, component, metric, radius, space, ty};

/// Measures a string's rendered width.
///
/// `Painter::layout_no_wrap` is memoized by the galley cache, so calling this
/// once per row per frame is cheap — and it is *correct* where a
/// `chars * average` estimate is not, which matters for a name column where one
/// misplaced pixel of estimate puts the middle-truncation point in the wrong
/// place.
#[must_use]
pub fn text_width(ui: &Ui, text: &str, font: FontId) -> f32 {
    // `FontId` is not `Copy` in epaint 0.36 (it carries a fallback family list),
    // so the layout takes ownership; the caller re-derives it where it needs to
    // paint. The galley cache is keyed on the job, so the repeat is free.
    ui.painter()
        .layout_no_wrap(text.to_owned(), font, Color32::WHITE)
        .size()
        .x
}

/// Strokes a rectangle, always [`component::STROKE_KIND`].
///
/// epaint's `rect_stroke` requires a `StrokeKind` and defaults to `Middle`, which
/// splits a 1px line across two pixel rows. Every border in this app is
/// specified as an inset stroke, so the kind is fixed once here rather than
/// being re-decided at each of the ~30 call sites.
pub fn rect_stroke(
    painter: &egui::Painter,
    rect: Rect,
    corner_radius: impl Into<egui::CornerRadius>,
    stroke: Stroke,
) {
    painter.rect_stroke(rect, corner_radius, stroke, component::STROKE_KIND);
}

/// A 1px `border.subtle` rule spanning `x0..=x1` at `y`.
///
/// Wraps the `Painter::hline(Rangef, f32, Stroke)` signature so every call site
/// reads in the same order as the spec's token tables.
pub fn rule(painter: &egui::Painter, x0: f32, x1: f32, y: f32, stroke: Stroke) {
    painter.hline(Rangef::new(x0, x1), y, stroke);
}

/// A 1px `border.subtle` rule spanning `y0..=y1` at `x`.
pub fn vertical_rule(painter: &egui::Painter, x: f32, y0: f32, y1: f32, stroke: Stroke) {
    painter.vline(x, Rangef::new(y0, y1), stroke);
}

/// An 11px/600/uppercase/tracked section label in `text.tertiary`.
///
/// §4.1 `sidebar.item-section-label`, §4.6 `menu.section-label`.
pub fn section_label(ui: &mut Ui, theme: &Theme, text: &str) {
    ui.label(
        RichText::new(text.to_ascii_uppercase())
            .font(tokens::font(ty::LABEL, theme))
            .color(theme.text.tertiary),
    );
}

/// A flat toolbar button: transparent, `icon.chrome`, `state.hover`, 28x28.
///
/// §4.4. The label is optional in the spec (icon button vs icon + label); this
/// build has no icon font yet, so the label is what the button *is* — which also
/// means no icon-only button ships without a visible name, satisfying §7.14
/// ("no icon-only toolbar button without a tooltip and an accessible name") in
/// the strictest possible way.
pub fn toolbar_button(ui: &mut Ui, theme: &Theme, label: &str, active: bool) -> egui::Response {
    let height = component::TOOLBAR_BTN_HEIGHT;
    let font = if active {
        tokens::font(component::TOOLBAR_BTN_LABEL_ACTIVE, theme)
    } else {
        tokens::font(component::TOOLBAR_BTN_LABEL, theme)
    };

    // Width is the label plus symmetric `toolbar.btn` padding-x, never below the
    // 28px square so the button keeps its shape when the label is short.
    let text_w = text_width(ui, label, font.clone());
    let width = (text_w + component::TOOLBAR_BTN_LABEL_GAP * 2.0).max(component::TOOLBAR_BTN_WIDTH);
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());

    let (bg, fg) = if response.is_pointer_button_down_on() {
        (theme.state.pressed, theme.text.primary)
    } else if response.hovered() {
        (theme.state.hover, theme.text.primary)
    } else if active {
        // `toolbar.btn-toggled-on-bg` — the one place a toolbar button uses the
        // accent background.
        (theme.accent.subtle_bg, theme.text.primary)
    } else {
        (component::TOOLBAR_BTN_BG, theme.text.secondary)
    };

    if bg != Color32::TRANSPARENT {
        ui.painter()
            .rect_filled(rect, radius::all(component::TOOLBAR_BTN_RADIUS), bg);
    }
    if active {
        // `toolbar.btn-toggled-on-icon` — `accent.base`. The glyph would be
        // Fill-weight here; there is no icon font, so the label takes the
        // accent colour, which is the same information.
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            label,
            font,
            theme.accent.base,
        );
    } else {
        ui.painter()
            .text(rect.center(), Align2::CENTER_CENTER, label, font, fg);
    }
    response
}

/// A 1px `border.subtle` vertical rule, `statusbar.section-divider` (12px tall).
pub fn status_divider(ui: &mut Ui, theme: &Theme) {
    let (rect, _) = ui.allocate_exact_size(
        vec2(border::HAIRLINE, component::STATUSBAR_SECTION_DIVIDER_H),
        Sense::hover(),
    );
    vertical_rule(
        ui.painter(),
        rect.center().x,
        rect.y_range().min,
        rect.y_range().max,
        component::hairline(theme),
    );
}

/// `statusbar.label` — `type.status`, `text.tertiary`.
pub fn status_label(ui: &mut Ui, theme: &Theme, text: &str) {
    ui.label(
        RichText::new(text)
            .font(tokens::font(component::STATUSBAR_LABEL, theme))
            .color(theme.text.tertiary),
    );
}

/// `statusbar.value` — `type.meta`, `text.secondary`, a machine value.
pub fn status_value(ui: &mut Ui, theme: &Theme, text: &str) {
    ui.label(
        RichText::new(text)
            .font(tokens::font(component::STATUSBAR_VALUE, theme))
            .color(theme.text.secondary),
    );
}

/// `statusbar.selection-count` — `type.meta-strong`, `accent.text`.
///
/// §4.5 pairs this with `statusbar.selection-count-idle` (`text.tertiary`); a
/// non-zero selection is the only thing in the status bar that is allowed to be
/// accent-coloured, because it is the thing you are acting on (§1.4).
pub fn status_selection_count(ui: &mut Ui, theme: &Theme, text: &str) {
    ui.label(
        RichText::new(text)
            .font(tokens::font(component::STATUSBAR_SELECTION_COUNT, theme))
            .color(theme.accent.text),
    );
}

/// `statusbar.error` — a `warning` glyph in `status.danger-text` plus a label.
///
/// §3.6: "Status text is always accompanied by a glyph. A status colour never
/// appears without `warning` / `check-circle` / `x-circle` / `info` next to
/// it." Here the glyph is a `!` in a rounded box, standing in for Phosphor's
/// `warning` until the icon font lands.
pub fn status_error(ui: &mut Ui, theme: &Theme, text: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = vec2(space::S1, 0.0);
        let (rect, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(rect, radius::all(radius::SM), theme.status.danger_bg);
        rect_stroke(
            painter,
            rect,
            radius::all(radius::SM),
            Stroke::new(border::HAIRLINE, theme.status.danger_border),
        );
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "!",
            tokens::font_sans(9.0),
            theme.status.danger_text,
        );
        ui.label(
            RichText::new(text)
                .font(tokens::font(component::STATUSBAR_LABEL, theme))
                .color(theme.status.danger_text),
        );
    });
}

/// `statusbar.busy` — a 12px `circle-notch` plus a text label.
///
/// §7.16: "The app never blocks input and never shows a spinner with no
/// words." The label is therefore mandatory, not optional.
pub fn status_busy(ui: &mut Ui, theme: &Theme, label: &str, detail: &str, motion: Motion) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = vec2(space::S1_5, 0.0);
        let size = component::STATUSBAR_BUSY_ICON_SIZE;
        let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
        // §2.9 `motion.loop.spinner` — 900ms linear, an indeterminate arc.
        //
        // The one animation §2.11 rule 5 explicitly *exempts* from the
        // reduced-motion collapse, and `Motion::duration(_, spinner = true)`
        // is where that exemption is encoded. A spinner that does not move is
        // indistinguishable from a hung process, which is a worse outcome for
        // someone who asked for less motion than a slowly-turning arc is.
        //
        // The phase comes from the clock rather than an accumulated delta, so it
        // stays correct across a tab that was backgrounded (where dt explodes)
        // and across a frame that took 300ms because a scan was saturating the
        // disk. An accumulator would jump; a clock would not.
        let period = motion.duration(tokens::motion::SPINNER, true);
        let radius = rect.width() / 2.0;
        if period.is_zero() {
            // Reduced motion requested *and* the spinner exempted: fall back to
            // a full static ring, which still reads as "busy" without spinning.
            ui.painter().circle_stroke(
                rect.center(),
                radius,
                Stroke::new(border::THICK, theme.icon.chrome),
            );
        } else {
            // `f64::sin` of a zeroed clock is 0, not NaN, so the very first
            // frame paints a determinate arc rather than garbage.
            let secs = ui.input(|i| i.time);
            let phase = (secs * (1.0 / period.as_secs_f64())).rem_euclid(1.0);
            let (start, sweep) = arc_from_phase(phase);
            // `circle_stroke` is all-or-nothing, so a partial ring is a stroked
            // polyline. 16 segments over 90 degrees is 5.6 degrees each: at a
            // 10px radius the chord error is under 0.01px, which no screenshot
            // and no eye can resolve.
            const SEGMENTS: usize = 16;
            let points: Vec<egui::Pos2> = (0..=SEGMENTS)
                .map(|i| {
                    let a = start + sweep * (i as f32 / SEGMENTS as f32);
                    egui::pos2(
                        rect.center().x + radius * a.cos(),
                        rect.center().y + radius * a.sin(),
                    )
                })
                .collect();
            ui.painter()
                .add(egui::Shape::Path(egui::epaint::PathShape::line(
                    points,
                    Stroke::new(border::THICK, theme.icon.chrome),
                )));
        }
        ui.label(
            RichText::new(label)
                .font(tokens::font(component::STATUSBAR_LABEL, theme))
                .color(theme.text.secondary),
        );
        status_value(ui, theme, detail);
    });
}

/// The arc a spinner draws at a given phase.
///
/// A 90-degree arc sweeping once per loop. Split out as a pure function so the
/// sweep is testable: a spinner whose arc collapses to nothing at some phase
/// looks like a rendering glitch, and the bug would be invisible in a still
/// capture.
#[must_use]
fn arc_from_phase(phase: f64) -> (f32, f32) {
    // `rem_euclid` guards the seam: a phase of exactly 1.0 must be the same as
    // 0.0, or the loop visibly stutters once per revolution.
    let p = phase.rem_euclid(1.0);
    const SWEEP_DEGREES: f32 = 90.0;
    const START_DEGREES: f32 = -90.0; // 12 o'clock, where a spinner belongs
    let full = std::f32::consts::TAU.to_degrees();
    let start = START_DEGREES - p as f32 * full;
    (start.to_radians(), SWEEP_DEGREES.to_radians())
}

/// The free-space meter from §4.5: a 60x4 track in `state.hover` with an
/// `accent.base` fill.
///
/// `fraction` is clamped to 0..=1; a negative value renders an empty track and
/// a value over 1 renders a full one, because a meter that can overflow its
/// track is a bug, not a feature.
pub fn free_space_meter(ui: &mut Ui, theme: &Theme, fraction: f32) {
    let (rect, _) = ui.allocate_exact_size(
        vec2(component::STATUSBAR_METER_W, component::STATUSBAR_METER_H),
        Sense::hover(),
    );
    let painter = ui.painter();
    painter.rect_filled(rect, radius::all(radius::XS), theme.state.hover);
    let fill = fraction.clamp(0.0, 1.0);
    if fill > 0.0 {
        let fill_rect = Rect::from_min_size(rect.min, vec2(rect.width() * fill, rect.height()));
        painter.rect_filled(fill_rect, radius::all(radius::XS), theme.accent.base);
    }
}

/// A swatch: a token's colour as a filled rounded square, for the gallery.
///
/// Outlined in 1px `border.strong` so a near-white swatch (`surface.raised` in
/// the light theme is `#FFFFFF`) is still visible against a near-white gallery
/// background. Sizing comes from the §2.9 icon metrics rather than invented, so
/// the gallery's swatches sit on the same grid as the components they document.
pub fn swatch(ui: &mut Ui, theme: &Theme, color: Color32, size: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, radius::all(radius::MD), color);
    rect_stroke(
        painter,
        rect,
        radius::all(radius::MD),
        Stroke::new(border::HAIRLINE, theme.borders.strong),
    );
    response
}

/// The `metric.target-min` hit area around a smaller painted control.
///
/// §6.6: "every interactive element's hit area is >=24px even when the painted
/// element is smaller" — checkbox 14px painted / 24px hit, scrollbar thumb 6px
/// painted / 12px hit, hidden-file toggle 14px painted / 24px hit. This is that
/// rule, implemented once.
#[must_use]
pub fn hit_area(painted: egui::Vec2) -> egui::Vec2 {
    painted.max(egui::vec2(metric::TARGET_MIN, metric::TARGET_MIN))
}
