//! Sidebar places (§4.1) and path resolution.
//!
//! `Home`, `Documents`, `Downloads` and the filesystem root, resolved from
//! `$HOME` with no `dirs` dependency. Resolution happens **once, at startup**,
//! never in `App::ui` — `$HOME` is an environment read, and while it is a
//! dictionary lookup rather than a syscall, reading it per frame would still be
//! the wrong shape (see `app.rs` on the no-blocking rule).

use std::path::{Path, PathBuf};

/// One entry in the sidebar's Places list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    /// Stable identity, so the active-place test survives a rename.
    pub id: &'static str,
    /// The label shown in the sidebar.
    pub label: &'static str,
    /// The Phosphor glyph **name** the spec assigns (§5.3). Resolved to a
    /// codepoint by [`Place::glyph`].
    pub glyph_name: &'static str,
    /// The resolved absolute path. `None` if the place does not exist on this
    /// machine, which renders as a disabled row (§4.1 `disabled`).
    pub path: Option<PathBuf>,
}

impl Place {
    /// `true` when this place can be navigated to.
    #[must_use]
    pub fn is_available(&self) -> bool {
        self.path.is_some()
    }

    /// The place's path, or `None` when unavailable.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The §5.3 Phosphor glyph, or `None` if this release does not have it.
    ///
    /// Taking the lookup here rather than in `app.rs` keeps the *name* — which
    /// is the spec's word — next to the resolution, so a rename in Phosphor is a
    /// one-line change in one place.
    #[must_use]
    pub fn glyph(&self, _theme: &crate::tokens::Theme) -> Option<crate::icons::Glyph> {
        crate::icons::codepoint(self.glyph_name)
    }
}

/// The user's home directory, from `$HOME`.
///
/// Falls back to `None` rather than to `std::env::home_dir()`, because
/// `home_dir` is itself a wrapper around `getpwuid` on some platforms and the
/// spec's environment is Artix, where `$HOME` is always set for a login shell.
/// `None` makes the sidebar fall back to `/` rather than panicking.
#[must_use]
pub fn home_dir() -> Option<PathBuf> {
    match std::env::var_os("HOME") {
        // An empty or relative `$HOME` is not a home directory; treating it as
        // one would put the file list somewhere the user did not ask for.
        Some(h) if !h.is_empty() => {
            let path = PathBuf::from(h);
            if path.is_absolute() { Some(path) } else { None }
        }
        _ => None,
    }
}

/// A subdirectory of `$HOME` that may or may not exist.
///
/// XDG says `Documents` is `$HOME/Documents` or `$HOME/Dokumente`; Artix users
/// commonly use the latter. Both are accepted, and the first that exists wins.
#[must_use]
fn under_home(home: &Path, names: &[&str]) -> Option<PathBuf> {
    names.iter().map(|n| home.join(n)).find(|p| p.is_dir())
}

/// The Places list, resolved once at startup.
///
/// Ordered as the spec's §4.1 covers suggest: places, then the filesystem root.
/// Nothing here touches the filesystem except two `is_dir` calls at startup,
/// which is a handful of stat(2) calls before the window exists.
#[must_use]
pub fn places() -> Vec<Place> {
    let home = home_dir();
    vec![
        // `house` / 18px / icon.chrome (§5.3)
        Place {
            id: "home",
            label: "Home",
            glyph_name: "house",
            path: home.clone(),
        },
        // `file-text` / 18px / icon.chrome — note this is a *chrome* icon, so it
        // is `icon.chrome`, not `icon.text`, despite naming a text file.
        Place {
            id: "documents",
            label: "Documents",
            glyph_name: "file-text",
            path: home
                .as_deref()
                .and_then(|h| under_home(h, &["Documents", "Dokumente", "My Documents"])),
        },
        // `download-simple` / 18px / icon.chrome
        Place {
            id: "downloads",
            label: "Downloads",
            glyph_name: "download-simple",
            path: home
                .as_deref()
                .and_then(|h| under_home(h, &["Downloads", "Downloads_iso", "Heruntergeladen"])),
        },
        // `hard-drive` / 18px / icon.chrome
        Place {
            id: "root",
            label: "Filesystem",
            glyph_name: "hard-drive",
            path: Some(PathBuf::from("/")),
        },
    ]
}

/// The directory the app opens on first launch.
///
/// `KESTREL_HOME` wins if set (handy for testing and for the gallery), then
/// `$HOME`, then `/`. Resolution is a `PathBuf` build, not a filesystem probe,
/// so it cannot fail.
#[must_use]
pub fn start_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("KESTREL_HOME") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_is_always_available() {
        // `/` exists on every Unix host, so the last place can never be a
        // disabled row. If this ever fails on Linux, the "Filesystem" entry
        // needs a real root check.
        let root = places().into_iter().find(|p| p.id == "root");
        assert!(root.is_some_and(|p| p.is_available()));
    }

    #[test]
    fn places_are_resolved_once_and_deduplicated_by_id() {
        let ps = places();
        let mut ids: Vec<&str> = ps.iter().map(|p| p.id).collect();
        let before = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(before, ids.len(), "duplicate place id");
        assert_eq!(ps.len(), 4, "spec asks for Home/Documents/Downloads/root");
    }

    #[test]
    fn every_place_glyph_resolves_in_the_vendored_font() {
        // A sidebar row whose glyph is missing renders a fallback square, which
        // is a silent regression. Assert the resolution instead.
        for p in places() {
            let theme = crate::tokens::Theme::dark();
            assert!(
                p.glyph(&theme).is_some(),
                "{} names {:?}, which this Phosphor release lacks",
                p.id,
                p.glyph_name
            );
        }
    }

    #[test]
    fn every_place_has_a_glyph_and_a_label() {
        // §7.14: no icon-only control without a name. A sidebar row is
        // icon + label, so both must be present and non-empty.
        for p in places() {
            assert!(!p.glyph_name.is_empty(), "{} has no glyph", p.id);
            assert!(!p.label.is_empty(), "{} has no label", p.id);
        }
    }

    #[test]
    fn start_dir_is_absolute() {
        // A relative start directory would make every breadcrumb segment and
        // every engine path relative, which the engine's canonicalisation does
        // not expect.
        assert!(start_dir().is_absolute());
    }
}
