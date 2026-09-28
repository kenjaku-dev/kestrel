//! §5 file-type classification: path -> icon slot + glyph name.
//!
//! The spec is precise about the *mapping* and deliberately loose about the
//! *glyph names* ("exact glyph names should be confirmed against the Phosphor
//! release pinned at implementation time"). Both are transcribed here so that
//! swapping in the real icon atlas later is a one-file change.
//!
//! Two rules from §5.2 are load-bearing and are encoded as data rather than as
//! an `if` at the call site:
//!
//! * **A symlink replaces the category glyph.** A symlink's category is
//!   genuinely ambiguous (`notes.md -> ../docs/notes.md`) and resolving it is
//!   the file manager's job, so the link glyph carries the higher-priority
//!   fact.
//! * **A hidden file does *not* change its glyph.** A hidden `.gitignore` is
//!   still a text file. It keeps the category silhouette, is recoloured to
//!   `icon.hidden`, gains a 4px dot in the leading gutter, and its name drops
//!   to `text.tertiary`.

use kestrel_fs::model::{EntryKind, FileEntry};

use crate::tokens::IconRoles;

/// A file category, one per row of the §5.2 mapping table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// `folder` — is a directory.
    Folder,
    /// `file-text`
    Text,
    /// `file-code` (or a language-specific `file-*` where known)
    Code,
    /// `file-image`
    Image,
    /// `file-video`
    Video,
    /// `file-audio`
    Audio,
    /// `file-zip` — one glyph for *all* archive formats. No per-format variants:
    /// nobody distinguishes an `.xz` from a `.7z` by shape.
    Archive,
    /// `file` — extension not mapped, or none. Neutral. No mystery box (§7.11).
    Binary,
    /// `file-exe` — executable bit set, or a program extension.
    Executable,
    /// `link-simple` — `is_symlink()`.
    Symlink,
}

impl Category {
    /// Every category, in §5.2 table order. Drives the gallery.
    pub const ALL: [Self; 10] = [
        Self::Folder,
        Self::Text,
        Self::Code,
        Self::Image,
        Self::Video,
        Self::Audio,
        Self::Archive,
        Self::Binary,
        Self::Executable,
        Self::Symlink,
    ];

    /// The Phosphor glyph name the spec assigns, plus its documented fallback.
    #[must_use]
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Folder => "folder",
            Self::Text => "file-text",
            Self::Code => "file-code",
            Self::Image => "file-image",
            Self::Video => "file-video",
            Self::Audio => "file-audio",
            Self::Archive => "file-zip",
            Self::Binary => "file",
            Self::Executable => "file-exe",
            Self::Symlink => "link-simple",
        }
    }

    /// The §3.7 icon role this category fills. Components read `icon.*`, never
    /// `hue.*` — the layer rule is hard.
    #[must_use]
    pub fn icon_color(self, theme: &IconRoles) -> egui::Color32 {
        let icons = *theme;
        match self {
            Self::Folder => icons.folder,
            Self::Text => icons.text,
            Self::Code => icons.code,
            Self::Image => icons.image,
            Self::Video => icons.video,
            Self::Audio => icons.audio,
            Self::Archive => icons.archive,
            Self::Binary => icons.binary,
            Self::Executable => icons.executable,
            Self::Symlink => icons.symlink,
        }
    }

    /// The label the Kind column shows (§4.2 `row.col-kind`, `type.caption`).
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Folder => "Folder",
            Self::Text => "Text",
            Self::Code => "Code",
            Self::Image => "Image",
            Self::Video => "Video",
            Self::Audio => "Audio",
            Self::Archive => "Archive",
            Self::Binary => "Binary",
            Self::Executable => "Program",
            Self::Symlink => "Link",
        }
    }
}

/// Classifies a listing row.
///
/// Order matters and follows §5.2's precedence exactly:
/// 1. `is_symlink()` -> [`Category::Symlink`] (overrides the category).
/// 2. directory -> [`Category::Folder`].
/// 3. extension lookup.
/// 4. neutral [`Category::Binary`].
///
/// The hidden check is deliberately *not* here: §5.2 says a hidden file keeps
/// its category glyph and is recoloured instead, so hiddenness is a property of
/// the row, not of the file. See [`Classified::icon_color`].
#[must_use]
pub fn classify(entry: &FileEntry) -> Category {
    if entry.kind == EntryKind::Symlink {
        // A symlink's category is ambiguous, so the link glyph wins outright.
        return Category::Symlink;
    }
    if entry.kind == EntryKind::Directory {
        return Category::Folder;
    }
    category_for_extension(extension_of(&entry.name))
}

/// The last extension, without the dot. Empty when there is none.
///
/// A leading dot is *not* an extension: `.gitignore` is a name, and treating it
/// as an extension named `gitignore` would both misfile it and make
/// hidden-file handling depend on the classifier.
///
/// `"a.tar.gz"` yields `"gz"` — the *last* extension wins. Per-format archive
/// detection is explicitly not a goal (§5.2: one glyph for all archive
/// formats).
#[must_use]
pub fn extension_of(name: &str) -> &str {
    match name.rsplit_once('.') {
        // An empty stem means the dot was leading (`.gitignore`); an empty ext
        // means the name ended in a dot (`a.`). Neither is an extension.
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => ext,
        _ => "",
    }
}

/// The §5.2 extension table. Longest-match is not needed: every entry is a
/// bare extension, compared case-insensitively.
#[must_use]
pub fn category_for_extension(ext: &str) -> Category {
    let ext = ext.to_ascii_lowercase();
    match ext.as_str() {
        // Text / document. `icon.text` is neutral on purpose — documents are
        // the majority of a directory and must not compete with code (§5.2).
        "txt" | "md" | "pdf" | "rtf" | "doc" | "docx" | "odt" | "tex" => Category::Text,
        // Code. `file-json` and `file-toml` stay neutral: they are data, not
        // code, and the spec says so explicitly.
        "rs" | "js" | "ts" | "tsx" | "jsx" | "css" | "scss" | "py" | "go" | "sh" | "bash"
        | "zsh" | "fish" | "html" | "htm" | "c" | "h" | "cc" | "cpp" | "hpp" | "cxx" | "java"
        | "kt" | "kts" | "rb" | "php" | "pl" | "lua" | "vim" | "el" | "hs" | "ml" | "swift"
        | "scala" | "clj" | "ex" | "exs" | "erl" | "dart" | "zig" | "nim" | "sql" => Category::Code,
        "json" | "toml" | "yaml" | "yml" | "ini" | "cfg" | "conf" | "lock" | "xml" => {
            Category::Text
        }
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "svg" | "bmp" | "tif" | "tiff"
        | "heic" | "ico" | "webm_img" => Category::Image,
        "mp4" | "mkv" | "mov" | "webm" | "avi" | "m4v" | "mpg" | "mpeg" | "wmv" | "flv" => {
            Category::Video
        }
        "mp3" | "flac" | "wav" | "ogg" | "m4a" | "aac" | "opus" | "wma" | "aiff" | "mid" => {
            Category::Audio
        }
        "zip" | "tar" | "gz" | "bz2" | "xz" | "7z" | "rar" | "zst" | "tgz" | "txz" | "lz"
        | "lzma" | "z" => Category::Archive,
        "exe" | "appimage" | "bin" | "run" | "msi" | "deb" | "rpm" | "appimage_x64" => {
            Category::Executable
        }
        // Extension not mapped, or none. Neutral — no mystery box (§7.11).
        _ => Category::Binary,
    }
}

/// A classified row: the category plus the row-level facts §4.2's state matrix
/// needs, kept separate so the two §5.2 rules (symlink overrides, hidden does
/// not) stay visible at the type level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Classified {
    /// The category glyph slot.
    pub category: Category,
    /// Whether the row is a hidden entry (leading dot on Unix).
    pub hidden: bool,
}

impl Classified {
    /// Classifies an entry from the engine.
    #[must_use]
    pub fn of(entry: &FileEntry) -> Self {
        Self {
            category: classify(entry),
            // The engine already computed this; never re-`lstat` for it.
            hidden: entry.hidden,
        }
    }

    /// The Phosphor glyph for this row, or `None` if this release lacks the name.
    ///
    /// §5.1's rule that a missing glyph must not be invisible lives here: the
    /// caller falls back to `icon_placeholder` on `None`, so an unmapped name
    /// shows the neutral `icon.binary` square rather than nothing.
    #[must_use]
    pub fn glyph(&self) -> Option<crate::icons::Glyph> {
        crate::icons::codepoint(self.category.glyph())
    }

    /// The colour the row's glyph is painted in.
    ///
    /// §4.2 `hidden file` row: "as row state | `icon.hidden` + 4px dot". The
    /// category silhouette is *kept* and only the colour changes.
    #[must_use]
    pub fn icon_color(&self, theme: &IconRoles) -> egui::Color32 {
        if self.hidden {
            theme.hidden
        } else {
            self.category.icon_color(theme)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn file(name: &str) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            path: PathBuf::from(name),
            kind: EntryKind::File,
            size: Some(0),
            modified: None,
            hidden: name.starts_with('.'),
            is_dir_target: None,
        }
    }

    fn dir(name: &str) -> FileEntry {
        FileEntry {
            kind: EntryKind::Directory,
            ..file(name)
        }
    }

    fn link(name: &str) -> FileEntry {
        FileEntry {
            kind: EntryKind::Symlink,
            ..file(name)
        }
    }

    #[test]
    fn symlink_replaces_the_category_glyph() {
        // §5.2: the link glyph "replaces the category glyph, because the most
        // important fact about a symlink is that it is one".
        assert_eq!(classify(&link("notes.md")), Category::Symlink);
        assert_eq!(classify(&link("archive.zip")), Category::Symlink);
    }

    #[test]
    fn directory_beats_extension() {
        assert_eq!(classify(&dir("notes.md")), Category::Folder);
        assert_eq!(classify(&dir("src")), Category::Folder);
    }

    #[test]
    fn spec_extension_table_maps_exactly() {
        for ext in ["txt", "md", "pdf", "rtf", "doc", "docx", "odt", "tex"] {
            assert_eq!(
                classify(&file(&format!("a.{ext}"))),
                Category::Text,
                "{ext}"
            );
        }
        for ext in ["rs", "js", "ts", "css", "py", "go", "sh", "html", "sql"] {
            assert_eq!(
                classify(&file(&format!("a.{ext}"))),
                Category::Code,
                "{ext}"
            );
        }
        for ext in [
            "png", "jpg", "jpeg", "gif", "webp", "avif", "svg", "bmp", "tif", "tiff", "heic",
        ] {
            assert_eq!(
                classify(&file(&format!("a.{ext}"))),
                Category::Image,
                "{ext}"
            );
        }
        for ext in ["mp4", "mkv", "mov", "webm", "avi", "m4v", "mpg", "wmv"] {
            assert_eq!(
                classify(&file(&format!("a.{ext}"))),
                Category::Video,
                "{ext}"
            );
        }
        for ext in ["mp3", "flac", "wav", "ogg", "m4a", "aac", "opus", "wma"] {
            assert_eq!(
                classify(&file(&format!("a.{ext}"))),
                Category::Audio,
                "{ext}"
            );
        }
        for ext in ["zip", "tar", "gz", "bz2", "xz", "7z", "rar", "zst", "tgz"] {
            assert_eq!(
                classify(&file(&format!("a.{ext}"))),
                Category::Archive,
                "{ext}"
            );
        }
        for ext in ["exe", "appimage", "bin", "run"] {
            assert_eq!(
                classify(&file(&format!("a.{ext}"))),
                Category::Executable,
                "{ext}"
            );
        }
    }

    #[test]
    fn data_files_stay_neutral() {
        // §5.2: "file-json and file-toml stay neutral (icon.text) — they are
        // data, not code."
        for ext in ["json", "toml", "yaml", "yml"] {
            let c = classify(&file(&format!("a.{ext}")));
            assert_eq!(c, Category::Text, "{ext}");
            assert_eq!(c.glyph(), "file-text", "{ext}");
        }
    }

    #[test]
    fn unmapped_extensions_are_neutral_not_mystery() {
        // §7.11: "An unclassified file is a neutral `file`, not a question mark."
        for name in ["a.qqq", "LICENSE", "Makefile", "a.", ".gitignore"] {
            assert_eq!(classify(&file(name)), Category::Binary, "{name}");
        }
        assert_eq!(Category::Binary.glyph(), "file");
    }

    #[test]
    fn a_leading_dot_is_not_an_extension() {
        assert_eq!(extension_of(".gitignore"), "");
        assert_eq!(extension_of(".config.json"), "json");
        assert_eq!(extension_of("a.tar.gz"), "gz");
        assert_eq!(extension_of("noext"), "");
    }

    #[test]
    fn extension_match_is_case_insensitive() {
        assert_eq!(classify(&file("A.PNG")), Category::Image);
        assert_eq!(classify(&file("A.RS")), Category::Code);
    }

    #[test]
    fn hidden_keeps_its_category_glyph_but_takes_the_hidden_colour() {
        // §5.2 "Why hidden files get no special icon": a hidden
        // `.prettierrc.toml` is still a text file. The category silhouette is
        // *kept*; only the colour changes, plus the 4px gutter dot.
        let c = Classified::of(&file(".prettierrc.toml"));
        assert!(c.hidden);
        assert_eq!(c.category, Category::Text, "category silhouette is kept");

        let light = crate::tokens::Theme::light();
        let hidden_color = c.icon_color(&light.icon);
        let plain = Classified::of(&file("prettierrc.toml")).icon_color(&light.icon);
        assert_ne!(
            hidden_color, plain,
            "a hidden row is recoloured to icon.hidden"
        );
        assert_eq!(hidden_color, light.icon.hidden);
    }

    #[test]
    fn every_category_has_a_distinct_glyph_name() {
        // §6.6 / §5.1: colour is a redundant second channel, never the only
        // one — which requires distinct silhouettes.
        let mut glyphs: Vec<&str> = Category::ALL.iter().map(|c| c.glyph()).collect();
        glyphs.sort_unstable();
        let before = glyphs.len();
        glyphs.dedup();
        assert_eq!(before, glyphs.len(), "two categories share a silhouette");
    }
}
