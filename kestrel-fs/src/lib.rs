//! # kestrel-fs
//!
//! The filesystem engine behind [Kestrel](https://example.invalid/kestrel), a
//! file manager. It is a plain library with **no UI dependency of any kind**,
//! so every behaviour here is testable headless — no GPU, no display server, no
//! event loop.
//!
//! ## The rules this crate is built around
//!
//! 1. **Never block the UI thread.** Everything that can take longer than a
//!    frame ([`scan::start`], [`size::start`], the [`watcher`]) runs on a worker
//!    thread and reports back over a channel. Each job hands out a
//!    [`model::CancellationToken`] so the user can abandon it.
//! 2. **Deliver incrementally.** [`scan`] emits [`scan::ScanEvent`]s as
//!    directories are read, batched with [`scan::ScanEvent::BatchEnd`], so the
//!    list paints immediately.
//! 3. **Sizes are lazy and cancellable.** Scanning never walks a tree to add
//!    up bytes; that is [`size::start`], on demand, cached by
//!    [`size::SizeCache`].
//! 4. **Errors are data.** Every failure is a [`error::KestrelError`] attached
//!    to the path that caused it. There is not a single `unwrap` on a
//!    filesystem call in this crate, and an unreadable directory is reported as
//!    one row in an error list, never a panic.
//!
//! ## Module map
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | [`model`] | [`model::FileEntry`], [`model::EntryKind`], hidden-file rules, sorting, [`model::CancellationToken`] |
//! | [`scan`] | directory traversal, loop protection, incremental events |
//! | [`size`] | recursive, cancellable, cached size computation |
//! | [`watcher`] | debounced filesystem watching |
//! | [`ops`] | copy / move / delete / trash |
//! | [`error`] | the typed error enum and `io::ErrorKind` mapping |
//!
//! ## Worked example: a directory view
//!
//! ```no_run
//! use kestrel_fs::error::ScanError;
//! use kestrel_fs::model::{FileEntry, SortSpec};
//! use kestrel_fs::scan::{ScanEvent, ScanOptions, start};
//! use kestrel_fs::size::{SizeCache, SizeEvent, start as start_size};
//! use kestrel_fs::watcher::{WatchEvent, watch};
//! use std::path::{Path, PathBuf};
//! use std::time::Duration;
//!
//! struct View {
//!     dir: PathBuf,
//!     rows: Vec<FileEntry>,
//!     errors: Vec<ScanError>,
//!     scan: Option<kestrel_fs::scan::ScanHandle>,
//!     sizes: SizeCache,
//! }
//!
//! impl View {
//!     fn open(dir: &Path) -> std::io::Result<Self> {
//!         Ok(Self {
//!             dir: dir.to_path_buf(),
//!             rows: Vec::new(),
//!             errors: Vec::new(),
//!             scan: Some(start(dir, ScanOptions::listing())?),
//!             sizes: SizeCache::new(),
//!         })
//!     }
//!
//!     /// Called once per UI frame. Does no filesystem work.
//!     fn pump(&mut self) {
//!         // Take the handle out for the duration of the drain so the sink can
//!         // also clear `self.scan` when the walk completes.
//!         let Some(mut scan) = self.scan.take() else { return };
//!         let mut finished = false;
//!         scan.drain_into(|event| match event {
//!             ScanEvent::Entry(entry) => self.rows.push(entry),
//!             ScanEvent::Error(err) => self.errors.push(err),
//!             ScanEvent::Complete { .. } => finished = true,
//!             ScanEvent::BatchEnd => {}
//!         });
//!         if !finished {
//!             self.scan = Some(scan);
//!         }
//!     }
//!
//!     /// "Show size" on a selected row: cheap to ask for, never blocks.
//!     fn request_size(&self, row: &FileEntry) {
//!         if !row.kind.is_directory() || self.sizes.get(&row.path).is_some() {
//!             return;
//!         }
//!         let handle = match start_size(&row.path, Default::default(), self.sizes.clone()) {
//!             Ok(handle) => handle,
//!             Err(_) => return,
//!         };
//!         std::thread::spawn(move || {
//!             while let Ok(event) = handle.recv() {
//!                 if let SizeEvent::Done(Ok(Some(size))) = event {
//!                     println!("{} -> {} bytes", size.path.display(), size.logical);
//!                 }
//!             }
//!         });
//!     }
//! }
//!
//! let mut view = View::open(Path::new("/home/achraf")).expect("open");
//! for _ in 0..60 {
//!     view.pump();
//!     std::thread::sleep(Duration::from_millis(16));
//!     if view.scan.is_none() { break }
//! }
//! view.rows.sort_by(SortSpec::default().comparator());
//!
//! let subscription = watch(&view.dir).expect("watch");
//! if let Ok(WatchEvent::Changed(changes)) = subscription.recv_timeout(Duration::from_millis(500)) {
//!     eprintln!("{} directories changed", changes.dirs.len());
//! }
//! ```

#![doc(html_root_url = "https://docs.rs/kestrel-fs/0.1.0")]
#![warn(missing_docs)]
#![warn(clippy::all)]

pub mod error;
pub mod model;
pub mod ops;
pub mod scan;
pub mod size;
pub mod watcher;

pub use error::{KestrelError, Result, ScanError};
pub use model::{CancellationToken, EntryKind, FileEntry, SortKey, SortSpec};
