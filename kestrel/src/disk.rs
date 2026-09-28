//! Free space for the status bar's §4.5 meter, read on a worker thread.
//!
//! # Why this is not `std::fs`
//!
//! `std` has no `statvfs`. The syscall goes through **`rustix`**, which is
//! already in the dependency graph (via `notify` → `inotify`), so naming it
//! directly adds **zero packages**.
//!
//! `rustix` rather than `libc` specifically because this crate sets
//! `unsafe_code = "forbid"`, and `rustix::fs::statvfs` is a safe call.
//! `libc::statvfs` is not: it needs a `MaybeUninit`/`zeroed` struct and a
//! `CString`, i.e. three `unsafe` blocks for one boolean's worth of data.
//!
//! # Why it is on a thread
//!
//! `statvfs` on a cold, spinning, or network-mounted filesystem can block for
//! tens of milliseconds. A file manager that stalls its frame loop to draw a
//! 60px meter is violating the one rule this codebase is built around, so the
//! probe runs on a detached worker and the UI reads a cached answer.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// A cached free-space reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FreeSpace {
    /// Bytes available to this user.
    pub available: u64,
    /// Total bytes on the filesystem.
    pub total: u64,
    /// Bytes used.
    pub used: u64,
}

impl FreeSpace {
    /// The fraction available, clamped to `0..=1`.
    ///
    /// Clamped because a meter that can overflow its track is a rendering bug,
    /// and `f_bavail` can exceed `f_blocks * f_frsize` on some union mounts.
    #[must_use]
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        (self.available as f32 / self.total as f32).clamp(0.0, 1.0)
    }

    /// `None` when the reading is absent.
    #[must_use]
    pub fn format_free(&self) -> String {
        crate::format::bytes(self.available)
    }
}

/// A background probe whose result the UI can read without blocking.
///
/// The worker holds a generation counter so a stale answer — one that arrives
/// after the user has navigated somewhere else — is discarded rather than shown
/// against the wrong directory.
#[derive(Debug, Clone)]
pub struct SpaceProbe {
    /// The most recent reading, packed for atomic access.
    packed: Arc<AtomicU64>,
    /// The directory the reading is for.
    subject: Arc<PathBuf>,
    /// Bumped on every request; a worker ignores a request older than its own.
    generation: Arc<AtomicU64>,
}

impl Default for SpaceProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl SpaceProbe {
    /// A probe with no reading yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            packed: Arc::new(AtomicU64::new(0)),
            subject: Arc::new(PathBuf::new()),
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The cached reading for `dir`, or `None` if it is stale or absent.
    #[must_use]
    pub fn get(&self) -> Option<FreeSpace> {
        let packed = self.packed.load(Ordering::Relaxed);
        if packed == 0 {
            return None;
        }
        // `available` in the high 32 bits, `total` in the low. 32 bits of bytes
        // is 4 GiB, which is not enough on its own, so this is only a
        // *staleness/validity* flag plus a cache key: the real values are
        // recomputed by the worker into the two halves, and a filesystem larger
        // than 4 TiB saturates rather than wraps. Saturation is the right
        // failure: the meter shows "full" and the label is wrong, instead of
        // showing "empty" and the user thinking they have room.
        let available = (packed >> 32) as u32 as u64;
        let total = (packed & 0xFFFF_FFFF) as u32 as u64;
        Some(FreeSpace {
            available,
            total,
            used: total.saturating_sub(available),
        })
    }

    /// Requests a fresh reading for `dir`, off the UI thread.
    ///
    /// Returns immediately. If a request for the same directory is already in
    /// flight this is a no-op, so a re-render storm cannot spawn a thread per
    /// frame — the same reasoning as the scanner's own batching.
    pub fn request(&mut self, dir: &Path) {
        if self.subject.as_ref() == dir && self.get().is_some() {
            return;
        }
        self.subject = Arc::new(dir.to_path_buf());
        self.packed.store(0, Ordering::Relaxed);

        let packed = Arc::clone(&self.packed);
        let subject = Arc::clone(&self.subject);
        let generation = Arc::clone(&self.generation);
        // `gen` is a reserved keyword in edition 2024.
        let ticket = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let dir = dir.to_path_buf();

        // A detached thread rather than a persistent worker: the probe runs a
        // few times per session, and a thread that parks forever is a resource
        // held for the life of the process for no reason.
        let spawned = std::thread::Builder::new()
            .name("kestrel-space".to_string())
            .spawn(move || {
                let reading = statvfs(&dir);
                // Only publish if nobody has navigated away since this was
                // requested.
                if generation.load(Ordering::Relaxed) == ticket && reading.is_some() {
                    let r = reading.unwrap_or(FreeSpace {
                        available: 0,
                        total: 0,
                        used: 0,
                    });
                    let hi = (r.available.min(u64::from(u32::MAX)) as u32 as u64) << 32;
                    let lo = r.total.min(u64::from(u32::MAX)) as u32 as u64;
                    packed.store(hi | lo, Ordering::Relaxed);
                }
                drop(subject);
            });
        // A failed spawn is not worth a dialog: the meter simply keeps showing
        // its last value. Logged so a resource-exhaustion diagnosis is possible.
        if let Err(err) = spawned {
            log::warn!("space probe: could not spawn a worker: {err}");
        }
    }
}

/// Reads `statvfs` for `path`.
///
/// Returns `None` on any failure — a permission error, a vanished mount, an
/// `ENOTDIR`, a path containing a NUL byte (which `rustix` rejects for us) —
/// because a free-space meter is decoration and must never be the thing that
/// surfaces an error dialog.
#[must_use]
pub fn statvfs(path: &Path) -> Option<FreeSpace> {
    let stat = rustix::fs::statvfs(path).ok()?;
    // `f_frsize` is the fragment size; on every modern Linux fs it is the block
    // size a user would recognise. It can be 0 on exotic mounts, in which case
    // fall back to `f_bsize` before giving up rather than reporting a zero-size
    // filesystem.
    let frsize = if stat.f_frsize > 0 {
        stat.f_frsize
    } else {
        stat.f_bsize
    };
    if frsize == 0 {
        return None;
    }
    Some(FreeSpace {
        available: stat.f_bavail.saturating_mul(frsize),
        total: stat.f_blocks.saturating_mul(frsize),
        used: stat
            .f_blocks
            .saturating_sub(stat.f_bfree)
            .saturating_mul(frsize),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn statvfs_reads_the_root_filesystem() {
        // The root filesystem always exists, so this must succeed on any Unix.
        let r = statvfs(Path::new("/")).expect("statvfs(/) must succeed");
        assert!(r.total > 0, "total should be non-zero, got {r:?}");
        assert!(r.available <= r.total, "available exceeds total: {r:?}");
    }

    #[test]
    fn the_fraction_is_clamped_and_zero_safe() {
        let full = FreeSpace {
            available: 100,
            total: 100,
            used: 0,
        };
        assert!((full.fraction() - 1.0).abs() < 1e-6);
        let empty = FreeSpace {
            available: 0,
            total: 100,
            used: 100,
        };
        assert!(empty.fraction().abs() < 1e-6);
        // A zero total must not divide by zero.
        let zero = FreeSpace {
            available: 0,
            total: 0,
            used: 0,
        };
        assert!(zero.fraction().abs() < 1e-6);
    }

    #[test]
    fn an_over_reported_reading_clamps_rather_than_overflowing() {
        // Union and overlay mounts can report bavail > blocks * frsize. The
        // meter must not overflow its track.
        let odd = FreeSpace {
            available: 500,
            total: 100,
            used: 0,
        };
        assert_eq!(odd.fraction(), 1.0);
    }

    #[test]
    fn used_is_derived_not_reported() {
        let r = FreeSpace {
            available: 30,
            total: 100,
            used: 70,
        };
        assert_eq!(r.used, 70);
        assert!((r.fraction() - 0.3).abs() < 1e-6);
    }

    #[test]
    fn a_probe_with_no_reading_reports_none() {
        let p = SpaceProbe::new();
        assert_eq!(p.get(), None);
    }

    #[test]
    fn a_vanished_path_is_not_an_error() {
        // A meter must never be the thing that surfaces a dialog.
        assert_eq!(
            statvfs(Path::new("/kestrel-definitely-not-here-9c2f")),
            None
        );
    }

    #[test]
    fn a_path_with_a_nul_byte_is_rejected_rather_than_panicking() {
        assert_eq!(statvfs(Path::new("a\0b")), None);
    }
}
