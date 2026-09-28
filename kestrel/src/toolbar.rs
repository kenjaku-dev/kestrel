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
//! # Every button here is labelled
//!
//! §7.14: "No icon-only toolbar button without a tooltip **and** an accessible
//! name." The spec's own `toolbar.btn` variants are *icon* and *icon + label*;
//! this build uses icon + label throughout, which satisfies §7.14 in the
//! strongest available way — the name is on screen, not behind a hover. The
//! tooltip still carries the shortcut, as §4.11 requires ("Every one of these
//! appears in its tooltip as text, in the form `Rename · F2`").

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
    ]
}

/// Draws one control and returns whether it was clicked.
///
/// `ahead` is how many entries the Forward stack holds; it is surfaced in the
/// tooltip so the button advertises what it will actually do, which is §7.14's
/// "no control without a name" taken one step further.
///
/// §4.4: 28px tall, 4px radius, transparent at rest, `state.hover`,
/// `state.pressed`, `accent.subtle-bg` when on. The `Fill`-weight glyph on an
/// active button is one of exactly two places §5.1 allows Fill.
pub fn draw(ui: &mut Ui, theme: &Theme, b: Button, ahead: usize) -> Response {
    let height = component::TOOLBAR_BTN_HEIGHT;
    let font = if b.active {
        tokens::font(component::TOOLBAR_BTN_LABEL_ACTIVE, theme)
    } else {
        tokens::font(component::TOOLBAR_BTN_LABEL, theme)
    };
    let text_w = crate::widgets::text_width(ui, b.label, font.clone());
    let width = (text_w
        + component::TOOLBAR_BTN_ICON_SIZE
        + component::TOOLBAR_BTN_LABEL_GAP * 2.0
        + component::TOOLBAR_BTN_LABEL_GAP)
        .max(component::TOOLBAR_BTN_WIDTH);

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
        paint_content(ui, rect, theme, b, font, c, c, false);
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

    paint_content(ui, rect, theme, b, font, icon_color, fg, b.active);
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
) {
    let icon_size = component::TOOLBAR_BTN_ICON_SIZE;
    let left = rect.left() + component::TOOLBAR_BTN_LABEL_GAP;
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
    let text_x = icon_rect.right() + component::TOOLBAR_BTN_LABEL_GAP;
    ui.painter().text(
        egui::pos2(text_x, rect.center().y),
        Align2::LEFT_CENTER,
        b.label,
        font,
        fg,
    );
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
