//! Inline rename: what the field accepts, and the copy for why not.
//!
//! # The field is the only place a user types a path component
//!
//! Everything else about a file's name came from the filesystem. Here it comes
//! from a keyboard, and so it has to be validated before it reaches the engine —
//! not because the engine is weak (it classifies errors well) but because
//! `rename` is **destructive and not undoable**. A rejected name must cost the
//! user a red sentence under the field, not a half-applied rename they have to
//! notice three directories later.
//!
//! So: validation is a pure function, its result is a typed problem with a
//! message, and the message is a *clause* — the field already supplies the
//! subject. §4.7's rule that a destructive action must name what it will do
//! applies to a rename exactly as it applies to a delete.
//!
//! # What is actually rejected
//!
//! The list is not "anything unusual". Each entry is something the filesystem
//! either refuses outright or accepts in a way that lies to the user:
//!
//! * **Empty, `.`, `..`** — the filesystem refuses, and `..` would move the row
//!   out from under the selection.
//! * **`/` and NUL** — the filesystem refuses.
//! * **Over `NAME_MAX`** — the filesystem refuses.
//! * **A control character, including a newline** — the filesystem *accepts* it,
//!   and a filename containing `\n` renders as two blank rows in a list. That is
//!   the one that matters most here, precisely because nothing rejects it.
//! * **A trailing dot or space** — accepted on Unix, silently stripped by
//!   Windows and by SMB shares. Flagged, not blocked: on the filesystem the user
//!   is actually on, it is a legal name, and a file manager that refuses legal
//!   names on a legal filesystem is worse than one that warns.

use std::path::Path;

/// The longest name most Linux filesystems accept, in bytes.
///
/// 255 is `NAME_MAX` on ext4, xfs, btrfs and tmpfs. The real limit is
/// `pathconf(_PC_NAME_MAX)`, but that is a syscall per rename for a value that
/// is 255 on essentially every modern Linux filesystem; the engine's
/// `InvalidFilename` still catches the rare filesystem that disagrees, and this
/// catches the ordinary case before it reaches the engine.
pub const NAME_MAX: usize = 255;

/// Why a name was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// Nothing typed, or only whitespace.
    Empty,
    /// `.` or `..`.
    Reserved,
    /// Contains a path separator.
    HasSeparator,
    /// Contains a NUL byte.
    HasNul,
    /// Longer than [`NAME_MAX`].
    TooLong {
        /// The limit that was exceeded.
        max: usize,
    },
    /// Contains a control character, including a newline.
    ///
    /// A separate variant from the others because it is a *warning* on a legal
    /// filesystem: the name would work, and would be invisible.
    HasControl {
        /// The offending character.
        ch: char,
    },
    /// Ends in a dot or a space.
    TrailingInvisible,
    /// Contains an unpaired surrogate.
    ///
    /// Cannot be constructed on Unix — a Rust `String` is always valid UTF-8, and
    /// `OsStr` on Unix is arbitrary bytes rather than possibly-invalid UTF-16. So
    /// this variant exists for the Windows build and for the tests that pin the
    /// contract, and on Unix it is simply unreachable. Documented rather than
    /// omitted, because "we check for surrogates on Windows" is a claim someone
    /// will otherwise have to take on trust.
    HasSurrogate,
}

impl Problem {
    /// The clause shown under the field.
    ///
    /// No subject, no trailing period on a fragment, and never a bare technical
    /// name: "invalid filename" tells a user nothing they can act on.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::Empty => "a name cannot be empty".to_string(),
            Self::Reserved => "`.` and `..` are not names".to_string(),
            Self::HasSeparator => "a name cannot contain `/`".to_string(),
            Self::HasNul => "a name cannot contain a null byte".to_string(),
            Self::TooLong { max } => format!("a name can be at most {max} bytes"),
            Self::HasControl { ch } => {
                let shown = if ch.is_control() {
                    // Naming the character as `U+000A` is unreadable; naming it
                    // as an escape is not.
                    format!("\\u{{{:04X}}}", *ch as u32)
                } else {
                    ch.to_string()
                };
                format!("a name cannot contain {shown}")
            }
            Self::TrailingInvisible => {
                "a name cannot end in a dot or a space — it would look shorter than it is"
                    .to_string()
            }
            Self::HasSurrogate => "a name cannot contain an unpaired surrogate".to_string(),
        }
    }

    /// `true` when the filesystem would accept this name and we are warning
    /// rather than blocking.
    #[must_use]
    pub fn is_warning(&self) -> bool {
        matches!(self, Self::TrailingInvisible)
    }
}

/// Validates a proposed name.
///
/// Returns every problem found, not just the first: a user who typed `a/b` *and*
/// a trailing space should be told both, and fixing them one dialog at a time is
/// the kind of tedium that makes people give up on a feature.
#[must_use]
pub fn validate(name: &str) -> Vec<Problem> {
    let mut out = Vec::new();

    if name.is_empty() || name.trim().is_empty() {
        out.push(Problem::Empty);
        // Nothing else is worth saying about an empty field: every other problem
        // is implied by it.
        return out;
    }
    if name == "." || name == ".." {
        out.push(Problem::Reserved);
    }
    if name.contains('/') {
        out.push(Problem::HasSeparator);
    }
    if name.contains('\0') {
        out.push(Problem::HasNul);
    }
    if name.len() > NAME_MAX {
        out.push(Problem::TooLong { max: NAME_MAX });
    }
    // Any control character. `\t` and `\n` are the ones that actually cause
    // damage — a newline in a filename becomes two rows in this very list — so
    // the whole class is rejected rather than special-casing two members.
    if let Some(ch) = name.chars().find(|c| c.is_control()) {
        out.push(Problem::HasControl { ch });
    }
    if name.ends_with('.') || name.ends_with(' ') {
        out.push(Problem::TrailingInvisible);
    }
    if name.contains('\u{FFFD}') {
        // The replacement character is what a lossy decode leaves behind, so its
        // presence means the name is not really the name being shown.
        out.push(Problem::HasSurrogate);
    }
    out
}

/// The problems that must block the rename.
///
/// Warnings are separated so a caller can render them differently: §4.8's field
/// has one error style, and treating a legal-but-odd name as an error would make
/// the field lie about the filesystem.
#[must_use]
pub fn blocking(name: &str) -> Vec<Problem> {
    validate(name)
        .into_iter()
        .filter(|p| !p.is_warning())
        .collect()
}

/// `true` when a name can be committed.
#[must_use]
pub fn is_committable(name: &str) -> bool {
    blocking(name).is_empty()
}

/// The message for the first blocking problem, or `None`.
#[must_use]
pub fn first_blocking_message(name: &str) -> Option<String> {
    blocking(name).first().map(Problem::message)
}

/// The destination a rename would produce.
#[must_use]
pub fn renamed_path(from: &Path, to: &str) -> std::path::PathBuf {
    match from.parent() {
        Some(dir) => dir.join(to),
        // A bare filename has no parent to join onto; the engine's listings always
        // give absolute paths, so this is a defensive branch rather than a live
        // one — and returning `to` alone is the only sensible answer.
        None => std::path::PathBuf::from(to),
    }
}

/// A rename's state inside the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inline {
    /// The row being renamed.
    pub row: usize,
    /// The path it had.
    pub original: std::path::PathBuf,
    /// The name it had.
    pub original_name: String,
    /// What the user has typed.
    pub draft: String,
    /// `true` once the field has focus, so plain letter keys are swallowed.
    pub focused: bool,
}

impl Inline {
    /// Starts a rename of `row`, seeded with the current name and the stem
    /// selected — the stem, because renaming is almost always an edit to the
    /// middle of a name and never a retyping of the extension.
    #[must_use]
    pub fn begin(row: usize, path: &Path) -> Self {
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        Self {
            row,
            original: path.to_path_buf(),
            draft: name.clone(),
            original_name: name,
            focused: true,
        }
    }

    /// The byte range the seed selected, as `(start, end)` char offsets.
    #[must_use]
    pub fn selection(&self) -> (usize, usize) {
        (0, stem_of(&self.original_name).chars().count())
    }

    /// The current problems, blocking and warning alike.
    #[must_use]
    pub fn problems(&self) -> Vec<Problem> {
        validate(&self.draft)
    }

    /// The first blocking message, or `None` when the field is committable.
    #[must_use]
    pub fn error(&self) -> Option<String> {
        first_blocking_message(&self.draft)
    }

    /// The first warning, which does not block.
    #[must_use]
    pub fn warning(&self) -> Option<String> {
        self.problems()
            .into_iter()
            .find(Problem::is_warning)
            .map(|p| p.message())
    }

    /// `true` when Enter would commit.
    #[must_use]
    pub fn can_commit(&self) -> bool {
        is_committable(&self.draft)
    }

    /// `true` when the draft is unchanged, so a commit is a no-op.
    #[must_use]
    pub fn is_unchanged(&self) -> bool {
        self.draft == self.original_name
    }
}

/// The part of a name before its extension.
///
/// The extension is the last dot-separated component that starts after the first
/// character, so `.bashrc` has no extension (the dot is part of the name) and
/// `archive.tar.gz` yields `archive.tar`.
#[must_use]
pub fn stem_of(name: &str) -> &str {
    let bytes = name.as_bytes();
    match name.rfind('.') {
        // A leading dot is the name, not an extension separator.
        Some(0) | None => name,
        Some(idx) if idx + 1 < bytes.len() => &name[..idx],
        // A trailing dot: the dot is part of the name, and stripping it would
        // hide the very problem `Problem::TrailingInvisible` is about.
        Some(_) => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // -- the blocking rules -------------------------------------------------

    #[test]
    fn a_normal_name_is_committable() {
        for name in [
            "a.txt",
            "My Document.pdf",
            ".bashrc",
            "file-with-many-hyphens.tar.gz",
            "日本語.txt",
            "emoji 🎉.png",
            "a",
        ] {
            assert!(
                is_committable(name),
                "{name:?} should be committable, got {:?}",
                validate(name)
            );
        }
    }

    /// A hidden file's dot is part of the name, not an extension.
    #[test]
    fn a_leading_dot_is_not_an_extension() {
        assert_eq!(stem_of(".bashrc"), ".bashrc");
        assert_eq!(stem_of(".config"), ".config");
        // ... but a dot later is.
        assert_eq!(stem_of("a.txt"), "a");
        assert_eq!(stem_of("archive.tar.gz"), "archive.tar");
    }

    #[test]
    fn empty_and_reserved_names_are_refused() {
        assert!(blocking("").contains(&Problem::Empty));
        assert!(blocking("   ").contains(&Problem::Empty));
        assert!(blocking(".").contains(&Problem::Reserved));
        assert!(blocking("..").contains(&Problem::Reserved));
    }

    #[test]
    fn a_separator_is_refused() {
        let p = blocking("a/b");
        assert!(p.contains(&Problem::HasSeparator), "{p:?}");
        // A backslash is a legal character in a Unix filename, so it is *not*
        // refused here — it is only a separator on Windows, and this validation
        // runs on the platform the user is on.
        assert!(is_committable(r"a\b"));
    }

    #[test]
    fn a_nul_byte_is_refused() {
        assert!(blocking("a\0b").contains(&Problem::HasNul));
    }

    #[test]
    fn an_over_long_name_is_refused() {
        let long = "x".repeat(NAME_MAX + 1);
        let p = blocking(&long);
        assert!(p.contains(&Problem::TooLong { max: NAME_MAX }), "{p:?}");
        // Exactly at the limit is fine.
        assert!(is_committable(&"x".repeat(NAME_MAX)));
    }

    /// The one the filesystem *accepts* and the user wishes it did not.
    #[test]
    fn a_newline_is_refused_even_though_unix_allows_it() {
        let p = blocking("a\nb");
        assert!(
            p.contains(&Problem::HasControl { ch: '\n' }),
            "a newline in a filename becomes two rows in this very list: {p:?}"
        );
        // A tab is the same class of problem.
        assert!(
            blocking("a\tb")
                .iter()
                .any(|p| matches!(p, Problem::HasControl { .. }))
        );
    }

    /// A trailing dot or space is a *warning*, not a block.
    ///
    /// On Unix it is a legal name. Refusing it would make this a worse file
    /// manager than `mv`; warning about it is the honest middle.
    #[test]
    fn a_trailing_dot_or_space_warns_but_does_not_block() {
        let p = validate("name.");
        assert!(p.contains(&Problem::TrailingInvisible), "{p:?}");
        assert!(!blocking("name.").contains(&Problem::TrailingInvisible));
        assert!(is_committable("name."), "legal on this filesystem");
        assert!(is_committable("name "));
        assert_eq!(
            Problem::TrailingInvisible.message(),
            "a name cannot end in a dot or a space — it would look shorter than it is"
        );
        assert!(Problem::TrailingInvisible.is_warning());
    }

    /// Every problem is reported at once, not one per attempt.
    #[test]
    fn all_problems_are_reported_together() {
        let p = validate("a/b\nc/");
        assert!(
            p.contains(&Problem::HasSeparator)
                && p.iter().any(|q| matches!(q, Problem::HasControl { .. })),
            "a user should not have to fix these one at a time: {p:?}"
        );
    }

    /// An empty field says only that it is empty.
    #[test]
    fn an_empty_field_says_only_that() {
        assert_eq!(validate(""), vec![Problem::Empty]);
    }

    // -- messages -----------------------------------------------------------

    /// Every message is a clause a user can act on, and none is a bare
    /// technical name.
    ///
    /// **No trailing period**, deliberately: these render under the rename field,
    /// where a period would dangle below a line of text that is not a sentence.
    /// The dialog messages in `dialog` are full sentences and do take one — the
    /// two are different registers and asserting a period here would have
    /// pushed punctuation into the wrong place to satisfy a test.
    #[test]
    fn messages_are_clauses_not_type_names() {
        for name in ["", ".", "a/b", "a\0b", "a\nb", "x.", &"y".repeat(300)] {
            for problem in validate(name) {
                let m = problem.message();
                assert!(!m.is_empty(), "{problem:?} has an empty message");
                assert!(
                    !m.contains("Problem") && !m.contains("Invalid"),
                    "a type name leaked into user copy: {m:?}"
                );
                assert!(
                    !m.ends_with('.'),
                    "a field message is a clause, not a sentence: {m:?}"
                );
            }
        }
    }

    /// A control character is named as an escape, not as an unprintable char.
    #[test]
    fn a_control_character_is_named_readably() {
        let m = Problem::HasControl { ch: '\n' }.message();
        assert!(m.contains("\\u{000A}"), "unreadable: {m:?}");
    }

    // -- the inline field ---------------------------------------------------

    #[test]
    fn starting_a_rename_seeds_the_stem() {
        let inline = Inline::begin(3, &PathBuf::from("/a/archive.tar.gz"));
        assert_eq!(inline.row, 3);
        assert_eq!(inline.original_name, "archive.tar.gz");
        assert_eq!(inline.draft, "archive.tar.gz");
        assert!(inline.focused, "the field must take focus on begin");
        assert_eq!(inline.selection(), (0, 11), "the stem is `archive.tar`");
    }

    #[test]
    fn the_destination_keeps_the_parent() {
        assert_eq!(
            renamed_path(&PathBuf::from("/a/b/c.txt"), "d.txt"),
            PathBuf::from("/a/b/d.txt")
        );
        // A bare filename has no parent to preserve.
        assert_eq!(
            renamed_path(&PathBuf::from("c.txt"), "d.txt"),
            PathBuf::from("d.txt")
        );
    }

    #[test]
    fn the_field_refuses_to_commit_an_invalid_name() {
        let mut inline = Inline::begin(0, &PathBuf::from("/a/ok.txt"));
        assert!(inline.can_commit());
        inline.draft = "bad/name".to_string();
        assert!(!inline.can_commit());
        assert!(inline.error().is_some());
        inline.draft = "ok.txt".to_string();
        assert!(inline.can_commit());
        assert!(inline.error().is_none());
    }

    #[test]
    fn a_warning_is_reported_separately_from_an_error() {
        let mut inline = Inline::begin(0, &PathBuf::from("/a/ok.txt"));
        inline.draft = "trailing.".to_string();
        assert!(
            inline.error().is_none(),
            "a warning must not read as an error"
        );
        assert!(inline.warning().is_some());
        inline.draft = "bad/name".to_string();
        assert!(inline.error().is_some());
    }

    #[test]
    fn an_unchanged_draft_is_recognised() {
        let inline = Inline::begin(0, &PathBuf::from("/a/ok.txt"));
        assert!(inline.is_unchanged());
        let mut edited = inline.clone();
        edited.draft = "ok2.txt".to_string();
        assert!(!edited.is_unchanged());
    }
}
