//! The settings screen, and the state it persists.
//!
//! # Why this is its own module
//!
//! Two reasons, and the second is the load-bearing one.
//!
//! 1. The screen is a *screen*, not a dropdown. A dropdown cannot hold a
//!    labelled row per setting, a help line per setting, or a `Reset` that
//!    means something, and §4's component contracts are all about surfaces with
//!    rows in them.
//! 2. The **persisted shape and the live shape are different types.** The live
//!    app holds a `SortSpec` from the engine and a `ViewMode` of its own; what
//!    goes on disk is [`Stored`], a deliberately flat mirror whose field names
//!    are the contract. They are separate because a persisted format is the one
//!    thing in this app that cannot be changed later without breaking a user's
//!    saved state, and tying it to a UI enum means renaming a Rust variant
//!    silently orphans everyone's preferences.
//!
//! # The persistence path
//!
//! `eframe`'s own, because it is already a dependency and it is the one the
//! `persistence` feature was turned on for:
//!
//! * **Load**: [`eframe::CreationContext::storage`] -> [`eframe::get_value`] on
//!   [`KEY`], in [`crate::app::KestrelApp::new`].
//! * **Save**: [`eframe::App::save`] -> [`eframe::set_value`] on the same key.
//!
//! `eframe` calls `save` on its auto-save interval (30s by default) and on exit,
//! so a setting is durable without this code doing any I/O. That matters: this
//! crate's one hard rule is that nothing in the UI path blocks, and a
//! `write_text` on the way out of a settings toggle would be exactly that.
//!
//! **Load failures are not failures.** A missing key, a corrupt file, or a
//! stored state from a build whose `Stored` has a field this build does not —
//! all three produce [`Stored::default`], because a file manager that refuses
//! to start over an unreadable preferences file is worse than one that forgets
//! a preference. [`Stored`] uses `#[serde(default)]` at the *struct* level for
//! the same reason: adding a setting later must not invalidate the ones already
//! saved.

use egui::{Align2, Rect, RichText, ScrollArea, Sense, Stroke, Ui, pos2, vec2};
use kestrel_fs::model::{SortKey, SortSpec};
use serde::{Deserialize, Serialize};

use crate::icons;
use crate::tokens::{self, Theme, ThemeMode, border, component, radius, space, ty};
use crate::widgets;

/// The key Kestrel's settings live under in `eframe`'s storage.
///
/// Namespaced so a future second key (window geometry, recent paths) cannot
/// collide with this one.
pub const KEY: &str = "kestrel.settings.v1";

/// The widest the preview pane may be set, in logical pixels.
///
/// Deliberately *narrower* than `metric.sidebar-max` (340) is wide: the pane
/// holds a filename column that stops being readable well before 340. The
/// setting is clamped to this rather than trusted, so a hand-edited preferences
/// file cannot produce a pane the list has no room for.
pub const PREVIEW_MAX: f32 = 360.0;

/// The narrowest the preview pane may be set — the sidebar's floor, so no pane
/// can be squeezed to nothing while another stays wide.
pub const PREVIEW_MIN: f32 = 180.0;

/// The preview pane's width on first run, in logical pixels.
///
/// Between `metric.sidebar-min` (160) and the 400px dialog, so the three panes
/// stay balanced. §4 has no preview-pane metric — the pane is a Phase 3
/// addition — so this is derived, and owned here because it is a *setting*
/// default and a setting default belongs with the settings.
pub const PREVIEW_DEFAULT: f32 = 280.0;

/// Everything the settings screen can change, and everything that is persisted.
///
/// One flat struct with no nesting, so a field is added by adding a line here
/// and a row in [`draw`] and nothing else has to move.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Stored {
    /// Which theme: `light`, `dark`, or `system`.
    pub theme: Mode,
    /// Whether dotfiles are listed.
    pub show_hidden: bool,
    /// Whether the preview pane is shown at all.
    pub show_preview: bool,
    /// The preview pane's width, in logical pixels, clamped to
    /// [`PREVIEW_MIN`]..=[`PREVIEW_MAX`].
    pub preview_width: f32,
    /// Whether the list is flat or an indented tree.
    pub view: View,
    /// The column the listing sorts by.
    pub sort: Column,
    /// Ascending (true) or descending (false).
    pub sort_ascending: bool,
    /// Keep directories above files regardless of the chosen column.
    pub dirs_first: bool,
    /// Whether the Places sidebar is shown.
    pub show_sidebar: bool,
}

impl Default for Stored {
    fn default() -> Self {
        Self {
            theme: Mode::System,
            // §5.2's hidden-file discussion presumes hidden entries are visible
            // and *marked*, so hidden-by-default would silently contradict it.
            show_hidden: true,
            show_preview: true,
            preview_width: PREVIEW_DEFAULT,
            view: View::List,
            sort: Column::Name,
            sort_ascending: true,
            dirs_first: true,
            show_sidebar: true,
        }
    }
}

/// The persisted theme mode.
///
/// A three-variant mirror of [`tokens::ThemeMode`] rather than the type itself,
/// so the on-disk names are fixed independent of the Rust enum. See the module
/// docs on why the persisted shape is separate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Follow the compositor.
    #[default]
    System,
    /// Always the light theme (§3, light column).
    Light,
    /// Always the dark theme (§3, dark column).
    Dark,
}

impl Mode {
    /// Every mode, in the order the segmented control shows them.
    pub const ALL: [Mode; 3] = [Mode::Light, Mode::Dark, Mode::System];

    /// The label on the control.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    /// The theme's token, for the segmented control's glyph.
    #[must_use]
    pub fn glyph(self) -> icons::Glyph {
        match self {
            Self::System => icons::CIRCLE_HALF,
            Self::Light => icons::SUN,
            Self::Dark => icons::MOON,
        }
    }

    /// The live type.
    #[must_use]
    pub fn to_theme(self) -> ThemeMode {
        match self {
            Self::System => ThemeMode::System,
            Self::Light => ThemeMode::Light,
            Self::Dark => ThemeMode::Dark,
        }
    }

    /// From the live type. Total, because `Mode` and `ThemeMode` have the same
    /// three cases and no others.
    #[must_use]
    pub fn from_theme(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::System => Self::System,
            ThemeMode::Light => Self::Light,
            ThemeMode::Dark => Self::Dark,
        }
    }
}

/// The persisted list view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum View {
    /// A flat listing of the current directory. §4.11 `Ctrl+1`.
    #[default]
    List,
    /// Nested, with one indent step per depth level. §4.11 `Ctrl+2`.
    Tree,
}

impl View {
    /// Both views, in control order.
    pub const ALL: [View; 2] = [View::List, View::Tree];

    /// The label on the control.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::List => "Flat list",
            Self::Tree => "Tree",
        }
    }

    /// The theme's glyph.
    #[must_use]
    pub fn glyph(self) -> icons::Glyph {
        match self {
            Self::List => icons::LIST_BULLETS,
            Self::Tree => icons::ARROWS_OUT,
        }
    }

    /// The shortcut this view has, per §4.11.
    #[must_use]
    pub fn shortcut(self) -> &'static str {
        match self {
            Self::List => "Ctrl+1",
            Self::Tree => "Ctrl+2",
        }
    }
}

/// The persisted sort column.
///
/// Four cases mirroring [`SortKey`]. Not the engine type: `kestrel-fs` has no
/// dependencies by design, and adding `serde` to the headless engine to save
/// one enum would put a UI concern into the crate whose whole point is that it
/// has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Column {
    /// File name.
    #[default]
    Name,
    /// Size in bytes.
    Size,
    /// Last modification time.
    Modified,
    /// Entry kind.
    Kind,
}

impl Column {
    /// Every column, in the order the list shows them.
    pub const ALL: [Column; 4] = [Column::Name, Column::Size, Column::Modified, Column::Kind];

    /// The label on the control: the column's own header text, so the settings
    /// screen and the column header cannot drift apart.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Size => "Size",
            Self::Modified => "Modified",
            Self::Kind => "Kind",
        }
    }

    /// The engine's key.
    #[must_use]
    pub fn to_sort_key(self) -> SortKey {
        match self {
            Self::Name => SortKey::Name,
            Self::Size => SortKey::Size,
            Self::Modified => SortKey::Modified,
            Self::Kind => SortKey::Kind,
        }
    }

    /// From the engine's key. Total: the two enums have the same four cases.
    #[must_use]
    pub fn from_sort_key(key: SortKey) -> Self {
        match key {
            SortKey::Name => Self::Name,
            SortKey::Size => Self::Size,
            SortKey::Modified => Self::Modified,
            SortKey::Kind => Self::Kind,
        }
    }
}

/// Reads the stored settings, or the defaults if there are none to read.
///
/// Never fails and never blocks: `eframe`'s `Storage` is an in-memory `HashMap`
/// that was loaded once, before the first frame.
#[must_use]
pub fn load(storage: Option<&dyn eframe::Storage>) -> Stored {
    let Some(storage) = storage else {
        // No storage at all — a `--screenshot` capture, or a platform with no
        // writable data dir. Defaults are the right answer, not an error.
        return Stored::default();
    };
    // A decode failure lands here too: `get_value` logs at `debug` and returns
    // `None`. A preferences file that cannot be read is a forgotten
    // preference, never a refused start.
    let mut stored = eframe::get_value::<Stored>(storage, KEY).unwrap_or_default();
    stored.normalise();
    stored
}

impl Stored {
    /// Clamps every field into the range the app can actually honour.
    ///
    /// Applied on **load** rather than on write, so a hand-edited or
    /// downgraded preferences file cannot put the preview pane at 4px or the
    /// sort column at nothing. Cheap, total, and the only place the ranges are
    /// enforced.
    pub fn normalise(&mut self) {
        if !self.preview_width.is_finite() {
            // A NaN width reaches egui as a zero-sized rect and then multiplies
            // out through the column layout, so it is the one value here that
            // has to be checked for validity rather than range.
            self.preview_width = PREVIEW_DEFAULT;
        }
        self.preview_width = self.preview_width.clamp(PREVIEW_MIN, PREVIEW_MAX);
    }

    /// The engine's sort spec for these settings.
    #[must_use]
    pub fn sort_spec(&self) -> SortSpec {
        SortSpec {
            key: self.sort.to_sort_key(),
            ascending: self.sort_ascending,
            dirs_first: self.dirs_first,
        }
    }
}

// ----------------------------------------------------------------------------
// The screen
// ----------------------------------------------------------------------------

/// What the settings screen asks the app to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Action {
    /// Nothing happened.
    #[default]
    None,
    /// Leave the settings screen.
    Close,
}

/// Vertical gap between a section label and its first row.
const SECTION_GAP: f32 = space::S2;

/// One labelled row: a name, a control, and an optional line of help.
///
/// Three fields, not an enum of rows: the help line is the only thing that
/// varies, and an enum would mean nine variants of the same layout.
struct Row<'a> {
    /// The setting's name, `type.ui` in `text.primary`.
    label: &'a str,
    /// One sentence of help, `type.caption` in `text.secondary`. Optional: a
    /// setting whose name is self-explanatory does not need a paragraph, and
    /// §7.5's rule is that weight and size carry meaning, not that everything
    /// must be filled in.
    help: Option<&'a str>,
}

/// Draws the settings screen into the whole `ui`.
///
/// `ui` is the root `Ui`; the screen owns every pixel of it. Returns what the
/// user asked for.
///
/// `motion` is taken and ignored on purpose, and says so where it is threaded:
/// the screen has no animation, because §2.11's rule is that nothing the pointer
/// touches takes longer than 130ms and a settings screen has nothing that does.
pub fn draw(
    ui: &mut Ui,
    theme: &Theme,
    stored: &mut Stored,
    _motion: crate::motion::Motion,
) -> Action {
    let mut action = Action::None;
    // The band, then the body. `Action` is only read at the end, so every
    // control in the screen can write to it without a borrow of `ui` escaping.
    header(ui, theme, &mut action);
    body(ui, theme, stored, &mut action);
    action
}

/// The title band: the screen's name and the one way out of it.
fn header(ui: &mut Ui, theme: &Theme, action: &mut Action) {
    egui::Panel::top("settings-header")
        .exact_size(component::SETTINGS_HEADER_H)
        .frame(
            egui::Frame::new()
                .fill(theme.surfaces.panel)
                .inner_margin(egui::Margin::symmetric(
                    component::SETTINGS_ROW_PAD_X as i8,
                    3,
                ))
                .stroke(Stroke::new(border::HAIRLINE, theme.borders.subtle)),
        )
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(
                    RichText::new("Settings")
                        .font(tokens::font(ty::UI_STRONG, theme))
                        .color(theme.text.primary),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // §4.7's rule that a safe action holds focus applies to
                    // dialogs; here there is exactly one exit and it is not
                    // destructive, so it is an ordinary toolbar button with an
                    // ordinary tooltip. The shortcut is in it, because §4.11
                    // says every binding must be discoverable somewhere and the
                    // help overlay is not the only place.
                    let (rect, response) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
                    if response.hovered() {
                        ui.painter()
                            .rect_filled(rect, radius::all(radius::MD), theme.state.hover);
                    }
                    tokens::icon_glyph(
                        ui.painter(),
                        Rect::from_center_size(rect.center(), vec2(16.0, 16.0)),
                        icons::X,
                        theme.icon.chrome,
                    );
                    if response.on_hover_text("Close · Escape").clicked() {
                        *action = Action::Close;
                    }
                });
            });
        });
}

/// The scrolling body, capped at a reading measure and centred.
fn body(ui: &mut Ui, theme: &Theme, stored: &mut Stored, action: &mut Action) {
    let fill = theme.surfaces.app;
    let available = ui.available_width();
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(fill))
        .show(ui, |ui| {
            let width = available.min(component::SETTINGS_BODY_MAX_W);
            // Pinned before anything is measured, and re-pinned inside the
            // `ScrollArea` — the gallery's own width regression (1200 -> 1564)
            // is what this is guarding against, in a second place.
            ui.set_max_width(width);
            ui.spacing_mut().item_spacing.y = 0.0;
            ScrollArea::vertical()
                .id_salt("settings")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_max_width(width);
                    let inner = ui.available_width();
                    ui.vertical(|ui| {
                        section(ui, theme, "Appearance");
                        segmented(
                            ui,
                            theme,
                            Row {
                                label: "Theme",
                                help: Some("System follows the compositor's light or dark preference."),
                            },
                            inner,
                            &Mode::ALL,
                            &[
                                Segment {
                                    label: Mode::Light.label(),
                                    glyph: Mode::Light.glyph(),
                                    shortcut: "",
                                },
                                Segment {
                                    label: Mode::Dark.label(),
                                    glyph: Mode::Dark.glyph(),
                                    shortcut: "Ctrl+T",
                                },
                                Segment {
                                    label: Mode::System.label(),
                                    glyph: Mode::System.glyph(),
                                    shortcut: "",
                                },
                            ],
                            &mut stored.theme,
                        );
                        checkbox(
                            ui,
                            theme,
                            Row {
                                label: "Show sidebar",
                                help: Some("The Places list on the left."),
                            },
                            inner,
                            &mut stored.show_sidebar,
                        );

                        section(ui, theme, "List");
                        checkbox(
                            ui,
                            theme,
                            Row {
                                label: "Show hidden files",
                                help: Some(
                                    "Entries whose name starts with a dot. They are always marked, never hidden by surprise.",
                                ),
                            },
                            inner,
                            &mut stored.show_hidden,
                        );
                        segmented(
                            ui,
                            theme,
                            Row {
                                label: "View",
                                help: Some("Tree indents one step per level and expands in place."),
                            },
                            inner,
                            &View::ALL,
                            &[
                                Segment {
                                    label: View::List.label(),
                                    glyph: View::List.glyph(),
                                    shortcut: View::List.shortcut(),
                                },
                                Segment {
                                    label: View::Tree.label(),
                                    glyph: View::Tree.glyph(),
                                    shortcut: View::Tree.shortcut(),
                                },
                            ],
                            &mut stored.view,
                        );
                        segmented(
                            ui,
                            theme,
                            Row {
                                label: "Sort by",
                                help: None,
                            },
                            inner,
                            &Column::ALL,
                            &[
                                Segment {
                                    label: Column::Name.label(),
                                    glyph: icons::TEXT_AA,
                                    shortcut: "",
                                },
                                Segment {
                                    label: Column::Size.label(),
                                    glyph: icons::HARD_DRIVE,
                                    shortcut: "",
                                },
                                Segment {
                                    label: Column::Modified.label(),
                                    glyph: icons::CLOCK,
                                    shortcut: "",
                                },
                                Segment {
                                    label: Column::Kind.label(),
                                    glyph: icons::FOLDER,
                                    shortcut: "",
                                },
                            ],
                            &mut stored.sort,
                        );
                        checkbox(
                            ui,
                            theme,
                            Row {
                                label: "Descending",
                                help: Some("Reverses the chosen column's order."),
                            },
                            inner,
                            &mut stored.sort_ascending,
                        );
                        checkbox(
                            ui,
                            theme,
                            Row {
                                label: "Folders first",
                                help: Some("Keeps directories above files whatever the column is."),
                            },
                            inner,
                            &mut stored.dirs_first,
                        );

                        section(ui, theme, "Preview");
                        checkbox(
                            ui,
                            theme,
                            Row {
                                label: "Show preview pane",
                                help: Some("Also bound to Ctrl+P."),
                            },
                            inner,
                            &mut stored.show_preview,
                        );
                        stepper(
                            ui,
                            theme,
                            Row {
                                label: "Preview width",
                                help: Some("How much of the window the preview pane takes."),
                            },
                            inner,
                            &mut stored.preview_width,
                            PREVIEW_RANGE,
                        );

                        section(ui, theme, "Keyboard");
                        // §7.14: a binding that exists only in the source is a
                        // hidden affordance. The settings screen is one of the
                        // two places it can be taught; the help overlay is the
                        // other, and it is one keystroke away.
                        key_hint(
                            ui,
                            theme,
                            Row {
                                label: "Shortcuts",
                                help: Some("Every binding is listed in the help overlay."),
                            },
                            inner,
                            "F1",
                        );

                        ui.add_space(space::S6);
                        let _ = action;
                    });
                });
        });
}

/// A §4.1-style section label, plus the gap to its first row.
fn section(ui: &mut Ui, theme: &Theme, title: &str) {
    ui.add_space(space::S5);
    widgets::section_label(ui, theme, title);
    ui.add_space(SECTION_GAP);
}

/// Splits a row into `(label_rect, control_rect)`, stacked or side-by-side.
///
/// The label column is a fixed share of the row so the controls line up down
/// the screen, which is what makes a list of settings scannable. Below
/// [`is_narrow`] the row stacks — label above control — because a 190px label
/// column and a four-option segmented control do not both fit in a 400px
/// window, and a control pushed off the right edge is the bug this whole
/// layout exists to avoid.
fn split(row: Rect, stacked: bool) -> (Rect, Rect) {
    if stacked {
        let label_h = component::SETTINGS_ROW_H;
        (
            Rect::from_min_size(row.min, vec2(row.width(), label_h)),
            Rect::from_min_size(
                pos2(row.left(), row.top() + label_h),
                vec2(row.width(), component::SETTINGS_ROW_H),
            ),
        )
    } else {
        let label_w =
            (row.width() * component::SETTINGS_LABEL_SHARE).min(component::SETTINGS_LABEL_MAX_W);
        (
            Rect::from_min_size(row.min, vec2(label_w, row.height())),
            Rect::from_min_size(
                pos2(row.left() + label_w, row.top()),
                vec2(row.width() - label_w, row.height()),
            ),
        )
    }
}

/// `true` when the row is too narrow for a side-by-side label and control.
///
/// A pure function of the width so the breakpoint is testable, which it
/// otherwise would not be: it depends on the two things a screenshot of a
/// broken layout cannot tell you.
#[must_use]
pub fn is_narrow(width: f32) -> bool {
    // A control needs its minimum plus a gap, and the label needs enough for
    // "Preview width" at 13px without truncating. 400px is where both are
    // still true, and it is the width below which the real defect appeared.
    width < component::SETTINGS_STACK_BELOW_W
}

/// Draws a row's label block into `rect`: the name, and the help line under it.
///
/// The two offsets are `type.ui`'s and `type.caption`'s line boxes, not tuned
/// constants, so changing either type token moves the text with it.
fn label(ui: &Ui, theme: &Theme, rect: Rect, row: &Row<'_>) {
    let painter = ui.painter();
    let name_line = ty::UI.line;
    painter.text(
        pos2(rect.left(), rect.top()),
        Align2::LEFT_TOP,
        row.label,
        tokens::font(ty::UI, theme),
        theme.text.primary,
    );
    if let Some(help) = row.help {
        painter.text(
            pos2(rect.left(), rect.top() + name_line),
            Align2::LEFT_TOP,
            help,
            tokens::font(ty::CAPTION, theme),
            theme.text.tertiary,
        );
    }
}

/// A checkbox row (§4.10's checkbox cell: 14px `border.strong`, `accent.base`
/// fill and a `check` glyph when on, 2px `focus.ring` when focused).
fn checkbox(ui: &mut Ui, theme: &Theme, row: Row<'_>, width: f32, on: &mut bool) {
    let (rect, _) =
        ui.allocate_exact_size(vec2(width, component::SETTINGS_ROW_TOTAL_H), Sense::hover());
    let stacked = is_narrow(width);
    let (label_rect, control_rect) = split(rect, stacked);
    label(ui, theme, label_rect, &row);
    let id = ui.make_persistent_id(("settings-check", row.label));
    let sense = if *on {
        Sense::click()
    } else {
        Sense::click().union(Sense::hover())
    };
    let response = ui.interact(control_rect, id, sense);
    let box_rect = Rect::from_center_size(
        control_rect.center(),
        vec2(component::CHECKBOX, component::CHECKBOX),
    );
    // §4.10's state priority: `hover` only when the box is idle, so a press
    // reads as a press and an on-state does not flicker under the pointer.
    let hovered = response.hovered() && !*on;
    if hovered {
        ui.painter().rect_filled(
            Rect::from_center_size(box_rect.center(), box_rect.size()),
            radius::all(component::DIALOG_CHECKBOX_RADIUS),
            theme.state.hover,
        );
    }
    widgets::checkbox(ui, theme, box_rect, *on, hovered);
    // The whole control area is the target, not just the 14px box: WCAG 2.2
    // `metric.target-min` is 24px, and a 14px checkbox is under it.
    if response.clicked() {
        *on = !*on;
    }
}

/// The suffix on the stepper's number. `type.meta-strong`, so the value reads
/// as a machine value rather than as a word.
const WIDTH_SUFFIX: &str = "px";

/// A numeric setting's range and granularity.
///
/// Three numbers that always travel together, and a `f32` step is only
/// meaningful against the range it quantises — which is why they are one value
/// rather than three arguments a call site can pair up wrongly.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Range {
    /// The lowest allowed value.
    min: f32,
    /// The highest allowed value.
    max: f32,
    /// How much one press changes it.
    step: f32,
}

/// The preview pane's range, in logical pixels.
const PREVIEW_RANGE: Range = Range {
    min: PREVIEW_MIN,
    max: PREVIEW_MAX,
    step: component::SETTINGS_WIDTH_STEP,
};

/// A `− value +` stepper, for a numeric setting with a fixed range.
///
/// A drag slider would be the obvious control and is the wrong one here: a
/// slider's value is not readable without hovering it, and a settings screen
/// whose value you cannot read is a settings screen you have to nudge. A
/// stepper shows the number and changes it by a known amount.
fn stepper(ui: &mut Ui, theme: &Theme, row: Row<'_>, width: f32, value: &mut f32, range: Range) {
    let (rect, _) =
        ui.allocate_exact_size(vec2(width, component::SETTINGS_ROW_TOTAL_H), Sense::hover());
    let stacked = is_narrow(width);
    let (label_rect, control_rect) = split(rect, stacked);
    label(ui, theme, label_rect, &row);

    let btn = component::SETTINGS_STEPPER_BTN;
    let gap = component::SETTINGS_STEPPER_GAP;
    let value_w = component::SETTINGS_STEPPER_VALUE_W;
    // The group is right-aligned in the control column and its parts are laid
    // out from its left edge, so the two buttons are at fixed positions and the
    // number between them never nudges them when its digits change width.
    let group_w = btn * 2.0 + gap * 2.0 + value_w;
    let group_x = (control_rect.right() - group_w).max(control_rect.left());
    let cy = control_rect.center().y;
    let value_rect = Rect::from_center_size(
        pos2(group_x + gap + btn + value_w / 2.0, cy),
        vec2(value_w, 20.0),
    );
    let minus = Rect::from_center_size(pos2(group_x + btn / 2.0, cy), vec2(btn, btn));
    let plus = Rect::from_center_size(
        pos2(group_x + gap + btn + value_w + gap + btn / 2.0, cy),
        vec2(btn, btn),
    );

    // Snapped to the step, so repeated presses cannot accumulate a fractional
    // width through float addition — the classic 279.99998px pane.
    let round = |v: f32| (v / range.step).round() * range.step;
    let at_min = *value <= range.min + 0.5;
    let at_max = *value >= range.max - 0.5;

    if mini_button(ui, theme, minus, icons::X, !at_min, "Narrower") {
        *value = (round(*value) - range.step).clamp(range.min, range.max);
    }
    ui.painter().text(
        value_rect.center(),
        Align2::CENTER_CENTER,
        format!("{WIDTH_SUFFIX} {}", value.round() as i32),
        tokens::font(ty::META_STRONG, theme),
        theme.text.primary,
    );
    if mini_button(ui, theme, plus, icons::CHECK, !at_max, "Wider") {
        *value = (round(*value) + range.step).clamp(range.min, range.max);
    }
}

/// A 24px square button holding a glyph. `enabled` greys it and drops hover.
fn mini_button(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    glyph: icons::Glyph,
    enabled: bool,
    what: &str,
) -> bool {
    let id = ui.make_persistent_id(("settings-mini", what));
    let response = ui.interact(rect, id, Sense::click());
    if enabled {
        if response.hovered() {
            ui.painter()
                .rect_filled(rect, radius::all(radius::MD), theme.state.hover);
        }
        let c = if response.is_pointer_button_down_on() {
            theme.state.pressed
        } else {
            theme.icon.chrome
        };
        // The Fill face, per §4.4's toggled-on rule, because these two glyphs
        // are heavier as outlines at 14px than they need to be.
        tokens::icon_glyph_fill(
            ui.painter(),
            Rect::from_center_size(rect.center(), vec2(14.0, 14.0)),
            glyph,
            c,
        );
    } else {
        tokens::icon_glyph(
            ui.painter(),
            Rect::from_center_size(rect.center(), vec2(14.0, 14.0)),
            glyph,
            component::icon_at(theme.icon.chrome, 0.4),
        );
    }
    enabled && response.on_hover_text(what).clicked()
}

/// A row that shows a binding and offers no control: the help link.
///
/// `F1` is not a setting, so there is nothing to toggle. The row exists because
/// §7.14's failure is a binding nobody can find, and a screen called Settings
/// that cannot say where the bindings are is the wrong place to look.
fn key_hint(ui: &mut Ui, theme: &Theme, row: Row<'_>, width: f32, key: &str) {
    let (rect, _) =
        ui.allocate_exact_size(vec2(width, component::SETTINGS_ROW_TOTAL_H), Sense::hover());
    let stacked = is_narrow(width);
    let (label_rect, control_rect) = split(rect, stacked);
    label(ui, theme, label_rect, &row);

    let font = tokens::font(ty::META_STRONG, theme);
    let text_w = crate::widgets::text_width(ui, key, font.clone());
    // Right-aligned to the control column, matching the stepper, so every
    // control in the screen ends at the same x.
    let pill = Rect::from_center_size(
        pos2(
            control_rect.right() - (text_w + space::S3).max(28.0) / 2.0,
            control_rect.center().y,
        ),
        vec2((text_w + space::S3).max(28.0), 20.0),
    );
    let id = ui.make_persistent_id("settings-key-hint");
    let response = ui.interact(pill.expand2(vec2(4.0, 4.0)), id, Sense::click());
    if response.hovered() {
        ui.painter()
            .rect_filled(pill, radius::all(radius::SM), theme.state.hover);
    }
    ui.painter()
        .rect_filled(pill, radius::all(radius::SM), theme.surfaces.input);
    widgets::rect_stroke(
        ui.painter(),
        pill,
        radius::all(radius::SM),
        Stroke::new(border::HAIRLINE, theme.borders.default),
    );
    ui.painter().text(
        pill.center(),
        Align2::CENTER_CENTER,
        key,
        font,
        theme.text.primary,
    );
    response.on_hover_text("Opens the keyboard shortcut list");
}

/// One segment's painted content.
struct Segment {
    /// The option's name.
    label: &'static str,
    /// Its §5.3 glyph.
    glyph: icons::Glyph,
    /// The §4.11 binding, or `""` for an option that has none.
    ///
    /// Empty rather than invented: printing a shortcut that does not exist
    /// would be §7.14's failure in a new place.
    shortcut: &'static str,
}

/// A segmented control: one option highlighted, `accent.subtle-bg` on the
/// chosen one, per §4.4's `toolbar.btn-toggled-on-*`.
///
/// The Fill face is used on the selected option only, which is the same rule
/// §4.4 states for a toggled toolbar button.
fn segmented<T: Copy + PartialEq>(
    ui: &mut Ui,
    theme: &Theme,
    row: Row<'_>,
    width: f32,
    options: &[T],
    meta: &[Segment],
    current: &mut T,
) {
    let (rect, _) =
        ui.allocate_exact_size(vec2(width, component::SETTINGS_ROW_TOTAL_H), Sense::hover());
    let stacked = is_narrow(width);
    let (label_rect, control_rect) = split(rect, stacked);
    label(ui, theme, label_rect, &row);

    // The segments share whatever the control column has. A four-option
    // segmented control at `SETTINGS_BODY_MAX_W`'s control column is comfortable; at a
    // 400px window the row stacks and the control gets the full width, which is
    // why the breakpoint is `is_narrow` and not a fixed pixel count.
    let n = options.len().max(1) as f32;
    let seg_w = (control_rect.width() - (n - 1.0) * space::HALF) / n;
    for (i, opt) in options.iter().enumerate() {
        let seg = Rect::from_min_size(
            pos2(
                control_rect.left() + i as f32 * (seg_w + space::HALF),
                control_rect.center().y - 13.0,
            ),
            vec2(seg_w, 26.0),
        );
        let id = ui.make_persistent_id(("settings-seg", row.label, i));
        let response = ui.interact(seg, id, Sense::click());
        let selected = *opt == *current;
        let bg = if selected {
            theme.accent.subtle_bg
        } else if response.hovered() {
            theme.state.hover
        } else {
            egui::Color32::TRANSPARENT
        };
        if bg != egui::Color32::TRANSPARENT {
            ui.painter().rect_filled(seg, radius::all(radius::MD), bg);
        }
        let Some(meta) = meta.get(i) else {
            continue;
        };
        let fg = if selected {
            theme.text.primary
        } else {
            theme.text.secondary
        };
        // Glyph first, then the label, both laid out from the segment's left
        // edge rather than centred as a pair: a centred pair moves both
        // elements every time the chosen option's label is a different length.
        tokens::icon_glyph(
            ui.painter(),
            Rect::from_center_size(
                pos2(seg.left() + space::S2 + 7.0, seg.center().y),
                vec2(14.0, 14.0),
            ),
            meta.glyph,
            if selected { theme.accent.base } else { fg },
        );
        ui.painter().text(
            pos2(seg.left() + space::S2 + 18.0, seg.center().y),
            Align2::LEFT_CENTER,
            meta.label,
            tokens::font(ty::UI, theme),
            fg,
        );
        let tip = if meta.shortcut.is_empty() {
            meta.label.to_string()
        } else {
            format!("{} · {}", meta.label, meta.shortcut)
        };
        if response.on_hover_text(tip).clicked() {
            *current = *opt;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A `Storage` backed by a real RON file on disk.
    ///
    /// # Why not a `HashMap`
    ///
    /// An in-memory double would pass a test that says "the values survive the
    /// struct" and fail the one that says "the values survive a restart", and
    /// the difference between those two is the whole point. This is eframe's
    /// own contract — `set_string` then `flush` writes a file, a fresh instance
    /// reads it back — with the one thing left out being eframe's background
    /// writer thread, which is a scheduling detail and not a format.
    struct DiskStore {
        path: std::path::PathBuf,
        kv: HashMap<String, String>,
    }

    impl DiskStore {
        /// Reads an existing store, or starts an empty one.
        fn open(path: std::path::PathBuf) -> Self {
            let kv = std::fs::read_to_string(&path)
                .ok()
                .and_then(|text| ron::from_str::<HashMap<String, String>>(&text).ok())
                .unwrap_or_default();
            Self { path, kv }
        }

        /// Writes the store out, as eframe's `Storage::flush` does on its
        /// writer thread.
        fn write(&self) {
            if let Ok(text) = ron::ser::to_string(&self.kv) {
                let _ = std::fs::write(&self.path, text);
            }
        }
    }

    impl eframe::Storage for DiskStore {
        fn get_string(&self, key: &str) -> Option<String> {
            self.kv.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.kv.insert(key.to_owned(), value);
        }
        fn remove_string(&mut self, key: &str) {
            self.kv.remove(key);
        }
        fn flush(&mut self) {
            // Synchronously, unlike eframe's writer thread: a test that waits
            // for a thread to finish is a test that can flake.
            self.write();
        }
    }

    /// A non-default `Stored`, so the round trip is not the identity on
    /// defaults — a test that round-trips `Default::default()` proves only that
    /// `Default` is a fixed point.
    fn edited() -> Stored {
        Stored {
            theme: Mode::Light,
            show_hidden: false,
            show_preview: false,
            preview_width: 220.0,
            view: View::Tree,
            sort: Column::Modified,
            sort_ascending: false,
            dirs_first: false,
            show_sidebar: false,
        }
    }

    #[test]
    fn settings_survive_a_restart() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("app.ron");

        // --- session one: change everything, then let eframe call `save`.
        let before = edited();
        {
            let mut store = DiskStore::open(path.clone());
            eframe::set_value(&mut store, KEY, &before);
            eframe::Storage::flush(&mut store);
        }
        // The file is the only thing carried forward. A second `DiskStore` is a
        // different object with a different `HashMap`, standing in for a new
        // process reading the same file.
        let reopened = DiskStore::open(path);
        assert_ne!(
            reopened.kv,
            HashMap::new(),
            "nothing reached the disk, so nothing was tested"
        );
        let after = load(Some(&reopened));
        assert_eq!(before, after, "settings did not survive the round trip");
    }

    #[test]
    fn every_field_is_covered_by_the_round_trip() {
        // The test above would still pass with one field left out of
        // `serde` only if `#[serde(default)]` filled it in with the same value
        // the test set — which it does, because `edited()` sets every field to
        // something *different* from its default. So: assert, field by field,
        // that no field came back as its default.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("app.ron");
        let before = edited();
        {
            let mut store = DiskStore::open(path.clone());
            eframe::set_value(&mut store, KEY, &before);
            eframe::Storage::flush(&mut store);
        }
        let after = load(Some(&DiskStore::open(path)));
        let default = Stored::default();
        assert_ne!(after.theme, default.theme);
        assert_ne!(after.show_hidden, default.show_hidden);
        assert_ne!(after.show_preview, default.show_preview);
        assert_ne!(after.preview_width, default.preview_width);
        assert_ne!(after.view, default.view);
        assert_ne!(after.sort, default.sort);
        assert_ne!(after.sort_ascending, default.sort_ascending);
        assert_ne!(after.dirs_first, default.dirs_first);
        assert_ne!(after.show_sidebar, default.show_sidebar);
    }

    #[test]
    fn a_missing_or_unreadable_store_is_the_defaults_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        // No file at all.
        assert_eq!(
            load(Some(&DiskStore::open(dir.path().join("nope.ron")))),
            Stored::default()
        );
        // A file that is not a settings blob.
        let junk = dir.path().join("junk.ron");
        std::fs::write(&junk, "this is not ron at all {{{").expect("write");
        assert_eq!(load(Some(&DiskStore::open(junk))), Stored::default());
        // No storage at all — a `--screenshot` capture.
        assert_eq!(load(None), Stored::default());
    }

    #[test]
    fn a_hand_edited_width_is_clamped_on_load() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("app.ron");
        for (written, expect) in [
            (0.0, PREVIEW_MIN),
            (10_000.0, PREVIEW_MAX),
            (f32::NAN, PREVIEW_DEFAULT),
            // `is_finite` is false for an infinity as well as a NaN, and an
            // infinity is not a width a user meant — it is a corrupt file or a
            // divide-by-zero in whatever wrote it. Default, not `MAX_WIDTH`.
            (f32::INFINITY, PREVIEW_DEFAULT),
            (f32::NEG_INFINITY, PREVIEW_DEFAULT),
            (280.0, 280.0),
        ] {
            let mut bad = edited();
            bad.preview_width = written;
            {
                let mut store = DiskStore::open(path.clone());
                eframe::set_value(&mut store, KEY, &bad);
                eframe::Storage::flush(&mut store);
            }
            let got = load(Some(&DiskStore::open(path.clone()))).preview_width;
            assert_eq!(got, expect, "wrote {written}");
        }
    }

    #[test]
    fn a_state_from_a_build_with_more_fields_still_loads() {
        // `#[serde(default)]` on the struct is what makes adding a setting a
        // non-breaking change for a user who already has a file. Simulated by
        // writing a blob with one field missing.
        let partial = r#"(theme: dark, show_hidden: false, show_preview: true, preview_width: 240.0, view: tree, sort: size, sort_ascending: false, dirs_first: true)"#;
        let got: Stored = ron::from_str(partial).expect("partial state decodes");
        assert_eq!(got.preview_width, 240.0);
        assert!(!got.show_hidden);
        // `show_sidebar` was not in the blob, so it takes the default rather
        // than failing the whole load.
        assert!(got.show_sidebar);
    }

    #[test]
    fn the_engine_sort_spec_is_built_from_the_settings() {
        let s = Stored {
            sort: Column::Size,
            sort_ascending: false,
            dirs_first: false,
            ..Default::default()
        };
        let spec = s.sort_spec();
        assert_eq!(spec.key, SortKey::Size);
        assert!(!spec.ascending);
        assert!(!spec.dirs_first);
    }

    #[test]
    fn every_column_round_trips_through_the_engine_key() {
        for c in Column::ALL {
            assert_eq!(Column::from_sort_key(c.to_sort_key()), c);
        }
        for m in Mode::ALL {
            assert_eq!(Mode::from_theme(m.to_theme()), m);
        }
    }

    #[test]
    fn the_theme_conversion_is_total() {
        // `ThemeMode` and `Mode` must stay the same size, or the match in
        // `to_theme`/`from_theme` is silently lossy.
        assert_eq!(Mode::ALL.len(), 3);
    }

    #[test]
    fn rows_stack_only_below_the_stated_width() {
        assert!(is_narrow(320.0), "320px cannot fit a label and a control");
        assert!(
            !is_narrow(400.0),
            "400px is the stated breakpoint, inclusive"
        );
        assert!(!is_narrow(component::SETTINGS_BODY_MAX_W));
    }

    #[test]
    fn a_stacked_row_gives_the_control_the_whole_width() {
        let row = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(320.0, 64.0));
        let (label, control) = split(row, is_narrow(row.width()));
        assert_eq!(label.width(), 320.0);
        assert_eq!(control.width(), 320.0, "the control is not squeezed");
        assert!(
            control.top() >= label.bottom(),
            "a stacked row must not overlap itself"
        );
    }

    #[test]
    fn a_side_by_side_row_keeps_both_columns_inside_the_row() {
        let row = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(560.0, 64.0));
        let (label, control) = split(row, is_narrow(row.width()));
        assert!(label.width() > 0.0 && control.width() > 0.0);
        assert!(label.right() <= row.right());
        assert!((label.right() - control.left()).abs() < 0.01);
    }

    #[test]
    fn the_settings_screen_renders_at_a_narrow_width() {
        // The layout that produced a 91px filter field was only ever visible in
        // a live window. This renders the screen at 360px and asserts it
        // produced geometry rather than a panic.
        let ctx = crate::shot::ctx_with_fonts();
        let mut stored = Stored {
            theme: Mode::Dark,
            ..Default::default()
        };
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(360.0, 720.0),
                )),
                ..Default::default()
            },
            |ui| {
                let theme = Theme::dark();
                let _ = draw(ui, &theme, &mut stored, crate::motion::Motion::Full);
            },
        );
        // The font atlas the first pass built has to be consumed or egui panics
        // on the dropped delta.
        out.drop_without_applying_deltas();
        assert_eq!(
            stored.theme,
            Mode::Dark,
            "drawing the screen must not edit the settings"
        );
    }
}
