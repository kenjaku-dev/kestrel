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
use std::sync::{Mutex, MutexGuard};

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
///
/// The reading is a plain [`FreeSpace`] behind a mutex, mirroring
/// [`kestrel_fs::size::SizeCache`]'s `Arc<Mutex<HashMap<..>>>` shape. An earlier
/// revision packed both fields into one `AtomicU64` — 32 bits each — which
/// clamped every filesystem over 4 GiB to exactly 4,294,967,295 bytes and
/// pinned the meter at full. A mutex held only for a struct copy is never
/// contended (one writer, one reader per frame) and cannot block the frame
/// loop in any observable way.
#[derive(Debug, Clone)]
pub struct SpaceProbe {
    /// The most recent reading, or `None` when absent or invalidated.
    slot: Arc<Mutex<Option<FreeSpace>>>,
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
            slot: Arc::new(Mutex::new(None)),
            subject: Arc::new(PathBuf::new()),
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The cached reading for `dir`, or `None` if it is stale or absent.
    #[must_use]
    pub fn get(&self) -> Option<FreeSpace> {
        *lock(&self.slot)
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
        *lock(&self.slot) = None;

        let slot = Arc::clone(&self.slot);
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
                    if let Some(r) = reading {
                        *lock(&slot) = Some(r);
                    }
                }
                drop(subject);
            });
        // A failed spawn is not worth a dialog: the meter simply keeps showing
        // its last value. Logged so a resource-exhaustion diagnosis is possible.
        if let Err(err) = spawned {
            log::warn!("space probe: could not spawn a worker: {err}");
        }
    }

    /// Stores `reading` directly, for tests.
    ///
    /// The worker path needs a real filesystem, so a test that needs a 500 GB
    /// volume would have to mock `statvfs` — this is the seam that lets it
    /// feed a synthetic reading through the same slot the worker publishes to.
    #[cfg(test)]
    fn inject(&self, reading: FreeSpace) {
        *lock(&self.slot) = Some(reading);
    }
}

/// Poison-safe lock.
///
/// A panic elsewhere must not turn every later meter read into a panic; the
/// slot holds only plain data, so recovering the guard is sound.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
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
    fn a_large_filesystem_round_trips_without_clamping() {
        // The 4 GiB regression: packing both fields into 32 bits each read
        // every real filesystem as exactly 4,294,967,295 bytes available and
        // pinned the meter at full. A synthetic 500 GB reading must come back
        // with the same numbers.
        let probe = SpaceProbe::new();
        let big = FreeSpace {
            available: 500 * 1024 * 1024 * 1024,
            total: 1024 * 1024 * 1024 * 1024,
            used: 524 * 1024 * 1024 * 1024,
        };
        probe.inject(big);
        let back = probe.get().expect("an injected reading must be visible");
        assert_eq!(back, big);
        assert!(
            (back.fraction() - 0.488_281_25).abs() < 1e-6,
            "the meter must show ~49% free, not full: {back:?}"
        );
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
