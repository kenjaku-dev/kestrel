//! Phase 1 IPC wire types.
//!
//! `kestrel-fs` is deliberately UI-free and has no `serde` dependency, and
//! three of its types cannot cross that boundary by derive anyway:
//!
//! * `FileEntry.modified` is `Option<SystemTime>` → `Option<i64>` epoch millis,
//! * `std::io::Error` (inside every `KestrelError`) does not implement
//!   `Serialize`,
//! * `KestrelError` is `#[non_exhaustive]`, so the wire must have a fallback.
//!
//! Hence this module: hand-written `Serialize` impls and total mapping
//! functions. Every `kind` string below is part of the frozen IPC contract —
//! the frontend matches on it, so do not rename any of them.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use kestrel_fs::error::KestrelError;
use kestrel_fs::model::{FileEntry, SortKey, SortSpec};
use kestrel_fs::open::OpenError;
use kestrel_fs::scan::{DEFAULT_BATCH_SIZE, DEFAULT_MAX_DEPTH, ScanOptions};
use serde::ser::{Serialize, SerializeStruct, Serializer};

// ---------------------------------------------------------------------------
// errors
// ---------------------------------------------------------------------------

/// The single error shape every Phase 1 command returns.
///
/// `{ kind: String, path: Option<String>, message: String }`, where `kind` is
/// a stable machine string from the tables in [`CmdError::kind_of_kestrel`]
/// / [`CmdError::kind_of_open`], `path` is the offending path when the error
/// carries one, and `message` is the human sentence (`Display` /
/// `OpenError::sentence()`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdError {
    /// Stable machine string, e.g. `"permission_denied"`. Matched on by the
    /// frontend; frozen — do not rename.
    pub kind: String,
    /// The offending path, lossy, when the error carries one.
    pub path: Option<String>,
    /// Human sentence for dialogs / error rows.
    pub message: String,
}

impl CmdError {
    /// Builds the wire error for an engine failure.
    pub fn from_kestrel(err: &KestrelError) -> Self {
        Self {
            kind: Self::kind_of_kestrel(err).to_string(),
            path: err.path().map(|p| p.to_string_lossy().into_owned()),
            message: err.to_string(),
        }
    }

    /// Builds the wire error for an `open_path` failure.
    pub fn from_open(err: &OpenError) -> Self {
        Self {
            kind: Self::kind_of_open(err).to_string(),
            path: Some(err.path().to_string_lossy().into_owned()),
            message: err.sentence(),
        }
    }

    /// Builds the wire error for a failure that carries a path but no engine
    /// error (spawn failure already classified aside, timeouts, ...).
    pub fn custom(kind: impl Into<String>, path: Option<&Path>, message: String) -> Self {
        Self {
            kind: kind.into(),
            path: path.map(|p| p.to_string_lossy().into_owned()),
            message,
        }
    }

    /// The stable `kind` for each [`KestrelError`] variant.
    ///
    /// The trailing wildcard is load-bearing, not laziness: `KestrelError` is
    /// `#[non_exhaustive]`, so a future engine variant must degrade to
    /// `"unknown"` on the wire rather than fail to compile the backend.
    #[must_use]
    pub fn kind_of_kestrel(err: &KestrelError) -> &'static str {
        match err {
            KestrelError::PermissionDenied { .. } => "permission_denied",
            KestrelError::NotFound { .. } => "not_found",
            KestrelError::AlreadyExists { .. } => "already_exists",
            KestrelError::InvalidFilename { .. } => "invalid_filename",
            KestrelError::FilesystemLoop { .. } => "filesystem_loop",
            KestrelError::LoopDetected { .. } => "loop_detected",
            KestrelError::CrossesDevices { .. } => "crosses_devices",
            KestrelError::NotADirectory { .. } => "not_a_directory",
            KestrelError::IsADirectory { .. } => "is_a_directory",
            KestrelError::DirectoryNotEmpty { .. } => "directory_not_empty",
            KestrelError::InvalidInput { .. } => "invalid_input",
            KestrelError::Cancelled => "cancelled",
            KestrelError::Trash { .. } => "trash",
            KestrelError::Watch { .. } => "watch",
            KestrelError::Io { .. } => "io",
            // `#[non_exhaustive]`: future variants degrade, never break.
            _ => "unknown",
        }
    }

    /// The stable `kind` for each [`OpenError`] variant.
    #[must_use]
    pub fn kind_of_open(err: &OpenError) -> &'static str {
        match err {
            OpenError::NoHandler { .. } => "no_handler",
            OpenError::Unreadable { .. } => "unreadable",
            OpenError::NoCommand { .. } => "no_command",
            OpenError::Spawn { .. } => "spawn_failed",
            OpenError::WorkerLost { .. } => "worker_lost",
        }
    }
}

impl Serialize for CmdError {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut out = s.serialize_struct("CmdError", 3)?;
        out.serialize_field("kind", &self.kind)?;
        out.serialize_field("path", &self.path)?;
        out.serialize_field("message", &self.message)?;
        out.end()
    }
}

// ---------------------------------------------------------------------------
// entries
// ---------------------------------------------------------------------------

/// Wire form of [`FileEntry`]. `modified` is epoch millis (`None` stays
/// `None`; pre-epoch times, which cannot be expressed as a positive
/// `duration_since`, also become `None` rather than a wrong number).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntryDto {
    /// File name only, lossy.
    pub name: String,
    /// Full path, lossy.
    pub path: String,
    /// `"directory" | "file" | "symlink" | "other"` (`EntryKind::as_str`).
    pub kind: String,
    /// Length in bytes, or `None` when unknown.
    pub size: Option<u64>,
    /// Last modification time as epoch millis, or `None`.
    pub modified: Option<i64>,
    /// Leading-dot / hidden-attribute rule, as the engine computed it.
    pub hidden: bool,
    /// For a symlink, whether its target is a directory. See
    /// `FileEntry::is_dir_target` for why `None` is three different facts.
    pub is_dir_target: Option<bool>,
}

impl FileEntryDto {
    /// Total conversion from the engine type.
    pub fn from_entry(entry: &FileEntry) -> Self {
        Self {
            name: entry.name.clone(),
            path: entry.path.to_string_lossy().into_owned(),
            kind: entry.kind.as_str().to_string(),
            size: entry.size,
            modified: system_time_millis(entry.modified),
            hidden: entry.hidden,
            is_dir_target: entry.is_dir_target,
        }
    }
}

/// `Option<SystemTime>` → `Option<i64>` epoch millis. `None` stays `None`;
/// times before the epoch become `None` (no representable positive duration)
/// rather than wrapping or panicking.
#[must_use]
pub fn system_time_millis(t: Option<SystemTime>) -> Option<i64> {
    t.and_then(|t| {
        t.duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|d| i64::try_from(d.as_millis()).ok())
    })
}

impl Serialize for FileEntryDto {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut out = s.serialize_struct("FileEntryDto", 7)?;
        out.serialize_field("name", &self.name)?;
        out.serialize_field("path", &self.path)?;
        out.serialize_field("kind", &self.kind)?;
        out.serialize_field("size", &self.size)?;
        out.serialize_field("modified", &self.modified)?;
        out.serialize_field("hidden", &self.hidden)?;
        out.serialize_field("isDirTarget", &self.is_dir_target)?;
        out.end()
    }
}

// ---------------------------------------------------------------------------
// scan options (frontend → backend)
// ---------------------------------------------------------------------------

/// Sort column, as the frontend sends it (`"name" | "size" | "modified" |
/// `"kind"`). Typed rather than stringly so a typo fails deserialization
/// loudly instead of silently re-sorting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortKeyDto {
    /// File name, natural order. The default.
    #[default]
    Name,
    /// Size in bytes; unknown sizes sort last.
    Size,
    /// Last modification time; unknown times sort last.
    Modified,
    /// Entry kind.
    Kind,
}

impl SortKeyDto {
    fn into_engine(self) -> SortKey {
        match self {
            Self::Name => SortKey::Name,
            Self::Size => SortKey::Size,
            Self::Modified => SortKey::Modified,
            Self::Kind => SortKey::Kind,
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_max_depth() -> usize {
    DEFAULT_MAX_DEPTH
}

fn default_batch_size() -> usize {
    DEFAULT_BATCH_SIZE
}

/// Full sort spec. Every field has a default so the frontend can send
/// `"sort": {}` for engine-default ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SortSpecDto {
    /// Column. Defaults to `"name"`.
    #[serde(default)]
    pub key: SortKeyDto,
    /// Ascending. Defaults to `true`.
    #[serde(default = "default_true")]
    pub ascending: bool,
    /// Directories above files regardless of column. Defaults to `true`.
    #[serde(default = "default_true")]
    pub dirs_first: bool,
}

impl Default for SortSpecDto {
    fn default() -> Self {
        Self {
            key: SortKeyDto::Name,
            ascending: true,
            dirs_first: true,
        }
    }
}

impl SortSpecDto {
    fn into_engine(self) -> SortSpec {
        SortSpec {
            key: self.key.into_engine(),
            ascending: self.ascending,
            dirs_first: self.dirs_first,
        }
    }
}

fn default_sort() -> Option<SortSpecDto> {
    Some(SortSpecDto::default())
}

/// Knobs for `scan_start`, mirroring [`ScanOptions`]. Every field has a
/// default so the frontend can send `{}` for a plain sorted listing:
/// non-recursive, hidden shown, engine-default sort, depth 16, batches of 256.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanOptionsDto {
    /// Descend into subdirectories. Default `false` — the file list wants a
    /// listing, not a walk.
    #[serde(default)]
    pub recursive: bool,
    /// Descend through symlinked directories. Default `false`.
    #[serde(default)]
    pub follow_symlinks: bool,
    /// Depth ceiling for recursive scans. Default 16.
    #[serde(default = "default_max_depth")]
    pub max_depth: usize,
    /// Emit dotfiles. Default `true`.
    #[serde(default = "default_true")]
    pub show_hidden: bool,
    /// Sort order, or `null` for `read_dir` order (faster). Default: engine
    /// default (name, ascending, dirs first).
    #[serde(default = "default_sort")]
    pub sort: Option<SortSpecDto>,
    /// Entries per `BatchEnd` on the engine side. Default 256.
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
}

impl Default for ScanOptionsDto {
    fn default() -> Self {
        serde_json::from_value(serde_json::json!({}))
            .expect("empty object must deserialize into defaults")
    }
}

impl ScanOptionsDto {
    /// Total, infallible conversion: every variant maps, every default is
    /// the engine's own.
    #[must_use]
    pub fn into_engine(self) -> ScanOptions {
        ScanOptions {
            recursive: self.recursive,
            follow_symlinks: self.follow_symlinks,
            max_depth: self.max_depth,
            show_hidden: self.show_hidden,
            sort: self.sort.map(SortSpecDto::into_engine),
            batch_size: self.batch_size,
        }
    }
}

// ---------------------------------------------------------------------------
// scan events (backend → frontend)
// ---------------------------------------------------------------------------

/// One message sent over the scan `Channel`.
///
/// The engine emits one event per entry; the pump coalesces each burst (up to
/// the next `BatchEnd`) into a single [`Entries`](ScanEventDto::Entries)
/// message so a 10k directory costs ~40 IPC messages instead of ~10k
/// `webview.eval` round-trips. `BatchEnd` itself is consumed by the pump and
/// never forwarded: the batch message *is* the boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanEventDto {
    /// A coalesced burst of rows, in engine-sorted order.
    Entries(Vec<FileEntryDto>),
    /// A path that could not be read. Flat (not nested under `CmdError`)
    /// so an error row renders from one object.
    Error {
        /// The unreadable path, lossy.
        path: String,
        /// Stable machine string (`CmdError::kind_of_kestrel`).
        kind: String,
        /// Human sentence.
        message: String,
    },
    /// Traversal finished. `total` is the engine's own count.
    Complete {
        /// Number of entry events the engine emitted.
        total: usize,
    },
}

impl Serialize for ScanEventDto {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Entries(entries) => {
                let mut out = s.serialize_struct("ScanEventDto", 2)?;
                out.serialize_field("type", "entries")?;
                out.serialize_field("entries", entries)?;
                out.end()
            }
            Self::Error {
                path,
                kind,
                message,
            } => {
                let mut out = s.serialize_struct("ScanEventDto", 4)?;
                out.serialize_field("type", "error")?;
                out.serialize_field("path", path)?;
                out.serialize_field("kind", kind)?;
                out.serialize_field("message", message)?;
                out.end()
            }
            Self::Complete { total } => {
                let mut out = s.serialize_struct("ScanEventDto", 2)?;
                out.serialize_field("type", "complete")?;
                out.serialize_field("total", total)?;
                out.end()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// open_path result
// ---------------------------------------------------------------------------

/// `open_path` answer: `{ entered_dir: bool }`. `true` means the path was a
/// directory and the frontend should navigate into it (nothing was launched);
/// `false` means a file was handed to its default application. Failures
/// (no handler, spawn refused, ...) come back as [`CmdError`], never here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenResultDto {
    /// Whether the path was entered as a directory.
    pub entered_dir: bool,
}

impl Serialize for OpenResultDto {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut out = s.serialize_struct("OpenResultDto", 1)?;
        out.serialize_field("enteredDir", &self.entered_dir)?;
        out.end()
    }
}

// ---------------------------------------------------------------------------
// tests — no Tauri runtime needed (pure mapping, serde_json only)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::path::PathBuf;
    use std::time::Duration;

    fn entry_with_modified(modified: Option<SystemTime>) -> FileEntry {
        FileEntry {
            name: "a.txt".to_string(),
            path: PathBuf::from("/tmp/a.txt"),
            kind: kestrel_fs::model::EntryKind::File,
            size: Some(3),
            modified,
            hidden: false,
            is_dir_target: None,
        }
    }

    #[test]
    fn system_time_becomes_epoch_millis() {
        let t = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        assert_eq!(
            system_time_millis(Some(t)),
            Some(1_700_000_000_000),
            "seconds must become millis, not pass through"
        );
    }

    #[test]
    fn no_time_stays_no_time() {
        assert_eq!(system_time_millis(None), None);
        assert_eq!(
            FileEntryDto::from_entry(&entry_with_modified(None)).modified,
            None
        );
    }

    #[test]
    fn pre_epoch_time_becomes_none_rather_than_wrong() {
        let t = UNIX_EPOCH - Duration::from_secs(1);
        assert_eq!(system_time_millis(Some(t)), None);
    }

    #[test]
    fn entry_dto_carries_every_field() {
        let t = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let dto = FileEntryDto::from_entry(&entry_with_modified(Some(t)));
        assert_eq!(dto.name, "a.txt");
        assert_eq!(dto.path, "/tmp/a.txt");
        assert_eq!(dto.kind, "file");
        assert_eq!(dto.size, Some(3));
        assert_eq!(dto.modified, Some(1_700_000_000_000));
        assert!(!dto.hidden);
        assert_eq!(dto.is_dir_target, None);
    }

    #[test]
    fn entry_dto_serializes_modified_as_millis_number() {
        let t = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let v = serde_json::to_value(FileEntryDto::from_entry(&entry_with_modified(Some(t))))
            .expect("serialize");
        assert_eq!(v["modified"], serde_json::json!(1_700_000_000_000i64));
        assert_eq!(v["kind"], serde_json::json!("file"));
        let v = serde_json::to_value(FileEntryDto::from_entry(&entry_with_modified(None)))
            .expect("serialize");
        assert!(v["modified"].is_null(), "None must stay null: {v}");
    }

    /// Every current `KestrelError` variant maps to its documented kind, and
    /// none of them falls through to the `#[non_exhaustive]` fallback.
    #[test]
    fn each_kestrel_error_maps_to_its_kind() {
        use kestrel_fs::error::KestrelError as E;
        let io = |k: io::ErrorKind| io::Error::from(k);
        let cases: Vec<(E, &str, bool)> = vec![
            (
                E::PermissionDenied {
                    path: "/x".into(),
                    source: io(io::ErrorKind::PermissionDenied),
                },
                "permission_denied",
                true,
            ),
            (
                E::NotFound {
                    path: "/x".into(),
                    source: io(io::ErrorKind::NotFound),
                },
                "not_found",
                true,
            ),
            (
                E::AlreadyExists { path: "/x".into() },
                "already_exists",
                true,
            ),
            (
                E::InvalidFilename {
                    path: "/x".into(),
                    source: io(io::ErrorKind::InvalidFilename),
                },
                "invalid_filename",
                true,
            ),
            (
                E::FilesystemLoop {
                    path: "/x".into(),
                    source: io(io::ErrorKind::Other),
                },
                "filesystem_loop",
                true,
            ),
            (E::LoopDetected { path: "/x".into() }, "loop_detected", true),
            (
                E::CrossesDevices {
                    path: "/x".into(),
                    source: io(io::ErrorKind::CrossesDevices),
                },
                "crosses_devices",
                true,
            ),
            (
                E::NotADirectory {
                    path: "/x".into(),
                    source: io(io::ErrorKind::NotADirectory),
                },
                "not_a_directory",
                true,
            ),
            (
                E::IsADirectory {
                    path: "/x".into(),
                    source: io(io::ErrorKind::IsADirectory),
                },
                "is_a_directory",
                true,
            ),
            (
                E::DirectoryNotEmpty {
                    path: "/x".into(),
                    source: io(io::ErrorKind::DirectoryNotEmpty),
                },
                "directory_not_empty",
                true,
            ),
            (
                E::InvalidInput {
                    path: "/x".into(),
                    reason: "nope".into(),
                },
                "invalid_input",
                true,
            ),
            (E::Cancelled, "cancelled", false),
            (
                E::Trash {
                    path: "/x".into(),
                    source: Box::new(io(io::ErrorKind::Other)),
                },
                "trash",
                true,
            ),
            (
                E::Watch {
                    path: Some("/x".into()),
                    message: "gone".into(),
                },
                "watch",
                true,
            ),
            (
                E::Watch {
                    path: None,
                    message: "gone".into(),
                },
                "watch",
                false,
            ),
            (
                E::Io {
                    path: Some("/x".into()),
                    source: io(io::ErrorKind::BrokenPipe),
                },
                "io",
                true,
            ),
            (
                E::Io {
                    path: None,
                    source: io(io::ErrorKind::BrokenPipe),
                },
                "io",
                false,
            ),
        ];
        for (err, want_kind, want_path) in cases {
            let dto = CmdError::from_kestrel(&err);
            assert_eq!(dto.kind, want_kind, "wrong kind for {err:?}");
            assert_eq!(dto.path.is_some(), want_path, "wrong path for {err:?}");
            assert!(!dto.message.is_empty(), "message must never be empty");
            assert_ne!(
                dto.kind, "unknown",
                "a current variant must not hit the non_exhaustive fallback: {err:?}"
            );
        }
    }

    #[test]
    fn cmd_error_wire_shape_is_kind_path_message() {
        let err = KestrelError::not_found("/nope");
        let v = serde_json::to_value(CmdError::from_kestrel(&err)).expect("serialize");
        assert_eq!(v["kind"], serde_json::json!("not_found"));
        assert_eq!(v["path"], serde_json::json!("/nope"));
        assert!(v["message"].as_str().is_some_and(|m| !m.is_empty()));
    }

    #[test]
    fn each_open_error_maps_to_its_kind_and_sentence() {
        let cases = [
            (
                OpenError::NoHandler {
                    path: "/a.png".into(),
                    mime: "image/png".into(),
                },
                "no_handler",
            ),
            (
                OpenError::Unreadable {
                    path: "/a.png".into(),
                    source: io::Error::from(io::ErrorKind::PermissionDenied),
                },
                "unreadable",
            ),
            (
                OpenError::NoCommand {
                    path: "/a.png".into(),
                    id: "broken.desktop".into(),
                },
                "no_command",
            ),
            (
                OpenError::Spawn {
                    path: "/a.png".into(),
                    app: "Viewer".into(),
                    source: io::Error::from(io::ErrorKind::NotFound),
                },
                "spawn_failed",
            ),
            (
                OpenError::WorkerLost {
                    path: "/a.png".into(),
                },
                "worker_lost",
            ),
        ];
        for (err, want_kind) in cases {
            let dto = CmdError::from_open(&err);
            assert_eq!(dto.kind, want_kind, "wrong kind for {err:?}");
            assert_eq!(dto.path.as_deref(), Some("/a.png"));
            assert!(
                dto.message.contains("a.png"),
                "sentence must name the file: {}",
                dto.message
            );
            assert!(!CmdError::kind_of_open(&err).is_empty());
        }
    }

    #[test]
    fn scan_options_default_to_a_plain_sorted_listing() {
        let opts: ScanOptionsDto = serde_json::from_value(serde_json::json!({})).expect("defaults");
        assert!(!opts.recursive);
        assert!(!opts.follow_symlinks);
        assert!(opts.show_hidden);
        assert_eq!(opts.max_depth, DEFAULT_MAX_DEPTH);
        assert_eq!(opts.batch_size, DEFAULT_BATCH_SIZE);
        let engine = opts.into_engine();
        assert_eq!(engine, ScanOptions::listing());
        // camelCase from the JS side lands on the same fields.
        let opts: ScanOptionsDto = serde_json::from_value(serde_json::json!({
            "recursive": true, "showHidden": false, "maxDepth": 2, "followSymlinks": true,
        }))
        .expect("camelCase");
        assert!(opts.recursive && opts.follow_symlinks && !opts.show_hidden);
        assert_eq!(opts.max_depth, 2);
    }

    #[test]
    fn scan_options_round_trip_sort_keys() {
        for (key, want) in [
            ("name", SortKey::Name),
            ("size", SortKey::Size),
            ("modified", SortKey::Modified),
            ("kind", SortKey::Kind),
        ] {
            let opts: ScanOptionsDto = serde_json::from_value(serde_json::json!({
                "sort": {"key": key, "ascending": false, "dirsFirst": false}
            }))
            .expect("sort dto");
            let engine = opts.into_engine();
            let spec = engine.sort.expect("sort survives");
            assert_eq!(spec.key, want);
            assert!(!spec.ascending);
            assert!(!spec.dirs_first);
        }
    }

    #[test]
    fn scan_event_dto_wire_tags() {
        let v = serde_json::to_value(ScanEventDto::Complete { total: 3 }).expect("serialize");
        assert_eq!(v, serde_json::json!({"type": "complete", "total": 3}));
        let v = serde_json::to_value(ScanEventDto::Error {
            path: "/x".into(),
            kind: "permission_denied".into(),
            message: "denied".into(),
        })
        .expect("serialize");
        assert_eq!(v["type"], serde_json::json!("error"));
        assert_eq!(v["kind"], serde_json::json!("permission_denied"));
        let v = serde_json::to_value(ScanEventDto::Entries(vec![])).expect("serialize");
        assert_eq!(v["type"], serde_json::json!("entries"));
    }
}
