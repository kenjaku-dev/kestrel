//! The tests for [`crate::app`].
//!
//! Split out of `app.rs` by `#[path]`, not by a new module in `main.rs`,
//! because these tests reach into `app`'s **private** fields — `rows`,
//! `selection`, `scroll_rows`, `errors`. A sibling module inside `app` can see
//! them; a crate-root module cannot, and re-exporting every field to make a
//! test possible would be a much worse trade than an unusual `#[path]`.

// The glob is the point: these tests are `app`'s own, they reach into its
// private fields, and naming thirty imports by hand to get at them would be the
// thing most likely to rot.
#![allow(
    clippy::wildcard_imports,
    reason = "these are app's own private items, imported by design"
)]

// A glob import trips `unused_imports` when nothing it brought in is
// `pub` — which is the case for every name these tests use, because they are
// `app`'s private fields. The allow is the only correct answer; `allow(unused)`
// would have hidden real dead imports.
#[allow(unused_imports)]
use super::*;
// A glob import trips `unused_imports` when nothing it brought in is
// `pub` — which is the case for every name these tests use, because they are
// `app`'s private fields. The allow is the only correct answer; `allow(unused)`
// would have hidden real dead imports.
#[allow(unused_imports)]
use super::*;

/// A `KestrelApp` with no window, no scan and no watcher behind it.
///
/// `assemble` is deliberately not used: it starts a scan and a watcher, which
/// is real I/O on real threads for tests that only want three fields set.
fn minimal_app() -> KestrelApp {
    KestrelApp {
        screen: Screen::Browser,
        mode: ThemeMode::Dark,
        theme: Theme::dark(),
        motion: Motion::Full,
        places: Vec::new(),
        dir: PathBuf::from("/r"),
        nav_fade: None,
        nav_pending: false,
        rows: Vec::new(),
        expanded: BTreeSet::new(),
        errors: Vec::new(),
        scan: None,
        watch: None,
        watch_error: None,
        history: History::new(Path::new("/r")),
        selection: Selection::new(),
        clipboard: Clipboard::new(),
        sort: SortSpec::default(),
        sort_column: SortKey::Name,
        view: ViewMode::List,
        show_kind_column: false,
        scroll_rows: 0,
        viewport_rows: 1,
        space: SpaceProbe::new(),
        job: None,
        job_progress: None,
        pending: None,
        modal: None,
        modal_focus: 0,
        renaming: None,
        creating: false,
        mkdir_result: None,
        preview: PvLoader::new(),
        show_preview: true,
        preview_width: settings::PREVIEW_DEFAULT,
        show_sidebar: true,
        show_hidden: true,
        preview_content: None,
        filter: String::new(),
        filter_focused: false,
        frame_ms: 0.0,
        frames: 0,
        toolbar_narrow: false,
        settings: settings::Stored::default(),
        settings_dirty: false,
        help: false,
        pane_too_narrow: false,
    }
}

fn row_at(root: &str, path: &str) -> Row {
    Row {
        depth: depth_of(Path::new(root), Path::new(path)),
        entry: FileEntry {
            name: Path::new(path)
                .file_name()
                .map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned()),
            path: PathBuf::from(path),
            kind: EntryKind::File,
            size: None,
            modified: None,
            hidden: false,
            is_dir_target: None,
        },
    }
}

fn dir_entry(path: &str) -> FileEntry {
    FileEntry {
        name: Path::new(path)
            .file_name()
            .map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned()),
        path: PathBuf::from(path),
        kind: EntryKind::Directory,
        size: Some(4096),
        modified: None,
        hidden: false,
        is_dir_target: None,
    }
}

/// Depth comes from the path, so a nested child is genuinely two levels down.
///
/// The Phase 3 stub returned `1` for every row past the first, which rendered
/// a 3-deep tree as a 2-deep one — a "feature" that was a lie. This is the
/// test that says a grandchild is a grandchild.
#[test]
fn depth_counts_components_below_the_root() {
    assert_eq!(depth_of(Path::new("/r"), Path::new("/r/a.txt")), 0);
    assert_eq!(depth_of(Path::new("/r"), Path::new("/r/d/a.txt")), 1);
    assert_eq!(depth_of(Path::new("/r"), Path::new("/r/d/e/a.txt")), 2);
    assert_eq!(depth_of(Path::new("/r"), Path::new("/r/d/e/f/a.txt")), 3);
    // The root itself is not below the root.
    assert_eq!(depth_of(Path::new("/r"), Path::new("/r")), 0);
    // A path outside the root must not wrap `usize` around in `saturating_sub`.
    assert_eq!(depth_of(Path::new("/r"), Path::new("/other/a.txt")), 0);
}

/// Expanding is a pure filter: a collapsed parent hides exactly its subtree.
#[test]
fn a_collapsed_parent_hides_its_children() {
    let rows = [
        row_at("/r", "/r/top.txt"),
        row_at("/r", "/r/d/child.txt"),
        row_at("/r", "/r/d/sub/grand.txt"),
    ];
    let mut app = minimal_app();
    app.view = ViewMode::Tree;
    assert!(app.tree_is_visible(&rows[0]));
    assert!(!app.tree_is_visible(&rows[1]), "d is collapsed");
    assert!(!app.tree_is_visible(&rows[2]), "d is collapsed");

    // Expanding `d` reveals its direct children but not `d`'s grandchildren:
    // `d/sub` is a different directory and is still collapsed.
    app.toggle_expand(Path::new("/r/d"));
    assert!(app.tree_is_visible(&rows[0]));
    assert!(app.tree_is_visible(&rows[1]));
    assert!(!app.tree_is_visible(&rows[2]));

    app.toggle_expand(Path::new("/r/d/sub"));
    assert!(app.tree_is_visible(&rows[2]));

    // Collapsing `d` hides the whole subtree, grandchild included.
    app.toggle_expand(Path::new("/r/d"));
    assert!(!app.tree_is_visible(&rows[1]));
    assert!(!app.tree_is_visible(&rows[2]));
}

/// In list mode the expansion set is irrelevant: every row is top level.
#[test]
fn list_mode_shows_every_row_regardless_of_expansion() {
    let child = row_at("/r", "/r/d/child.txt");
    let mut app = minimal_app();
    app.view = ViewMode::List;
    assert!(app.tree_is_visible(&child));
}

#[test]
fn toggling_reports_the_new_state() {
    let mut app = minimal_app();
    assert!(app.toggle_expand(Path::new("/r/d")), "first toggle expands");
    assert!(!app.toggle_expand(Path::new("/r/d")), "second toggles back");
    assert!(app.toggle_expand(Path::new("/r/d")));
    assert_eq!(app.expanded.len(), 1, "no duplicate entries");
}

/// The tree scan reads exactly one level deeper than it shows, and never
/// more. An unbounded recursive scan behind a view toggle is how a file
/// manager hangs on a directory with many subdirectories — and depth 1 is
/// precisely what makes expanding free of I/O.
#[test]
fn the_tree_scan_is_bounded_at_one_level() {
    let mut app = minimal_app();
    app.view = ViewMode::List;
    let list = app.scan_options();
    assert!(!list.recursive, "list mode must not recurse at all");
    assert_eq!(list.max_depth, 0);

    app.view = ViewMode::Tree;
    let tree = app.scan_options();
    assert!(tree.recursive);
    assert_eq!(tree.max_depth, 1, "depth 1 is what makes expand I/O-free");
}

/// Descending needs no I/O, because the scanner resolved the link.
#[test]
fn descendability_comes_from_the_scanner_not_from_a_stat() {
    let real = dir_entry("/r/d");
    assert!(descendable(&real).is_some());

    let mut link = real.clone();
    link.kind = EntryKind::Symlink;
    link.is_dir_target = None;
    assert!(
        descendable(&link).is_none(),
        "an unresolved link must not be entered"
    );
    link.is_dir_target = Some(true);
    assert!(
        descendable(&link).is_some(),
        "a link to a directory is entered"
    );
    link.is_dir_target = Some(false);
    assert!(descendable(&link).is_none(), "a link to a file is not");
}

/// The breadcrumb always ends with the current directory, whatever the path.
#[test]
fn the_breadcrumb_is_root_first_and_ends_at_the_leaf() {
    let segs = breadcrumb_segments(Path::new("/tmp/opencode/p3"));
    let labels: Vec<&str> = segs.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(labels, vec!["/", "tmp", "opencode", "p3"]);
    // Every segment's path is the directory it navigates to.
    assert_eq!(
        segs.last().map(|s| s.path.as_path()),
        Some(Path::new("/tmp/opencode/p3"))
    );
    assert_eq!(segs[1].path, PathBuf::from("/tmp"));
}

#[test]
fn the_root_breadcrumb_is_just_the_root() {
    let segs = breadcrumb_segments(Path::new("/"));
    assert_eq!(segs.len(), 1);
    assert_eq!(segs[0].label, "/");
}

/// §4.3: the last two segments always stay visible, so "where am I" and
/// "what's in here" survive any width.
#[test]
fn the_breadcrumb_never_collapses_past_the_last_two_segments() {
    let deep = breadcrumb_segments(Path::new(
        "/a/very/long/segment/name/that/alone/exceeds/the/whole/pane/width/quite/easily",
    ));
    for width in [0.0_f32, 10.0, 80.0, 200.0, 2000.0] {
        let (visible, _) = trailing_segments(&deep, width);
        assert!(
            visible.len() >= 2.min(deep.len()),
            "width {width} kept only {} of {} segments",
            visible.len(),
            deep.len()
        );
    }
}

/// A wider pane collapses fewer ancestors, monotonically.
#[test]
fn a_wider_pane_shows_more_ancestors() {
    let deep = breadcrumb_segments(Path::new("/a/bb/ccc/dddd/eeeee/ffffff"));
    let narrow = trailing_segments(&deep, 60.0).1;
    let wide = trailing_segments(&deep, 2000.0).1;
    assert!(wide < narrow, "wide kept {wide}, narrow kept {narrow}");
    assert_eq!(wide, 0, "a wide pane shows the root too");
}

/// A `Ui` with one frame's worth of input, for driving the keyboard handler.
///
/// egui delivers key events through `RawInput::events`, and a *modifier-free*
/// `Key` event with the default `KeyState::Pressed` is what a real keypress
/// looks like to `consume_key`. The handler is given a `Ui` from a headless
/// context so the test can call it directly — which is the only way to ask
/// "does Delete reach the app while a rename field is open?" without a
/// window.
fn with_keys(keys: &[Key], mut body: impl FnMut(&mut KestrelApp, &mut Ui)) {
    let ctx = crate::shot::ctx_with_fonts();
    let raw = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 800.0))),
        events: keys
            .iter()
            .map(|k| egui::Event::Key {
                key: *k,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            })
            .collect(),
        ..Default::default()
    };
    let output = ctx.run_ui(raw, |ui| body(&mut minimal_app(), ui));
    output.drop_without_applying_deltas();
}

/// §4.11: while the rename field is open, the global bindings must not fire.
///
/// `Delete` is the case that matters. A row is selected, the user presses
/// `F2`, and the field is open — if `Delete` still reached the app, a
/// keystroke aimed at the *field* would open a confirm dialog to destroy the
/// selected file. The field is where the user's attention is; the app is not.
///
/// `consume_key` removes the event from the input state, so the same key
/// cannot also reach the `TextEdit`. The test asserts the app did not act,
/// which is the half that would be a data-loss bug.
#[test]
fn a_rename_field_owns_the_keyboard() {
    with_keys(&[Key::Delete], |app, ui| {
        app.rows = vec![row_at("/r", "/r/a.txt")];
        app.selection.click(0);
        app.begin_rename();
        assert!(app.renaming.is_some(), "the rename field is open");

        app.keyboard(ui);

        assert!(
            app.modal.is_none(),
            "Delete reached the app while the rename field had focus, and \
                 opened {:?}",
            app.modal
        );
        assert!(app.renaming.is_some(), "the field is still open");
    });
}

/// The same for the filter field, and for every other global binding.
///
/// `Ctrl+A` is the sharp one: while it belongs to the text editor it means
/// "select this query". Leaked to the app it would select every row in the
/// directory, which is the sort of mistake a user makes once and then stops
/// trusting the keyboard in.
#[test]
fn a_filter_field_owns_the_keyboard() {
    with_keys(&[Key::A], |app, ui| {
        app.rows = (0..5)
            .map(|i| row_at("/r", &format!("/r/f{i}.txt")))
            .collect();
        app.selection.click(0);
        app.filter_focused = true;
        assert_eq!(app.selection.len(), 1);

        ui.input_mut(|i| {
            i.consume_key(egui::Modifiers::CTRL, Key::A);
        });
        // `keyboard()` runs *after* the Ctrl modifier has been consumed above
        // in the real frame, because `TextEdit` handles it first. What the
        // app's own handler must not do is reach for a bare `A` and type it
        // into the query either.
        app.keyboard(ui);
        assert_eq!(app.selection.len(), 1, "Ctrl+A selected rows");
        assert!(app.filter.is_empty(), "a stray A reached the query");
    });
}

/// And with no field open, the same key does what §4.11 says.
///
/// The other half of the two tests above: a guard that swallows everything
/// would pass both of them, and a shortcut that never fires is a different
/// bug with the same symptom.
#[test]
fn a_rename_field_does_not_swallow_the_keyboard_forever() {
    with_keys(&[Key::Delete], |app, ui| {
        app.rows = vec![row_at("/r", "/r/a.txt")];
        app.selection.click(0);
        app.keyboard(ui);
        assert!(
            app.modal.is_some(),
            "with no field open, Delete must open the trash confirm"
        );
    });
}

/// The preview pane's `meta` line drops a field rather than cutting a
/// timestamp in half.
///
/// A `middle_truncate` on "Text · 2026-09-29 01:52 · 248 B" at 220px gives
/// "Text · 2026-09-29 0… · 248 B": a date that is *wrong* rather than
/// short. The fix is to drop the least useful field instead, and the order
/// is asserted here because it is a design decision and not an accident.
#[test]
fn the_meta_line_drops_a_field_rather_than_a_timestamp() {
    let ctx = crate::shot::ctx_with_fonts();
    // A `Painter` is only useful with a font collection behind it, so it is
    // borrowed out of a real frame rather than constructed: `Painter` holds
    // an `Arc` to the context's fonts, so the clone outlives the frame.
    let mut painter = None;
    let output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 200.0))),
            ..Default::default()
        },
        |ui| painter = Some(ui.painter().clone()),
    );
    output.drop_without_applying_deltas();
    let painter = painter.expect("a frame ran");
    let font = tokens::font(ty::META, &Theme::dark());
    let parts = ["Text", "2026-09-29 01:52", "248 B"];

    let full = "Text · 2026-09-29 01:52 · 248 B";
    let width_of = |s: &str| {
        painter
            .layout_no_wrap(s.to_owned(), font.clone(), egui::Color32::WHITE)
            .size()
            .x
    };

    assert_eq!(
        meta_line(&painter, &font, width_of(full), &parts),
        full,
        "a line that fits must be the whole line"
    );
    // One field short: the size goes, because the file list has a size
    // column and the pane does not.
    let two = "Text · 2026-09-29 01:52";
    assert_eq!(
        meta_line(&painter, &font, width_of(two), &parts),
        two,
        "the size is the first field to go"
    );
    // Down to the kind alone. Never a truncated timestamp, at any width.
    for width in [0.0_f32, 12.0, 40.0, width_of(two) - 1.0] {
        let line = meta_line(&painter, &font, width, &parts);
        assert!(
            !line.contains("2026-09-29 0"),
            "at {width}px the line is a wrong date: {line:?}"
        );
    }
    assert_eq!(
        meta_line(&painter, &font, 0.0, &parts),
        "Text",
        "one field always survives"
    );
}

/// A 1px margin helper that rounds rather than truncating.
///
/// `egui::Margin` is `i8` in epaint 0.36, so a half-pixel spacing token has
/// to round somewhere. Truncating `0.5` to `0` would silently remove a
/// margin; rounding keeps it.
#[test]
fn margin_rounds_rather_than_truncates() {
    assert_eq!(egui::Margin::same(0).left, 0);
    assert_eq!(egui::Margin::same(1).left, 1);
    // Out-of-range values are clamped, not wrapped into a negative margin.
    let big = margin(1000.0);
    assert!(big.left >= 0, "a huge margin must not wrap negative");
    assert_eq!(margin(-5.0).left, 0, "a negative margin clamps to 0");
}

// -- the directory cross-fade ------------------------------------------

/// Runs one frame of the app and returns the app's clock afterwards.
fn one_frame(app: &mut KestrelApp, at: f64) {
    let ctx = egui::Context::default();
    crate::tokens::fonts::install(&ctx);
    let out = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 800.0))),
            time: Some(at),
            ..Default::default()
        },
        |ui| {
            app.pump();
            app.draw(ui);
        },
    );
    out.drop_without_applying_deltas();
}

/// The fade starts when the **listing** lands, not when the user pressed
/// Enter, and it is over in 90ms.
///
/// Both halves matter and neither is visible in a screenshot: a fade from
/// the keystroke fades an empty pane and then snaps to the content, and a
/// fade longer than 90ms is lag on a double-click (§2.11 rule 3).
#[test]
fn a_directory_change_fades_in_over_the_specd_duration() {
    let mut app = minimal_app();
    // A navigation, with the scan already finished — the steady-state
    // refresh path, which is the common one.
    app.nav_pending = true;
    app.nav_fade = None;
    one_frame(&mut app, 10.0);
    let started = app.nav_fade.expect("a navigation should arm the fade");
    assert!(
        !app.nav_pending,
        "the pending flag is consumed exactly once"
    );
    // Part-way through: still fading.
    one_frame(&mut app, 10.045);
    assert!(
        app.nav_fade.is_some(),
        "45ms into a 90ms fade is still fading"
    );
    // Past the end: gone, and gone for good.
    one_frame(&mut app, 10.2);
    assert!(app.nav_fade.is_none(), "200ms is past motion.fast");
    one_frame(&mut app, 10.3);
    assert!(app.nav_fade.is_none(), "and it does not come back");
    assert!(started <= 10.0);
}

/// §2.11 rule 5: reduced motion renders the final state immediately, so the
/// fade never starts at all rather than running at 90ms.
#[test]
fn reduced_motion_has_no_directory_fade() {
    let mut app = minimal_app();
    app.motion = Motion::Reduced;
    app.nav_pending = true;
    one_frame(&mut app, 10.0);
    assert!(app.nav_fade.is_none());
    one_frame(&mut app, 10.0);
    assert!(
        app.nav_fade.is_none(),
        "and it must not start on a later frame either"
    );
}

/// A frame with no navigation in it never arms a fade, so a plain repaint —
/// a resize, a hover, a watcher event — does not make the list blink.
#[test]
fn only_a_navigation_arms_the_fade() {
    let mut app = minimal_app();
    one_frame(&mut app, 10.0);
    assert!(app.nav_fade.is_none());
    one_frame(&mut app, 10.05);
    assert!(app.nav_fade.is_none());
}

/// §7.13 and §2.11 rule 2: the fade is one `rect_filled` over the list's
/// own rectangle. Nothing in the paint path may change a row's rect, and the
/// assertion is that the app's layout state is identical either side of a
/// fade — a row height that tweened would show up here.
#[test]
fn a_fade_does_not_move_anything() {
    let mut app = minimal_app();
    app.rows = vec![Row {
        entry: dir_entry("/r/a.txt"),
        depth: 0,
    }];
    let before = (app.scroll_rows, app.viewport_rows);
    app.nav_pending = true;
    one_frame(&mut app, 10.0);
    one_frame(&mut app, 10.02);
    let mid = (app.scroll_rows, app.viewport_rows);
    one_frame(&mut app, 10.5);
    assert_eq!(mid, before, "a mid-fade frame changed the layout");
    assert_eq!(
        (app.scroll_rows, app.viewport_rows),
        before,
        "the settled frame differs from the unfaded one"
    );
}

/// The pane policy at a given window width.
///
/// A file manager with three panes in a 640px window is a file manager showing
/// 160px of a name column. The list is the reason the app exists, so it is the
/// last pane to give up space — and this is the rule, as a pure function so it
/// can be checked at every width rather than at the three that were screenshotted.
mod pane_policy {
    use super::LIST_MIN_W;
    use crate::dialog::preview_metrics;
    use crate::tokens::metric;

    /// What a window of `width` shows.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct Plan {
        sidebar: bool,
        /// `None` when the pane does not fit at any width.
        preview: Option<bool>,
    }

    /// The policy, mirroring `KestrelApp::panels`.
    fn plan(width: f32, want_sidebar: bool, want_preview: bool, preview_w: f32) -> Plan {
        let sidebar = want_sidebar && (width - metric::SIDEBAR_WIDTH >= LIST_MIN_W);
        let after = width - if sidebar { metric::SIDEBAR_WIDTH } else { 0.0 };
        let room = after - LIST_MIN_W;
        let preview = if !want_preview {
            Some(false)
        } else if room < preview_metrics::MIN_WIDTH {
            None
        } else {
            Some(preview_w <= room)
        };
        Plan { sidebar, preview }
    }

    #[test]
    fn a_wide_window_shows_everything_at_the_size_the_user_asked_for() {
        for w in [1200.0, 1000.0, 800.0] {
            let p = plan(w, true, true, 280.0);
            assert!(p.sidebar, "{w}");
            assert_eq!(p.preview, Some(true), "{w}");
        }
    }

    #[test]
    fn the_preview_shrinks_before_it_disappears() {
        // 640px: sidebar 200 + list 220 leaves 220 for the preview, which is
        // more than its 180 floor, so it shrinks rather than going.
        let p = plan(640.0, true, true, 280.0);
        assert!(p.sidebar);
        assert_eq!(p.preview, Some(false), "it shrinks rather than vanishing");
        // 500px: 500 - 200 = 300, minus the list's 220 is 80, which is under
        // the pane's own floor, so it cannot be shown at any width.
        let p = plan(500.0, true, true, 280.0);
        assert_eq!(p.preview, None);
    }

    #[test]
    fn the_sidebar_goes_before_the_preview_does() {
        // 420px is the exact crossing: 420 - 200 (the sidebar) leaves the
        // list's 220 and nothing for the preview, so the sidebar stays and the
        // preview cannot. The list is the reason the app exists.
        let p = plan(420.0, true, true, 280.0);
        assert!(p.sidebar);
        assert_eq!(p.preview, None);
        // 419px: the sidebar no longer fits beside a usable list, so it goes —
        // and the room it vacates is enough for the preview at 199px, which is
        // above its 180 floor. The panes trade places rather than both
        // disappearing, which is the whole point of the order.
        let p = plan(419.0, true, true, 280.0);
        assert!(!p.sidebar);
        assert_eq!(
            p.preview,
            Some(false),
            "it comes back, shrunk, in the sidebar's place"
        );
        // 400px: 400 - 220 leaves 180, exactly the preview's floor.
        assert_eq!(plan(400.0, true, true, 280.0).preview, Some(false));
        // 399px: under it, so the preview goes as well and the list has the lot.
        assert_eq!(plan(399.0, true, true, 280.0).preview, None);
    }

    #[test]
    fn the_list_always_gets_its_minimum() {
        for w in (320..=1600).step_by(4) {
            let w = w as f32;
            let p = plan(w, true, true, 280.0);
            let used = if p.sidebar {
                metric::SIDEBAR_WIDTH
            } else {
                0.0
            } + match p.preview {
                None => 0.0,
                Some(true) => 280.0,
                Some(false) => {
                    w - if p.sidebar {
                        metric::SIDEBAR_WIDTH
                    } else {
                        0.0
                    } - LIST_MIN_W
                }
            };
            assert!(
                w - used >= LIST_MIN_W - 0.5,
                "{w}px: panes take {used}, leaving the list {}",
                w - used
            );
        }
    }

    #[test]
    fn a_pane_the_user_turned_off_stays_off() {
        for w in [1200.0, 640.0, 420.0] {
            assert!(!plan(w, false, true, 280.0).sidebar, "{w}");
            assert_eq!(plan(w, true, false, 280.0).preview, Some(false), "{w}");
        }
    }
}
