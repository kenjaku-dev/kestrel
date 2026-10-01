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
// watch events (backend → frontend)
//
// Frozen Phase 2 contract, internally tagged with `"type"` exactly like
// [`ScanEventDto`]: `{ "type": "changed", "dirs": [...] }` carries the
// directories whose contents may have changed, and
// `{ "type": "error", "error": CmdError }` carries a watcher failure.
// The frontend re-runs `scan_start` for the current directory when any
// listed dir is the one it is viewing — deliberately no incremental
// patching, so both UIs share the one scan code path.
// ---------------------------------------------------------------------------

/// One message sent over a `watch_subscribe` [`Channel`](tauri::ipc::Channel).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEventDto {
    /// Directories whose listing may be stale, as lossy absolute paths.
    Changed {
        /// Every directory that may need re-listing.
        dirs: Vec<String>,
    },
    /// The watcher reported an error. Nested under `"error"` (unlike scan
    /// errors, which are flat rows) — this nesting is the frozen Phase 2
    /// contract, so do not flatten it.
    Error {
        /// The watcher failure, as the standard error shape.
        error: CmdError,
    },
}

impl Serialize for WatchEventDto {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Changed { dirs } => {
                let mut out = s.serialize_struct("WatchEventDto", 2)?;
                out.serialize_field("type", "changed")?;
                out.serialize_field("dirs", dirs)?;
                out.end()
            }
            Self::Error { error } => {
                let mut out = s.serialize_struct("WatchEventDto", 2)?;
                out.serialize_field("type", "error")?;
                out.serialize_field("error", error)?;
                out.end()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// op request / events (backend → frontend)
//
// Frozen Phase 3a contract, internally tagged exactly like [`ScanEventDto`]
// and [`WatchEventDto`]:
//
// ```json
// { "op": "copy", "src": "/a", "dst": "/b" }
// { "op": "move", "src": "/a", "dst": "/b" }
// { "op": "trash", "src": "/a" }
// { "op": "delete", "src": "/a", "recursive": true }
// ```
//
// ```json
// { "type": "progress", "id": 7, "phase": "copying",
//   "doneBytes": 1, "totalBytes": 9, "doneItems": 1, "totalItems": 9 }
// { "type": "done", "id": 7, "summary": "copied report.pdf" }
// { "type": "error", "id": 7, "error": CmdError }
// ```
//
// Phase 3b adds the human-in-the-loop round-trip, one variant and one command:
// ```json
// { "type": "collision", "id": 7, "dst": "/abs/path", "kind": "file" }
// op_collision_answer(id: number, dst: string, decision: "overwrite"|"skip"|"abort")
// ```
// A `collision` is **not** terminal: the op is blocked, waiting, and exactly one
// terminal event still follows. For 3a — and for every collision the 3b worker
// cannot ask about — a collision is an `error` event with kind
// `"already_exists"` and the op fails with the destination untouched.
// ---------------------------------------------------------------------------

/// What already occupies a colliding destination, for the dialog's headline.
///
/// `symlink_metadata` answers for the *name*, never its target: a dangling
/// link occupies a name, so it is a `symlink` collision even though nothing it
/// points at exists. Frozen wire strings — the frontend matches on them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CollisionKindDto {
    /// A regular file.
    File,
    /// A directory (a merge, not a replacement).
    Dir,
    /// A symlink, dangling or not.
    Symlink,
    /// A socket, fifo or device node: the destination exists, and no
    /// sensible copy exists either.
    Other,
}

impl CollisionKindDto {
    /// Classifies whatever occupies `dst`, without following a final symlink.
    ///
    /// An `lstat` that fails (the entry vanished between the engine's check
    /// and this call — nothing holds a lock on a directory entry) degrades to
    /// [`Other`](Self::Other) rather than failing: the question is still
    /// answerable, and an unreadable path must not take the op down.
    #[must_use]
    pub fn of(dst: &Path) -> Self {
        let Ok(md) = std::fs::symlink_metadata(dst) else {
            return Self::Other;
        };
        let ft = md.file_type();
        if ft.is_symlink() {
            Self::Symlink
        } else if ft.is_dir() {
            Self::Dir
        } else if ft.is_file() {
            Self::File
        } else {
            Self::Other
        }
    }
}

/// One collision an in-flight op is currently blocked on, as
/// `op_pending_collisions` reports it.
///
/// The discovery half of the round-trip. A `collision` event is fire-and-forget
/// over a `Channel`, so a frontend that mounted (or reloaded) while an op was
/// paused never saw it and has no other way to learn there is a question
/// waiting. This command is the resynchronisation point, mirroring the scan
/// pump: the registry is the truth, and the UI keys by `id` rather than
/// assuming it caught every message.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PendingCollisionDto {
    /// The backend-minted job id from `op_start`. The answer must name it.
    pub id: u64,
    /// The colliding destination, absolute, exactly as the pending
    /// [`OpEventDto::Collision`] event carried it — the answer must match this
    /// string byte-for-byte, which is why it is stored, not recomputed.
    pub dst: String,
    /// What already occupies `dst`. Carried alongside `dst` so the dialog can
    /// render its headline without a second `stat` round-trip.
    pub kind: CollisionKindDto,
}

/// The user's answer to one [`OpEventDto::Collision`], sent back by
/// `op_collision_answer`.
///
/// Frozen wire strings — the frontend matches on them. An unrecognised value
/// is a rejected command, never a guessed answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CollisionDecisionDto {
    /// Replace what is at the destination — **for this one path only**. The
    /// backend records it in the engine's `AnsweredCollisions`, keyed by `dst`,
    /// and retries with [`Collision::Fail`] still in effect, so the next
    /// collision in the same tree is asked about separately.
    Overwrite,
    /// Leave the destination exactly as it is and carry on with the rest.
    Skip,
    /// Stop the whole operation. Any partial destination is left in place for
    /// the user to inspect — never silently cleaned up.
    Abort,
}

impl CollisionDecisionDto {
    /// The engine policy this answer records, or `None` for
    /// [`Abort`](Self::Abort) — which is not a policy but a stop, and so never
    /// reaches `AnsweredCollisions`.
    ///
    /// The mapping is deliberately narrow: there is no answer that means
    /// "overwrite every remaining collision", because that is the one answer
    /// `Collision::Overwrite` would express and the one this design forbids.
    #[must_use]
    pub fn as_engine_collision(self) -> Option<kestrel_fs::ops::Collision> {
        use kestrel_fs::ops::Collision;
        match self {
            Self::Overwrite => Some(Collision::Overwrite),
            Self::Skip => Some(Collision::Skip),
            Self::Abort => None,
        }
    }
}

/// One mutating operation the frontend asks for. Internally tagged with
/// `"op"`; every variant carries exactly the paths the frozen contract shows
/// and nothing else.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum OpRequestDto {
    /// Copy `src` to `dst`, refusing to overwrite.
    Copy {
        /// Source path.
        src: String,
        /// Destination path.
        dst: String,
    },
    /// Move `src` to `dst`, refusing to overwrite.
    Move {
        /// Source path.
        src: String,
        /// Destination path.
        dst: String,
    },
    /// Send `src` to the platform recycle bin.
    Trash {
        /// Path to trash.
        src: String,
    },
    /// Permanently remove `src`. Recursive only when asked: a non-recursive
    /// delete of a populated directory fails rather than losing a tree.
    Delete {
        /// Path to delete.
        src: String,
        /// Delete a directory and everything under it. Default `false`.
        #[serde(default)]
        recursive: bool,
    },
}

/// Which stage an [`OpEventDto::Progress`] event reports on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpPhase {
    /// Measuring the source tree before copying. Totals are still unknown, so
    /// `totalBytes`/`totalItems` are 0 (indeterminate, never guessed).
    Measuring,
    /// Copying (or moving) bytes.
    Copying,
    /// Deleting entries. `totalBytes` is always 0: a delete counts entries,
    /// it has no byte total to measure.
    Deleting,
}

impl OpPhase {
    /// The frozen wire string.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Measuring => "measuring",
            Self::Copying => "copying",
            Self::Deleting => "deleting",
        }
    }
}

/// One message sent over an `op_start` [`Channel`](tauri::ipc::Channel).
///
/// Deliberately extensible: Phase 3b adds the [`Collision`](Self::Collision)
/// variant here without touching any existing one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpEventDto {
    /// Running totals for `id`. `totalBytes`/`totalItems` are 0 while unknown
    /// (measuring, deletes) — the frontend shows indeterminate then.
    Progress {
        /// The backend-minted job id from `op_start`.
        id: u64,
        /// Which stage this reports on.
        phase: OpPhase,
        /// Bytes written so far (0 for deletes).
        done_bytes: u64,
        /// Bytes in total, or 0 when unknown.
        total_bytes: u64,
        /// Files copied, or entries removed, so far.
        done_items: usize,
        /// Files/entries in total, or 0 when unknown.
        total_items: usize,
    },
    /// The op finished. Always exactly one terminal event (`Done` or
    /// `Error`) per started op.
    Done {
        /// The backend-minted job id from `op_start`.
        id: u64,
        /// Human sentence, e.g. `"copied report.pdf"`.
        summary: String,
    },
    /// The op failed or was cancelled. Nested under `"error"` per the frozen
    /// `WatchEventDto` shape — do not flatten it.
    Error {
        /// The backend-minted job id from `op_start`.
        id: u64,
        /// The failure, as the standard error shape.
        error: CmdError,
    },
    /// A destination already exists and the op is **blocked**, waiting for
    /// `op_collision_answer(id, dst, decision)`.
    ///
    /// Not a terminal event: the worker is parked at this `dst` and nothing
    /// else is happening. Exactly one terminal event still follows — `done`,
    /// or an `error` once every collision is answered, cancelled, or timed
    /// out.
    ///
    /// Emitted only for a path with **no answer recorded**. Once an answer is
    /// in the engine's `AnsweredCollisions` the path is settled and is never
    /// asked about again, however many times the op is retried.
    Collision {
        /// The backend-minted job id from `op_start`.
        id: u64,
        /// The colliding destination, absolute, as the engine reported it. The
        /// answer must name this exact string.
        dst: String,
        /// What already occupies `dst`.
        kind: CollisionKindDto,
    },
}

impl Serialize for OpEventDto {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Progress {
                id,
                phase,
                done_bytes,
                total_bytes,
                done_items,
                total_items,
            } => {
                let mut out = s.serialize_struct("OpEventDto", 7)?;
                out.serialize_field("type", "progress")?;
                out.serialize_field("id", id)?;
                out.serialize_field("phase", phase.as_str())?;
                out.serialize_field("doneBytes", done_bytes)?;
                out.serialize_field("totalBytes", total_bytes)?;
                out.serialize_field("doneItems", done_items)?;
                out.serialize_field("totalItems", total_items)?;
                out.end()
            }
            Self::Done { id, summary } => {
                let mut out = s.serialize_struct("OpEventDto", 3)?;
                out.serialize_field("type", "done")?;
                out.serialize_field("id", id)?;
                out.serialize_field("summary", summary)?;
                out.end()
            }
            Self::Error { id, error } => {
                let mut out = s.serialize_struct("OpEventDto", 3)?;
                out.serialize_field("type", "error")?;
                out.serialize_field("id", id)?;
                out.serialize_field("error", error)?;
                out.end()
            }
            Self::Collision { id, dst, kind } => {
                // Field order is part of the committed golden bytes: `type`
                // first, then the job id, then the question.
                let mut out = s.serialize_struct("OpEventDto", 4)?;
                out.serialize_field("type", "collision")?;
                out.serialize_field("id", id)?;
                out.serialize_field("dst", dst)?;
                out.serialize_field("kind", kind)?;
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

    /// `CollisionKindDto` classifies a destination from the *name*, never its
    /// target — the same rule the engine's `path_entry_exists` follows, and the
    /// reason a dangling link is a collision at all.
    #[test]
    fn collision_kind_reads_the_name_and_never_the_target() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file = tmp.path().join("file");
        let dir = tmp.path().join("dir");
        let link = tmp.path().join("link");
        let dangling = tmp.path().join("dangling");
        std::fs::write(&file, b"x").expect("write");
        std::fs::create_dir(&dir).expect("mkdir");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&file, &link).expect("link");
            std::os::unix::fs::symlink(tmp.path().join("nowhere"), &dangling).expect("dangling");
        }
        let absent = tmp.path().join("absent");

        assert_eq!(CollisionKindDto::of(&file), CollisionKindDto::File);
        assert_eq!(CollisionKindDto::of(&dir), CollisionKindDto::Dir);
        #[cfg(unix)]
        {
            assert_eq!(
                CollisionKindDto::of(&link),
                CollisionKindDto::Symlink,
                "a link to a file is a symlink, not a file: the question is what \
                 occupies the name"
            );
            assert_eq!(
                CollisionKindDto::of(&dangling),
                CollisionKindDto::Symlink,
                "a dangling link still occupies the name — `Path::exists` would \
                 have said this was absent, which is the wrong answer"
            );
        }
        // A failed `lstat` degrades rather than failing: an unreadable path
        // must never take the op down.
        assert_eq!(CollisionKindDto::of(&absent), CollisionKindDto::Other);

        // And the frozen wire strings, in both directions.
        for (dto, wire) in [
            (CollisionKindDto::File, "\"file\""),
            (CollisionKindDto::Dir, "\"dir\""),
            (CollisionKindDto::Symlink, "\"symlink\""),
            (CollisionKindDto::Other, "\"other\""),
        ] {
            assert_eq!(serde_json::to_string(&dto).expect("serializes"), wire);
            assert_eq!(
                serde_json::from_str::<CollisionKindDto>(wire).expect(wire),
                dto
            );
        }
    }

    /// `abort` is not a policy and must not be recorded as one: there is no
    /// `Collision` that means "stop the whole operation", and inventing one
    /// would make it reachable from `AnsweredCollisions`.
    #[test]
    fn abort_maps_to_no_engine_collision() {
        assert_eq!(
            CollisionDecisionDto::Overwrite.as_engine_collision(),
            Some(kestrel_fs::ops::Collision::Overwrite)
        );
        assert_eq!(
            CollisionDecisionDto::Skip.as_engine_collision(),
            Some(kestrel_fs::ops::Collision::Skip)
        );
        assert_eq!(
            CollisionDecisionDto::Abort.as_engine_collision(),
            None,
            "abort stops the op; it is not a per-path policy"
        );
    }

    /// The discovery row is the shape a frontend needs to rebuild a dialog
    /// after mounting mid-pause: an id to key on, the exact `dst` the answer
    /// must name, and a kind so it can render without a second `stat`.
    #[test]
    fn pending_collision_row_serializes_to_the_frozen_shape() {
        let dto = PendingCollisionDto {
            id: 7,
            dst: "/home/u/dst.txt".to_string(),
            kind: CollisionKindDto::File,
        };
        assert_eq!(
            serde_json::to_string(&dto).expect("serializes"),
            r#"{"id":7,"dst":"/home/u/dst.txt","kind":"file"}"#
        );
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
    fn watch_event_dto_wire_tags() {
        let v = serde_json::to_value(WatchEventDto::Changed {
            dirs: vec!["/tmp/x".to_string()],
        })
        .expect("serialize");
        assert_eq!(
            v,
            serde_json::json!({"type": "changed", "dirs": ["/tmp/x"]})
        );
        let err = CmdError::custom("watch", Some(Path::new("/tmp/x")), "gone".to_string());
        let v = serde_json::to_value(WatchEventDto::Error { error: err }).expect("serialize");
        assert_eq!(v["type"], serde_json::json!("error"));
        assert_eq!(v["error"]["kind"], serde_json::json!("watch"));
        assert_eq!(v["error"]["path"], serde_json::json!("/tmp/x"));
        assert!(
            v["error"]["message"]
                .as_str()
                .is_some_and(|m| !m.is_empty())
        );
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

    #[test]
    fn op_request_dto_deserializes_all_four_shapes() {
        let req: OpRequestDto = serde_json::from_value(serde_json::json!({
            "op": "copy", "src": "/a", "dst": "/b",
        }))
        .expect("copy");
        assert_eq!(
            req,
            OpRequestDto::Copy {
                src: "/a".into(),
                dst: "/b".into()
            }
        );
        let req: OpRequestDto = serde_json::from_value(serde_json::json!({
            "op": "move", "src": "/a", "dst": "/b",
        }))
        .expect("move");
        assert_eq!(
            req,
            OpRequestDto::Move {
                src: "/a".into(),
                dst: "/b".into()
            }
        );
        let req: OpRequestDto = serde_json::from_value(serde_json::json!({
            "op": "trash", "src": "/a",
        }))
        .expect("trash");
        assert_eq!(req, OpRequestDto::Trash { src: "/a".into() });
        let req: OpRequestDto = serde_json::from_value(serde_json::json!({
            "op": "delete", "src": "/a", "recursive": true,
        }))
        .expect("delete");
        assert_eq!(
            req,
            OpRequestDto::Delete {
                src: "/a".into(),
                recursive: true
            }
        );
        // `recursive` defaults to false: a bare delete never takes a tree.
        let req: OpRequestDto = serde_json::from_value(serde_json::json!({
            "op": "delete", "src": "/a",
        }))
        .expect("delete default");
        assert_eq!(
            req,
            OpRequestDto::Delete {
                src: "/a".into(),
                recursive: false
            }
        );
    }

    #[test]
    fn op_event_dto_wire_tags() {
        let v = serde_json::to_value(OpEventDto::Progress {
            id: 7,
            phase: OpPhase::Copying,
            done_bytes: 123,
            total_bytes: 456,
            done_items: 1,
            total_items: 9,
        })
        .expect("serialize");
        assert_eq!(
            v,
            serde_json::json!({
                "type": "progress", "id": 7, "phase": "copying",
                "doneBytes": 123, "totalBytes": 456,
                "doneItems": 1, "totalItems": 9,
            })
        );
        let v = serde_json::to_value(OpEventDto::Progress {
            id: 7,
            phase: OpPhase::Measuring,
            done_bytes: 0,
            total_bytes: 0,
            done_items: 0,
            total_items: 0,
        })
        .expect("serialize");
        assert_eq!(v["phase"], serde_json::json!("measuring"));
        let v = serde_json::to_value(OpEventDto::Done {
            id: 7,
            summary: "copied report.pdf".to_string(),
        })
        .expect("serialize");
        assert_eq!(
            v,
            serde_json::json!({"type": "done", "id": 7, "summary": "copied report.pdf"})
        );
        // The error shape is nested per the frozen WatchEventDto contract.
        let err = CmdError::custom("watch", Some(Path::new("/tmp/x")), "gone".to_string());
        let v = serde_json::to_value(OpEventDto::Error { id: 7, error: err }).expect("serialize");
        assert_eq!(v["type"], serde_json::json!("error"));
        assert_eq!(v["id"], serde_json::json!(7));
        assert_eq!(v["error"]["kind"], serde_json::json!("watch"));
    }

    // ------------------------------------------------------------------
    // the golden fixture (the Rust half of the wire-seam test)
    //
    // The defining near-miss in this codebase was a UI that ran entirely on
    // `MockIpc` and looked correct on screen while the real seam was never
    // exercised. `tsc` cannot catch a wire mismatch — TypeScript types are
    // erased at runtime, so a `doneBytes` → `done_bytes` rename in the
    // serializer compiles cleanly and then silently zeroes every progress bar.
    //
    // So the exact bytes `Serialize` produces are committed to
    // `ui/test/fixtures/op-events.json` and asserted here. The frontend half
    // (`ui/test/op-seam.test.ts`) reads the same file and pushes those bytes
    // through the real `normalizeOpEvent` / `OpManager`. Rename a field, flip
    // a tag's casing, or flatten the nested `error`, and *both* halves fail.
    //
    // Regenerate deliberately — never to make a failing test pass:
    //
    // ```sh
    // KESTREL_UPDATE_GOLDEN=1 cargo test -p kestrel-tauri --lib op_event_bytes_match_the_golden_fixture
    // ```
    // ------------------------------------------------------------------

    /// Path to the committed golden, shared with the frontend test.
    fn golden_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../ui/test/fixtures/op-events.json")
    }

    /// One committed case: a stable name plus the **exact** bytes the
    /// serializer emits. The bytes are the point — comparing parsed
    /// `serde_json::Value`s would tolerate key reordering that the frontend
    /// never sees and could hide a rename behind a cosmetic diff.
    #[derive(Debug, serde::Deserialize, serde::Serialize)]
    struct GoldenCase {
        /// Why the case exists; shows up in the failure message.
        name: String,
        /// The literal JSON the webview would receive and `JSON.parse`.
        json: String,
    }

    /// Every `OpEventDto` variant and every error shape the op layer can
    /// produce, as the real `Serialize` output.
    fn golden_cases() -> Vec<GoldenCase> {
        let mut cases: Vec<GoldenCase> = Vec::new();
        let mut push = |name: &str, dto: &OpEventDto| {
            cases.push(GoldenCase {
                name: name.to_string(),
                json: serde_json::to_string(dto).expect("OpEventDto must serialize"),
            });
        };

        push(
            "progress_copying_with_known_totals",
            &OpEventDto::Progress {
                id: 7,
                phase: OpPhase::Copying,
                done_bytes: 123,
                total_bytes: 456,
                done_items: 1,
                total_items: 9,
            },
        );
        push(
            // The measuring phase has no total yet. A guessed percentage here
            // would be a lie, so the frontend must render indeterminate.
            "progress_measuring_with_unknown_totals",
            &OpEventDto::Progress {
                id: 7,
                phase: OpPhase::Measuring,
                done_bytes: 0,
                total_bytes: 0,
                done_items: 3,
                total_items: 0,
            },
        );
        push(
            // A delete counts entries; inventing a byte total is forbidden.
            "progress_deleting_has_no_byte_total",
            &OpEventDto::Progress {
                id: 7,
                phase: OpPhase::Deleting,
                done_bytes: 0,
                total_bytes: 0,
                done_items: 42,
                total_items: 42,
            },
        );
        push(
            // > 2^32 bytes: the JS side must not lose precision on a 4 GB+ file.
            "progress_beyond_u32_byte_counts",
            &OpEventDto::Progress {
                id: u64::from(u32::MAX) + 1,
                phase: OpPhase::Copying,
                done_bytes: 5_368_709_120,
                total_bytes: 8_589_934_592,
                done_items: 4_294_967_296,
                total_items: 4_294_967_296,
            },
        );
        push(
            "done_names_the_file",
            &OpEventDto::Done {
                id: 7,
                summary: "copied report.pdf".to_string(),
            },
        );
        push(
            // A user cancel arrives as a terminal `error` with kind
            // `cancelled`, nested exactly like `WatchEventDto::Error`. The
            // frontend routes this to `cancelled`, not to `failed`.
            "error_cancelled_for_a_user_cancel",
            &OpEventDto::Error {
                id: 7,
                error: CmdError::from_kestrel(&KestrelError::Cancelled),
            },
        );
        push(
            // Phase 3a has no collision dialog: a collision is an error and the
            // destination is left untouched.
            "error_already_exists_for_a_collision",
            &OpEventDto::Error {
                id: 9,
                error: CmdError::from_kestrel(&KestrelError::AlreadyExists {
                    path: "/tmp/dst.txt".into(),
                }),
            },
        );
        push(
            // `path: null` must stay null, not become the string "null".
            "error_not_found_with_a_path",
            &OpEventDto::Error {
                id: 11,
                error: CmdError::from_kestrel(&KestrelError::not_found("/nope")),
            },
        );
        push(
            // The `#[non_exhaustive]` fallback. It must still parse as an op
            // error rather than dropping the event on the floor.
            "error_unknown_from_the_non_exhaustive_fallback",
            &OpEventDto::Error {
                id: 13,
                error: CmdError::custom("unknown", None, "a future engine error".to_string()),
            },
        );
        cases
    }

    /// THE Rust half of the wire-seam test: these exact bytes are what the
    /// frontend will be handed, so they are committed and asserted.
    #[test]
    fn op_event_bytes_match_the_golden_fixture() {
        let path = golden_path();
        let cases = golden_cases();
        if std::env::var_os("KESTREL_UPDATE_GOLDEN").is_some() {
            let json = serde_json::to_string_pretty(&cases).expect("golden serializes");
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("fixture dir");
            }
            std::fs::write(&path, json + "\n").expect("write golden fixture");
            eprintln!("wrote {} cases to {}", cases.len(), path.display());
            return;
        }
        let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "missing golden fixture {}: {e}\n\
                 regenerate with KESTREL_UPDATE_GOLDEN=1 cargo test -p kestrel-tauri \
                 --lib op_event_bytes_match_the_golden_fixture",
                path.display()
            )
        });
        let committed: Vec<GoldenCase> =
            serde_json::from_str(&raw).expect("golden fixture must be valid JSON");
        assert_eq!(
            committed.len(),
            cases.len(),
            "the golden fixture covers {} cases but the serializer produces {} — \
             add the new shape to golden_cases() and regenerate",
            committed.len(),
            cases.len()
        );
        for (want, got) in cases.iter().zip(committed.iter()) {
            assert_eq!(want.name, got.name, "golden case names drifted");
            assert_eq!(
                want.json, got.json,
                "the wire bytes for `{}` changed. The frontend normaliser \
                 (ui/src/lib/ipc.ts) reads this exact text, so a mismatch here \
                 is a production break, not a cosmetic diff. If the change is \
                 intended, update MIGRATION.md §3 and regenerate with \
                 KESTREL_UPDATE_GOLDEN=1.",
                want.name
            );
        }
    }
}
