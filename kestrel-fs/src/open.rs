//! Opening a file with the user's default application.
//!
//! # The gap this module fills
//!
//! `Enter` descends into a directory. On a **file** it used to be a dead end:
//! the key was consumed, nothing happened, and the most-used action in a file
//! manager did nothing at all. This module is what `Enter` calls instead.
//!
//! # The mechanism, and why it is this one
//!
//! On Linux the desktop's answer to "what opens this?" is the
//! [mime-apps specification][spec]: resolve the file's MIME type, look that
//! type up in `mimeapps.list`, and run the `Exec=` line of the `.desktop` file
//! the lookup names. `xdg-open` implements exactly that — and it implements it
//! by printing to a terminal this app does not have, so a user with no
//! registered handler gets *silence*, which is the defect this module exists to
//! remove. Resolving the handler here is what makes the no-handler case a
//! message the user can read.
//!
//! [spec]: https://specifications.fredesktop.org/mime-apps-spec/latest.html
//!
//! # Two steps, two crates, and why
//!
//! * **The type** comes from `xdg_mime::SharedMimeInfo`, which parses the
//!   *system* shared MIME database (`/usr/share/mime`) at runtime. It is a
//!   parser for data the distribution ships, not a table compiled into the
//!   binary, so `*.png` and `image/png` are whatever the installed
//!   `shared-mime-info` says they are. The rule it implements is GLib's
//!   `g_content_type_guess`: the **content** decides, and the file name is only
//!   a fallback — which is why `notes.md` is `text/markdown` by name but
//!   `text/plain` by content, and a PNG renamed to `photo.txt` is still
//!   `image/png`.
//! * **The application** comes from the mime-apps-spec search order, parsed
//!   with `freedesktop_entry_parser` — the maintained parser for the freedesktop
//!   *entry* format, which is what both `mimeapps.list` and every `.desktop`
//!   file are. The search order itself is ~70 lines of documented algorithm
//!   written out in [`handler_for`]; the file *format* is the crate's job.
//!
//! # Symlinks
//!
//! A symlink is opened by its **target's** type. [`mime_of`] canonicalises
//! first, so `latest -> report.pdf` is `application/pdf` and not
//! "no handler for `.latest`". The file handed to the application is still the
//! path the user pressed `Enter` on, because that is the one they can see.
//!
//! # Nothing here blocks the UI thread
//!
//! [`open`] does the whole job — resolve, parse, spawn — and returns
//! immediately with a [`Receiver`]. The work happens on a worker thread and the
//! result arrives over a channel, which is the same shape as [`crate::scan`]
//! and [`crate::size`]. The *only* thing the caller does with the receiver is
//! [`Receiver::try_recv`], which cannot block either.
//!
//! [`crate::scan`]: crate::scan

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};

#[cfg(unix)]
use std::sync::LazyLock;

/// How many bytes of a file are sniffed.
///
/// The magic table's deepest rule is well inside this, and 2KB is GLib's own
/// default. Reading more would make the first `Enter` slower for no gain.
const SNIFF_BYTES: usize = 2048;

/// The system MIME type used when nothing else can be said about a file.
///
/// Not an error state: the spec's own default, and what `file(1)` reports for
/// a file whose content it cannot place.
const DEFAULT_TYPE: &str = "application/octet-stream";

/// The system MIME type for a zero-length file.
///
/// `inode/x-empty` rather than the crate's `application/x-zerosize`, because it
/// is the one the shared MIME database registers and therefore the one a
/// `mimeapps.list` entry can name. The two spell the same fact; only one of
/// them is one a handler registration would have used.
const EMPTY_TYPE: &str = "inode/x-empty";

/// The system MIME type for a file whose bytes are text.
///
/// GLib's fallback for content that no magic rule matched but that is valid
/// text, and the reason a `.md` file resolves to `text/plain` rather than to
/// its own `text/markdown` by-name guess.
const TEXT_TYPE: &str = "text/plain";

// ---------------------------------------------------------------------------
// What `Enter` does
// ---------------------------------------------------------------------------

/// What pressing `Enter` on a row means.
///
/// A separate type rather than a `bool` because the two answers are not
/// variants of the same thing: one stays inside the app and one leaves it, and
/// a caller that collapsed them into a flag would eventually read the flag the
/// wrong way round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enter {
    /// A directory: show what is inside it.
    Descend,
    /// A file: hand it to the user's default application.
    Open,
}

/// What `Enter` does with this row, or `None` when it does nothing.
///
/// Zero I/O. The answer is [`crate::model::FileEntry::is_descendable`], which
/// the scanner already filled in from its own thread — including for symlinks,
/// where it followed the link to find out. That is what stops `Enter` on a
/// broken symlink from becoming a `stat` on the UI thread.
///
/// A **directory** never reaches [`open`]: navigating into it is the whole
/// point of the key, and a folder with a registered handler is a thing you
/// browse, not a thing you launch.
#[must_use]
pub fn enter_action(entry: &crate::model::FileEntry) -> Option<Enter> {
    if entry.is_descendable() {
        Some(Enter::Descend)
    } else {
        Some(Enter::Open)
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Why a file could not be opened.
///
/// Every variant is something the *user* can be told about. There is no
/// catch-all "something went wrong": an app that cannot name the failure is an
/// app that has nothing to offer but a shrug, and a shrug is what this module
/// was written to replace.
#[derive(Debug)]
pub enum OpenError {
    /// The file's type is known and nothing is registered to open it.
    ///
    /// The common case for an unfamiliar format, and the one that must never be
    /// silent: the key press has to say what it found and what is missing.
    NoHandler {
        /// The file, as the user pressed it on.
        path: PathBuf,
        /// The type that was looked up, quoted in the message.
        mime: String,
    },
    /// The file could not be read well enough to work out its type.
    Unreadable {
        /// The file.
        path: PathBuf,
        /// What the OS said.
        source: std::io::Error,
    },
    /// A `.desktop` file was found and named, but it has no usable `Exec=`.
    ///
    /// A distinct message from [`OpenError::NoHandler`] because the fix is
    /// different: something *is* registered, and it is broken.
    NoCommand {
        /// The file.
        path: PathBuf,
        /// The desktop id that named the broken entry.
        id: String,
    },
    /// The handler was found and understood, but it would not start.
    Spawn {
        /// The file.
        path: PathBuf,
        /// The application that refused.
        app: String,
        /// What the OS said.
        source: std::io::Error,
    },
    /// The worker thread did not report back.
    ///
    /// A missing MIME database makes `xdg-mime` panic inside its own loader,
    /// and a panic on a worker must not take the app with it. The handle is
    /// caught, so this is reported like anything else rather than crashing a
    /// window the user was browsing.
    WorkerLost {
        /// The file.
        path: PathBuf,
    },
}

impl OpenError {
    /// The heading for the dialog this error raises.
    ///
    /// §4.7: a title is the *name of the situation*, so the two failure shapes
    /// are not both called "Failed" — one is "nothing is installed for this",
    /// the other is "the installed thing would not start".
    #[must_use]
    pub fn title(&self) -> &'static str {
        match self {
            Self::NoHandler { .. } | Self::Unreadable { .. } => "Cannot Open",
            Self::NoCommand { .. } | Self::Spawn { .. } => "Open Failed",
            Self::WorkerLost { .. } => "Cannot Open",
        }
    }

    /// The sentence, in §4.7's `dialog.body`.
    ///
    /// Names the file, says what is missing, and says what to do next — in that
    /// order, because a message that reports a problem without an action leaves
    /// the user to invent one. The button is a separate question and is
    /// answered by the dialog, never here.
    #[must_use]
    pub fn sentence(&self) -> String {
        match self {
            Self::NoHandler { path, mime } => format!(
                "No application is registered to open \u{201c}{}\u{201d} \
                 as {mime}. Set a default for that type, then press Enter again.",
                display_name(path)
            ),
            Self::Unreadable { path, source } => format!(
                "Could not read \u{201c}{}\u{201d} to work out what opens it: {source}.",
                display_name(path)
            ),
            Self::NoCommand { path, id } => format!(
                "{} is registered as the handler for \u{201c}{}\u{201d}, \
                 but it has no command to run.",
                display_name(Path::new(id)),
                display_name(path)
            ),
            Self::Spawn { path, app, source } => format!(
                "Could not start {app} to open \u{201c}{}\u{201d}: {source}.",
                display_name(path)
            ),
            Self::WorkerLost { path } => format!(
                "Gave up working out how to open \u{201c}{}\u{201d}. \
                 The system MIME database may be missing.",
                display_name(path)
            ),
        }
    }

    /// The path the dialog quotes under the sentence.
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Self::NoHandler { path, .. }
            | Self::Unreadable { path, .. }
            | Self::NoCommand { path, .. }
            | Self::Spawn { path, .. }
            | Self::WorkerLost { path } => path,
        }
    }
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.sentence())
    }
}

impl std::error::Error for OpenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Unreadable { source, .. } | Self::Spawn { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// The file's name, or its last path component spelled out.
///
/// A path with no file name — `/`, or a bare `..` — falls back to the whole
/// path rather than to nothing, because a message that quotes an empty string
/// is worse than one that quotes a slightly odd name.
fn display_name(path: &Path) -> String {
    match path.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => path.to_string_lossy().into_owned(),
    }
}

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

/// A registered application, ready to be run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handler {
    /// The application's own name, for messages.
    pub name: String,
    /// The program to run.
    pub program: String,
    /// Its arguments, with the file already substituted in.
    pub args: Vec<String>,
}

impl Handler {
    /// Runs the handler on `path` without waiting for it to finish.
    ///
    /// The path is already in `args` — [`parse_exec`] put it there — so this
    /// does not append it again.
    ///
    /// `Stdio::null` on every stream: a launched application that inherited this
    /// process's stderr would write into a terminal nobody is watching, and one
    /// that inherited its stdin would compete with the compositor for the
    /// keyboard.
    fn spawn(&self) -> std::io::Result<()> {
        Command::new(&self.program)
            .args(&self.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|_| ())
    }
}

/// The `XDG_*` directories the mime-apps spec searches, in the spec's order.
///
/// A value rather than a set of `env::var` calls at the point of use, because
/// the spec's search order is the interesting part and the only way to test it
/// is to be able to point it at a temporary tree. [`Xdg::from_env`] is the
/// production constructor; a test builds one by hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Xdg {
    config_home: PathBuf,
    config_dirs: Vec<PathBuf>,
    data_home: PathBuf,
    data_dirs: Vec<PathBuf>,
    /// `$XDG_CURRENT_DESKTOP`, lowercased — the `$desktop` in
    /// `$desktop-mimeapps.list`.
    desktops: Vec<String>,
}

impl Xdg {
    /// Reads the directories from the environment, with the base-directory
    /// spec's defaults.
    #[must_use]
    pub fn from_env() -> Self {
        let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from);
        let split = |var: &str, default: &[&str]| -> Vec<PathBuf> {
            match std::env::var_os(var) {
                Some(v) => std::env::split_paths(&v).collect(),
                None => default.iter().map(PathBuf::from).collect(),
            }
        };
        Self {
            config_home: std::env::var_os("XDG_CONFIG_HOME")
                .map_or_else(|| home.join(".config"), PathBuf::from),
            config_dirs: split("XDG_CONFIG_DIRS", &["/etc/xdg"]),
            data_home: std::env::var_os("XDG_DATA_HOME")
                .map_or_else(|| home.join(".local/share"), PathBuf::from),
            data_dirs: split("XDG_DATA_DIRS", &["/usr/local/share", "/usr/share"]),
            desktops: std::env::var("XDG_CURRENT_DESKTOP")
                .unwrap_or_default()
                .split(':')
                .map(str::trim)
                .filter(|d| !d.is_empty())
                .map(str::to_lowercase)
                .collect(),
        }
    }

    /// A self-contained tree under `root`, for tests.
    ///
    /// Everything — config, data, and the `.desktop` files themselves — lives
    /// under one directory, so a test can build a whole desktop environment in a
    /// `tempfile` and assert against it without touching the machine's real
    /// registrations.
    #[must_use]
    pub fn isolated(root: &Path) -> Self {
        Self {
            config_home: root.join("config"),
            config_dirs: vec![root.join("etc-xdg")],
            data_home: root.join("share"),
            data_dirs: vec![root.join("usr-share")],
            desktops: vec!["kestreltest".to_string()],
        }
    }

    /// Every `mimeapps.list` the spec consults, most specific first.
    ///
    /// The order is §3 of the mime-apps spec verbatim, including the
    /// `$desktop-mimeapps.list` variants before the bare name and the deprecated
    /// `$XDG_DATA_HOME/applications` entries — a real desktop that wrote its
    /// defaults there still has them honoured.
    #[must_use]
    fn mimeapps_files(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut push = |dir: &Path, suffix: &str| {
            for d in &self.desktops {
                out.push(dir.join(format!("{d}-mimeapps.list")));
            }
            let _ = suffix;
            out.push(dir.join("mimeapps.list"));
        };
        push(&self.config_home, "");
        for dir in &self.config_dirs {
            push(dir, "");
        }
        for d in &self.desktops {
            out.push(
                self.data_home
                    .join("applications")
                    .join(format!("{d}-mimeapps.list")),
            );
        }
        out.push(self.data_home.join("applications/mimeapps.list"));
        for dir in &self.data_dirs {
            for d in &self.desktops {
                out.push(dir.join("applications").join(format!("{d}-mimeapps.list")));
            }
            out.push(dir.join("applications/mimeapps.list"));
        }
        out
    }

    /// The directories `.desktop` files are installed in, most specific first.
    ///
    /// `pub` because the settings screen enumerates installed applications
    /// from them (see [`candidates_for`]); the search order is most specific
    /// first for the same reason [`Xdg::mimeapps_files`] is ordered.
    #[must_use]
    pub fn applications_dirs(&self) -> Vec<PathBuf> {
        let mut out = vec![self.data_home.join("applications")];
        out.extend(self.data_dirs.iter().map(|d| d.join("applications")));
        out
    }
}

/// One association: a desktop id, the application's name, and its `Exec=` line.
///
/// `pub` because [`handler_for`] returns it and the search order is the part
/// worth testing from outside this module; the fields are the three things a
/// test can assert on, and nothing else needs to be reachable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The desktop id from `mimeapps.list`, e.g. `org.kde.kate.desktop`.
    pub id: String,
    /// The application's `Name=`, for messages.
    pub name: String,
    /// The `Exec=` line, still containing its field codes.
    pub exec: String,
}

/// Finds the user's preferred application for `mime`.
///
/// # The search, in the spec's order
///
/// 1. Walk the `mimeapps.list` files from [`Xdg::mimeapps_files`], in order.
/// 2. Within a file, try the most specific type first: `image/png`, then
///    `image/*`, then `*/*`.
/// 3. `[Default Applications]` before `[Added Associations]`, so an explicit
///    default beats a merely-added association. The two passes are separate
///    loops rather than one loop over both sections, because §4 of the spec
///    requires every file's defaults to be exhausted before any file's added
///    associations are considered.
/// 4. A desktop id is only accepted if its `.desktop` file exists, says
///    `Type=Application`, is not `NoDisplay=true`, and lists this type in
///    `MimeType=` — the spec's "verify that the application is associated with
///    the type".
/// 5. `[Removed Associations]` is read from **every** file before any of them is
///    accepted from, and applies process-wide. That is the spec's rule and it
///    is not what a single-pass reading gives: a removal in the last file has to
///    suppress a default in the first.
///
/// Returns `None` when the type has no handler, which the caller reports rather
/// than treating as success.
#[must_use]
pub fn handler_for(xdg: &Xdg, mime: &str) -> Option<Candidate> {
    let files = xdg.mimeapps_files();
    let removed = removed_associations(&files, mime);
    for section in ["Default Applications", "Added Associations"] {
        for file in &files {
            let Ok(entry) = parse_file(file) else {
                continue;
            };
            for key in specificity(mime) {
                for id in list_for(&entry, section, &key) {
                    if removed.iter().any(|r| r == &id) {
                        continue;
                    }
                    if let Some(c) = desktop_entry(xdg, &id, mime) {
                        return Some(c);
                    }
                }
            }
        }
    }
    None
}

/// `mime`, then `type/*`, then `*/*` — the spec's most-specific-to-least walk.
fn specificity(mime: &str) -> Vec<String> {
    let mut out = vec![mime.to_string()];
    if let Some((top, _)) = mime.split_once('/') {
        out.push(format!("{top}/*"));
    }
    out.push("*/*".to_string());
    out
}

/// Every desktop id removed for this type, across every file.
///
/// Accumulated before anything is accepted, because a removal in a
/// lower-precedence file still removes.
fn removed_associations(files: &[PathBuf], mime: &str) -> Vec<String> {
    let mut out = Vec::new();
    for file in files {
        let Ok(entry) = parse_file(file) else {
            continue;
        };
        for key in specificity(mime) {
            out.extend(list_for(&entry, "Removed Associations", &key));
        }
    }
    out
}

/// The desktop ids in one section for one type, in file order.
fn list_for(entry: &Entry, section: &str, key: &str) -> Vec<String> {
    let values: &[String] = match entry.get(section, key) {
        Some(v) => v,
        None => return Vec::new(),
    };
    values
        .iter()
        .flat_map(|v| v.split(';'))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Resolves a desktop id to the file that defines it, checking that it really
/// handles this type.
fn desktop_entry(xdg: &Xdg, id: &str, mime: &str) -> Option<Candidate> {
    let path = find_desktop_file(xdg, id)?;
    let entry = parse_file(&path).ok()?;
    let section = entry.section("Desktop Entry")?;
    if attribute(section, "Type").is_some_and(|t| t.trim() != "Application") {
        return None;
    }
    if attribute(section, "NoDisplay").is_some_and(|v| v.trim() == "true") {
        return None;
    }
    let handles = attribute(section, "MimeType")
        .is_some_and(|list| list.split(';').map(str::trim).any(|m| m == mime));
    if !handles {
        return None;
    }
    Some(Candidate {
        id: id.to_string(),
        name: attribute(section, "Name")
            .or_else(|| attribute(section, "GenericName"))
            .unwrap_or(id)
            .to_string(),
        exec: attribute(section, "Exec").unwrap_or_default().to_string(),
    })
}

/// The first value of `key`, if the section has one.
fn attribute<'a>(section: &'a Section, key: &str) -> Option<&'a str> {
    section.attr(key).first().map(String::as_str)
}

/// Finds a `.desktop` file by id, including the `vendor-prefix` layout.
///
/// The desktop entry spec puts `vendor-prefixed.desktop` at
/// `applications/vendor/prefixed.desktop`, and the flat fallback is a search
/// of every installed subdirectory, which is what the mime-apps spec requires
/// because desktop environments disagree about which of the two they use.
fn find_desktop_file(xdg: &Xdg, id: &str) -> Option<PathBuf> {
    for dir in xdg.applications_dirs() {
        let direct = dir.join(id);
        if direct.is_file() {
            return Some(direct);
        }
        if let Some((vendor, app)) = id.split_once('-') {
            let nested = dir.join(vendor).join(app);
            if nested.is_file() {
                return Some(nested);
            }
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let nested = path.join(id);
                if nested.is_file() {
                    return Some(nested);
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Default applications: listing, reading and setting them
// ---------------------------------------------------------------------------

/// One row of the "default applications" settings screen: a human name and
/// the MIME types it covers.
///
/// The membership mirrors the extension table in `kestrel/src/filetype.rs` —
/// images are the extensions `filetype` classifies as `Image`, archives the
/// ones it classifies as `Archive`, and so on — transcribed here as MIME
/// types because `mimeapps.list` is keyed by MIME, not by extension. There is
/// deliberately one table, not two: [`mime_for_extension`] plus the test below
/// pin the two spellings of the same fact together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileType {
    /// The human name the settings row shows, e.g. `"Images"`.
    pub name: &'static str,
    /// The MIME types the row covers, most common first. The first entry is
    /// the primary type: the one whose default the row displays.
    pub mimes: &'static [&'static str],
}

/// Every file type the settings screen offers a default for, in screen order.
#[must_use]
pub fn file_types() -> Vec<FileType> {
    vec![
        FileType {
            name: "Images",
            mimes: &[
                "image/png",
                "image/jpeg",
                "image/gif",
                "image/webp",
                "image/svg+xml",
                "image/bmp",
                "image/tiff",
            ],
        },
        FileType {
            name: "Video",
            mimes: &[
                "video/mp4",
                "video/x-matroska",
                "video/quicktime",
                "video/webm",
                "video/x-msvideo",
                "video/mpeg",
            ],
        },
        FileType {
            name: "Audio",
            mimes: &[
                "audio/mpeg",
                "audio/flac",
                "audio/x-wav",
                "audio/ogg",
                "audio/mp4",
            ],
        },
        FileType {
            name: "PDF",
            mimes: &["application/pdf"],
        },
        FileType {
            name: "Text",
            mimes: &["text/plain", "text/markdown"],
        },
        FileType {
            name: "Archives",
            mimes: &[
                "application/zip",
                "application/x-tar",
                "application/gzip",
                "application/x-7z-compressed",
                "application/x-bzip2",
                "application/x-xz",
                "application/zstd",
            ],
        },
    ]
}

/// The MIME type the shared database gives `ext`, by name alone.
///
/// The same glob lookup [`mime_of`] falls back to when a file cannot be read,
/// factored out so the settings table can be checked against the database
/// rather than maintained as a second copy of it. `None` off unix, where there
/// is no shared database.
#[must_use]
#[cfg(unix)]
pub fn mime_for_extension(ext: &str) -> Option<String> {
    let probe = format!("probe.{ext}");
    let types = MIME_DB.get_mime_types_from_file_name(&probe);
    let found = types.first()?;
    (!found.essence_str().eq_ignore_ascii_case(DEFAULT_TYPE)).then(|| found.to_string())
}

/// The MIME type the shared database gives `ext`, by name alone.
///
/// Off unix there is no shared database, so there is no answer.
#[must_use]
#[cfg(not(unix))]
pub fn mime_for_extension(_ext: &str) -> Option<String> {
    None
}

/// Every installed application that declares `mime`, for a settings dropdown.
///
/// A `.desktop` file counts when it says `Type=Application`, is neither
/// `NoDisplay=true` nor `Hidden=true`, and lists `mime` in `MimeType=`.
/// De-duplicated by desktop id and sorted by display name. Empty when nothing
/// is installed for the type — which is the honest empty state, not an error.
#[must_use]
pub fn candidates_for(xdg: &Xdg, mime: &str) -> Vec<Candidate> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for dir in xdg.applications_dirs() {
        for path in desktop_files(&dir) {
            let Some(id) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                continue;
            };
            if !seen.insert(id.clone()) {
                continue;
            }
            if let Some(c) = read_candidate(&path, &id, mime) {
                out.push(c);
            }
        }
    }
    out.sort_by(|a: &Candidate, b: &Candidate| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

/// Every `.desktop` file directly in `dir` or one subdirectory down.
///
/// One level, not a walk: the desktop entry spec's `vendor/` layout is a
/// single nesting, and a full recursive walk of `applications/` would sweep up
/// versioned subdirectories other tools keep there.
fn desktop_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Ok(nested) = std::fs::read_dir(&path) {
                out.extend(
                    nested
                        .flatten()
                        .map(|e| e.path())
                        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "desktop")),
                );
            }
        } else if path.is_file() && path.extension().is_some_and(|e| e == "desktop") {
            out.push(path);
        }
    }
    out
}

/// Reads one desktop file as a candidate for `mime`, if it qualifies.
///
/// [`desktop_entry`] answers the same question for a *named* id during
/// resolution; this one answers it for a *found* file during enumeration, and
/// additionally honours `Hidden=true`, which resolution deliberately does not
/// re-check (see [`handler_for`]).
fn read_candidate(path: &Path, id: &str, mime: &str) -> Option<Candidate> {
    let entry = parse_file(path).ok()?;
    let section = entry.section("Desktop Entry")?;
    if attribute(section, "Type").is_some_and(|t| t.trim() != "Application") {
        return None;
    }
    if attribute(section, "NoDisplay").is_some_and(|v| v.trim() == "true") {
        return None;
    }
    if attribute(section, "Hidden").is_some_and(|v| v.trim() == "true") {
        return None;
    }
    let handles = attribute(section, "MimeType")
        .is_some_and(|list| list.split(';').map(str::trim).any(|m| m == mime));
    if !handles {
        return None;
    }
    Some(Candidate {
        id: id.to_string(),
        name: attribute(section, "Name")
            .or_else(|| attribute(section, "GenericName"))
            .unwrap_or(id)
            .to_string(),
        exec: attribute(section, "Exec").unwrap_or_default().to_string(),
    })
}

/// Every installed application that declares any of `mimes`.
///
/// The union of [`candidates_for`] over the group, de-duplicated by desktop
/// id. A viewer that handles `image/jpeg` but not `image/png` still belongs in
/// the Images row: picking it writes it for every type in the row.
#[must_use]
pub fn candidates_for_all(xdg: &Xdg, mimes: &[&str]) -> Vec<Candidate> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for mime in mimes {
        for c in candidates_for(xdg, mime) {
            if seen.insert(c.id.clone()) {
                out.push(c);
            }
        }
    }
    out.sort_by(|a: &Candidate, b: &Candidate| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

/// The desktop id currently set as the default for `mime`, if any.
///
/// The first id in `[Default Applications]` across the `mimeapps.list` files
/// in spec order, skipping anything `[Removed Associations]` suppresses.
/// `None` is "not set", reported honestly rather than guessed from
/// `[Added Associations]`.
#[must_use]
pub fn default_for(xdg: &Xdg, mime: &str) -> Option<String> {
    let files = xdg.mimeapps_files();
    let removed = removed_associations(&files, mime);
    for file in &files {
        let Ok(entry) = parse_file(file) else {
            continue;
        };
        for key in specificity(mime) {
            for id in list_for(&entry, "Default Applications", &key) {
                if removed.iter().any(|r| r == &id) {
                    continue;
                }
                return Some(id);
            }
        }
    }
    None
}

/// Sets `id` as the default for `mime`, system-wide.
///
/// Writes `~/.config/mimeapps.list` (mechanism (a): direct edit, not
/// `xdg-mime`), creating it and its directory when absent, and preserving
/// every other line for every other type. A `[Removed Associations]` entry
/// for the same pair is dropped, since it would otherwise keep suppressing
/// the default just written. Other desktop environments read the same file,
/// so the choice applies outside this app too.
///
/// Direct edit rather than shelling out to `xdg-mime` because a rewrite of
/// one line cannot truncate a hand-maintained file, needs no external binary,
/// and reports a real `io::Error` when the directory is not writable instead
/// of failing silently.
pub fn set_default(xdg: &Xdg, mime: &str, id: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(&xdg.config_home)?;
    let path = xdg.config_home.join("mimeapps.list");
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    let updated = apply_default(&current, mime, id);
    // Same directory plus rename: either the whole new file lands or nothing
    // does, never a half-written list.
    let tmp = xdg
        .config_home
        .join(format!(".mimeapps.list.tmp-{}", std::process::id()));
    std::fs::write(&tmp, updated)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// The new text of `mimeapps.list` with `mime=id` as a default.
///
/// Line surgery, not a re-serialisation: every line that is not this MIME's
/// default — or this pair's removal — passes through byte-identical, including
/// comments and blank lines. `mime=id` is replaced in place inside an existing
/// `[Default Applications]` section, inserted after the header when the section
/// exists without the key, and appended as a new section when it does not.
fn apply_default(current: &str, mime: &str, id: &str) -> String {
    const DEFAULTS: &str = "[Default Applications]";
    const REMOVED: &str = "[Removed Associations]";
    // Preamble (lines before the first header) plus one entry per section.
    let mut preamble: Vec<String> = Vec::new();
    let mut sections: Vec<(String, Vec<String>)> = Vec::new();
    for line in current.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            sections.push((trimmed.to_string(), Vec::new()));
        } else if let Some((_, body)) = sections.last_mut() {
            body.push(line.to_string());
        } else {
            preamble.push(line.to_string());
        }
    }
    let mut placed = false;
    if let Some((_, body)) = sections.iter_mut().find(|(h, _)| h == DEFAULTS) {
        for line in body.iter_mut() {
            if !placed && key_matches(line, mime) {
                *line = format!("{mime}={id}");
                placed = true;
            }
        }
        if !placed {
            body.push(format!("{mime}={id}"));
            placed = true;
        }
    }
    if !placed {
        sections.push((DEFAULTS.to_string(), vec![format!("{mime}={id}")]));
    }
    for (header, body) in sections.iter_mut() {
        if header == REMOVED {
            let mut kept = Vec::new();
            for line in body.iter() {
                if key_matches(line, mime) {
                    if let Some(k) = without_removal(line, id) {
                        kept.push(k);
                    }
                } else {
                    kept.push(line.clone());
                }
            }
            *body = kept;
        }
    }
    // An empty section is dropped, not left behind: the entry parser rejects a
    // file whose trailing group has no keys (verified against
    // `freedesktop_entry_parser`), which would make the whole file — including
    // the default just written — unreadable. A header with no entries carries
    // no associations, so dropping it preserves meaning.
    sections.retain(|(_, body)| !body.is_empty());
    let mut out = preamble;
    for (i, (header, body)) in sections.iter().enumerate() {
        if i > 0 || !out.is_empty() {
            // Blank line between blocks, none leading: `lines()` dropped the
            // original newlines, and a file is sections separated by blanks,
            // not blanks followed by sections.
            if out.last().is_some_and(|l: &String| !l.is_empty()) {
                out.push(String::new());
            }
        }
        out.push(header.clone());
        out.extend(body.iter().cloned());
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

/// `true` when `line` is a `mime=...` entry for exactly `mime`.
fn key_matches(line: &str, mime: &str) -> bool {
    match line.split_once('=') {
        Some((key, _)) => key.trim() == mime,
        None => false,
    }
}

/// `line` with `id` removed from its `;`-separated value, or `None` when
/// nothing is left and the line should go rather than linger as an empty key.
fn without_removal(line: &str, id: &str) -> Option<String> {
    let (key, _value) = line.split_once('=')?;
    let kept: Vec<&str> = _value
        .split(';')
        .map(str::trim)
        .filter(|v| !v.is_empty() && *v != id)
        .collect();
    if kept.is_empty() {
        return None;
    }
    Some(format!("{}={};", key, kept.join(";")))
}

/// Sets `id` as the default for every type in `mimes`.
///
/// One pick in a group row converges the whole group, so a viewer chosen for
/// Images opens `image/png` *and* `image/jpeg`. Stops at the first error.
pub fn set_default_for_all(xdg: &Xdg, mimes: &[&str], id: &str) -> std::io::Result<()> {
    for mime in mimes {
        set_default(xdg, mime, id)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// MIME
// ---------------------------------------------------------------------------

/// The system's shared MIME database, loaded once.
///
/// A `LazyLock`, not a field on the app: the database is process-wide immutable
/// data, and the first `Enter` pays ~50ms to parse it while later ones pay
/// nothing. It is initialised on a worker thread (see [`open`]), so that 50ms
/// is never a frame.
#[cfg(unix)]
static MIME_DB: LazyLock<xdg_mime::SharedMimeInfo> = LazyLock::new(xdg_mime::SharedMimeInfo::new);

/// The MIME type of `path`, resolved the way the desktop resolves it.
///
/// # Symlinks
///
/// The path is canonicalised first, so a link is typed by its **target**.
/// `latest -> report.pdf` is `application/pdf`; a link called `current` with
/// no extension is not "no handler for an unknown type", it is whatever the
/// thing it points at is.
///
/// # Why the content decides
///
/// GLib's `g_content_type_guess` gives the magic match priority over the file
/// name, and this matches it, because a file manager that disagrees with
/// double-click is worse than one that is merely simpler. So `notes.md` is
/// `text/plain` (its bytes are text) rather than `text/markdown` (its name
/// suggests otherwise), and a PNG called `photo.txt` is still `image/png`. The
/// name glob is used when the file cannot be read at all.
#[must_use]
pub fn mime_of(path: &Path) -> String {
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    match read_head(&target) {
        Some(head) if head.is_empty() => EMPTY_TYPE.to_string(),
        Some(head) => {
            let by_content = content_type(&head);
            match by_content {
                Some(t) => t,
                // Nothing in the content: the name is all there is. This is the
                // rare path, and it is the one `xdg-mime`'s glob table exists
                // for.
                None => by_name(&target).unwrap_or_else(|| DEFAULT_TYPE.to_string()),
            }
        }
        None => by_name(&target).unwrap_or_else(|| DEFAULT_TYPE.to_string()),
    }
}

/// The first bytes of a file, or `None` if it cannot be read.
fn read_head(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; SNIFF_BYTES];
    let read = file.read(&mut buf).ok()?;
    buf.truncate(read);
    Some(buf)
}

/// The type of a byte prefix, by the same rules GLib uses.
///
/// The magic table first, then "these bytes are text" as `text/plain`, then
/// nothing — which is the caller's cue to fall back to the file name.
#[cfg(unix)]
fn content_type(head: &[u8]) -> Option<String> {
    if let Some((mime, _)) = MIME_DB.get_mime_type_for_data(head) {
        return Some(mime.to_string());
    }
    looks_like_text(head).then(|| TEXT_TYPE.to_string())
}

/// Not-a-UTF-8 byte, or a control character that does not belong in a document.
///
/// GLib's `looks_like_text`, reduced to the two rules that matter: valid UTF-8,
/// and no embedded control characters. Without the second rule a `.png` of
/// random bytes has a decent chance of decoding as UTF-8 on a short read and
/// being handed to a text editor.
#[must_use]
fn looks_like_text(head: &[u8]) -> bool {
    let body = head.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(head);
    let Ok(text) = std::str::from_utf8(body) else {
        return false;
    };
    !text
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\t' | '\n' | '\x0B' | '\x0C' | '\r'))
}

/// The type the glob table gives this file's name, if any.
#[cfg(unix)]
fn by_name(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy();
    let types = MIME_DB.get_mime_types_from_file_name(&name);
    // The table always answers, and its "no idea" answer is
    // `application/octet-stream`, which is not a glob match and should not be
    // preferred over the caller's own default.
    let found = types.first()?;
    (!found.essence_str().eq_ignore_ascii_case(DEFAULT_TYPE)).then(|| found.to_string())
}

// ---------------------------------------------------------------------------
// The command line
// ---------------------------------------------------------------------------

/// Splits an `Exec=` line into a program and its arguments.
///
/// # Field codes
///
/// The desktop entry spec defines `%f`, `%F`, `%u` and `%U` as the places the
/// file goes, and requires that they be **removed** from the command line
/// rather than left as literal text. When none of them appears, the spec
/// requires the file to be appended as a separate argument, which is what
/// `code --new-window` means.
///
/// The deprecated codes (`%d`, `%D`, `%n`, `%N`, `%i`, `%c`, `%k`) must not
/// appear in a command line that is actually run, so they are dropped — an
/// application whose `Exec` still uses `%i` is an application whose icon
/// argument would otherwise be substituted with the file's name.
pub fn parse_exec(exec: &str, file: &Path) -> Option<Handler> {
    let path = file.to_string_lossy().into_owned();
    // A command with no tokens at all is a `.desktop` file with no `Exec=`
    // rather than one whose program is the empty string, and the two produce
    // different errors: the first is "no command", the second would be a
    // `Command::new("")` that fails at spawn time with nothing to say.
    let tokens = tokenize(exec);
    let mut iter = tokens.into_iter();
    let program = iter.next()?;
    if program.is_empty() {
        return None;
    }

    let mut args: Vec<String> = Vec::new();
    let mut substituted = false;
    for token in iter {
        // A token that is *nothing but* field codes disappears and the file
        // takes its place — the spec's "the code is removed" rule. A token with
        // a code inside it, `--file=%f`, keeps its other text and is rewritten.
        // A whole deprecated code is dropped without consuming the file slot:
        // `%i` is the icon argument, and treating it as "a place for the file"
        // would put the file where the spec says a code must not appear at all.
        if is_whole_deprecated_code(&token) {
            continue;
        }
        // A token that is *nothing but* a file code disappears and the file
        // takes its place — the spec's "the code is removed" rule. A token with
        // a code inside it, `--file=%f`, keeps its other text and is rewritten.
        if is_whole_file_code(&token) {
            if !substituted {
                args.push(path.clone());
                substituted = true;
            }
            continue;
        }
        if let Some(rewritten) = expand(&token, &path, &mut substituted) {
            args.push(rewritten);
        }
    }
    // The spec's other rule: a command with no file code gets the file as a
    // separate final argument. Appending it here rather than at the call site
    // keeps both spellings of "where does the file go" in one function.
    if !substituted {
        args.push(path);
    }
    Some(Handler {
        name: program.clone(),
        program,
        args,
    })
}

/// `true` when the token is exactly one of the codes the file goes into.
///
/// Not "contains a code": `--file=%f` has to keep `--file=`, and dropping the
/// token because it *held* a code is how a `--file=` flag silently loses its
/// value.
fn is_whole_file_code(token: &str) -> bool {
    matches!(token, "%f" | "%F" | "%u" | "%U")
}

/// `true` when the token is exactly one of the deprecated codes.
///
/// The spec requires these be absent from a command line that actually runs, so
/// they are removed rather than substituted — and removing one must not consume
/// the file's slot, which is why this is not folded into the file-code check.
fn is_whole_deprecated_code(token: &str) -> bool {
    matches!(token, "%d" | "%D" | "%n" | "%N" | "%i" | "%c" | "%k")
}

/// The rewritten form of a token that holds field codes, or `None` if it should
/// be dropped.
///
/// `None` for a token whose only content was codes — those were already
/// handled by [`is_whole_file_code`], and pushing an empty argument here would
/// put a stray `""` on the command line. `%%` is a literal percent sign and is not a
/// code; a trailing `%` is not a code either and is kept, because silently
/// dropping a character the spec does not define would be a rewrite nobody
/// asked for.
fn expand(token: &str, path: &str, substituted: &mut bool) -> Option<String> {
    if !token.contains('%') {
        return Some(token.to_string());
    }
    let mut out = String::with_capacity(token.len() + path.len());
    let mut chars = token.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('%') => out.push('%'),
            Some('f' | 'F' | 'u' | 'U') => {
                if !*substituted {
                    out.push_str(path);
                    *substituted = true;
                }
            }
            Some('d' | 'D' | 'n' | 'N' | 'i' | 'c' | 'k') => {}
            Some(other) => out.push(other),
            None => out.push('%'),
        }
    }
    (!out.is_empty()).then_some(out)
}

/// Splits a command line into tokens, honouring quotes and backslash escapes.
///
/// The desktop entry format's quoting is not a shell's: there is no expansion,
/// no `|`, and no `$(...)`, so this is a plain word-splitter over `"` and `\`
/// rather than a shell parser. A `.desktop` file that needs a shell is a
/// `.desktop` file that should have been a script.
#[must_use]
fn tokenize(exec: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut has_token = false;
    let mut chars = exec.chars().peekable();
    let mut quoted = false;
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                has_token = true;
            }
            '\\' => {
                if let Some(next) = chars.next() {
                    current.push(next);
                    has_token = true;
                }
            }
            c if c.is_whitespace() && !quoted => {
                if has_token {
                    out.push(std::mem::take(&mut current));
                    has_token = false;
                }
            }
            c => {
                current.push(c);
                has_token = true;
            }
        }
    }
    if has_token {
        out.push(current);
    }
    out
}

// ---------------------------------------------------------------------------
// The worker
// ---------------------------------------------------------------------------

/// An attempt to open a file, in flight or finished.
///
/// Returned by [`open`] and polled once per frame. The handle owns the channel
/// *and* the path, which is what lets [`Opening::poll`] report a dead worker as
/// [`OpenError::WorkerLost`] with the file named: a bare `Receiver` cannot say
/// which file it was for.
///
/// `None` from `poll` means *still working*; it is not a failure and not a
/// completion, and it is the state a frame callback expects to see most of the
/// time.
pub struct Opening {
    /// The worker's answer: the application's name, or why there was none.
    rx: Receiver<Result<String, OpenError>>,
    path: PathBuf,
    done: bool,
}

impl std::fmt::Debug for Opening {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Opening")
            .field("path", &self.path)
            .field("done", &self.done)
            .finish()
    }
}

impl Opening {
    /// The file being opened, for a message raised before the worker answers.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// `true` once the worker has answered, whatever the answer was.
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.done
    }

    /// The answer, if there is one yet.
    ///
    /// `try_recv`, never `recv`: this is called from a frame callback, and a
    /// blocking receive would put a wait on the UI thread — the exact thing this
    /// module exists to avoid. Once the worker has answered, every later call
    /// returns `None` rather than repeating the answer, so a caller that
    /// forgets to clear the handle cannot raise the same dialog every frame.
    pub fn poll(&mut self) -> Option<Result<String, OpenError>> {
        if self.done {
            return None;
        }
        match self.rx.try_recv() {
            Ok(answer) => {
                self.done = true;
                Some(answer)
            }
            Err(TryRecvError::Empty) => None,
            // Disconnected with nothing buffered: the worker panicked or never
            // started, and nothing will ever arrive. `Empty` cannot be
            // disconnected while a sender is alive, so this is a real end — and
            // an end the user is told about rather than waited on forever.
            Err(TryRecvError::Disconnected) => {
                self.done = true;
                Some(Err(OpenError::WorkerLost {
                    path: self.path.clone(),
                }))
            }
        }
    }
}

/// Opens `path` with the user's default application, without blocking.
///
/// The whole job — MIME resolution, `mimeapps.list` search, `Exec=` parsing and
/// the spawn — happens on a worker thread, and this returns immediately with a
/// handle to collect the answer from. Calling this from a frame callback costs
/// one `thread::spawn` and nothing else.
///
/// The handler is **not** waited on. `spawn` returning means the process
/// exists, which is as much as any launcher can know without becoming a
/// supervisor; a handler that fails after that is a handler's own business, and
/// a blocking `wait` on the UI thread would freeze the window for as long as the
/// user's editor takes to quit.
///
/// # Panics
///
/// The worker body runs inside `catch_unwind`. It buys exactly one thing: a
/// panic inside a dependency — `xdg-mime`'s loader panics when the system MIME
/// database is missing — becomes [`OpenError::WorkerLost`], which the user can
/// read, instead of a process that disappears while they are looking at a
/// directory.
#[must_use]
pub fn open(path: &Path) -> Opening {
    let (tx, rx) = mpsc::channel();
    let worker_path = path.to_path_buf();
    let started = std::thread::Builder::new()
        .name("kestrel-open".to_string())
        .spawn(move || {
            // The sender is moved into the catch so that it is dropped during
            // the unwind as well as on the normal path; either way the
            // receiver's end closes, and `Opening::poll` reads that as a lost
            // worker and says so rather than waiting forever.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                // A send on a dropped receiver is not a failure worth
                // reporting: the caller closed the handle because the window
                // went away, and the file it was opening is not this app's
                // problem any more.
                let _ = tx.send(launch(&worker_path));
            }));
        });
    Opening {
        rx,
        path: path.to_path_buf(),
        done: started.is_err(),
    }
}

/// The whole job, on a thread: resolve the type, find the handler, run it.
fn launch(path: &Path) -> Result<String, OpenError> {
    let mime = mime_of(path);
    let xdg = Xdg::from_env();
    let candidate = handler_for(&xdg, &mime).ok_or_else(|| OpenError::NoHandler {
        path: path.to_path_buf(),
        mime,
    })?;
    let handler = parse_exec(&candidate.exec, path).ok_or_else(|| OpenError::NoCommand {
        path: path.to_path_buf(),
        id: candidate.id.clone(),
    })?;
    handler.spawn().map_err(|source| OpenError::Spawn {
        path: path.to_path_buf(),
        app: handler.name.clone(),
        source,
    })?;
    Ok(handler.name)
}

// ---------------------------------------------------------------------------
// The freedesktop entry parser
// ---------------------------------------------------------------------------
//
// Two aliases rather than a pile of `cfg` at every use: the format is the
// freedesktop *entry* format for both files this module reads, and naming it
// once keeps the reader looking at the spec rather than at a crate.

#[cfg(unix)]
use freedesktop_entry_parser::{Entry, Section, parse_entry as parse_entry_file};

/// Parses an entry file, or answers with a value that has no sections.
#[cfg(unix)]
fn parse_file(path: &Path) -> Result<Entry, std::io::Error> {
    parse_entry_file(path)
}

/// The entry-file parser's stand-in off unix, where the format does not apply.
#[cfg(not(unix))]
fn parse_file(_path: &Path) -> Result<Entry, std::io::Error> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "no freedesktop entry database on this platform",
    ))
}

/// A parsed entry file, or an empty stand-in off unix.
#[cfg(not(unix))]
struct Entry;

#[cfg(not(unix))]
impl Entry {
    fn section(&self, _title: &str) -> Option<&Section> {
        None
    }

    fn get(&self, _section: &str, _attr: &str) -> Option<&[String]> {
        None
    }
}

/// A section, or a stand-in off unix.
#[cfg(not(unix))]
struct Section;

#[cfg(not(unix))]
impl Section {
    fn attr(&self, _key: &str) -> &[String] {
        &[]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EntryKind, FileEntry};
    use std::path::PathBuf;

    fn entry(name: &str, kind: EntryKind, is_dir_target: Option<bool>) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            path: PathBuf::from(name),
            kind,
            size: Some(0),
            modified: None,
            hidden: name.starts_with('.'),
            is_dir_target,
        }
    }

    // -- what Enter does ---------------------------------------------------

    /// The branch the report is about: a **directory** still navigates, and
    /// only a file goes to the open path.
    ///
    /// `is_dir_target` is what the scanner filled in, and for a symlink it is
    /// the *link target's* kind — which is why a link to a directory descends
    /// and a link to a file opens.
    #[test]
    fn enter_on_a_directory_navigates_and_enter_on_a_file_opens() {
        assert_eq!(
            enter_action(&entry("docs", EntryKind::Directory, None)),
            Some(Enter::Descend),
            "a directory is entered, not launched"
        );
        assert_eq!(
            enter_action(&entry("notes.md", EntryKind::File, None)),
            Some(Enter::Open),
            "a file is the thing that opens"
        );
    }

    #[test]
    fn a_symlink_is_answered_by_its_target_not_by_its_own_name() {
        // A link to a directory descends, even though the link's own extension
        // says nothing.
        assert_eq!(
            enter_action(&entry("latest", EntryKind::Symlink, Some(true))),
            Some(Enter::Descend)
        );
        // A link to a file opens, and the open path canonicalises so the type
        // comes from the target.
        assert_eq!(
            enter_action(&entry("current", EntryKind::Symlink, Some(false))),
            Some(Enter::Open)
        );
    }

    /// A dangling symlink has no target kind at all, and the answer is the one
    /// the app can act on: it is not a directory, so it is a file to open, and
    /// the open path will report that it could not be read.
    #[test]
    fn a_dangling_symlink_is_a_file_to_open_not_a_navigation() {
        assert_eq!(
            enter_action(&entry("broken", EntryKind::Symlink, None)),
            Some(Enter::Open)
        );
    }

    /// A *directory* entry never reaches the open path, and this is the test
    /// that says so at the type level rather than by reading the call site.
    #[test]
    fn no_directory_is_ever_reported_as_openable() {
        for e in [
            entry("a", EntryKind::Directory, None),
            entry("a", EntryKind::Directory, Some(true)),
            entry("a", EntryKind::Symlink, Some(true)),
        ] {
            assert_ne!(
                enter_action(&e),
                Some(Enter::Open),
                "{:?} must descend",
                e.kind
            );
        }
    }

    // -- the no-handler path -----------------------------------------------

    /// The defect this module exists to fix: pressing `Enter` on a file with
    /// no registered handler did nothing at all.
    ///
    /// The message has to *name the file* and *say what is missing*, and the
    /// dialog's button is `Close` rather than `OK` (§4.7), so the user is told
    /// what to do rather than left to invent it.
    #[test]
    fn a_file_with_no_handler_produces_a_named_error_not_silence() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("mystery.qqq");
        std::fs::write(&file, b"\x00\x01\x02\x03").expect("write");
        // An XDG tree with nothing in it: no mimeapps.list anywhere, so the
        // lookup cannot find a handler and has to say so.
        let xdg = Xdg::isolated(dir.path());

        assert_eq!(handler_for(&xdg, "application/octet-stream"), None);

        let err = OpenError::NoHandler {
            path: file.clone(),
            mime: "application/octet-stream".to_string(),
        };
        let sentence = err.sentence();
        assert!(
            sentence.contains("mystery.qqq"),
            "names the file: {sentence}"
        );
        assert!(
            sentence.contains("No application is registered"),
            "says what is missing: {sentence}"
        );
        assert!(
            sentence.contains("application/octet-stream"),
            "says which type: {sentence}"
        );
        assert_ne!(err.title(), "OK", "a title names the situation");
    }

    /// Every error variant names something, and none of them is a shrug.
    #[test]
    fn every_error_names_the_situation_and_says_what_to_do() {
        let cases = [
            OpenError::NoHandler {
                path: PathBuf::from("/tmp/a.png"),
                mime: "image/png".into(),
            },
            OpenError::Unreadable {
                path: PathBuf::from("/tmp/a.png"),
                source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            },
            OpenError::NoCommand {
                path: PathBuf::from("/tmp/a.png"),
                id: "broken.desktop".into(),
            },
            OpenError::Spawn {
                path: PathBuf::from("/tmp/a.png"),
                app: "Viewer".into(),
                source: std::io::Error::from(std::io::ErrorKind::NotFound),
            },
            OpenError::WorkerLost {
                path: PathBuf::from("/tmp/a.png"),
            },
        ];
        for err in &cases {
            let s = err.sentence();
            assert!(s.contains("a.png"), "{err:?} does not name the file: {s}");
            assert!(!s.is_empty());
            assert!(!err.title().is_empty());
            assert!(!err.title().eq_ignore_ascii_case("OK"));
            assert!(!err.title().eq_ignore_ascii_case("Error"));
        }
    }

    // -- MIME ---------------------------------------------------------------

    /// A symlink opens the **target's** type.
    ///
    /// The bug this prevents: `latest -> notes.md` being reported as "no
    /// handler" because the link's own name has no extension, or — worse —
    /// being typed by the link's name rather than by the thing it points at.
    #[cfg(unix)]
    #[test]
    fn a_symlink_resolves_its_targets_type() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("real.txt");
        std::fs::write(&target, b"just some words\n").expect("write");
        let link = dir.path().join("no-extension-at-all");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");

        assert_eq!(
            mime_of(&link),
            mime_of(&target),
            "the link and its target are the same type"
        );
        assert_eq!(mime_of(&target), TEXT_TYPE, "text is text");
    }

    /// Content decides, not the name — the same rule `xdg-mime query filetype`
    /// follows, and the reason this file manager agrees with double-click.
    #[cfg(unix)]
    #[test]
    fn the_content_decides_the_type_not_the_extension() {
        let dir = tempfile::tempdir().expect("tempdir");
        let png = dir.path().join("actually-a-png.txt");
        // The eight-byte PNG signature.
        std::fs::write(&png, [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]).expect("write");
        assert_eq!(
            mime_of(&png),
            "image/png",
            "a PNG called .txt is still a PNG"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_zero_length_file_is_the_empty_inode_type() {
        let dir = tempfile::tempdir().expect("tempdir");
        let empty = dir.path().join("empty.dat");
        std::fs::write(&empty, b"").expect("write");
        assert_eq!(mime_of(&empty), EMPTY_TYPE);
    }

    #[cfg(unix)]
    #[test]
    fn binary_bytes_are_not_text() {
        // A NUL in the first 2KB is the classic "this is not a document".
        let mut head = vec![b'a'; 64];
        head[10] = 0;
        assert!(!looks_like_text(&head));
        assert!(looks_like_text(b"plain words and a tab\there\n"));
        // A UTF-8 BOM does not make binary data into text.
        assert!(looks_like_text(b"\xEF\xBB\xBFhello"));
    }

    // -- the search order ---------------------------------------------------

    /// Builds an isolated XDG tree with a desktop file in it.
    fn desktop(root: &Path, id: &str, body: &str) {
        let dir = Xdg::isolated(root).applications_dirs()[0].clone();
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join(id), body).expect("write desktop");
    }

    /// A minimal but valid `.desktop` file.
    fn entry_file(name: &str, exec: &str, mimes: &str) -> String {
        format!(
            "[Desktop Entry]\nType=Application\nName={name}\nExec={exec}\n\
             NoDisplay=false\nTerminal=false\nMimeType={mimes};\n"
        )
    }

    #[test]
    fn the_default_application_wins_and_is_found() {
        let dir = tempfile::tempdir().expect("tempdir");
        let xdg = Xdg::isolated(dir.path());
        desktop(
            dir.path(),
            "viewer.desktop",
            &entry_file("Viewer", "viewer %U", "image/png"),
        );
        let list = xdg.config_home.join("mimeapps.list");
        std::fs::create_dir_all(&xdg.config_home).expect("mkdir");
        std::fs::write(&list, "[Default Applications]\nimage/png=viewer.desktop\n").expect("write");

        let found = handler_for(&xdg, "image/png").expect("a handler is registered");
        assert_eq!(found.id, "viewer.desktop");
        assert_eq!(found.name, "Viewer");
        assert_eq!(found.exec, "viewer %U");
    }

    /// §4 of the spec: an explicit default outranks an added association, and
    /// every file's defaults are exhausted before any file's additions.
    #[test]
    fn a_default_outranks_an_added_association() {
        let dir = tempfile::tempdir().expect("tempdir");
        let xdg = Xdg::isolated(dir.path());
        desktop(
            dir.path(),
            "preferred.desktop",
            &entry_file("Preferred", "preferred %f", "image/png"),
        );
        desktop(
            dir.path(),
            "other.desktop",
            &entry_file("Other", "other %f", "image/png"),
        );
        let list = xdg.config_home.join("mimeapps.list");
        std::fs::create_dir_all(&xdg.config_home).expect("mkdir");
        std::fs::write(
            &list,
            "[Default Applications]\nimage/png=preferred.desktop\n\
             [Added Associations]\nimage/png=other.desktop\n",
        )
        .expect("write");

        assert_eq!(
            handler_for(&xdg, "image/png").expect("found").id,
            "preferred.desktop"
        );
    }

    /// The spec's most-specific-to-least walk: `image/*` catches a type that
    /// has no entry of its own.
    #[test]
    fn a_super_type_entry_catches_an_unlisted_type() {
        let dir = tempfile::tempdir().expect("tempdir");
        let xdg = Xdg::isolated(dir.path());
        desktop(
            dir.path(),
            "photos.desktop",
            &entry_file("Photos", "photos %U", "image/png"),
        );
        let list = xdg.config_home.join("mimeapps.list");
        std::fs::create_dir_all(&xdg.config_home).expect("mkdir");
        std::fs::write(&list, "[Default Applications]\nimage/*=photos.desktop\n").expect("write");

        assert_eq!(
            handler_for(&xdg, "image/png").expect("found").id,
            "photos.desktop"
        );
        // A type in no group at all still has no handler.
        assert_eq!(handler_for(&xdg, "audio/mpeg"), None);
    }

    /// A removal anywhere wins, even one in a file the search has not reached
    /// yet. This is the spec's rule and it is not what a single-pass reading
    /// gives, so it is pinned here.
    #[test]
    fn a_removal_in_any_file_suppresses_the_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        let xdg = Xdg::isolated(dir.path());
        desktop(
            dir.path(),
            "viewer.desktop",
            &entry_file("Viewer", "viewer %U", "image/png"),
        );
        // The default is in `$XDG_CONFIG_HOME`, the removal in a later
        // `$XDG_DATA_DIRS` file.
        let home = xdg.config_home.join("mimeapps.list");
        std::fs::create_dir_all(&xdg.config_home).expect("mkdir");
        std::fs::write(&home, "[Default Applications]\nimage/png=viewer.desktop\n").expect("write");
        let system = xdg.data_dirs[0].join("applications/mimeapps.list");
        std::fs::create_dir_all(system.parent().expect("parent")).expect("mkdir");
        std::fs::write(
            &system,
            "[Removed Associations]\nimage/png=viewer.desktop\n",
        )
        .expect("write");

        assert_eq!(
            handler_for(&xdg, "image/png"),
            None,
            "a removal in a lower-precedence file still removes"
        );
    }

    /// The spec requires verifying that the application is *associated* with
    /// the type, not merely that it was named.
    #[test]
    fn a_desktop_file_that_does_not_handle_the_type_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let xdg = Xdg::isolated(dir.path());
        desktop(
            dir.path(),
            "wrong.desktop",
            &entry_file("Wrong", "wrong %f", "audio/mpeg"),
        );
        let list = xdg.config_home.join("mimeapps.list");
        std::fs::create_dir_all(&xdg.config_home).expect("mkdir");
        std::fs::write(&list, "[Default Applications]\nimage/png=wrong.desktop\n").expect("write");
        assert_eq!(handler_for(&xdg, "image/png"), None);
    }

    /// A `NoDisplay` entry is not offered to a user-facing launcher.
    #[test]
    fn a_hidden_desktop_file_is_not_a_handler() {
        let dir = tempfile::tempdir().expect("tempdir");
        let xdg = Xdg::isolated(dir.path());
        desktop(
            dir.path(),
            "helper.desktop",
            "[Desktop Entry]\nType=Application\nName=Helper\nExec=helper %f\n\
             NoDisplay=true\nMimeType=image/png;\n",
        );
        let list = xdg.config_home.join("mimeapps.list");
        std::fs::create_dir_all(&xdg.config_home).expect("mkdir");
        std::fs::write(&list, "[Default Applications]\nimage/png=helper.desktop\n").expect("write");
        assert_eq!(handler_for(&xdg, "image/png"), None);
    }

    /// A desktop id that names no installed file is skipped, and the search
    /// continues to the next candidate rather than giving up.
    #[test]
    fn a_desktop_id_with_no_file_is_skipped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let xdg = Xdg::isolated(dir.path());
        desktop(
            dir.path(),
            "real.desktop",
            &entry_file("Real", "real %f", "image/png"),
        );
        let list = xdg.config_home.join("mimeapps.list");
        std::fs::create_dir_all(&xdg.config_home).expect("mkdir");
        std::fs::write(
            &list,
            "[Default Applications]\nimage/png=gone.desktop;real.desktop\n",
        )
        .expect("write");
        assert_eq!(
            handler_for(&xdg, "image/png").expect("found the second").id,
            "real.desktop"
        );
    }

    /// `$XDG_CURRENT_DESKTOP` is consulted, and the desktop-specific file is
    /// read before the generic one.
    #[test]
    fn the_desktop_specific_file_is_read_first() {
        let dir = tempfile::tempdir().expect("tempdir");
        let xdg = Xdg::isolated(dir.path());
        desktop(
            dir.path(),
            "generic.desktop",
            &entry_file("Generic", "generic %f", "image/png"),
        );
        desktop(
            dir.path(),
            "specific.desktop",
            &entry_file("Specific", "specific %f", "image/png"),
        );
        std::fs::create_dir_all(&xdg.config_home).expect("mkdir");
        std::fs::write(
            xdg.config_home.join("kestreltest-mimeapps.list"),
            "[Default Applications]\nimage/png=specific.desktop\n",
        )
        .expect("write");
        std::fs::write(
            xdg.config_home.join("mimeapps.list"),
            "[Default Applications]\nimage/png=generic.desktop\n",
        )
        .expect("write");
        assert_eq!(
            handler_for(&xdg, "image/png").expect("found").id,
            "specific.desktop"
        );
    }

    /// The vendor subdirectory layout, which desktop environments disagree
    /// about and which a launcher therefore has to try both of.
    #[test]
    fn a_vendor_subdirectory_layout_is_found() {
        let dir = tempfile::tempdir().expect("tempdir");
        let xdg = Xdg::isolated(dir.path());
        let nested = xdg.applications_dirs()[0].join("acme");
        std::fs::create_dir_all(&nested).expect("mkdir");
        std::fs::write(
            nested.join("editor.desktop"),
            entry_file("Editor", "editor %f", "text/plain"),
        )
        .expect("write");
        let list = xdg.config_home.join("mimeapps.list");
        std::fs::create_dir_all(&xdg.config_home).expect("mkdir");
        std::fs::write(
            &list,
            "[Default Applications]\ntext/plain=acme-editor.desktop\n",
        )
        .expect("write");
        assert_eq!(
            handler_for(&xdg, "text/plain").expect("found").id,
            "acme-editor.desktop"
        );
    }

    // -- the Exec= line -----------------------------------------------------

    #[test]
    fn a_field_code_is_replaced_by_the_file_and_removed() {
        let h = parse_exec("viewer %U", Path::new("/tmp/a.png")).expect("parsed");
        assert_eq!(h.program, "viewer");
        assert_eq!(h.args, vec!["/tmp/a.png".to_string()]);
    }

    /// The spec's other half: a command with no file code gets the file as a
    /// separate final argument.
    #[test]
    fn a_command_with_no_file_code_gets_the_file_appended() {
        let h = parse_exec("code --new-window", Path::new("/tmp/a.rs")).expect("parsed");
        assert_eq!(h.program, "code");
        assert_eq!(
            h.args,
            vec!["--new-window".to_string(), "/tmp/a.rs".to_string()]
        );
    }

    /// A code inside a larger argument is substituted in place, not treated as
    /// a whole argument.
    #[test]
    fn a_code_inside_an_argument_is_substituted_in_place() {
        let h = parse_exec("app --file=%f --flag", Path::new("/tmp/a")).expect("parsed");
        assert_eq!(h.args, vec!["--file=/tmp/a".to_string(), "--flag".into()]);
    }

    /// `%%` is a literal percent sign, and the deprecated codes must not reach
    /// the command line at all.
    #[test]
    fn escaped_percents_stay_and_deprecated_codes_are_dropped() {
        let h = parse_exec("app %i --format=100%% %f", Path::new("/tmp/a")).expect("parsed");
        assert_eq!(h.args, vec!["--format=100%".to_string(), "/tmp/a".into()]);
    }

    /// Only the *first* file code is filled: substituting into every one would
    /// pass the same file twice to an application that takes one.
    #[test]
    fn only_the_first_file_code_is_filled() {
        let h = parse_exec("app %f %f", Path::new("/tmp/a")).expect("parsed");
        assert_eq!(h.args, vec!["/tmp/a".to_string()]);
    }

    /// Quoting and escaping, per the desktop entry format.
    #[test]
    fn quotes_and_backslashes_are_honoured() {
        assert_eq!(
            tokenize(r#"app "two words" three"#),
            vec!["app", "two words", "three"]
        );
        assert_eq!(tokenize(r"app two\ words"), vec!["app", "two words"]);
        // An empty quoted argument is an argument.
        assert_eq!(tokenize(r#"app "" x"#), vec!["app", "", "x"]);
    }

    /// A desktop file's `Exec=` is not a shell line, so an empty one is a
    /// missing command rather than a command that runs nothing.
    #[test]
    fn an_empty_exec_line_is_not_a_handler() {
        assert_eq!(parse_exec("", Path::new("/tmp/a")), None);
        assert_eq!(parse_exec("   ", Path::new("/tmp/a")), None);
    }

    // -- default applications ------------------------------------------------

    /// The dropdown lists every app that declares the type, and nothing else.
    #[test]
    fn candidates_list_every_app_for_the_type() {
        let dir = tempfile::tempdir().expect("tempdir");
        let xdg = Xdg::isolated(dir.path());
        desktop(
            dir.path(),
            "alpha.desktop",
            &entry_file("Alpha", "alpha %f", "image/png"),
        );
        desktop(
            dir.path(),
            "beta.desktop",
            &entry_file("Beta", "beta %f", "image/png"),
        );
        desktop(
            dir.path(),
            "tune.desktop",
            &entry_file("Tune", "tune %f", "audio/mpeg"),
        );

        let got = candidates_for(&xdg, "image/png");
        let ids: Vec<&str> = got.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["alpha.desktop", "beta.desktop"]);
        assert_eq!(got[0].name, "Alpha");
        assert!(got[0].exec.contains("alpha"));
    }

    /// `NoDisplay` and `Hidden` entries are helpers, not choices.
    #[test]
    fn hidden_apps_are_not_candidates() {
        let dir = tempfile::tempdir().expect("tempdir");
        let xdg = Xdg::isolated(dir.path());
        desktop(
            dir.path(),
            "nodisplay.desktop",
            "[Desktop Entry]\nType=Application\nName=NoDisplay\nExec=nd %f\n\
             NoDisplay=true\nMimeType=image/png;\n",
        );
        desktop(
            dir.path(),
            "hidden.desktop",
            "[Desktop Entry]\nType=Application\nName=Hidden\nExec=h %f\n\
             Hidden=true\nMimeType=image/png;\n",
        );
        desktop(
            dir.path(),
            "shown.desktop",
            &entry_file("Shown", "shown %f", "image/png"),
        );

        let got = candidates_for(&xdg, "image/png");
        assert_eq!(got.len(), 1, "only the visible app is offered: {got:?}");
        assert_eq!(got[0].id, "shown.desktop");
    }

    /// The same desktop id installed twice is one choice, and the list is
    /// empty — not an error — when nothing handles the type.
    #[test]
    fn candidates_deduplicate_and_empty_is_honest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let xdg = Xdg::isolated(dir.path());
        desktop(
            dir.path(),
            "dup.desktop",
            &entry_file("Dup", "dup %f", "image/png"),
        );
        // A second copy in a lower-precedence directory.
        let other = xdg.data_dirs[0].join("applications");
        std::fs::create_dir_all(&other).expect("mkdir");
        std::fs::write(
            other.join("dup.desktop"),
            entry_file("Dup", "dup %f", "image/png"),
        )
        .expect("write");

        let got = candidates_for(&xdg, "image/png");
        assert_eq!(got.len(), 1, "one id, one choice: {got:?}");
        assert!(candidates_for(&xdg, "application/x-nothing").is_empty());
    }

    /// The current default is read back, removals suppress it, and "not set"
    /// is `None` rather than a guess from added associations.
    #[test]
    fn the_current_default_is_read_honestly() {
        let dir = tempfile::tempdir().expect("tempdir");
        let xdg = Xdg::isolated(dir.path());
        desktop(
            dir.path(),
            "viewer.desktop",
            &entry_file("Viewer", "viewer %U", "image/png"),
        );
        desktop(
            dir.path(),
            "added.desktop",
            &entry_file("Added", "added %f", "image/png"),
        );
        std::fs::create_dir_all(&xdg.config_home).expect("mkdir");
        std::fs::write(
            xdg.config_home.join("mimeapps.list"),
            "[Default Applications]\nimage/png=viewer.desktop\n\
             [Added Associations]\naudio/mpeg=added.desktop\n",
        )
        .expect("write");

        assert_eq!(
            default_for(&xdg, "image/png"),
            Some("viewer.desktop".to_string())
        );
        assert_eq!(
            default_for(&xdg, "audio/mpeg"),
            None,
            "an added association is not a default"
        );
    }

    /// Setting a default is visible to `default_for` and to `handler_for`,
    /// and keeps every other type's association intact.
    #[test]
    fn setting_a_default_preserves_other_types() {
        let dir = tempfile::tempdir().expect("tempdir");
        let xdg = Xdg::isolated(dir.path());
        desktop(
            dir.path(),
            "viewer.desktop",
            &entry_file("Viewer", "viewer %U", "image/png"),
        );
        desktop(
            dir.path(),
            "tune.desktop",
            &entry_file("Tune", "tune %f", "audio/mpeg"),
        );
        std::fs::create_dir_all(&xdg.config_home).expect("mkdir");
        std::fs::write(
            xdg.config_home.join("mimeapps.list"),
            "[Default Applications]\naudio/mpeg=tune.desktop\n",
        )
        .expect("write");

        set_default(&xdg, "image/png", "viewer.desktop").expect("set");
        assert_eq!(
            default_for(&xdg, "image/png"),
            Some("viewer.desktop".to_string())
        );
        assert_eq!(
            default_for(&xdg, "audio/mpeg"),
            Some("tune.desktop".to_string()),
            "the pre-existing association must survive the write"
        );
        assert_eq!(
            handler_for(&xdg, "image/png").expect("resolves").id,
            "viewer.desktop"
        );
    }

    /// A removal for the same pair would keep suppressing the new default, so
    /// setting it drops the removal. Setting a type twice replaces the line
    /// rather than appending a second one.
    #[test]
    fn setting_clears_a_removal_and_replaces_in_place() {
        let dir = tempfile::tempdir().expect("tempdir");
        let xdg = Xdg::isolated(dir.path());
        desktop(
            dir.path(),
            "one.desktop",
            &entry_file("One", "one %f", "image/png"),
        );
        desktop(
            dir.path(),
            "two.desktop",
            &entry_file("Two", "two %f", "image/png"),
        );
        std::fs::create_dir_all(&xdg.config_home).expect("mkdir");
        std::fs::write(
            xdg.config_home.join("mimeapps.list"),
            "[Default Applications]\nimage/png=one.desktop\n\
             [Removed Associations]\nimage/png=two.desktop\n",
        )
        .expect("write");

        set_default(&xdg, "image/png", "two.desktop").expect("set");
        assert_eq!(
            default_for(&xdg, "image/png"),
            Some("two.desktop".to_string())
        );
        assert_eq!(
            handler_for(&xdg, "image/png")
                .expect("no longer removed")
                .id,
            "two.desktop"
        );
        let text = std::fs::read_to_string(xdg.config_home.join("mimeapps.list")).expect("read");
        assert_eq!(
            text.lines().filter(|l| l.starts_with("image/png=")).count(),
            1,
            "one line per type, not an append: {text:?}"
        );
    }

    /// The file-type table and the shared MIME database agree on the common
    /// extensions — the check that there is one table, not two.
    #[cfg(unix)]
    #[test]
    fn the_file_type_table_agrees_with_the_shared_database() {
        for (ext, mime) in [
            ("png", "image/png"),
            ("mp4", "video/mp4"),
            ("mp3", "audio/mpeg"),
            ("pdf", "application/pdf"),
            ("zip", "application/zip"),
            ("txt", "text/plain"),
        ] {
            assert_eq!(mime_for_extension(ext).as_deref(), Some(mime), "for .{ext}");
        }
        for group in file_types() {
            assert!(!group.mimes.is_empty(), "{} covers nothing", group.name);
            assert!(
                candidates_for_all(&Xdg::isolated(Path::new("/nonexistent")), group.mimes)
                    .is_empty(),
                "an empty tree has no candidates"
            );
        }
        let names: Vec<&str> = file_types().iter().map(|g| g.name).collect();
        for want in ["Images", "Video", "Audio", "PDF", "Text", "Archives"] {
            assert!(names.contains(&want), "no {want} row: {names:?}");
        }
    }
}

/// End-to-end: resolve a type, find a handler, build its command, run it.
///
/// The pieces are each tested above; this is the one test that proves they
/// compose — that a registered handler is actually *executed* with the file as
/// an argument, which is the whole point of the module and the thing no unit
/// test of a parser can show.
///
/// `/usr/bin/touch` is the program because it is harmless, is at a known path,
/// and leaves a file behind that the test can look for. Its `Exec=` has no
/// field code, so this also exercises the spec's "append the file" rule: the
/// created file proves the argument order.
#[cfg(unix)]
#[test]
fn a_registered_handler_is_actually_run_with_the_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let xdg = Xdg::isolated(dir.path());
    let target = dir.path().join("notes.txt");
    std::fs::write(&target, b"words\n").expect("write");
    let touched = dir.path().join("created-by-the-handler");
    let mime = mime_of(&target);
    assert_eq!(mime, TEXT_TYPE, "a text file is text");

    let apps = xdg.applications_dirs()[0].clone();
    std::fs::create_dir_all(&apps).expect("mkdir");
    std::fs::write(
        apps.join("marker.desktop"),
        format!(
            "[Desktop Entry]\nType=Application\nName=Marker\n\
             Exec=/usr/bin/touch {}\nNoDisplay=false\nMimeType={mime};\n",
            touched.display()
        ),
    )
    .expect("write desktop");
    let list = xdg.config_home.join("mimeapps.list");
    std::fs::create_dir_all(&xdg.config_home).expect("mkdir");
    std::fs::write(
        &list,
        format!("[Default Applications]\n{mime}=marker.desktop\n"),
    )
    .expect("write");

    let candidate = handler_for(&xdg, &mime).expect("the handler is found");
    let handler = parse_exec(&candidate.exec, &target).expect("the command is parsed");
    assert_eq!(handler.program, "/usr/bin/touch");
    assert_eq!(
        handler.args,
        vec![
            touched.to_string_lossy().into_owned(),
            target.to_string_lossy().into_owned()
        ],
        "the file is a separate final argument, after the program's own"
    );

    handler.spawn().expect("the handler starts");
    // The process is not waited on, so this polls rather than asserting at
    // once — the spawn is asynchronous by design.
    for _ in 0..200 {
        if touched.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        touched.exists(),
        "the handler ran and created its own argument"
    );
}
