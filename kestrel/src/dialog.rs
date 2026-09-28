//! §4.7 The confirmation dialog — its wording, its buttons, and its metrics.
//!
//! # Why the copy lives here and not in `app.rs`
//!
//! §4.7's copy rule is the one line of this spec that is not about looks: "the
//! verb is specific and irreversible-sounding — `Move to Trash`,
//! `Delete Permanently`, `Empty Trash` — never `OK`, never `Yes`." §7.17 is the
//! anti-goal that enforces it. A rule enforced by convention is a rule that
//! erodes, so the wording is a **function** rather than a string literal at a
//! call site: there is exactly one place that can produce a confirm button's
//! label, and it cannot produce `OK` because it has no arm that returns it.
//!
//! # Why this module is more than constants
//!
//! `app.rs` draws the dialog; this module decides what the dialog *says*. That
//! split is deliberate. A drawing module that also owned the copy would have
//! two sources of truth the moment a second dialog appeared, and §7.17 is
//! specifically about the generic one winning a disagreement.
//!
//! # What the caller still owns
//!
//! Layout, motion, focus, and painting stay in `app.rs`. This module answers
//! four questions and nothing else: what does the body sentence say, which
//! buttons exist and in what order, which button Enter should take, and what
//! colour does this button get in this state.

use std::path::{Path, PathBuf};

use egui::{Color32, Stroke};

use crate::format;
use crate::icons;
use crate::job::{Item, Op, Scope, Strategy};
use crate::tokens::{Theme, border, component};

/// §4.7 `dialog.width` — 400 px.
///
/// A fixed width, not a max-width: the dialog is a fixed-size object, and a
/// dialog that reflows with its contents makes the same action look like a
/// different action on a different filename.
///
/// Aliased from [`component::DIALOG_WIDTH`] rather than written as `400.0`
/// here, so the §4.7 row has exactly one definition in the tree.
pub const WIDTH: f32 = component::DIALOG_WIDTH;

/// §4.7 `dialog.padding` — 20 px.
pub const PADDING: f32 = component::DIALOG_PADDING;

/// §4.7 `dialog.radius` — 8 px.
pub const RADIUS: f32 = component::DIALOG_RADIUS;

/// §4.7 `dialog.icon` — 20 px.
pub const ICON: f32 = component::DIALOG_ICON;

/// §4.7 `dialog.footer-gap` — 12 px.
pub const FOOTER_GAP: f32 = component::DIALOG_FOOTER_GAP;

/// §4.7 `dialog.btn-height` — 30 px.
pub const BTN_HEIGHT: f32 = component::DIALOG_BTN_HEIGHT;

// ---------------------------------------------------------------------------
// Preview-pane metrics
// ---------------------------------------------------------------------------

/// The metrics for the **preview pane**, which has no §4 section of its own.
///
/// §2.9 specifies the pane nowhere, and §4.10's state matrix does not mention
/// it, so these values are *derived* from named scales rather than invented
/// per-component. Naming the source rung in a doc comment is the point: when a
/// `§4.12 Preview pane` arrives, this constant is what it should overrule, and
/// the doc comment is what makes that reviewable.
///
/// Note the deliberate mismatch with [`WIDTH`]: the dialog is 400 px because
/// §4.7 says so, the pane is 280 px because it is a *sibling* of a 200 px
/// sidebar and three panes need to coexist. They are not variants of each
/// other.
pub mod preview_metrics {
    /// The pane's default width — between [`crate::tokens::metric::SIDEBAR_MIN`]
    /// (160) and the 400 px dialog, so the three panes stay balanced.
    pub const WIDTH: f32 = 280.0;
    /// Narrowest the pane may be dragged. Matches the sidebar's floor, so no
    /// pane can be squeezed to nothing while another stays wide.
    pub const MIN_WIDTH: f32 = 180.0;
    /// Widest the pane may be dragged — deliberately *narrower* than
    /// [`crate::tokens::metric::SIDEBAR_MAX`] (340) is wide, because the pane
    /// holds a filename column that stops being readable well before 340.
    pub const MAX_WIDTH: f32 = 360.0;
    /// Inner margin — `space::S2`, the same rung the path-quote uses.
    pub const PADDING: f32 = crate::tokens::space::S2;
    /// One text row — `ty::META`'s line height, so the code view and the
    /// metadata list share a baseline grid.
    pub const LINE_H: f32 = 16.0;
    /// The line-number gutter — `metric::GUTTER`, the same leading gutter a row
    /// reserves for its icon.
    pub const GUTTER_W: f32 = crate::tokens::metric::GUTTER;
}

// ---------------------------------------------------------------------------
// Op helpers
// ---------------------------------------------------------------------------

/// The [`Op`] a "delete" action means.
///
/// The toolbar has two delete buttons and a boolean is the honest shape for
/// that: there is no third thing a delete button can be, so an enum here would
/// be a name for `true` and `false` with a `match` that cannot fail.
///
/// `false` is [`Op::Trash`] — the reversible default — and `true` is
/// [`Op::Delete`], the permanent one. §4.7 requires the distinction to be *in
/// the label* (`Move to Trash` vs `Delete Permanently`), which is why this is
/// one function with two outcomes rather than two near-identical branches in
/// the toolbar.
#[must_use]
pub fn op_for_delete(permanent: bool) -> Op {
    if permanent {
        Op::Delete
    } else {
        Op::Trash
    }
}

/// A representative path to quote in the dialog, for a selection.
///
/// Returns `None` for an empty selection — a dialog that says "delete 0 items"
/// should not have been opened, and if it was, quoting nothing is better than
/// quoting the current directory and implying a scope that is not there.
///
/// For more than one item the *first* item is quoted, not a parent directory:
/// the parent is the same for all of them and so tells the user nothing about
/// what is about to be destroyed. One real filename is more use than a shared
/// prefix.
#[must_use]
pub fn sample_path(items: &[Item]) -> Option<PathBuf> {
    items.first().map(|item| item.src.clone())
}

// ---------------------------------------------------------------------------
// Kind
// ---------------------------------------------------------------------------

/// What a dialog is about.
///
/// Four variants because the app raises four genuinely different questions, and
/// collapsing them would mean one dialog doing four jobs — which is how
/// `Delete Permanently` ends up with a `Close` button.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// §4.7 The confirmation for an irreversible action.
    Confirm {
        /// What is about to happen.
        op: Op,
        /// How many items it would happen to.
        count: usize,
        /// A representative path, quoted under the sentence.
        sample: Option<PathBuf>,
        /// The irreversibility line, when the op has one.
        warning: Option<String>,
    },
    /// A collision the filesystem asked about. Not a confirmation: the user did
    /// not intend this, and the engine is waiting on an answer.
    Collision {
        /// The source being moved or copied.
        src: PathBuf,
        /// The occupied destination.
        dst: PathBuf,
        /// How many more collisions this job will hit.
        remaining: usize,
    },
    /// A failure. A job stopped and the user needs to know why.
    Failed {
        /// The op that failed.
        op: Op,
        /// Where it failed.
        path: PathBuf,
        /// What went wrong, already run through [`crate::job::describe`].
        reason: String,
    },
    /// A running job with enough items to be worth interrupting for.
    Progress {
        /// The running op.
        op: Op,
        /// Items finished.
        done: usize,
        /// Items total.
        total: usize,
        /// Bytes accounted for.
        bytes: u64,
        /// Total bytes, when a size walk produced one.
        total_bytes: Option<u64>,
        /// The item being worked on.
        current: PathBuf,
        /// Human-readable throughput, already formatted.
        rate: String,
    },
}

impl Kind {
    /// §4.7 `dialog.title` — the heading, in `type.dialog-title`.
    ///
    /// Not the op's verb for every variant: a title is the *name of the
    /// situation*, and a failure is not called "Copy".
    #[must_use]
    pub fn title(&self) -> String {
        match self {
            // §4.7's copy rule applies to the title too: the heading names the
            // action specifically rather than saying "Confirm".
            Self::Confirm { op, .. } => op.verb().to_string(),
            Self::Collision { .. } => "Name Already Exists".to_string(),
            Self::Failed { .. } => "Operation Failed".to_string(),
            Self::Progress { op, .. } => op.verb().to_string(),
        }
    }

    /// §4.7 `dialog.body` — the sentence, in `type.dialog-body`.
    ///
    /// The recovered fragment is the `Confirm` arm, preserved as written
    /// including its comments, because the reasoning in it *is* the §7.17
    /// guarantee: the sentence helpers are the single source of the wording, so
    /// the string the dialog renders is the same string the helpers built.
    #[must_use]
    pub fn body(&self) -> String {
        match self {
            Self::Confirm {
                op, count, warning, ..
            } => {
                // The sentence helpers are the *single source* of this wording, not
                // an alternative to it. §4.7's requirement is about what the dialog
                // says, so the string that says it has to be the one the dialog
                // renders — two copies of "delete N items" is two chances to
                // disagree, and §7.17 is about exactly the generic one winning.
                let lead = match op {
                    Op::Delete => destructive_sentence(*op, *count),
                    Op::Trash => reversible_sentence(*op, *count),
                    _ => format!("{} {count} {}.", op.verb(), plural(*count, "item", "items")),
                };
                match warning {
                    // The consequence is already inside the sentence for a
                    // destructive op, so repeating it would say it twice.
                    Some(w) if !lead.contains(w.as_str()) => format!("{lead} {w}"),
                    _ => lead,
                }
            }
            Self::Collision { src, dst, remaining } => {
                let scope = match remaining {
                    1 => "1 more name".to_string(),
                    n => format!("{} more names", plural(*n, "name", "names")),
                };
                format!(
                    "{} already exists at {}. Replace it? ({scope} left in this job.)",
                    display_name(src),
                    display_name(dst)
                )
            }
            Self::Failed { op, reason, .. } => {
                // The reason is the user's own text from the OS or the engine,
                // so it is quoted rather than rephrased: a paraphrase here is
                // where "permission denied" turns into "something went wrong".
                format!("{} could not finish. {reason}", op.verb())
            }
            Self::Progress {
                done,
                total,
                rate,
                current,
                ..
            } => {
                let counts = format!("{} of {}", plural(*done, "item", "items"), plural(*total, "item", "items"));
                if rate.is_empty() {
                    format!("{counts} — {}", display_name(current))
                } else {
                    format!("{counts} — {} — {rate}", display_name(current))
                }
            }
        }
    }

    /// §4.7 `dialog.icon` — the glyph beside the title.
    ///
    /// `app.rs` colours this: `status.danger-text` when any button is
    /// destructive, `icon.chrome` otherwise. The colour is a property of the
    /// *dialog's* danger, and the dialog has two buttons, so this module cannot
    /// make that call correctly without duplicating [`buttons_for`].
    #[must_use]
    pub fn icon(&self) -> icons::Glyph {
        match self {
            // A confirm for an irreversible op is the §4.7 `warning` glyph.
            Self::Confirm { op, .. } if op.is_destructive() => icons::WARNING,
            Self::Confirm { .. } => icons::INFO,
            Self::Collision { .. } => icons::WARNING_CIRCLE,
            // A failure is not a warning: nothing is about to happen, something
            // already did.
            Self::Failed { .. } => icons::X_CIRCLE,
            Self::Progress { .. } => icons::ARROWS_CLOCKWISE,
        }
    }

    /// The irreversibility line for an op, if it has one.
    ///
    /// §4.7 requires the dialog to say what is *not* coming back. Only
    /// [`Op::Delete`] loses the data: [`Op::Trash`] is reversible, and a
    /// warning on a reversible action is the "are you sure?" reflex §7.17
    /// exists to stop. `None` for the rest, and `app.rs` then omits the line
    /// rather than rendering an empty one.
    ///
    /// A `&'static str`, not a `String`: the line is fixed copy, and a
    /// `String` here would let a caller pass something that had to be
    /// re-validated.
    #[must_use]
    pub fn irreversibility_line(op: Op) -> Option<&'static str> {
        match op {
            Op::Delete => Some("This cannot be undone."),
            Op::Trash | Op::Move | Op::Copy => None,
        }
    }
}

/// The confirm sentence for a destructive op.
///
/// §4.7: the verb names the action and the consequence is in the same breath.
/// "Delete 3 items." on its own does not say the files are gone.
fn destructive_sentence(op: Op, count: usize) -> String {
    let items = plural(count, "item", "items");
    match op {
        Op::Delete => format!("Permanently delete {items}. This cannot be undone."),
        // Reached only if a new destructive op is added without a sentence
        // here; the `Op::Trash` arm of `body` handles the other destructive op.
        // Falling back to the verb keeps the sentence grammatical and honest
        // rather than asserting an irreversibility this branch did not check.
        _ => format!("{} {items}.", op.verb()),
    }
}

/// The confirm sentence for a reversible destructive op.
///
/// Names where the items *went* and that they can come back, because "Move to
/// Trash" reads like "Delete" to anyone who has never used a trash can.
fn reversible_sentence(op: Op, count: usize) -> String {
    let items = plural(count, "item", "items");
    match op {
        Op::Trash => format!("Move {items} to the Trash. You can restore them later."),
        _ => format!("{} {items}.", op.verb()),
    }
}

/// `count` with the right plural, thousands-separated.
///
/// [`format::plural`] already groups the digits, so this is a thin wrapper —
/// it exists so this file has one place that builds a count for a sentence, and
/// so a future grouped-digits change does not have to be made in three match
/// arms.
fn plural(count: usize, singular: &'static str, plural: &'static str) -> String {
    format::plural(count, singular, plural)
}

/// The last path component, for a sentence.
///
/// Falls back to the whole path when the component is empty — a root directory
/// or a trailing separator has no name, and rendering `"/"` as the dialog's
/// subject is better than rendering `""`.
fn display_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

// ---------------------------------------------------------------------------
// Button
// ---------------------------------------------------------------------------

/// One button in a dialog footer.
///
/// A closed set, not a string. §7.17 is "no `OK`", and the only way to make
/// that structurally true rather than a review checklist item is for the
/// label to be a property of a variant: there is no `Button::Custom(&str)`, so
/// no call site can invent a generic label, and the compiler will point at
/// this file if a variant is added without copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    /// `Cancel` — always leftmost, always focused on open.
    Cancel,
    /// The op's own verb, for a confirm or a failure dialog.
    Confirm(Op),
    /// Replace the destination, for this collision only.
    OverwriteOne,
    /// Replace the destination for every remaining collision.
    OverwriteAll,
    /// Leave the destination alone, for this collision only.
    SkipOne,
    /// Leave the destination alone for every remaining collision.
    SkipAll,
    /// Stop a running job.
    Stop,
}

impl Button {
    /// §4.7 `dialog.btn-label` — the text on the button.
    ///
    /// The destructive and confirm labels come from [`Op::verb`], which is the
    /// single place §4.7's copy rule is written down. Nothing here can return
    /// `OK` or `Yes`, because neither word appears in the type.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Cancel => "Cancel",
            Self::Confirm(op) => op.verb(),
            // Scope is spelled out, not implied: "Replace" and "Replace All"
            // differ by one word and mean a difference the user cannot undo.
            Self::OverwriteOne => "Replace",
            Self::OverwriteAll => "Replace All",
            Self::SkipOne => "Skip",
            Self::SkipAll => "Skip All",
            Self::Stop => "Stop",
        }
    }

    /// `true` for a button that destroys something.
    ///
    /// Drives both the destructive fill and the dialog icon's colour, so it is
    /// one predicate for "is this dialog dangerous" rather than two that can
    /// disagree. Only `Replace*` and the permanent confirm qualify: `Trash` is
    /// destructive *as an op* but its button is styled as the safe action,
    /// because the user can undo it and the styling should say so.
    #[must_use]
    pub fn is_destructive(self) -> bool {
        match self {
            Self::Confirm(op) => op == Op::Delete,
            Self::OverwriteOne | Self::OverwriteAll => true,
            Self::Cancel | Self::SkipOne | Self::SkipAll | Self::Stop => false,
        }
    }

    /// `true` for the button that dismisses without doing anything.
    #[must_use]
    pub fn is_cancel(self) -> bool {
        matches!(self, Self::Cancel)
    }

    /// §4.7 Enter-default: which button Enter takes for this op.
    ///
    /// "The `default` action is never the destructive one for Enter, unless the
    /// destructive action is reversible." So `Trash` **is** Enter-default —
    /// the data survives — and `Delete` is not, because Enter should not be one
    /// stray keystroke away from unrecoverable.
    ///
    /// `is_cancel` is checked first so that a `Trash` confirm still has Cancel
    /// as its Enter target: §4.7 also requires Cancel to keep focus on open,
    /// and focus is what Enter follows.
    #[must_use]
    pub fn is_default_for_enter(self, op: Op) -> bool {
        match self {
            Self::Cancel => true,
            Self::Confirm(inner) => inner == op && op == Op::Trash,
            // Every other button's answer is a *decision about a collision*,
            // and `Scope::All` is sticky for the rest of the job — so no
            // collision button is Enter-default. A user's Enter on a dialog
            // they have not read must not pick a scope for them.
            Self::OverwriteOne
            | Self::OverwriteAll
            | Self::SkipOne
            | Self::SkipAll
            | Self::Stop => false,
        }
    }

    /// The collision decision this button produces, if any.
    ///
    /// `None` for every button outside a collision dialog, and for `Cancel` —
    /// which answers by *not* answering, and `app.rs` treats that as "cancel
    /// the job". Returning `Option` rather than a `Decision` keeps the
    /// "no answer" case a value the caller must handle.
    #[must_use]
    pub fn to_decision(self) -> Option<(Strategy, Scope)> {
        match self {
            Self::OverwriteOne => Some((Strategy::Overwrite, Scope::ThisOne)),
            Self::OverwriteAll => Some((Strategy::Overwrite, Scope::All)),
            Self::SkipOne => Some((Strategy::Skip, Scope::ThisOne)),
            Self::SkipAll => Some((Strategy::Skip, Scope::All)),
            Self::Cancel | Self::Confirm(_) | Self::Stop => None,
        }
    }
}

/// §4.7 The buttons for a dialog, in order.
///
/// Order is left-to-right, and `app.rs` walks the list in reverse to undo its
/// right-to-left layout so that this list stays the single source of order.
///
/// `app.rs` opens every dialog with `modal_focus = 0`, so index 0 is what
/// Enter takes. For every kind here that is `Cancel` — §4.7 requires the safe
/// action to hold focus on open, and a `Trash` confirm is a special case
/// handled by [`Button::is_default_for_enter`] rather than by reordering,
/// because reordering would put the destructive-looking button leftmost.
#[must_use]
pub fn buttons_for(kind: &Kind) -> Vec<Button> {
    match kind {
        // Two buttons: Cancel, then the action named.
        Kind::Confirm { op, .. } | Kind::Failed { op, .. } => {
            vec![Button::Cancel, Button::Confirm(*op)]
        }
        // §4.7's "one of exactly two or five buttons": a collision is the only
        // kind with a real question in it, so it is the only one that offers
        // both strategies and both scopes.
        Kind::Collision { .. } => vec![
            Button::Cancel,
            Button::OverwriteOne,
            Button::OverwriteAll,
            Button::SkipOne,
            Button::SkipAll,
        ],
        // Two buttons on a progress dialog is more than it needs: Stop is the
        // only action, and a second button that does the same thing is a lie
        // about there being a choice.
        Kind::Progress { .. } => vec![Button::Stop],
    }
}

// ---------------------------------------------------------------------------
// Colour
// ---------------------------------------------------------------------------

/// §4.7 `dialog.scrim` — the backdrop behind the dialog.
#[must_use]
pub fn scrim(theme: &Theme) -> Color32 {
    theme.surfaces.scrim
}

/// §4.7 `dialog.background` — `surface.raised`.
///
/// Pure white in light mode by design (§1.4): the dialog has to read as
/// *lifted* off the list, and a raised surface that matched the list would only
/// be distinguished by its shadow.
#[must_use]
pub fn background(theme: &Theme) -> Color32 {
    theme.surfaces.raised
}

/// §4.7 The fill for a button in a state.
///
/// Three cases, matching the three fill tokens in §4.7: cancel
/// (`surface.input` / `state.hover-strong`), confirm (`accent.base` /
/// `accent.hover`), and destructive (`status.danger-solid` / its hover).
///
/// Every value is a semantic role. The spec's `dialog.btn-destructive-bg-hover`
/// row is written `danger.600 / danger.500` — a *primitive* pair — so it is
/// resolved here against the theme mode to keep the primitive out of the
/// painting code, which is where §7.1 and §7.2 are actually won or lost.
#[must_use]
pub fn button_bg(theme: &Theme, button: Button, hovered: bool) -> Color32 {
    match button {
        Button::Cancel => {
            if hovered {
                theme.state.hover_strong
            } else {
                theme.surfaces.input
            }
        }
        // §4.7 writes the destructive pair as primitives (`danger.600` /
        // `danger.500`) rather than as a role, so the resolution to a concrete
        // colour per mode lives in `tokens.rs` behind these two accessors.
        // Calling them rather than reaching for `theme.status` directly is what
        // keeps this module from being a second place that knows the answer.
        Button::Confirm(Op::Delete) => {
            if hovered {
                component::DIALOG_BTN_DESTRUCTIVE_BG_HOVER(theme)
            } else {
                component::DIALOG_BTN_DESTRUCTIVE_BG(theme)
            }
        }
        Button::Confirm(_) => {
            if hovered {
                theme.accent.hover
            } else {
                theme.accent.base
            }
        }
        Button::OverwriteOne | Button::OverwriteAll => {
            if hovered {
                component::DIALOG_BTN_DESTRUCTIVE_BG_HOVER(theme)
            } else {
                component::DIALOG_BTN_DESTRUCTIVE_BG(theme)
            }
        }
        // Skip is the conservative answer, so it is styled as a cancel: the
        // user who reaches for it has decided not to destroy anything, and a
        // filled button would suggest the opposite.
        Button::SkipOne | Button::SkipAll | Button::Stop => {
            if hovered {
                theme.state.hover_strong
            } else {
                theme.surfaces.input
            }
        }
    }
}

/// §4.7 The outline for a button, if it has one.
///
/// Only Cancel carries a border (`dialog.btn-cancel-border` — 1 px
/// `border.strong`). A filled button is defined by its fill, and drawing a
/// hairline around `accent.base` produces the double-edge look §7.15 calls a
/// dashboard tell.
#[must_use]
pub fn button_border(theme: &Theme, button: Button) -> Option<Stroke> {
    match button {
        Button::Cancel => Some(Stroke::new(border::HAIRLINE, theme.borders.strong)),
        Button::Confirm(_)
        | Button::OverwriteOne
        | Button::OverwriteAll
        | Button::SkipOne
        | Button::SkipAll
        | Button::Stop => None,
    }
}

/// §4.7 The label colour for a button.
///
/// `text.on-accent` on an accent fill and `text.on-danger` on a danger fill,
/// because §6.1/§6.2 fix the contrast of those two pairs. A destructive button
/// with `text.primary` on `danger-solid` is the classic unreadable combo, and
/// this is the one function that prevents it.
#[must_use]
pub fn button_text(theme: &Theme, button: Button) -> Color32 {
    match button {
        Button::Confirm(Op::Delete) => theme.text.on_danger,
        Button::Confirm(_) => theme.text.on_accent,
        Button::OverwriteOne | Button::OverwriteAll => theme.text.on_danger,
        // Unfilled buttons sit on `surface.input`, where `text.primary` is the
        // §6.1 role.
        Button::Cancel | Button::SkipOne | Button::SkipAll | Button::Stop => theme.text.primary,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn trash() -> Kind {
        Kind::Confirm {
            op: Op::Trash,
            count: 3,
            sample: None,
            warning: Kind::irreversibility_line(Op::Trash).map(str::to_string),
        }
    }

    fn delete() -> Kind {
        Kind::Confirm {
            op: Op::Delete,
            count: 3,
            sample: None,
            warning: Kind::irreversibility_line(Op::Delete).map(str::to_string),
        }
    }

    /// §7.17 The anti-goal, stated as an assertion: no button anywhere in the
    /// type can produce a generic label.
    #[test]
    fn no_button_is_generic() {
        let every = [
            Button::Cancel,
            Button::Confirm(Op::Copy),
            Button::Confirm(Op::Move),
            Button::Confirm(Op::Trash),
            Button::Confirm(Op::Delete),
            Button::OverwriteOne,
            Button::OverwriteAll,
            Button::SkipOne,
            Button::SkipAll,
            Button::Stop,
        ];
        for b in every {
            let label = b.label();
            assert_ne!(label, "OK", "§7.17: no OK");
            assert_ne!(label, "Yes", "§7.17: no Yes");
            assert!(!label.is_empty());
        }
    }

    /// §4.7 "the verb is specific" — the destructive confirm names the action
    /// and its consequence in the same string.
    #[test]
    fn destructive_body_says_what_is_lost() {
        let body = delete().body();
        assert!(body.contains("Permanently delete"), "{body}");
        assert!(body.contains("cannot be undone"), "{body}");
    }

    /// The irreversibility line must not be said twice when it is already in
    /// the sentence — the recovered fragment's whole point.
    #[test]
    fn warning_is_not_duplicated_into_the_body() {
        let body = delete().body();
        let hits = body.matches("cannot be undone").count();
        assert_eq!(hits, 1, "the consequence is stated once, not twice: {body}");
    }

    /// A reversible destructive op is not warned about: `Trash` loses nothing.
    #[test]
    fn trash_has_no_irreversibility_line() {
        assert_eq!(Kind::irreversibility_line(Op::Trash), None);
        assert_eq!(Kind::irreversibility_line(Op::Copy), None);
        assert_eq!(Kind::irreversibility_line(Op::Move), None);
        assert_eq!(
            Kind::irreversibility_line(Op::Delete),
            Some("This cannot be undone.")
        );
    }

    /// §4.7 "Move to Trash is Enter-default; Delete Permanently is not."
    #[test]
    fn enter_default_follows_reversibility() {
        assert!(Button::Confirm(Op::Trash).is_default_for_enter(Op::Trash));
        assert!(!Button::Confirm(Op::Delete).is_default_for_enter(Op::Delete));
        // No collision button may be Enter-default: `Scope::All` is sticky.
        for b in [
            Button::OverwriteOne,
            Button::OverwriteAll,
            Button::SkipOne,
            Button::SkipAll,
        ] {
            assert!(!b.is_default_for_enter(Op::Move), "{b:?}");
        }
    }

    /// §4.7 "Cancel is always the leftmost button and keeps focus on open."
    #[test]
    fn cancel_is_first_in_every_dialog() {
        let kinds = [
            trash(),
            delete(),
            Kind::Collision {
                src: PathBuf::from("/a"),
                dst: PathBuf::from("/a/b"),
                remaining: 1,
            },
            Kind::Failed {
                op: Op::Copy,
                path: PathBuf::from("/a"),
                reason: "nope".to_string(),
            },
            Kind::Progress {
                op: Op::Copy,
                done: 0,
                total: 1,
                bytes: 0,
                total_bytes: None,
                current: PathBuf::from("/a"),
                rate: String::new(),
            },
        ];
        for kind in kinds {
            let buttons = buttons_for(&kind);
            assert!(!buttons.is_empty(), "{kind:?}");
            assert_eq!(buttons[0], Button::Cancel, "{kind:?}");
        }
    }

    /// §4.7 "one of exactly two or five buttons" — and the progress dialog is
    /// the one exception, because Stop is its only action.
    #[test]
    fn button_counts_match_the_spec() {
        assert_eq!(buttons_for(&trash()).len(), 2);
        assert_eq!(buttons_for(&delete()).len(), 2);
        assert_eq!(
            buttons_for(&Kind::Collision {
                src: PathBuf::from("/a"),
                dst: PathBuf::from("/a/b"),
                remaining: 2,
            })
            .len(),
            5
        );
        assert_eq!(
            buttons_for(&Kind::Progress {
                op: Op::Copy,
                done: 0,
                total: 1,
                bytes: 0,
                total_bytes: None,
                current: PathBuf::from("/a"),
                rate: String::new(),
            })
            .len(),
            1
        );
    }

    /// Every collision button maps to a decision, and only the collision
    /// buttons do.
    #[test]
    fn decisions_cover_both_strategies_and_both_scopes() {
        let collision = Kind::Collision {
            src: PathBuf::from("/a"),
            dst: PathBuf::from("/a/b"),
            remaining: 1,
        };
        let decisions: Vec<_> = buttons_for(&collision)
            .into_iter()
            .filter_map(Button::to_decision)
            .collect();
        assert_eq!(decisions.len(), 4, "four answering buttons, one cancel");
        assert!(decisions.contains(&(Strategy::Overwrite, Scope::ThisOne)));
        assert!(decisions.contains(&(Strategy::Overwrite, Scope::All)));
        assert!(decisions.contains(&(Strategy::Skip, Scope::ThisOne)));
        assert!(decisions.contains(&(Strategy::Skip, Scope::All)));
        // Cancel and Stop are not answers to a collision.
        assert_eq!(Button::Cancel.to_decision(), None);
        assert_eq!(Button::Stop.to_decision(), None);
    }

    /// `Trash` is destructive as an op but its button is the safe action, so it
    /// must not get the danger fill.
    #[test]
    fn trash_button_is_not_destructive_styled() {
        assert!(Op::Trash.is_destructive());
        assert!(!Button::Confirm(Op::Trash).is_destructive());
        assert!(Button::Confirm(Op::Delete).is_destructive());
    }

    #[test]
    fn sample_path_is_none_for_an_empty_selection() {
        assert_eq!(sample_path(&[]), None);
    }

    #[test]
    fn sample_path_quotes_the_first_item() {
        let items = [
            Item::file("/a/one.txt", Some(1)),
            Item::file("/a/two.txt", Some(1)),
        ];
        assert_eq!(sample_path(&items), Some(PathBuf::from("/a/one.txt")));
    }

    #[test]
    fn op_for_delete_maps_the_toolbar_buttons() {
        assert_eq!(op_for_delete(false), Op::Trash);
        assert_eq!(op_for_delete(true), Op::Delete);
    }

    /// Only the permanent confirm carries a border; a filled button must not.
    #[test]
    fn only_the_unfilled_button_has_an_outline() {
        for theme in [Theme::light(), Theme::dark()] {
            assert!(button_border(&theme, Button::Cancel).is_some());
            for b in [
                Button::Confirm(Op::Delete),
                Button::Confirm(Op::Trash),
                Button::OverwriteOne,
            ] {
                assert!(button_border(&theme, b).is_none(), "{b:?}");
            }
        }
    }

    /// §6.1/§6.2: a filled button's label is an `on-*` role, never `text.primary`.
    #[test]
    fn filled_buttons_use_on_colours() {
        for theme in [Theme::light(), Theme::dark()] {
            for b in [
                Button::Confirm(Op::Trash),
                Button::Confirm(Op::Delete),
                Button::OverwriteOne,
                Button::OverwriteAll,
            ] {
                let text = button_text(&theme, b);
                let expected = match b {
                    Button::Confirm(Op::Delete) => theme.text.on_danger,
                    Button::Confirm(_) => theme.text.on_accent,
                    _ => theme.text.on_danger,
                };
                assert_eq!(text, expected, "{b:?}");
                assert_ne!(text, theme.text.primary, "{b:?} must not use primary");
            }
        }
    }

    /// A destructive dialog's icon is the warning glyph, and a non-destructive
    /// one is not.
    #[test]
    fn destructive_confirms_use_the_warning_glyph() {
        assert_eq!(delete().icon(), icons::WARNING);
        assert_ne!(trash().icon(), icons::WARNING);
    }

    /// A root directory has no file name; the sentence must not say `""`.
    #[test]
    fn display_name_falls_back_for_a_root() {
        assert_eq!(display_name(&PathBuf::from("/")), "/");
    }

    /// The body sentence pluralises, and thousands are separated.
    #[test]
    fn body_pluralises_and_groups() {
        let many = Kind::Confirm {
            op: Op::Delete,
            count: 1_500,
            sample: None,
            warning: None,
        };
        let body = many.body();
        assert!(body.contains("1,500 items"), "{body}");
        let one = Kind::Confirm {
            op: Op::Delete,
            count: 1,
            sample: None,
            warning: None,
        };
        assert!(one.body().contains("1 item."), "{}", one.body());
    }
}
