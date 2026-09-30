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
        retired_jobs: Vec::new(),
        stopped_note: None,
        kind_rows: 0,
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
        opening: None,
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

/// `Enter` on the focused row branches on what the row *is*, and the two
/// branches are reached from one key.
///
/// The bug this pins: `Enter` descended into a directory and did nothing at all
/// on a file, so the app's most-used action was a no-op on most of its rows.
/// The branch is now total — a row is either descended into or opened — and
/// this asserts both arms and the symlink cases between them.
#[test]
fn enter_navigates_for_a_directory_and_opens_for_a_file() {
    use kestrel_fs::open::{Enter, enter_action};

    let mut dir = dir_entry("/r/src");
    dir.kind = EntryKind::Directory;
    assert_eq!(
        enter_action(&dir),
        Some(Enter::Descend),
        "a folder navigates"
    );

    let mut file = dir_entry("/r/notes.md");
    file.kind = EntryKind::File;
    assert_eq!(enter_action(&file), Some(Enter::Open), "a file opens");

    // A link is answered by its target, not by its own name: a link to a
    // directory descends, and a link to a file opens — the latter by its
    // target's type, which the open path gets by canonicalising.
    let mut link = dir_entry("/r/latest");
    link.kind = EntryKind::Symlink;
    link.is_dir_target = Some(true);
    assert_eq!(enter_action(&link), Some(Enter::Descend));
    link.is_dir_target = Some(false);
    assert_eq!(enter_action(&link), Some(Enter::Open));
    // A dangling link has no target kind; it is not a directory, so it is a
    // file to open, and the open path will report that it could not be read.
    link.is_dir_target = None;
    assert_eq!(enter_action(&link), Some(Enter::Open));
}

/// The frame pump raises the open failure **once**, and never blocks.
///
/// The handle is a single slot, so pressing `Enter` twice cannot stack two
/// attempts — that part is structural, and what is worth testing is the
/// consequence: whichever answer the worker gives, exactly one dialog appears
/// and the slot is empty afterwards, so the next frame is a no-op rather than
/// the same message reappearing sixty times a second.
///
/// The answer itself depends on this machine's `mimeapps.list`, so the
/// assertions are the invariants that hold either way: at most one dialog, and
/// it names the file if it is there.
#[test]
fn the_frame_pump_reports_a_finished_open_at_most_once() {
    let mut app = minimal_app();
    let dir = tempfile::tempdir().expect("tempdir");
    // A file whose type nothing realistically claims a handler for, so the
    // common outcome is the no-handler report. A machine that does claim one
    // simply exercises the "no dialog" half of the same invariant.
    let file = dir.path().join("kestrel-fixture.q7qzx");
    std::fs::write(&file, b"\x00\x01\x02\x03 not text").expect("write");

    app.open_path(&file);
    assert!(app.opening.is_some(), "the attempt is in flight");

    // Pump until the worker answers, or give up after a bounded number of
    // frames. `pump_open` cannot block, so this is a deadline, not a wait.
    // `raised` counts *transitions* into the dialog rather than frames spent
    // with it up: a modal stays up until the user dismisses it, so counting
    // frames would report one per frame and say nothing.
    let mut raised = 0usize;
    for _ in 0..600 {
        let was_open = app.opening.is_some();
        let had_dialog = matches!(app.modal, Some(DlgKind::CannotOpen { .. }));
        app.pump_open();
        if !had_dialog && matches!(app.modal, Some(DlgKind::CannotOpen { .. })) {
            raised += 1;
        }
        if !was_open {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        app.opening.is_none(),
        "the handle is cleared once the worker has answered"
    );
    assert_eq!(raised, 1, "exactly one dialog was raised");

    if let Some(DlgKind::CannotOpen { reason, .. }) = &app.modal {
        assert!(
            reason.contains("kestrel-fixture.q7qzx"),
            "the message names the file: {reason}"
        );
    }
}

/// A file nothing is registered to open produces a dialog naming it, not
/// silence. Driven end-to-end through the real resolver against a `PATH`-less
/// environment is not possible, so this drives the *reporting* half: the error
/// the worker produces, through the same code path a frame runs.
#[test]
fn a_failed_open_raises_a_dialog_that_names_the_file() {
    let mut app = minimal_app();
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("definitely-not-here.kfx");

    // Stand in for the worker's answer. The alternative — a real
    // `kestrel_fs::open::open` — depends on this machine's registrations, and
    // a test that passes on a desktop with every handler registered and fails
    // on a container with none is not a test.
    let err = kestrel_fs::open::OpenError::NoHandler {
        path: missing.clone(),
        mime: "application/x-kestrel-fixture".to_string(),
    };
    let path = err.path().to_path_buf();
    let reason = err.sentence();
    app.modal = Some(DlgKind::CannotOpen { path, reason });

    let Some(DlgKind::CannotOpen { reason, .. }) = &app.modal else {
        panic!("expected a CannotOpen dialog, got {:?}", app.modal);
    };
    assert!(reason.contains("definitely-not-here.kfx"), "{reason}");
    assert!(reason.contains("No application is registered"), "{reason}");
    // §4.7: not `OK`.
    let buttons = dialog::buttons_for(app.modal.as_ref().expect("dialog"));
    assert_eq!(buttons, vec![dialog::Button::Close]);
    assert_ne!(buttons[0].label(), "OK");
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

// -- the pane's width policy -------------------------------------------

/// The width of the right-hand panel, as egui actually painted it.
///
/// Read back off the frame's own shapes rather than off the plan, because the
/// whole bug was that the plan and the painted width were two different things.
/// The app fills the pane with `surface.panel`, and so does the toolbar and the
/// status bar — but those span the window, so the panel is the right-anchored
/// rect that does not.
fn painted_pane_width(out: &egui::FullOutput, app: &KestrelApp, screen_w: f32) -> f32 {
    let panel = app.theme.surfaces.panel;
    out.shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Rect(r) => Some((r.rect, r.fill)),
            _ => None,
        })
        .filter(|(rect, fill)| {
            *fill == panel
                && rect.right() >= screen_w - 1.0
                && rect.width() < screen_w - 4.0
                && rect.width() > 0.0
        })
        .map(|(rect, _)| rect.width())
        .fold(0.0, f32::max)
}

/// Runs one frame on a context the caller owns, and returns the painted pane
/// width.
///
/// [`one_frame`] builds a fresh `egui::Context` every call, and that is exactly
/// what hides this bug: `Panel::outer_size` reads a **persisted** `PanelState`
/// first, so on a brand new context there is nothing persisted and
/// `default_size` wins. A real window has memory persistence on and writes the
/// panel's rect to `app.ron`, so from the second frame — and from the next run
/// — the stored rect wins and `default_size` is dead code. Holding the context
/// across frames is what reproduces that.
fn frame_on(ctx: &egui::Context, app: &mut KestrelApp, at: f64, screen: egui::Vec2) -> f32 {
    let out = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), screen)),
            time: Some(at),
            ..Default::default()
        },
        |ui| {
            app.pump();
            app.draw(ui);
        },
    );
    let width = painted_pane_width(&out, app, screen.x);
    out.drop_without_applying_deltas();
    width
}

/// The settings stepper changes the pane, from the second frame on.
///
/// This is the test for the half of the bug that a screenshot cannot see. The
/// pane is not resizable and was never going to be, but `default_size` is only
/// the *first* frame's guess: egui persists the panel's rect and prefers it over
/// `default_size` from then on, so the app re-asserted its width every frame
/// into a slot nobody read, and moving the stepper in Settings did nothing at
/// all. `exact_size` fixes it by making the range a point, which the persisted
/// value is then clamped into.
///
/// A row is focused throughout, so the plan is [`PanePlan::Full`] and the number
/// under test is the configured one — the rail has its own tests.
#[test]
fn the_pane_follows_the_setting_from_the_second_frame_on() {
    let mut app = minimal_app();
    // One row, focused: the pane has something to describe, so it is the full
    // configured width rather than the rail.
    app.rows = vec![row_at("/r", "/r/a.txt")];
    app.selection.click(0);
    assert!(
        app.has_preview_target(),
        "the fixture row must be previewable"
    );

    let ctx = egui::Context::default();
    crate::tokens::fonts::install(&ctx);
    let screen = egui::vec2(1200.0, 800.0);

    // The first frame has no stored state, so this one *would* have honoured
    // `default_size` too — it is here to make the second frame's result
    // unambiguous rather than to be the assertion.
    let first = frame_on(&ctx, &mut app, 10.0, screen);
    assert!(
        (first - settings::PREVIEW_DEFAULT).abs() <= 4.0,
        "the first frame paints a {first}px pane, not the {}px default",
        settings::PREVIEW_DEFAULT
    );

    // The stepper. Then a second frame, on the *same* context, so egui's stored
    // panel rect is there to be preferred.
    app.preview_width = 340.0;
    let second = frame_on(&ctx, &mut app, 10.05, screen);
    assert!(
        (second - 340.0).abs() <= 4.0,
        "after setting the width to 340 the pane painted at {second}px — the stored \
         panel rect beat the setting"
    );

    // And a third, to show it is not a one-frame fluke of the store.
    app.preview_width = settings::PREVIEW_MIN;
    let third = frame_on(&ctx, &mut app, 10.1, screen);
    assert!(
        (third - settings::PREVIEW_MIN).abs() <= 4.0,
        "after setting the width to {} the pane painted at {third}px",
        settings::PREVIEW_MIN
    );
}

/// The pane policy at a given window width.
///
/// A file manager with three panes in a 640px window is a file manager showing
/// 160px of a name column. The list is the reason the app exists, so it is the
/// last pane to give up space — and this is the rule, as a pure function so it
/// can be checked at every width rather than at the three that were screenshotted.
///
/// **This calls [`preview_plan`] rather than restating it.** The older version
/// of this module re-implemented the policy as `Option<bool>` — shown, shrunk,
/// or impossible — which meant it was checking a *paraphrase* of the app and
/// would have gone on passing if the app changed. It did not go on passing: the
/// paraphrase had no notion of an empty pane at all, so when the rail arrived
/// here was a module that agreed with nothing. Calling the real function is the
/// fix, and it is why the widths below can be asserted to the pixel.
mod pane_policy {
    use super::{LIST_MIN_W, PanePlan, preview_plan};
    use crate::dialog::preview_metrics;
    use crate::tokens::metric;

    /// What a window of `width` shows.
    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Plan {
        sidebar: bool,
        /// `false` when the user has turned the pane off in Settings.
        want_preview: bool,
        /// `None` when the pane is wanted but does not fit at any width.
        preview: Option<PanePlan>,
    }

    impl Plan {
        /// What the pane takes off the window, in pixels.
        ///
        /// `Hidden` and "turned off" both take nothing, which is correct and is
        /// why the two are told apart by `want_preview` rather than by the width
        /// alone.
        fn preview_taken(&self) -> f32 {
            match self.preview {
                None | Some(PanePlan::Hidden) => 0.0,
                Some(PanePlan::Strip { width }) | Some(PanePlan::Full { width }) => width,
            }
        }
    }

    /// The policy, mirroring `KestrelApp::panels` and then delegating.
    ///
    /// `selected` is the app's `has_preview_target`: a row is focused *and*
    /// still on screen. It is `false` for every browser that has not been
    /// clicked, which is the state the rail exists for.
    fn plan(
        width: f32,
        want_sidebar: bool,
        want_preview: bool,
        preview_w: f32,
        selected: bool,
    ) -> Plan {
        let sidebar = want_sidebar && (width - metric::SIDEBAR_WIDTH >= LIST_MIN_W);
        let after = width - if sidebar { metric::SIDEBAR_WIDTH } else { 0.0 };
        let room = after - LIST_MIN_W;
        let preview = want_preview.then(|| preview_plan(room, preview_w, selected));
        Plan {
            sidebar,
            want_preview,
            preview,
        }
    }

    /// The old four-argument shape, for the tests that do not care about
    /// selection.
    fn with_a_selection(width: f32) -> Plan {
        plan(width, true, true, 280.0, true)
    }

    #[test]
    fn a_wide_window_shows_everything_at_the_size_the_user_asked_for() {
        for w in [1200.0, 1000.0, 800.0] {
            let p = with_a_selection(w);
            assert!(p.sidebar, "{w}");
            assert_eq!(p.preview, Some(PanePlan::Full { width: 280.0 }), "{w}");
        }
    }

    #[test]
    fn the_preview_shrinks_before_it_disappears() {
        // 640px: sidebar 200 + list 220 leaves 220 for the preview, which is
        // more than its 180 floor, so it shrinks rather than going.
        let p = with_a_selection(640.0);
        assert!(p.sidebar);
        assert_eq!(
            p.preview,
            Some(PanePlan::Full { width: 220.0 }),
            "it shrinks rather than vanishing"
        );
        // 500px: 500 - 200 = 300, minus the list's 220 is 80, which is under
        // the pane's own floor, so it cannot be shown at any width.
        let p = with_a_selection(500.0);
        assert_eq!(p.preview, Some(PanePlan::Hidden));
    }

    #[test]
    fn the_sidebar_goes_before_the_preview_does() {
        // 420px is the exact crossing: 420 - 200 (the sidebar) leaves the
        // list's 220 and nothing for the preview, so the sidebar stays and the
        // preview cannot. The list is the reason the app exists.
        let p = with_a_selection(420.0);
        assert!(p.sidebar);
        assert_eq!(p.preview, Some(PanePlan::Hidden));
        // 419px: the sidebar no longer fits beside a usable list, so it goes —
        // and the room it vacates is enough for the preview at 199px, which is
        // above its 180 floor. The panes trade places rather than both
        // disappearing, which is the whole point of the order.
        let p = with_a_selection(419.0);
        assert!(!p.sidebar);
        assert_eq!(
            p.preview,
            Some(PanePlan::Full { width: 199.0 }),
            "it comes back, shrunk, in the sidebar's place"
        );
        // 400px: 400 - 220 leaves 180, exactly the preview's floor.
        assert_eq!(
            with_a_selection(400.0).preview,
            Some(PanePlan::Full { width: 180.0 })
        );
        // 399px: under it, so the preview goes as well and the list has the lot.
        assert_eq!(with_a_selection(399.0).preview, Some(PanePlan::Hidden));
    }

    /// The floor is the same for both states, and the crossing is the same
    /// pixel for both.
    ///
    /// 180 is where the pane *starts* being showable; the rail only changes what
    /// it shows once it is there, never whether it is. A rail at 80px would fit
    /// in 100px of room, so this test is the one that says the pane is not
    /// showing itself just because there is nowhere to put it.
    #[test]
    fn the_floor_is_the_same_whether_or_not_a_row_is_selected() {
        let floor = preview_metrics::MIN_WIDTH;
        // Either side of the floor, for both states.
        assert_eq!(
            preview_plan(floor - 0.5, 280.0, false),
            PanePlan::Hidden,
            "an empty pane is not shown below the floor either"
        );
        assert_eq!(
            preview_plan(floor, 280.0, false),
            PanePlan::Strip { width: 80.0 }
        );
        assert_eq!(preview_plan(floor - 0.5, 280.0, true), PanePlan::Hidden);
        assert_eq!(
            preview_plan(floor, 280.0, true),
            PanePlan::Full { width: floor }
        );
    }

    /// With nothing selected the pane is 80px, whatever the window is doing.
    ///
    /// The rail is not a *narrower version* of the pane — it is a different
    /// thing, and the whole point is that it does not scale with the setting the
    /// user chose for a pane they are not looking at.
    #[test]
    fn nothing_selected_gives_the_pane_a_rail_not_a_small_pane() {
        for w in [1600.0, 1200.0, 972.0, 800.0, 640.0, 420.0] {
            let p = plan(w, true, true, 360.0, false);
            let strip = preview_metrics::STRIP_WIDTH;
            match p.preview {
                Some(PanePlan::Strip { width }) => assert_eq!(width, strip, "{w}"),
                // Below the floor the pane is gone, which is the window being
                // narrow, not the row being unselected.
                other => assert_eq!(other, Some(PanePlan::Hidden), "{w}"),
            }
        }
    }

    /// The transition is between two *widths*, at one frame, with no in-between.
    ///
    /// §2.11 rule 1 and rule 2 between them: the width is a pure function of
    /// scalars, recomputed every frame, so there is no state that could
    /// interpolate. This pins the two numbers either side of the decision, which
    /// is the only place that could have hidden an animation.
    #[test]
    fn the_rail_and_the_pane_are_one_frame_apart_and_nothing_else() {
        let empty = preview_plan(600.0, 280.0, false);
        let selected = preview_plan(600.0, 280.0, true);
        assert_eq!(empty, PanePlan::Strip { width: 80.0 });
        assert_eq!(selected, PanePlan::Full { width: 280.0 });
        // The rail is never a pane at its own floor, and the pane is never a
        // rail: the two states are distinguished by more than a size.
        assert_ne!(empty, selected);
    }

    #[test]
    fn the_list_always_gets_its_minimum() {
        for w in (320..=1600).step_by(4) {
            for selected in [true, false] {
                let w = w as f32;
                let p = plan(w, true, true, 280.0, selected);
                let used = if p.sidebar {
                    metric::SIDEBAR_WIDTH
                } else {
                    0.0
                } + p.preview_taken();
                assert!(
                    w - used >= LIST_MIN_W - 0.5,
                    "{w}px (selected: {selected}): panes take {used}, leaving the list {}",
                    w - used
                );
            }
        }
    }

    /// An empty pane must not cost the list the column it was costing it.
    ///
    /// This is the bug, in the form that matters: a 972px window with a 280px
    /// pane left the list ~410px, which is under what the `Modified` timestamp
    /// needs, so the column disappeared — and it disappeared in a window where
    /// there was nothing at all to preview.
    #[test]
    fn an_empty_pane_returns_the_columns_it_was_costing_the_list() {
        let window = 972.0;
        let empty = plan(window, true, true, 280.0, false);
        let full = plan(window, true, true, 280.0, true);
        let list_of = |p: &Plan| {
            window
                - if p.sidebar {
                    metric::SIDEBAR_WIDTH
                } else {
                    0.0
                }
                - p.preview_taken()
        };
        assert!(
            !crate::columns::columns_for(list_of(&full), true).modified,
            "the premise: a full pane does *not* cost the Modified column at {window}px \
             (list {}px), so this test is no longer about anything",
            list_of(&full),
        );
        assert!(
            crate::columns::columns_for(list_of(&empty), true).modified,
            "and the rail must give it back — the list is {}px with the rail against {}px \
             with the pane",
            list_of(&empty),
            list_of(&full),
        );
    }

    #[test]
    fn a_pane_the_user_turned_off_stays_off() {
        for w in [1200.0, 640.0, 420.0] {
            assert!(!plan(w, false, true, 280.0, true).sidebar, "{w}");
            let p = plan(w, true, false, 280.0, true);
            assert!(!p.want_preview, "{w}");
            assert_eq!(p.preview, None, "{w}");
            assert_eq!(p.preview_taken(), 0.0, "{w}");
        }
    }
}

/// External changes refresh a settled listing, and the Kind column appears.
///
/// Regression: `pump` returned early while the scan was settled — which is the
/// steady state of every directory — so the watcher drain and the Kind-column
/// recompute never ran again. `F5` kept working (it bypasses `pump`), which is
/// why nobody noticed the list had gone deaf.
#[test]
fn a_settled_listing_still_hears_the_watcher() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.txt"), b"a").expect("write");
    std::fs::create_dir(dir.path().join("sub")).expect("mkdir");
    let mut app = KestrelApp::assemble(
        Options {
            start_dir: Some(dir.path().to_path_buf()),
            ..Options::default()
        },
        Theme::dark(),
        settings::Stored::default(),
    );
    // Settle the initial scan, bounded: a test must not hang if a worker wedges.
    let mut settled = false;
    for _ in 0..500 {
        app.pump();
        if app.is_listed() {
            settled = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(settled, "the initial scan must settle");
    // One more pump with no scan in flight: the watcher and the columns must
    // run anyway — this is the frame the old early return skipped.
    app.pump();
    let before = app.rows.len();
    assert!(before >= 2, "the fixture must have listed, got {before}");
    // Mixed files and directories earn the Kind column — recomputed after the
    // scan settled, which the early return never reached.
    assert!(
        app.show_kind_column,
        "a mixed listing earns the Kind column, even with no scan in flight"
    );

    // An external change: a file this app did not create.
    std::fs::write(dir.path().join("b.txt"), b"b").expect("write");
    // Bounded polls past the engine's 250 ms debounce: the watcher must
    // notice, `pump_watch` must re-list, and the rows must grow.
    let mut grown = false;
    for _ in 0..400 {
        app.pump();
        if app.rows.len() > before {
            grown = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(
        grown,
        "the watcher must refresh a settled listing (rows stayed at {before})"
    );
}

/// Starting a second operation reports the first instead of dropping it — and
/// joins nothing on the frame thread.
///
/// The replaced job is cancelled and retired; its terminal event becomes a
/// "Stopped" status-bar line (§7.16), and every join happens on a reaper
/// thread. The mechanism is asserted structurally — handle counts and bounded
/// polls — not by timing a wall clock.
#[test]
fn replacing_a_job_reports_the_old_one_without_joining() {
    let src = tempfile::tempdir().expect("tempdir");
    let dst = tempfile::tempdir().expect("tempdir");
    let dst2 = tempfile::tempdir().expect("tempdir");
    std::fs::write(src.path().join("a.txt"), b"new").expect("write");
    std::fs::write(dst.path().join("a.txt"), b"old").expect("write");
    std::fs::write(src.path().join("b.txt"), b"b").expect("write");

    let mut app = minimal_app();
    // Job A parks on a collision question: deterministic, no timing involved.
    app.start_job(
        job::Op::Copy,
        vec![Item::file(src.path().join("a.txt"), Some(3))],
        Some(dst.path().to_path_buf()),
    );
    let mut asked = false;
    for _ in 0..500 {
        app.pump_job();
        if matches!(app.modal, Some(DlgKind::Collision { .. })) {
            asked = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(asked, "job A must park on the collision question");

    // Job B replaces it. This call must retire A's handle, not join its
    // worker: with the old `old.cancel(); old.finish();` this blocked the
    // frame for the length of the in-flight work.
    app.start_job(
        job::Op::Copy,
        vec![Item::file(src.path().join("b.txt"), Some(1))],
        Some(dst2.path().to_path_buf()),
    );
    assert_eq!(
        app.retired_jobs.len(),
        1,
        "the replaced job is retired for draining, not dropped"
    );
    assert!(app.job.is_some(), "the new job is running");
    // The old collision dialog died with the old job: answering it now would
    // write into the *new* job's decision slot, which its worker would take as
    // an answer to a question nobody asked.
    assert!(
        !matches!(app.modal, Some(DlgKind::Collision { .. })),
        "a dialog about the old job must not survive it"
    );

    // Bounded polls: the retired job's worker observes the cancel, its
    // terminal event is drained into a "Stopped" line, and the handle leaves
    // for the reaper thread.
    let mut reported = false;
    for _ in 0..1000 {
        app.pump_job();
        if app.retired_jobs.is_empty() && app.stopped_note.is_some() {
            reported = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(
        reported,
        "the retired job must be drained, reported and detached"
    );
    let (note, _) = app.stopped_note.as_ref().expect("a Stopped line");
    assert!(note.starts_with("Stopped Copy"), "got {note:?}");
    assert_eq!(
        std::fs::read(dst.path().join("a.txt")).expect("read"),
        b"old",
        "the cancelled copy must leave the destination alone"
    );
}

/// Creating over an occupied name is the worker's verdict, not a `stat`.
///
/// Regression: `create_from_field` called `Path::exists()` on the frame
/// thread — a `stat(2)` on a path that may live on a network mount, reached
/// from `ui()`. Now the name goes to the `mkdir` worker unconditionally, which
/// fails with `AlreadyExists` one frame later. So this asserts the synchronous
/// path says nothing at all, then that the verdict still reaches the user.
#[test]
fn create_from_field_sends_an_occupied_name_to_the_worker() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(dir.path().join("taken")).expect("mkdir");
    let mut app = minimal_app();
    app.dir = dir.path().to_path_buf();
    let mut inline = RenameInline::begin(0, &dir.path().join("taken"));
    inline.draft = "taken".to_string();
    app.renaming = Some(inline);
    app.creating = true;
    app.create_from_field();
    assert!(
        app.mkdir_result.is_some(),
        "an occupied name must still go to the worker"
    );
    assert!(
        app.modal.is_none(),
        "no synchronous verdict: the frame thread did not stat"
    );
    // And the worker's answer still surfaces, one frame later. A generous
    // bound: under a fully parallel suite this thread competes with hundreds
    // of others for scheduling.
    let mut failed = false;
    for _ in 0..2000 {
        app.pump_mkdir();
        if matches!(app.modal, Some(DlgKind::Failed { .. })) {
            failed = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(
        failed,
        "the occupied name must still raise a failure dialog"
    );
}
