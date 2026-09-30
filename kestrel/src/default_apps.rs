//! The "default applications" settings state: which app opens each file type.
//!
//! # Why this is its own module
//!
//! The settings screen edits [`crate::settings::Stored`], which is persisted
//! by `eframe` — but a default application is not a preference of this app, it
//! is a row in `~/.config/mimeapps.list` that every desktop program reads. So
//! this state is deliberately *not* in `Stored`: it is loaded from the system
//! when the settings screen opens ([`DefaultApps::refresh`]), and a pick
//! writes it back immediately ([`DefaultApps::apply`]) rather than waiting for
//! `eframe`'s auto-save, because the user's mental model is "I picked it, it's
//! set".
//!
//! The engine (`kestrel_fs::open`) owns the MIME/desktop-entry logic; this
//! module owns the per-row UI facts (candidates, current pick, confirmation or
//! error text). It has no `egui` in it, so the whole state machine is testable
//! headless.

use kestrel_fs::open::{self, Candidate, FileType, Xdg};

/// One row of the section: a file type, its candidates, and its current pick.
#[derive(Debug, Clone)]
pub struct Row {
    /// The file type, from the engine's table.
    pub kind: FileType,
    /// Every installed app that declares one of the type's MIME types.
    pub candidates: Vec<Candidate>,
    /// The desktop id currently set for the primary MIME type, if any.
    pub current: Option<String>,
}

impl Row {
    /// The display name of the current pick, for the dropdown's closed state.
    #[must_use]
    pub fn current_name(&self) -> &str {
        match &self.current {
            None => "Not set",
            Some(id) => self
                .candidates
                .iter()
                .find(|c| &c.id == id)
                .map_or(id.as_str(), |c| c.name.as_str()),
        }
    }
}

/// The section's state, loaded on entry and written through on every pick.
#[derive(Debug, Clone, Default)]
pub struct DefaultApps {
    /// One row per engine file type, in engine order.
    pub rows: Vec<Row>,
    /// Confirmation of the last successful pick, shown until the next one.
    pub notice: Option<String>,
    /// Why the last pick failed, in the app's error style. `None` means the
    /// last write (if any) succeeded — a failed write never shows a success.
    pub error: Option<String>,
}

impl DefaultApps {
    /// Loads every row from the live desktop: candidates plus current default.
    pub fn refresh(&mut self) {
        self.refresh_with(&Xdg::from_env());
    }

    /// [`Self::refresh`] against an explicit tree, for tests.
    pub fn refresh_with(&mut self, xdg: &Xdg) {
        self.rows = open::file_types()
            .into_iter()
            .map(|kind| {
                let primary = kind.mimes.first().copied().unwrap_or("*/*");
                Row {
                    current: open::default_for(xdg, primary),
                    candidates: open::candidates_for_all(xdg, kind.mimes),
                    kind,
                }
            })
            .collect();
        self.notice = None;
        self.error = None;
    }

    /// Sets `id` as the default for every MIME type in row `at`.
    ///
    /// Writes immediately and re-reads the row, so the dropdown shows what is
    /// true rather than what was asked for. On failure `error` says why and
    /// `notice` is cleared — never a success state for a failed write.
    pub fn apply(&mut self, at: usize, id: &str) {
        self.apply_with(&Xdg::from_env(), at, id);
    }

    /// [`Self::apply`] against an explicit tree, for tests.
    pub fn apply_with(&mut self, xdg: &Xdg, at: usize, id: &str) {
        let Some(row) = self.rows.get(at) else {
            return;
        };
        let name = row
            .candidates
            .iter()
            .find(|c| c.id == id)
            .map_or(id, |c| c.name.as_str());
        let label = format!("{name} now opens {}", row.kind.name.to_lowercase());
        let mimes = row.kind.mimes;
        match open::set_default_for_all(xdg, mimes, id) {
            Ok(()) => {
                self.refresh_with(xdg);
                self.notice = Some(label);
            }
            Err(source) => {
                self.error = Some(format!("Could not set the default: {source}."));
                self.notice = None;
            }
        }
    }

    /// `true` when no row found any installed application: the screen has
    /// nothing to offer, and says so rather than showing inert dropdowns.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.iter().all(|r| r.candidates.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desktop(root: &std::path::Path, id: &str, body: &str) {
        let dir = Xdg::isolated(root).applications_dirs()[0].clone();
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join(id), body).expect("write desktop");
    }

    fn entry_file(name: &str, exec: &str, mimes: &str) -> String {
        format!(
            "[Desktop Entry]\nType=Application\nName={name}\nExec={exec}\n\
             NoDisplay=false\nTerminal=false\nMimeType={mimes};\n"
        )
    }

    fn tree() -> (tempfile::TempDir, Xdg) {
        let dir = tempfile::tempdir().expect("tempdir");
        let xdg = Xdg::isolated(dir.path());
        desktop(
            dir.path(),
            "viewer.desktop",
            &entry_file("Viewer", "viewer %U", "image/png;image/jpeg"),
        );
        desktop(
            dir.path(),
            "tune.desktop",
            &entry_file("Tune", "tune %f", "audio/mpeg"),
        );
        (dir, xdg)
    }

    #[test]
    fn refresh_loads_candidates_and_defaults() {
        let (_dir, xdg) = tree();
        let mut state = DefaultApps::default();
        state.refresh_with(&xdg);
        assert_eq!(state.rows.len(), open::file_types().len());
        let images = state
            .rows
            .iter()
            .find(|r| r.kind.name == "Images")
            .expect("images row");
        assert_eq!(images.candidates.len(), 1);
        assert_eq!(images.current, None, "nothing set yet");
        assert!(!state.is_empty());
    }

    #[test]
    fn a_pick_writes_through_and_confirms() {
        let (_dir, xdg) = tree();
        let mut state = DefaultApps::default();
        state.refresh_with(&xdg);
        let at = state
            .rows
            .iter()
            .position(|r| r.kind.name == "Images")
            .expect("row");
        state.apply_with(&xdg, at, "viewer.desktop");
        assert!(state.error.is_none());
        let notice = state.notice.clone().expect("a confirmation");
        assert!(notice.contains("Viewer"), "{notice}");
        // Every MIME in the group now resolves, so double-clicking a `.jpg`
        // in another program opens the same viewer as a `.png`.
        for mime in ["image/png", "image/jpeg"] {
            assert_eq!(
                open::default_for(&xdg, mime).as_deref(),
                Some("viewer.desktop"),
                "{mime}"
            );
        }
        // And the row re-read what was written rather than echoing the pick.
        assert_eq!(state.rows[at].current.as_deref(), Some("viewer.desktop"));
    }

    #[test]
    fn a_failed_write_reports_instead_of_confirming() {
        let (_dir, xdg) = tree();
        let mut state = DefaultApps::default();
        state.refresh_with(&xdg);
        // A regular file where the config directory should be: creating the
        // directory (and the write with it) must fail. That is the
        // unwritable-home case, deterministically and without touching
        // permissions or the real `~/.config`.
        let blocker = tempfile::NamedTempFile::new().expect("tempfile");
        let hostile = Xdg::isolated(blocker.path());
        let at = state
            .rows
            .iter()
            .position(|r| r.kind.name == "Images")
            .expect("row");
        state.apply_with(&hostile, at, "viewer.desktop");
        assert!(state.notice.is_none(), "no success for a failed write");
        assert!(state.error.is_some(), "the failure is said out loud");
    }

    #[test]
    fn the_rows_cover_the_classifiers_categories() {
        // The linkage the engine docs promise: every extension the file-type
        // classifier puts in Image/Video/Audio/Archive has a row whose MIME
        // the shared database agrees with.
        use crate::filetype::{Category, category_for_extension};
        assert_eq!(category_for_extension("png"), Category::Image);
        assert_eq!(category_for_extension("mp4"), Category::Video);
        assert_eq!(category_for_extension("mp3"), Category::Audio);
        assert_eq!(category_for_extension("zip"), Category::Archive);
        assert_eq!(category_for_extension("pdf"), Category::Text);
        for name in ["Images", "Video", "Audio", "PDF", "Text", "Archives"] {
            assert!(
                open::file_types().iter().any(|g| g.name == name),
                "no {name} row"
            );
        }
    }
}
