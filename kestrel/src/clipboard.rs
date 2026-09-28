//! Clipboard *intent* — the cut/copy pending-op set.
//!
//! # This is not an OS clipboard
//!
//! It deliberately does not touch `wl-clipboard` or `xclip`. A file manager's
//! clipboard is a *promise* ("these 3 items are staged for a move"), and that
//! promise has to survive the source window being closed and reopened. The OS
//! clipboard is a byte buffer; Phase 4's job is to bridge the two — publish a
//! `text/uri-list` so other apps can paste — but the authoritative state has to
//! live here, in the app, or the promise evaporates.
//!
//! # `state.cut` was a dead token until now
//!
//! §4.2 specifies a `cut` row state with three distinct marks: the 55% tint, the
//! bar at 55%, and the **icon at 70%**. §4.2 rule 6 forbids strikethrough,
//! because "strikethrough on a filename reads as 'deleted' and causes
//! mistakes". The token existed in `tokens.rs` and nothing consumed it; this
//! module is what consumes it.

use std::collections::BTreeSet;
use std::path::PathBuf;

/// What a staged selection will do when pasted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Op {
    /// Staged to be copied — the rows keep their normal appearance.
    #[default]
    Copy,
    /// Staged to be *moved*. This is the only op with a distinct visual state.
    Cut,
}

impl Op {
    /// `true` for the one op that changes how a row looks.
    #[must_use]
    pub fn is_cut(self) -> bool {
        matches!(self, Op::Cut)
    }
}

/// The app's pending clipboard: a set of paths and what will happen to them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Clipboard {
    paths: BTreeSet<PathBuf>,
    op: Op,
}

impl Clipboard {
    /// An empty clipboard staging a copy.
    #[must_use]
    pub fn new() -> Self {
        Self {
            paths: BTreeSet::new(),
            // `Op::Copy` as the zero value says nothing about an empty
            // clipboard, and is never observed: every accessor that reads `op`
            // checks `is_empty` first.
            op: Op::default(),
        }
    }

    /// Stages `paths` for `op`, replacing whatever was staged.
    pub fn stage<I, P>(&mut self, paths: I, op: Op) -> usize
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        self.paths.clear();
        self.paths.extend(paths.into_iter().map(Into::into));
        self.op = op;
        self.len()
    }

    /// Empties the clipboard. §4.11 `Escape` cancels a pending op.
    pub fn clear(&mut self) {
        self.paths.clear();
    }

    /// `true` when nothing is staged.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// How many paths are staged.
    #[must_use]
    pub fn len(&self) -> usize {
        self.paths.len()
    }

    /// The staged op. Meaningless when empty; see [`Clipboard::is_empty`].
    #[must_use]
    pub fn op(&self) -> Op {
        self.op
    }

    /// The staged paths, in sorted order.
    ///
    /// Order is the `BTreeSet`'s, so a paste is deterministic. A file manager
    /// whose paste order depended on hash iteration would be untestable and would
    /// move files in a different order on every run.
    #[must_use]
    pub fn paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.paths.iter()
    }

    /// What will happen to `path`, if anything.
    ///
    /// A row is "cut" only while it is *both* staged and the op is `Cut` — a
    /// copy-staged row looks completely normal, which is what makes cut and copy
    /// distinguishable at a glance.
    #[must_use]
    pub fn op_for(&self, path: &std::path::Path) -> Option<Op> {
        if self.paths.contains(path) {
            Some(self.op)
        } else {
            None
        }
    }

    /// The status-bar phrase for a pending op, or `None` when there is none.
    ///
    /// §7.16: "A long operation always reports itself in the status bar." A
    /// staged cut that the user cannot see in the status bar is a staged cut
    /// they will forget about.
    #[must_use]
    pub fn status_phrase(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let verb = match self.op {
            Op::Cut => "Cut",
            Op::Copy => "Copied",
        };
        let noun = if self.len() == 1 { "item" } else { "items" };
        Some(format!("{verb} {} {noun}", self.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn an_empty_clipboard_stages_nothing() {
        let c = Clipboard::new();
        assert!(c.is_empty());
        assert_eq!(c.len(), 0);
        assert_eq!(c.op_for(Path::new("/a")), None);
        assert_eq!(c.status_phrase(), None);
    }

    #[test]
    fn cut_stages_the_cut_op_for_its_paths_only() {
        let mut c = Clipboard::new();
        c.stage([p("/a"), p("/b")], Op::Cut);
        assert_eq!(c.len(), 2);
        assert_eq!(c.op_for(Path::new("/a")), Some(Op::Cut));
        assert_eq!(c.op_for(Path::new("/b")), Some(Op::Cut));
        assert_eq!(c.op_for(Path::new("/c")), None, "unstaged rows are normal");
        assert!(c.op_for(Path::new("/a")).is_some_and(Op::is_cut));
    }

    #[test]
    fn copy_does_not_mark_its_rows_as_cut() {
        // The whole point of the `cut` state: it must be *distinguishable* from
        // copy, or staging a copy would look like staging a move.
        let mut c = Clipboard::new();
        c.stage([p("/a")], Op::Copy);
        assert_eq!(c.op_for(Path::new("/a")), Some(Op::Copy));
        assert!(
            !c.op_for(Path::new("/a")).is_some_and(Op::is_cut),
            "a copied row must not render the cut treatment"
        );
    }

    #[test]
    fn staging_replaces_rather_than_accumulating() {
        let mut c = Clipboard::new();
        c.stage([p("/a"), p("/b")], Op::Cut);
        c.stage([p("/c")], Op::Copy);
        assert_eq!(c.len(), 1);
        assert_eq!(c.op_for(Path::new("/a")), None);
        assert_eq!(c.op_for(Path::new("/c")), Some(Op::Copy));
    }

    #[test]
    fn duplicates_collapse() {
        let mut c = Clipboard::new();
        c.stage([p("/a"), p("/a"), p("/a")], Op::Cut);
        assert_eq!(c.len(), 1, "the same path staged three times is one path");
    }

    #[test]
    fn clearing_empties_the_stage_and_the_phrase() {
        let mut c = Clipboard::new();
        c.stage([p("/a"), p("/b"), p("/c")], Op::Cut);
        assert_eq!(c.status_phrase().as_deref(), Some("Cut 3 items"));
        c.clear();
        assert!(c.is_empty());
        assert_eq!(c.status_phrase(), None);
    }

    #[test]
    fn the_status_phrase_agrees_in_number_with_the_stage() {
        let mut c = Clipboard::new();
        c.stage([p("/only")], Op::Cut);
        assert_eq!(c.status_phrase().as_deref(), Some("Cut 1 item"));
        c.stage([p("/a"), p("/b")], Op::Copy);
        assert_eq!(c.status_phrase().as_deref(), Some("Copied 2 items"));
    }
}
