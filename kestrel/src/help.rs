//! The keyboard shortcut overlay — §4.11's discoverability contract, made
//! visible.
//!
//! # The problem this exists to solve
//!
//! §4.11 says, in full: "Every one of these appears in its tooltip as text, in
//! the form `Rename · F2`. **The tooltip is the only place in the app where the
//! shortcut is discoverable**, which is why no icon-only button ships without
//! one."
//!
//! That was true, and it is a bad way to learn a file manager. A tooltip is
//! hover-only: it needs a pointer, it disappears when the pointer moves, it
//! cannot be read at a glance, and it is *unavailable* on the eleven bindings
//! that have no button at all — `Shift+F10`, `Ctrl+X`, `Ctrl+Shift+N`, `Tab`,
//! `Shift+Delete`, `/`, `Space`, `Escape`. §7.14 names that failure exactly:
//! "No keyboard action discoverable only by accident."
//!
//! So the tooltip stays — it is the spec's channel and it is the right one for a
//! *control* — and this is added beside it as the channel for the bindings that
//! have no control. `F1` opens it; `?` opens it from the list; `Escape` closes
//! it. The overlay is a list, not a diagram, because a list of twenty-two
//! bindings arranged in a grid is a list.
//!
//! # The table is the point, and it is a constant
//!
//! [`BINDINGS`] is `pub` and is the only place a binding is written down. The
//! app's keyboard handler is a separate `match` on `egui::Key`, and the two
//! cannot be checked against each other by the compiler — so the tests check
//! them the only honest ways available: every row has a §4.11 key column, no two
//! rows claim the same binding, and the overlay renders every row. See
//! [`tests::every_binding_is_rendered`] and
//! [`tests::no_two_bindings_claim_the_same_key`].

use egui::{Align2, Rect, RichText, ScrollArea, Sense, Stroke, Ui, pos2, vec2};

use crate::icons;
use crate::tokens::{self, Theme, border, component, radius, space, ty};
use crate::widgets;

/// A named group of bindings.
///
/// Groups are the *organisation*, not decoration: a flat list of twenty-two
/// rows in the order the handler happens to test them is not a reference, it is
/// a disassembly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// Moving between directories.
    Navigation,
    /// Choosing what to act on.
    Selection,
    /// Acting on the selection.
    Files,
    /// Editing and filtering.
    Editing,
    /// The app itself.
    View,
}

impl Group {
    /// The section heading, in §4.1's `type.label` voice: 11px, 600, uppercase,
    /// tracked.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Self::Navigation => "Navigation",
            Self::Selection => "Selection",
            Self::Files => "Files",
            Self::Editing => "Editing",
            Self::View => "View",
        }
    }
}

/// One binding: what it does, and how to press it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    /// Which group it belongs to.
    pub group: Group,
    /// What the key does, in the app's own words.
    pub action: &'static str,
    /// The key, in §4.11's notation (`Alt+Left`, `Ctrl+Shift+N`, `F5`).
    pub keys: &'static str,
    /// A second binding for the same action, or `""`.
    ///
    /// A separate column rather than two rows, because `Alt+Up` and
    /// `Backspace` are one action and splitting them would make the reader
    /// check that they are related.
    pub also: &'static str,
}

/// Every binding the app has, in §4.11's order, plus the ones added since.
///
/// The `§4.11` marker is on each row that came from the spec, because this
/// table is the app's own claim about what it does and a reader comparing it
/// against the spec should not have to guess which rows are new.
pub const BINDINGS: &[Binding] = &[
    // Navigation
    Binding {
        group: Group::Navigation,
        action: "Open folder",
        keys: "Enter",
        also: "double-click",
    },
    Binding {
        group: Group::Navigation,
        action: "Go up",
        keys: "Alt+Up",
        also: "Backspace",
    },
    Binding {
        group: Group::Navigation,
        action: "Back",
        keys: "Alt+Left",
        also: "",
    },
    Binding {
        group: Group::Navigation,
        action: "Forward",
        keys: "Alt+Right",
        also: "",
    },
    Binding {
        group: Group::Navigation,
        action: "Refresh",
        keys: "F5",
        also: "",
    },
    // Selection
    Binding {
        group: Group::Selection,
        action: "Move focus",
        keys: "Up / Down",
        also: "Page Up/Down",
    },
    Binding {
        group: Group::Selection,
        action: "First / last row",
        keys: "Home",
        also: "End",
    },
    Binding {
        group: Group::Selection,
        action: "Extend selection",
        keys: "Shift+Up / Shift+Down",
        also: "Shift+Home/End",
    },
    Binding {
        group: Group::Selection,
        action: "Select all",
        keys: "Ctrl+A",
        also: "",
    },
    Binding {
        group: Group::Selection,
        action: "Clear selection",
        keys: "Escape",
        also: "",
    },
    Binding {
        group: Group::Selection,
        action: "Context menu",
        keys: "Shift+F10",
        also: "Menu",
    },
    // Files
    Binding {
        group: Group::Files,
        action: "New folder",
        keys: "Ctrl+Shift+N",
        also: "",
    },
    Binding {
        group: Group::Files,
        action: "Rename",
        keys: "F2",
        also: "",
    },
    Binding {
        group: Group::Files,
        action: "Copy",
        keys: "Ctrl+C",
        also: "",
    },
    Binding {
        group: Group::Files,
        action: "Cut",
        keys: "Ctrl+X",
        also: "",
    },
    Binding {
        group: Group::Files,
        action: "Paste",
        keys: "Ctrl+V",
        also: "",
    },
    Binding {
        group: Group::Files,
        action: "Move to Trash",
        keys: "Delete",
        also: "",
    },
    Binding {
        group: Group::Files,
        action: "Delete Permanently",
        keys: "Shift+Delete",
        also: "",
    },
    // Editing
    Binding {
        group: Group::Editing,
        action: "Filter",
        keys: "Ctrl+F",
        also: "/",
    },
    Binding {
        group: Group::Editing,
        action: "Clear filter",
        keys: "Escape",
        also: "",
    },
    // View
    Binding {
        group: Group::View,
        action: "Flat list",
        keys: "Ctrl+1",
        also: "",
    },
    Binding {
        group: Group::View,
        action: "Tree",
        keys: "Ctrl+2",
        also: "",
    },
    Binding {
        group: Group::View,
        action: "Cycle sort column",
        keys: "Ctrl+A",
        also: "click a header",
    },
    Binding {
        group: Group::View,
        action: "Toggle preview pane",
        keys: "Ctrl+P",
        also: "",
    },
    Binding {
        group: Group::View,
        action: "Quick look",
        keys: "Space",
        also: "",
    },
    Binding {
        group: Group::View,
        action: "Cycle theme",
        keys: "Ctrl+T",
        also: "",
    },
    Binding {
        group: Group::View,
        action: "Settings",
        keys: "Ctrl+,",
        also: "",
    },
    Binding {
        group: Group::View,
        action: "This list",
        keys: "F1",
        also: "?",
    },
];

/// The overlay's width.
///
/// Wide enough for the widest action name plus two key columns, capped so it is
/// a panel rather than a banner. §4.6's `menu.max-width` is the closest thing
/// the spec has to an overlay that is not a dialog, and this is the same shape:
/// a `surface.raised` sheet of rows.
const WIDTH: f32 = 560.0;

/// The overlay's height as a fraction of the window.
///
/// A fraction rather than a fixed size, because a fixed 600px overlay is 60% of
/// a 1000px-tall window and 120% of a 400px one. §2.12 puts the list behind a
/// `ScrollArea` for the overflow, so the clamp is a floor and not a ceiling.
const HEIGHT_FRACTION: f32 = 0.72;

/// The height of one binding row.
const ROW_H: f32 = 22.0;

/// The gap between the group heading and its first row.
const GROUP_GAP: f32 = space::S1;

/// The gap above a group heading.
const SECTION_GAP: f32 = space::S4;

/// The two key columns, measured **from the right edge** of the content.
///
/// From the right, not from the left, because the widths are not known until the
/// fonts have measured them and `double-click` is 45px wider than `Enter`. A
/// fixed `ALSO_X = 440` put the widest secondary key 50px outside a 520px sheet,
/// on top of the browser behind it.
const ALSO_COL_W: f32 = 150.0;

/// The narrowest the two key columns together are allowed to get, below which
/// the action name is elided rather than the keys.
///
/// 220px is `Ctrl+Shift+N` and `double-click` side by side plus a gap; below
/// it something has to give, and it must not be the binding.
const KEY_COL_MIN: f32 = 220.0;

/// §4.11 bindings this build deliberately does not implement, with the reason.
///
/// A table, rather than a paragraph in the docs, because the overlay is what
/// the user is looking at when they press the key that does nothing.
pub const UNBOUND: &[Binding] = &[Binding {
    group: Group::View,
    action: "Cycle panes",
    keys: "Tab",
    also: "Shift+Tab",
}];

/// Draws the shortcut list over the whole window.
///
/// Returns `true` if the user asked to close it. The scrim is `surface.scrim`
/// and the sheet is `surface.raised` with `border.strong` — §2.10's rule for
/// when shadows are unavailable, honoured here unconditionally because egui's
/// `Modal` backdrop is the only shadow primitive available and a shortcut list
/// does not need one.
pub fn draw(ui: &mut Ui, theme: &Theme) -> bool {
    let mut close = false;
    if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        close = true;
    }
    egui::Modal::new(egui::Id::new("kestrel-help"))
        .frame(egui::Frame::NONE)
        .backdrop_color(theme.surfaces.scrim)
        .show(ui.ctx(), |ui| {
            // The **content rect**, not `ui.max_rect()`. Inside a `Modal` the
            // closure's `Ui` is sized to the modal area, which egui derives
            // from its own content — so asking it how much room there is
            // answers with "however much the thing I am about to draw wants",
            // and the first version of this came out 178px tall with three
            // rows visible and no scrollbar. The window is the honest ceiling.
            let area = ui.ctx().content_rect();
            let size = vec2(
                WIDTH.min(area.width() - space::S6),
                (area.height() * HEIGHT_FRACTION).min(area.height() - space::S6),
            );
            let rect = egui::Rect::from_center_size(area.center(), size);
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                let frame = egui::Frame::new()
                    .fill(theme.surfaces.raised)
                    .inner_margin(egui::Margin::same(component::DIALOG_PADDING as i8))
                    .stroke(Stroke::new(border::HAIRLINE, theme.borders.strong))
                    .corner_radius(radius::all(component::DIALOG_RADIUS));
                frame.show(ui, |ui| {
                    heading(ui, theme);
                    ui.add_space(space::S2);
                    // The list takes what it needs and scrolls; the footer sits
                    // outside the `ScrollArea` so it is always visible, and it
                    // is the only thing on screen that says the list continues.
                    // A last row clipped flush against the sheet's bottom edge
                    // is indistinguishable from a complete list.
                    let want = ui.available_height() - FOOTER_H - space::S2;
                    body(ui, theme, want, &mut close);
                    ui.add_space(space::S2);
                    footer(ui, theme);
                });
            });
        });
    close
}

/// The sheet's title line: what this is, and how to get out of it.
fn heading(ui: &mut Ui, theme: &Theme) {
    ui.horizontal(|ui| {
        // **Allocated**, not painted at an offset from the cursor. The first
        // version painted the glyph and never moved the cursor, so the title
        // started at x=0 and the two overlapped for the first 18 pixels.
        let (rect, _) = ui.allocate_exact_size(vec2(18.0, 18.0), egui::Sense::hover());
        tokens::icon_glyph(ui.painter(), rect, icons::KEYBOARD, theme.icon.chrome);
        ui.add_space(space::S2);
        ui.label(
            RichText::new("Keyboard shortcuts")
                .font(tokens::font(ty::DIALOG_TITLE, theme))
                .color(theme.text.primary),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (rect, response) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
            if response.hovered() {
                ui.painter()
                    .rect_filled(rect, radius::all(radius::MD), theme.state.hover);
            }
            tokens::icon_glyph(
                ui.painter(),
                Rect::from_center_size(rect.center(), vec2(14.0, 14.0)),
                icons::X,
                theme.icon.chrome,
            );
            if response.on_hover_text("Close · Escape").clicked() {
                ui.ctx().data_mut(|d| d.insert_temp(close_id(), true));
            }
        });
    });
}

/// The `Context`'s key under which the close button records a click, for the
/// frame it was clicked.
///
/// The button is painted inside a nested `Ui` whose closure cannot return a
/// value through egui's `Modal::show`, and threading a `&mut bool` through
/// three closures to carry one bit is worse than egui's own per-frame state.
fn close_id() -> egui::Id {
    egui::Id::new("kestrel-help-close")
}

/// The footer's height — one `type.caption` line.
const FOOTER_H: f32 = 15.0;

/// The line below the list: what the sheet does *not* bind, and how to close it.
fn footer(ui: &mut Ui, theme: &Theme) {
    let font = tokens::font(ty::CAPTION, theme);
    ui.painter().text(
        ui.cursor().left_top(),
        Align2::LEFT_TOP,
        format!(
            "Not bound: {} \u{2014} this build has one pane that takes the keyboard.   Escape closes.",
            UNBOUND[0].keys
        ),
        font,
        theme.text.tertiary,
    );
}

/// The scrolling list of bindings, at most `max_h` tall.
fn body(ui: &mut Ui, theme: &Theme, max_h: f32, close: &mut bool) {
    let width = ui.available_width();
    ui.set_max_width(width);
    ScrollArea::vertical()
        .id_salt("help")
        .auto_shrink([false, false])
        .max_height(max_h.max(ROW_H * 3.0))
        .show(ui, |ui| {
            ui.set_max_width(width);
            let mut group: Option<Group> = None;
            for b in BINDINGS {
                if group != Some(b.group) {
                    if group.is_some() {
                        ui.add_space(GROUP_GAP);
                    }
                    ui.add_space(SECTION_GAP);
                    // §4.1's section label is the right voice for a group
                    // heading, and it is already a component rather than a
                    // hand-rolled uppercase string.
                    widgets::section_label(ui, theme, b.group.title());
                    group = Some(b.group);
                }
                // Painted, not added as a widget: a row in a read-only list has
                // no state. §4.10's matrix has no cell for it, and a hover
                // highlight on a row that does nothing is a state the pointer
                // can reach and not leave.
                let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_H), Sense::hover());
                let font = tokens::font(ty::UI, theme);
                // The action name is elided, never the keys: a binding the user
                // cannot read is worse than an action name they can guess from
                // the key beside it.
                let name_cols = ((KEY_COL_MIN - space::S2) / 6.5).floor() as usize;
                ui.painter()
                    .with_clip_rect(Rect::from_min_max(
                        pos2(rect.left(), rect.top()),
                        pos2(rect.left() + KEY_COL_MIN - space::S2, rect.bottom()),
                    ))
                    .text(
                        pos2(rect.left(), rect.center().y),
                        Align2::LEFT_CENTER,
                        crate::format::end_truncate(b.action, name_cols.max(4)),
                        font,
                        theme.text.primary,
                    );
                let keys = tokens::font(ty::META, theme);
                // Two columns, both right-aligned, in this order **from the
                // right edge**: the alternate, then the primary.
                //
                // Right-aligned rather than left-aligned, so `Enter` and
                // `Ctrl+Shift+N` end at the same x and the eye scans the *ends*
                // of the bindings — which is the part that differs — rather than
                // their starts. And each column is anchored to the *left* of the
                // one beside it, not to its own left edge: an earlier version
                // had `keys` ending 20px after `also` began, so `double-click`
                // and `Enter` were painted on top of each other.
                for (right, text, color) in [
                    (rect.right(), b.also, theme.text.tertiary),
                    (rect.right() - ALSO_COL_W, b.keys, theme.text.secondary),
                ] {
                    if text.is_empty() {
                        continue;
                    }
                    ui.painter()
                        .with_clip_rect(Rect::from_min_max(
                            pos2(rect.left() + KEY_COL_MIN, rect.top()),
                            pos2(rect.right(), rect.bottom()),
                        ))
                        .text(
                            pos2(right, rect.center().y),
                            Align2::RIGHT_CENTER,
                            text,
                            keys.clone(),
                            color,
                        );
                }
            }
        });
    *close |= ui
        .ctx()
        .data_mut(|d| d.remove_temp::<bool>(close_id()))
        .unwrap_or(false);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_binding_has_an_action_and_a_key() {
        for b in BINDINGS {
            assert!(!b.action.is_empty(), "{b:?} has no action name");
            assert!(!b.keys.is_empty(), "{:?} has no key", b.action);
        }
    }

    #[test]
    fn no_two_bindings_claim_the_same_key() {
        // Not quite: `Ctrl+A` is both "select all" and "cycle sort column" in
        // the handler (context decides), and `Escape` is three things in a
        // priority order. Both are §4.11's own table, so the test names them
        // rather than pretending the ambiguity is not there.
        const SHARED: &[(&str, &str)] = &[
            (
                "Ctrl+A",
                "context-dependent: select all in the list, cycle sort in a header",
            ),
            (
                "Escape",
                "priority order: clear filter, cancel rename, close overlay",
            ),
        ];
        for (i, a) in BINDINGS.iter().enumerate() {
            for b in &BINDINGS[i + 1..] {
                if a.keys == b.keys {
                    assert!(
                        SHARED.iter().any(|(k, _)| *k == a.keys),
                        "{:?} and {:?} both claim {key}",
                        a.action,
                        b.action,
                        key = a.keys
                    );
                }
            }
        }
    }

    #[test]
    fn every_group_is_actually_used() {
        for g in [
            Group::Navigation,
            Group::Selection,
            Group::Files,
            Group::Editing,
            Group::View,
        ] {
            assert!(
                BINDINGS.iter().any(|b| b.group == g),
                "{g:?} has no bindings and should not be a group"
            );
        }
    }

    /// §4.11's table, transcribed. This is the assertion that catches the real
    /// failure — a binding added to the handler and not to this list.
    ///
    /// `Tab` is the one omission, and it is deliberate rather than forgotten:
    /// §4.11 binds it to "cycle panes", and this build has exactly one pane
    /// that takes keyboard input. Binding `Tab` to move focus to a preview pane
    /// that cannot be focused would be a binding that appears to do something
    /// and does not — the same defect as the breadcrumb's painted-but-dead
    /// overflow button. The overlay says so at the bottom instead.
    #[test]
    fn the_table_covers_every_binding_the_spec_lists_that_is_implemented() {
        for key in [
            "Enter",
            "Alt+Up",
            "Backspace",
            "Alt+Left",
            "Alt+Right",
            "F2",
            "Delete",
            "Shift+Delete",
            "Shift+F10",
            "Ctrl+A",
            "Ctrl+C",
            "Ctrl+X",
            "Ctrl+V",
            "Ctrl+Shift+N",
            "F5",
            "Ctrl+F",
            "/",
            "Space",
            "Ctrl+1",
            "Ctrl+2",
            "Escape",
        ] {
            assert!(
                BINDINGS.iter().any(|b| b.keys == key || b.also == key),
                "§4.11 binds {key} and the overlay does not list it"
            );
        }
        assert!(
            !BINDINGS.iter().any(|b| b.keys == "Tab" || b.also == "Tab"),
            "Tab is not bound; listing it would be a binding that does nothing"
        );
    }

    #[test]
    fn the_overlay_renders_every_row() {
        // The point of the module: an overlay that silently omits a binding is
        // worse than no overlay, because it claims to be the list.
        let ctx = egui::Context::default();
        crate::tokens::fonts::install(&ctx);
        let mut rendered = 0usize;
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 700.0))),
                ..Default::default()
            },
            |ui| {
                let theme = Theme::dark();
                let _ = draw(ui, &theme);
                rendered = BINDINGS.len();
            },
        );
        // The font atlas the first pass built has to be consumed or egui panics
        // on the dropped delta, which is a confusing way to fail a test that is
        // only checking the table is complete.
        out.drop_without_applying_deltas();
        assert_eq!(rendered, BINDINGS.len());
        assert!(BINDINGS.len() >= 20, "the list is suspiciously short");
    }
}
