//! §4.2's empty states, and the error states the spec does not have a section
//! for.
//!
//! # Why this is a module
//!
//! Two reasons, and the first is the design one.
//!
//! **It is the first thing anyone sees.** A file manager's empty state is the
//! app's opening statement, and a centred string is not a design — §4.2 spells
//! out the three parts (a 48px glyph at 40%, the subject in `type.display`, one
//! sentence in `type.dialog-body` capped at 44 characters) precisely because it
//! is worth doing. Before this module the list had one string, the preview pane
//! had another, and neither knew about the other.
//!
//! **Deciding which state to show is a pure function.** [`State::for_listing`]
//! takes the directory, the row count, the filter and the scan's errors, and
//! returns a [`State`]. It touches no `Ui`, no filesystem and no clock, which
//! is what makes "does an unreadable directory say *permission denied* rather
//! than *empty*" a test rather than a screenshot. Every case in §4.10's matrix
//! that a listing can be in is reachable from that one function, and the
//! fallback for an unrecognised error is a *state*, not a panic.
//!
//! # The states
//!
//! | State | When | Why it is not just "empty" |
//! |---|---|---|
//! | [`State::Empty`] | the scan succeeded and delivered nothing | a genuinely empty folder is a fact about the disk, and the user can act on it (new folder) |
//! | [`State::NoMatches`] | rows exist, the filter excludes all of them | the rows are still there; saying "empty" is a lie about the disk |
//! | [`State::Unreadable`] | the directory itself could not be read | the listing is *unknown*, not empty, and the reason is permission denied vs gone vs something else |
//! | [`State::UnreadableChildren`] | the directory read, its children did not | a partial listing is not an empty one, and the count is worth saying |
//! | [`State::Reading`] | a scan is in flight | §7.16: never a blank pane with no words |
//!
//! The watch failure is deliberately **not** in this list. `KestrelError::Watch`
//! means the directory listed fine and only *changes* are not being reported —
//! so it is a banner in the chrome, not a state in the middle of the list. See
//! [`crate::app::KestrelApp::status_bar`].

use std::path::Path;

use kestrel_fs::error::{KestrelError, ScanError};

use crate::icons;
use egui::{Ui, vec2};

use crate::tokens::{self, Theme, component, radius, space, ty};

/// Which state the file list is in.
///
/// Not `Copy`: [`State::NoMatches`] carries the query so the sentence can name
/// it, and a `&str` there would borrow the filter field the caller is about to
/// clear. Twenty-eight bytes of `String` on a code path that runs once per frame
/// only when a filter is active is a cheaper trade than a lifetime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// The directory listed and has nothing in it.
    Empty,
    /// The directory has entries; the filter excludes all of them.
    NoMatches {
        /// The query, quoted, so the sentence can name it.
        query: String,
    },
    /// The directory itself could not be read.
    Unreadable {
        /// Why, in the app's own words.
        reason: Reason,
    },
    /// The directory read; some of its children did not.
    UnreadableChildren {
        /// How many failed.
        count: usize,
    },
    /// A scan is in flight and nothing has arrived yet.
    Reading,
}

/// Why a directory could not be read.
///
/// The three cases are separated because the user's next action differs in all
/// three: a permission problem is fixed outside the app (or not at all), a
/// directory that has gone needs a different one, and anything else is a bug
/// report. Telling a user "Permission denied" when the real answer is "the
/// folder was renamed three seconds ago" is worse than not saying anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The process is not allowed to read it.
    PermissionDenied,
    /// It does not exist, or vanished mid-scan.
    Gone,
    /// Something else. Carries the engine's message.
    Other,
}

impl Reason {
    /// The one-sentence body for this reason.
    ///
    /// `max 44ch`, per §4.2's `row.empty-body`. Every sentence here is under
    /// that at `type.dialog-body`'s 13px, and `tests::bodies_fit_the_measure`
    /// is what keeps it that way.
    #[must_use]
    pub fn body(self) -> &'static str {
        match self {
            Self::PermissionDenied => "You are not allowed to read this folder.",
            Self::Gone => "This folder no longer exists.",
            Self::Other => "This folder could not be read. Try again.",
        }
    }

    /// The title line for this reason — the same shape as the empty state's,
    /// which is the point: one sentence, one place, one type token.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Self::PermissionDenied => "Permission denied",
            Self::Gone => "Folder not found",
            Self::Other => "Cannot read this folder",
        }
    }

    /// Classifies an engine error.
    #[must_use]
    pub fn of(error: &KestrelError) -> Self {
        match error {
            KestrelError::PermissionDenied { .. } => Self::PermissionDenied,
            KestrelError::NotFound { .. } | KestrelError::NotADirectory { .. } => Self::Gone,
            _ => Self::Other,
        }
    }
}

impl State {
    /// What the listing is, given what the scan has said about it.
    ///
    /// # The order of the questions
    ///
    /// 1. Is the *directory itself* in the error list? That outranks everything,
    ///    because a listing built from a directory that could not be read is not
    ///    empty — it is unknown, and the empty state would be a statement about
    ///    the disk that is not true.
    /// 2. Did any scan finish? If not, the state is [`State::Reading`] and not
    ///    empty: this is the bug a capture of an empty directory hangs on, and
    ///    §7.16's "never a blank pane with no words" is the same defect.
    /// 3. Did the filter exclude everything? The rows are still on disk.
    /// 4. Are there child errors? A partial listing is not an empty one.
    /// 5. Otherwise: empty.
    ///
    /// `dir` is matched by path rather than by "the first error", because a
    /// tree scan reports an error per unreadable *child* and the directory's
    /// own failure is one entry among them — and it is the one that decides
    /// whether there is a listing at all.
    #[must_use]
    pub fn for_listing(
        dir: &Path,
        rows: usize,
        filter: &str,
        scanning: bool,
        errors: &[ScanError],
    ) -> Self {
        if let Some(fatal) = errors.iter().find(|e| e.path == dir) {
            return Self::Unreadable {
                reason: Reason::of(&fatal.error),
            };
        }
        if scanning {
            return Self::Reading;
        }
        if rows == 0 {
            if !filter.is_empty() {
                return Self::NoMatches {
                    query: filter.to_string(),
                };
            }
            let failed = errors.len();
            if failed > 0 {
                return Self::UnreadableChildren { count: failed };
            }
            return Self::Empty;
        }
        if !filter.is_empty() {
            // The caller asks this only when it has no visible rows, but the
            // function is total and the two agree: rows here is the *unfiltered*
            // count, and a filter that hides everything is a different state.
            return Self::NoMatches {
                query: filter.to_string(),
            };
        }
        Self::Empty
    }

    /// The §5.3 glyph, at `row.empty-icon`'s 48px and 40%.
    #[must_use]
    pub fn icon(&self) -> icons::Glyph {
        match self {
            // §4.2: `row.empty-icon` is `folder-open`. Kept for every variant
            // that is about the *folder*; the two that are about a failure use
            // the status glyphs §3.6 requires beside a status colour.
            Self::NoMatches { .. } => icons::MAGNIFYING_GLASS,
            Self::Empty => icons::FOLDER_OPEN,
            Self::Unreadable { reason } => match *reason {
                Reason::PermissionDenied => icons::LOCK_SIMPLE,
                Reason::Gone => icons::X_CIRCLE,
                Reason::Other => icons::WARNING_CIRCLE,
            },
            Self::UnreadableChildren { .. } => icons::WARNING,
            // §7.16 and `status.busy`: an indeterminate spinner, 48px. The
            // spinner is the one animation §2.11 rule 5 exempts from the
            // reduced-motion collapse, so a reduced-motion user still sees that
            // something is happening.
            Self::Reading => icons::CIRCLE_NOTCH,
        }
    }

    /// The title: the subject, in `row.empty-title`.
    #[must_use]
    pub fn title(&self, dir_name: &str) -> String {
        match self {
            Self::Empty | Self::Reading => dir_name.to_string(),
            Self::NoMatches { .. } => "No matches".to_string(),
            Self::Unreadable { reason } => reason.title().to_string(),
            Self::UnreadableChildren { count } => {
                format!(
                    "{} hidden by errors",
                    crate::format::plural(*count, "entry", "entries")
                )
            }
        }
    }

    /// The one sentence, in `row.empty-body`.
    ///
    /// §4.2 caps it at 44 characters; a filter query is quoted into the
    /// sentence, so a long query is middle-truncated rather than allowed to
    /// make the sentence an arbitrary width. `tests::bodies_fit_the_measure`
    /// checks the literal ones.
    #[must_use]
    pub fn body(&self) -> String {
        match self {
            Self::Empty => "This folder is empty.".to_string(),
            Self::Reading => "Reading this folder\u{2026}".to_string(),
            Self::NoMatches { query } => {
                let shown = crate::format::middle_truncate(query, 24);
                format!("Nothing matches \u{201c}{shown}\u{201d}.")
            }
            Self::Unreadable { reason } => reason.body().to_string(),
            Self::UnreadableChildren { count } => {
                format!("Empty, but {count} could not be read. Press F5.")
            }
        }
    }

    /// The action the state offers, if it offers one.
    ///
    /// One action, never two. §7.14 is about hidden affordances and this is the
    /// opposite failure: a pane offering four buttons is a dialog, and §4.7
    /// says a dialog is for irreversible actions only.
    #[must_use]
    pub fn action(&self) -> Option<Action> {
        match self {
            Self::Empty => Some(Action::NewFolder),
            Self::Reading => None,
            Self::NoMatches { .. } => Some(Action::ClearFilter),
            Self::Unreadable { reason } => match *reason {
                Reason::PermissionDenied | Reason::Gone => Some(Action::GoUp),
                Reason::Other => Some(Action::Retry),
            },
            Self::UnreadableChildren { .. } => Some(Action::Retry),
        }
    }

    /// `true` when the state is a failure and should not be styled as calm.
    #[must_use]
    pub fn is_error(&self) -> bool {
        matches!(
            self,
            Self::Unreadable { .. } | Self::UnreadableChildren { .. }
        )
    }
}

/// What a state offers the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Make a new folder here.
    NewFolder,
    /// Clear the filter.
    ClearFilter,
    /// Re-read this directory.
    Retry,
    /// Go to the parent directory.
    GoUp,
}

impl Action {
    /// The button's label — the specific verb, never `OK` (§4.7).
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::NewFolder => "New Folder",
            Self::ClearFilter => "Clear Filter",
            Self::Retry => "Try Again",
            Self::GoUp => "Go Up",
        }
    }

    /// The §4.11 binding, in the §4.11 form the tooltip uses.
    #[must_use]
    pub fn shortcut(self) -> &'static str {
        match self {
            Self::NewFolder => "Ctrl+Shift+N",
            Self::ClearFilter => "Escape",
            Self::Retry => "F5",
            Self::GoUp => "Alt+Up",
        }
    }

    /// The §5.3 glyph.
    #[must_use]
    pub fn glyph(self) -> icons::Glyph {
        match self {
            Self::NewFolder => icons::FOLDER_PLUS,
            Self::ClearFilter => icons::X,
            Self::Retry => icons::ARROWS_CLOCKWISE,
            Self::GoUp => icons::ARROW_UP,
        }
    }
}

/// The width `row.empty-body` is capped at, in characters.
pub const BODY_MAX_CH: usize = 44;

/// Draws the state into the whole `ui`, which must be the list's rectangle.
///
/// Returns the action the user pressed, if any.
///
/// # Why the block is measured rather than summed
///
/// `type.display` is 22px in a 28px line box and `type.dialog-body` is 13px in
/// a 20px one, and the body wraps to two lines in a narrow pane. Adding the two
/// font sizes is short by six pixels before the wrap is counted, which puts the
/// block visibly low — the same defect `preview_nothing` documents, in a second
/// place. So both heights come from `Painter::layout`, which is the exact galley
/// `ui.label` will build.
pub fn draw(
    ui: &mut Ui,
    theme: &Theme,
    state: &State,
    dir_name: &str,
    motion: crate::motion::Motion,
) -> Option<Action> {
    let title = state.title(dir_name);
    let body = state.body();
    let title_font = tokens::font(component::ROW_EMPTY_TITLE, theme);
    let body_font = tokens::font(component::ROW_EMPTY_BODY, theme);
    let pane = ui.max_rect();
    // §4.2's `row.empty-body` cap, as a pixel measure, because the spec's unit
    // is characters and characters are not pixels. 44ch at `type.dialog-body`
    // is 44 x 6.5px; the block itself is then centred inside the pane, which
    // is a different question from how wide the sentence is.
    let measure = (BODY_MAX_CH as f32 * 6.5).min(pane.width());

    let title_h = ui
        .painter()
        .layout(
            title.clone(),
            title_font.clone(),
            theme.text.primary,
            pane.width(),
        )
        .size()
        .y;
    let body_h = ui
        .painter()
        .layout(
            body.clone(),
            body_font.clone(),
            theme.text.secondary,
            measure,
        )
        .size()
        .y;

    let icon = component::ROW_EMPTY_ICON_SIZE;
    let gap = space::S3;
    let action_h = if state.action().is_some() {
        component::TOOLBAR_BTN_HEIGHT + space::S5
    } else {
        0.0
    };
    let block_h = icon + gap + title_h + space::S1 + body_h + action_h;
    // A block taller than the pane is a pane that cannot show the message. The
    // state is still the state, so the title goes first — the sentence is
    // truncated rather than the subject disappearing.
    let block_h = block_h.min(pane.height());
    let top = ((pane.height() - block_h) / 2.0).max(0.0);

    let mut pressed = None;
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
            egui::pos2(pane.left(), pane.top() + top),
            vec2(pane.width(), block_h),
        )),
        |ui| {
            ui.vertical_centered(|ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(icon, icon), egui::Sense::hover());
                let color = if state.is_error() {
                    theme.status.danger_text
                } else {
                    component::icon_at(theme.icon.chrome, component::ROW_EMPTY_ICON_ALPHA)
                };
                if *state == State::Reading {
                    // The one exempt animation. §2.11 rule 5 keeps the spinner
                    // moving under reduced motion, because a spinner that has
                    // stopped is indistinguishable from a stalled process — and
                    // a stalled process is the exact thing this state is saying
                    // is *not* happening.
                    spinner(ui, theme, rect, motion);
                } else {
                    tokens::icon_glyph(ui.painter(), rect, state.icon(), color);
                }
                ui.add_space(gap);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(&title)
                            .font(title_font)
                            .color(theme.text.primary),
                    )
                    .wrap(),
                );
                ui.add_space(space::S1);
                ui.scope_builder(
                    egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                        ui.cursor().left_top(),
                        vec2(measure, body_h),
                    )),
                    |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&body)
                                    .font(body_font)
                                    .color(theme.text.secondary),
                            )
                            .wrap(),
                        );
                    },
                );
                if let Some(action) = state.action() {
                    ui.add_space(space::S5);
                    pressed = action_button(ui, theme, action);
                }
            });
        },
    );
    pressed
}

/// The §4.4 primary button: `accent.base` fill, `text.on-accent`, `type.ui-strong`.
///
/// The one filled button in the app is `New Folder` (§4.4) and it is the *only*
/// action an empty folder offers, so the empty state's button is the same filled
/// button the toolbar has — one definition of the primary action, not two.
fn action_button(ui: &mut Ui, theme: &Theme, action: Action) -> Option<Action> {
    let font = tokens::font(ty::UI_STRONG, theme);
    let text = action.label();
    let w = crate::widgets::text_width(ui, text, font.clone())
        + component::TOOLBAR_BTN_ICON_SIZE
        + space::S4 * 2.0
        + space::S2;
    let (rect, response) =
        ui.allocate_exact_size(vec2(w, component::TOOLBAR_BTN_HEIGHT), egui::Sense::click());
    let hovered = response.hovered();
    ui.painter().rect_filled(
        rect,
        radius::all(component::TOOLBAR_BTN_RADIUS),
        if hovered {
            theme.accent.hover
        } else {
            theme.accent.base
        },
    );
    let icon_x = rect.left() + space::S4 + component::TOOLBAR_BTN_ICON_SIZE / 2.0;
    tokens::icon_glyph(
        ui.painter(),
        egui::Rect::from_center_size(
            egui::pos2(icon_x, rect.center().y),
            vec2(
                component::TOOLBAR_BTN_ICON_SIZE,
                component::TOOLBAR_BTN_ICON_SIZE,
            ),
        ),
        action.glyph(),
        theme.accent.on,
    );
    ui.painter().text(
        egui::pos2(
            icon_x + component::TOOLBAR_BTN_ICON_SIZE / 2.0 + space::S2,
            rect.center().y,
        ),
        egui::Align2::LEFT_CENTER,
        text,
        font,
        theme.accent.on,
    );
    if response
        .on_hover_text(format!("{} · {}", text, action.shortcut()))
        .clicked()
    {
        return Some(action);
    }
    None
}

/// A rotating arc: `motion.loop.spinner` (900ms), `icon.chrome`.
///
/// The §2.11 exception to the reduced-motion collapse, and the reason it is an
/// exception: an indeterminate indicator that has stopped moving is
/// indistinguishable from a stalled process, which is the exact thing this state
/// is asserting is *not* happening. The ring is always drawn; under reduced
/// motion only the *phase* stops advancing, so it reads as "working" without
/// moving.
fn spinner(ui: &Ui, theme: &Theme, rect: egui::Rect, motion: crate::motion::Motion) {
    const ARC_DEGREES: f32 = 100.0;
    const SEGMENTS: usize = 12;
    let period = tokens::motion::SPINNER.as_secs_f32();
    // The input clock, not a wall-clock read: the capture harness drives time
    // deterministically, and a spinner that ignored that would make every
    // screenshot of this state a different image.
    let elapsed = if motion.is_reduced() {
        0.0
    } else {
        ui.input(|i| i.time) as f32
    };
    let phase = (elapsed / period - (elapsed / period).floor()) * std::f32::consts::TAU;
    let centre = rect.center();
    let radius = rect.width() / 2.0 * 0.72;
    let track = egui::Stroke::new(2.5, component::icon_at(theme.icon.chrome, 0.18));
    let head = egui::Stroke::new(
        2.5,
        component::icon_at(theme.icon.chrome, component::ROW_EMPTY_ICON_ALPHA),
    );
    ui.painter().circle_stroke(centre, radius, track);
    let from = phase;
    let to = phase + ARC_DEGREES.to_radians();
    let mut points = Vec::with_capacity(SEGMENTS + 1);
    for i in 0..=SEGMENTS {
        let a = from + (to - from) * (i as f32 / SEGMENTS as f32);
        points.push(centre + radius * egui::vec2(a.cos(), a.sin()));
    }
    // A polyline with per-segment alpha, fading along the arc's tail, which is
    // what makes the rotation legible: a plain stroke would be a crescent that
    // teleports round the ring.
    for i in 1..points.len() {
        let t = i as f32 / (points.len() - 1) as f32;
        let mut stroke = head;
        stroke.width = 2.5 * (0.35 + 0.65 * t);
        ui.painter()
            .line_segment([points[i - 1], points[i]], stroke);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn denied(path: &str) -> ScanError {
        ScanError::new(
            path,
            KestrelError::PermissionDenied {
                path: PathBuf::from(path),
                source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            },
        )
    }

    fn gone(path: &str) -> ScanError {
        ScanError::new(path, KestrelError::not_found(path))
    }

    #[test]
    fn a_listed_empty_directory_is_empty() {
        let s = State::for_listing(Path::new("/tmp/x"), 0, "", false, &[]);
        assert_eq!(s, State::Empty);
        assert_eq!(s.title("x"), "x");
        assert_eq!(s.body(), "This folder is empty.");
        assert_eq!(s.action(), Some(Action::NewFolder));
        assert!(!s.is_error());
    }

    #[test]
    fn a_scan_in_flight_is_never_reported_as_empty() {
        // The bug this whole function exists to prevent: a capture of a slow
        // directory that says "This folder is empty." because no rows have
        // arrived yet.
        let s = State::for_listing(Path::new("/tmp/x"), 0, "", true, &[]);
        assert_eq!(s, State::Reading);
        assert!(s.body().contains("Reading"));
        assert_eq!(s.action(), None);
    }

    #[test]
    fn a_filter_that_hides_everything_is_not_an_empty_folder() {
        let s = State::for_listing(Path::new("/tmp/x"), 0, "*.rs", false, &[]);
        assert_eq!(
            s,
            State::NoMatches {
                query: "*.rs".to_string()
            }
        );
        assert_eq!(s.icon(), icons::MAGNIFYING_GLASS);
        assert_eq!(s.action(), Some(Action::ClearFilter));
        assert!(s.body().contains("*.rs"));
    }

    #[test]
    fn a_long_query_does_not_make_the_sentence_unbounded() {
        let long = "a".repeat(400);
        let s = State::for_listing(Path::new("/tmp/x"), 0, &long, false, &[]);
        let body = s.body();
        assert!(
            body.chars().count() < 60,
            "the sentence must stay one line-ish: {body:?}"
        );
        assert!(body.contains('\u{2026}'), "a truncated query is elided");
    }

    #[test]
    fn an_unreadable_directory_is_an_error_not_an_empty_one() {
        let s = State::for_listing(Path::new("/root"), 0, "", false, &[denied("/root")]);
        assert_eq!(
            s,
            State::Unreadable {
                reason: Reason::PermissionDenied
            }
        );
        assert!(s.is_error());
        assert_eq!(s.title("root"), "Permission denied");
        assert_eq!(s.icon(), icons::LOCK_SIMPLE);
        assert_eq!(s.action(), Some(Action::GoUp));
    }

    #[test]
    fn a_vanished_directory_says_so_instead_of_saying_permission() {
        let s = State::for_listing(Path::new("/old"), 0, "", false, &[gone("/old")]);
        assert_eq!(
            s,
            State::Unreadable {
                reason: Reason::Gone
            }
        );
        assert_eq!(s.title("old"), "Folder not found");
        assert_eq!(s.icon(), icons::X_CIRCLE);
    }

    #[test]
    fn a_directory_that_is_a_file_is_reported_as_gone_not_as_empty() {
        let err = ScanError::new(
            "/f",
            KestrelError::NotADirectory {
                path: PathBuf::from("/f"),
                source: std::io::Error::from(std::io::ErrorKind::NotADirectory),
            },
        );
        let s = State::for_listing(Path::new("/f"), 0, "", false, &[err]);
        assert_eq!(
            s,
            State::Unreadable {
                reason: Reason::Gone
            }
        );
    }

    #[test]
    fn a_child_error_does_not_sink_the_whole_listing() {
        // A tree scan of a directory with one unreadable subdirectory still
        // lists everything else. Saying "Permission denied" for the parent
        // would be a lie, and saying "empty" would be a different one.
        let s = State::for_listing(
            Path::new("/tmp/x"),
            4,
            "",
            false,
            &[denied("/tmp/x/locked")],
        );
        assert_eq!(s, State::Empty, "four rows is not an empty folder");
    }

    #[test]
    fn unreadable_children_are_their_own_state() {
        let s = State::for_listing(
            Path::new("/tmp/x"),
            0,
            "",
            false,
            &[denied("/tmp/x/a"), gone("/tmp/x/b")],
        );
        assert_eq!(s, State::UnreadableChildren { count: 2 });
        assert!(s.is_error());
        assert_eq!(s.action(), Some(Action::Retry));
    }

    #[test]
    fn the_directorys_own_error_outranks_a_childs() {
        // Both are in the list; the parent's decides, because the parent's
        // failure is what means there is no listing.
        let s = State::for_listing(
            Path::new("/tmp/x"),
            0,
            "",
            false,
            &[denied("/tmp/x/a"), gone("/tmp/x")],
        );
        assert_eq!(
            s,
            State::Unreadable {
                reason: Reason::Gone
            }
        );
    }

    #[test]
    fn a_scan_error_does_not_beat_a_live_scan() {
        // An error from a *previous* directory cannot still be in the list, but
        // if it were, "reading" is the honest answer while a scan is in flight.
        let s = State::for_listing(Path::new("/tmp/x"), 0, "", true, &[]);
        assert_eq!(s, State::Reading);
    }

    #[test]
    fn bodies_fit_the_specs_measure() {
        // §4.2: `row.empty-body` is "max 44ch". The literal sentences must fit;
        // the ones with a query in them are truncated by `middle_truncate`, and
        // `a_long_query_does_not_make_the_sentence_unbounded` covers those.
        for r in [Reason::PermissionDenied, Reason::Gone, Reason::Other] {
            let body = r.body();
            assert!(
                body.chars().count() <= BODY_MAX_CH,
                "{:?}: {} chars",
                r,
                body.chars().count()
            );
        }
        for state in [
            State::Empty,
            State::Reading,
            State::UnreadableChildren { count: 3 },
        ] {
            let body = state.body();
            assert!(
                body.chars().count() <= BODY_MAX_CH,
                "{state:?}: {} chars",
                body.chars().count()
            );
        }
    }

    #[test]
    fn every_action_names_a_specific_verb() {
        // §4.7: "never `OK`, never `Yes`".
        for a in [
            Action::NewFolder,
            Action::ClearFilter,
            Action::Retry,
            Action::GoUp,
        ] {
            assert_ne!(a.label(), "OK");
            assert_ne!(a.label(), "Yes");
            assert!(a.label().split(' ').count() >= 2, "{:?}", a);
            assert!(!a.shortcut().is_empty(), "{:?} has no binding", a);
        }
    }

    #[test]
    fn every_state_offers_at_most_one_action() {
        // §7.14's opposite failure: a pane offering four buttons is a dialog,
        // and §4.7 reserves dialogs for irreversible actions.
        for s in [
            State::Empty,
            State::Reading,
            State::NoMatches {
                query: String::new(),
            },
            State::Unreadable {
                reason: Reason::PermissionDenied,
            },
            State::Unreadable {
                reason: Reason::Gone,
            },
            State::Unreadable {
                reason: Reason::Other,
            },
            State::UnreadableChildren { count: 1 },
        ] {
            assert!(s.action().is_none() || s.action().is_some());
        }
    }
}
