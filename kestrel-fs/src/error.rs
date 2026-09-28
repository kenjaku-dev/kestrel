//! Typed errors for the Kestrel filesystem engine.
//!
//! # Why a domain error type
//!
//! A file manager meets hostile input constantly: half of `/home` is
//! unreadable, paths exceed `PATH_MAX`, symlinks dangle, mounts disappear
//! mid-traverse. None of that is exceptional, it is *data*. Every fallible
//! operation in this crate therefore returns [`KestrelError`] (aliased as
//! [`Result`]) and callers decide how to surface it — usually as a row in an
//! error list, never as a panic.
//!
//! [`std::io::ErrorKind`] is deliberately coarse, so [`KestrelError`] keeps the
//! offending [`PathBuf`] and the original [`std::io::Error`] around for
//! inspection while presenting a message a human can act on.
//!
//! ```
//! use kestrel_fs::error::{KestrelError, classify_io};
//! use std::io;
//! use std::path::Path;
//!
//! // Every `io::ErrorKind` this engine cares about round-trips into a variant.
//! let err = classify_io(
//!     Path::new("/etc/shadow"),
//!     io::Error::from(io::ErrorKind::PermissionDenied),
//! );
//! assert!(matches!(err, KestrelError::PermissionDenied { .. }));
//! assert_eq!(err.path(), Some(std::path::Path::new("/etc/shadow")));
//! ```

use std::error::Error as StdError;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// Convenient result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, KestrelError>;

/// An error produced by the Kestrel filesystem engine.
///
/// Variants fall into three groups:
///
/// * **Kernel errors** — [`PermissionDenied`](Self::PermissionDenied),
///   [`NotFound`](Self::NotFound), [`CrossesDevices`](Self::CrossesDevices),
///   [`FilesystemLoop`](Self::FilesystemLoop) and friends. These wrap the
///   originating [`io::Error`] in `source`.
/// * **Engine errors** — conditions this crate detects itself, such as a
///   symlink loop caught by canonical-path bookkeeping
///   ([`LoopDetected`](Self::LoopDetected)) or a copy that would recurse into
///   its own destination ([`InvalidInput`](Self::InvalidInput)).
/// * **Control flow** — [`Cancelled`](Self::Cancelled), which is returned when
///   a [`CancellationToken`](crate::model::CancellationToken) was tripped.
#[derive(Debug)]
#[non_exhaustive]
pub enum KestrelError {
    /// The path exists but the process is not allowed to read or change it.
    PermissionDenied {
        /// The path that was refused.
        path: PathBuf,
        /// Underlying OS error.
        source: io::Error,
    },
    /// The path does not exist, or vanished while we were traversing.
    NotFound {
        /// The missing path.
        path: PathBuf,
        /// Underlying OS error.
        source: io::Error,
    },
    /// A destination already exists and the caller did not opt in to clobbering.
    AlreadyExists {
        /// The colliding path.
        path: PathBuf,
    },
    /// The name or a full path component exceeded the OS limit (`ENAMETOOLONG`).
    InvalidFilename {
        /// The offending path.
        path: PathBuf,
        /// Underlying OS error.
        source: io::Error,
    },
    /// The kernel refused a symlink chain as too deep (`ELOOP`).
    FilesystemLoop {
        /// The offending path.
        path: PathBuf,
        /// Underlying OS error.
        source: io::Error,
    },
    /// The engine detected a traversal loop itself (a symlink pointing at an
    /// already-visited canonical directory) and stopped descending.
    ///
    /// Distinct from [`FilesystemLoop`](Self::FilesystemLoop): the kernel never
    /// saw the loop, we did.
    LoopDetected {
        /// The path that would have re-entered an already-visited directory.
        path: PathBuf,
    },
    /// The operation cannot be performed across a mount point (`EXDEV`).
    ///
    /// [`move_`](crate::ops::move_) handles this internally by falling back to
    /// copy-then-delete; the variant exists so callers can react if they ever
    /// see it.
    CrossesDevices {
        /// The offending path.
        path: PathBuf,
        /// Underlying OS error.
        source: io::Error,
    },
    /// A path component that should have been a directory was not (`ENOTDIR`).
    NotADirectory {
        /// The offending path.
        path: PathBuf,
        /// Underlying OS error.
        source: io::Error,
    },
    /// A directory was used where a file was expected (`EISDIR`).
    IsADirectory {
        /// The offending path.
        path: PathBuf,
        /// Underlying OS error.
        source: io::Error,
    },
    /// A non-empty directory was passed to a non-recursive delete.
    DirectoryNotEmpty {
        /// The offending path.
        path: PathBuf,
        /// Underlying OS error.
        source: io::Error,
    },
    /// The engine rejected the request before touching the filesystem.
    ///
    /// Used for conditions the OS cannot express, e.g. copying a directory into
    /// one of its own descendants.
    InvalidInput {
        /// The offending path.
        path: PathBuf,
        /// Human-readable explanation.
        reason: String,
    },
    /// The worker's [`CancellationToken`](crate::model::CancellationToken) was
    /// tripped before the operation completed.
    Cancelled,
    /// The platform trash implementation refused the item.
    Trash {
        /// The path that could not be trashed.
        path: PathBuf,
        /// The `trash` crate's boxed error.
        source: Box<dyn StdError + Send + Sync>,
    },
    /// The filesystem watcher failed (backend unavailable, watch limit hit...).
    Watch {
        /// The path being watched, if known.
        path: Option<PathBuf>,
        /// Human-readable explanation.
        message: String,
    },
    /// Any other I/O failure, tagged with the path it happened on.
    Io {
        /// The path involved, if one is known.
        path: Option<PathBuf>,
        /// Underlying OS error.
        source: io::Error,
    },
}

impl KestrelError {
    /// Builds a `NotFound` error for `path` without an underlying `io::Error`.
    ///
    /// Useful when reporting a race we detected ourselves (a directory that
    /// disappeared between `read_dir` and `read_dir`).
    pub fn not_found(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let source = io::Error::new(io::ErrorKind::NotFound, "path not found");
        Self::NotFound { path, source }
    }

    /// The path this error is about, when the variant carries one.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::PermissionDenied { path, .. }
            | Self::NotFound { path, .. }
            | Self::AlreadyExists { path }
            | Self::InvalidFilename { path, .. }
            | Self::FilesystemLoop { path, .. }
            | Self::LoopDetected { path }
            | Self::CrossesDevices { path, .. }
            | Self::NotADirectory { path, .. }
            | Self::IsADirectory { path, .. }
            | Self::DirectoryNotEmpty { path, .. }
            | Self::InvalidInput { path, .. }
            | Self::Trash { path, .. } => Some(path),
            Self::Cancelled => None,
            Self::Watch { path, .. } => path.as_deref(),
            Self::Io { path, .. } => path.as_deref(),
        }
    }

    /// `true` when the error means "the user is not allowed to do this", which
    /// the UI usually renders as a dimmed row rather than an error dialog.
    pub fn is_permission_denied(&self) -> bool {
        matches!(self, Self::PermissionDenied { .. })
    }

    /// `true` when the error means "this path does not exist (any more)".
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound { .. })
    }

    /// The `io::ErrorKind` behind this error, if there is one.
    pub fn io_kind(&self) -> Option<io::ErrorKind> {
        let source = match self {
            Self::PermissionDenied { source, .. }
            | Self::NotFound { source, .. }
            | Self::InvalidFilename { source, .. }
            | Self::FilesystemLoop { source, .. }
            | Self::CrossesDevices { source, .. }
            | Self::NotADirectory { source, .. }
            | Self::IsADirectory { source, .. }
            | Self::DirectoryNotEmpty { source, .. }
            | Self::Io { source, .. } => Some(source),
            _ => None,
        };
        source.map(io::Error::kind)
    }
}

impl fmt::Display for KestrelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PermissionDenied { path, .. } => {
                write!(f, "permission denied: {}", path.display())
            }
            Self::NotFound { path, .. } => {
                write!(f, "no such file or directory: {}", path.display())
            }
            Self::AlreadyExists { path } => write!(f, "already exists: {}", path.display()),
            Self::InvalidFilename { path, .. } => {
                write!(f, "invalid or too long filename: {}", path.display())
            }
            Self::FilesystemLoop { path, .. } => {
                write!(f, "too many levels of symbolic links: {}", path.display())
            }
            Self::LoopDetected { path } => write!(
                f,
                "skipped {}: symlink loop into an already-visited directory",
                path.display()
            ),
            Self::CrossesDevices { path, .. } => {
                write!(f, "crosses filesystem boundary: {}", path.display())
            }
            Self::NotADirectory { path, .. } => write!(f, "not a directory: {}", path.display()),
            Self::IsADirectory { path, .. } => write!(f, "is a directory: {}", path.display()),
            Self::DirectoryNotEmpty { path, .. } => {
                write!(f, "directory not empty: {}", path.display())
            }
            Self::InvalidInput { path, reason } => {
                write!(f, "invalid operation on {}: {reason}", path.display())
            }
            Self::Cancelled => write!(f, "operation cancelled"),
            Self::Trash { path, .. } => write!(f, "could not move {} to the trash", path.display()),
            Self::Watch {
                path: Some(path),
                message,
            } => {
                write!(f, "watch failed for {}: {message}", path.display())
            }
            Self::Watch {
                path: None,
                message,
            } => write!(f, "watch failed: {message}"),
            Self::Io {
                path: Some(path),
                source,
            } => write!(f, "{}: {source}", path.display()),
            Self::Io { path: None, source } => write!(f, "{source}"),
        }
    }
}

impl StdError for KestrelError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::PermissionDenied { source, .. }
            | Self::NotFound { source, .. }
            | Self::InvalidFilename { source, .. }
            | Self::FilesystemLoop { source, .. }
            | Self::CrossesDevices { source, .. }
            | Self::NotADirectory { source, .. }
            | Self::IsADirectory { source, .. }
            | Self::DirectoryNotEmpty { source, .. }
            | Self::Io { source, .. } => Some(source),
            Self::Trash { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}

/// A failure attached to a specific path, as reported by a scan.
///
/// Scans never abort because one path is unreadable; they emit one of these and
/// carry on, which is what lets a listing of `/` show thousands of rows plus a
/// handful of "permission denied" lines instead of nothing at all.
#[derive(Debug)]
pub struct ScanError {
    /// The path that could not be read.
    pub path: PathBuf,
    /// What went wrong.
    pub error: KestrelError,
}

impl ScanError {
    /// Pairs a path with an error.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>, error: KestrelError) -> Self {
        Self {
            path: path.into(),
            error,
        }
    }

    /// `true` when the path is merely inaccessible rather than actually broken.
    #[must_use]
    pub fn is_permission_denied(&self) -> bool {
        self.error.is_permission_denied()
    }
}

impl fmt::Display for ScanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.error)
    }
}

impl StdError for ScanError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(&self.error)
    }
}

/// A non-recursive listing attempt where the path is not a directory.
///
/// Distinguishing this from a generic I/O error is what lets the UI say
/// "that is a file, not a folder" instead of showing a bare `errno`.
pub fn not_a_directory(path: &Path, source: io::Error) -> KestrelError {
    KestrelError::NotADirectory {
        path: path.to_path_buf(),
        source,
    }
}

/// Maps an [`io::Error`] plus the path it happened on into a [`KestrelError`].
///
/// The mapping prefers [`io::ErrorKind`] (stable and portable) and only falls
/// back to raw `errno` values, which differ between platforms, as a last
/// resort.
///
/// ```
/// use kestrel_fs::error::classify_io;
/// use std::io;
/// use std::path::Path;
///
/// let raw = io::Error::from_raw_os_error(18); // EXDEV on Linux
/// match classify_io(Path::new("/mnt/x"), raw) {
///     kestrel_fs::KestrelError::CrossesDevices { .. } => {}
///     other => panic!("expected CrossesDevices, got {other}"),
/// }
/// ```
pub fn classify_io(path: &Path, source: io::Error) -> KestrelError {
    // Linux/BSD `errno` values. Only consulted when the kind is not specific
    // enough, because `ErrorKind` is portable and these are not. (There is no
    // stable `ErrorKind` for ELOOP yet, so the errno check is the only route.)
    const EXDEV: i32 = 18;
    const ELOOP: i32 = 40;
    const ENAMETOOLONG: i32 = 36;

    let raw = source.raw_os_error();
    match source.kind() {
        io::ErrorKind::PermissionDenied => KestrelError::PermissionDenied {
            path: path.to_path_buf(),
            source,
        },
        io::ErrorKind::NotFound => KestrelError::NotFound {
            path: path.to_path_buf(),
            source,
        },
        io::ErrorKind::AlreadyExists => KestrelError::AlreadyExists {
            path: path.to_path_buf(),
        },
        io::ErrorKind::CrossesDevices => KestrelError::CrossesDevices {
            path: path.to_path_buf(),
            source,
        },
        io::ErrorKind::InvalidFilename => KestrelError::InvalidFilename {
            path: path.to_path_buf(),
            source,
        },
        io::ErrorKind::NotADirectory => KestrelError::NotADirectory {
            path: path.to_path_buf(),
            source,
        },
        io::ErrorKind::IsADirectory => KestrelError::IsADirectory {
            path: path.to_path_buf(),
            source,
        },
        io::ErrorKind::DirectoryNotEmpty => KestrelError::DirectoryNotEmpty {
            path: path.to_path_buf(),
            source,
        },
        _ => match raw {
            Some(EXDEV) => KestrelError::CrossesDevices {
                path: path.to_path_buf(),
                source,
            },
            Some(ELOOP) => KestrelError::FilesystemLoop {
                path: path.to_path_buf(),
                source,
            },
            Some(ENAMETOOLONG) => KestrelError::InvalidFilename {
                path: path.to_path_buf(),
                source,
            },
            _ => KestrelError::Io {
                path: Some(path.to_path_buf()),
                source,
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_common_kinds() {
        /// (kind, predicate that must match the mapped variant)
        type Case = (io::ErrorKind, fn(&KestrelError) -> bool);
        let cases: Vec<Case> = vec![
            (io::ErrorKind::PermissionDenied, |e| {
                matches!(e, KestrelError::PermissionDenied { .. })
            }),
            (io::ErrorKind::NotFound, |e| {
                matches!(e, KestrelError::NotFound { .. })
            }),
            (io::ErrorKind::AlreadyExists, |e| {
                matches!(e, KestrelError::AlreadyExists { .. })
            }),
            (io::ErrorKind::CrossesDevices, |e| {
                matches!(e, KestrelError::CrossesDevices { .. })
            }),
            (io::ErrorKind::InvalidFilename, |e| {
                matches!(e, KestrelError::InvalidFilename { .. })
            }),
        ];
        for (kind, check) in cases {
            let err = classify_io(Path::new("/x"), io::Error::from(kind));
            assert!(check(&err), "{kind:?} mapped to {err}");
        }
    }

    #[test]
    fn falls_back_to_errno_when_kind_is_generic() {
        let err = classify_io(Path::new("/x"), io::Error::from_raw_os_error(18));
        assert!(matches!(err, KestrelError::CrossesDevices { .. }));
        let err = classify_io(Path::new("/x"), io::Error::from_raw_os_error(36));
        assert!(matches!(err, KestrelError::InvalidFilename { .. }));
        // ELOOP has no stable `ErrorKind` yet, so the errno path must catch it.
        let err = classify_io(Path::new("/x"), io::Error::from_raw_os_error(40));
        assert!(matches!(err, KestrelError::FilesystemLoop { .. }));
    }

    #[test]
    fn unmapped_errors_keep_their_path() {
        let err = classify_io(Path::new("/x"), io::Error::from(io::ErrorKind::BrokenPipe));
        assert_eq!(err.path(), Some(Path::new("/x")));
        assert_eq!(err.io_kind(), Some(io::ErrorKind::BrokenPipe));
        assert!(!err.is_permission_denied());
    }

    #[test]
    fn source_chain_is_preserved() {
        let err = classify_io(Path::new("/x"), io::Error::from(io::ErrorKind::NotFound));
        assert!(StdError::source(&err).is_some());
    }
}
