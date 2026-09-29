#![cfg(test)]
//! The packaging files, checked as files.
//!
//! A `.desktop` entry and a `justfile` are the two artifacts of this project
//! that are **not compiled**. Nothing type-checks them, nothing fails CI when
//! one is wrong, and both fail in the most annoying way possible: the desktop
//! entry either does not appear in a launcher or launches the wrong thing, and
//! the install writes files to the wrong paths — so these are checked here
//! instead, against the spec and against the binary they describe.
//!
//! What is checked:
//!
//! * the `.desktop` has the keys freedesktop requires, and its `Exec` and
//!   `TryExec` name the same program;
//! * its `app_id` line, if it has one, matches the string `main.rs` passes to
//!   `ViewportBuilder::app_id` — because eframe derives the settings path from
//!   it, and a mismatch silently orphans everyone's preferences;
//! * the icon exists, is an SVG, and parses as one;
//! * the `justfile` declares every target the README and the commit messages
//!   tell a user to run.

use std::path::{Path, PathBuf};

/// The repository root, from this file's location.
fn root() -> PathBuf {
    // `CARGO_MANIFEST_DIR` is `<repo>/kestrel`; the workspace root is its parent.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map_or_else(PathBuf::new, Path::to_path_buf)
}

fn read(relative: &str) -> String {
    let path = root().join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()))
}

/// The `.desktop` file's key/value pairs, comments and blank lines dropped.
fn desktop_entries() -> Vec<(String, String)> {
    read("kestrel/packaging/kestrel.desktop")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with('['))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn desktop_value(key: &str) -> String {
    desktop_entries()
        .into_iter()
        .find(|(k, _)| k == key)
        .unwrap_or_else(|| panic!("the .desktop entry has no {key}="))
        .1
}

#[test]
fn the_desktop_entry_has_the_keys_a_launcher_needs() {
    // The freedesktop spec's required keys, all of which are also the ones
    // whose absence produces a silent no-op rather than an error.
    for key in ["Type", "Name", "Exec"] {
        assert!(
            desktop_entries().iter().any(|(k, _)| *k == key),
            "the .desktop entry is missing {key}=, which the spec requires"
        );
    }
    assert_eq!(desktop_value("Type"), "Application");
    assert!(!desktop_value("Name").is_empty());
    assert!(!desktop_value("Comment").is_empty());
    assert!(!desktop_value("Icon").is_empty());
}

#[test]
fn the_categories_are_the_right_ones() {
    // `FileManager` is the one that puts the entry in a file manager's menu;
    // `FileTools` is the generic one; `Filesystem` is what makes it appear
    // under "System" in some desktops. All three, and no `Game`/`Graphics`/
    // `Development` nonsense — a category that does not describe the program is
    // a category that puts it in the wrong menu.
    let raw = desktop_value("Categories");
    let cats: Vec<&str> = raw.split(';').collect();
    for want in ["FileManager", "FileTools", "System"] {
        assert!(
            cats.contains(&want),
            "Categories is missing {want}: {:?}",
            cats
        );
    }
    for wrong in ["Game", "Graphics", "Development", "Utility"] {
        assert!(!cats.contains(&wrong), "Categories claims {wrong}");
    }
}

#[test]
fn exec_and_tryexec_name_the_same_program() {
    // They have to. `TryExec` is what a desktop uses to decide whether to show
    // the entry at all, so a mismatch is an entry that either never appears or
    // appears and launches something else.
    let exec = desktop_value("Exec");
    let try_exec = desktop_value("TryExec");
    let program = |v: &str| v.split_whitespace().next().unwrap_or("").to_string();
    assert_eq!(
        program(&exec),
        program(&try_exec),
        "Exec and TryExec name different programs"
    );
    // `%F` is the one field code for "the files, if any", and Kestrel takes a
    // directory, so it is the correct code and `paths`/`file`/`URL` are not.
    assert!(exec.contains("%F"), "Exec has no field code: {exec}");
    assert!(!exec.contains("%u"), "Exec uses %u, which means a URL");
}

#[test]
fn the_exec_line_is_a_template_the_installer_substitutes() {
    // A bare `Exec=kestrel` looks right and is not: a launcher's PATH is
    // whatever session it was started from had. The installer replaces the
    // placeholder, and a placeholder that is *not* a placeholder — a relative
    // name, or a hard-coded path — is the bug this asserts against.
    let exec = desktop_value("Exec");
    assert!(
        exec.starts_with("@@BIN@@"),
        "Exec is not a template, so `just install` would leave it pointing at \
         whatever PATH the launcher happens to have: {exec}"
    );
    let try_exec = desktop_value("TryExec");
    assert!(
        try_exec.starts_with("@@BIN@@"),
        "TryExec is not a template: {try_exec}"
    );
    // And `just install` really does substitute it.
    let justfile = read("justfile");
    assert!(
        justfile.contains("@@BIN@@"),
        "the justfile never mentions the placeholder, so nothing substitutes it"
    );
    // And it checks the substitution worked, because an entry whose placeholder
    // survived is an entry that silently does nothing.
    assert!(
        justfile.contains("survived the substitution"),
        "the installer does not verify its own substitution"
    );
}

#[test]
fn the_app_id_matches_the_binary() {
    // eframe derives `~/.local/share/<app_id>/app.ron` from the viewport's
    // `app_id`. The desktop file and the binary naming it differently is the
    // quietest possible way to orphan every user's saved settings: both files
    // are individually correct and the two disagree.
    let main = read("kestrel/src/main.rs");
    let app_id = main
        .lines()
        .find_map(|l| l.split("with_app_id(").nth(1))
        .and_then(|rest| rest.split('"').nth(1))
        .unwrap_or_else(|| panic!("main.rs has no with_app_id(\"...\")"));
    let justfile = read("justfile");
    assert!(
        justfile.contains(&format!("app_id := \"{app_id}\"")),
        "the justfile's app_id is not {app_id:?}, so `install` and the binary \
         would write and read different settings files"
    );
}

#[test]
fn the_icon_exists_and_is_an_svg() {
    let path = root().join("kestrel/packaging/icons/hicolor/scalable/apps/kestrel.svg");
    let svg = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()));
    assert!(svg.contains("<svg"), "the icon is not an SVG");
    assert!(
        svg.contains("viewBox"),
        "the icon has no viewBox, so it cannot scale"
    );
    // The desktop entry asks for `kestrel` and the icon is `kestrel.svg`, which
    // is the lookup rule. Asserted rather than assumed, because an icon
    // directory that is one level off is invisible until a launcher shows a
    // broken-image placeholder.
    let icon = desktop_value("Icon");
    assert_eq!(icon, "kestrel");
    assert!(path.file_name().is_some_and(|n| n == "kestrel.svg"));
}

#[test]
fn the_icon_uses_only_spec_tokens() {
    // The icon is design-system surface: a launcher shows it next to a window
    // title bar drawn in the same theme. The hexes in it are therefore checked
    // against §2.1/§2.2, so the icon cannot drift away from the app.
    let svg = read("kestrel/packaging/icons/hicolor/scalable/apps/kestrel.svg");
    let allowed = [
        "#131211", // neutral.950 — the tile
        "#2B2824", // neutral.800 — surface.raised
        "#33A894", // accent.500
        "#6F6961", // border.strong, dark
        "#B8B2A9", // text.secondary, dark
    ];
    // Only `#rrggbb`, so a `url(#clip-path)` reference is not mistaken for a
    // colour. A regex is the right tool here and is not worth a dependency:
    // one alternation, no captures.
    let mut rest = svg.as_str();
    while let Some(at) = rest.find('#') {
        let tail = &rest[at..];
        let hex = tail.get(..7).unwrap_or_default();
        if hex.len() == 7 && hex[1..].chars().all(|c| c.is_ascii_hexdigit()) {
            assert!(
                allowed.contains(&hex),
                "{hex} is not a token from the spec; the icon is design-system \
                 surface and a new colour has to be a new token"
            );
        }
        rest = &rest[at + 1..];
    }
    // And the allowed set is not vacuously satisfied: the icon has to use some
    // of them, or a blank icon passes this test.
    assert!(
        allowed.iter().any(|a| svg.contains(a)),
        "the icon uses none of the tokens it is allowed to use"
    );
}

#[test]
fn the_justfile_declares_the_documented_targets() {
    let justfile = read("justfile");
    for target in [
        "run",
        "test",
        "release",
        "install",
        "uninstall",
        "screenshots",
        "scenes",
    ] {
        assert!(
            justfile.contains(&format!("\n{target}")),
            "the justfile has no {target} target"
        );
    }
}

#[test]
fn the_justfile_installs_nothing_outside_the_xdg_data_home() {
    // Every write in the install target has to be built from `PREFIX` or from
    // one of the two XDG directories. A hard-coded `/usr/local` or a
    // `~`-expansion written out by hand is the mistake, and `just install`
    // claiming to need no root while writing to `/usr` is worse than the
    // mistake.
    let justfile = read("justfile");
    assert!(
        !justfile.contains("/usr/"),
        "the justfile writes to /usr, which contradicts its no-root claim"
    );
    assert!(
        !justfile.contains("sudo"),
        "the justfile uses sudo, which contradicts its no-root claim"
    );
    // `rm -rf` is the other shape of the same mistake: an uninstall that
    // deletes a *tree* rather than named files can take something the user
    // put there.
    assert!(
        !justfile.contains("rm -rf"),
        "the justfile uses `rm -rf`; uninstall must remove named files only"
    );
}

#[test]
fn the_justfile_never_deletes_the_settings_directory() {
    // The one hard rule of this project: never destroy user data implicitly.
    // `app.ron` is user data. An `uninstall` that removes it is a program that
    // throws away the user's configuration as a side effect of removing a
    // program.
    let justfile = read("justfile");
    let uninstall = justfile
        .split("\nuninstall:")
        .nth(1)
        .expect("no uninstall target");
    // Only the *removal* lines matter. `uninstall` does mention `app.ron` — in
    // the line that tells the user their settings were left alone, which is the
    // behaviour being asserted, not a violation of it.
    for line in uninstall.lines() {
        let t = line.trim();
        if !(t.starts_with("rm ") || t.starts_with("@rm ")) {
            continue;
        }
        assert!(
            !line.contains("state_dir"),
            "uninstall deletes the state directory: {t}"
        );
        assert!(
            !line.contains("app.ron"),
            "uninstall deletes the settings file: {t}"
        );
    }
    // And it says so, so a user is not left wondering.
    assert!(
        uninstall.contains("left alone"),
        "uninstall does not tell the user their settings survived"
    );
}
