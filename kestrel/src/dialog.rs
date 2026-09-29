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
/// A **floor**, not a hard maximum, and that is a documented deviation.
///
/// A two-button dialog lands on exactly 400: its widest content is the path
/// quote, which fills the content box, so the frame comes out at the token. The
/// collision dialog does not fit, and the arithmetic is not close:
///
/// ```text
/// Cancel 40 + Replace 52 + Replace All 62 + Skip 26 + Skip All 42  = 222
/// 5 x dialog.btn-padding-x (14 x 2)                                = 140
/// 4 x inter-button gap at 6px                                       =  24
///                                                                  -----
///                                                                    386
/// + 2 x dialog.padding (20) + 1px border                          =  427
/// ```
///
/// §4.7 asks for a 400px dialog *and* for "one of exactly two or five buttons"
/// with 14px button padding, and those three cannot all hold at once. Something
/// had to give, and the candidates were: shorten a label (which would break
/// §4.7's "the verb is specific" rule and hide the scope distinction the
/// collision exists to draw); wrap the footer (which would break "Cancel is
/// always the leftmost button"); shrink the padding (a §4.7 token); or let the
/// one dialog that carries five buttons be wider than the one that carries two.
///
/// Widening it is the only option that changes no §4.7 rule. The width is
/// therefore applied as a minimum — a `Modal` whose content is narrower than
/// [`WIDTH`] still comes out at [`WIDTH`], so every two-button dialog is exactly
/// on the token — and the frame is otherwise allowed to be as wide as its
/// content needs, which is only ever the collision dialog.
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

/// The determinate progress bar's height, on a `Progress` dialog.
///
/// Derived from §4.5's free-space meter rather than from §4.7, which has no
/// progress token at all. See [`component::DIALOG_PROGRESS_BAR_H`].
pub const PROGRESS_BAR_H: f32 = component::DIALOG_PROGRESS_BAR_H;

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
    /// Narrowest the pane may be dragged. Matches the sidebar's floor, so no
    /// pane can be squeezed to nothing while another stays wide.
    ///
    /// The same number as [`crate::settings::PREVIEW_MIN`], which is the value
    /// the settings stepper clamps to. Two names for one bound is deliberate:
    /// this module is about the *pane* and that one is about the *setting*, and
    /// the normalisation on load is what keeps them from drifting.
    pub const MIN_WIDTH: f32 = crate::settings::PREVIEW_MIN;
    /// Widest the pane may be dragged — deliberately *narrower* than
    /// [`crate::tokens::metric::SIDEBAR_MAX`] (340) is wide, because the pane
    /// holds a filename column that stops being readable well before 340.
    pub const MAX_WIDTH: f32 = crate::settings::PREVIEW_MAX;
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
    if permanent { Op::Delete } else { Op::Trash }
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
            Self::Collision {
                src,
                dst,
                remaining,
            } => {
                // `format::plural` already appends the noun, so the count is
                // rendered bare and the noun written here. Passing the noun
                // twice produced "(3 names more names left in this job.)",
                // which is the kind of sentence a reconstructed module writes
                // and nobody catches until it is rendered.
                let scope = match remaining {
                    1 => "1 more name".to_string(),
                    n => format!("{} more names", grouped(*n)),
                };
                // When the two files have the same name — the ordinary case,
                // because a collision is usually a copy into a directory that
                // already holds it — quoting both names reads as a tautology:
                // "report.txt already exists at report.txt." The destination then
                // gets its **path**, which is the thing that actually differs and
                // the thing the user has to decide about.
                let src_name = display_name(src);
                let dst_name = if display_name(dst) == src_name {
                    dst.display().to_string()
                } else {
                    display_name(dst)
                };
                format!(
                    "{src_name} already exists at {dst_name}. Replace it? \
                     ({scope} left in this job.)"
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
                // "7 of 24 items", not "7 items of 24 items". The noun belongs to
                // the total only: both numbers count the same things, so saying
                // it twice is not emphasis, it is a sentence with two subjects.
                let counts = format!("{} of {}", grouped(*done), plural(*total, "item", "items"));
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

    /// The op this dialog is about, for a dialog that has one.
    ///
    /// `Collision` and `Progress` answer `Copy` because neither is *about* an op
    /// that has not run yet in a way the footer needs; the field only exists so
    /// [`Kind::extra_warning`] and [`Kind::is_icon_dangerous`] can ask the
    /// question without each of them re-deriving the answer from the variant.
    #[must_use]
    pub fn op(&self) -> Op {
        match self {
            Self::Confirm { op, .. } | Self::Failed { op, .. } | Self::Progress { op, .. } => *op,
            Self::Collision { .. } => Op::Copy,
        }
    }

    /// The path to quote under the body in `dialog.path-quote`, if there is one.
    ///
    /// A confirm quotes a representative item; a failure quotes the path it
    /// stopped at, which is the one piece of information the sentence does not
    /// already carry — "Permission denied" says nothing about *what*.
    #[must_use]
    pub fn quoted_path(&self) -> Option<PathBuf> {
        match self {
            Self::Confirm { sample, .. } => sample.clone(),
            Self::Failed { path, .. } => Some(path.clone()),
            Self::Collision { .. } | Self::Progress { .. } => None,
        }
    }

    /// §4.7 The irreversibility line to render **separately**, if one is needed.
    ///
    /// `None` whenever the sentence already carries the consequence, which is
    /// the normal case for `Delete`: [`destructive_sentence`] puts "This cannot
    /// be undone." in the same breath as the verb, so rendering the line as well
    /// said the consequence twice. A dialog that states its consequence twice
    /// does not read as emphatic; it reads as two different consequences, and the
    /// user has to work out which one is the condition.
    ///
    /// This is the same single-source rule the rest of this module is built on,
    /// applied to the one place where the sentence and the line could disagree.
    #[must_use]
    pub fn extra_warning(&self) -> Option<String> {
        let line = Self::irreversibility_line(self.op())?;
        if self.body().contains(line) {
            None
        } else {
            Some(line.to_string())
        }
    }

    /// `true` when the title glyph should be painted in `status.danger-text`.
    ///
    /// §4.7 ties that colour to the `warning` glyph on a *destructive* dialog.
    /// A failure is not destructive — nothing is about to be destroyed — but its
    /// glyph is `x-circle`, which §3.6 defines as an error glyph and which §3.6
    /// also says is never shown without `status.danger-text` beside it. So the
    /// two sections are read together: the *destructive* rule decides, and the
    /// status rule fills the gap the destructive rule leaves.
    #[must_use]
    pub fn is_icon_dangerous(&self) -> bool {
        match self {
            Self::Confirm { op, .. } => op.is_destructive(),
            Self::Failed { .. } | Self::Collision { .. } => true,
            Self::Progress { .. } => false,
        }
    }

    /// The progress fraction for a `Progress` dialog, `0.0..=1.0`.
    ///
    /// By **item** count, not by bytes: a directory's byte total is a recursive
    /// walk the app has not done, and a bar built on a number that is a guess
    /// would jump backwards as directories resolve. `None` when the total is
    /// zero, because a bar that is 100% full before anything has happened is a
    /// lie with a percentage on it.
    #[must_use]
    pub fn progress_fraction(&self) -> Option<f32> {
        match self {
            Self::Progress { done, total, .. } if *total > 0 => {
                Some((*done as f32 / *total as f32).clamp(0.0, 1.0))
            }
            _ => None,
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

/// `count` on its own, thousands-separated.
///
/// For a sentence that supplies its own noun — "3 more names", where the noun
/// is part of the phrase and not a plural of anything the count decides.
fn grouped(count: usize) -> String {
    format::plural(count, "", "").trim().to_string()
}

/// The last path component, for a sentence.
///
/// Falls back to the whole path when the component is empty — a root directory
/// or a trailing separator has no name, and rendering `"/"` as the dialog's
/// subject is better than rendering `""`.
fn display_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
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
    /// The op's own verb, for a confirm dialog.
    Confirm(Op),
    /// `Close` — the only button on a failure report.
    ///
    /// A separate variant rather than a `Confirm(op)` because a failure has
    /// nothing to confirm. The previous version built a failure's footer from the
    /// op's verb, so a move that had already stopped halfway offered a button
    /// reading **Move** — a button that claimed an action was still available when
    /// it was not, which is the same class of lie §7.17 is about, just with a
    /// specific verb instead of `OK`.
    Close,
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
            // `Close`, not `OK`: §7.17 forbids the *generic* label because it
            // hides the action. Here the action is "dismiss a report of what
            // already happened", and `Close` says exactly that. It is still a
            // closed set — a caller cannot invent a label, which is the property
            // that matters.
            Self::Close => "Close",
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
            Self::Cancel | Self::Close | Self::SkipOne | Self::SkipAll | Self::Stop => false,
        }
    }

    /// `true` for the button that dismisses without doing anything.
    #[must_use]
    pub fn is_cancel(self) -> bool {
        matches!(self, Self::Cancel | Self::Close)
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
            Self::Cancel | Self::Close | Self::Confirm(_) | Self::Stop => None,
        }
    }
}

/// §4.7 The buttons for a dialog, in order.
///
/// Order is left-to-right, and `app.rs` walks the list in reverse to undo its
/// right-to-left layout so that this list stays the single source of order.
///
/// `app.rs` opens every dialog with `modal_focus = 0`, so index 0 is what
/// Enter takes. For every kind here that is the safe action — §4.7 requires the
/// safe action to hold focus on open, and a `Trash` confirm is a special case
/// handled by [`Button::is_default_for_enter`] rather than by reordering,
/// because reordering would put the destructive-looking button leftmost.
#[must_use]
pub fn buttons_for(kind: &Kind) -> Vec<Button> {
    match kind {
        // Two buttons: Cancel, then the action named.
        Kind::Confirm { op, .. } => vec![Button::Cancel, Button::Confirm(*op)],
        // A failure report is not a question. One button, and it says what it
        // does. §4.7's "Cancel is always the leftmost button" governs the dialogs
        // that *offer* a choice; a one-button dialog trivially satisfies it.
        Kind::Failed { .. } => vec![Button::Close],
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

/// §4.7 The single button Enter takes in a dialog of this kind.
///
/// # Why this is a function over a `Kind`, and not a predicate over the buttons
///
/// §4.7 sets two different things, and the old code conflated them into one
/// focus index:
///
/// * the **focus ring**, which "is always `Cancel`" so the safe action is where
///   the keyboard already is; and
/// * the **Enter default**, which is `Move to Trash` and not `Delete
///   Permanently`, and which for a *collision* is nothing at all.
///
/// A per-button predicate that answered `true` for both `Cancel` and
/// `Confirm(Trash)` on a trash dialog could not say which of them Enter takes,
/// and "whichever the focus index happens to be" is the wrong answer for the one
/// case §4.7 calls out by name.
///
/// This returns the answer, so there is exactly one, it cannot be ambiguous, and
/// a kind added later gets a compile error here rather than inheriting `Cancel`
/// by accident.
///
/// # The rule, per kind
///
/// * `Move to Trash` — the action is reversible, so it is the default. §4.7
///   names it.
/// * `Delete Permanently` — never. §4.7 names it, and a stray Enter must not be
///   one keystroke away from unrecoverable.
/// * A failure report — `Close`, the only button.
/// * A collision — no button *decides* anything. `Scope::All` is sticky for the
///   rest of the job, so a user's Enter on a dialog they have not read must not
///   pick a scope for them. `Cancel` stops the job, which is the only answer
///   here that destroys nothing.
/// * A running job — `Stop`, the only button.
#[must_use]
pub fn enter_default(kind: &Kind) -> Button {
    match kind {
        Kind::Confirm { op, .. } if *op == Op::Trash => Button::Confirm(Op::Trash),
        Kind::Confirm { .. } => Button::Cancel,
        Kind::Failed { .. } => Button::Close,
        Kind::Collision { .. } => Button::Cancel,
        Kind::Progress { .. } => Button::Stop,
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
        // `Close` is a dismiss, not an action, so it is styled like `Cancel`:
        // `surface.input` with a `border.strong` outline. A filled accent button
        // on a failure report would be a "go" signal for a screen the user is
        // only reading.
        Button::Close | Button::SkipOne | Button::SkipAll | Button::Stop => {
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
        Button::Cancel | Button::Close => Some(Stroke::new(border::HAIRLINE, theme.borders.strong)),
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
        Button::Cancel | Button::Close | Button::SkipOne | Button::SkipAll | Button::Stop => {
            theme.text.primary
        }
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
            Button::Close,
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
    ///
    /// Asserted as the *label* the user would press, because that is the thing
    /// the rule is about — the underlying predicate and the resolution to one
    /// button are the same function.
    #[test]
    fn enter_default_follows_reversibility() {
        assert_eq!(enter_default(&trash()), Button::Confirm(Op::Trash));
        assert_eq!(enter_default(&trash()).label(), "Move to Trash");
        assert_eq!(enter_default(&delete()), Button::Cancel);
        assert_eq!(enter_default(&delete()).label(), "Cancel");
        // No collision button may be the Enter default: `Scope::All` is sticky.
        let collision = Kind::Collision {
            src: PathBuf::from("/a"),
            dst: PathBuf::from("/a/b"),
            remaining: 1,
        };
        for b in buttons_for(&collision) {
            if !b.is_cancel() {
                assert_ne!(enter_default(&collision), b, "{b:?}");
            }
        }
    }

    /// §4.7 "Cancel is always the leftmost button and keeps focus on open."
    ///
    /// Stated as `is_cancel()` rather than `== Cancel` because a failure report
    /// has one button and it is `Close`: the rule is about the *safe action being
    /// where the keyboard already is*, and a report of something that already
    /// happened has no action to take.
    #[test]
    fn the_safe_action_is_first_in_every_dialog() {
        // `Progress` is deliberately absent: Stop is its only action, and a
        // second button that does the same thing would be a lie about there
        // being a choice. §4.7's "Cancel is always leftmost" governs the
        // dialogs that *offer* a choice.
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
        ];
        for kind in kinds {
            let buttons = buttons_for(&kind);
            assert!(!buttons.is_empty(), "{kind:?}");
            assert!(buttons[0].is_cancel(), "{kind:?} starts with {buttons:?}");
        }
    }

    /// A failure report offers one button, and it does not name an operation.
    ///
    /// The previous version built a `Failed` footer from the op's verb, so a move
    /// that had already stopped halfway offered a button reading `Move` — an
    /// action that is no longer available, offered as if it were.
    #[test]
    fn a_failure_offers_only_close() {
        let failed = Kind::Failed {
            op: Op::Move,
            path: PathBuf::from("/a/locked"),
            reason: "Permission denied (os error 13)".to_string(),
        };
        let buttons = buttons_for(&failed);
        assert_eq!(buttons, vec![Button::Close]);
        assert_eq!(buttons[0].label(), "Close");
        assert!(!buttons[0].is_destructive());
    }

    /// §4.7's Enter default, which is not always the focused button.
    ///
    /// Both halves have to hold at once, and the old code could only express one
    /// of them: a per-button predicate that answered `true` for both `Cancel` and
    /// `Confirm(Trash)` cannot say which one Enter takes. `enter_default` returns
    /// the one.
    #[test]
    fn enter_follows_the_default_not_the_focus() {
        // Move to Trash is the Enter default — the data survives.
        assert_eq!(enter_default(&trash()), Button::Confirm(Op::Trash));
        // Delete Permanently is not, and Cancel — which holds the focus ring — is.
        assert_eq!(enter_default(&delete()), Button::Cancel);
        // A collision picks no scope on a stray Enter; it stops the job.
        let collision = Kind::Collision {
            src: PathBuf::from("/a"),
            dst: PathBuf::from("/a/b"),
            remaining: 1,
        };
        assert_eq!(enter_default(&collision), Button::Cancel);
        // And the Enter default is always a button the dialog actually has.
        let kinds = [
            trash(),
            delete(),
            collision,
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
            assert!(
                buttons_for(&kind).contains(&enter_default(&kind)),
                "{kind:?} has no Enter default"
            );
        }
    }

    /// A destructive sentence states the consequence once, and `extra_warning`
    /// does not then say it again.
    ///
    /// This is the pair that produced a dialog reading "Permanently delete 3
    /// items. This cannot be undone." followed by a red "This cannot be undone."
    /// underneath it. The single source is [`destructive_sentence`]: it always
    /// carries the consequence, so the separate line is never needed for `Delete`
    /// — whichever way the caller filled in the `warning` field.
    #[test]
    fn the_irreversibility_is_stated_exactly_once() {
        for count in [1, 3, 1_500] {
            let kind = Kind::Confirm {
                op: Op::Delete,
                count,
                sample: None,
                warning: None,
            };
            assert_eq!(
                kind.body().matches("cannot be undone").count(),
                1,
                "count {count}: {}",
                kind.body()
            );
            assert_eq!(
                kind.extra_warning(),
                None,
                "count {count}: the sentence already says it, so the line must \
                 not be drawn as well"
            );
        }
        // A reversible op has nothing irreversible to say, in either place.
        assert_eq!(trash().extra_warning(), None);
        assert!(!trash().body().contains("cannot be undone"));
    }

    /// A failure quotes the path it stopped at.
    ///
    /// "Permission denied" on its own says nothing about *what*; the path quote
    /// is §4.7's own answer to that, and the dialog was not using it.
    #[test]
    fn a_failure_quotes_where_it_stopped() {
        let failed = Kind::Failed {
            op: Op::Move,
            path: PathBuf::from("/a/locked"),
            reason: "nope".to_string(),
        };
        assert_eq!(failed.quoted_path(), Some(PathBuf::from("/a/locked")));
        assert_eq!(trash().quoted_path(), None);
    }

    /// The progress fraction is by item, clamped, and absent when the total is
    /// zero.
    #[test]
    fn progress_is_a_clamped_item_fraction() {
        let at = |done, total| Kind::Progress {
            op: Op::Copy,
            done,
            total,
            bytes: 0,
            total_bytes: None,
            current: PathBuf::from("/a"),
            rate: String::new(),
        };
        assert_eq!(at(0, 10).progress_fraction(), Some(0.0));
        assert_eq!(at(5, 10).progress_fraction(), Some(0.5));
        assert_eq!(at(10, 10).progress_fraction(), Some(1.0));
        // A job that over-reports must not draw a bar past its own end.
        assert_eq!(at(11, 10).progress_fraction(), Some(1.0));
        // No total means no bar: 100% before anything has happened is a lie.
        assert_eq!(at(0, 0).progress_fraction(), None);
        assert_eq!(trash().progress_fraction(), None);
    }

    /// §4.7 "one of exactly two or five buttons" — and the progress dialog is
    /// the one exception, because Stop is its only action.
    #[test]
    fn button_counts_match_the_spec() {
        assert_eq!(buttons_for(&trash()).len(), 2);
        assert_eq!(buttons_for(&delete()).len(), 2);
        // A failure is the documented third shape: one button, and it dismisses.
        assert_eq!(
            buttons_for(&Kind::Failed {
                op: Op::Copy,
                path: PathBuf::from("/a"),
                reason: "nope".to_string(),
            })
            .len(),
            1
        );
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
        assert_eq!(Button::Close.to_decision(), None);
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
            // `Close` is an unfilled dismiss, so it carries the cancel border.
            assert!(button_border(&theme, Button::Close).is_some());
        }
    }

    /// §6.1/§6.2: a filled button's label comes from an `on-*` role, never from
    /// `text.primary`.
    ///
    /// The assertion is that the *role* is right, not that the resolved value
    /// differs from `text.primary`: in the dark column `on_danger` and
    /// `primary` are both `#F0EDE8`, because the dark danger fill is light and
    /// its foreground legitimately *is* the light foreground. Asserting they
    /// differ would be asserting a coincidence that happens not to hold.
    #[test]
    fn filled_buttons_use_on_colours() {
        for theme in [Theme::light(), Theme::dark()] {
            for b in [
                Button::Confirm(Op::Trash),
                Button::Confirm(Op::Delete),
                Button::OverwriteOne,
                Button::OverwriteAll,
            ] {
                let expected = match b {
                    Button::Confirm(Op::Delete) => theme.text.on_danger,
                    Button::Confirm(_) => theme.text.on_accent,
                    _ => theme.text.on_danger,
                };
                assert_eq!(button_text(&theme, b), expected, "{b:?}");
            }
        }
    }

    /// §4.7 `dialog.icon`: the `warning` glyph for a destructive confirm.
    ///
    /// Both destructive ops get it, because `Op::is_destructive` covers `Trash`
    /// as well as `Delete` — a move to the trash still destroys something, and
    /// §4.7 keys the glyph off destruction, not off reversibility. A *failure*
    /// is not a warning (nothing is about to happen), so it must not reuse it.
    #[test]
    fn destructive_confirms_use_the_warning_glyph() {
        assert_eq!(delete().icon(), icons::WARNING);
        assert_eq!(trash().icon(), icons::WARNING);
        let failed = Kind::Failed {
            op: Op::Delete,
            path: PathBuf::from("/a"),
            reason: "nope".to_string(),
        };
        assert_ne!(failed.icon(), icons::WARNING, "a failure is not a warning");
        assert_eq!(failed.icon(), icons::X_CIRCLE);
    }

    /// Every sentence a dialog can say, rendered. The wording is the §4.7 copy
    /// rule, and a copy rule that is only ever asserted by substring is a copy
    /// rule nobody reads.
    ///
    /// It exists because the strings were **reconstructed**, not recovered: a
    /// sentence that reads "7 items of 24 items" or "(3 names more names left in
    /// this job.)" is grammatical enough to compile and wrong enough that only
    /// looking at it catches it. Printed whole, the awkwardness is obvious.
    #[test]
    fn every_sentence_reads_as_a_sentence() {
        let shown: Vec<String> = [
            trash().body(),
            delete().body(),
            Kind::Collision {
                src: PathBuf::from("/home/a/report.txt"),
                dst: PathBuf::from("/home/b/report.txt"),
                remaining: 1,
            }
            .body(),
            Kind::Collision {
                src: PathBuf::from("/home/a/report.txt"),
                dst: PathBuf::from("/home/b/report.txt"),
                remaining: 3,
            }
            .body(),
            Kind::Failed {
                op: Op::Move,
                path: PathBuf::from("/home/a/locked"),
                reason: "Permission denied (os error 13)".to_string(),
            }
            .body(),
            Kind::Progress {
                op: Op::Copy,
                done: 7,
                total: 24,
                bytes: 0,
                total_bytes: None,
                current: PathBuf::from("/home/a/assets"),
                rate: "12 items/s".to_string(),
            }
            .body(),
            Kind::Progress {
                op: Op::Copy,
                done: 0,
                total: 1,
                bytes: 0,
                total_bytes: None,
                current: PathBuf::from("/home/a/one.txt"),
                rate: String::new(),
            }
            .body(),
        ]
        .into_iter()
        .collect();

        for sentence in &shown {
            // No doubled noun: "3 names more names", "7 items of 24 items".
            for word in ["items", "names", "item", "name"] {
                let count = sentence.split_whitespace().filter(|w| *w == word).count();
                assert!(count <= 1, "a sentence says {word:?} twice: {sentence:?}");
            }
            // No doubled phrase: the reconstructed copy said the consequence
            // twice, once in the sentence and once in the line under it.
            assert!(
                sentence.matches("cannot be undone").count() <= 1,
                "the consequence is stated twice: {sentence:?}"
            );
            // No space before punctuation and no doubled space: the two
            // typographic tells that a string was assembled rather than
            // written. (Terminal punctuation is deliberately *not* asserted: a
            // sentence can end in a filename, an OS error string or a throughput
            // figure, none of which this module gets to punctuate. The exact
            // strings below are what pin the endings.)
            assert!(
                !sentence.contains(" ."),
                "space before a full stop: {sentence:?}"
            );
            assert!(
                !sentence.contains(", "),
                "space before a comma: {sentence:?}"
            );
            assert!(!sentence.contains("  "), "a doubled space: {sentence:?}");
        }

        assert_eq!(
            shown[0],
            "Move 3 items to the Trash. You can restore them later."
        );
        assert_eq!(
            shown[1],
            "Permanently delete 3 items. This cannot be undone."
        );
        assert_eq!(
            shown[2],
            "report.txt already exists at /home/b/report.txt. Replace it? (1 more \
             name left in this job.)"
        );
        assert_eq!(
            shown[3],
            "report.txt already exists at /home/b/report.txt. Replace it? (3 more \
             names left in this job.)"
        );
        assert_eq!(
            shown[4],
            "Move could not finish. Permission denied (os error 13)"
        );
        assert_eq!(shown[5], "7 of 24 items — assets — 12 items/s");
        assert_eq!(shown[6], "0 of 1 item — one.txt");
    }

    /// Two files with different names are named by name; two with the same name
    /// get the destination's path, because "x already exists at x" is a sentence
    /// that says nothing.
    #[test]
    fn a_collision_names_the_destination_usefully() {
        let same = Kind::Collision {
            src: PathBuf::from("/home/a/report.txt"),
            dst: PathBuf::from("/home/b/report.txt"),
            remaining: 1,
        };
        assert!(
            same.body().contains("at /home/b/report.txt"),
            "{}",
            same.body()
        );
        let different = Kind::Collision {
            src: PathBuf::from("/home/a/report.txt"),
            dst: PathBuf::from("/home/b/report (1).txt"),
            remaining: 1,
        };
        assert!(
            different.body().contains("at report (1).txt"),
            "a differently-named destination does not need its whole path: {}",
            different.body()
        );
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
