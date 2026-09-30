//! The application shell: toolbar, sidebar, breadcrumb, column headers, the
//! virtualized file list, and the status bar.
//!
//! # The two rules this file exists to enforce
//!
//! 1. **Nothing in `ui()` blocks, and there is no `std::fs` in the render path
//!    at all.** Directory reads go through [`kestrel_fs::scan::start`] on a
//!    worker; filesystem *changes* through [`kestrel_fs::watcher`] on its own
//!    thread; free space through [`crate::disk::SpaceProbe`], also on a worker.
//!    Symlink targets are resolved *by the scanner* and ride along on
//!    [`kestrel_fs::model::FileEntry::is_dir_target`], so deciding whether
//!    `Enter` descends is a field read, not a `stat`. The only I/O in this file
//!    is `std::fs::write` in the screenshot path, which is not in `ui()`.
//!
//! 2. **No `unwrap`/`expect`/`panic!` in non-test code.** Every fallible call —
//!    `scan::start`, `watcher::watch`, `Response::clicked`, `Vec::get` — is
//!    matched explicitly.
//!
//! # Layout
//!
//! egui 0.36 removed `SidePanel`/`TopBottomPanel`; panels are
//! [`egui::Panel::left`]/`::top`/`::bottom`, and the `CentralPanel` goes **last**
//! so it claims the rectangle the others left.
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────────┐
//! │  toolbar   (Panel::top, 34px + hairline)                     │
//! ├────────────┬─────────────────────────────────────────────────┤
//! │  sidebar   │  breadcrumb  (Panel::top, 28px + hairline)       │
//! │  Places    ├─────────────────────────────────────────────────┤
//! │            │  column headers (24px)                          │
//! │            ├─────────────────────────────────────────────────┤
//! │            │  file list  (CentralPanel, virtualized)         │
//! ├────────────┴─────────────────────────────────────────────────┤
//! │  status bar  (Panel::bottom, 24px + hairline)                │
//! └──────────────────────────────────────────────────────────────┘
//! ```
//!
//! Every sticky band is a **layout sibling with a 1px border**, never an
//! overlay (§2.12). That is what makes WCAG 2.2 "focus not obscured" a
//! structural property rather than something a scroll offset has to remember.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use egui::{Align, Align2, Key, Layout, Rect, RichText, ScrollArea, Sense, Stroke, Ui, pos2, vec2};
use kestrel_fs::error::ScanError;
use kestrel_fs::model::{EntryKind, FileEntry, SortKey, SortSpec};
use kestrel_fs::scan::{ScanEvent, ScanHandle, ScanOptions};
use kestrel_fs::watcher::{WatchEvent, WatchSubscription};

use crate::clipboard::{Clipboard, Op};
use crate::columns::{self, ColumnLayout, ColumnSet};
use crate::dialog::{self, Button as DlgButton, Kind as DlgKind};
use crate::disk::SpaceProbe;
use crate::filetype::{self, Classified};
use crate::format;
use crate::history::History;
use crate::icons;
use crate::job::{self, Item, JobEvent, JobHandle};
use crate::motion::Motion;
use crate::places::{self, Place};
use crate::preview::{self as pv, Kind as PvKind, Loaded as PvLoaded, Loader as PvLoader};
use crate::rename::Inline as RenameInline;
use crate::selection::Selection;
use crate::settings;
use crate::states;
use crate::tokens::{
    self, Theme, ThemeMode, border, component, metric, radius, space, system_preference, ty,
    with_alpha,
};

/// The width the status bar's trailing section needs, for the path to avoid it.
///
/// The free-space meter's `metric.scrollbar`-plus-label, measured with the fonts
/// that will draw it. Measured here rather than counted because the label is a
/// formatted byte count whose length depends on the disk, and a fixed reserve
/// either wastes 40px on a small disk or overlaps on a full one.
fn trailing_status_width(ui: &Ui, theme: &Theme, space: Option<crate::disk::FreeSpace>) -> f32 {
    // No reading yet: the section is absent rather than showing a fabricated
    // number, so nothing is reserved for it.
    let Some(space) = space else {
        return 0.0;
    };
    let label = format!("{} free", space.format_free());
    component::STATUSBAR_METER_W
        + space::S2
        + widgets::text_width(ui, &label, tokens::font(component::STATUSBAR_LABEL, theme))
}

/// The path, middle-truncated to the width left after the trailing section.
///
/// `0` when the trailing section has the whole bar, and a single `…` rather
/// than nothing: a path that is not shown at all is a status bar with no
/// position in it, which is the thing §4.5 says the bar is for.
fn truncate_path(ui: &Ui, path: &str, reserved: f32) -> String {
    let budget = (ui.available_width() - reserved - space::S3).max(0.0);
    if budget <= 0.0 {
        return "\u{2026}".to_string();
    }
    // `type.meta` is 12px in Plex **Mono**, whose advance is a fixed 7.2px, so
    // the column count is exact here — the same property that makes the
    // preview's code lines right and the file list's names slightly wrong.
    let cols = (budget / 7.2).floor().max(1.0) as usize;
    format::middle_truncate(path, cols)
}

/// The file list's own minimum width, in logical pixels.
///
/// `metric.column-name-min` (160) plus `metric.gutter` (10) plus a size column
/// that the header can still show an ellipsis in (50). The list is the reason
/// the app exists — a sidebar and a preview pane are both optional views of
/// what is in it — so it is the last thing to give up space and the first to
/// get it back.
const LIST_MIN_W: f32 = 220.0;

/// How long a directory's contents take to arrive.
///
/// §2.11 names `motion.deliberate` (380ms) for "directory content cross-fade on
/// navigation", and this uses half of that. The deviation is deliberate and is
/// the one motion decision this app makes against its own spec:
///
/// * 380ms is longer than `motion.base` (130ms), which §2.11 rule 3 caps for
///   anything the pointer touches — and navigation is a pointer action in the
///   three cases that matter most (double-click a folder, click a breadcrumb,
///   click the overflow menu's ancestor). 380ms of "where did it go" after a
///   double-click reads as lag, which is the exact thing §2.11 rule 1 is written
///   to prevent for selection.
/// * the fade is opacity over a background of the same colour, so there is
///   nothing to interpolate except the blend. A 380ms version of that is a long
///   time to look at a list at 60% opacity.
///
/// §2.11's ceiling still holds: this is the *only* animated duration in the
/// shell, and it is 90ms.
const NAV_FADE: std::time::Duration = tokens::motion::FAST;
use crate::toolbar::{self, Action};
use crate::widgets;

/// Which screen the app is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// The real file manager shell.
    Browser,
    /// `--gallery`: every §4 component at every state, for design review.
    Gallery,
    /// The settings screen. A *screen*, not a modal: it replaces the browser
    /// rather than covering it, so the browser behind it keeps its scroll
    /// position and its selection, and coming back is instant rather than a
    /// redraw of a window that was there all along.
    Settings,
}

/// A starting state for a headless `--screenshot` capture.
///
/// # Why the capture path needs this
///
/// The dialogs and the populated preview pane are **states**, not screens: they
/// exist only as a result of a keystroke or a background answer, and there is no
/// flag that reaches them. Without a way to *put the app into* them, the only
/// way to review §4.7 is to reproduce the operation by hand in a live window and
/// hope the machine is idle — which is precisely the "screenshot grabbed off the
/// desktop" failure [`crate::shot`] exists to avoid.
///
/// So the states are named here, applied to a real [`KestrelApp`], and rendered
/// through the same [`KestrelApp::draw`] the window runs. Nothing about the
/// paint path is special-cased for a capture; a scene is just the app's state
/// before the first frame.
///
/// Every variant is something a user can actually reach. There is no
/// `Scene::BrokenLayout` that only exists to make a screenshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scene {
    /// The default: a listing, nothing selected, no dialog.
    #[default]
    Browser,
    /// §4.7 `Delete Permanently` for three items, with a quoted path.
    ConfirmPermanent,
    /// §4.7 `Move to Trash` for three items — the reversible default.
    ConfirmTrash,
    /// §4.7 A collision the filesystem asked about: five buttons, both scopes.
    Collision,
    /// §4.7 A running copy with a Stop button.
    Progress,
    /// §4.7 A job that stopped halfway.
    Failed,
    /// `Enter` on a file with no registered application.
    ///
    /// The state that was a dead end until the open path existed: the key was
    /// consumed, nothing happened, and there was no message explaining why.
    CannotOpen,
    /// The preview pane's empty state, centred.
    PreviewEmpty,
    /// The preview pane on a text file, syntax-highlighted.
    PreviewText,
    /// The preview pane on an image.
    PreviewImage,
    /// The preview pane's "too large" degradation.
    PreviewTooLarge,
    /// The settings screen, at its default state.
    Settings,
    /// The keyboard shortcut overlay over the browser.
    Help,
    /// An empty directory: §4.2's three-part empty state.
    Empty,
    /// A directory the process may not read.
    Denied,
    /// A directory that is no longer there.
    Gone,
    /// A directory that lists fine, with the watcher off.
    ///
    /// The degradation the engine documents: `KestrelError::Watch` happens when
    /// inotify's per-user watch limit is hit on a large tree, and it must show
    /// as a *visible* manual-refresh affordance rather than a log line.
    NoWatch,
}

/// What the preview pane needs about the focused row.
///
/// A small owned copy rather than a borrow, so requesting a load does not alias
/// `&mut self`. Three fields; cloning a `PathBuf` per frame is cheaper than the
/// alternative, which is a `RefCell`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PreviewTarget {
    /// The file to preview.
    path: PathBuf,
    /// Its size, for the cap.
    size: Option<u64>,
    /// Whether it is an image, so the loaders decode rather than the highlighter.
    is_image: bool,
}

/// What a confirm dialog has decided to do, once the user says yes.
///
/// Held on the app rather than inside the dialog so the dialog is plain data —
/// a `Kind` that had to carry a closure would be a `Kind` that could not be
/// compared, logged, or rendered by the `--screenshot` path.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pending {
    /// The operation.
    op: job::Op,
    /// Where it goes, for copy and move.
    destination: Option<PathBuf>,
    /// What it operates on.
    items: Vec<Item>,
}

/// A job the user replaced before it ended: cancelled, still draining.
///
/// Carries the progress snapshot from the moment of replacement so the terminal
/// event can be reported with exact numbers — `JobEvent::Cancelled` knows
/// `done` but not `total`, and the live [`JobProgress`] already belongs to the
/// newer job.
#[derive(Debug)]
struct RetiredJob {
    /// The cancelled handle. Never dropped on the frame thread: see
    /// [`detach_job`].
    handle: JobHandle,
    /// Progress as last observed, updated by each reap's drain.
    progress: JobProgress,
}

/// Joins a finished job handle off the frame thread.
///
/// `JobHandle::drop` joins, so even a handle whose worker has already sent its
/// terminal event must not be dropped where a frame is being built: the join
/// waits for thread exit, and thread exit is the worker's business, not the
/// frame's. The reaper is one transient thread per finished job — jobs end
/// rarely, and a parked-forever thread would be the worse trade.
///
/// If the spawn itself fails the closure (and the handle with it) is dropped
/// inline: one hitched frame rather than a leaked worker thread.
fn detach_job(handle: JobHandle) {
    if std::thread::Builder::new()
        .name("kestrel-reap".to_string())
        .spawn(move || handle.finish())
        .is_err()
    {
        log::warn!("job: could not spawn a reaper; joined on the frame thread");
    }
}

/// Drops a replaced watcher off the frame thread.
///
/// `DirWatcher::drop` joins the debouncer thread, which can sleep up to one
/// 25 ms poll before it notices the cancel flag — a hitch the frame loop must
/// never take. Same spawn-failure trade as [`detach_job`]: the closure owns
/// the subscription, so a failed spawn drops it inline rather than leaking the
/// inotify watch.
fn retire_watch(sub: WatchSubscription) {
    if std::thread::Builder::new()
        .name("kestrel-reap".to_string())
        .spawn(move || drop(sub))
        .is_err()
    {
        log::warn!("watch: could not spawn a reaper; joined on the frame thread");
    }
}

/// The status-bar line for a replaced job's ending.
///
/// Short, because the status bar at 500px is already carrying an item count, a
/// path and a free-space meter: the numbers are what the job actually did, not
/// what it was asked to do.
fn report_retired(progress: &JobProgress, event: &JobEvent) -> String {
    match event {
        JobEvent::Cancelled { done, bytes } => {
            let mut line = format!(
                "Stopped {}",
                job::summary(progress.op, *done, progress.total)
            );
            if *bytes > 0 {
                line.push_str(&format!(" · {}", format::bytes(*bytes)));
            }
            line
        }
        JobEvent::Done { done, .. } => {
            // The cancel landed after the last byte: the job finished, and
            // saying "Stopped" about it would be the lie in the other direction.
            format!(
                "{} — finished after a newer job started",
                job::summary(progress.op, *done, progress.total)
            )
        }
        JobEvent::Failed { message, .. } => {
            format!(
                "{} — failed: {message}",
                job::summary(progress.op, progress.done, progress.total)
            )
        }
        _ => format!(
            "Stopped {}",
            job::summary(progress.op, progress.done, progress.total)
        ),
    }
}

/// A running job's progress, for the status bar and the progress dialog.
///
/// The clock is started when the job is, not when the first item lands, so the
/// rate reflects the user's experience — including the time spent waiting for the
/// first collision answer, which is real elapsed time.
#[derive(Debug, Clone, PartialEq)]
struct JobProgress {
    /// Which operation.
    op: job::Op,
    /// Items finished.
    done: usize,
    /// Items total, once the job has reported its start.
    total: usize,
    /// Bytes accounted for.
    bytes: u64,
    /// Bytes known to be in total, or `None` when no size walk ran.
    ///
    /// `None` is the honest answer for a directory: the listing's size for a
    /// directory is its own `lstat` length, not a recursive total, and a
    /// progress bar built on that number would be fiction.
    total_bytes: Option<u64>,
    /// Items the user chose to skip.
    skipped: usize,
    /// The item currently being worked on.
    current: PathBuf,
    /// When the job started.
    started: Instant,
    /// Set when the job has sent its terminal event.
    finished: bool,
}

impl JobProgress {
    /// Fresh progress for a job about to start.
    fn new(op: job::Op, total: usize) -> Self {
        Self {
            op,
            done: 0,
            total,
            bytes: 0,
            total_bytes: None,
            skipped: 0,
            started: Instant::now(),
            current: PathBuf::new(),
            finished: false,
        }
    }

    /// How long the job has been running.
    fn elapsed(&self) -> std::time::Duration {
        self.started.elapsed()
    }

    /// `true` once the job has ended.
    fn is_finished(&self) -> bool {
        self.finished
    }

    /// The status-bar summary, e.g. `Copy 3 of 9 · 12 items/s · 1.2 MiB`.
    fn summary(&self) -> String {
        let mut parts = vec![job::summary(self.op, self.done, self.total)];
        let rate = job::format_rate(self.done, self.elapsed());
        if !rate.is_empty() {
            parts.push(rate);
        }
        if self.bytes > 0 {
            parts.push(format::bytes(self.bytes));
        }
        if self.skipped > 0 {
            parts.push(format::plural(self.skipped, "skipped", "skipped"));
        }
        parts.join(" \u{b7} ")
    }
}

/// One line in the list, with its indentation depth.
///
/// Depth is carried on the row rather than re-derived at paint time. The Phase 3
/// first attempt had a `tree_depth(index) -> usize` helper that returned
/// `0` for row 0 and `1` for everything else: a "tree" that indented all but the
/// first row by one step and so looked like a feature while being a lie. Depth is
/// a property the *scanner* knows, not something the view index can supply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The engine's entry.
    pub entry: FileEntry,
    /// Indentation depth, 0 for a child of the listed directory.
    pub depth: usize,
}

impl Row {
    /// The entry, for the many call sites that only care about the file.
    #[must_use]
    pub fn entry(&self) -> &FileEntry {
        &self.entry
    }

    /// `true` when this row is a directory the tree can expand.
    #[must_use]
    pub fn is_expandable(&self) -> bool {
        self.entry.is_descendable()
    }
}

/// How rows are arranged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    /// A flat listing of the current directory. §4.11 `Ctrl+1`.
    #[default]
    List,
    /// Nested, with `row.indent-step` per depth level. §4.11 `Ctrl+2`.
    Tree,
}

/// Command-line options, parsed once in `main`.
#[derive(Debug, Clone)]
pub struct Options {
    /// Show the design-system gallery instead of the shell.
    pub gallery: bool,
    /// `--light` / `--dark` force a mode; otherwise follow the system.
    pub theme: ThemeMode,
    /// An explicit start directory. `None` falls back to `$KESTREL_HOME`, then
    /// `$HOME`, then `/`.
    pub start_dir: Option<PathBuf>,
    /// Start in tree view.
    pub tree: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            gallery: false,
            theme: ThemeMode::System,
            start_dir: None,
            tree: false,
        }
    }
}

/// The app. One instance per viewport.
pub struct KestrelApp {
    /// Which screen is showing.
    screen: Screen,
    /// The user's theme preference (may be `System`).
    mode: ThemeMode,
    /// The theme resolved for the current frame.
    theme: Theme,
    /// The resolved motion preference (§2.11 rule 5).
    motion: Motion,
    /// `$HOME`-relative places, resolved once at construction.
    places: Vec<Place>,
    /// The directory currently listed.
    dir: PathBuf,
    /// Rows delivered by the scan, already sorted by the engine, in the engine's
    /// depth-first order so a parent always precedes its children.
    rows: Vec<Row>,
    /// Directories whose children are visible, in tree mode.
    ///
    /// Expanding is a **pure filter** over rows the scan has already delivered,
    /// so it performs no I/O at all. See [`KestrelApp::tree_is_visible`].
    expanded: BTreeSet<PathBuf>,
    /// Paths the scan could not read. Reported as data, never as a dialog.
    errors: Vec<ScanError>,
    /// The in-flight scan, or `None` once the last one finished.
    scan: Option<ScanHandle>,
    /// The debounced watcher for [`Self::dir`].
    watch: Option<WatchSubscription>,
    /// Why the watcher is not running, when it is not.
    ///
    /// `KestrelError::Watch` is what the engine returns when the platform's
    /// watch limit is hit — a real outcome on a large tree, and one the user
    /// has to be able to see, because a file manager whose list silently stops
    /// updating is *lying* about the directory. The status bar carries it as a
    /// permanent section with a Refresh button beside it, which is §7.16's
    /// "never a silent status change" and the engine's documented degradation
    /// to manual refresh.
    watch_error: Option<String>,
    /// Back/forward stacks.
    history: History,
    /// The §2.11 cross-fade for a directory change, as egui seconds.
    ///
    /// `None` when there is nothing to fade. Set from the *input* clock rather
    /// than [`Instant`] for the reason `shot` exists: a capture advances time
    /// deterministically and a fade driven by the wall clock would be caught at
    /// progress 0 in every screenshot, which is a list rendered as an empty
    /// rectangle.
    nav_fade: Option<f64>,
    /// A navigation is waiting for its listing; the fade starts when it lands.
    ///
    /// Two fields rather than one `Option<Instant>` because the two events are
    /// genuinely different: the *navigation* happens on the frame the user
    /// presses Enter, and the *listing* happens whenever the scan finishes,
    /// which for a 50,000-row directory on a cold cache is a fifth of a second
    /// later. Fading from the keystroke would fade an empty pane and then snap
    /// to the content.
    nav_pending: bool,
    /// The user's multi-selection.
    selection: Selection,
    /// Staged cut/copy.
    clipboard: Clipboard,
    /// The sort handed to the engine for each directory.
    sort: SortSpec,
    /// Which sort column is active, for the header indicator.
    sort_column: SortKey,
    /// List or tree.
    view: ViewMode,
    /// Whether the listing is mixed enough to justify the Kind column.
    show_kind_column: bool,
    /// The visible scroll offset, in rows. Surfaced so `Home`/`End` and
    /// scroll-to-focused can work — WCAG 2.2 focus-not-obscured.
    scroll_rows: usize,
    /// Background free-space probe.
    space: SpaceProbe,
    /// How many rows fit in the list viewport, measured each frame. The keyboard
    /// path needs it to scroll the focused row into view, and a keyboard handler
    /// has no `Ui` to ask.
    viewport_rows: usize,
    /// The live filter query; empty means no filter.
    filter: String,
    /// `true` while the filter field has focus.
    filter_focused: bool,
    /// The running file operation, or `None` when idle.
    job: Option<JobHandle>,
    /// Progress for the running job, for the dialog and the status bar.
    job_progress: Option<JobProgress>,
    /// What a confirm dialog is confirming, resolved when the user answers.
    pending: Option<Pending>,
    /// The modal on top, if any. Only ever one — a dialog about a dialog is how
    /// a user loses the thread.
    modal: Option<DlgKind>,
    /// The button the keyboard is on, for the dialog's focus ring.
    modal_focus: usize,
    /// Inline rename in progress, if any.
    renaming: Option<RenameInline>,
    /// `true` when the inline field is creating a new folder rather than renaming
    /// an existing one, which changes what Enter does.
    creating: bool,
    /// A `mkdir` in flight, so its answer can be collected without blocking.
    mkdir_result:
        Option<std::sync::mpsc::Receiver<std::result::Result<(), kestrel_fs::error::KestrelError>>>,
    /// The preview pane's loader.
    preview: PvLoader,
    /// Whether the preview pane is shown.
    show_preview: bool,
    /// The preview pane's width, from the settings screen.
    ///
    /// The preview pane's own `default_size` is only ever the *first* frame's
    /// guess: `Panel::outer_size` (egui 0.36.2, `panel.rs:1064`) reads a
    /// persisted `PanelState` **first** and only falls back to `default_size`
    /// when there is none — and egui's memory persistence is on by default, so
    /// from the second frame onwards, and across restarts, the stored rect wins.
    /// Re-asserting `.default_size(..)` every frame is therefore a no-op, which
    /// is why the settings stepper could be moved and nothing happened: the only
    /// thing that ever moved the pane was dragging its separator.
    ///
    /// `Panel::exact_size` is what actually holds the line, and not for the
    /// reason it reads like it should: it does not make the default win, it sets
    /// `outer_size_range = Rangef::point(size)`, and the loaded `PanelState` is
    /// then *clamped into that point*. The stored value is still read; it just
    /// has nowhere to go.
    preview_width: f32,
    /// Whether the Places sidebar is shown.
    show_sidebar: bool,
    /// Whether dotfiles are listed.
    show_hidden: bool,
    /// The preview's last content, so it survives a redraw.
    preview_content: Option<PvLoaded>,
    /// Rolling frame-time average in ms, for the status bar.
    frame_ms: f32,
    /// Frames observed so far, used to seed the average.
    frames: u32,
    /// What the settings screen persists, and what it edits live.
    ///
    /// One value, not a copy: the screen writes straight into the field the
    /// app reads, so "the setting on screen" and "the setting the app is
    /// using" cannot be two numbers that drift.
    settings: settings::Stored,
    /// `true` when the toolbar could not fit the icon row, so the window is
    /// narrower than the app's own minimum and buttons are being clipped.
    ///
    /// Read by the status bar. Not a dialog: the user did not do anything wrong
    /// and there is nothing to decide, so it is a fact, and §4.5 is where facts
    /// live.
    toolbar_narrow: bool,
    /// `true` when [`Self::settings`] has been edited since the last save.
    ///
    /// Only affects *when* the state is written, never *whether*: `save` runs
    /// on eframe's interval and on exit either way.
    settings_dirty: bool,
    /// The keyboard shortcut overlay, per §4.11's discoverability contract.
    help: bool,
    /// A pane the settings asked for did not fit and was dropped this frame.
    ///
    /// §7.16: a change in what the app is showing is a status change, and a
    /// status change that is not announced is the same defect as the watcher's
    /// failure being a log line.
    pane_too_narrow: bool,
    /// An `Enter`-on-a-file in flight, if any.
    ///
    /// `None` for the overwhelming majority of frames, which is why this is a
    /// handle and not a state: the launch runs on a worker and the only work
    /// per frame is one non-blocking [`kestrel_fs::open::Opening::poll`].
    opening: Option<kestrel_fs::open::Opening>,
    /// Jobs the user replaced before they ended.
    ///
    /// A replaced job is cancelled but **not** joined here — [`JobHandle::drop`]
    /// joins, so dropping it on the frame thread would block for the length of
    /// the in-flight work. Retired jobs are drained every frame in
    /// [`KestrelApp::pump_job`]; once a terminal event has been observed the
    /// handle moves to a reaper thread for the join, and the job is reported
    /// as a "Stopped" status-bar line rather than discarded silently (§7.16).
    retired_jobs: Vec<RetiredJob>,
    /// What a replaced job reported when it ended, with when it ended.
    ///
    /// Rendered in the status bar until it ages out (twenty seconds) — long
    /// enough to read, short enough not to become stale furniture.
    stopped_note: Option<(String, Instant)>,
    /// Row count the Kind column was last computed for.
    ///
    /// [`KestrelApp::pump_columns`] samples the whole listing, so it only runs
    /// when the count changed rather than on every frame of a 50,000-row
    /// directory.
    kind_rows: usize,
}

impl KestrelApp {
    /// Builds the app from an eframe `CreationContext`.
    ///
    /// The two I/O calls — `scan::start` and `watcher::watch` — both hand their
    /// work to a thread and return a handle, so neither can block the first
    /// frame. Font installation happens even earlier, in the `AppCreator`.
    #[must_use]
    pub fn new(cc: &eframe::CreationContext<'_>, opts: Options) -> Self {
        // egui tracks the OS theme preference itself, and its answer is
        // available from the `Context` at construction time. `ui()` refines this
        // against the live winit window every frame.
        let system_dark = if opts.theme == ThemeMode::System {
            Some(matches!(
                cc.egui_ctx.options(|o| o.theme_preference),
                egui::ThemePreference::Dark
            ))
        } else {
            None
        };
        let theme = Theme::resolve(opts.theme, system_dark);
        theme.apply(&cc.egui_ctx);
        // Read the stored preferences *before* `assemble`, because the theme is
        // one of them and getting the first frame wrong means a window that
        // flashes the wrong theme before settling. `storage()` is an in-memory
        // map eframe loaded before the first frame, so this is not I/O.
        let stored = settings::load(cc.storage);
        let mut opts = opts;
        if stored.theme != settings::Mode::System {
            // A stored preference beats a `--light`/`--dark` flag: the flag is a
            // one-off for a screenshot or a bug report, the preference is what
            // the user chose. A `--light` flag on a machine whose saved
            // preference is dark therefore *loses*, and `--light` alone on a
            // machine with no saved preference still wins over the stored
            // `System`.
            opts.theme = stored.theme.to_theme();
        }
        let theme = Theme::resolve(opts.theme, system_dark.or(Some(theme.is_dark)));
        theme.apply(&cc.egui_ctx);
        // egui's built-in id-instability check, debug builds only: fires when
        // the same screen rect is claimed by different widget `Id`s across
        // passes. A diagnostic, not a fix — findings are reported, not
        // silenced.
        #[cfg(debug_assertions)]
        cc.egui_ctx
            .all_styles_mut(|s| s.debug.warn_if_rect_changes_id = true);
        Self::assemble(opts, theme, stored)
    }

    /// Builds the app against a bare [`egui::Context`], for the headless
    /// `--screenshot` path.
    ///
    /// A [`eframe::CreationContext`] cannot be constructed outside eframe, so
    /// the two things `new` takes from it are supplied directly. Everything
    /// else, including the initial scan of the start directory, is identical, so
    /// a capture shows the same app a real window would.
    #[must_use]
    pub fn for_capture(ctx: egui::Context, opts: Options) -> Self {
        // `Some(true)`: a capture is deterministic by construction, and a
        // screenshot that changes colour with the hour is not reviewable.
        let theme = Theme::resolve(opts.theme, Some(true));
        theme.apply(&ctx);
        // Same id-instability diagnostic as `new`: a capture that silently
        // reuses one rect for two widget ids reviews a layout bug as a theme.
        #[cfg(debug_assertions)]
        ctx.all_styles_mut(|s| s.debug.warn_if_rect_changes_id = true);
        // Defaults, never the stored state: a capture must be reproducible, and
        // a screenshot that changed colour because a preferences file did is
        // not reviewable. `--light`/`--dark` is how a capture picks a theme.
        let theme_pref = match opts.theme {
            ThemeMode::Light => settings::Mode::Light,
            ThemeMode::Dark => settings::Mode::Dark,
            ThemeMode::System => settings::Mode::System,
        };
        let stored = settings::Stored {
            theme: theme_pref,
            ..Default::default()
        };
        Self::assemble(opts, theme, stored)
    }

    /// The shared construction path, once the theme is settled.
    fn assemble(opts: Options, theme: Theme, settings: settings::Stored) -> Self {
        let dir = opts.start_dir.clone().unwrap_or_else(places::start_dir);
        let view = if opts.tree {
            ViewMode::Tree
        } else {
            ViewMode::List
        };
        let mut app = Self {
            screen: if opts.gallery {
                Screen::Gallery
            } else {
                Screen::Browser
            },
            mode: opts.theme,
            theme,
            motion: crate::motion::detect(),
            places: places::places(),
            dir: dir.clone(),
            rows: Vec::new(),
            expanded: BTreeSet::new(),
            errors: Vec::new(),
            scan: None,
            watch: None,
            watch_error: None,
            history: History::new(&dir),
            nav_fade: None,
            nav_pending: false,
            selection: Selection::new(),
            clipboard: Clipboard::new(),
            sort: SortSpec {
                key: SortKey::Name,
                ascending: true,
                dirs_first: true,
            },
            sort_column: SortKey::Name,
            view,
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
            settings,
            settings_dirty: false,
            help: false,
            pane_too_narrow: false,
            opening: None,
        };
        app.apply_settings();
        app.open_dir(dir);
        app
    }

    /// Pushes [`Self::settings`] into the fields the app actually reads.
    ///
    /// Called once at construction and again whenever the settings screen
    /// closes. The indirection exists because a setting has two homes — the
    /// persisted struct and the live field — and the moment there are two homes
    /// is the moment something has to say which way the copy goes. Here it is
    /// the settings, always: the app never writes into `self.settings`, so a
    /// live change the app makes (cycling the theme with `Ctrl+T`, say) is
    /// picked up the next time the screen opens rather than being silently
    /// discarded.
    fn apply_settings(&mut self) {
        self.mode = self.settings.theme.to_theme();
        self.show_hidden = self.settings.show_hidden;
        self.show_preview = self.settings.show_preview;
        self.preview_width = self.settings.preview_width;
        self.show_sidebar = self.settings.show_sidebar;
        self.sort = self.settings.sort_spec();
        self.sort_column = self.settings.sort.to_sort_key();
        self.view = match self.settings.view {
            settings::View::List => ViewMode::List,
            settings::View::Tree => ViewMode::Tree,
        };
    }

    // -- Capture scenes -----------------------------------------------------

    /// Puts the app into `scene`, given the directory it is listing.
    ///
    /// The scenes that raise a dialog go through the *real* entry points
    /// ([`KestrelApp::begin`]) rather than assigning [`DlgKind`] directly, so a
    /// capture cannot show a dialog the app would never actually build. The two
    /// the app only raises from a background worker — a collision and a failure —
    /// are constructed directly, because reproducing them for real would mean
    /// copying files and deleting them on the reviewer's machine to look at a
    /// PNG.
    pub fn apply_scene(&mut self, scene: Scene) {
        match scene {
            // `Browser` is "no scene", and `PreviewEmpty` is "no scene" too: it
            // is the *absence* of a selection, which is what the browser scene
            // already is.
            Scene::Browser | Scene::PreviewEmpty => {}
            Scene::ConfirmPermanent => {
                self.select_fixture_sample();
                self.begin(dialog::op_for_delete(true), None);
            }
            Scene::ConfirmTrash => {
                self.select_fixture_sample();
                self.begin(dialog::op_for_delete(false), None);
            }
            Scene::Collision => {
                let src = self.dir.join("report.txt");
                let dst = self.dir.join("report (copy).txt");
                self.modal = Some(DlgKind::Collision {
                    src,
                    dst,
                    remaining: 3,
                });
                self.modal_focus = 0;
            }
            Scene::Progress => {
                self.modal = Some(DlgKind::Progress {
                    op: job::Op::Copy,
                    done: 7,
                    total: 24,
                    bytes: 48 * 1024 * 1024,
                    total_bytes: Some(310 * 1024 * 1024),
                    current: self.dir.join("assets"),
                    rate: "12 items/s".to_string(),
                });
                self.modal_focus = 0;
            }
            Scene::Failed => {
                self.modal = Some(DlgKind::Failed {
                    op: job::Op::Move,
                    path: self.dir.join("locked"),
                    reason: "Permission denied (os error 13)".to_string(),
                });
                self.modal_focus = 0;
            }
            // The message a user gets for pressing Enter on a file nothing is
            // registered to open. Built from the real error type rather than
            // written as a literal, so the capture cannot drift from what the
            // app says.
            Scene::CannotOpen => {
                let path = self.dir.join("archive.kfx");
                let reason = kestrel_fs::open::OpenError::NoHandler {
                    path: path.clone(),
                    mime: "application/x-kestrel-fixture".to_string(),
                }
                .sentence();
                self.modal = Some(DlgKind::CannotOpen { path, reason });
                self.modal_focus = 0;
            }
            Scene::PreviewText => {
                self.focus_named("notes.md");
            }
            Scene::PreviewImage => {
                self.focus_named("diagram.png");
            }
            Scene::PreviewTooLarge => {
                self.focus_named("archive.tar");
            }
            Scene::Settings => {
                // Through the real entry point, so a capture cannot show a
                // screen the app would never actually reach.
                self.open_settings();
            }
            Scene::Help => {
                self.help = true;
            }
            // The next four are states the filesystem decides, not keystrokes:
            // reproducing a permission-denied directory or a dead watcher for
            // real would mean chmod-ing a fixture and filling an inotify quota
            // on the reviewer's machine to look at a PNG. The errors are
            // constructed from the engine's own types, so the scene exercises
            // the same `states::State::for_listing` the live path does.
            Scene::Empty => {
                // The fixture already has rows; an empty state is a listing with
                // none, which is the same app with a different answer from the
                // scanner. Simulated rather than faked: the scene clears the
                // delivered rows and marks the scan finished.
                self.rows.clear();
                self.scan = None;
            }
            Scene::Denied | Scene::Gone => {
                self.rows.clear();
                self.scan = None;
                let err = if scene == Scene::Denied {
                    kestrel_fs::error::KestrelError::PermissionDenied {
                        path: self.dir.clone(),
                        source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
                    }
                } else {
                    kestrel_fs::error::KestrelError::not_found(&self.dir)
                };
                self.errors = vec![ScanError::new(self.dir.clone(), err)];
            }
            Scene::NoWatch => {
                self.watch = None;
                self.watch_error = Some(
                    "The filesystem watcher could not be started, so changes here will not appear on their own."
                        .to_string(),
                );
            }
        }
    }

    /// What the preview pane currently holds, for a test or a capture that has
    /// to know whether content landed rather than merely that nothing hung.
    #[must_use]
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn preview_state(&self) -> Option<&PvLoaded> {
        self.preview_content.as_ref()
    }

    /// `true` once the initial scan has finished delivering.
    ///
    /// The engine keeps the handle until `Complete`, so this is the honest
    /// "the listing is here" signal. It says nothing about whether the listing is
    /// *empty*, and must not: an empty directory is a settled directory, and a
    /// check that conflated the two would hang a capture of one forever.
    #[must_use]
    pub fn is_listed(&self) -> bool {
        self.scan.is_none()
    }

    /// `true` once the app has nothing left in flight for a capture to wait on.
    ///
    /// A capture has to be *settled*, not merely *drawn*: a PNG of a preview
    /// pane that still says "Loading…" reviews nothing. The scan and the preview
    /// loader are both non-blocking by design, so the honest way to wait for them
    /// is to keep running frames until they say they are done — never to sleep on
    /// a guessed duration, and never to block the worker.
    #[must_use]
    pub fn is_settled(&self) -> bool {
        if !self.is_listed() || self.preview.is_busy() {
            return false;
        }
        match self.focused_preview_target() {
            // Nothing to preview: there is nothing to wait for.
            None => true,
            Some(target) => {
                // A non-viewable target (a directory, a file with no size, an
                // oversized file) never queues a load at all, so "no content has
                // landed" is the *correct* final state, not a hang.
                !pv::kind_for(target.is_image, target.size).is_viewable()
                    || self.preview_content.is_some()
            }
        }
    }

    /// Selects the three fixture rows a confirm dialog is captured with.
    ///
    /// The first is a plain click and the rest are ctrl-clicks, so the capture
    /// shows a genuine multi-selection — anchor, focus and all — rather than a
    /// selection assembled by a method the pointer never calls.
    fn select_fixture_sample(&mut self) {
        let mut first = true;
        for name in ["notes.md", "diagram.png", "archive.tar"] {
            let Some(at) = self.row_index(name) else {
                continue;
            };
            if first {
                self.selection.click(at);
                first = false;
            } else {
                self.selection.toggle(at);
            }
        }
    }

    /// Focuses the row called `name`, if the listing has one.
    fn focus_named(&mut self, name: &str) {
        if let Some(at) = self.row_index(name) {
            self.selection.click(at);
        }
    }

    /// The visible index of the row called `name`.
    fn row_index(&self, name: &str) -> Option<usize> {
        self.rows.iter().position(|r| r.entry.name == name)
    }

    /// Cycles Light → Dark → System, re-resolving on the next frame.
    pub fn cycle_theme(&mut self) {
        self.mode = match self.mode {
            ThemeMode::Light => ThemeMode::Dark,
            ThemeMode::Dark => ThemeMode::System,
            ThemeMode::System => ThemeMode::Light,
        };
        self.settings.theme = settings::Mode::from_theme(self.mode);
        self.settings_dirty = true;
    }

    // -- Navigation --------------------------------------------------------

    /// Navigates to `dir`: cancels anything in flight, re-arms the watcher,
    /// records the history, and refreshes the free-space probe.
    ///
    /// This is the only place navigation is initiated, so "cancel the old scan,
    /// join its worker, drop the old watcher, start the new ones" cannot be
    /// half-done.
    pub fn open_dir(&mut self, dir: PathBuf) {
        // §4.11: navigating away abandons the in-flight walk. Cancelling first
        // means the worker stops *before* the next drain rather than after it.
        //
        // The old scan is cancelled and *detached*, never joined here:
        // `ScanHandle::drop` only sets the cancel flag, so the worker exits on
        // its own and there is nothing to wait for. Joining (`finish`) would
        // block the frame for the length of the abandoned walk.
        if let Some(scan) = self.scan.take() {
            scan.cancel();
            drop(scan);
        }
        // `WatchSubscription::drop` joins the debouncer thread, so the old
        // watcher is retired on a reaper thread rather than dropped here —
        // dropping it inline would hitch the frame for up to one 25 ms poll.
        if let Some(sub) = self.watch.take() {
            retire_watch(sub);
        }

        self.dir = dir;
        // A navigation, whatever else it does. The fade starts when the listing
        // lands; see `nav_pending`.
        self.nav_pending = true;
        self.nav_fade = None;
        self.rows.clear();
        self.errors.clear();
        self.selection.clear();
        self.scroll_rows = 0;
        // Staged paths that no longer exist are dropped from the clipboard: a
        // cut staged in another directory is not a promise about this one.
        self.clipboard.clear();

        match kestrel_fs::scan::start(&self.dir, self.scan_options()) {
            Ok(handle) => self.scan = Some(handle),
            Err(err) => {
                // `scan::start` returns `io::Error`; `classify_io` maps it onto
                // the engine's typed error so the status bar reports the same
                // thing it would for a mid-scan failure.
                let err = kestrel_fs::error::classify_io(&self.dir, err);
                log::error!("scan: cannot start for {}: {err}", self.dir.display());
                self.errors.push(ScanError::new(self.dir.clone(), err));
            }
        }

        // A watcher that cannot start degrades to a **visible** manual-refresh
        // affordance, not an error dialog and not a log line. `F5` is in the
        // toolbar; the status bar says *why* it is the only way the list will
        // change, and offers the button, because a silently stale list is worse
        // than a visible degraded one.
        self.watch_error = None;
        match kestrel_fs::watcher::watch(&self.dir) {
            Ok(sub) => self.watch = Some(sub),
            Err(err) => {
                log::warn!("watch: {err}; falling back to manual refresh");
                self.watch_error = Some(format!("{err}"));
            }
        }

        self.history.push(&self.dir);
        self.space.request(&self.dir);
    }

    /// Records a navigation that resolves to a path already on the stack.
    ///
    /// A symlinked directory can resolve to a different path than the one the
    /// user clicked. Without this, following a link adds a history entry whose
    /// `Back` re-enters the same link, and the user loops. Called when the
    /// scanner proves a symlink's target is a directory.
    fn note_same_place(&mut self, dir: PathBuf) {
        self.history.replace_current(&dir);
    }

    /// The engine's scan options for the current sort.
    ///
    /// The sort is handed to the engine so each directory is sorted *before* it
    /// is emitted. That is what lets the list paint incrementally: the UI never
    /// sorts a 50,000-row `Vec`, which would be a frame-losing `O(n log n)` on a
    /// scan the user cannot cancel.
    fn scan_options(&self) -> ScanOptions {
        let recursive = self.view == ViewMode::Tree;
        ScanOptions {
            sort: Some(self.sort),
            // §5.2's hidden-file discussion presumes hidden entries are visible
            // and *marked*, which is why the settings default is `true` and why
            // the hidden-file dot exists at all. This is the only place the
            // setting reaches the filesystem: hiding an entry is a scan option,
            // never a filter, so it costs no post-processing on a 50,000-row
            // directory and a rescan is the only correct way to apply it.
            show_hidden: self.show_hidden,
            // Tree mode reads exactly one level deeper than it shows at first.
            //
            // Depth 1 is a deliberate ceiling for two reasons. It bounds the work
            // to one `readdir` per subdirectory, so a directory of 5,000 folders
            // cannot turn a view toggle into a full-tree walk; and it means every
            // child row is *already delivered*, which is what lets expanding a
            // folder be a pure filter with no I/O at all. Deeper nesting needs
            // either a second scan or a per-directory handle, which is Phase 4's
            // tree work.
            recursive,
            max_depth: if recursive { 1 } else { 0 },
            ..ScanOptions::listing()
        }
    }

    /// `true` when a row should be shown in tree mode.
    ///
    /// A depth-0 row is always shown. A deeper row is shown only if its parent
    /// directory is expanded, so collapsing hides exactly one subtree.
    ///
    /// Pure and free of I/O, which is the entire reason expansion does not
    /// rescan: the rows are already in `self.rows`, and the engine's depth-first
    /// order means a hidden parent means its children are hidden too.
    fn tree_is_visible(&self, row: &Row) -> bool {
        if self.view != ViewMode::Tree || row.depth == 0 {
            return true;
        }
        // Every directory from the row's parent up to the scan root must be
        // expanded — not just the immediate parent.
        //
        // Checking only the parent is the classic tree bug and it was caught
        // here by a test rather than by eye: with `d/sub` expanded, collapsing `d`
        // left `d/sub/grand.txt` on screen, orphaned above a closed folder. The
        // walk is O(depth) per row, and depth is currently capped at 1, so it is
        // two comparisons; the loop is written for the day the cap is raised.
        let mut dir = row.entry.path.parent();
        while let Some(d) = dir {
            // The scan root is the boundary: it is never "expanded" because the
            // user did not open it, they navigated to it.
            if d == self.dir {
                return true;
            }
            if !self.expanded.contains(d) {
                return false;
            }
            dir = d.parent();
        }
        true
    }

    /// Expands or collapses a directory, returning the new state.
    fn toggle_expand(&mut self, path: &Path) -> bool {
        if self.expanded.remove(path) {
            false
        } else {
            self.expanded.insert(path.to_path_buf());
            true
        }
    }

    // -- Frame pump ---------------------------------------------------------

    /// Drains the scan channel, the watcher channel, and the space probe.
    ///
    /// Runs once per frame, before any widget is built. All three are
    /// non-blocking: two `try_recv` loops and one atomic load.
    ///
    /// The scan's shape is the engine's documented pattern from
    /// `kestrel-fs/src/lib.rs`, and it is not incidental. `drain_into` takes a
    /// plain `FnMut(ScanEvent)` whose return value cannot be inverted — a
    /// `FnMut(ScanEvent) -> bool` sink caused a real bug during engine
    /// development, where inverted semantics silently truncated a scan to one
    /// entry. The closure still needs to observe `Complete` to decide whether to
    /// put the handle back, and it cannot clear `self.scan` from inside itself,
    /// so the handle is taken out for the duration of the drain and restored
    /// only if the walk is unfinished.
    ///
    /// Three unrelated jobs lived here behind one early return: the scan drain
    /// returned before the watcher and the Kind column whenever the scan was
    /// settled — which is the steady state — so external changes never
    /// refreshed the list and the Kind column never appeared. Each job is now
    /// its own method and `pump` runs all three unconditionally.
    fn pump(&mut self) {
        self.pump_scan();
        self.pump_watch();
        self.pump_columns();
    }

    /// The scan drain. Takes the handle for the drain, restores it while the walk
    /// is unfinished, and drops it once `Complete` arrives — `ScanHandle::drop`
    /// only sets the cancel flag, so dropping here never joins and never
    /// blocks the frame.
    fn pump_scan(&mut self) {
        let Some(scan) = self.scan.take() else {
            return;
        };
        let mut finished = false;
        scan.drain_into(|event| match event {
            ScanEvent::Entry(entry) => {
                let row = Row {
                    depth: depth_of(&self.dir, &entry.path),
                    entry,
                };
                if self.tree_is_visible(&row) {
                    self.rows.push(row);
                }
            }
            ScanEvent::Error(err) => self.errors.push(err),
            ScanEvent::Complete { .. } => finished = true,
            ScanEvent::BatchEnd => {}
        });
        if finished {
            // The worker sends `Complete` last, so the channel is now empty and
            // the thread is about to exit. `ScanHandle::drop` does not join —
            // the worker exits on its own — so this cannot block the frame.
            drop(scan);
        } else {
            self.scan = Some(scan);
        }
    }

    /// The watcher drain. `try_recv`, never `recv`: a blocking read here would
    /// stall the frame loop, which is the one thing this file forbids.
    fn pump_watch(&mut self) {
        let mut stale = false;
        if let Some(sub) = self.watch.as_ref() {
            while let Ok(event) = sub.try_recv() {
                match event {
                    WatchEvent::Changed(changes) => stale |= changes.touches(&self.dir),
                    // A backend failure mid-watch degrades to the same visible
                    // manual-refresh affordance as a watch that never started
                    // (§7.16: never a silent status change) — never just a log
                    // line the user never sees.
                    WatchEvent::Error(err) => {
                        log::warn!("watch: {err}");
                        self.watch_error = Some(format!("{err}"));
                    }
                }
            }
        }
        if stale {
            // Re-listing is cheap and cancellable, so it goes straight through
            // the same `open_dir` path a click takes. The engine's 250ms debounce
            // is what stops a `cargo build` from re-listing 400 times.
            let dir = self.dir.clone();
            self.open_dir(dir);
        }
    }

    /// The Kind column's justification. Only when the row count changes, so
    /// a 50,000-row listing is not sampled every frame.
    fn pump_columns(&mut self) {
        if self.rows.len() == self.kind_rows {
            return;
        }
        self.kind_rows = self.rows.len();
        let mixed =
            columns::listing_is_mixed(self.rows.iter().map(|r| r.entry.kind.is_directory()));
        self.show_kind_column = mixed;
    }

    // -- Row helpers -------------------------------------------------------

    /// `true` when `row` passes the live filter.
    fn passes_filter(&self, row: &Row) -> bool {
        if self.filter.is_empty() {
            return true;
        }
        row.entry
            .name
            .to_lowercase()
            .contains(&self.filter.to_lowercase())
    }

    /// The indices of rows that pass the filter, in order.
    ///
    /// Rebuilt each frame, and only over the rows already delivered — the filter
    /// is a display concern, so it must not require a rescan.
    fn visible_rows(&self) -> Vec<usize> {
        if self.filter.is_empty() {
            return (0..self.rows.len()).collect();
        }
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, r)| self.passes_filter(r))
            .map(|(i, _)| i)
            .collect()
    }

    /// The row at a *visible* position, accounting for the filter.
    fn visible_row(&self, at: usize) -> Option<&FileEntry> {
        self.visible_rows()
            .get(at)
            .and_then(|i| self.rows.get(*i))
            .map(Row::entry)
    }

    fn visible_count(&self) -> usize {
        if self.filter.is_empty() {
            self.rows.len()
        } else {
            self.rows.iter().filter(|r| self.passes_filter(r)).count()
        }
    }

    // -- File operations ---------------------------------------------------

    /// Starts a job over the current selection, after any confirm the op needs.
    ///
    /// §4.7: "Used for irreversible actions only". So `Trash` and `Delete` get a
    /// dialog and `Copy` does not — but a *move* onto an occupied destination
    /// still raises the collision dialog from the worker, because that is not a
    /// dialog about an intention, it is a question the filesystem asked.
    fn begin(&mut self, op: job::Op, destination: Option<PathBuf>) {
        let items = self.selected_items();
        if items.is_empty() {
            return;
        }
        if op.is_destructive() && self.modal.is_none() {
            self.modal = Some(DlgKind::Confirm {
                op,
                count: items.len(),
                sample: dialog::sample_path(&items),
                warning: DlgKind::irreversibility_line(op).map(str::to_string),
            });
            // Remember what to do when the dialog is answered, so the dialog
            // itself does not have to carry a closure.
            self.pending = Some(Pending {
                op,
                destination,
                items,
            });
            self.modal_focus = 0;
            return;
        }
        self.start_job(op, items, destination);
    }

    /// The selected rows as job items, in visible order.
    fn selected_items(&self) -> Vec<Item> {
        let visible = self.visible_rows();
        self.selection
            .rows()
            .filter_map(|v| visible.get(v).copied())
            .filter_map(|i| self.rows.get(i))
            .map(|row| {
                let known = row.entry.size;
                if row.entry.kind.is_directory() {
                    // A directory's `lstat` length is not a recursive total, and
                    // reporting it as one would be a lie in the progress bar.
                    Item::dir(row.entry.path.clone())
                } else {
                    Item::file(row.entry.path.clone(), known)
                }
            })
            .collect()
    }

    /// Launches the worker.
    fn start_job(&mut self, op: job::Op, items: Vec<Item>, destination: Option<PathBuf>) {
        // Replacing a running job cancels it — but the old job is **not**
        // silently discarded. Its handle is retired: the next `pump_job` drains
        // whatever it still says and reports the ending as a "Stopped"
        // status-bar line (§7.16: a long operation always reports itself).
        // Refusing the new job instead would hold explicit user intent hostage
        // to work the user has already decided to abandon; reporting the old
        // one keeps both visible.
        //
        // The join never happens here: `JobHandle::drop` joins, so dropping —
        // or `finish`ing — the old handle would block the frame for the length
        // of the in-flight work. See `detach_job`.
        if let Some(old) = self.job.take() {
            old.cancel();
            if let Some(progress) = self.job_progress.take() {
                self.retired_jobs.push(RetiredJob {
                    handle: old,
                    progress,
                });
            } else {
                // Replaced before its first pump: no progress was ever
                // published, so there is nothing to report — just join
                // off-frame.
                detach_job(old);
            }
            // A dialog about the old job must not survive it: answering the
            // old collision question would write into the *new* job's decision
            // slot, which its worker would then consume as an answer to a
            // question nobody asked. `Confirm`/`Failed`/`CannotOpen` are not
            // job-state dialogs and are left alone.
            if matches!(
                self.modal,
                Some(DlgKind::Collision { .. }) | Some(DlgKind::Progress { .. })
            ) {
                self.modal = None;
            }
        }
        let count = items.len();
        match job::start(op, items, destination) {
            Ok(handle) => {
                self.job = Some(handle);
                self.job_progress = Some(JobProgress::new(op, count));
                // A one-file copy reports itself in the status bar; a
                // hundred-item copy gets a dialog with a Stop button, because
                // that is the one a user is watching a clock for. The threshold
                // is 1 because "many items" is the only thing that makes a modal
                // less annoying than useful.
                if count > 1 && self.modal.is_none() {
                    self.modal = Some(DlgKind::Progress {
                        op,
                        done: 0,
                        total: count,
                        bytes: 0,
                        total_bytes: None,
                        current: self.dir.clone(),
                        rate: String::new(),
                    });
                    self.modal_focus = 0;
                }
            }
            Err(err) => {
                self.modal = Some(DlgKind::Failed {
                    op,
                    path: self.dir.clone(),
                    reason: job::describe(&err),
                });
                self.modal_focus = 0;
            }
        }
    }

    /// Drains the running job, and raises a dialog when it needs an answer.
    ///
    /// Also reaps [`Self::retired_jobs`]: replaced jobs are cancelled, not
    /// forgotten. Every drain here is a `try_recv` loop and every join happens
    /// on a reaper thread, so this never blocks the frame no matter what the
    /// workers are doing.
    fn pump_job(&mut self) {
        self.reap_retired_jobs();
        // A "Stopped" line is news, not furniture: it ages out on its own.
        if self
            .stopped_note
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() > std::time::Duration::from_secs(20))
        {
            self.stopped_note = None;
        }
        let Some(mut handle) = self.job.take() else {
            return;
        };
        let mut collided: Option<(PathBuf, PathBuf, usize)> = None;
        let mut failed: Option<(PathBuf, String)> = None;
        handle.drain_into(|event| match event {
            JobEvent::Started { total } => {
                if let Some(p) = self.job_progress.as_mut() {
                    p.total = total;
                }
            }
            JobEvent::Item { bytes, skipped, .. } => {
                if let Some(p) = self.job_progress.as_mut() {
                    p.done += 1;
                    if let Some(b) = bytes {
                        p.bytes += b;
                        *p.total_bytes.get_or_insert(0) += b;
                    }
                    if skipped {
                        p.skipped += 1;
                    }
                }
            }
            JobEvent::Collided {
                src,
                dst,
                remaining,
            } => {
                collided = Some((src, dst, remaining));
            }
            JobEvent::Progress {
                total_bytes,
                current,
                ..
            } => {
                if let Some(p) = self.job_progress.as_mut() {
                    p.total_bytes = total_bytes;
                    p.current = current;
                }
            }
            JobEvent::Done {
                done,
                bytes,
                skipped,
            } => {
                // A success is **not** a dialog. §4.7: dialogs are for irreversible
                // actions, and a completed copy is neither irreversible nor
                // surprising — it is a status-bar line that clears itself. Raising
                // a modal here would be a dialog for good news.
                if let Some(p) = self.job_progress.as_mut() {
                    p.done = done;
                    p.bytes = bytes;
                    p.skipped += skipped;
                    p.finished = true;
                }
            }
            JobEvent::Cancelled { done, .. } => {
                if let Some(p) = self.job_progress.as_mut() {
                    p.done = done;
                    p.finished = true;
                }
            }
            JobEvent::Failed {
                path,
                message: reason,
                ..
            } => {
                failed = Some((path, reason));
            }
        });

        if let Some((src, dst, remaining)) = collided {
            self.modal = Some(DlgKind::Collision {
                src,
                dst,
                remaining,
            });
            self.modal_focus = 0;
        } else if let Some((path, reason)) = failed {
            // A failure **is** worth interrupting for: the user needs to know a
            // job stopped halfway, or they will believe it finished.
            self.modal = Some(DlgKind::Failed {
                op: self.job_progress.as_ref().map_or(job::Op::Copy, |p| p.op),
                path,
                reason,
            });
            self.modal_focus = 0;
        } else if self
            .job_progress
            .as_ref()
            .is_some_and(JobProgress::is_finished)
        {
            // Finished cleanly: the status bar keeps the summary for a moment and
            // then drops it. No modal.
            self.job_progress = None;
        }
        // Keep a progress dialog's numbers live, and retire it when the job ends.
        if let (Some(DlgKind::Progress { op, .. }), Some(p)) =
            (self.modal.as_ref(), &self.job_progress)
        {
            let snapshot = DlgKind::Progress {
                op: *op,
                done: p.done,
                total: p.total,
                bytes: p.bytes,
                total_bytes: p.total_bytes,
                current: p.current.clone(),
                rate: job::format_rate(p.done, p.elapsed()),
            };
            self.modal = Some(snapshot);
        } else if matches!(self.modal, Some(DlgKind::Progress { .. })) {
            // The job ended: retire the dialog. A completion is not a dialog.
            if self.job_progress.is_none() {
                self.modal = None;
            }
        }

        // The handle goes back **unless** the job is waiting on a question or has
        // ended, in which case dropping it would cancel a job that is merely
        // parked — and `JobHandle::drop` joins, so holding it is what keeps the
        // worker alive across the dialog.
        let parked = handle.decisions().is_waiting();
        let terminal = self
            .job_progress
            .as_ref()
            .is_some_and(JobProgress::is_finished);
        if parked || !terminal {
            self.job = Some(handle);
        } else {
            // The terminal event is the worker's last send on an unbounded
            // channel, so the thread has already exited or is within a return
            // epilogue of doing so — but "almost joined" is still a join on
            // the frame thread, so the handle goes to a reaper instead.
            detach_job(handle);
        }
    }

    /// Drains replaced jobs and reports the ones that have ended.
    ///
    /// A retired job whose terminal event has arrived gets its ending told as
    /// a status-bar line and its handle detached for the join. One that has
    /// not ended yet stays queued — its worker is cancelled and will get
    /// there, and holding the handle is what keeps a parked worker's channel
    /// open until it does.
    fn reap_retired_jobs(&mut self) {
        let mut index = 0;
        while index < self.retired_jobs.len() {
            let mut terminal: Option<JobEvent> = None;
            {
                let retired = &mut self.retired_jobs[index];
                retired.handle.drain_into(|event| match event {
                    JobEvent::Started { total } => {
                        retired.progress.total = total;
                    }
                    JobEvent::Item { bytes, skipped, .. } => {
                        retired.progress.done += 1;
                        if let Some(b) = bytes {
                            retired.progress.bytes += b;
                            *retired.progress.total_bytes.get_or_insert(0) += b;
                        }
                        if skipped {
                            retired.progress.skipped += 1;
                        }
                    }
                    JobEvent::Progress {
                        total_bytes,
                        current,
                        ..
                    } => {
                        retired.progress.total_bytes = total_bytes;
                        retired.progress.current = current;
                    }
                    // A collision question for a dead job gets no dialog: the
                    // worker was cancelled and will take `Cancelled` instead of
                    // an answer. Answering it here would be worse than
                    // ignoring it — the answer has nowhere to go.
                    JobEvent::Collided { .. } => {}
                    ended @ (JobEvent::Done { .. }
                    | JobEvent::Cancelled { .. }
                    | JobEvent::Failed { .. }) => {
                        terminal = Some(ended);
                    }
                });
            }
            if let Some(event) = terminal {
                let retired = self.retired_jobs.remove(index);
                let line = report_retired(&retired.progress, &event);
                self.stopped_note = Some((line, Instant::now()));
                detach_job(retired.handle);
            } else {
                index += 1;
            }
        }
    }

    /// Answers the collision dialog.
    fn answer_collision(&mut self, button: DlgButton) {
        let Some(DlgKind::Collision { remaining, .. }) = self.modal.take() else {
            return;
        };
        let _ = remaining;
        let Some(job) = self.job.as_ref() else {
            return;
        };
        match button.to_decision() {
            // Cancel answers nothing, which cancels the job: the user's only way
            // out of a dialog they do not want to answer is to stop the operation.
            Some((strategy, scope)) => job.decisions().answer(Some(job::decide(strategy, scope))),
            None => {
                job.decisions().answer(None);
            }
        }
        self.modal_focus = 0;
    }

    /// Confirms or cancels a `Confirm` dialog.
    fn answer_confirm(&mut self, button: DlgButton) {
        let pending = self.pending.take();
        self.modal = None;
        let Some(p) = pending else {
            return;
        };
        if button.is_cancel() {
            return;
        }
        self.start_job(p.op, p.items, p.destination);
    }

    // -- Actions -----------------------------------------------------------

    /// §4.11 `F2` — begin an inline rename on the focused row.
    fn begin_rename(&mut self) {
        let Some(at) = self.selection.focus() else {
            return;
        };
        let Some(entry) = self.visible_row(at) else {
            return;
        };
        self.renaming = Some(RenameInline::begin(at, &entry.path));
    }

    /// Commits the inline rename, if the field is valid.
    ///
    /// The commit is a `std::fs::rename` on a **worker** like everything else in
    /// Phase 4 — a rename across a slow network mount is not instant, and
    /// `rename` is not. It is done here as a one-item `Move` job so it reuses the
    /// collision protocol: renaming onto an existing name is a collision, and
    /// "never silently overwrite" applies to a rename exactly as it does to a
    /// copy.
    fn commit_rename(&mut self) {
        let Some(inline) = self.renaming.take() else {
            return;
        };
        if !inline.can_commit() || inline.is_unchanged() {
            return;
        }
        let from = inline.original.clone();
        let to = crate::rename::renamed_path(&from, &inline.draft);
        let Some(parent) = to.parent().map(Path::to_path_buf) else {
            return;
        };
        self.start_job(job::Op::Move, vec![Item::file(from, None)], Some(parent));
    }

    /// Reverts the inline rename.
    fn cancel_rename(&mut self) {
        self.renaming = None;
    }

    /// §4.11 `Ctrl+V` — paste the staged paths into the current directory.
    ///
    /// A `Cut` is a move, a `Copy` is a copy, and the difference is the whole
    /// point of the staged set. The clipboard is cleared either way, because a
    /// clipboard that keeps a cut staged after the move has already happened
    /// would let a second paste try to move files that are gone.
    fn paste(&mut self) {
        if self.clipboard.is_empty() {
            return;
        }
        let op = if self.clipboard.op() == Op::Cut {
            job::Op::Move
        } else {
            job::Op::Copy
        };
        let items: Vec<Item> = self
            .clipboard
            .paths()
            .map(|p| Item::file(p.clone(), None))
            .collect();
        let dest = self.dir.clone();
        self.clipboard.clear();
        if op.needs_destination() {
            self.start_job(op, items, Some(dest));
        } else {
            self.start_job(op, items, None);
        }
    }

    /// §4.11 `Ctrl+Shift+N` — a new, empty folder, named in the inline field.
    ///
    /// Created empty and then renamed by the user, rather than created with a
    /// generated name: an `Untitled folder 2` the user has to rename anyway is a
    /// worse default than an empty field that is already focused.
    fn new_folder(&mut self) {
        if self.renaming.is_some() {
            return;
        }
        // The field edits a path that does not exist yet, so `begin` is faked from
        // a synthetic path inside the current directory.
        let synthetic = self.dir.join("New folder");
        self.renaming = Some(RenameInline::begin(usize::MAX, &synthetic));
        self.creating = true;
    }

    /// Goes back one history entry.
    fn go_back(&mut self) {
        let Some(target) = self.history.back().map(Path::to_path_buf) else {
            return;
        };
        self.open_dir(target);
    }

    /// Goes forward one history entry.
    fn go_forward(&mut self) {
        let Some(target) = self.history.forward().map(Path::to_path_buf) else {
            return;
        };
        self.open_dir(target);
    }

    /// Goes up one level. §4.11 `Backspace` / `Alt+Up`.
    fn go_up(&mut self) {
        // `Path::parent` on "/" is `None`, which is the natural "nowhere to go":
        // no clamping to "/" and no spurious navigation.
        if let Some(parent) = self.dir.parent() {
            let parent = parent.to_path_buf();
            self.open_dir(parent);
        }
    }

    /// Re-reads the current directory. §4.11 `F5`.
    fn refresh(&mut self) {
        let dir = self.dir.clone();
        // A refresh is not a navigation; it must not add history depth.
        self.open_dir(dir);
    }

    /// Acts on the focused row: navigate into a directory, or open a file.
    ///
    /// In tree mode a directory **expands or collapses in place** instead: the
    /// whole point of the view is to see a hierarchy, and navigating away from
    /// under the user to show one folder is a list view with extra steps.
    ///
    /// Zero I/O for the *decision*: the answer is
    /// [`kestrel_fs::model::FileEntry::is_descendable`], which the scanner
    /// filled in on its own thread, and toggling expansion re-filters rows that
    /// are already in memory.
    ///
    /// Opening a **file** used to be a dead end — the key was consumed and
    /// nothing happened, in the app's most-used action. It now goes through
    /// [`kestrel_fs::open`], which resolves the type, finds the registered
    /// handler and runs it on a worker thread. A directory never reaches that
    /// path: navigating into it is the whole point of the key, and a folder with
    /// a registered handler is something you browse, not something you launch.
    fn enter(&mut self) {
        let Some(at) = self.selection.focus() else {
            return;
        };
        let Some(entry) = self.visible_row(at) else {
            return;
        };
        // The two fields the branch needs are copied out before anything takes
        // `&mut self`, so the decision can be made without holding a borrow
        // across the call.
        let action = kestrel_fs::open::enter_action(entry);
        let path = entry.path.clone();
        let kind = entry.kind;
        match action {
            Some(kestrel_fs::open::Enter::Open) => self.open_path(&path),
            Some(kestrel_fs::open::Enter::Descend) => self.descend(&path, kind),
            None => {}
        }
    }

    /// Navigates into a directory the scanner proved is one.
    fn descend(&mut self, path: &Path, kind: EntryKind) {
        if self.view == ViewMode::Tree {
            let target = path.to_path_buf();
            self.toggle_expand(&target);
            return;
        }
        // A symlink resolves to its own path, which the history may already
        // hold under a different name; record it as the same place.
        if kind == EntryKind::Symlink {
            self.open_dir(path.to_path_buf());
            self.note_same_place(path.to_path_buf());
        } else {
            self.open_dir(path.to_path_buf());
        }
    }

    /// Opens a file with the user's default application.
    ///
    /// Returns immediately: [`kestrel_fs::open::open`] does the resolution and
    /// the spawn on a worker thread, and the only thing this frame does with
    /// the answer is store it. A double-click's second click, or a second
    /// `Enter` while the first is still in flight, replaces the handle rather
    /// than queueing a second launch — the first attempt is still running and
    /// will report its own outcome, and two applications opening at once
    /// because the key was pressed twice is not what anybody meant.
    fn open_path(&mut self, path: &Path) {
        let attempt = kestrel_fs::open::open(path);
        log::info!("open: {}", path.display());
        self.opening = Some(attempt);
    }

    /// Collects a finished open attempt, if there is one.
    ///
    /// Called once per frame next to [`Self::pump_job`]. `poll` cannot block,
    /// so this is safe to run unconditionally; the handle is cleared the frame
    /// the answer arrives, so the dialog is raised once and not every frame
    /// after.
    fn pump_open(&mut self) {
        let Some(mut attempt) = self.opening.take() else {
            return;
        };
        match attempt.poll() {
            Some(Ok(app)) => {
                log::info!("open: handed to {app}");
            }
            Some(Err(err)) => {
                log::warn!("open: {err}");
                // A dialog already up is not replaced: the user is in the middle
                // of answering a question that matters more than this one, and
                // swapping it out loses their answer.
                if self.modal.is_none() {
                    self.modal = Some(DlgKind::CannotOpen {
                        path: err.path().to_path_buf(),
                        reason: err.sentence(),
                    });
                    self.modal_focus = 0;
                }
            }
            None => self.opening = Some(attempt),
        }
    }

    /// Cycles the sort column: Name → Size → Modified → Kind → Name, each click
    /// also flipping the direction.
    ///
    /// The direction cycles **ascending → descending** and stops, rather than
    /// adding a third "natural" state the spec's `SortSpec` has no field for.
    fn cycle_sort(&mut self) {
        self.sort_column = match self.sort_column {
            SortKey::Name => SortKey::Size,
            SortKey::Size => SortKey::Modified,
            SortKey::Modified => SortKey::Kind,
            SortKey::Kind => SortKey::Name,
        };
        self.apply_sort(true);
    }

    /// Sets the sort from a header click, flipping direction if the column is
    /// already active.
    fn sort_by_column(&mut self, key: SortKey) {
        self.settings.sort = settings::Column::from_sort_key(key);
        self.settings_dirty = true;
        if self.sort_column == key {
            self.sort.ascending = !self.sort.ascending;
        } else {
            self.sort_column = key;
            self.sort.ascending = true;
        }
        self.apply_sort(false);
    }

    /// Re-runs the scan under the new sort.
    ///
    /// The GUI never sorts. A second comparator in the UI would be a second
    /// source of truth, and the day they disagreed the list would jitter
    /// between two orders on the same directory.
    fn apply_sort(&mut self, _from_toolbar: bool) {
        self.sort.key = self.sort_column;
        let dir = self.dir.clone();
        self.open_dir(dir);
    }

    /// Stages the current selection for `op`. §4.11 `Ctrl+X` / `Ctrl+C`.
    ///
    /// Nothing selected is a no-op rather than an empty clipboard: `Ctrl+C` on
    /// a fresh directory must not report "Copied 0 items" in the status bar,
    /// because a user who sees that has been told the wrong thing about a
    /// selection they can plainly see is empty.
    fn stage(&mut self, op: Op) {
        if self.selection.is_empty() {
            return;
        }
        let visible = self.visible_rows();
        let paths: Vec<PathBuf> = self
            .selection
            .rows()
            .filter_map(|v| visible.get(v).copied())
            .filter_map(|i| self.rows.get(i).map(|r| r.entry.path.clone()))
            .collect();
        self.clipboard.stage(paths, op);
    }

    // -- Keyboard ----------------------------------------------------------

    /// Handles the §4.11 bindings.
    ///
    /// `input_mut(|i| ...)` **consumes** each key, so a binding fires exactly
    /// once and cannot also reach a focused text widget. This runs before the
    /// panels are built, so a key that navigates is applied before the frame
    /// paints the new directory — no one-frame flash of the old list.
    fn keyboard(&mut self, ui: &mut Ui) {
        // A modal is modal: while one is up, the only keys that mean anything
        // are its own. Without this, `Delete` behind a "Move 3 items to Trash?"
        // dialog deletes something else, which is the single worst thing a
        // confirm dialog can be for.
        if self.modal.is_some() {
            self.modal_keys(ui);
            return;
        }
        // A rename field owns the keyboard outright. `Delete` is the sharpest
        // case: with a row selected and a rename open, `Delete` must delete a
        // *character*, not the file. §4.11's "Escape — cancel rename" is the
        // only global that survives.
        if self.renaming.is_some() {
            let enter = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter));
            let escape = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape));
            if enter {
                if self.creating {
                    self.create_from_field();
                } else {
                    self.commit_rename();
                }
            }
            if escape {
                self.cancel_rename();
            }
            return;
        }
        // While the filter field has focus, only Escape and Enter are ours;
        // everything else belongs to the text editor. §4.11: "Escape — clear
        // search → close menu → cancel rename, in that order."
        if self.filter_focused {
            let escape = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape));
            if escape {
                if self.filter.is_empty() {
                    self.filter_focused = false;
                } else {
                    self.filter.clear();
                }
            }
            return;
        }

        enum Act {
            None,
            // §4.11 file operations.
            Rename,
            DeleteToTrash,
            DeletePermanent,
            Paste,
            NewFolder,
            QuickLook,
            TogglePreview,
            Help,
            Settings,
            Filter,
            /// Arrow-key movement. `true` means Shift was held, which extends the
            /// anchored range instead of collapsing onto one row.
            Move(isize, bool),
            Top,
            Bottom,
            ExtendToTop,
            ExtendToBottom,
            Enter,
            Parent,
            Back,
            Forward,
            Refresh,
            SelectAll,
            ClearSelection,
            Cut,
            Copy,
            ViewList,
            ViewTree,
            ToggleTheme,
        }

        let count = self.visible_count();
        let has_filter = !self.filter.is_empty();
        let act = ui.input_mut(|i| {
            // Most specific first: `consume_key` matches modifiers logically, so
            // `alt+up` must be tested before bare `up`.
            if i.consume_key(egui::Modifiers::ALT, Key::ArrowLeft) {
                Act::Back
            } else if i.consume_key(egui::Modifiers::ALT, Key::ArrowRight) {
                Act::Forward
            } else if i.consume_key(egui::Modifiers::ALT, Key::ArrowUp) {
                Act::Parent
            } else if i.consume_key(egui::Modifiers::CTRL, Key::A) {
                Act::SelectAll
            } else if i.consume_key(egui::Modifiers::CTRL, Key::C) {
                Act::Copy
            } else if i.consume_key(egui::Modifiers::CTRL, Key::X) {
                Act::Cut
            } else if i.consume_key(egui::Modifiers::CTRL, Key::F) {
                // Focus the filter; handled by the caller, which owns the field.
                Act::None
            } else if i.consume_key(egui::Modifiers::CTRL, Key::T) {
                Act::ToggleTheme
            } else if i.consume_key(egui::Modifiers::CTRL, Key::Num1) {
                Act::ViewList
            } else if i.consume_key(egui::Modifiers::CTRL, Key::Num2) {
                Act::ViewTree
            } else if i.consume_key(egui::Modifiers::NONE, Key::F2) {
                Act::Rename
            } else if i.consume_key(egui::Modifiers::SHIFT, Key::Delete) {
                Act::DeletePermanent
            } else if i.consume_key(egui::Modifiers::NONE, Key::Delete) {
                Act::DeleteToTrash
            } else if i.consume_key(egui::Modifiers::CTRL, Key::V) {
                Act::Paste
            } else if i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, Key::N) {
                Act::NewFolder
            } else if i.consume_key(egui::Modifiers::NONE, Key::Space) {
                Act::QuickLook
            } else if i.consume_key(egui::Modifiers::CTRL, Key::P) {
                Act::TogglePreview
            } else if i.consume_key(egui::Modifiers::CTRL, Key::F1)
                || i.consume_key(egui::Modifiers::NONE, Key::F1)
            {
                // Both spellings because `consume_key` compares modifiers
                // *logically*: `Ctrl+F1` with Ctrl held is also a plain `F1`
                // press, so the two arms cannot be separate branches.
                Act::Help
            } else if i.consume_key(egui::Modifiers::CTRL, Key::Comma) {
                Act::Settings
            } else if i.consume_key(egui::Modifiers::NONE, Key::Slash) {
                Act::Filter
            } else if i.consume_key(egui::Modifiers::NONE, Key::F5) {
                Act::Refresh
            } else if i.consume_key(egui::Modifiers::NONE, Key::Enter) {
                Act::Enter
            } else if i.consume_key(egui::Modifiers::NONE, Key::Escape) {
                if has_filter {
                    Act::None
                } else {
                    Act::ClearSelection
                }
            } else if i.consume_key(egui::Modifiers::SHIFT, Key::Home) {
                Act::ExtendToTop
            } else if i.consume_key(egui::Modifiers::SHIFT, Key::End) {
                Act::ExtendToBottom
            } else if i.consume_key(egui::Modifiers::NONE, Key::Home) {
                Act::Top
            } else if i.consume_key(egui::Modifiers::NONE, Key::End) {
                Act::Bottom
            } else if i.consume_key(egui::Modifiers::SHIFT, Key::ArrowUp) {
                Act::Move(-1, true)
            } else if i.consume_key(egui::Modifiers::SHIFT, Key::ArrowDown) {
                Act::Move(1, true)
            } else if i.consume_key(egui::Modifiers::SHIFT, Key::PageUp) {
                Act::Move(-(component::SCROLLBAR_KEYBOARD_LINES as isize), true)
            } else if i.consume_key(egui::Modifiers::SHIFT, Key::PageDown) {
                Act::Move(component::SCROLLBAR_KEYBOARD_LINES as isize, true)
            } else if i.consume_key(egui::Modifiers::NONE, Key::ArrowUp) {
                Act::Move(-1, false)
            } else if i.consume_key(egui::Modifiers::NONE, Key::ArrowDown) {
                Act::Move(1, false)
            } else if i.consume_key(egui::Modifiers::NONE, Key::PageUp) {
                Act::Move(-(component::SCROLLBAR_KEYBOARD_LINES as isize), false)
            } else if i.consume_key(egui::Modifiers::NONE, Key::PageDown) {
                Act::Move(component::SCROLLBAR_KEYBOARD_LINES as isize, false)
            } else {
                Act::None
            }
        });

        match act {
            Act::None => {}
            Act::Rename => self.begin_rename(),
            // §4.11: `Delete` is to trash, `Shift+Delete` is permanent. Trash is
            // the default, so a slip of the finger is recoverable.
            Act::DeleteToTrash => self.begin(dialog::op_for_delete(false), None),
            Act::DeletePermanent => self.begin(dialog::op_for_delete(true), None),
            Act::Paste => self.paste(),
            Act::NewFolder => self.new_folder(),
            Act::QuickLook => self.show_preview = true,
            Act::TogglePreview => {
                self.show_preview = !self.show_preview;
                self.settings.show_preview = self.show_preview;
                self.settings_dirty = true;
            }
            Act::Help => self.help = true,
            Act::Settings => self.open_settings(),
            Act::Filter => self.filter_focused = true,
            Act::Move(d, extend) => {
                if extend {
                    self.selection.extend_by(d, count);
                } else {
                    self.selection.move_focus(d, count);
                }
                self.scroll_to_focus(count);
            }
            Act::Top => {
                self.selection.focus_row(0, count);
                self.scroll_rows = 0;
            }
            Act::Bottom => {
                if count > 0 {
                    self.selection.focus_row(count - 1, count);
                    self.scroll_rows = count.saturating_sub(1);
                }
            }
            // Shift+Home / Shift+End: select everything from the anchor to an
            // end. The keyboard equivalent of shift-clicking the first or last
            // row, and the only practical way to select a 50,000-row directory.
            Act::ExtendToTop => {
                self.selection.extend_to(0);
                self.scroll_rows = 0;
            }
            Act::ExtendToBottom => {
                if count > 0 {
                    self.selection.extend_to(count - 1);
                    self.scroll_rows = count.saturating_sub(1);
                }
            }
            Act::Enter => self.enter(),
            Act::Parent => self.go_up(),
            Act::Back => self.go_back(),
            Act::Forward => self.go_forward(),
            Act::Refresh => self.refresh(),
            Act::SelectAll => self.selection.select_all(count),
            Act::ClearSelection => self.selection.clear(),
            Act::Cut => self.stage(Op::Cut),
            Act::Copy => self.stage(Op::Copy),
            Act::ViewList => self.set_view(ViewMode::List),
            Act::ViewTree => self.set_view(ViewMode::Tree),
            Act::ToggleTheme => self.cycle_theme(),
        }
    }

    /// Switches the list view, and records the choice as a setting.
    ///
    /// One method for both the keyboard and the toolbar, so the two cannot
    /// disagree about what "change the view" means — and so neither can change
    /// the view without the setting following, which is the bug a second
    /// assignment would introduce.
    fn set_view(&mut self, view: ViewMode) {
        self.view = view;
        self.settings.view = match view {
            ViewMode::List => settings::View::List,
            ViewMode::Tree => settings::View::Tree,
        };
        self.settings_dirty = true;
    }

    /// Keyboard handling while a dialog is up.
    ///
    /// Enter presses the button that has focus, which on open is always the safe
    /// one (§4.7). Escape is *not* bound: dismissing a "Delete Permanently"
    /// dialog with Escape would put a destructive action one stray key away from
    /// every other dialog, and §4.7's ordering for Escape ("clear search → close
    /// menu → cancel rename") does not mention confirmations.
    fn modal_keys(&mut self, ui: &mut Ui) {
        let enter = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter));
        if !enter {
            return;
        }
        let Some(kind) = self.modal.clone() else {
            return;
        };
        // Enter follows §4.7's *default*, which is not always the focused button:
        // the focus ring is always on the safe action, and the Enter default is
        // `Move to Trash` when — and only when — the action is reversible.
        let button = dialog::enter_default(&kind);
        match &kind {
            DlgKind::Collision { .. } => self.answer_collision(button),
            DlgKind::Confirm { .. } | DlgKind::Failed { .. } => {
                self.modal = None;
                self.answer_confirm(button)
            }
            // A report, not a question: every button on it dismisses it, and
            // `answer_confirm` would otherwise look for a pending job and do
            // nothing — which is right, but by accident rather than by decision.
            DlgKind::CannotOpen { .. } => {
                self.modal = None;
            }
            DlgKind::Progress { .. } => {
                if let Some(job) = self.job.as_ref() {
                    job.cancel();
                }
                self.modal = None;
            }
        }
    }

    /// Creates the folder the inline field names.
    fn create_from_field(&mut self) {
        let Some(inline) = self.renaming.take() else {
            return;
        };
        self.creating = false;
        if !inline.can_commit() {
            return;
        }
        let path = crate::rename::renamed_path(&self.dir, &inline.draft);
        // No `exists()` check here: that is a `stat(2)` on the frame thread,
        // on a path that may live on a network mount — and this file's first
        // rule is that there is no `std::fs` in the render path. The worker
        // reports an occupied name as `AlreadyExists` anyway, which `pump_mkdir`
        // turns into the same "already there" dialog one frame later.
        // Off the frame thread, with its answer collected: a `create_dir` on a
        // network mount is not instant, and a create that fails silently is a
        // new folder that never appears and an error the user never sees.
        match job::mkdir(&path) {
            Ok(rx) => self.mkdir_result = Some(rx),
            Err(e) => {
                let reason = job::describe_io(&path, e);
                self.modal = Some(DlgKind::Failed {
                    op: job::Op::Copy,
                    path,
                    reason,
                });
                self.modal_focus = 0;
            }
        }
    }

    /// Collects a pending `mkdir` answer, if one is outstanding.
    ///
    /// A `try_recv`, so a slow create is never waited on — the same discipline as
    /// the scan and the job.
    fn pump_mkdir(&mut self) {
        let Some(rx) = self.mkdir_result.as_ref() else {
            return;
        };
        let Ok(result) = rx.try_recv() else {
            return;
        };
        self.mkdir_result = None;
        if let Err(err) = result {
            self.modal = Some(DlgKind::Failed {
                op: job::Op::Copy,
                path: self.dir.clone(),
                reason: job::describe(&err),
            });
            self.modal_focus = 0;
        } else {
            // A new directory changes what is there, so the listing is re-read.
            // The watcher would notice, but a user who just pressed
            // Ctrl+Shift+N should not wait for the debounce.
            let dir = self.dir.clone();
            self.open_dir(dir);
        }
    }

    /// Scrolls so the focused row is fully visible.
    ///
    /// WCAG 2.2 focus-not-obscured, done properly: the offset is a *row* count
    /// this module owns, not a ScrollArea-internal pixel value, so "is it
    /// visible" is a comparison rather than a guess. One row of slack is kept
    /// above and below, which is §4.2 rule 3's "with 1px to spare".
    fn scroll_to_focus(&mut self, count: usize) {
        let Some(focus) = self.selection.focus() else {
            return;
        };
        let visible_rows = self.visible_rows_in_view(count);
        if visible_rows == 0 {
            return;
        }
        if focus < self.scroll_rows {
            self.scroll_rows = focus;
        } else if focus + 1 > self.scroll_rows + visible_rows {
            self.scroll_rows = focus + 1 - visible_rows;
        }
    }

    /// How many rows fit in the current list viewport, from the last frame's
    /// height. Kept as a field so the keyboard path does not need a `Ui`.
    fn visible_rows_in_view(&self, _count: usize) -> usize {
        self.viewport_rows.max(1)
    }
}

impl eframe::App for KestrelApp {
    /// egui 0.36's entry point.
    ///
    /// (`App::update` was deprecated in 0.34 and **removed** in 0.35.)
    ///
    /// A thin adapter: it resolves the `System` theme against the live winit
    /// window and hands off to [`KestrelApp::draw`], which is the whole app. The
    /// split exists so `--screenshot` can render the identical frame through
    /// `Context::run_ui` with no `Frame` in scope.
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Re-resolve `System` every frame so a compositor theme change is picked
        // up. `Window::theme()` is a cheap query on the thread already owning
        // the window, and the documented Wayland behaviour ("only returns theme
        // overrides") is covered by `Theme::resolve`'s dark default.
        if self.mode == ThemeMode::System {
            let resolved = Theme::resolve(self.mode, system_preference(frame));
            if resolved != self.theme {
                self.theme = resolved;
                self.theme.apply(&ctx);
            }
        }
        self.draw(ui);
    }

    /// Writes the settings to `eframe`'s storage.
    ///
    /// Called on eframe's auto-save interval and once on exit, which is why
    /// there is no `write` anywhere in this crate: the storage is an in-memory
    /// map, and eframe's own writer thread does the file I/O off the frame.
    /// A `std::fs::write` on the way out of a settings toggle would have been a
    /// blocking call in the UI path, which is the one thing this crate does not
    /// do.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.settings.normalise();
        eframe::set_value(storage, settings::KEY, &self.settings);
    }
}

impl KestrelApp {
    /// Draws one frame. Everything the app does, with no windowing types in
    /// scope.
    pub fn draw(&mut self, ui: &mut Ui) {
        let start = Instant::now();
        match self.screen {
            Screen::Gallery => crate::gallery::show(ui, &mut self.theme, &mut self.mode),
            Screen::Settings => self.settings_screen(ui),
            Screen::Browser => {
                // Non-blocking pumps, before any widget reads the row vector.
                self.pump();
                self.pump_job();
                self.pump_mkdir();
                self.pump_open();
                self.keyboard(ui);
                self.panels(ui);
            }
        }

        // The shortcut overlay is drawn last, over everything including the
        // dialog scrim, and is *not* part of `Screen`: it is a layer, and a
        // layer that replaced the browser would mean F1 destroyed the selection
        // you were about to act on.
        if self.help && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::F1)) {
            self.help = false;
        }
        if self.help && crate::help::draw(ui, &self.theme) {
            self.help = false;
        }

        self.frames = self.frames.wrapping_add(1);
        let elapsed = start.elapsed().as_secs_f32() * 1000.0;
        // Exponential moving average, so the status-bar readout is stable enough
        // to read instead of flickering every frame.
        self.frame_ms = if self.frames < 8 {
            elapsed
        } else {
            self.frame_ms * 0.9 + elapsed * 0.1
        };
    }
}

// ----------------------------------------------------------------------------
// Panels
// ----------------------------------------------------------------------------

impl KestrelApp {
    /// The settings screen.
    ///
    /// A replacement for the browser rather than a window on top of it: the
    /// browser's scroll offset, selection and staged clipboard are untouched
    /// underneath, so closing the screen is instant and nothing is lost by
    /// having looked at the settings.
    fn settings_screen(&mut self, ui: &mut Ui) {
        // `Escape` closes, and only `Escape` — a settings screen has no
        // unsaved state to lose (every change is already in `self.settings`,
        // which is what gets saved), so there is nothing to confirm and a
        // confirm would be §4.7's rule applied to a non-destructive action.
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
            self.close_settings();
            return;
        }
        let theme = self.theme;
        let motion = self.motion;
        let before = self.settings;
        if settings::draw(ui, &theme, &mut self.settings, motion) == settings::Action::Close {
            self.close_settings();
            return;
        }
        if self.settings != before {
            // A changed setting that needs a rescan applies immediately rather
            // than on close: `show_hidden` is a scan option, so waiting for the
            // Close button would show a setting the app is not honouring.
            self.settings_dirty = true;
            if self.settings.show_hidden != before.show_hidden {
                let dir = self.dir.clone();
                self.open_dir(dir);
            }
        }
    }

    /// Leaves the settings screen, folding the edits into the live app.
    fn close_settings(&mut self) {
        self.apply_settings();
        self.screen = Screen::Browser;
    }

    /// Opens the settings screen.
    fn open_settings(&mut self) {
        // The other direction of `apply_settings`'s rule: the app may have
        // changed a setting since the screen was last closed (`Ctrl+T` cycles
        // the theme, `Ctrl+2` the view), and reopening must show what is true
        // now rather than what was true then.
        self.apply_settings();
        self.screen = Screen::Settings;
    }

    /// Builds the whole shell. Panels before the `CentralPanel`, always.
    fn panels(&mut self, ui: &mut Ui) {
        // Panel order is not cosmetic: an egui panel is sized against whatever
        // the *root* `Ui` has left at the moment it is shown, so a panel shown
        // before a full-width band does not know that band exists.
        //
        // The status bar used to be shown sixth, after the preview pane, which
        // meant the pane was 748px tall when 723px of it was visible: the last
        // 25px sat under the status bar. Nothing looked wrong until something
        // was *centred* in the pane — the empty state landed 15px below the
        // middle of what the user can see, and an image's fit box was 25px taller
        // than the space it was centred in. The two full-width bands therefore
        // go first, and every side panel after them.
        self.top_toolbar(ui);
        self.status_bar(ui);
        // `pane_too_narrow` is cleared **here**, immediately after the band that
        // reads it and before the panes that set it. The status bar is drawn
        // second and the panes that decide whether they fit are drawn after it,
        // so a flag cleared at the top of this function is read as `false` every
        // frame and the notice never appears — which is the exact bug the flag
        // was added to fix. Cleared here, it survives from the frame the pane
        // made the decision to the frame the status bar reports it, which at
        // 16ms is not a thing anyone can see. Reordering the bands would fix it
        // too, and §2.12 and the note below make their order load-bearing.
        self.pane_too_narrow = false;
        // Places first, then the detail pane, then the list.
        //
        // The *order* the two side panels are shown in is load-bearing and the
        // positions are not: a left and a right panel both measure themselves
        // against the central rectangle whatever order they appear in, so
        // showing the sidebar first is free — and it is the only way the
        // preview pane can ask "how much is left?" and get an answer that
        // accounts for a sidebar the user has *dragged* to a width this code
        // has never heard of. `metric::SIDEBAR_WIDTH` is a default, not a fact.
        if self.show_sidebar && self.sidebar_fits(ui.available_width()) {
            self.sidebar(ui);
        } else if self.show_sidebar {
            // The setting says show the sidebar and the window says there is no
            // room for it *and* a list. Said so in the status bar rather than
            // silently overridden.
            self.pane_too_narrow = true;
        }
        if self.show_preview {
            // Right, so the list keeps the left-to-right reading order of a file
            // manager: places, list, detail.
            self.preview_panel(ui);
        }
        self.breadcrumb(ui);
        // The list's width is measured **once**, here, and handed to both the
        // header and the rows.
        //
        // The three `available_width()` reads this replaces were each correct in
        // isolation and wrong as a set. `columns_for` took the `CentralPanel`'s
        // width, the rows took the width *inside* the `ScrollArea`, and the
        // header took the root `Ui`'s. They agree today, and that agreement is
        // the trap: `ScrollArea`'s content `Ui` reports a `max_rect` built from
        // the viewport, and reading it works — right up until a widget inside the
        // closure widens the content, at which point the content `Ui`'s
        // `max_rect` grows with it and `available_width()` reports the *content*
        // width, not the viewport's. The gallery hit exactly that (1200 -> 1564)
        // and the fix there was `set_max_width` at the top of every section.
        //
        // Here the honest fix is not a clamp but a single source: the width the
        // panel actually has, threaded explicitly. The header and the rows then
        // cannot disagree even in principle, and §4.9's "the list is never inset
        // to make room" for a floating scrollbar is honoured — a bar that appears
        // overlays the last column instead of silently re-flowing the values out
        // from under their own header.
        let list_width = ui.available_width();
        self.column_headers(ui, list_width);
        // The central panel goes **last**: it claims whatever rectangle the
        // others did not take.
        let fill = self.theme.surfaces.list;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(fill))
            .show(ui, |ui| self.file_list(ui, list_width));
        // The modal is drawn after everything, so it is genuinely on top — and
        // the scrim it paints is what makes the window behind it clearly not
        // clickable, which §2.12's "never an overlay" is about for *chrome*, not
        // for a modal that is *supposed* to block.
        self.modal_layer(ui);
    }

    /// §4.7 The confirmation / collision / failure dialog.
    ///
    /// A `Modal` rather than a `Window`, because a dialog must not be movable,
    /// resizable, or closable by its title bar: §4.7 gives it a scrim, a fixed
    /// `dialog.width`, and one of exactly two or five buttons.
    ///
    /// # Why the content is scoped, and why there is no height guess
    ///
    /// `dialog.width` is 400px, and the previous version "applied" it by
    /// allocating a 400 x 140-172 rect and then calling `Frame::show` on the
    /// *remaining* space. Two things went wrong, both of them layout rather than
    /// style, and neither was visible without a render:
    ///
    /// * the frame wrapped a `Ui` whose width was still the whole `Modal` area,
    ///   so the dialog came out **600px** wide; and
    /// * the reservation stayed on the cursor, so the frame was painted *below*
    ///   it — the dialog was 172px too low, which read as "slightly off-centre"
    ///   until you measured it.
    ///
    /// An egui `Frame` sizes itself to its **content** (`Prepared::outer_rect` is
    /// `content_ui.min_rect()` plus the margins), so constraining the content's
    /// `max_rect` to `dialog::WIDTH` and letting it grow vertically gives both
    /// numbers for free: a dialog that is exactly 400px wide and exactly as tall
    /// as its content, with the `Modal`'s own centring doing the rest. A
    /// hard-coded height is the thing that has to be wrong when a sentence wraps
    /// to three lines instead of two.
    fn modal_layer(&mut self, ui: &mut Ui) {
        let Some(kind) = self.modal.clone() else {
            return;
        };
        let theme = self.theme;
        let buttons = dialog::buttons_for(&kind);
        // Keep the focus index inside the button list: the list changes shape
        // between dialog kinds, and a stale index would index out of bounds.
        if self.modal_focus >= buttons.len() {
            self.modal_focus = 0;
        }
        let op = kind.op();
        let title = kind.title();
        let body = kind.body();
        // `extra_warning`, not the raw line: for `Delete` the sentence already
        // ends in "This cannot be undone.", and rendering the line as well said
        // it twice.
        let warning = kind.extra_warning();
        let quote = kind.quoted_path();
        let icon = kind.icon();
        let icon_danger = kind.is_icon_dangerous();
        let fraction = kind.progress_fraction();
        let destructive = buttons.iter().any(|b| b.is_destructive());
        let focused_button = buttons[self.modal_focus];

        let mut clicked: Option<DlgButton> = None;
        // `dialog.scrim` is `egui::Modal`'s **backdrop colour**, which is the
        // one thing that draws it — not a second rect painted here.
        //
        // The previous version painted `ui.max_rect()` itself and left
        // `Modal`'s own default backdrop in place, so the window behind the
        // dialog was darkened *twice* (38% + 39%: 0.62 x 0.61 = 0.38 of the
        // original, against the 0.62 the spec asks for), and the painted rect
        // was the *list* rectangle rather than the window, so it did not even
        // cover the toolbar and the status bar. `Modal`'s backdrop covers
        // `ctx.content_rect()`, which is what "a scrim over everything" means,
        // and it also eats the clicks — the hand-painted rect never did.
        egui::Modal::new(egui::Id::new("kestrel-dialog"))
            .frame(egui::Frame::NONE)
            .backdrop_color(dialog::scrim(&theme))
            .show(ui.ctx(), |ui| {
                // The area's own top-left, and a *finite* height. An infinite
                // one is the obvious way to say "as tall as it needs", and egui
                // answers it with `Rect::NOTHING` and a NaN: `align_size_within_rect`
                // takes a midpoint of two infinities. The frame sizes itself to
                // its content regardless, so the available height is only a
                // ceiling.
                let ceiling = ui.max_rect().size();
                let box_rect =
                    Rect::from_min_size(ui.max_rect().min, vec2(dialog::WIDTH, ceiling.y));
                ui.scope_builder(egui::UiBuilder::new().max_rect(box_rect), |ui| {
                    let frame = egui::Frame::new()
                        .fill(dialog::background(&theme))
                        .inner_margin(egui::Margin::same(dialog::PADDING as i8))
                        .corner_radius(radius::all(dialog::RADIUS))
                        // `dialog.border` — 1px `border.strong`, so the dialog
                        // reads as an object even where the scrim is subtle.
                        .stroke(Stroke::new(border::HAIRLINE, theme.borders.strong));
                    frame.show(ui, |ui| {
                        // --- title row: icon + title ---
                        ui.horizontal(|ui| {
                            // The icon is **allocated**, not painted at the
                            // cursor. Painting it left the cursor 8px further on
                            // than the icon's own 20px, so a 20px glyph was
                            // overlapped by the first letter of the title. The
                            // bug was invisible until the capture path could
                            // actually typeset text: with every glyph a solid
                            // block it just looked like a wide icon.
                            let (ir, _) = ui.allocate_exact_size(
                                vec2(dialog::ICON, dialog::ICON),
                                Sense::hover(),
                            );
                            tokens::icon_glyph(
                                ui.painter(),
                                ir,
                                icon,
                                if icon_danger {
                                    theme.status.danger_text
                                } else {
                                    theme.icon.chrome
                                },
                            );
                            ui.add_space(space::S1);
                            ui.label(
                                RichText::new(title.clone())
                                    .font(tokens::font(ty::DIALOG_TITLE, &theme))
                                    .color(theme.text.primary),
                            );
                        });
                        ui.add_space(space::S2);
                        // --- body ---
                        ui.label(
                            RichText::new(body.clone())
                                .font(tokens::font(ty::DIALOG_BODY, &theme))
                                .color(theme.text.secondary),
                        );
                        // --- the irreversibility line, in danger text ---
                        if let Some(w) = warning {
                            ui.add_space(space::S1);
                            ui.label(
                                RichText::new(w)
                                    .font(tokens::font(ty::CAPTION, &theme))
                                    .color(if destructive {
                                        theme.status.danger_text
                                    } else {
                                        theme.text.tertiary
                                    }),
                            );
                        }
                        // --- the progress bar, on a `Progress` dialog ---
                        //
                        // §4.7 has no tokens for one, so it is derived from §4.5's
                        // free-space meter — the only determinate meter already in
                        // the spec, and the right shape for "how far along is
                        // this". A progress dialog with no progress indicator is
                        // a dialog that interrupts the user to tell them a number.
                        if let Some(fraction) = fraction {
                            ui.add_space(space::S2);
                            let (bar, _) = ui.allocate_exact_size(
                                vec2(ui.available_width(), dialog::PROGRESS_BAR_H),
                                Sense::hover(),
                            );
                            ui.painter().rect_filled(
                                bar,
                                radius::all(radius::XS),
                                theme.state.hover,
                            );
                            let fill = Rect::from_min_size(
                                bar.min,
                                vec2(bar.width() * fraction, bar.height()),
                            );
                            ui.painter().rect_filled(
                                fill,
                                radius::all(radius::XS),
                                theme.accent.base,
                            );
                        }
                        // --- `dialog.path-quote` ---
                        if let Some(path) = quote {
                            ui.add_space(space::S2);
                            let quoted = format::middle_truncate(
                                &path.to_string_lossy(),
                                (ui.available_width() / 6.5) as usize,
                            );
                            let (qr, _) = ui.allocate_exact_size(
                                vec2(ui.available_width(), component::DIALOG_PATH_QUOTE_H),
                                Sense::hover(),
                            );
                            ui.painter().rect_filled(
                                qr,
                                radius::all(component::DIALOG_PATH_QUOTE_RADIUS),
                                theme.state.hover,
                            );
                            ui.painter().text(
                                qr.left_center() + vec2(component::DIALOG_PATH_QUOTE_PAD_X, 0.0),
                                Align2::LEFT_CENTER,
                                quoted,
                                tokens::font(ty::META, &theme),
                                theme.text.secondary,
                            );
                        }
                        // --- footer, right-aligned per `dialog.footer-align` ---
                        ui.add_space(dialog::FOOTER_GAP);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            // `space::S1_5` between buttons rather than the
                            // theme's 8px. §4.7 specifies no inter-button gap,
                            // and the collision dialog is the one kind with five
                            // buttons: at 10px between them they are 40px of
                            // padding competing with the labels inside a 400px
                            // box. Six is the chrome band and it is what the
                            // row can afford.
                            ui.spacing_mut().item_spacing.x = space::S1_5;
                            // Laid right-to-left, so the *last* button in the
                            // list ends up leftmost... which is wrong: §4.7
                            // requires Cancel leftmost. So the list is walked in
                            // reverse to undo the layout, keeping one source of
                            // truth for the order.
                            for b in buttons.iter().rev().copied() {
                                if self.dialog_button(ui, b, b == focused_button, op) {
                                    clicked = Some(b);
                                }
                            }
                        });
                    });
                });
            });

        if let Some(b) = clicked {
            self.modal_focus = buttons.iter().position(|x| *x == b).unwrap_or(0);
            match &kind {
                DlgKind::Collision { .. } => self.answer_collision(b),
                DlgKind::Confirm { .. } | DlgKind::Failed { .. } => {
                    self.modal = None;
                    self.answer_confirm(b)
                }
                DlgKind::CannotOpen { .. } => {
                    self.modal = None;
                }
                DlgKind::Progress { .. } => {
                    // Stop.
                    if let Some(job) = self.job.as_ref() {
                        job.cancel();
                    }
                    self.modal = None;
                }
            }
        }
    }

    /// One dialog button, with `dialog.btn-focus-ring` on the focused one.
    fn dialog_button(&self, ui: &mut Ui, b: DlgButton, focused: bool, op: job::Op) -> bool {
        let theme = self.theme;
        // §4.7 `dialog.btn-label` is "`type.ui`, **500**" — see
        // `component::DIALOG_BTN_LABEL` for why that is `ty::UI_STRONG` and not
        // `ty::UI`.
        let font = tokens::font(component::DIALOG_BTN_LABEL, &theme);
        let text_w = crate::widgets::text_width(ui, b.label(), font.clone());
        // `dialog.btn-padding-x` on each side, from the token, not from a
        // multiple of the height: 14 x 2 is 28, and `BTN_HEIGHT * 0.9` was 27.
        let w = text_w + component::DIALOG_BTN_PADDING_X * 2.0;
        let (rect, response) = ui.allocate_exact_size(vec2(w, dialog::BTN_HEIGHT), Sense::click());
        let hovered = response.hovered();
        ui.painter().rect_filled(
            rect,
            radius::all(dialog::RADIUS * 0.5),
            dialog::button_bg(&theme, b, hovered),
        );
        if let Some(stroke) = dialog::button_border(&theme, b) {
            crate::widgets::rect_stroke(
                ui.painter(),
                rect,
                radius::all(dialog::RADIUS * 0.5),
                stroke,
            );
        }
        // §4.7 `dialog.btn-focus-ring` — 2px `focus.ring` at 2px offset. Drawn on
        // the *focused* button, which on open is always the safe one. The
        // previous version wrote `focused || b.is_default_for_enter(op) && focused`,
        // whose second arm is implied by its first: the ring follows the focus
        // index and nothing else, and `Button::is_default_for_enter` documents
        // *which* button that index is for each op.
        let _ = op;
        if focused {
            crate::widgets::rect_stroke(
                ui.painter(),
                rect.expand(2.0),
                radius::all(dialog::RADIUS * 0.5 + 2.0),
                Stroke::new(2.0, theme.borders.focus_ring),
            );
        }
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            b.label(),
            font,
            dialog::button_text(&theme, b),
        );
        response.clicked()
    }

    /// `true` when a sidebar and a usable list both fit in `width`.
    ///
    /// Measured against [`metric::SIDEBAR_WIDTH`] rather than
    /// [`metric::SIDEBAR_MIN`], and the difference is a real limit: the sidebar
    /// is resizable, so a user who drags it to `SIDEBAR_MAX` (340) in a 640px
    /// window *will* leave the list short. The check cannot see that — egui
    /// keeps the dragged width in panel state and exposes no getter — and the
    /// honest options are to be conservative to the point of hiding the sidebar
    /// at 500px for a reason that is not the user's fault, or to answer for the
    /// width the sidebar actually has on first run and let a user who drags it
    /// very wide know they have. The second is what this does.
    fn sidebar_fits(&self, width: f32) -> bool {
        width - metric::SIDEBAR_WIDTH >= LIST_MIN_W
    }

    /// The preview pane.
    ///
    /// Every token here is **derived** — the spec has no §4 section for a preview
    /// pane, only a `Space` binding for "Quick look". See
    /// [`crate::dialog::preview_metrics`], which says which scale each value came
    /// from, so a future §4 section replaces these rather than having to
    /// reconcile two sets of numbers.
    ///
    /// **Never blank.** §4.2's empty-state rule applies with more force here: a
    /// preview pane that shows nothing is indistinguishable from a broken one, so
    /// every path renders *something* — the icon, the type, the size, the modified
    /// time — and only the body is conditional.
    ///
    /// There are two widths and one of them is not the setting. A focused row
    /// gets the pane the user asked for; nothing focused gets a rail. The width
    /// is [`preview_plan`]'s to decide and `exact_size`'s to apply, and the long
    /// comment at the panel's construction is the one that says why those two
    /// are the right pair — read it before changing either.
    fn preview_panel(&mut self, ui: &mut Ui) {
        let theme = self.theme;
        // The pane gives way before the list does, and only after it has been
        // asked to. Three panes in a 640px window leave the list 160px — one
        // name column with no room for a size, which is not a file manager, it
        // is a column of truncated names. That order lives in `preview_plan`;
        // this is only its caller.
        let available = ui.available_width();
        let room = available - LIST_MIN_W;
        // The predicate is the same test `focused_preview_target` makes, minus
        // the `PathBuf` copy: focus is set *and* the row is still on screen. The
        // second half matters — a focus index pointing at a row the filter has
        // hidden has no preview, so it must size the pane like any other empty
        // one, or the pane would be full-width and empty. `focus()` is O(1) and
        // allocates nothing, and it is read here rather than inside the closure
        // so `&mut self` is still unaliased when the panel is built.
        let plan = preview_plan(room, self.preview_width, self.has_preview_target());
        let width = match plan {
            PanePlan::Hidden => {
                self.pane_too_narrow = true;
                return;
            }
            PanePlan::Strip { width } | PanePlan::Full { width } => width,
        };
        // --- the width policy, in the only three lines that matter -----------
        //
        // `exact_size`, not `default_size`, and the difference is the whole bug
        // this replaced. egui's `Panel::outer_size` (0.36.2, `panel.rs:1064`)
        // reads a **persisted** `PanelState` first and only falls back to
        // `default_size` when there is none, then clamps whatever it got into
        // `outer_size_range`. Panel memory persistence is on by default, so from
        // the second frame onwards — and across restarts, from `app.ron` — the
        // stored rect wins and `default_size` is dead code. The Settings stepper
        // changed `preview_width` and the pane did not move; a drag of the
        // separator was the only thing that ever moved it.
        //
        // `exact_size` does not make the default win. It sets
        // `outer_size_range = Rangef::point(size)`, and the stored value is
        // still loaded — it is just clamped into a range one pixel wide. The
        // stored number cannot be the wrong number in a range that has only one
        // number in it.
        //
        // `resizable(false)` is stated rather than implied. A point range already
        // makes a drag a no-op, but `Panel::new` defaults to `resizable: true`
        // and a live drag handle over a pane that cannot be dragged is a lie the
        // next reader has to disprove by reading egui's source. §2.11 rule 2
        // covers the motion; this is the honesty.
        //
        // Do not reach for `Panel::show_collapsible` / `show_switched`. They
        // interpolate over `Style::animation_time` (200ms, never overridden here)
        // by translating the panel toward its fixed edge and letting the
        // allocation follow — §2.11 rule 2 bans animating `width` and `left`, and
        // §7.13 repeats it. Worse, at `how_expanded == 0.0` `show_collapsible`
        // returns `None` and draws no panel at all, which is the blank rectangle
        // "Never blank" exists to prevent. The strip is a width set by
        // `preview_plan` and applied with `exact_size`, recomputed every frame
        // from scalars — it has no state to interpolate, and it snaps on the
        // same frame the selection does, as §2.11 rule 1 requires.
        //
        // `min_size`/`max_size` are gone with it: `exact_size` replaces the whole
        // range, and leaving a second range behind would be the kind of
        // near-duplicate a later reader has to reconcile.
        egui::Panel::right("preview")
            .resizable(false)
            .exact_size(width)
            .frame(
                egui::Frame::new()
                    .fill(theme.surfaces.panel)
                    .inner_margin(margin(dialog::preview_metrics::PADDING))
                    // A layout sibling with a hairline, like every other sticky
                    // band: §2.12, and the reason focus-not-obscured is structural.
                    .stroke(Stroke::new(border::HAIRLINE, theme.borders.subtle)),
            )
            .show(ui, |ui| {
                // Pin the content to the full panel width, every frame.
                //
                // The pane's content is a column that spans the pane, so stating
                // that is not a workaround; it is what the pane is. It is *not*
                // how the pane's width is decided — that is `exact_size` and
                // `preview_plan`, above. This line only stops the content from
                // being narrower than the pane it lives in, which is what put
                // the eye icon flush against the strip's left edge.
                ui.set_min_width(ui.available_width());
                // The entry's preview *inputs* are read out first, so the loader
                // can be borrowed mutably. Holding `&FileEntry` across
                // `self.preview.request(..)` is the borrow checker correctly
                // pointing out that `self` is aliased.
                let Some(focus) = self.focused_preview_target() else {
                    self.preview_nothing(ui, &theme, matches!(plan, PanePlan::Strip { .. }));
                    return;
                };
                let kind = self
                    .preview
                    .request(&focus.path, focus.size, focus.is_image);
                if let Some(loaded) = self.preview.poll() {
                    self.preview_content = Some(loaded);
                }
                self.preview_body(ui, &theme, kind);
            });
    }

    /// Whether there is a row for the pane to describe.
    ///
    /// Exactly the condition [`Self::focused_preview_target`] tests, without
    /// building the `PreviewTarget` — the width decision runs before the panel
    /// exists and must not allocate a `PathBuf` to find out whether to be
    /// 80 or 280 pixels wide. Both halves are needed: a focus index is not the
    /// same as a visible row, and a focus pointing at a row the filter has
    /// hidden has no preview, so the pane sizes and draws as empty.
    fn has_preview_target(&self) -> bool {
        self.selection
            .focus()
            .is_some_and(|at| self.visible_row(at).is_some())
    }

    /// What the preview needs about the focused row, copied out so the loader
    /// can be borrowed mutably while the row is still being read.
    fn focused_preview_target(&self) -> Option<PreviewTarget> {
        let at = self.selection.focus()?;
        let entry = self.visible_row(at)?;
        Some(PreviewTarget {
            path: entry.path.clone(),
            size: entry.size,
            is_image: Classified::of(entry).category == filetype::Category::Image,
        })
    }

    /// The row the preview describes, if any.
    fn focused_entry(&self) -> Option<&FileEntry> {
        let at = self.selection.focus()?;
        self.visible_row(at)
    }

    /// Shown when nothing is focused. Metadata-shaped, never blank.
    ///
    /// `strip` is `true` when the pane is the narrow rail rather than a full
    /// pane — see [`PanePlan::Strip`]. This function owns both states on
    /// purpose: the icon, its centring, and the wording are one design decision,
    /// and two owners would be two chances to disagree about whether the rail is
    /// a pane.
    ///
    /// # Why the strip drops the words
    ///
    /// §2 says a label that cannot fit is a layout defect, not a label. At 80px
    /// wide, "Nothing selected" is six words' worth of a 22px display face in a
    /// 55px column: it would wrap to three lines, clip to two, and read as a
    /// rendering failure rather than as an empty state. The sentence beneath it
    /// is worse — it is the one element in the app that has to be *read*, and
    /// there is no width to read it in.
    ///
    /// So the rail is the icon, centred, and the sentence moves to a tooltip.
    /// §7.14 is unambiguous that an affordance nobody can find is an anti-goal,
    /// and §4.11's convention is that a shortcut is discoverable in exactly one
    /// place — the tooltip — so the rail says which pane it is and which key
    /// brings it back.
    ///
    /// # Why this is centred and not offset
    ///
    /// The previous version did `ui.vertical_centered` and then
    /// `add_space(available_height * 0.2)`, which produced two separate defects
    /// that read as one sloppy placement:
    ///
    /// * `vertical_centered` *shrinks* its `Ui` to the content's width, so the
    ///   block sat wherever the pane's own padding put it, not in the middle of
    ///   the pane; and
    /// * the glyph rect was built with `Rect::from_center_size(cursor + 24x, ...)`
    ///   without ever **allocating** it, so the cursor did not move past the
    ///   48px icon and the caption was laid out *inside* the icon's lower half.
    ///
    /// §4.2's empty state is "a real design moment": one 48px glyph at 40%, the
    /// subject, one sentence, and nothing else. The three are allocated in one
    /// column of exactly the pane's width, and that column is placed in the
    /// middle of the pane's remaining height. The subject is `type.empty-state`
    /// (`type.display`) in `text.primary` and the sentence is
    /// `row.empty-body` in `text.secondary` — the same two roles the file list's
    /// own empty state uses, so the two read as the same idea.
    fn preview_nothing(&mut self, ui: &mut Ui, theme: &Theme, strip: bool) {
        let icon = component::ROW_EMPTY_ICON_SIZE;
        let title = "Nothing selected";
        // Was: "Choose a file to preview it. Space opens quick look."
        //
        // Space does not do that. `Act::QuickLook` is bound to Space and sets
        // `show_preview = true` — which is already true whenever this pane is on
        // screen at all, so with the pane visible, Space does *nothing*. It is
        // a promise the app does not keep, printed in the app's own voice, and
        // §7.14 is an anti-goal by name. The sentence now says only what is
        // true: pick a file.
        let body = "Pick a file to see its details.";
        let title_font = tokens::font(component::ROW_EMPTY_TITLE, theme);
        let body_font = tokens::font(component::ROW_EMPTY_BODY, theme);
        let pane = ui.max_rect();
        // The block's height is **measured**, not summed from font sizes.
        //
        // `type.empty-state` is 22px but its line box is 28, `type.dialog-body` is
        // 13px in a 20px box, and the body wraps to two lines in a 280px pane. A
        // sum of the sizes is short by 10px before the wrap is counted at all,
        // which puts the block a visible few pixels low. `Painter::layout` is the
        // same galley `ui.label` will lay out, so the number is exact and the
        // galley cache makes it free after the first frame.
        //
        // A strip has no title and no body, so it measures nothing: its column is
        // the icon and nothing else, and the icon is still placed by the same
        // arithmetic as the full block. One centring rule, two states.
        let (title_h, body_h) = if strip {
            (0.0, 0.0)
        } else {
            (
                ui.painter()
                    .layout(
                        title.to_string(),
                        title_font.clone(),
                        theme.text.primary,
                        pane.width(),
                    )
                    .size()
                    .y,
                ui.painter()
                    .layout(
                        body.to_string(),
                        body_font.clone(),
                        theme.text.secondary,
                        pane.width(),
                    )
                    .size()
                    .y,
            )
        };
        // The gaps go with their labels. `S3` separated the icon from the
        // subject and `S1` the subject from the sentence; a strip has neither,
        // and a trailing `S3` below a lone icon would push it 6px above the
        // vertical centre — enough to be visible and impossible to explain.
        let column_h = if strip {
            icon
        } else {
            icon + space::S3 + title_h + space::S1 + body_h
        };
        let top = ((pane.height() - column_h) / 2.0).max(0.0);
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                pos2(pane.left(), pane.top() + top),
                vec2(pane.width(), column_h),
            )),
            |ui| {
                // `vertical_centered` is egui's "horizontally centred column"; the
                // vertical centring is the `top` above. Naming that here is the
                // point: the earlier `add_space(available_height * 0.2)` was an
                // invented fraction of a space that has nothing to do with the
                // block's height.
                ui.vertical_centered(|ui| {
                    let (rect, response) = ui.allocate_exact_size(vec2(icon, icon), Sense::hover());
                    tokens::icon_glyph(
                        ui.painter(),
                        rect,
                        icons::EYE,
                        component::icon_at(theme.icon.chrome, component::ROW_EMPTY_ICON_ALPHA),
                    );
                    if strip {
                        // The only place in the rail that can name what it is.
                        // `Ctrl+P` is `Act::TogglePreview` — the key that actually
                        // brings the pane back once it has been switched off, so
                        // the tooltip advertises the one shortcut that works here
                        // rather than the one that does nothing.
                        response.on_hover_text("Preview · Ctrl+P");
                        return;
                    }
                    ui.add_space(space::S3);
                    ui.label(
                        RichText::new(title)
                            .font(title_font)
                            .color(theme.text.primary),
                    );
                    ui.add_space(space::S1);
                    ui.label(
                        RichText::new(body)
                            .font(body_font)
                            .color(theme.text.secondary),
                    );
                });
            },
        );
    }

    /// The pane's body: header, then whatever the kind allows.
    ///
    /// The focused row is re-fetched here rather than passed in, so the whole
    /// function owns its borrows and the loader is free to be mutable.
    fn preview_body(&mut self, ui: &mut Ui, theme: &Theme, kind: PvKind) {
        let Some(entry) = self.focused_entry() else {
            return;
        };
        let classified = Classified::of(entry);
        // The name is the texture cache key and the header's text, and it has to
        // outlive the mutable borrow of `self` that the image arm takes.
        let name = entry.name.clone();
        let available = ui.available_width();

        // --- header: icon, name, kind, size, modified. Always rendered. -----
        ui.horizontal(|ui| {
            // Allocated, not painted at the cursor — see the same note in
            // `modal_layer`. Painted, the 16px icon shared space with the first
            // two characters of the filename.
            let (ir, _) = ui.allocate_exact_size(
                vec2(component::ROW_ICON_SIZE, component::ROW_ICON_SIZE),
                Sense::hover(),
            );
            if let Some(g) = classified.glyph() {
                tokens::icon_glyph(ui.painter(), ir, g, classified.icon_color(&theme.icon));
            } else {
                tokens::icon_placeholder(ui.painter(), ir, classified.icon_color(&theme.icon));
            }
            ui.add_space(component::ROW_ICON_GAP);
            let name_font = tokens::font(ty::UI_STRONG, theme);
            let budget = truncation_cols(
                ui.painter(),
                &name,
                &name_font,
                (available - component::ROW_ICON_SIZE - component::ROW_ICON_GAP).max(0.0),
            );
            ui.label(
                RichText::new(format::middle_truncate(&name, budget))
                    .font(name_font)
                    .color(theme.text.primary),
            );
        });
        ui.add_space(space::S1);
        // A `meta` line: kind · size · modified. §2.8's machine values, in mono.
        let size = match (entry.kind, entry.size) {
            (EntryKind::Directory, _) => format::NOT_APPLICABLE.to_string(),
            (_, Some(n)) => format::bytes(n),
            (_, None) => "-".to_string(),
        };
        let modified = entry
            .modified
            .map_or_else(|| "-".to_string(), format::timestamp);
        let meta_font = tokens::font(ty::META, theme);
        ui.label(
            RichText::new(meta_line(
                ui.painter(),
                &meta_font,
                available,
                // Dropped from the right, so the **size** goes first: a file
                // list shows a size in a column of its own, while the timestamp
                // appears nowhere else in the pane.
                &[classified.category.label(), &modified, &size],
            ))
            .font(meta_font)
            .color(theme.text.tertiary),
        );
        ui.add_space(space::S2);
        widgets::rule(
            ui.painter(),
            ui.cursor().left_top().x,
            ui.available_width(),
            ui.cursor().left_top().y,
            component::hairline(theme),
        );
        ui.add_space(space::S2);

        // --- body, by kind. Never empty: every arm renders something. -------
        match kind {
            PvKind::Image => self.preview_image(ui, theme, &name),
            PvKind::Text => self.preview_text(ui, theme, available),
            PvKind::TooLarge => {
                let n = entry.size.unwrap_or(pv::MAX_PREVIEW_BYTES + 1);
                ui.label(
                    RichText::new("Too large to preview")
                        .font(tokens::font(ty::UI, theme))
                        .color(theme.text.secondary),
                );
                // "stops at" rather than "reads at most": both numbers are
                // rounded to one decimal, so a file 4 KiB over the cap formatted
                // as "1.0 MiB — the preview reads at most 1.0 MiB", which reads as
                // a contradiction. Naming the second number as the *threshold*
                // keeps the sentence true at every size.
                ui.label(
                    RichText::new(format!(
                        "{} — the preview stops at {}.",
                        format::bytes(n),
                        format::bytes(pv::MAX_PREVIEW_BYTES)
                    ))
                    .font(tokens::font(ty::CAPTION, theme))
                    .color(theme.text.tertiary),
                );
            }
            PvKind::Unknown => {
                ui.label(
                    RichText::new("Size unknown")
                        .font(tokens::font(ty::UI, theme))
                        .color(theme.text.secondary),
                );
                ui.label(
                    RichText::new("The preview needs a size to apply its limit safely.")
                        .font(tokens::font(ty::CAPTION, theme))
                        .color(theme.text.tertiary),
                );
            }
            PvKind::Metadata => {
                ui.label(
                    RichText::new(classified.category.label())
                        .font(tokens::font(ty::UI, theme))
                        .color(theme.text.secondary),
                );
                ui.label(
                    RichText::new("This kind of file has no preview.")
                        .font(tokens::font(ty::CAPTION, theme))
                        .color(theme.text.tertiary),
                );
            }
        }
    }

    /// An image, via egui's loaders.
    ///
    /// Centred in the pane's remaining space and outlined in `border.subtle`:
    /// an image with a transparent background is otherwise invisible against
    /// `surface.panel`, and "the preview pane is blank" is indistinguishable from
    /// "the preview is broken" — which is the one thing §4.2's empty-state rule
    /// exists to prevent.
    fn preview_image(&mut self, ui: &mut Ui, theme: &Theme, entry_name: &str) {
        let Some(PvLoaded::Image { image, size }) = self.preview_content.clone() else {
            return self.preview_waiting(ui, theme);
        };
        // **No decoder here.** The bytes were read *and decoded* on the preview
        // worker (`preview::Loader::spawn` -> `load` -> `decode_image`), and all
        // this does is hand finished pixels to the texture manager. The decode
        // used to happen at this line, which meant a 4000x3000 PNG froze the
        // frame for 150-400ms on every arrow press — the one file type whose
        // whole job is decoding, blocking the one thread that must not block.
        //
        // What is left is a `load_texture` call, and even that is once-per-file
        // rather than once-per-frame: the name is the file's name, not a
        // pointer, so egui's manager hits its own cache on every redraw. A
        // pointer key would change every frame and re-upload the image each time.
        let handle = ui.ctx().load_texture(
            format!("preview:{}", entry_name),
            image,
            egui::TextureOptions::LINEAR,
        );
        // The size is the *reduced* size the worker reported, read before the
        // image is handed over, so there is no texture-manager round trip and no
        // way for the two to disagree about the aspect ratio.
        let available = ui.available_size();
        let drawn = fit(size, available);
        // Reserve the whole remaining space and centre the image in it, both
        // ways. `Align::Center` on a top-down layout only centres the cross axis
        // (x), so the vertical centring has to be arithmetic — and an image hard
        // against the header rule with 600px of blank pane under it reads as a
        // failed preview rather than a small one.
        let top = ((available.y - drawn.y) / 2.0).max(0.0);
        let left = ((available.x - drawn.x) / 2.0).max(0.0);
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(egui::Rect::from_min_size(ui.cursor().left_top(), available))
                .layout(Layout::top_down(Align::Min)),
            |ui| {
                let (rect, _) = ui.allocate_exact_size(drawn, Sense::hover());
                let rect = rect.translate(vec2(left, top));
                egui::Image::new(egui::load::SizedTexture::new(&handle, drawn)).paint_at(ui, rect);
                widgets::rect_stroke(
                    ui.painter(),
                    rect,
                    radius::all(radius::NONE),
                    Stroke::new(border::HAIRLINE, theme.borders.subtle),
                );
            },
        );
    }

    /// Syntax-highlighted text, **virtualized**.
    ///
    /// The same rule as the file list: a 1 MiB file is 60,000 lines, and building
    /// 60,000 rows per frame would drop frames. So `show_rows`, and only the
    /// visible slice is laid out.
    ///
    /// `width` is the pane's own content width, measured by the caller **before**
    /// the `ScrollArea` — for the same reason the file list does not read it
    /// inside one.
    fn preview_text(&mut self, ui: &mut Ui, theme: &Theme, width: f32) {
        let Some(PvLoaded::Text { lines, truncated }) = self.preview_content.clone() else {
            return self.preview_waiting(ui, theme);
        };
        if truncated {
            ui.label(
                RichText::new("Shown truncated.")
                    .font(tokens::font(ty::MICRO, theme))
                    .color(theme.status.warning_text),
            );
        }
        if lines.is_empty() {
            ui.label(
                RichText::new("This file is empty.")
                    .font(tokens::font(ty::CAPTION, theme))
                    .color(theme.text.tertiary),
            );
            return;
        }
        let line_h = dialog::preview_metrics::LINE_H;
        let total = lines.len();
        let gutter = dialog::preview_metrics::GUTTER_W;
        let font = tokens::font(ty::META, theme);
        let palette = pv::palette(theme);
        ScrollArea::vertical()
            .id_salt("preview-text")
            .auto_shrink([false, false])
            .show_rows(ui, line_h, total, |ui, range| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for i in range {
                    let Some(line) = lines.get(i) else { continue };
                    let y = ui.cursor().top();
                    // The gutter, in `text.tertiary`, right-aligned.
                    ui.painter().text(
                        egui::pos2(
                            ui.cursor().left_top().x + gutter - space::S1,
                            y + line_h / 2.0,
                        ),
                        Align2::RIGHT_CENTER,
                        format!("{}", line.number),
                        font.clone(),
                        theme.text.tertiary,
                    );
                    let text_x = ui.cursor().left_top().x + gutter;
                    self.paint_code_line(
                        ui.painter(),
                        line,
                        text_x,
                        y,
                        line_h,
                        width - gutter,
                        font.clone(),
                        &palette,
                        theme,
                    );
                    ui.allocate_space(vec2(width, line_h));
                }
            });
    }

    /// One line of highlighted code, coloured by token run.
    ///
    /// Widths are **accumulated**, not computed from byte offsets. A byte offset
    /// is not a pixel offset: `日本語` is 3 bytes and 3 ems wide, and `é` is 2
    /// bytes and about half an em. The first version of this used `cx as f32`
    /// and drew every token run on top of the first one, which looked correct
    /// for ASCII and wrong for every other script in the app's own test corpus.
    ///
    /// One `painter.text` call per run rather than per character: egui's galley
    /// cache is keyed on the whole string, so a per-character call would be a
    /// per-character cache miss.
    #[allow(clippy::too_many_arguments)]
    fn paint_code_line(
        &self,
        painter: &egui::Painter,
        line: &pv::Line,
        x: f32,
        y: f32,
        line_h: f32,
        max_w: f32,
        font: egui::FontId,
        palette: &pv::Palette,
        theme: &Theme,
    ) {
        let baseline = egui::pos2(x, y + line_h / 2.0);
        // Measure the whole line first, so a long line can be middle-truncated
        // (§2.8: all list text is single-line, centred, middle-truncated) rather
        // than being drawn off the pane's edge. The budget is measured in the
        // code face, not in the sans average — see [`truncation_cols`], which
        // exists because this line used to overflow the pane by exactly the
        // difference between the two advances.
        let cols = truncation_cols(painter, &line.text, &font, max_w);
        if cols < line.text.chars().count() {
            painter.text(
                baseline,
                Align2::LEFT_CENTER,
                format::middle_truncate(&line.text, cols),
                font,
                theme.text.primary,
            );
            return;
        }

        let mut cursor = x;
        let mut start = 0usize;
        while start < line.spans.len() {
            let token = pv::Token::from_u8(line.spans[start]);
            let mut end = start;
            while end < line.spans.len() && pv::Token::from_u8(line.spans[end]) == token {
                end += 1;
            }
            let run = &line.text[start..end];
            if !run.is_empty() {
                let color = palette.color(token, theme);
                let galley = painter.layout_no_wrap(run.to_string(), font.clone(), color);
                // `galley()` consumes the `Arc`, so the width is read first.
                let run_w = galley.size().x;
                painter.galley(egui::pos2(cursor, baseline.y), galley, theme.text.primary);
                cursor += run_w;
            }
            start = end;
        }
    }

    fn preview_waiting(&mut self, ui: &mut Ui, theme: &Theme) {
        ui.label(
            RichText::new(if self.preview.is_pending() {
                "Loading…"
            } else {
                "Reading…"
            })
            .font(tokens::font(ty::CAPTION, theme))
            .color(theme.text.tertiary),
        );
    }

    /// §4.4 The toolbar. Replaces the Phase 2 sidebar toggle.
    fn top_toolbar(&mut self, ui: &mut Ui) {
        let theme = self.theme;
        let has_filter = !self.filter.is_empty();
        let btns = toolbar::buttons(
            self.history.can_go_back(),
            self.history.can_go_forward(),
            self.dir.parent().is_some(),
            self.view == ViewMode::Tree,
            has_filter,
        );

        let ahead = self.history.forward_len();
        let mut pending: Option<Action> = None;
        // Measured **outside** the panel, against the root `Ui`: a `Panel` sized
        // against its own content has no idea how much room it is being given,
        // so deciding the density inside one would be deciding it against the
        // number of pixels the previous frame happened to paint.
        let plan = toolbar::plan(ui, &theme, ui.available_width(), &btns);
        let mut filter_rect: Option<Rect> = None;
        egui::Panel::top("toolbar")
            .exact_size(metric::TOOLBAR)
            .frame(toolbar::frame(&theme))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    // §4.4: proximity does the grouping, so separators sit only
                    // between functional groups, never between adjacent buttons.
                    //
                    // `plan.shows` is the one place that decides whether the
                    // `Filter` button is drawn: it is drawn only in the stage
                    // where the field was left out for space, so the two filter
                    // controls are mutually exclusive rather than both
                    // appearing because the field "had priority".
                    for b in btns.iter().filter(|b| plan.shows(b)) {
                        if toolbar::separator_before(b) {
                            toolbar::separator(ui, &theme);
                        }
                        if toolbar::draw(ui, &theme, *b, ahead, plan.density).clicked() {
                            pending = Some(b.action);
                        }
                    }
                    if let Some(width) = plan.filter {
                        toolbar::separator(ui, &theme);
                        filter_rect = Some(self.filter_field(ui, &theme, width));
                    }
                });
            });

        // §7.14 and the narrow-window contract: a window too small for even the
        // icon row says so, rather than letting buttons run off the right edge
        // with no indication that the toolbar has been cut. The status bar is
        // where a permanent condition belongs (§4.5), and it is the only band
        // that is not the thing being clipped.
        self.toolbar_narrow = plan.filter.is_none();

        if let Some(action) = pending {
            self.apply(action);
        }
    }

    /// Runs a toolbar action.
    fn apply(&mut self, action: Action) {
        match action {
            Action::Back => self.go_back(),
            Action::Forward => self.go_forward(),
            Action::Up => self.go_up(),
            Action::Refresh => self.refresh(),
            Action::ViewList => self.set_view(ViewMode::List),
            Action::ViewTree => self.set_view(ViewMode::Tree),
            Action::Sort => self.cycle_sort(),
            Action::Filter => self.filter_focused = true,
            Action::Theme => self.cycle_theme(),
            Action::Settings => self.open_settings(),
            Action::Help => self.help = true,
        }
    }

    /// §4.8 The search field: `Filter…` placeholder, leading glyph, a live
    /// match count *outside* the field, and a clear button that appears only
    /// when the query is non-empty.
    fn filter_field(&mut self, ui: &mut Ui, theme: &Theme, width: f32) -> Rect {
        let height = component::INPUT_HEIGHT;
        // The width is [`toolbar::plan`]'s, never `min(pref, available)`. The
        // old `min` is what produced a 91px field whose right half sat outside
        // the window: it answered "how much is left" with a number and then
        // drew a field of that width anyway, including the clear button and the
        // text inset that assume there is room for them.
        let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());

        let focused = self.filter_focused || response.has_focus();
        self.filter_focused = focused;

        let painter = ui.painter();
        painter.rect_filled(
            rect,
            radius::all(component::INPUT_RADIUS),
            theme.surfaces.input,
        );
        let border = if focused {
            theme.borders.accent
        } else {
            theme.borders.default
        };
        widgets::rect_stroke(
            painter,
            rect,
            radius::all(component::INPUT_RADIUS),
            Stroke::new(border::HAIRLINE, border),
        );
        // `input.ring` — 2px `focus.ring` at 1px offset.
        if focused {
            widgets::rect_stroke(
                painter,
                rect.expand(border::HAIRLINE),
                radius::all(component::INPUT_RADIUS + 1.0),
                Stroke::new(border::HAIRLINE, theme.borders.focus_ring),
            );
        }

        // Leading `magnifying-glass`, 16px.
        let icon_rect = Rect::from_center_size(
            pos2(
                rect.left() + component::INPUT_PADDING_X + component::INPUT_ICON_SIZE / 2.0,
                rect.center().y,
            ),
            vec2(component::INPUT_ICON_SIZE, component::INPUT_ICON_SIZE),
        );
        tokens::icon_glyph(
            painter,
            icon_rect,
            icons::MAGNIFYING_GLASS,
            theme.icon.chrome,
        );

        // The text. A `TextEdit` owns its own background and focus ring, so the
        // field is composed: a background painted above, and a `TextEdit` with
        // both stripped, so exactly the tokens above are what the user sees.
        // The painter borrow ends before the `TextEdit` is allocated, because
        // `TextEdit` needs `&mut Ui`.
        let text_left = icon_rect.right() + component::INPUT_ICON_GAP;
        let clear_w = if self.filter.is_empty() {
            0.0
        } else {
            component::INPUT_CLEAR_BTN
        };
        let inner = Rect::from_min_max(
            pos2(text_left, rect.top() + border::HAIRLINE),
            pos2(
                rect.right() - component::INPUT_PADDING_X - clear_w,
                rect.bottom() - border::HAIRLINE,
            ),
        );
        let edit_id = ui.id().with("filter");
        let changed = {
            let edit = egui::TextEdit::singleline(&mut self.filter)
                .id(edit_id)
                .desired_width(inner.width())
                // The field's own chrome is drawn above; a `TextEdit` frame would
                // repaint over it with egui's stock widget styling.
                .frame(egui::Frame::NONE)
                .text_color(theme.text.primary)
                .hint_text(egui::RichText::new(component::SEARCH_PLACEHOLDER));
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(inner)
                    .layout(Layout::left_to_right(Align::Center)),
                |ui| edit.show(ui),
            )
            .response
            .changed()
        };
        if changed {
            // Filtering is a display concern; no rescan.
            self.selection.clear();
            self.scroll_rows = 0;
        }
        // Re-borrow the painter for the clear button.
        let painter = ui.painter();

        // The clear button, only when there is something to clear.
        if clear_w > 0.0 {
            let clear_rect = Rect::from_center_size(
                pos2(
                    rect.right() - component::INPUT_PADDING_X - component::INPUT_CLEAR_BTN / 2.0,
                    rect.center().y,
                ),
                vec2(component::INPUT_CLEAR_BTN, component::INPUT_CLEAR_BTN),
            );
            let hit = ui.interact(clear_rect, ui.id().with("filter-clear"), Sense::click());
            if hit.hovered() {
                painter.rect_filled(clear_rect, radius::all(radius::SM), theme.state.hover);
            }
            tokens::icon_glyph(
                painter,
                clear_rect,
                icons::X,
                component::icon_at(theme.icon.chrome, if hit.hovered() { 1.0 } else { 0.4 }),
            );
            if hit.clicked() {
                self.filter.clear();
            }
        }
        rect
    }

    /// §4.1 Sidebar: the Places list, and nothing else.
    ///
    /// The Phase 2 theme toggle lived here because there was no toolbar. §4.4
    /// puts it in the toolbar, and §4.1 scopes the sidebar to places, bookmarks
    /// and volumes — so the sidebar is now exclusively Places.
    fn sidebar(&mut self, ui: &mut Ui) {
        let theme = self.theme;
        egui::Panel::left("sidebar")
            .resizable(true)
            // For a left panel, `default_size`/`min_size`/`max_size` are the
            // *width*; for a top/bottom panel they are the height.
            .default_size(metric::SIDEBAR_WIDTH)
            .min_size(metric::SIDEBAR_MIN)
            .max_size(metric::SIDEBAR_MAX)
            .frame(
                egui::Frame::new()
                    .fill(theme.surfaces.panel)
                    .inner_margin(margin(space::S1))
                    // §4.1 `sidebar.divider` — 1px `border.subtle`. Chrome is
                    // separated by hairlines, never by shadow (§7.8).
                    .stroke(Stroke::new(border::HAIRLINE, theme.borders.subtle))
                    .corner_radius(radius::all(radius::NONE)),
            )
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = space::HALF;
                let height = ui.available_height();
                ui.allocate_ui(vec2(ui.available_width(), height), |ui| {
                    widgets::section_label(ui, &theme, "Places");
                    ui.add_space(space::S1);

                    // `Place` is cloned per row rather than borrowed: clicking one
                    // calls `open_dir`, which needs `&mut self`, and the borrow
                    // checker is right that holding `&self.places` across it is
                    // an aliasing hazard waiting for a second reader. Five rows
                    // of a four-field struct is not a cost worth a `RefCell`.
                    for place in self.places.clone() {
                        let active = place.path().is_some_and(|p| p == self.dir);
                        let clicked = self.place_row(ui, &theme, &place, active);
                        if clicked {
                            if let Some(path) = place.path() {
                                let path = path.to_path_buf();
                                self.open_dir(path);
                            }
                        }
                    }
                });
            });
    }

    /// One sidebar place row (§4.1). Returns `true` when clicked.
    fn place_row(&self, ui: &mut Ui, theme: &Theme, place: &Place, active: bool) -> bool {
        let height = component::SIDEBAR_ITEM_HEIGHT;
        let width = ui.available_width();
        let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());

        let (bg, text_color, icon_color) = if !place.is_available() {
            // `disabled` — `text.disabled`, no hover.
            (
                component::SIDEBAR_ITEM_BG_DISABLED,
                theme.text.disabled,
                component::icon_at(theme.icon.chrome, 0.4),
            )
        } else if active {
            // §4.1 `active-place` — `accent.subtle-bg` + 3px bar +
            // `text.primary` + `accent.base` icon. The active place is the ONLY
            // sidebar item that uses the accent; if more than one ever does, the
            // rule has broken.
            (
                theme.accent.subtle_bg,
                theme.text.primary,
                theme.icon.chrome_active,
            )
        } else if response.hovered() {
            (theme.state.hover, theme.text.primary, theme.icon.chrome)
        } else {
            (
                component::SIDEBAR_ITEM_BG,
                theme.text.secondary,
                theme.icon.chrome,
            )
        };

        let painter = ui.painter();
        if bg != egui::Color32::TRANSPARENT {
            painter.rect_filled(rect, radius::all(component::SIDEBAR_ITEM_RADIUS), bg);
        }
        if active {
            // `sidebar.item-active-bar` — 3px, `state.selected-bar`, left, full
            // height, radius 0.
            painter.rect_filled(
                Rect::from_min_size(
                    rect.left_top(),
                    vec2(component::SIDEBAR_ITEM_ACTIVE_BAR, rect.height()),
                ),
                radius::all(radius::NONE),
                theme.state.selected_bar,
            );
        }

        // 18px icon slot, then the label at `sidebar.item-gap` (10px).
        let icon_rect = Rect::from_center_size(
            rect.left_center()
                + vec2(
                    component::SIDEBAR_ITEM_PADDING_X + component::SIDEBAR_ICON_SIZE / 2.0,
                    0.0,
                ),
            vec2(component::SIDEBAR_ICON_SIZE, component::SIDEBAR_ICON_SIZE),
        );
        if let Some(g) = place.glyph(theme) {
            tokens::icon_glyph(painter, icon_rect, g, icon_color);
        } else {
            tokens::icon_placeholder(painter, icon_rect, icon_color);
        }

        painter.text(
            pos2(
                icon_rect.right() + component::SIDEBAR_ITEM_GAP,
                rect.center().y,
            ),
            Align2::LEFT_CENTER,
            place.label,
            tokens::font(ty::UI, theme),
            text_color,
        );
        response.clicked()
    }

    /// §4.3 Breadcrumb for the current path.
    fn breadcrumb(&mut self, ui: &mut Ui) {
        let theme = self.theme;
        let segments = breadcrumb_segments(&self.dir);
        let mut collapsed: Vec<PathBuf> = Vec::new();
        egui::Panel::top("breadcrumb")
            .exact_size(component::BREADCRUMB_HEIGHT)
            .frame(
                egui::Frame::new()
                    .fill(theme.surfaces.panel)
                    .inner_margin(margin(space::S0))
                    // §4.3 `breadcrumb.bottom-border`. Sticky chrome is a layout
                    // sibling with a border, never an overlay, which is what
                    // makes "focus not obscured" structural rather than a
                    // scroll offset (§2.12, §6.6).
                    .stroke(Stroke::new(border::HAIRLINE, theme.borders.subtle)),
            )
            .show(ui, |ui| {
                // §4.3: overflow collapses from the left and the last two
                // segments always stay visible, because the user's actual
                // question is always "where am I" and "what's in here".
                let (visible, first) = trailing_segments(&segments, ui.available_width());
                // The band's own vertical centre, read **once**.
                //
                // This is the whole fix for the clipped separator. The previous
                // version positioned the overflow button and the `caret-right`
                // separators at `ui.cursor().max.y`, which in a
                // `horizontal_centered` layout is the *bottom of the band*, not
                // the middle of the text: egui stretches the first item's frame
                // to the full available height (`Layout::next_frame_ignore_wrap`
                // maximises the cross axis when `vertical_align == Center`), so
                // the cursor's `max.y` is the panel's lower edge from the very
                // first segment onward. A 20px-tall hit rect centred there hangs
                // half outside the 28px band, and the last four rows of every
                // separator are clipped by `breadcrumb.bottom-border` — which is
                // what made `/ home achraf` read as cut off. The segments were
                // never clipped; the separators were.
                let center_y = ui.max_rect().center().y;
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = component::BREADCRUMB_SEGMENT_GAP;
                    let last = visible.len().saturating_sub(1);
                    if first > 0 {
                        // `breadcrumb.overflow` — a leading `dots-three` button
                        // standing in for the collapsed ancestors.
                        //
                        // §4.3: it "opens a menu of collapsed ancestors", and it
                        // now does. It was painted and dead for three phases,
                        // which is §7.14's hidden affordance exactly: a control
                        // that looks like a control and does nothing.
                        let c = component::icon_at(
                            theme.icon.chrome,
                            component::BREADCRUMB_SEPARATOR_ALPHA,
                        );
                        let (r, response) = ui.allocate_exact_size(
                            vec2(
                                component::BREADCRUMB_OVERFLOW_SIZE,
                                component::BREADCRUMB_OVERFLOW_SIZE,
                            ),
                            Sense::click(),
                        );
                        let r = Rect::from_center_size(pos2(r.center().x, center_y), r.size());
                        if response.hovered() {
                            ui.painter().rect_filled(
                                r,
                                radius::all(component::MENU_ITEM_RADIUS),
                                theme.state.hover,
                            );
                        }
                        tokens::icon_glyph(ui.painter(), r, icons::DOTS_THREE_HORIZONTAL, c);
                        // `CloseOnClick` — the default for a menu, and the
                        // right one: picking an ancestor must dismiss the menu,
                        // or the user has to click away from a menu that is now
                        // pointing at the directory they are already in.
                        egui::Popup::menu(&response)
                            .align(egui::RectAlign::BOTTOM_START)
                            .gap(1.0)
                            .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
                            .show(|ui| {
                                for seg in segments.iter().take(first) {
                                    if widgets::menu_item(ui, &theme, &seg.label, "", false, false)
                                        .clicked()
                                    {
                                        collapsed.push(seg.path.clone());
                                    }
                                }
                            });
                        // The tooltip is what makes the button's purpose
                        // discoverable without clicking it, which is §4.4's rule
                        // for icon-only controls applied to the breadcrumb.
                        response.on_hover_text(format!(
                            "{} hidden ancestor{}",
                            first,
                            if first == 1 { "" } else { "s" }
                        ));
                    }
                    for (i, seg) in visible.iter().enumerate() {
                        if i > 0 {
                            // `breadcrumb.separator-clickable` — a 16 x 20 hit
                            // area, which is a *hit area* and so is allocated
                            // rather than painted at an arbitrary offset. Its
                            // 12px `caret-right` sits inside it, centred.
                            let c = component::icon_at(
                                theme.icon.chrome,
                                component::BREADCRUMB_SEPARATOR_ALPHA,
                            );
                            let (hit, hit_response) = ui.allocate_exact_size(
                                vec2(
                                    component::BREADCRUMB_SEPARATOR_HIT_W,
                                    component::BREADCRUMB_SEPARATOR_HIT_H,
                                ),
                                Sense::click(),
                            );
                            tokens::icon_glyph(
                                ui.painter(),
                                Rect::from_center_size(hit.center(), vec2(12.0, 12.0)),
                                icons::CARET_RIGHT,
                                c,
                            );
                            // Clicking a separator goes to the segment on its
                            // left: the directory the caret is pointing away
                            // from. `visible[i]` is the segment that follows.
                            if hit_response.clicked() {
                                if let Some(target) = visible.get(i - 1) {
                                    let path = target.path.clone();
                                    self.open_dir(path);
                                }
                            }
                        }
                        let is_current = i == last;
                        // The final segment is the current directory, rendered
                        // at 500 weight, and is **not** clickable — clicking it
                        // does nothing and it must not show a hover state.
                        let (color, token) = if is_current {
                            (
                                theme.text.primary,
                                component::BREADCRUMB_SEGMENT_TEXT_CURRENT,
                            )
                        } else {
                            (theme.text.secondary, component::BREADCRUMB_SEGMENT_TEXT)
                        };
                        let text = RichText::new(&seg.label)
                            .font(tokens::font(token, &theme))
                            .color(color);
                        if is_current {
                            ui.add(egui::Label::new(text));
                        } else {
                            let response = ui
                                .add(egui::Label::new(text).sense(Sense::click()))
                                .on_hover_text(format!("Go to {}", seg.path.display()));
                            if response.clicked() {
                                let path = seg.path.clone();
                                self.open_dir(path);
                            }
                        }
                    }
                });
            });
        if let Some(path) = collapsed.into_iter().next() {
            self.open_dir(path);
        }
    }

    /// §4.2 `row.col-header`: clickable, with a sort glyph on the active column.
    ///
    /// `list_width` is the panel's own width, threaded in by
    /// [`KestrelApp::panels`] so the header and the rows resolve their columns
    /// from the same number.
    fn column_headers(&mut self, ui: &mut Ui, list_width: f32) {
        let theme = self.theme;
        let set = columns::columns_for(list_width, self.show_kind_column);
        let height = component::ROW_COL_HEADER_HEIGHT;
        let (rect, _) = ui.allocate_exact_size(vec2(list_width, height), Sense::hover());
        // A header is a layout sibling with a 1px border, never an overlay
        // (§2.12): a sticky header that floats over the list would be exactly
        // the thing that hides a focused row at the top of the viewport.
        ui.painter()
            .rect_filled(rect, radius::all(radius::NONE), theme.surfaces.panel);
        widgets::rule(
            ui.painter(),
            rect.left(),
            rect.right(),
            rect.bottom() - 0.5,
            component::hairline(&theme),
        );

        // Lay the header cells out with the *same* geometry as the rows, so a
        // column's label sits above its values rather than near them.
        let layout = ColumnLayout::resolve(rect, set);

        let mut pending: Option<SortKey> = None;
        for (key, cell, label) in [
            (SortKey::Name, Some(layout.name), "Name"),
            (SortKey::Size, layout.size, "Size"),
            (SortKey::Kind, layout.kind, "Kind"),
            (SortKey::Modified, layout.modified, "Modified"),
        ] {
            let Some(cell) = cell else { continue };
            let active = self.sort_column == key;
            let hit = ui.interact(cell, ui.id().with(("col", label)), Sense::click());
            if hit.clicked() {
                pending = Some(key);
            }
            if hit.hovered() {
                ui.painter()
                    .rect_filled(cell, radius::all(radius::XS), theme.state.hover);
            }

            // `row.col-header` — `type.label` (11/600/uppercase/tracked),
            // `text.tertiary`. Right-aligned for the two machine columns, which
            // is what makes their values line up under their labels.
            let align_right = matches!(key, SortKey::Size | SortKey::Modified);
            let font = tokens::font(ty::LABEL, &theme);
            let label_w = widgets::text_width(ui, label, font.clone());
            // `row.padding-x`, on both edges — a right-aligned machine column
            // used to be inset 4px, which is half the token and put the timestamp
            // 4px from the preview pane's divider.
            let label_left = if align_right {
                cell.right() - component::ROW_PADDING_X - label_w
            } else {
                cell.left() + component::ROW_PADDING_X
            };
            // The alignment is already baked into `label_left`, so both
            // columns draw left-to-right from a pre-computed x. A right-aligned
            // column is a *label* positioned at the right edge, not a mirrored
            // draw call: the sort glyph is then placed off that x rather than
            // being swapped to the other end of the cell.
            ui.painter().text(
                egui::pos2(label_left, cell.center().y),
                Align2::LEFT_CENTER,
                label,
                font,
                if active {
                    theme.text.primary
                } else {
                    theme.text.tertiary
                },
            );

            // `row.col-header-sortable` — a 12px sort glyph, `icon.chrome`:
            // caret-up / caret-down / caret-up-down.
            //
            // It is placed immediately **beside the label**, not at the cell's
            // edge. The name column is 690px wide, so an edge-anchored glyph put
            // the sort indicator 600px from the word "Name" and made the whole
            // header look broken; the Size and Kind glyphs landed 150px from
            // their own labels, straddling the previous column. Measuring the
            // label and hanging the glyph off it is what "trailing" has to mean
            // once a column is allowed to be flexible.
            let glyph = if !active {
                icons::CARET_UP_DOWN
            } else if self.sort.ascending {
                icons::CARET_UP
            } else {
                icons::CARET_DOWN
            };
            let gsize = 12.0_f32;
            let gx = if align_right {
                label_left - component::ROW_PADDING_X - gsize / 2.0
            } else {
                label_left + label_w + component::ROW_PADDING_X + gsize / 2.0
            };
            let grect = Rect::from_center_size(egui::pos2(gx, cell.center().y), vec2(gsize, gsize));
            let gc = component::icon_at(
                if active {
                    theme.icon.chrome_active
                } else {
                    theme.icon.chrome
                },
                if active { 1.0 } else { 0.5 },
            );
            tokens::icon_glyph(ui.painter(), grect, glyph, gc);
        }

        if let Some(key) = pending {
            self.sort_by_column(key);
        }
    }

    /// §4.5 Status bar.
    fn status_bar(&mut self, ui: &mut Ui) {
        let theme = self.theme;
        let busy = self.scan.is_some();
        let items = self.visible_count();
        let selected = self.selection.len();
        let errors = self.errors.len();
        let frame = format!("{:.1} ms", self.frame_ms);
        let phrase = self.clipboard.status_phrase();
        let running = self.job_progress.as_ref().map(JobProgress::summary);

        egui::Panel::bottom("statusbar")
            .exact_size(component::STATUSBAR_HEIGHT)
            .frame(
                egui::Frame::new()
                    .fill(theme.surfaces.panel)
                    .inner_margin(margin(space::S0))
                    // `statusbar.top-border` — 1px `border.subtle`. Again a
                    // layout sibling, so it can never obscure a focused row.
                    .stroke(Stroke::new(border::HAIRLINE, theme.borders.subtle)),
            )
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = component::STATUSBAR_SECTION_GAP;
                // A plain full-width `horizontal`, not `horizontal_centered`: a
                // centered Ui shrinks to its content, so §4.5's trailing
                // free-space meter would end up pinned near the middle of the bar
                // instead of at its right edge.
                ui.horizontal(|ui| {
                    ui.add_space(component::STATUSBAR_PADDING_X);

                    // §4.5 section order: selection count (or item count) ·
                    // errors · busy · path · spacer · free space.
                    if selected > 0 {
                        widgets::status_selection_count(
                            ui,
                            &theme,
                            &format::plural(selected, "item", "items"),
                        );
                        // A range, when the selection is a contiguous one. The
                        // anchor is what makes it knowable: the focus alone
                        // cannot say "rows 10 to 14", which is the number a user
                        // checking a multi-select actually wants (§7.16).
                        if let Some(anchor) = self.selection.anchor() {
                            if let Some(focus) = self.selection.focus() {
                                if focus != anchor {
                                    let (lo, hi) = if anchor < focus {
                                        (anchor, focus)
                                    } else {
                                        (focus, anchor)
                                    };
                                    widgets::status_label(
                                        ui,
                                        &theme,
                                        &format!("rows {}–{}", lo + 1, hi + 1),
                                    );
                                }
                            }
                        }
                    } else {
                        widgets::status_label(ui, &theme, &format::plural(items, "item", "items"));
                    }

                    if let Some(phrase) = phrase {
                        widgets::status_divider(ui, &theme);
                        widgets::status_label(ui, &theme, &phrase);
                    }

                    if errors > 0 {
                        widgets::status_divider(ui, &theme);
                        widgets::status_error(
                            ui,
                            &theme,
                            &format::plural(errors, "error", "errors"),
                        );
                    }

                    if self.pane_too_narrow {
                        widgets::status_divider(ui, &theme);
                        // Short, because the status bar at 500px is already
                        // carrying an item count, a path and a free-space
                        // meter. A sentence here is the one string that pushes
                        // something off the end.
                        widgets::status_label(ui, &theme, "pane hidden: window too narrow");
                    }

                    // §7.16: never a silent status change. The watcher's absence
                    // is a permanent change to how the list behaves, so it gets
                    // a permanent chip with the one action that fixes it — not a
                    // log line, and not a `log::warn!` the user never sees.
                    if self.watch_error.is_some() {
                        widgets::status_divider(ui, &theme);
                        if widgets::status_notice(
                            ui,
                            &theme,
                            "Not watching for changes",
                            "Refresh",
                            "The filesystem watcher could not be started, so this folder will not update on its own. Press F5, or click Refresh, to re-read it.",
                        ) {
                            self.refresh();
                        }
                    }

                    // §7.16: "A long operation always reports itself in the status
                    // bar with a spinner and a text label." A running file
                    // operation outranks both the item count and the clipboard
                    // phrase, because it is the only thing whose completion the
                    // user is waiting on.
                    if let Some(running) = running.as_deref() {
                        widgets::status_divider(ui, &theme);
                        widgets::status_busy(ui, &theme, "Working", running, self.motion);
                    } else if busy {
                        widgets::status_divider(ui, &theme);
                        widgets::status_busy(ui, &theme, "Reading", &frame, self.motion);
                    }

                    // A replaced job's ending (§7.16: a long operation reports
                    // itself, even when it is not the operation running now).
                    if let Some((note, _)) = self.stopped_note.as_ref() {
                        widgets::status_divider(ui, &theme);
                        widgets::status_label(ui, &theme, note);
                    }

                    // `statusbar.path` — `type.meta`, `text.secondary`, left,
                    // middle-truncate, flex.
                    //
                    // "Flex" is the part that was missing, and it is the whole
                    // reason the trailing free-space block ends up painted on
                    // top of it: the path reported its full 64-column width, the
                    // cursor advanced past the middle of the bar, and the
                    // `right_to_left` section then drew from there. The fix is
                    // to spend only what is left after the trailing section has
                    // had its width, which means measuring it *before* the path
                    // rather than after.
                    let trailing = trailing_status_width(ui, &theme, self.space.get());
                    widgets::status_value(
                        ui,
                        &theme,
                        &truncate_path(ui, &self.dir.to_string_lossy(), trailing),
                    );

                    // `statusbar.selection-count-idle` when nothing is selected
                    // is handled by the count branch above.
                    if self.motion.is_reduced() {
                        widgets::status_divider(ui, &theme);
                        widgets::status_label(ui, &theme, "reduced motion");
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add_space(component::STATUSBAR_PADDING_X);
                        // The real `statvfs` reading, read on a worker. Before
                        // the first answer lands the meter is absent rather than
                        // showing a fabricated 42%.
                        if let Some(space) = self.space.get() {
                            widgets::free_space_meter(ui, &theme, space.fraction());
                            ui.label(
                                RichText::new(format!("{} free", space.format_free()))
                                    .font(tokens::font(component::STATUSBAR_LABEL, &theme))
                                    .color(theme.text.tertiary),
                            );
                        }
                    });
                });
            });
    }

    /// §4.2 File list. **Virtualized**: only the visible row range is built.
    ///
    /// `list_width` is the panel's own width, threaded in by
    /// [`KestrelApp::panels`]. It is used for the column set *and* for every row
    /// rect, and it is deliberately **not** re-read from the `Ui` inside the
    /// `ScrollArea` closure: see the note in `panels` for why that read is a
    /// trap.
    fn file_list(&mut self, ui: &mut Ui, list_width: f32) {
        let theme = self.theme;
        // The list's own rectangle, captured before anything is allocated into
        // it: the cross-fade has to cover the list and *only* the list, because
        // the breadcrumb and the column headers are chrome, and §2.12 makes
        // chrome a layout sibling that never moves.
        let list_rect = ui.max_rect();
        let visible = self.visible_rows();
        let total = visible.len();
        if total == 0 {
            self.empty_list(ui, &theme);
            self.paint_nav_fade(ui, &theme, list_rect);
            return;
        }

        let set = columns::columns_for(list_width, self.show_kind_column);
        // `ScrollArea::show_rows` builds only `range` of `total_rows` widgets, so
        // a 50,000-item directory costs what a 50-item one does. Mapping the
        // whole `Vec` into a widget per row would build 50,000 closures per frame
        // and drop frames doing it.
        // The offset is applied from `self.scroll_rows` and written back, so
        // `Home`/`End`/scroll-to-focused can move the list without a `Ui` in
        // the keyboard handler. `ScrollAreaOutput` reports the offset the user
        // produced by scrolling; the two are reconciled by taking whichever is
        // further along, which is the only rule that does not fight the user
        // mid-gesture.
        let out = ScrollArea::vertical()
            .id_salt("file-list")
            .auto_shrink([false, false])
            .vertical_scroll_offset(self.scroll_rows as f32 * component::ROW_HEIGHT)
            .show_rows(ui, component::ROW_HEIGHT, total, |ui, range| {
                // egui adds `item_spacing.y` to the row height, so the list band
                // (the 2px half-step) must be zeroed here or rows come out at
                // 28px instead of the specified 26px.
                ui.spacing_mut().item_spacing.y = 0.0;
                let width = list_width;
                for at in range {
                    let Some(&index) = visible.get(at) else {
                        continue;
                    };
                    let Some(row) = self.rows.get(index) else {
                        continue;
                    };
                    let entry = &row.entry;
                    let (rect, response) =
                        ui.allocate_exact_size(vec2(width, component::ROW_HEIGHT), Sense::click());
                    let state = self.row_state(at, entry, response.hovered());
                    let is_open = self.expanded.contains(&entry.path);
                    // §4.11 `F2`: the field replaces the name in the row, so the
                    // row keeps its selection bar, focus ring and icon while the
                    // name becomes editable.
                    let renaming_here =
                        !self.creating && self.renaming.as_ref().is_some_and(|r| r.row == at);
                    // The row is painted from `&self`; the field is drawn from
                    // `&mut self`. So the row's borrow has to end first, which is
                    // why the paint call is separated from the field call rather
                    // than the field being drawn inside `paint_row`.
                    self.paint_row(ui, &theme, rect, row, at, state, set, is_open);
                    if renaming_here {
                        self.rename_field(ui, &theme, rect, set);
                    }
                    let _ = index;

                    if response.clicked() {
                        let m = ui.input(|i| i.modifiers);
                        if m.shift {
                            self.selection.extend_to(at);
                        } else if m.ctrl {
                            self.selection.toggle(at);
                        } else {
                            self.selection.click(at);
                        }
                    }
                }
            });

        // Record how many rows fit, for the keyboard's scroll-to-focused.
        self.viewport_rows = (ui.available_height() / component::ROW_HEIGHT)
            .floor()
            .max(1.0) as usize;
        // Reconcile the keyboard's intent with the user's own scrolling.
        let from_pixels = (out.state.offset.y / component::ROW_HEIGHT)
            .round()
            .max(0.0);
        self.scroll_rows = self
            .scroll_rows
            .max(from_pixels as usize)
            .min(total.saturating_sub(1));

        self.paint_nav_fade(ui, &theme, list_rect);
    }

    /// §2.11's "directory content cross-fade on navigation".
    ///
    /// # Opacity only, and nothing else
    ///
    /// §2.11 rule 2 forbids animating layout, and §7.13 names this exact
    /// hazard: "a file manager that animates row height while you scroll is a
    /// file manager that drops frames on a weak machine". So there is no row
    /// tween, no height change, no column reflow — one `rect_filled` of the
    /// list's own background over the list's own rectangle, whose alpha goes
    /// 1 -> 0. Everything behind it keeps its final geometry from the first
    /// frame, and only its opacity changes.
    ///
    /// # Why a fade *in* and not a cross-fade
    ///
    /// A true cross-fade needs both directories' rows on screen at once, which
    /// means holding the old listing alive for 90ms and laying out two 50,000-row
    /// virtual lists. The cost is not the paint, it is the memory and the second
    /// layout pass, and it buys a difference no one can see at 90ms. What the eye
    /// actually reads is the *arrival*: content that is there one frame and settled
    /// a tenth of a second later reads as having travelled. So the new listing is
    /// fully laid out from the first frame and simply comes up out of the list's
    /// background colour.
    ///
    /// # Reduced motion
    ///
    /// §2.11 rule 5: every duration becomes 0 and the final state renders
    /// immediately. [`KestrelApp::motion`] is already resolved against the
    /// preference, so the check here is a branch and not a second signal.
    fn paint_nav_fade(&mut self, ui: &Ui, theme: &Theme, list_rect: Rect) {
        let now = ui.input(|i| i.time);
        if self.nav_pending && self.scan.is_none() {
            // The listing has landed. Start the clock now, not at the keystroke.
            self.nav_pending = false;
            if !self.motion.is_reduced() {
                self.nav_fade = Some(now);
            }
        }
        let Some(at) = self.nav_fade else {
            return;
        };
        let t = ((now - at) / NAV_FADE.as_secs_f64()).clamp(0.0, 1.0);
        if t >= 1.0 {
            self.nav_fade = None;
            return;
        }
        // §2.11 rule 3 is a ceiling for anything the *pointer* touches, and
        // rule 4 bans a shimmer, so this is a plain linear ramp with no easing
        // curve: at 90ms an ease-in-out spends more than half the duration
        // apparently doing nothing.
        let alpha = (1.0 - t) as f32;
        ui.painter().rect_filled(
            list_rect,
            radius::all(radius::NONE),
            with_alpha(theme.surfaces.list, alpha),
        );
    }

    /// A row's state, from §4.10's priority order.
    ///
    /// §4.10: `disabled` > `drop-target` > `pressed` > `selected` >
    /// `focus-visible` > `hover` > `default`, with the one exception that
    /// `selected + focus-visible` renders **both**.
    ///
    /// `hovered` is passed rather than read from a rect because the rect is only
    /// known after allocation, and the painter needs the state at paint time.
    ///
    /// The clipboard is resolved by *path*, not by row position: positions shift
    /// the moment a filter is typed, and a cut that follows a position is a cut
    /// that silently starts marking the wrong rows.
    fn row_state(&self, at: usize, entry: &FileEntry, hovered: bool) -> component::RowState {
        // A cut row is `cut`; a copied row is visually normal, which is the
        // whole difference between the two §4.2 states.
        let forced = self
            .clipboard
            .op_for(&entry.path)
            .filter(|op| op.is_cut())
            .map(|_| component::RowState::Cut);
        component::RowState::resolve(
            self.selection.contains(at),
            self.selection.focus() == Some(at) && self.selection.focus_is_selected(),
            hovered,
            forced,
        )
    }

    /// Paints one row, straight from §4.2's state matrix.
    #[allow(clippy::too_many_arguments)]
    fn paint_row(
        &self,
        ui: &Ui,
        theme: &Theme,
        rect: Rect,
        row: &Row,
        at: usize,
        state: component::RowState,
        set: ColumnSet,
        is_open: bool,
    ) {
        let painter = ui.painter();
        let entry = row.entry();
        let depth = row.depth;
        let classified = Classified::of(entry);

        // Background: flat, 3px radius, no shadow, no gradient (§4.2 rule 5).
        painter.rect_filled(
            rect,
            component::row_corner_radius(),
            state.background(theme),
        );

        // Selection's second channel: the 2px left bar (§6.6 "never colour
        // alone").
        if let Some(bar) = state.selected_bar(theme) {
            let inset = component::ROW_SELECTED_BAR_INSET;
            // 1px radius = half the bar width: the smallest rounding that keeps
            // the bar from poking out of the row's 3px corner radius. The spec
            // gives the bar no radius of its own, so this is the one place a
            // radius is chosen rather than transcribed.
            painter.rect_filled(
                Rect::from_min_max(
                    pos2(rect.left(), rect.top() + inset),
                    pos2(
                        rect.left() + component::ROW_SELECTED_BAR_WIDTH,
                        rect.bottom() - inset,
                    ),
                ),
                radius::all(1.0),
                bar,
            );
        }

        // The focus ring is a 2px **inset** stroke, drawn on top of the tint
        // when the row is both selected and focused — the combination §4.2 calls
        // "the single most important state in the whole app".
        if let Some(ring) = state.focus_ring(theme, window_focused(ui)) {
            widgets::rect_stroke(painter, rect, component::row_corner_radius(), ring);
        }

        // `row.divider` — 1px `border.subtle`, inset 8px left and right.
        widgets::rule(
            painter,
            rect.left() + component::ROW_DIVIDER_INSET_X,
            rect.right() - component::ROW_DIVIDER_INSET_X,
            rect.bottom() - 0.5,
            component::hairline(theme),
        );

        // Hidden-file marker: a 4px dot in the 3px leading gutter (§4.2
        // `row.col-hidden`). This is the hidden state's *second channel*, and it
        // is why a hidden file needs no icon change (§5.2).
        if entry.hidden {
            painter.circle_filled(
                pos2(rect.left() + metric::GUTTER_MARKER, rect.center().y),
                component::ROW_HIDDEN_DOT / 2.0,
                component::hidden_dot_color(theme),
            );
        }

        // Column geometry from the shared layout, so a header sits above its
        // values by construction rather than by coincidence.
        let mut layout = ColumnLayout::resolve(rect, set);
        // `row.indent-step` — 14px per depth level, now for real.
        let mut shift = (depth as f32) * component::ROW_INDENT_STEP;
        if self.view == ViewMode::Tree {
            // Every tree row reserves the disclosure column, not just the ones
            // that draw a caret. Without the shared reservation the caret was
            // painted straight on top of the folder icon — both occupied
            // `gutter..gutter+16` — so a collapsed folder rendered as an
            // unreadable smudge. Reserving it for *all* rows is also what makes
            // depth 1 line up under depth 0 instead of half a step out.
            shift += component::TREE_DISCLOSURE + space::HALF;
        }
        if shift > 0.0 {
            layout.name = layout.name.translate(vec2(shift, 0.0));
            layout.icon = layout.icon.translate(vec2(shift, 0.0));
        }
        // A disclosure caret in the reserved column, for expandable rows only.
        // §5.3's `caret-right` / `caret-down`: without it a child row is just
        // indented text with nothing saying where its parent is.
        if self.view == ViewMode::Tree && row.is_expandable() {
            let cr = Rect::from_center_size(
                pos2(
                    rect.left() + metric::GUTTER + component::TREE_DISCLOSURE / 2.0,
                    rect.center().y,
                ),
                vec2(component::TREE_DISCLOSURE, component::TREE_DISCLOSURE),
            );
            tokens::icon_glyph(
                painter,
                cr,
                if is_open {
                    icons::CARET_DOWN
                } else {
                    icons::CARET_RIGHT
                },
                component::icon_at(theme.icon.chrome, 0.7),
            );
        }

        // The real Phosphor glyph, in its §3.7 role colour.
        let icon_color = classified.icon_color(&theme.icon);
        let icon_color = match state {
            component::RowState::Disabled => component::icon_at(icon_color, 0.5),
            component::RowState::Cut => component::icon_at(icon_color, 0.7),
            _ => icon_color,
        };
        if let Some(g) = classified.glyph() {
            tokens::icon_glyph(painter, layout.icon, g, icon_color);
        } else {
            tokens::icon_placeholder(painter, layout.icon, icon_color);
        }

        let name_font = tokens::font(state.name_token(), theme);
        // `text.tertiary` on a hidden file's name (§4.2 `hidden file` row), and
        // `text.disabled` only in the `disabled` state — which is exactly the
        // §6.2 structural rule, since `RowState::Disabled` has no bar and so can
        // never also be a selected row.
        let name_color = match (entry.hidden, state.text_role()) {
            (_, component::DisabledText::Disabled) => theme.text.disabled,
            (true, component::DisabledText::Normal) => theme.text.tertiary,
            (false, component::DisabledText::Normal) => theme.text.primary,
        };
        // §2.8: all list text is single-line, vertically centred, never wrapped,
        // middle-truncated. The character budget is *measured*, not estimated —
        // see [`truncation_cols`].
        let name = format::middle_truncate(
            &entry.name,
            truncation_cols(painter, &entry.name, &name_font, layout.name_text_width()),
        );
        painter.text(
            pos2(layout.name_text_x, rect.center().y),
            Align2::LEFT_CENTER,
            &name,
            name_font,
            name_color,
        );

        // `row.col-modified` — `type.meta`, `text.tertiary`, right-aligned,
        // absolute UTC (§4.2).
        if let Some(cell) = layout.modified {
            let text = entry
                .modified
                .map_or_else(|| "-".to_string(), format::timestamp);
            painter.text(
                pos2(cell.right() - component::ROW_PADDING_X, rect.center().y),
                Align2::RIGHT_CENTER,
                &text,
                tokens::font(ty::META, theme),
                theme.text.tertiary,
            );
        }

        // `row.col-kind` — `type.caption`, `text.tertiary`, left. Only present
        // when the listing is genuinely mixed; see `columns::listing_is_mixed`.
        if let Some(cell) = layout.kind {
            painter.text(
                pos2(cell.left() + component::ROW_PADDING_X, rect.center().y),
                Align2::LEFT_CENTER,
                classified.category.label(),
                tokens::font(ty::CAPTION, theme),
                theme.text.tertiary,
            );
        }

        // `row.col-size` — `type.meta`, `text.tertiary`, right-aligned, tabular.
        //
        // The engine's documented caveat, and §4.2's: `FileEntry::size` for a
        // directory is that directory's own `lstat` length (typically 4096), NOT
        // a recursive total. Printing `4.0 KB` there is a lie, so directories
        // render an em dash. `None` means "unknown/unreadable", which the engine
        // documents as `-` rather than `0`.
        if let Some(cell) = layout.size {
            let text = match (entry.kind, entry.size) {
                (EntryKind::Directory, _) => format::NOT_APPLICABLE.to_string(),
                (_, Some(n)) => format::bytes(n),
                (_, None) => "-".to_string(),
            };
            painter.text(
                pos2(cell.right() - component::ROW_PADDING_X, rect.center().y),
                Align2::RIGHT_CENTER,
                &text,
                tokens::font(ty::META, theme),
                theme.text.tertiary,
            );
        }

        // §4.2 rule 3: the focused row must be fully visible. A tree row that is
        // focused draws a 1px marker in the margin so the eye can find it in a
        // deep indentation, which is the tree-mode equivalent of the ring.
        if self.selection.focus() == Some(at) && depth > 0 {
            painter.circle_filled(
                pos2(rect.left() + space::S1, rect.center().y),
                component::ROW_SELECTED_BAR_WIDTH,
                theme.state.selected_bar,
            );
        }
    }

    /// The inline rename field, drawn over the row's name.
    ///
    /// §4.7/§4.8: the field takes the row's name column, the same `surface.input`
    /// and `border.accent` a search field would, and the error appears **under**
    /// it rather than in a dialog — a rename mistake is a one-word fix, and a
    /// modal for a one-word fix is a modal in the way.
    fn rename_field(&mut self, ui: &mut Ui, theme: &Theme, rect: Rect, set: ColumnSet) {
        let Some(inline) = self.renaming.as_mut() else {
            return;
        };
        let layout = ColumnLayout::resolve(rect, set);
        let field = Rect::from_min_max(
            egui::pos2(layout.name_text_x, rect.center().y - 9.0),
            egui::pos2(layout.name.right() - space::S1, rect.center().y + 9.0),
        );
        let painter = ui.painter();
        let error = inline.error();
        painter.rect_filled(field, radius::all(radius::SM), theme.surfaces.input);
        // A field with a problem in it is outlined in danger, not accent: the
        // accent would say "this is fine, you are typing".
        let border_color = if error.is_some() {
            theme.borders.danger
        } else {
            theme.borders.accent
        };
        crate::widgets::rect_stroke(
            painter,
            field,
            radius::all(radius::SM),
            Stroke::new(border::HAIRLINE, border_color),
        );

        let id = ui.id().with("rename-field");
        let text = format::middle_truncate(&inline.draft, 64);
        let out = ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(field)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                egui::TextEdit::singleline(&mut inline.draft)
                    .id(id)
                    .desired_width(field.width() - space::S1)
                    .frame(egui::Frame::NONE)
                    .text_color(theme.text.primary)
                    .hint_text(egui::RichText::new("name"))
                    .show(ui)
            },
        );
        let _ = text;
        // The field must keep focus, or typing the first character drops it and
        // the rename silently truncates to one letter.
        if self.renaming.as_ref().is_some_and(|r| !r.focused) && !out.response.has_focus() {
            if let Some(r) = self.renaming.as_mut() {
                r.focused = true;
            }
            out.response.request_focus();
        }
    }

    /// §4.2 empty state: a 48px glyph at 40%, the folder name, one sentence.
    ///
    /// "The empty state is a real design moment, not an afterthought… No
    /// illustration, no illustration-adjacent illustration."
    /// §4.2's empty state, and every other way a listing can have nothing to
    /// show. See [`crate::states`] for why the decision is a pure function and
    /// what each state says.
    fn empty_list(&mut self, ui: &mut Ui, theme: &Theme) {
        let dir_name = self.dir.file_name().map_or_else(
            || self.dir.to_string_lossy().into_owned(),
            |n| n.to_string_lossy().into_owned(),
        );
        let state = states::State::for_listing(
            &self.dir,
            self.rows.len(),
            &self.filter,
            self.scan.is_some(),
            &self.errors,
        );
        let motion = self.motion;
        let pressed = states::draw(ui, theme, &state, &dir_name, motion);
        if let Some(action) = pressed {
            match action {
                states::Action::NewFolder => self.new_folder(),
                states::Action::ClearFilter => self.filter.clear(),
                states::Action::Retry => self.refresh(),
                states::Action::GoUp => self.go_up(),
            }
        }
    }
}

// ----------------------------------------------------------------------------
// Helpers
// ----------------------------------------------------------------------------

/// Scales `natural` to fit inside `max` while keeping its aspect ratio.
///
/// Never upscales: a 16x16 icon blown up to 200px is a blurry square, and a
/// preview that lies about an image's sharpness is worse than a small honest one.
#[must_use]
fn fit(natural: egui::Vec2, max: egui::Vec2) -> egui::Vec2 {
    if natural.x <= 0.0 || natural.y <= 0.0 {
        return egui::vec2(0.0, 0.0);
    }
    let scale = (max.x / natural.x).min(max.y / natural.y).min(1.0);
    egui::vec2(natural.x * scale, natural.y * scale)
}

/// Whether the OS window has keyboard focus.
///
/// Drives `focus.ring` vs `focus.ring-inactive` (§4.2 `window inactive +
/// focused`, §6.4). `InputState::focused` is documented as "the native window
/// has the keyboard focus (i.e. is receiving key presses). False when the user
/// alt-tab away" — which is exactly this question, and is *not* the same as
/// whether an egui widget holds focus.
fn window_focused(ui: &Ui) -> bool {
    ui.input(|i| i.focused)
}

/// One breadcrumb segment: a label plus the path it navigates to.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Segment {
    /// The name shown.
    label: String,
    /// The absolute path this segment navigates to.
    path: PathBuf,
}

/// Splits `path` into breadcrumb segments, root first.
#[must_use]
fn breadcrumb_segments(path: &Path) -> Vec<Segment> {
    // "/" has no components, so the root is seeded explicitly.
    let mut out = vec![Segment {
        label: "/".to_string(),
        path: PathBuf::from("/"),
    }];
    for part in path.components() {
        // The root component of an absolute path is `RootDir`, already seeded.
        let Some(name) = part.as_os_str().to_str() else {
            // A non-UTF-8 component cannot be rendered losslessly. The engine
            // lossily converts names for exactly this reason; do the same and
            // keep walking rather than truncating the breadcrumb.
            let lossy = part.as_os_str().to_string_lossy().into_owned();
            let parent = out
                .last()
                .map_or_else(|| PathBuf::from("/"), |s| s.path.clone());
            out.push(Segment {
                label: lossy,
                path: parent.join(part.as_os_str()),
            });
            continue;
        };
        if name == "/" {
            continue;
        }
        let parent = out
            .last()
            .map_or_else(|| PathBuf::from("/"), |s| s.path.clone());
        out.push(Segment {
            label: name.to_string(),
            path: parent.join(name),
        });
    }
    out
}

/// How many trailing segments fit, and where they start.
///
/// Returns `(segments, first_visible_index)`. §4.3: the last two segments
/// always stay visible, so the answer is at least `BREADCRUMB_MIN_VISIBLE`
/// whenever there are that many at all.
#[must_use]
fn trailing_segments(segments: &[Segment], available: f32) -> (Vec<&Segment>, usize) {
    // A fixed 7.2px per character at `type.ui`'s 13px in Plex Sans. Deliberately
    // an estimate rather than a `Context` measurement: this is a pure function
    // the tests call without a `Ui`, and the consequence of being 10% out is that
    // one extra or one fewer ancestor is collapsed — not a wrong path.
    let width_of = |label: &str| label.chars().count() as f32 * 7.2;

    let min_keep = component::BREADCRUMB_MIN_VISIBLE.min(segments.len());
    let mut first = segments.len().saturating_sub(min_keep);
    let mut used: f32 = segments[first..]
        .iter()
        .map(|s| width_of(&s.label) + component::BREADCRUMB_SEGMENT_GAP)
        .sum();
    while first > 0 && used + width_of(&segments[first - 1].label) <= available {
        first -= 1;
        used += width_of(&segments[first].label);
    }
    (segments[first..].iter().collect(), first)
}

/// What the preview pane does with the room the window has left for it.
///
/// Three answers, and the width is the only thing that differs between two of
/// them. A pane that is a strip and a pane that is full draw different things,
/// but they draw them from this one decision, made before the panel is built —
/// see [`preview_plan`] and [`KestrelApp::preview_panel`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PanePlan {
    /// The window cannot hold the pane at its floor. It is not shown at all,
    /// and the status bar says so.
    Hidden,
    /// Nothing is focused, so there is nothing to describe: a narrow rail
    /// carrying the empty state's icon.
    ///
    /// **Not blank** (§4.2). The icon is the whole of the state, and it is
    /// centred, so the rail reads as "there is a pane here and it is empty"
    /// rather than as a border with something clipped inside it.
    Strip { width: f32 },
    /// A row is focused: the pane is wide enough to hold a filename, a meta
    /// line and a preview.
    Full { width: f32 },
}

/// The preview pane's plan for one frame, as a pure function of two numbers and
/// a predicate.
///
/// # Why a function and not a field
///
/// Because a function has no state, it cannot interpolate. The pane's width
/// changes on the same frame the selection does and for no other reason, and
/// §2.11 rule 1 is the app's most important motion decision — *"Selection is
/// instant (0 ms). A fading selection reads as lag in a file list."* An
/// animated width would be that lag arriving by a side door: the selection ring
/// would snap while the pane crawled, and the eye would read the crawl as the
/// file list being slow.
///
/// This is also why the obvious egui API is banned here. `Panel::show_collapsible`
/// and `show_switched` animate over `Style::animation_time` (200ms, never
/// overridden in this codebase) by translating the panel toward its fixed edge
/// and letting the allocation follow — §2.11 rule 2 bans animating `width` and
/// `left`, and §7.13 ("Layout-animating properties") says the same thing again as
/// an anti-goal. Worse, at `how_expanded == 0.0` `show_collapsible` returns
/// `None` and draws **no panel at all**, which is precisely the blank rectangle
/// §4.2's "Never blank" exists to prevent. The strip is a width this function
/// returns and `Panel::exact_size` applies: recomputed every frame from scalars,
/// nothing to interpolate, snapping with the selection.
///
/// # The three questions, in this order
///
/// 1. Can the pane be *shrunk* to what is left after reserving the list's
///    minimum? If not it does not fit at all — a setting that says "show the
///    preview" is overridden by a window that cannot hold one, and the status
///    bar says so, because a pane that silently stops appearing is the same
///    hidden-affordance bug as a button that silently stops working.
/// 2. Is there a row to describe? If not, the strip.
/// 3. Otherwise the configured width, shrunk to fit if it must be.
#[must_use]
pub fn preview_plan(room: f32, preview_width: f32, has_target: bool) -> PanePlan {
    if room < dialog::preview_metrics::MIN_WIDTH {
        return PanePlan::Hidden;
    }
    if has_target {
        // Shrink-then-hide: the list's minimum is reserved above, so `room` is
        // already what the pane may have at most.
        //
        // The `clamp` is the tail of the panel's old `min_size`/`max_size`
        // range, applied here because `exact_size` replaced that range with a
        // single point. `settings::normalize` already clamps the stored value,
        // so today the clamp is a no-op — but the plan is the total function
        // that decides the pane's width, and a plan that trusted its caller to
        // pre-validate the argument is not one.
        PanePlan::Full {
            width: preview_width
                .clamp(
                    dialog::preview_metrics::MIN_WIDTH,
                    dialog::preview_metrics::MAX_WIDTH,
                )
                .min(room),
        }
    } else {
        PanePlan::Strip {
            // The `.min(room)` is not ceremony. `MIN_WIDTH` is 180 and the strip
            // is 80, so today the strip always fits; if a future floor ever
            // dropped below the strip this is what stops the pane pushing the
            // list under `LIST_MIN_W`, which is the one invariant here.
            width: dialog::preview_metrics::STRIP_WIDTH.min(room),
        }
    }
}

/// The middle-truncation budget for `text` in `max_w` — how many characters of
/// it can be shown before the ellipsis.
///
/// # Measured, not estimated
///
/// §2.8 requires list text to be "middle-truncated with ellipsis at 60% width if
/// needed", which means the budget has to be *right*: too many and the text runs
/// into the next column, too few and a short name gets needlessly elided. A
/// constant like "6.5px per character at 13px" is only right for one face at one
/// size — the file list's names are Plex **Sans** at 13px, where it is roughly
/// correct, and the preview's code lines are Plex **Mono** at 12px, where the
/// advance is a fixed 7.2px. Using the sans average for the mono face
/// under-measures by about 11%, so a long code line was truncated to a
/// "fitting" budget and then overflowed the pane anyway.
///
/// So the width is measured with the font that will draw it, and the
/// per-character average is taken from *this* string rather than from a sample:
/// a name of narrow characters (`iii`) then keeps more of itself than one of wide
/// ones (`WWW`), which is what the eye expects of a truncation point.
///
/// One `layout_no_wrap` when the text fits, two when it does not. The galley
/// cache is keyed on the job, so the first is free after the first frame.
fn truncation_cols(painter: &egui::Painter, text: &str, font: &egui::FontId, max_w: f32) -> usize {
    let chars = text.chars().count();
    if chars == 0 {
        return 0;
    }
    let full = painter
        .layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::WHITE)
        .size()
        .x;
    if full <= max_w {
        return chars;
    }
    // `chars >= 1` here, so the division is safe; `floor`, because a budget that
    // rounds up is a budget that overflows.
    let per_char = full / chars as f32;
    let cols = (max_w / per_char).floor();
    // Four is the smallest budget `format::middle_truncate` can work with and
    // still keep something at each end.
    (cols as usize).clamp(4, chars)
}

/// The preview pane's `meta` line, degraded by **dropping fields**, never by
/// truncating one.
///
/// The line is `kind · modified · size` in the order it drops from, and all
/// three are machine values. A timestamp is a single unbreakable token, so a
/// `middle_truncate` that runs out of room produces
/// `Text · 2026-09-29 · .26-09-29 01:52` — a date that is *wrong* rather than
/// short, which is the one thing §2.8's machine-value rule exists to prevent. A
/// `Label` would instead wrap, which is worse: a wrapped timestamp reads as a
/// second line of the file listing.
///
/// So the line is assembled widest-first and the *least* useful field is dropped
/// until it fits. The size goes first, because a file list gives a size a column
/// of its own and a preview pane does not; the kind goes last, because it is the
/// one field the icon does not already say.
fn meta_line(painter: &egui::Painter, font: &egui::FontId, width: f32, parts: &[&str]) -> String {
    let fits = |s: &str| {
        painter
            .layout_no_wrap(s.to_owned(), font.clone(), egui::Color32::WHITE)
            .size()
            .x
            <= width
    };
    for count in (1..=parts.len()).rev() {
        let joined = parts[..count].join(" · ");
        if fits(&joined) {
            return joined;
        }
    }
    parts.first().copied().unwrap_or_default().to_string()
}

/// A 1-px all-round `egui::Margin` from a spacing token.
///
/// `egui::Margin` is `i8` in epaint 0.36, so a sub-pixel spacing token has to
/// round; the tokens used for margins are all whole pixels.
fn margin(px: f32) -> egui::Margin {
    egui::Margin::same(px.clamp(0.0, i8::MAX as f32).round() as i8)
}

/// The path an entry navigates to, if the scanner proved it is a directory.
///
/// **Zero I/O.** [`kestrel_fs::model::FileEntry::is_descendable`] is a field
/// read: the scanner resolved every symlink's target on its own thread and
/// recorded the answer. This is the call that used to `stat` in the frame loop.
#[must_use]
// `app.rs` inlines `is_descendable()` at its two call sites, so this wrapper
// survives only for the symlink tests below — which are the ones that pin the
// zero-I/O rule. Kept: those tests are the contract.
#[cfg_attr(not(test), allow(dead_code))]
fn descendable(entry: &FileEntry) -> Option<&Path> {
    entry.is_descendable().then_some(entry.path.as_path())
}

/// A row's indentation depth: how many components it sits below `root`.
///
/// Derived from the **path**, which is the only thing that knows the truth — the
/// scan is depth-first and the entry carries the full path, so a child of
/// `root/a` is unambiguously one level below `root` and a child of `root/a/b` is
/// two. A view index cannot supply this, which is why the earlier
/// `tree_depth(index)` stub could not be repaired by indexing.
///
/// An entry outside `root` — which should not happen, since the scanner walks
/// only its own root — is treated as top-level rather than panicking or wrapping
/// through a huge `usize`.
#[must_use]
fn depth_of(root: &Path, path: &Path) -> usize {
    match path.strip_prefix(root) {
        Ok(rel) => rel.components().count().saturating_sub(1),
        Err(_) => 0,
    }
}

// ----------------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------------
//
// Split into its own file because `app.rs` had grown past 4,500 lines and the
// test module was a fifth of it. `#[path]` rather than a new `mod` in
// `main.rs` because these are `app`'s own private fields: a sibling module can
// see them, a crate-root module cannot, and the tests are worth the awkwardness
// precisely because they can reach in.
#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
