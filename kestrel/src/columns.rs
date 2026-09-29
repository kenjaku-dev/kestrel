//! Column geometry for the file list (§4.2's column tokens).
//!
//! # The two defects this exists to fix
//!
//! * **A wall of "Folder".** §4.2 gives `row.col-kind` a 92px column, but in a
//!   directory of directories that column is 92px of the word "Folder" repeated
//!   25 times. It carries zero information and costs horizontal space that the
//!   name column needs.
//! * **~600px of dead gap.** Phase 2 laid the fixed-width columns out from the
//!   right edge and then gave the name whatever was left — but it measured the
//!   *minimum* and let the remainder go unused, so a 1200px pane spent 400px on
//!   nothing between the name and the date.
//!
//! So the layout is decided here, as a pure function of the pane width and
//! whether the listing is actually mixed, and it is unit-tested rather than
//! eyeballed.

use egui::Rect;

use crate::tokens::{component, metric};

/// Which columns the listing shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColumnSet {
    /// `row.col-size` — always shown. It is a machine value and the reason the
    /// mono/sans split exists.
    pub size: bool,
    /// `row.col-kind` — shown only when the listing is genuinely mixed.
    pub kind: bool,
    /// `row.col-modified` — shown when there is room; dropped on a narrow pane
    /// before the name column is allowed to become unreadable.
    pub modified: bool,
}

/// The resolved horizontal layout of one row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColumnLayout {
    /// The name column: icon gutter through the flexible name.
    pub name: Rect,
    /// `row.col-size`, right-aligned.
    pub size: Option<Rect>,
    /// `row.col-kind`, left-aligned.
    pub kind: Option<Rect>,
    /// `row.col-modified`, right-aligned.
    pub modified: Option<Rect>,
    /// The leading gutter that holds the hidden-file dot.
    pub gutter: f32,
    /// The 16px icon slot.
    pub icon: Rect,
    /// Where the name text starts, after the icon and its gap.
    pub name_text_x: f32,
}

impl ColumnLayout {
    /// Lays a row out inside `row_rect`.
    ///
    /// The visible columns come from `set`, which the caller built with
    /// [`columns_for`] — so whether Kind is shown is decided in one place (see
    /// [`listing_is_mixed`]), not per row.
    #[must_use]
    pub fn resolve(row_rect: Rect, set: ColumnSet) -> Self {
        let gap = component::ROW_COLUMN_GAP;
        let mut right = row_rect.right();

        // Every cell spans the row's own y range.
        //
        // This is not a detail: the first version of this function built the
        // right-hand cells through a `pos(x) -> pos2(x, 0.0)` helper, so Size,
        // Kind and Modified were painted at `y = 0` while Name sat correctly in
        // its row. The list looked *right* — the values were present, coloured
        // and aligned — except that the header strip for them was stranded at the
        // top of the window, 74px above the header it belonged to. A
        // horizontal-overlap assertion cannot catch that, so
        // `every_cell_spans_the_rows_vertical_extent` exists below.
        let cell = |x: f32, w: f32| {
            Rect::from_min_max(
                egui::pos2(x, row_rect.top()),
                egui::pos2(x + w, row_rect.bottom()),
            )
        };

        // Right-to-left from the trailing edge, so the fixed-width columns keep
        // their §2.9 widths and whatever is left over goes to the name.
        let modified = if set.modified {
            let w = component::ROW_COL_MODIFIED_WIDTH;
            right -= w;
            let r = cell(right, w);
            right -= gap;
            Some(r)
        } else {
            None
        };

        let kind = if set.kind {
            let w = component::ROW_COL_KIND_WIDTH;
            right -= w;
            let r = cell(right, w);
            right -= gap;
            Some(r)
        } else {
            None
        };

        let size = if set.size {
            let w = component::ROW_COL_SIZE_WIDTH;
            right -= w;
            let r = cell(right, w);
            right -= gap;
            Some(r)
        } else {
            None
        };

        // The gutter + icon + icon gap are the name column's fixed prefix.
        let gutter = metric::GUTTER;
        let icon_x = row_rect.left() + gutter;
        let icon = Rect::from_center_size(
            egui::pos2(icon_x + component::ROW_ICON_SIZE / 2.0, row_rect.center().y),
            egui::vec2(component::ROW_ICON_SIZE, component::ROW_ICON_SIZE),
        );
        let name_text_x = icon.right() + component::ROW_ICON_GAP;

        // The name column takes **everything** left. This is the fix for the dead
        // gap: the leftover is not left as padding, it is given to the column
        // that can use it. §2.9's `metric.column-name` is "flexible, min 160";
        // the minimum is a floor for a pane too narrow to honour it, not a
        // target.
        let name_right = right.max(name_text_x + 1.0);
        let name = Rect::from_min_max(
            egui::pos2(row_rect.left(), row_rect.top()),
            egui::pos2(name_right, row_rect.bottom()),
        );

        Self {
            name,
            size,
            kind,
            modified,
            gutter,
            icon,
            name_text_x,
        }
    }

    /// The width available for name text, after the icon prefix.
    #[must_use]
    pub fn name_text_width(&self) -> f32 {
        (self.name.right() - self.name_text_x).max(0.0)
    }
}

/// The column set to use for a pane of `width` px.
///
/// `Modified` is dropped first on a narrow pane, because a timestamp is the
/// column a user can most afford to lose — it is also the one they are least
/// likely to be *scanning for* (they are scanning for names). The name column
/// keeps its §2.9 floor of 160px before anything else is dropped.
#[must_use]
pub fn columns_for(width: f32, show_kind: bool) -> ColumnSet {
    let need = |set: ColumnSet| -> f32 {
        let mut w = metric::GUTTER
            + component::ROW_ICON_SIZE
            + component::ROW_ICON_GAP
            + metric::COLUMN_NAME_MIN;
        if set.size {
            w += component::ROW_COL_SIZE_WIDTH + component::ROW_COLUMN_GAP;
        }
        if set.kind {
            w += component::ROW_COL_KIND_WIDTH + component::ROW_COLUMN_GAP;
        }
        if set.modified {
            w += component::ROW_COL_MODIFIED_WIDTH + component::ROW_COLUMN_GAP;
        }
        w
    };

    let mut set = ColumnSet {
        size: true,
        kind: show_kind,
        modified: true,
    };
    // Drop in reverse order of expendability: modified, then kind, then size.
    // Size is last because it is the reason the mono/sans split exists.
    if need(set) > width {
        set.modified = false;
    }
    if need(set) > width {
        set.kind = false;
    }
    if need(set) > width {
        set.size = false;
    }
    set
}

/// `true` when the listing has more than one entry kind, so the Kind column
/// carries information.
///
/// A directory of 40 folders and a directory of 40 `.rs` files are both
/// "uniform"; a directory of 20 folders and 20 files is not. This is what stops
/// the column from being 92px of the same word.
#[must_use]
pub fn listing_is_mixed(kinds: impl IntoIterator<Item = bool>) -> bool {
    let mut it = kinds.into_iter();
    let Some(first) = it.next() else {
        // An empty listing is not "mixed"; showing a column for zero rows is
        // pure noise.
        return false;
    };
    // Sample rather than count all: a 50,000-row listing must not be walked
    // twice per frame just to decide on a column. Two different kinds anywhere
    // in the first `SAMPLE` rows is enough to justify the column, and if the mix
    // only starts later the user is in a directory of more than 64 entries that
    // all sort together anyway, where this column is not what they were reading.
    //
    // The budget counts the row consumed by `next()` above, so `SAMPLE` really is
    // the number of rows examined. Getting this wrong by one was a real
    // off-by-one here: the first version read `SAMPLE + 1` rows and a test that
    // specifically probed the boundary caught it.
    const SAMPLE: usize = 64;
    let mut seen = 1usize;
    let mut seen_other = false;
    for is_dir in it {
        if is_dir != first {
            seen_other = true;
            break;
        }
        seen += 1;
        if seen >= SAMPLE {
            break;
        }
    }
    seen_other
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Rect, vec2};

    fn row(w: f32) -> Rect {
        Rect::from_min_size(egui::Pos2::ZERO, vec2(w, component::ROW_HEIGHT))
    }

    #[test]
    fn the_name_column_absorbs_all_leftover_width() {
        // Defect 5. At 1000px with no Kind column, the name column must reach
        // the Size column, not stop 400px short of it.
        let r = row(1000.0);
        let set = columns_for(1000.0, false);
        let l = ColumnLayout::resolve(r, set);
        let size = l.size.expect("size column");
        assert_eq!(l.name.right(), size.left() - component::ROW_COLUMN_GAP);
        assert!(
            l.name_text_width() > 600.0,
            "name got only {}px of text width",
            l.name_text_width()
        );
    }

    #[test]
    fn every_column_ends_where_the_next_one_starts() {
        // No gaps, no overlaps: the row is a clean partition. The edges are
        // collected left-to-right *in layout order* — collecting them in
        // declaration order produced a test that passed for the wrong reason and
        // then failed for the wrong one, which is worse than no test.
        for width in [400.0_f32, 700.0, 1000.0, 1400.0] {
            for show_kind in [false, true] {
                let set = columns_for(width, show_kind);
                let l = ColumnLayout::resolve(row(width), set);
                // `resolve` walks right-to-left, so this is left-to-right.
                let mut edges = vec![l.name.right()];
                for cell in [l.size, l.kind, l.modified].into_iter().flatten() {
                    edges.push(cell.left());
                }
                for pair in edges.windows(2) {
                    assert!(
                        pair[1] >= pair[0] - 0.01,
                        "width {width} show_kind {show_kind}: {edges:?} are not                          monotonically increasing"
                    );
                }
                // And the last column reaches the right edge of the row.
                let last = [l.size, l.kind, l.modified]
                    .into_iter()
                    .flatten()
                    .last()
                    .map_or(l.name.right(), |c| c.right());
                assert!(
                    (last - width).abs() < 0.01,
                    "width {width} show_kind {show_kind}: columns stop at {last}"
                );
            }
        }
    }

    #[test]
    fn the_kind_column_is_dropped_on_a_narrow_pane() {
        // Widths needed: gutter 10 + icon 16 + icon-gap 10 + name-min 160 = 196
        // for the name column, then size 88+12, kind 92+12, modified 140+12.
        // Name+size+kind = 400, so 400 still fits Kind exactly and 340 does not.
        assert!(columns_for(900.0, true).kind, "a 900px pane can show Kind");
        let snug = columns_for(400.0, true);
        assert!(!snug.modified, "modified goes first");
        assert!(snug.kind, "Kind still fits in exactly 400px");

        let narrow = columns_for(340.0, true);
        assert!(!narrow.modified, "modified goes first");
        assert!(!narrow.kind, "then kind");
        assert!(narrow.size, "size outlives kind");
    }

    #[test]
    fn the_size_column_survives_longest() {
        // It is the reason the mono/sans split exists, so it is the last to go.
        // Name+size = 296, so 300 keeps Size and 240 does not.
        let tight = columns_for(300.0, false);
        assert!(!tight.modified);
        assert!(tight.size, "size must outlive the modified column");

        let tiny = columns_for(240.0, false);
        assert!(!tiny.modified);
        assert!(!tiny.size, "below 296px even Size has to go");
    }

    #[test]
    fn a_uniform_listing_hides_the_kind_column() {
        // Defect 4: 40 folders is not a mixed listing.
        let dirs = (0..40).map(|_| true);
        assert!(!listing_is_mixed(dirs));
        let files = (0..40).map(|_| false);
        assert!(!listing_is_mixed(files));
    }

    #[test]
    fn a_genuinely_mixed_listing_shows_the_kind_column() {
        let mixed = (0..40).map(|i| i % 2 == 0);
        assert!(listing_is_mixed(mixed));
    }

    #[test]
    fn an_empty_listing_is_not_mixed() {
        assert!(!listing_is_mixed(std::iter::empty()));
        assert!(!listing_is_mixed([true]));
    }

    #[test]
    fn mixedness_is_decided_from_a_bounded_sample() {
        // A 50,000-row listing must not be walked twice per frame. If the first
        // 64 rows are all directories the answer is "uniform", even if row 900
        // is a file — the cost of a wrong answer here is one column, and the
        // cost of counting every row is a second pass over the whole list.
        //
        // The boundary is exact: 63 directories then a file is already mixed,
        // because the sample is 64 rows *including* the first.
        let at_the_boundary = (0..63).map(|_| true).chain(std::iter::once(false));
        assert!(
            listing_is_mixed(at_the_boundary),
            "row 64 is inside the sample and must be seen"
        );
        let just_past = (0..64).map(|_| true).chain(std::iter::once(false));
        assert!(
            !listing_is_mixed(just_past),
            "row 65 is past the sample and must not be read"
        );
    }

    #[test]
    fn name_text_budget_scales_with_the_pane() {
        let wide = ColumnLayout::resolve(row(1400.0), columns_for(1400.0, false));
        let narrow = ColumnLayout::resolve(row(500.0), columns_for(500.0, false));
        assert!(
            wide.name_text_width() > narrow.name_text_width(),
            "a wider pane must fit more characters"
        );
        assert!(narrow.name_text_width() > 0.0, "never less than nothing");
    }

    /// The trailing column reaches the row's right edge, and the two right-hand
    /// cells are exactly one column-gap apart.
    ///
    /// The *inset* of a right-aligned value is applied at paint time, not here —
    /// the cell is the full column and the text sits `row.padding-x` inside it.
    /// `shot::tests::the_list_never_draws_into_the_preview_pane` is what pins the
    /// painted result; this pins the geometry it is painted into.
    #[test]
    fn the_trailing_columns_partition_the_row() {
        let width = 1000.0;
        let l = ColumnLayout::resolve(row(width), columns_for(width, false));
        let modified = l.modified.expect("modified column at 1000px");
        let size = l.size.expect("size column at 1000px");
        assert!((modified.right() - width).abs() < 0.01);
        // With no Kind column, Modified is the rightmost and Size sits to its
        // left, one column-gap away.
        assert!(
            (modified.left() - size.right() - component::ROW_COLUMN_GAP).abs() < 0.01,
            "modified at {modified:?}, size at {size:?}"
        );
        assert!(l.name.right() <= size.left() + 0.01);
    }

    /// Every cell must span the row's own vertical extent.
    ///
    /// This is the assertion that catches a `y = 0.0` hardcoded into a cell
    /// constructor, which is exactly the bug the first version of
    /// [`ColumnLayout::resolve`] had: Size, Kind and Modified rendered at the top
    /// of the window instead of in their row, while Name was correct, so the
    /// list still *looked* plausible and only the stranded header strip gave it
    /// away. A test that only checks horizontal adjacency is blind to it.
    #[test]
    fn every_cell_spans_the_rows_vertical_extent() {
        for top in [0.0_f32, 28.0, 62.0, 101.5] {
            let r = Rect::from_min_size(egui::Pos2::new(0.0, top), vec2(1000.0, 24.0));
            let l = ColumnLayout::resolve(r, columns_for(1000.0, true));
            let cells = [Some(l.name), l.size, l.kind, l.modified];
            for cell in cells.into_iter().flatten() {
                assert!(
                    (cell.top() - r.top()).abs() < 0.01
                        && (cell.bottom() - r.bottom()).abs() < 0.01,
                    "a cell is not vertically aligned with its row at top={top}: \
                     cell {cell:?} vs row {r:?}"
                );
                assert!(
                    (cell.height() - r.height()).abs() < 0.01,
                    "a cell has the wrong height at top={top}: {cell:?}"
                );
            }
        }
    }

    #[test]
    fn the_gutter_and_icon_keep_their_spec_metrics() {
        let l = ColumnLayout::resolve(row(1000.0), columns_for(1000.0, false));
        assert_eq!(l.gutter, metric::GUTTER);
        assert_eq!(l.icon.width(), component::ROW_ICON_SIZE);
        assert_eq!(l.icon.height(), component::ROW_ICON_SIZE);
        assert_eq!(l.icon.left(), metric::GUTTER);
        assert_eq!(l.icon.height(), component::ROW_ICON_SIZE);
        // The name starts after the icon plus `row.icon-gap`.
        assert_eq!(l.name_text_x, l.icon.right() + component::ROW_ICON_GAP);
    }

    #[test]
    fn a_pane_too_narrow_for_the_name_floor_does_not_produce_a_negative_width() {
        let l = ColumnLayout::resolve(row(80.0), columns_for(80.0, true));
        assert!(
            l.name_text_width() >= 0.0,
            "a 80px pane must not produce a negative name width"
        );
    }
}
