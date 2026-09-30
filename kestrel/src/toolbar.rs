//! §4.4 The toolbar: back/forward/up, path actions, view, sort, theme.
//!
//! # Why this replaced the sidebar toggle
//!
//! Phase 2 put the theme toggle in the sidebar because there was no toolbar
//! yet. §4.4 is unambiguous that these are *toolbar* controls, and the toolbar is
//! where the eye already is after a navigation — a control you have to look away
//! from the path to find is a control you stop using. The sidebar is now
//! exclusively Places, which is what §4.1 scopes it to.
//!
//! # Every button here is labelled, until there is no room to
//!
//! §7.14: "No icon-only toolbar button without a tooltip **and** an accessible
//! name." §4.4 defines *two* button variants — icon, and icon + label — and at
//! full width this build uses icon + label throughout, which satisfies §7.14 in
//! the strongest available way: the name is on screen, not behind a hover. The
//! tooltip carries the shortcut either way, as §4.11 requires ("Every one of
//! these appears in its tooltip as text, in the form `Rename · F2`").
//!
//! Below [`DENSITY_ICONS_BELOW`] the labels go and the *specified* icon variant
//! takes over. That is a variant rather than a degradation, and §7.14 stays
//! whole because the tooltip carries the name and the shortcut in this variant
//! too. See [`plan`].

use egui::{Align2, Rect, Response, Sense, Stroke, Ui, vec2};

use crate::icons;
use crate::tokens::{self, Theme, border, component, radius, space};

/// One toolbar control's identity, and what it did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// `Alt+Left` — step back in history.
    Back,
    /// `Alt+Right` — step forward in history.
    Forward,
    /// `Backspace` / `Alt+Up` — go to the parent directory.
    Up,
    /// `F5` — re-read the current directory.
    Refresh,
    /// `Ctrl+1` — flat list.
    ViewList,
    /// `Ctrl+2` — indented tree.
    ViewTree,
    /// `Ctrl+A` / the header — cycle the sort column.
    Sort,
    /// `Ctrl+F` — focus the filter field.
    Filter,
    /// Cycle Light → Dark → System.
    Theme,
    /// `Ctrl+,` — the settings screen.
    Settings,
    /// `F1` — the keyboard shortcut list.
    Help,
}

/// The filter field's preferred width, at full width.
const FILTER_PREF_W: f32 = 220.0;

/// The filter field's floor, below which the field is left out entirely.
///
/// 120px is the narrowest a `Filter…` field with a leading glyph and a clear
/// button is still usable, and it is the point past which the field is a
/// sliver: 90px of it is the placeholder, and typing replaces the placeholder
/// immediately so the query is never visible. Below the floor the field goes
/// away, and the `Filter` button — which keeps its name in *both* densities —
/// becomes the *only* way in, so nothing becomes unreachable.
///
/// # The button and the field are mutually exclusive
///
/// The two controls do the same job, and a labelled `Filter` button sitting
/// next to a `Filter…` field that is already on screen reads as a mistake
/// rather than as two things. So exactly one of them is ever in the row, and
/// which one is not a preference — it falls out of the layout:
///
/// * the field is shown when there is room for it, and then
///   [`Plan::shows`] hides the button;
/// * the button is shown only in the one stage where the field was left out
///   for space, and then it is the sole route to the field.
///
/// The button earns its place in exactly one situation, and that situation is
/// also the only one in which it is drawn. See [`Plan::shows`].
const FILTER_MIN_W: f32 = 120.0;

/// How much of each button the toolbar shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Density {
    /// Icon + label. §4.4's second variant; the name is on screen.
    Labelled,
    /// Icon only, centred in `toolbar.btn-width`. §4.4's default variant; the
    /// tooltip is the name.
    Icons,
}

/// What the toolbar decided to do at a given width.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plan {
    /// How much of each button to paint.
    pub density: Density,
    /// The filter field's width, or `None` to leave the field out of the row.
    pub filter: Option<f32>,
    /// Whether the row fits at all. `false` means the buttons alone overflow,
    /// which cannot be fixed by dropping labels — the app is narrower than its
    /// own minimum, and the status bar says so rather than the toolbar
    /// silently clipping.
    pub fits: bool,
}

impl Plan {
    /// Whether `button` belongs in the row this plan describes.
    ///
    /// Every button, always, except one: the `Filter` button is drawn **only**
    /// when the field has been left out. See [`FILTER_MIN_W`] for why the two
    /// are exclusive, and [`plan`] for the arithmetic that decides which.
    ///
    /// This is the single place the rule lives. The row builder asks it, the
    /// tests ask it, and nothing else decides — which is what stops "field
    /// preferred, button also drawn" from being a second, softer policy that
    /// drifts.
    #[must_use]
    pub fn shows(&self, button: &Button) -> bool {
        !(button.action == Action::Filter && self.filter.is_some())
    }
}

/// A control in the toolbar, with everything needed to paint and describe it.
#[derive(Debug, Clone, Copy)]
pub struct Button {
    /// What it does.
    pub action: Action,
    /// The §5.3 glyph.
    pub glyph: icons::Glyph,
    /// The visible label.
    pub label: &'static str,
    /// The §4.11 shortcut, shown in the tooltip.
    pub shortcut: &'static str,
    /// `false` disables the button (a back button with nowhere to go).
    pub enabled: bool,
    /// `true` when the button represents the current state (a view toggle).
    pub active: bool,
}

impl Button {
    /// A control.
    #[must_use]
    pub const fn new(
        action: Action,
        glyph: icons::Glyph,
        label: &'static str,
        shortcut: &'static str,
        enabled: bool,
        active: bool,
    ) -> Self {
        Self {
            action,
            glyph,
            label,
            shortcut,
            enabled,
            active,
        }
    }

    /// The tooltip text, in §4.11's `Name · Shortcut` form.
    #[must_use]
    pub fn tooltip(self) -> String {
        if self.shortcut.is_empty() {
            self.label.to_string()
        } else {
            format!("{} · {}", self.label, self.shortcut)
        }
    }
}

/// Builds the toolbar's buttons for the current state.
///
/// Pure, so the enabled/active wiring is testable without a window — a Back
/// button that is enabled with nowhere to go is the kind of thing that only
/// shows up as "it did nothing and I don't know why".
#[must_use]
pub fn buttons(
    can_back: bool,
    can_forward: bool,
    has_parent: bool,
    tree_view: bool,
    filter_active: bool,
) -> Vec<Button> {
    vec![
        Button::new(
            Action::Back,
            icons::ARROW_LEFT,
            "Back",
            "Alt+Left",
            can_back,
            false,
        ),
        Button::new(
            Action::Forward,
            icons::ARROW_RIGHT,
            "Forward",
            "Alt+Right",
            can_forward,
            false,
        ),
        Button::new(
            Action::Up,
            icons::ARROW_UP,
            "Up",
            "Alt+Up",
            has_parent,
            false,
        ),
        Button::new(
            Action::Refresh,
            icons::ARROWS_CLOCKWISE,
            "Refresh",
            "F5",
            true,
            false,
        ),
        Button::new(
            Action::ViewList,
            icons::LIST_BULLETS,
            "List",
            "Ctrl+1",
            true,
            !tree_view,
        ),
        Button::new(
            Action::ViewTree,
            icons::ROWS,
            "Tree",
            "Ctrl+2",
            true,
            tree_view,
        ),
        Button::new(
            Action::Filter,
            icons::MAGNIFYING_GLASS,
            "Filter",
            "Ctrl+F",
            true,
            filter_active,
        ),
        Button::new(
            Action::Sort,
            icons::CARET_UP_DOWN,
            "Sort",
            "click a column",
            true,
            false,
        ),
        Button::new(
            Action::Theme,
            icons::CIRCLE_HALF,
            "Theme",
            "Ctrl+T",
            true,
            false,
        ),
        Button::new(
            Action::Settings,
            icons::GEAR,
            "Settings",
            "Ctrl+,",
            true,
            false,
        ),
        Button::new(Action::Help, icons::KEYBOARD, "Help", "F1", true, false),
    ]
}

/// Whether a separator is drawn before `b`.
///
/// §4.4: "Proximity does the grouping, so separators are only between *groups*,
/// never between adjacent buttons." The groups are the navigation cluster
/// (Back, Forward, Up), the icon row's own breaks, and the filter — a rule
/// before `Refresh`, before `ViewTree`, and before `Filter`.
///
/// The rule identifies a button by *what it is* rather than by where it
/// happens to sit, because [`Plan::shows`] removes the `Filter` button from the
/// row whenever the field is drawn. A position-based rule would then draw a
/// rule beside the wrong control and shift every separator after it; naming the
/// button means a removed button takes its rule with it and the rest of the
/// row is untouched.
///
/// [`Plan::shows`]: crate::toolbar::Plan::shows
#[must_use]
pub fn separator_before(b: &Button) -> bool {
    matches!(
        b.action,
        Action::Refresh | Action::ViewTree | Action::Filter
    )
}

/// How wide one button is in a given density.
///
/// Pure, so the layout decision can be tested without a window — a toolbar that
/// overflows is exactly the defect a screenshot of the *default* size cannot
/// show, and it was found at 760px only because someone asked.
#[must_use]
pub fn button_width(ui: &Ui, theme: &Theme, b: &Button, density: Density) -> f32 {
    match density {
        Density::Icons => component::TOOLBAR_BTN_WIDTH,
        Density::Labelled => {
            let font = if b.active {
                tokens::font(component::TOOLBAR_BTN_LABEL_ACTIVE, theme)
            } else {
                tokens::font(component::TOOLBAR_BTN_LABEL, theme)
            };
            let text_w = crate::widgets::text_width(ui, b.label, font);
            (text_w
                + component::TOOLBAR_BTN_ICON_SIZE
                + component::TOOLBAR_BTN_LABEL_GAP * 2.0
                + component::TOOLBAR_BTN_LABEL_GAP)
                .max(component::TOOLBAR_BTN_WIDTH)
        }
    }
}

/// The total width of every separator in the row.
const SEPARATOR_W: f32 = (border::HAIRLINE + component::TOOLBAR_BTN_SEPARATOR_MARGIN * 2.0) * 3.0;

/// The toolbar's own inner padding, both sides.
const FRAME_PAD_X: f32 = space::S2 * 2.0;

/// The width the row needs at one density, with and without the `Filter` button.
///
/// Two numbers rather than one because the button and the field are exclusive
/// and the row has to be measured both ways: with the field drawn the `Filter`
/// button is not in the row, and with the button drawn there is no field. Adding
/// a button's width and then also reserving the field's is the arithmetic that
/// produced a toolbar with two filter controls and a row that overflowed.
fn row_width(
    ui: &Ui,
    theme: &Theme,
    btns: &[Button],
    density: Density,
    with_filter_button: bool,
) -> f32 {
    let mut total = FRAME_PAD_X;
    for b in btns {
        if b.action == Action::Filter && !with_filter_button {
            // Skipped, and so is the separator that would have preceded it —
            // the same decision [`Plan::shows`] makes at paint time.
            continue;
        }
        if separator_before(b) {
            total += SEPARATOR_W / 3.0;
        }
        total += button_width(ui, theme, b, density);
    }
    total
}

/// The space the filter field needs beyond its own width: a separator and the
/// gap before it.
const FILTER_CHROME: f32 = border::HAIRLINE + component::TOOLBAR_BTN_SEPARATOR_MARGIN * 2.0;

/// Decides the toolbar's density, filter width, and — by exclusion — whether the
/// `Filter` button is in the row at all.
///
/// # Why a decision function and not a `clamp`
///
/// The two defects this replaces were both "clamp and hope": a filter field
/// that took `min(220, available)` and so rendered a 91px field with its right
/// half outside the panel, and a button row that ran past the window's edge
/// with no indication that it had. Neither is fixed by shrinking things; both
/// are fixed by *choosing a layout*.
///
/// The choice is §4.4's own two variants, in priority order:
///
/// 1. Labels on, filter at [`FILTER_PREF_W`] — the wide layout. The `Filter`
///    button is *not* in this row; the field is the control.
/// 2. Labels off, filter at [`FILTER_PREF_W`] — §4.4's icon variant. This is
///    what happens below ~912px available, which is where the labelled row
///    stops fitting: the ten drawn buttons (≈624.9px of icon + label at a 4px
///    `toolbar.btn-gap`) plus 16px frame padding, two 17px separators, 17px
///    filter chrome and the 220px field total ≈911.9px. Do not trust the
///    number — recompute it by sweeping `plan` over widths (see
///    `labels_are_used_exactly_when_the_labelled_row_fits`), because it moves
///    with the font metrics and the gap token.
/// 3. Labels off, filter at whatever is left, down to [`FILTER_MIN_W`].
/// 4. Labels off, no filter field — and *only* now does the `Filter` button
///    appear, because it is the only remaining way to reach the field.
///
/// The filter field is the thing that gives way, not the buttons: a button that
/// is not there cannot be pressed, while a narrow filter still filters.
///
/// # Why this converges in one pass
///
/// Dropping the button frees width, and freed width can promote an earlier
/// stage — which frees the field's width too, which is a fixed point, because
/// promoting further would require *more* width than promoting just did. The
/// stages are therefore tried in order against a *precomputed* measurement of
/// the row each one would actually draw, rather than against a running total
/// that is mutated as the answer is discovered. There is no loop to diverge.
///
/// [`Plan::shows`] is what makes the two controls exclusive at paint time; the
/// arithmetic here is what makes it consistent at test time.
#[must_use]
pub fn plan(ui: &Ui, theme: &Theme, available: f32, btns: &[Button]) -> Plan {
    // Rows *with* the field: the `Filter` button is not drawn, so the row is
    // whatever is left without it.
    let labelled = row_width(ui, theme, btns, Density::Labelled, false);
    let icons = row_width(ui, theme, btns, Density::Icons, false);
    // The last stage's row: the button is back, and the field is gone.
    let icons_with_button = row_width(ui, theme, btns, Density::Icons, true);

    // The last group is followed by a separator before the filter field, in
    // every density, because the field is chrome rather than a control group.
    let with_field = |row: f32| row + FILTER_CHROME + FILTER_PREF_W;
    if with_field(labelled) <= available {
        return Plan {
            density: Density::Labelled,
            filter: Some(FILTER_PREF_W),
            fits: true,
        };
    }
    if with_field(icons) <= available {
        return Plan {
            density: Density::Icons,
            filter: Some(FILTER_PREF_W),
            fits: true,
        };
    }
    let left = available - icons - FILTER_CHROME;
    if left >= FILTER_MIN_W {
        return Plan {
            density: Density::Icons,
            filter: Some(left.min(FILTER_PREF_W)),
            fits: true,
        };
    }
    // Stage 4: the field does not fit, so the `Filter` button is the control.
    Plan {
        density: Density::Icons,
        filter: None,
        fits: icons_with_button <= available,
    }
}

/// Draws one control and returns whether it was clicked.
///
/// `ahead` is how many entries the Forward stack holds; it is surfaced in the
/// tooltip so the button advertises what it will actually do, which is §7.14's
/// "no control without a name" taken one step further.
///
/// `density` selects §4.4's two variants. The icon variant is not a stripped
/// version of the labelled one: it centres the glyph in `toolbar.btn-width`,
/// paints no label at all, and carries the name **and** the shortcut in the
/// tooltip — which is exactly what §4.4 specifies for it and what keeps §7.14
/// whole at narrow widths.
///
/// §4.4: 28px tall, 4px radius, transparent at rest, `state.hover`,
/// `state.pressed`, `accent.subtle-bg` when on. The `Fill`-weight glyph on an
/// active button is one of exactly two places §5.1 allows Fill.
pub fn draw(ui: &mut Ui, theme: &Theme, b: Button, ahead: usize, density: Density) -> Response {
    let height = component::TOOLBAR_BTN_HEIGHT;
    let font = if b.active {
        tokens::font(component::TOOLBAR_BTN_LABEL_ACTIVE, theme)
    } else {
        tokens::font(component::TOOLBAR_BTN_LABEL, theme)
    };
    let width = button_width(ui, theme, &b, density);

    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());

    // §4.4 `toolbar.btn-disabled`: 40% icon, `text.disabled`, **no hover**.
    if !b.enabled {
        paint_bg(ui, rect, theme, b, false);
        let c = component::icon_at(
            if b.active {
                theme.accent.base
            } else {
                theme.icon.chrome
            },
            0.4,
        );
        paint_content(ui, rect, theme, b, font, c, c, false, density);
        return response.on_hover_text(b.tooltip());
    }

    paint_bg(ui, rect, theme, b, response.hovered());

    // §4.4: at rest the label is `text.secondary`; hover *and* press both
    // promote it to `text.primary`, and only the background distinguishes them.
    // Two states sharing a foreground is deliberate — it is what makes a press
    // read as a press rather than a colour change.
    let (fg, icon_color) = if b.active {
        // `toolbar.btn-toggled-on-icon` — `accent.base`, Fill weight.
        (theme.text.primary, theme.accent.base)
    } else if response.hovered() || response.is_pointer_button_down_on() {
        (theme.text.primary, theme.icon.chrome)
    } else {
        (theme.text.secondary, theme.icon.chrome)
    };

    paint_content(ui, rect, theme, b, font, icon_color, fg, b.active, density);
    response.on_hover_text(if b.action == Action::Forward && ahead > 0 {
        format!("{} · {ahead} ahead", b.tooltip())
    } else {
        b.tooltip()
    })
}

fn paint_bg(ui: &Ui, rect: Rect, theme: &Theme, b: Button, hovered: bool) {
    let pressed = ui.input(|i| i.pointer.any_down());
    let bg = if b.active {
        theme.accent.subtle_bg
    } else if !b.enabled {
        component::TOOLBAR_BTN_BG
    } else if pressed && hovered {
        theme.state.pressed
    } else if hovered {
        theme.state.hover
    } else {
        component::TOOLBAR_BTN_BG
    };
    if bg != egui::Color32::TRANSPARENT {
        ui.painter()
            .rect_filled(rect, radius::all(component::TOOLBAR_BTN_RADIUS), bg);
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_content(
    ui: &Ui,
    rect: Rect,
    _theme: &Theme,
    b: Button,
    font: egui::FontId,
    icon_color: egui::Color32,
    fg: egui::Color32,
    fill: bool,
    density: Density,
) {
    let icon_size = component::TOOLBAR_BTN_ICON_SIZE;
    // In the icon variant the glyph is *centred* in `toolbar.btn-width`; in
    // the labelled one it sits at the left padding. Two different origins, and
    // using the labelled one for both is what leaves a 28px button with a
    // glyph hanging off its left edge.
    let left = match density {
        Density::Labelled => rect.left() + component::TOOLBAR_BTN_LABEL_GAP,
        Density::Icons => rect.center().x - icon_size / 2.0,
    };
    let icon_rect = Rect::from_center_size(
        egui::pos2(left + icon_size / 2.0, rect.center().y),
        vec2(icon_size, icon_size),
    );
    // The icon font is asked for the **rect width**, so the glyph scales with
    // §5.1's grid rather than being drawn at a fixed point size and clipped.
    let id = tokens::font_icon(icon_size, false);
    // §4.4 `toolbar.btn-toggled-on-icon`: `accent.base` in the **Fill** face.
    // One of exactly two places §5.1 permits Fill.
    if fill {
        tokens::icon_glyph_fill(ui.painter(), icon_rect, b.glyph, icon_color);
    } else {
        ui.painter().text(
            icon_rect.center(),
            Align2::CENTER_CENTER,
            b.glyph.char(),
            id,
            icon_color,
        );
    }
    if density == Density::Labelled {
        let text_x = icon_rect.right() + component::TOOLBAR_BTN_LABEL_GAP;
        ui.painter().text(
            egui::pos2(text_x, rect.center().y),
            Align2::LEFT_CENTER,
            b.label,
            font,
            fg,
        );
    }
}

/// A 1px `border.subtle` vertical rule between functional groups.
///
/// §4.4 `toolbar.btn-separator` — 1px `border.subtle`, 16px tall, 8px margins.
/// Proximity does the grouping, so separators are only between *groups*, never
/// between adjacent buttons.
pub fn separator(ui: &mut Ui, theme: &Theme) {
    let (rect, _) = ui.allocate_exact_size(
        vec2(border::HAIRLINE, component::TOOLBAR_BTN_SEPARATOR_H),
        Sense::hover(),
    );
    crate::widgets::vertical_rule(
        ui.painter(),
        rect.center().x,
        rect.y_range().min,
        rect.y_range().max,
        component::hairline(theme),
    );
    ui.add_space(component::TOOLBAR_BTN_SEPARATOR_MARGIN);
}

/// The frame for the whole toolbar.
///
/// §4.4 `toolbar.transition` is `motion.fast`, colour only, and §2.11 rule 2
/// forbids animating layout — so the frame is static. It is a *layout sibling*
/// with a 1px border, never an overlay (§2.12), which is what makes WCAG 2.2
/// focus-not-obscured structural rather than a scroll offset.
pub fn frame(theme: &Theme) -> egui::Frame {
    egui::Frame::new()
        .fill(theme.surfaces.chrome)
        .inner_margin(egui::Margin::symmetric(
            space::S2 as i8,
            (component::TOOLBAR_BTN_HEIGHT / 2.0).round() as i8,
        ))
        .stroke(Stroke::new(border::HAIRLINE, theme.borders.subtle))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real `Ui` with the real fonts, so [`plan`]'s text measurement is the
    /// measurement the app will make.
    ///
    /// A `Ui` per width rather than a `Context` per width, because `Context`
    /// creation is the expensive part and `plan` only needs a painter with a
    /// font atlas.
    fn with_ui(width: f32, body: impl FnOnce(&Ui, &Theme)) {
        let ctx = crate::shot::ctx_with_fonts();
        let mut body = Some(body);
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(width, 200.0),
                )),
                ..Default::default()
            },
            |ui| {
                if let Some(body) = body.take() {
                    body(ui, &Theme::dark());
                }
            },
        );
        // The first pass builds the font atlas; dropping the delta unapplied
        // makes egui panic, and a test about widths should not die on fonts.
        out.drop_without_applying_deltas();
    }

    /// Every button row at a given width, and the plan for it.
    fn plan_at(width: f32) -> Plan {
        let mut out = Plan {
            density: Density::Labelled,
            filter: Some(FILTER_PREF_W),
            fits: true,
        };
        with_ui(width, |ui, theme| {
            out = plan(ui, theme, width, &buttons(true, true, true, false, false));
        });
        out
    }

    /// The buttons the plan would actually draw, in order.
    fn visible<'a>(p: &Plan, btns: &'a [Button]) -> Vec<&'a Button> {
        btns.iter().filter(|b| p.shows(b)).collect()
    }

    // -- the two regressions this function exists to prevent ---------------

    /// At 760px the filter field used to lose its right half, because the
    /// field took `min(220, available)` and then drew a 220px-wide field's
    /// worth of chrome — clear button, text inset and all — into the space it
    /// had. The plan has to give the field a width it can actually paint in.
    #[test]
    fn the_filter_field_is_never_wider_than_the_space_left_for_it() {
        for width in [
            1600.0, 1200.0, 1060.0, 1000.0, 900.0, 820.0, 760.0, 640.0, 560.0, 520.0,
        ] {
            let p = plan_at(width);
            let Some(field) = p.filter else {
                panic!("{width}px dropped the filter field; the icon row fits easily there");
            };
            // The buttons that are actually drawn, at whichever density, plus the
            // field's own chrome plus the frame padding, must fit.
            with_ui(width, |ui, theme| {
                let btns = buttons(true, true, true, false, false);
                let used: f32 = visible(&p, &btns)
                    .iter()
                    .map(|b| {
                        let sep = if separator_before(b) {
                            SEPARATOR_W / 3.0
                        } else {
                            0.0
                        };
                        sep + button_width(ui, theme, b, p.density)
                    })
                    .sum::<f32>()
                    + FRAME_PAD_X
                    + FILTER_CHROME;
                assert!(
                    used + field <= width + 0.5,
                    "{width}px: buttons {used} + filter {field} overflows"
                );
            });
        }
    }

    /// The labelled row used to overflow the window and the buttons ran off the
    /// right edge with nothing saying so.
    ///
    /// The breakpoint moved from ~1060px to ~975px when the `Filter` button
    /// stopped sharing the row with the field: the labelled row is ~86px
    /// narrower without it, so it now fits in a window it used to overflow. A
    /// test that asserted the old number would be asserting the duplication.
    /// What is pinned here is the *behaviour* — labels exactly when the labelled
    /// row fits, icons exactly when it does not — and the number falls out of
    /// the font, which is the right place for it to live.
    #[test]
    fn the_labelled_row_is_used_exactly_when_it_fits() {
        // Boundary is ~912px available (ten labelled buttons ≈ 624.9px + 16px
        // frame + two 17px separators + 17px filter chrome + 220px field ≈
        // 911.9px). Was ~972px before the `toolbar.btn-gap` trim; the lists
        // below moved with it, and the invariant test underneath pins the rule
        // rather than any one number.
        for width in [
            1600.0, 1400.0, 1200.0, 1100.0, 1000.0, 972.0, 960.0, 940.0, 920.0,
        ] {
            assert_eq!(plan_at(width).density, Density::Labelled, "{width}px");
        }
        for width in [900.0, 820.0, 760.0, 700.0, 640.0] {
            let p = plan_at(width);
            assert_eq!(p.density, Density::Icons, "{width}px");
            assert!(p.fits, "{width}px fits the icon row");
        }
    }

    /// The invariant behind the numbers: labels are used **iff** the labelled row
    /// fits beside the filter field, at every width.
    ///
    /// The table above pins the widths anyone will actually try — 1200, 820,
    /// 760 — because those are the ones that were reported broken. This pins the
    /// rule, because a rule that only holds at three widths is a coincidence.
    #[test]
    fn labels_are_used_exactly_when_the_labelled_row_fits() {
        for width in (360..=1600).step_by(4) {
            let w = width as f32;
            with_ui(w, |ui, theme| {
                let btns = buttons(true, true, true, false, false);
                // The row as it would be drawn with the field in it: no
                // `Filter` button, plus the field's separator and chrome.
                let row = |density: Density| -> f32 {
                    row_width(ui, theme, &btns, density, false) + FILTER_CHROME + FILTER_PREF_W
                };
                let p = plan(ui, theme, w, &btns);
                if row(Density::Labelled) <= w {
                    assert_eq!(p.density, Density::Labelled, "{w}px: it fits");
                } else {
                    assert_eq!(p.density, Density::Icons, "{w}px: it does not fit");
                }
            });
        }
    }

    /// The user's case: at a 972px window the labelled row must survive.
    ///
    /// Pinned because a 972px window once fell back to icons — several of
    /// which (Sort, Theme, Settings, Help) are ambiguous without text — while
    /// the stale doc comment claimed the labelled row survived down to 820px.
    /// Before the `toolbar.btn-gap` trim (6px → 4px) the row needed ~971.9px,
    /// so 972px passed by a tenth of a pixel and any sub-pixel variance in a
    /// real window flipped it to icons. The 960px assertion is the part that
    /// actually failed before the fix; it pins the ~60px of margin the trim
    /// bought, so rounding can never flip the density at the user's size.
    #[test]
    fn labels_survive_a_972px_window() {
        let p = plan_at(972.0);
        assert_eq!(p.density, Density::Labelled, "972px must keep labels");
        assert!(p.fits, "972px fits the labelled row");
        assert_eq!(p.filter, Some(FILTER_PREF_W));
        assert_eq!(
            plan_at(960.0).density,
            Density::Labelled,
            "960px must keep labels with margin to spare"
        );
    }

    /// The icon row is 10 * 28px plus separators; a window narrower than
    /// that cannot show the toolbar at all, and the plan says so rather than
    /// letting the buttons be clipped without comment.
    #[test]
    fn a_window_narrower_than_the_icon_row_is_reported() {
        with_ui(200.0, |ui, theme| {
            let p = plan(ui, theme, 200.0, &buttons(true, true, true, false, false));
            assert!(!p.fits, "200px cannot fit ten 28px buttons and their rules");
            assert_eq!(p.density, Density::Icons, "icons are already the floor");
            assert_eq!(p.filter, None, "and the field is the first thing to go");
        });
    }

    /// The filter field gives way before the buttons do, and only after
    /// `FILTER_MIN_W` — below that it is a sliver with no visible query.
    #[test]
    fn the_filter_field_gives_way_in_stages() {
        with_ui(1000.0, |ui, theme| {
            let btns = buttons(true, true, true, false, false);
            assert_eq!(
                plan(ui, theme, 1000.0, &btns).filter,
                Some(FILTER_PREF_W),
                "there is room for the preferred width"
            );
            // Somewhere between the icon row and `FILTER_PREF_W`.
            let icons: f32 = btns
                .iter()
                .filter(|b| b.action != Action::Filter)
                .map(|b| {
                    // The icon row's width does not depend on which button it
                    // is: every one is `toolbar.btn-width`.
                    (if separator_before(b) {
                        SEPARATOR_W / 3.0
                    } else {
                        0.0
                    }) + component::TOOLBAR_BTN_WIDTH
                })
                .sum();
            let chrome = FRAME_PAD_X + icons + FILTER_CHROME;
            let narrow = chrome + FILTER_MIN_W;
            let p = plan(ui, theme, narrow, &btns);
            assert_eq!(p.density, Density::Icons);
            assert_eq!(
                p.filter,
                Some(FILTER_MIN_W),
                "exactly at the floor the field is still there"
            );
            let p = plan(ui, theme, narrow - 1.0, &btns);
            assert_eq!(p.filter, None, "one pixel below the floor it is gone");
        });
    }

    // -- the duplicated filter control this file's layout exists to prevent --

    /// The `Filter` button and the `Filter…` field are the same job, and a
    /// labelled button beside a field that is already on screen reads as a
    /// mistake. They are mutually exclusive: the field is the control whenever
    /// it fits, and the button is drawn only in the one stage where the field
    /// was dropped for space.
    ///
    /// The widths are the ones the report named, plus a sweep, because a rule
    /// that holds at five widths is a coincidence rather than a rule.
    #[test]
    fn the_filter_button_and_the_field_are_never_both_present() {
        for width in [320.0, 500.0, 760.0, 1000.0, 1400.0] {
            let p = plan_at(width);
            let btns = buttons(true, true, true, false, false);
            let shown = visible(&p, &btns);
            let button = shown.iter().any(|b| b.action == Action::Filter);
            assert!(
                !(button && p.filter.is_some()),
                "{width}px: button={button}, field={:?} — exactly one, never both",
                p.filter
            );
        }
        for width in (200..=1600).step_by(3) {
            let w = width as f32;
            let p = plan_at(w);
            let btns = buttons(true, true, true, false, false);
            let button = visible(&p, &btns)
                .iter()
                .any(|b| b.action == Action::Filter);
            assert_eq!(
                button,
                p.filter.is_none(),
                "{w}px: the button is the way in exactly when the field is out"
            );
        }
    }

    /// The two controls are not merely "field preferred" — the button is *only*
    /// in the row when the field is out, so the field's presence removes a
    /// button rather than sitting next to one.
    #[test]
    fn the_filter_button_appears_only_where_the_field_was_dropped() {
        // 1000px: the field is at its preferred width and the button is gone.
        let wide = plan_at(1000.0);
        assert_eq!(wide.filter, Some(FILTER_PREF_W));
        let btns = buttons(true, true, true, false, false);
        assert!(
            !visible(&wide, &btns)
                .iter()
                .any(|b| b.action == Action::Filter),
            "1000px shows the field, so the button must not be in the row"
        );

        // Far enough down that the field is dropped, the button is the control.
        let narrow = plan_at(440.0);
        assert_eq!(narrow.filter, None, "440px has no room for the field");
        assert!(
            visible(&narrow, &btns)
                .iter()
                .any(|b| b.action == Action::Filter),
            "with the field gone the button is the only way to it"
        );
    }

    /// Dropping the button frees width, which could in principle promote an
    /// earlier stage, which frees more. It has to settle: at every width the
    /// row the plan chose actually fits, and the stage it chose is the first
    /// one that does.
    #[test]
    fn the_stages_settle_in_one_pass() {
        for width in (200..=1600).step_by(3) {
            let w = width as f32;
            let p = plan_at(w);
            with_ui(w, |ui, theme| {
                let btns = buttons(true, true, true, false, false);
                let drawn: f32 = visible(&p, &btns)
                    .iter()
                    .map(|b| {
                        (if separator_before(b) {
                            SEPARATOR_W / 3.0
                        } else {
                            0.0
                        }) + button_width(ui, theme, b, p.density)
                    })
                    .sum::<f32>()
                    + FRAME_PAD_X
                    + p.filter.unwrap_or(0.0)
                    + if p.filter.is_some() {
                        FILTER_CHROME
                    } else {
                        0.0
                    };
                if p.fits {
                    assert!(
                        drawn <= w + 0.5,
                        "{w}px: the row it chose ({drawn}px) overflows"
                    );
                }
                // A strictly smaller width must never also have been acceptable,
                // or the stage order is not a total order and the result is not
                // a fixed point.
                if let Some(f) = p.filter {
                    let shrunk = row_width(ui, theme, &btns, p.density, false)
                        + FILTER_CHROME
                        + f.min(FILTER_PREF_W);
                    assert!(
                        shrunk <= w + 0.5,
                        "{w}px: it chose a layout that needs {shrunk}px"
                    );
                }
            });
        }
    }

    /// The plan never proposes a density the spec does not define, and never a
    /// filter width outside the field's own range.
    #[test]
    fn every_plan_is_inside_the_tokens() {
        for width in (240..=1400).step_by(7) {
            let w = width as f32;
            let p = plan_at(w);
            assert!(matches!(p.density, Density::Icons | Density::Labelled));
            if let Some(f) = p.filter {
                assert!(f > 0.0 && f <= FILTER_PREF_W, "{w}px: field {f}");
            }
        }
    }

    #[test]
    fn back_and_forward_are_disabled_when_there_is_nowhere_to_go() {
        let bs = buttons(false, false, true, false, false);
        let by = |a: Action| bs.iter().find(|b| b.action == a).copied();
        assert!(!by(Action::Back).expect("back button").enabled);
        assert!(!by(Action::Forward).expect("forward button").enabled);
        assert!(by(Action::Up).expect("up button").enabled);
    }

    #[test]
    fn back_and_forward_enable_when_history_has_depth() {
        let bs = buttons(true, false, true, false, false);
        let by = |a: Action| bs.iter().find(|b| b.action == a).copied();
        assert!(by(Action::Back).expect("back").enabled);
        assert!(
            !by(Action::Forward).expect("forward").enabled,
            "forward has nothing to reach"
        );
    }

    #[test]
    fn up_is_disabled_at_the_filesystem_root() {
        // `Path::parent` on "/" is None; the toolbar must reflect that rather
        // than offering a button that does nothing.
        let bs = buttons(false, false, false, false, false);
        let up = bs.iter().find(|b| b.action == Action::Up).copied();
        assert!(!up.expect("up button").enabled);
    }

    #[test]
    fn exactly_one_view_button_is_active() {
        for tree in [false, true] {
            let bs = buttons(false, false, true, tree, false);
            let active: Vec<Action> = bs
                .iter()
                .filter(|b| b.active)
                .map(|b| b.action)
                .filter(|a| matches!(a, Action::ViewList | Action::ViewTree))
                .collect();
            assert_eq!(
                active.len(),
                1,
                "tree={tree} must activate exactly one view"
            );
            assert_eq!(
                active[0],
                if tree {
                    Action::ViewTree
                } else {
                    Action::ViewList
                }
            );
        }
    }

    #[test]
    fn the_filter_button_reflects_a_live_filter() {
        let bs = buttons(false, false, true, false, true);
        let filter = bs.iter().find(|b| b.action == Action::Filter).copied();
        assert!(filter.expect("filter button").active);
    }

    #[test]
    fn every_button_has_a_label_a_glyph_and_an_accessible_name() {
        // §7.14: no icon-only control without a name. Label-on-screen is the
        // strongest form; the tooltip is the discoverability channel.
        for b in buttons(true, true, true, false, false) {
            assert!(!b.label.is_empty(), "{:?} has no label", b.action);
            assert!(b.glyph.0 > 0xE000, "{:?} has no glyph", b.action);
            assert!(
                b.tooltip().starts_with(b.label),
                "{:?} tooltip must start with its name",
                b.action
            );
        }
    }

    #[test]
    fn tooltips_use_the_specs_name_dash_shortcut_form() {
        // §4.11: "in the form `Rename · F2`".
        let b = Button::new(
            Action::Back,
            icons::ARROW_LEFT,
            "Back",
            "Alt+Left",
            true,
            false,
        );
        assert_eq!(b.tooltip(), "Back · Alt+Left");
        let no_shortcut = Button::new(Action::Sort, icons::CARET_UP_DOWN, "Sort", "", true, false);
        assert_eq!(
            no_shortcut.tooltip(),
            "Sort",
            "no trailing separator when unbound"
        );
    }

    #[test]
    fn the_toolbar_covers_every_binding_it_advertises() {
        let bs = buttons(true, true, true, false, false);
        for (action, shortcut) in [
            (Action::Back, "Alt+Left"),
            (Action::Forward, "Alt+Right"),
            (Action::Up, "Alt+Up"),
            (Action::Refresh, "F5"),
            (Action::ViewList, "Ctrl+1"),
            (Action::ViewTree, "Ctrl+2"),
            (Action::Filter, "Ctrl+F"),
        ] {
            let b = bs.iter().find(|b| b.action == action).copied();
            assert_eq!(
                b.expect("button exists").shortcut,
                shortcut,
                "{action:?} advertises the wrong shortcut"
            );
        }
    }
}
