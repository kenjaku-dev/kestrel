//! Kestrel — a lightweight file manager.
//!
//! Phase 0 (project scaffold) + Phase 1 (the design system), as one unit: the
//! result is a themed, running window rather than a grey placeholder.
//!
//! # Module map
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | [`tokens`] | the design system: primitive -> semantic -> component, plus the light/dark [`tokens::Theme`] |
//! | [`icons`] | §5.1 Phosphor glyph names, codepoints, and the icon families |
//! | [`filetype`] | §5 path -> file-type category -> icon slot + Phosphor glyph name |
//! | [`format`] | machine-value formatting (bytes, timestamps, plural counts) |
//! | [`places`] | §4.1 sidebar places, resolved from `$HOME` once at startup |
//! | [`widgets`] | the shared token-driven widgets (toolbar button, status-bar sections, swatch) |
//! | [`app`] | the shell: sidebar, breadcrumb, virtualized list, status bar, keyboard |
//! | [`gallery`] | `--gallery`: every §4 component at every state |
//! | [`shot`] | `--screenshot`: render offscreen to a PNG, for reviewing the theme |
//!
//! # Load-bearing rules
//!
//! * **The UI thread never blocks.** Every filesystem read goes through
//!   `kestrel-fs`'s background handles; see [`app`]'s module docs.
//! * **No `unwrap`/`expect` on anything that can fail at runtime**, in any
//!   non-test code in this crate.
//!
//! # CLI
//!
//! ```text
//! kestrel [--gallery] [--tree] [--light | --dark | --system] [--screenshot PATH]
//!         [--scene NAME] [--size WxH] [DIR]
//!
//!   --gallery   render the design-system gallery instead of the file manager
//!   --tree      start in the indented tree view (default: a flat list)
//!   --screenshot PATH
//!               render the UI, write a PNG, and exit. The capture comes from
//!               egui's own tessellation rather than a screen grab, so it works
//!               on a busy desktop and is the way the theme is reviewed.
//!   --scene NAME
//!               drive the app into a named state before capturing it. The
//!               dialogs and the populated preview pane are states, not screens,
//!               so this is the only way to see them; see [`app::Scene`].
//!   --size WxH  capture at an explicit size, e.g. 900x700. A layout that only
//!               works at the default width is a layout that does not work.
//!   --light     force the light theme (§3, light column)
//!   --dark      force the dark theme (§3, dark column) — the default fallback
//!   --system    follow the compositor (the default)
//!   DIR         directory to open (default: $HOME, or $KESTREL_HOME)
//!   -h, --help  this text
//! ```

#![warn(missing_docs)]
#![warn(clippy::all)]

mod app;
mod clipboard;
mod columns;
mod dialog;
mod disk;
mod filetype;
mod format;
mod gallery;
mod help;
mod history;
mod icons;
mod job;
mod motion;
#[cfg(test)]
mod packaging;
mod places;
mod preview;
mod rename;
mod selection;
mod settings;
mod shot;
mod states;
mod tokens;
mod toolbar;
mod widgets;

use std::error::Error;

use eframe::NativeOptions;
use tokens::ThemeMode;

/// The return type `AppCreator` expects: the app, or a boxed error.
///
/// This is *not* [`eframe::Result`], which is `Result<(), eframe::Error>` — the
/// type for `run_native` itself, not for the app factory.
type CreationResult = std::result::Result<Box<dyn eframe::App>, DynError>;

/// eframe's boxed error type, re-declared because eframe does not export the
/// alias it uses internally.
type DynError = Box<dyn std::error::Error + Send + Sync>;

/// Parsed command-line arguments.
struct Cli {
    /// Show the design-system gallery.
    gallery: bool,
    /// The requested theme mode.
    theme: ThemeMode,
    /// An explicit start directory, if one was given.
    dir: Option<std::path::PathBuf>,
    /// `--help` was asked for; print and exit 0.
    help: bool,
    /// `--screenshot PATH`: write a PNG of the UI here and exit.
    screenshot: Option<std::path::PathBuf>,
    /// `--scene NAME`: the state to drive the app into before capturing.
    scene: app::Scene,
    /// `--size WxH`: an explicit capture size.
    size: Option<egui::Vec2>,
    /// `--tree`: start in the indented tree view.
    tree: bool,
}

/// The `--help` text, matching the module docs above.
const HELP: &str = "\
kestrel — a lightweight file manager

USAGE:
    kestrel [OPTIONS] [DIR]

OPTIONS:
        --gallery      render the design-system gallery instead of the file manager
    -l, --light        force the light theme
    -d, --dark         force the dark theme (the default fallback)
    -s, --system       follow the compositor (the default)
    -h, --help         print this help
        --tree         start in the indented tree view (default: flat list)
        --screenshot PATH
                       render the UI, write a PNG to PATH, and exit
        --scene NAME
                       drive the app into a state, then capture it. One of:
                       browser, confirm-permanent, confirm-trash, collision,
                       progress, failed, cannot-open, preview-empty,
                       preview-text, preview-image, preview-too-large, settings,
                       help, empty, denied, gone, nowatch
        --size WxH     capture at an explicit size, e.g. 900x700

ARGS:
    <DIR>              directory to open (default: $KESTREL_HOME, else $HOME, else /)

KEYBOARD:
    F1, ?              the full shortcut list
    Ctrl+,             settings
    Ctrl+F, /          focus the filter field
    Alt+Left/Right     back / forward
    Alt+Up, Backspace  up one level
    F5                 refresh

ENVIRONMENT:
    KESTREL_HOME       overrides the start directory
    WAYLAND_DISPLAY    the Wayland socket to open against
    KESTREL_REDUCED_MOTION
                       1/true/yes collapses every motion duration to zero
                       (spec 2.11 rule 5); 0/false forces motion on
    KESTREL_DEBUG_LAYOUT
                       dev builds only: print the gallery's layout widths
";

/// The `--scene` names, in the order they appear in `--help`.
///
/// A closed table rather than a `FromStr` on an open string: a mistyped scene
/// has to be an error listing the real names, not a silent fallback to the
/// default state that then produces a perfectly reviewable PNG of the wrong
/// thing.
const SCENES: &[(&str, app::Scene)] = &[
    ("browser", app::Scene::Browser),
    ("confirm-permanent", app::Scene::ConfirmPermanent),
    ("confirm-trash", app::Scene::ConfirmTrash),
    ("collision", app::Scene::Collision),
    ("progress", app::Scene::Progress),
    ("failed", app::Scene::Failed),
    ("cannot-open", app::Scene::CannotOpen),
    ("preview-empty", app::Scene::PreviewEmpty),
    ("preview-text", app::Scene::PreviewText),
    ("preview-image", app::Scene::PreviewImage),
    ("preview-too-large", app::Scene::PreviewTooLarge),
    ("settings", app::Scene::Settings),
    ("help", app::Scene::Help),
    ("empty", app::Scene::Empty),
    ("denied", app::Scene::Denied),
    ("gone", app::Scene::Gone),
    ("nowatch", app::Scene::NoWatch),
];

/// `None` for a name no scene answers to, with the list of the ones that do.
fn scene_by_name(name: &str) -> Option<app::Scene> {
    SCENES.iter().find(|(n, _)| *n == name).map(|(_, s)| *s)
}

impl Cli {
    /// Parses `args` (excluding `argv[0]`).
    ///
    /// Hand-rolled rather than pulled from a crate: this is eleven branches, and
    /// a dependency for it would be more code than the parser. An unknown flag is
    /// an error, not a silent no-op — a file manager that quietly ignores
    /// `--galery` is a bad afternoon.
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut out = Self {
            gallery: false,
            theme: ThemeMode::System,
            dir: None,
            help: false,
            screenshot: None,
            scene: app::Scene::Browser,
            size: None,
            tree: false,
        };
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--gallery" => out.gallery = true,
                "--tree" => out.tree = true,
                "--screenshot" => {
                    // A missing value is a usage error, not a default path.
                    let Some(path) = iter.next() else {
                        return Err("--screenshot needs a path".to_string());
                    };
                    out.screenshot = Some(std::path::PathBuf::from(path));
                }
                "--scene" => {
                    let Some(name) = iter.next() else {
                        return Err("--scene needs a name".to_string());
                    };
                    out.scene = scene_by_name(name).ok_or_else(|| {
                        let names: Vec<&str> = SCENES.iter().map(|(n, _)| *n).collect();
                        format!("unknown scene: {name} (try: {})", names.join(", "))
                    })?;
                }
                "--size" => {
                    let Some(spec) = iter.next() else {
                        return Err("--size needs WxH".to_string());
                    };
                    out.size = Some(parse_size(spec)?);
                }
                "-l" | "--light" => out.theme = ThemeMode::Light,
                "-d" | "--dark" => out.theme = ThemeMode::Dark,
                "-s" | "--system" => out.theme = ThemeMode::System,
                "-h" | "--help" => out.help = true,
                other if other.starts_with('-') => {
                    return Err(format!("unknown option: {other}"));
                }
                dir => {
                    if out.dir.is_some() {
                        return Err("only one directory may be given".to_string());
                    }
                    out.dir = Some(std::path::PathBuf::from(dir));
                }
            }
        }
        Ok(out)
    }
}

/// Parses `WxH` into a size.
///
/// A wrong size is a wrong size, not a default: a capture that quietly ignored
/// `--size 900x700` would render a 1200px layout into a file named for a 900px
/// one, and the whole point of the flag is to see a narrow pane.
fn parse_size(spec: &str) -> Result<egui::Vec2, String> {
    let Some((w, h)) = spec.split_once(['x', 'X']) else {
        return Err(format!("--size wants WxH, got {spec}"));
    };
    let width: f32 = w
        .parse()
        .map_err(|_| format!("--size width is not a number: {w}"))?;
    let height: f32 = h
        .parse()
        .map_err(|_| format!("--size height is not a number: {h}"))?;
    if width < 200.0 || height < 200.0 {
        return Err(format!("--size is too small to lay out: {spec}"));
    }
    Ok(egui::vec2(width, height))
}

fn main() -> std::process::ExitCode {
    // `env_logger` reads `RUST_LOG`. The default is `warn` for every crate and
    // `info` for this one: the GUI dependency tree is chatty at `info` — the
    // `accesskit` feature logs a whole D-Bus handshake to stderr on startup —
    // and a file manager that prints 8 lines of protocol trace before it shows
    // a window is a file manager nobody debugs a second time.
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn,kestrel=info"),
    )
    .format_timestamp_millis()
    .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut cli = match Cli::parse(&args) {
        Ok(cli) => cli,
        Err(message) => {
            eprintln!("kestrel: {message}\n\n{HELP}");
            return std::process::ExitCode::from(2);
        }
    };
    if cli.help {
        print!("{HELP}");
        return std::process::ExitCode::SUCCESS;
    }

    // An explicit DIR wins over `$KESTREL_HOME`, which wins over `$HOME`.
    // `Options::start_dir = None` is what selects the fallback chain, so the
    // distinction matters and is not collapsed here.
    let opts = app::Options {
        gallery: cli.gallery,
        theme: cli.theme,
        start_dir: cli.dir.take(),
        tree: cli.tree,
    };

    // `--screenshot` replaces the whole run with a short, self-terminating
    // capture, so the theme can be reviewed from a file rather than through
    // whatever happens to be on top of the desktop. It is checked *before* the
    // `AppCreator` is built because that closure moves `opts`.
    if let Some(path) = cli.screenshot.take() {
        return match shot::capture(opts, cli.scene, cli.size, &path) {
            Ok(()) => {
                println!("wrote {}", path.display());
                std::process::ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("kestrel: screenshot failed: {err}");
                std::process::ExitCode::FAILURE
            }
        };
    }

    let native = NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Kestrel")
            .with_app_id("dev.kestrel.app")
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([640.0, 400.0])
            // Hyprland draws its own decorations, and `winit/default` (which
            // would pull in `wayland-csd-adwaita`) is deliberately not enabled.
            .with_decorations(false),
        ..NativeOptions::default()
    };

    // Two 0.36 details are load-bearing here and neither is guessable:
    //
    // * `run_native` takes **three** arguments — there is no `persistence`
    //   argument any more; and
    // * the creator returns a `Result`, not a bare `Box`.
    //
    // `AppCreator` is a higher-ranked boxed `FnOnce`, so the closure's parameter
    // and return types have to be spelled out: the compiler cannot infer `cc`
    // from `cc.egui_ctx` alone against the HRTB.
    let create: eframe::AppCreator<'_> =
        Box::new(move |cc: &eframe::CreationContext<'_>| -> CreationResult {
            // Fonts are installed before any style work, so the very first
            // frame is already laid out with the right families.
            tokens::fonts::install(&cc.egui_ctx);
            // `egui_extras` is linked for its image loaders only; there are no
            // images yet, but Phase 3 adds thumbnails and shipping the loaders
            // late would mean a rebuild of a 400-crate dependency graph.
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Ok(Box::new(app::KestrelApp::new(cc, opts)) as Box<dyn eframe::App>)
        });

    let result = eframe::run_native("Kestrel", native, create);

    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            // A windowing or GL failure is the one error a user can act on, so
            // it goes to stderr with the full chain rather than being swallowed.
            log::error!("eframe: {err}");
            eprintln!("kestrel: could not open a window: {err}");
            let mut source = err.source();
            let mut depth = 0usize;
            while let Some(cause) = source {
                depth += 1;
                eprintln!("  {depth}: {cause}");
                source = cause.source();
            }
            std::process::ExitCode::FAILURE
        }
    }
}
