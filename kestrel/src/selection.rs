//! Multi-select: anchor-based range selection over row indices.
//!
//! # What was wrong in Phase 2
//!
//! Shift-click extended from `self.focus`, which is the row the user *last
//! clicked* — not the row the range *started* from. Click row 10, arrow to 20,
//! then shift-click 15 gave you 15..20 instead of 10..15. The difference is
//! invisible until the user does it twice, and then it is maddening.
//!
//! §4.2's "Selection model" is explicit that selection and focus are distinct,
//! and this module is the proof of it: a selection is a *set*, a focus is a
//! *cursor*, and the anchor is what connects them.
//!
//! # Why it is a separate module
//!
//! Every rule here is about set algebra over indices, with no egui, no paths and
//! no engine types. That makes the behaviour testable without a window, which is
//! the only practical way to test "shift-click 5, shift-click 12, ctrl-click 3"
//! honestly.

use std::collections::BTreeSet;

/// The user's selection over row indices.
///
/// A `BTreeSet` rather than a `HashSet` for two reasons that both matter here:
/// iteration is in row order, so "select all" and "clear" produce a stable
/// sequence, and `range` is a first-class operation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    rows: BTreeSet<usize>,
    /// Where a shift-range starts. `None` until a plain click sets it.
    anchor: Option<usize>,
    /// The row holding the keyboard focus.
    focus: Option<usize>,
    /// The inclusive span, if the current selection is exactly one range.
    ///
    /// # Why this cannot be derived from `anchor` and `focus`
    ///
    /// The natural guess is "the selection is `anchor..=focus`, plus any
    /// ctrl-clicked rows outside it". That guess is wrong in two ways, and both
    /// were caught by tests during Phase 3:
    ///
    /// * After `Ctrl+A` the selection is the whole list while
    ///   `anchor..=focus` is one row, so a following shift-click would re-derive
    ///   a one-row span, leave the other 49 selected, and appear to do nothing.
    /// * A `Ctrl`+click makes the selection genuinely non-contiguous, so "span"
    ///   has no meaning at all until something collapses it back to a range.
    ///
    /// Storing the span explicitly is the honest version of the model: it says
    /// which rows the current *range* gesture owns, and `None` says "this
    /// selection is not a range". `extend_to` then only ever rewrites the rows
    /// inside its own span, which is why a narrowing shift-click cannot
    /// disturb a ctrl-clicked row three lines away.
    span: Option<(usize, usize)>,
}

impl Selection {
    /// An empty selection with no focus.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The selected row indices, in ascending order.
    pub fn rows(&self) -> impl Iterator<Item = usize> + '_ {
        self.rows.iter().copied()
    }

    /// `true` when `row` is selected.
    #[must_use]
    pub fn contains(&self, row: usize) -> bool {
        self.rows.contains(&row)
    }

    /// How many rows are selected.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// `true` when nothing is selected.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The row holding keyboard focus, if any.
    ///
    /// Distinct from selection: in multi-select, only the focused *selected*
    /// row carries the focus ring (§4.2 "Selection model").
    #[must_use]
    pub fn focus(&self) -> Option<usize> {
        self.focus
    }

    /// Where the next shift-range will start.
    #[must_use]
    pub fn anchor(&self) -> Option<usize> {
        self.anchor
    }

    /// A plain click: selects exactly `row`, and re-anchors there.
    ///
    /// §4.2: `selected` is a single tint. A plain click always collapses.
    pub fn click(&mut self, row: usize) {
        self.rows.clear();
        self.rows.insert(row);
        self.anchor = Some(row);
        self.focus = Some(row);
        self.span = Some((row, row));
    }

    /// A ctrl-click: toggles `row` and **moves the anchor**.
    ///
    /// Moving the anchor is what makes a ctrl-click followed by a shift-click
    /// do what a user expects: extend from the row they last touched. Not
    /// moving it makes the second gesture appear to do nothing.
    pub fn toggle(&mut self, row: usize) {
        if !self.rows.remove(&row) {
            self.rows.insert(row);
        }
        self.anchor = Some(row);
        self.focus = Some(row);
        // A ctrl-click makes the selection non-contiguous, so it is no longer a
        // range and has no span to rewrite. The anchor still moves, so a
        // following shift-click extends from the row just touched.
        self.span = None;
    }

    /// A shift-click: selects everything between the anchor and `row`.
    ///
    /// §4.2's shift-select model, inclusive at both ends. If there is no anchor
    /// — a shift-click before any plain click — this degrades to a plain click
    /// rather than selecting the whole list, which would be a surprising way to
    /// lose your selection.
    ///
    /// # Growing and shrinking are the same operation
    ///
    /// The selection is always exactly "anchor to focus" plus whatever was
    /// ctrl-clicked outside it. So this *re-derives* the anchored span rather
    /// than unioning a new one into the old:
    ///
    /// ```text
    /// rows.retain(outside the old anchored span)
    /// rows.extend(the new anchored span)
    /// ```
    ///
    /// That single rule gets both directions right. Unioning instead would make
    /// a range that can only ever grow, so `Ctrl+A` then shift-click row 5 would
    /// still leave all 50 selected and a mistaken shift-click would be
    /// unrecoverable except by starting over. Rewriting the span is what GTK's
    /// `TreeSelection` does, and it is the only version where `Ctrl+A` followed
    /// by a narrowing shift-click means what it looks like it means.
    ///
    /// Rows ctrl-clicked *outside* the span survive untouched, because the
    /// `retain` only removes the old span.
    pub fn extend_to(&mut self, row: usize) {
        let Some(anchor) = self.anchor else {
            self.click(row);
            return;
        };
        // Only the rows the *previous* range owned are rewritten. Anything
        // outside is a ctrl-click and is not this gesture's to touch.
        if let Some((lo, hi)) = self.span {
            self.rows.retain(|r| *r < lo || *r > hi);
        }
        let (lo, hi) = if anchor <= row {
            (anchor, row)
        } else {
            (row, anchor)
        };
        self.rows.extend(lo..=hi);
        self.span = Some((lo, hi));
        // The focus follows the click, but the anchor does **not** move: a second
        // shift-click re-extends from where the range started, which is what
        // every list view in every toolkit does.
        self.focus = Some(row);
    }

    /// A shift-arrow: extends the anchored range by `delta` rows.
    ///
    /// **This is what makes the anchor load-bearing.** Without it, nothing can
    /// move the focus while leaving the anchor behind, so an anchor and a focus
    /// are indistinguishable and the "anchor" is a synonym for "last row
    /// clicked". With it, `click(10)` then `shift+Down x3` gives 10..13, which
    /// is the gesture every file manager trains you to make.
    pub fn extend_by(&mut self, delta: isize, count: usize) {
        if count == 0 {
            self.clear();
            return;
        }
        let last = count as isize - 1;
        let current = self.focus.map_or(0, |f| f as isize);
        let next = (current + delta).clamp(0, last) as usize;
        self.extend_to(next);
    }

    /// Selects every row in `0..count`. §4.11 `Ctrl+A`.
    ///
    /// The anchor goes to the top and the focus to the first row, and the span
    /// is recorded as the *whole* list.
    ///
    /// Recording the span is what makes the following shift-click work: with
    /// `span = Some((0, count - 1))`, `extend_to(5)` rewrites 0..50 to 0..6. Had
    /// the span been left as the one row `anchor..=focus` describes, the other
    /// 49 rows would have survived and the click would have appeared to do
    /// nothing.
    pub fn select_all(&mut self, count: usize) {
        self.rows.clear();
        self.rows.extend(0..count);
        if count == 0 {
            self.anchor = None;
            self.focus = None;
            self.span = None;
            return;
        }
        self.anchor = Some(0);
        self.focus = Some(0);
        self.span = Some((0, count - 1));
    }

    /// Clears the selection and the anchor. §4.11 `Escape`.
    pub fn clear(&mut self) {
        self.rows.clear();
        self.anchor = None;
        self.focus = None;
        self.span = None;
    }

    /// Moves the focus by `delta` rows, clamped to `0..count`, and collapses the
    /// selection onto it.
    ///
    /// Arrow keys move *selection*, not a second invisible cursor. In a file
    /// list, a focus ring that moves without selecting is a selection model the
    /// user has to learn twice.
    ///
    /// A bare arrow **resets the anchor** (via [`Selection::click`]). That is
    /// standard — the selection has collapsed to one row, so the range that
    /// anchor described no longer exists — and it is what distinguishes a bare
    /// arrow from a shift-arrow.
    ///
    /// With no focus yet, direction picks the end: `Down` selects the first row
    /// and `Up` the last. "No selection, press Down, get row 2" is the kind of
    /// off-by-one that makes a list feel broken.
    pub fn move_focus(&mut self, delta: isize, count: usize) {
        if count == 0 {
            self.clear();
            return;
        }
        let last = count as isize - 1;
        let next = match self.focus {
            Some(f) => ((f as isize) + delta).clamp(0, last) as usize,
            None if delta < 0 => last as usize,
            None => 0,
        };
        self.click(next);
    }

    /// Moves the focus to an absolute row, clamped.
    pub fn focus_row(&mut self, row: usize, count: usize) {
        if count == 0 {
            self.clear();
            return;
        }
        self.click(row.min(count - 1));
    }

    /// `true` when the focused row is also selected — the only state that draws
    /// the focus ring on a selected row (§4.2's `selected + focused`).
    #[must_use]
    pub fn focus_is_selected(&self) -> bool {
        self.focus.is_some_and(|f| self.rows.contains(&f))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(s: &Selection) -> Vec<usize> {
        s.rows().collect()
    }

    #[test]
    fn a_plain_click_selects_exactly_one_row_and_anchors() {
        let mut s = Selection::new();
        s.click(7);
        assert_eq!(rows(&s), vec![7]);
        assert_eq!(s.anchor(), Some(7));
        assert_eq!(s.focus(), Some(7));
    }

    #[test]
    fn a_second_plain_click_collapses_the_selection() {
        let mut s = Selection::new();
        s.click(3);
        s.click(9);
        assert_eq!(
            rows(&s),
            vec![9],
            "plain click must collapse, not accumulate"
        );
    }

    #[test]
    fn ctrl_click_toggles_and_moves_the_anchor() {
        let mut s = Selection::new();
        s.click(1);
        s.toggle(5);
        s.toggle(1); // deselect
        assert_eq!(rows(&s), vec![5]);
        assert_eq!(s.anchor(), Some(1), "the anchor follows the ctrl-click");
    }

    #[test]
    fn the_anchor_survives_whatever_moved_the_focus() {
        // The Phase 2 bug, stated as a property: with a real anchor, a
        // shift-click's range is decided by the *anchor*, so anything that
        // moves the focus without moving the anchor cannot change the range's
        // origin. Shift-arrow is the only such gesture, so it is the one tested.
        let mut s = Selection::new();
        s.click(10);
        s.extend_by(1, 100);
        assert_eq!(s.focus(), Some(11));
        assert_eq!(
            s.anchor(),
            Some(10),
            "the anchor must not follow a shift-arrow"
        );
        assert_eq!(rows(&s), (10..=11).collect::<Vec<_>>());
    }

    #[test]
    fn a_bare_arrow_collapses_and_resets_the_anchor() {
        // Deliberately the opposite of shift-arrow: collapsing the selection to
        // one row destroys the range, so the anchor moves with it.
        let mut s = Selection::new();
        s.click(10);
        s.move_focus(1, 100);
        assert_eq!(rows(&s), vec![11]);
        assert_eq!(s.anchor(), Some(11));
        // ... so this shift-click anchors at 11, not at 10.
        s.extend_to(13);
        assert_eq!(rows(&s), (11..=13).collect::<Vec<_>>());
    }

    #[test]
    fn shift_arrow_extends_a_range_row_by_row() {
        // The gesture every file manager trains you to make.
        let mut s = Selection::new();
        s.click(4);
        for _ in 0..3 {
            s.extend_by(1, 50);
        }
        assert_eq!(rows(&s), (4..=7).collect::<Vec<_>>());
        // And back up, narrowing.
        s.extend_by(-1, 50);
        assert_eq!(rows(&s), (4..=6).collect::<Vec<_>>());
        assert_eq!(s.anchor(), Some(4), "narrowing must keep the anchor");
    }

    #[test]
    fn shift_arrow_on_an_empty_list_does_nothing() {
        let mut s = Selection::new();
        s.extend_by(1, 0);
        assert!(s.is_empty());
    }

    #[test]
    fn a_second_shift_click_re_extends_from_the_same_anchor() {
        let mut s = Selection::new();
        s.click(10);
        s.extend_to(12);
        assert_eq!(rows(&s), (10..=12).collect::<Vec<_>>());
        s.extend_to(20);
        assert_eq!(
            rows(&s),
            (10..=20).collect::<Vec<_>>(),
            "the range grows from the anchor, it does not restart at 12"
        );
    }

    #[test]
    fn shift_click_above_the_anchor_selects_backwards() {
        let mut s = Selection::new();
        s.click(20);
        s.extend_to(5);
        assert_eq!(rows(&s), (5..=20).collect::<Vec<_>>());
    }

    #[test]
    fn shift_click_without_an_anchor_degrades_to_a_plain_click() {
        let mut s = Selection::new();
        s.extend_to(4);
        assert_eq!(
            rows(&s),
            vec![4],
            "no anchor must not select the whole list"
        );
        assert_eq!(s.anchor(), Some(4));
    }

    #[test]
    fn shift_click_after_select_all_narrows_instead_of_selecting_everything() {
        // `Ctrl+A` anchors at 0, so a following shift-click means "no, that far".
        // Without the narrow, a mistaken shift-click could only ever be undone by
        // starting the selection over.
        let mut s = Selection::new();
        s.select_all(50);
        assert_eq!(s.len(), 50);
        s.extend_to(5);
        assert_eq!(rows(&s), (0..=5).collect::<Vec<_>>());
        // The anchor survived the narrow, so a second shift-click still knows
        // what the range is anchored to.
        assert_eq!(s.anchor(), Some(0));
        s.extend_to(2);
        assert_eq!(rows(&s), (0..=2).collect::<Vec<_>>());
    }

    #[test]
    fn ctrl_click_then_shift_click_extends_from_the_ctrl_click() {
        let mut s = Selection::new();
        s.click(2);
        s.toggle(20);
        s.extend_to(24);
        assert_eq!(rows(&s), vec![2, 20, 21, 22, 23, 24]);
    }

    #[test]
    fn arrows_collapse_the_selection_onto_the_focused_row() {
        let mut s = Selection::new();
        s.click(0);
        s.extend_to(10);
        assert_eq!(s.len(), 11);
        s.move_focus(1, 100);
        assert_eq!(rows(&s), vec![11], "an arrow collapses to one row");
        assert_eq!(s.anchor(), Some(11));
    }

    #[test]
    fn arrow_movement_is_clamped_to_the_row_range() {
        let mut s = Selection::new();
        s.move_focus(1, 5);
        assert_eq!(s.focus(), Some(0));
        s.move_focus(-100, 5);
        assert_eq!(s.focus(), Some(0), "cannot go above the first row");
        s.move_focus(1000, 5);
        assert_eq!(s.focus(), Some(4), "cannot go past the last row");
    }

    #[test]
    fn an_arrow_from_no_selection_picks_the_nearest_end() {
        // "No selection, press Down, land on row 2" reads as an off-by-one bug.
        let mut down = Selection::new();
        down.move_focus(1, 9);
        assert_eq!(down.focus(), Some(0));
        let mut up = Selection::new();
        up.move_focus(-1, 9);
        assert_eq!(up.focus(), Some(8));
    }

    #[test]
    fn an_empty_list_has_no_focus() {
        let mut s = Selection::new();
        s.move_focus(1, 0);
        assert_eq!(s.focus(), None);
        assert!(s.is_empty());
    }

    #[test]
    fn escape_clears_everything_including_the_anchor() {
        let mut s = Selection::new();
        s.select_all(20);
        s.clear();
        assert!(s.is_empty());
        assert_eq!(s.anchor(), None);
        assert_eq!(s.focus(), None);
        // ... and a following shift-click is a plain click, not a full select.
        s.extend_to(3);
        assert_eq!(rows(&s), vec![3]);
    }

    #[test]
    fn focus_is_reported_as_selected_only_when_it_really_is() {
        let mut s = Selection::new();
        s.click(4);
        assert!(s.focus_is_selected());
        s.toggle(4); // deselect; focus stays on 4
        assert!(
            !s.focus_is_selected(),
            "a deselected focused row has no ring"
        );
    }

    #[test]
    fn the_whole_binding_sequence_behaves() {
        // The canonical sequence, as a single narrative test.
        let mut s = Selection::new();
        s.click(2); // click
        assert_eq!(rows(&s), vec![2]);
        s.toggle(6); // ctrl-click
        s.extend_to(9); // shift-click
        assert_eq!(rows(&s), vec![2, 6, 7, 8, 9]);
        s.move_focus(1, 20); // down arrow collapses
        assert_eq!(rows(&s), vec![10]);
        s.select_all(20); // ctrl+A
        assert_eq!(s.len(), 20);
        s.clear(); // escape
        assert!(s.is_empty());
    }
}
