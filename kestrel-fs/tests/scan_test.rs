//! End-to-end tests against real filesystems, using `tempfile` fixtures.
//!
//! These cover the behaviours a file manager is judged on: correct listing,
//! symlink-loop termination, symlink classification, permission errors as data
//! (not panics), natural sorting, size totals, cross-device move fallback and
//! collision refusal.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kestrel_fs::error::{KestrelError, ScanError};
use kestrel_fs::model::{CancellationToken, EntryKind, FileEntry, SortKey, SortSpec};
use kestrel_fs::ops::{Collision, CopyOptions, MoveStrategy, copy, delete_recursive, move_};
use kestrel_fs::scan::{self, ScanEvent, ScanOptions, ScanSummary};
use kestrel_fs::size::{SizeCache, SizeEvent, SizeOptions, compute_blocking};

/// Collects a whole synchronous scan.
fn run_scan(root: &Path, options: ScanOptions) -> (Vec<ScanEvent>, ScanSummary) {
    let mut events = Vec::new();
    let mut sink = |event: ScanEvent| {
        events.push(event);
        true
    };
    let summary = scan::scan(root, options, &CancellationToken::new(), &mut sink)
        .expect("scan should not fail on a readable directory");
    (events, summary)
}

fn entry_names(events: &[ScanEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            ScanEvent::Entry(entry) => Some(entry.name.clone()),
            _ => None,
        })
        .collect()
}

fn entries_of_kind(events: &[ScanEvent], kind: EntryKind) -> Vec<&FileEntry> {
    events
        .iter()
        .filter_map(|event| match event {
            ScanEvent::Entry(entry) if entry.kind == kind => Some(entry),
            _ => None,
        })
        .collect()
}

fn scan_errors(events: &[ScanEvent]) -> Vec<&ScanError> {
    events
        .iter()
        .filter_map(|event| match event {
            ScanEvent::Error(err) => Some(err),
            _ => None,
        })
        .collect()
}

/// Waits for the watcher's next coalesced change set, skipping backend noise.
fn next_change(
    sub: &kestrel_fs::watcher::WatchSubscription,
    timeout: Duration,
) -> kestrel_fs::watcher::ChangeSet {
    let deadline = Instant::now() + timeout;
    let mut found = None;
    while found.is_none() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match sub.recv_timeout(remaining) {
            Ok(kestrel_fs::watcher::WatchEvent::Changed(changes)) => found = Some(changes),
            Ok(kestrel_fs::watcher::WatchEvent::Error(err)) => panic!("watcher error: {err}"),
            Err(e) => panic!("no change event within {timeout:?}: {e}"),
        }
    }
    found.expect("checked above")
}

fn is_root() -> bool {
    // No libc dependency: read the uid out of /proc when available, otherwise
    // fall back to the classic root-owned test-probe heuristic.
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status.lines().find_map(|line| {
                line.strip_prefix("Uid:")
                    .map(|rest| rest.split_whitespace().next().is_some_and(|uid| uid == "0"))
            })
        })
        .unwrap_or(false)
}

/// A directory with a predictable nested shape:
///
/// ```text
/// root/
///   a.txt          "alpha"
///   b/  b1.txt     "beta"
///   b/  c/  c1.txt "gamma"
/// ```
fn nested_fixture() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    fs::write(root.join("a.txt"), b"alpha").expect("write a.txt");
    fs::create_dir_all(root.join("b/c")).expect("mkdir b/c");
    fs::write(root.join("b/b1.txt"), b"beta").expect("write b1");
    fs::write(root.join("b/c/c1.txt"), b"gamma").expect("write c1");
    tmp
}

#[test]
fn nested_scan_returns_expected_entries() {
    let tmp = nested_fixture();
    let root = tmp.path();

    // Non-recursive: exactly one level, which is what the file list needs.
    let (events, summary) = run_scan(root, ScanOptions::listing());
    assert_eq!(
        entry_names(&events),
        vec!["b", "a.txt"],
        "dirs first, sorted"
    );
    assert_eq!(summary.total, 2);
    assert_eq!(summary.errors, 0);

    // Recursive: the whole tree.
    let (events, summary) = run_scan(root, ScanOptions::recursive());
    let mut names = entry_names(&events);
    names.sort();
    assert_eq!(names, vec!["a.txt", "b", "b1.txt", "c", "c1.txt"]);
    assert_eq!(summary.errors, 0);
    assert!(!summary.cancelled);
}

#[test]
fn entry_metadata_is_populated() {
    let tmp = nested_fixture();
    let (events, _) = run_scan(tmp.path(), ScanOptions::listing());
    let a = entries_of_kind(&events, EntryKind::File)
        .into_iter()
        .find(|entry| entry.name == "a.txt")
        .expect("a.txt present");
    assert_eq!(a.path, tmp.path().join("a.txt"));
    assert_eq!(a.size, Some(5));
    assert!(a.modified.is_some(), "mtime should be available");
    assert!(!a.hidden);

    let b = entries_of_kind(&events, EntryKind::Directory)
        .into_iter()
        .find(|entry| entry.name == "b")
        .expect("b present");
    assert!(b.kind.is_directory());
    assert!(b.name_os().eq_ignore_ascii_case("b"));
}

#[test]
fn hidden_entries_are_flagged_and_filterable() {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::write(tmp.path().join(".hidden"), b"x").expect("write");
    fs::write(tmp.path().join("visible"), b"x").expect("write");

    let (events, _) = run_scan(tmp.path(), ScanOptions::listing());
    let hidden = entries_of_kind(&events, EntryKind::File)
        .into_iter()
        .find(|entry| entry.name == ".hidden")
        .expect("hidden entry present by default");
    assert!(hidden.hidden);

    let options = ScanOptions {
        show_hidden: false,
        ..ScanOptions::listing()
    };
    let (events, _) = run_scan(tmp.path(), options);
    assert_eq!(entry_names(&events), vec!["visible"]);
}

#[test]
fn symlink_is_classified_as_symlink_not_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let target = tmp.path().join("target.txt");
    fs::write(&target, b"payload").expect("write");

    #[cfg(unix)]
    {
        // Both a link to a file and a link to a directory.
        std::os::unix::fs::symlink(&target, tmp.path().join("link_file")).expect("link file");
        fs::create_dir(tmp.path().join("dir")).expect("mkdir");
        std::os::unix::fs::symlink(tmp.path().join("dir"), tmp.path().join("link_dir"))
            .expect("link dir");
        // A broken link too: it must still be a Symlink, not an error.
        std::os::unix::fs::symlink(tmp.path().join("gone"), tmp.path().join("link_broken"))
            .expect("link broken");
    }

    let (events, _) = run_scan(tmp.path(), ScanOptions::listing());
    let links = entries_of_kind(&events, EntryKind::Symlink);
    assert!(
        !links.is_empty(),
        "symlinks must not be reported as files, got {:?}",
        entry_names(&events)
    );
    for link in &links {
        assert!(link.kind.is_symlink());
        assert!(
            fs::symlink_metadata(&link.path)
                .map(|md| md.file_type().is_symlink())
                .unwrap_or(false),
            "{} is a symlink on disk",
            link.name
        );
    }
    #[cfg(unix)]
    {
        let names: Vec<&str> = links.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"link_file"));
        assert!(names.contains(&"link_dir"));
        assert!(names.contains(&"link_broken"));
    }
}

#[test]
fn symlink_loop_terminates() {
    // `root/inner/loop -> root`: a link straight back to an ancestor. Any
    // traversal that follows links without bookkeeping recurses forever (or
    // until the disk fills up).
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    fs::create_dir_all(root.join("inner/deeper")).expect("mkdir");
    fs::write(root.join("inner/deeper/leaf.txt"), b"leaf").expect("write");

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root, root.join("inner/loop")).expect("link to ancestor");
        std::os::unix::fs::symlink(root.join("inner"), root.join("inner/deeper/back"))
            .expect("link to parent");
    }

    for (label, options, expect_loop_error) in [
        // Links are not followed, so no loop is even reachable.
        (
            "default (links not followed)",
            ScanOptions::recursive(),
            false,
        ),
        (
            "follow_symlinks = true",
            ScanOptions {
                follow_symlinks: true,
                ..ScanOptions::recursive()
            },
            true,
        ),
    ] {
        let started = Instant::now();
        let (events, summary) = run_scan(root, options);

        assert!(
            started.elapsed() < Duration::from_secs(20),
            "{label}: scan did not terminate promptly"
        );
        assert!(
            summary.total < 20,
            "{label}: runaway traversal produced {} entries",
            summary.total
        );

        let looped = scan_errors(&events)
            .iter()
            .any(|err| matches!(err.error, KestrelError::LoopDetected { .. }));
        assert_eq!(
            looped,
            expect_loop_error,
            "{label}: unexpected loop reporting, errors: {:?}",
            scan_errors(&events)
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
        );
        // The real content is still found: loop protection must not prune the
        // actual tree.
        assert!(
            entry_names(&events).contains(&"leaf.txt".to_string()),
            "{label}: traversal lost the real subtree"
        );
    }
}

#[test]
fn permission_denied_is_reported_as_data_not_a_panic() {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::write(tmp.path().join("open.txt"), b"x").expect("write");

    if is_root() {
        // root bypasses mode bits, so the permission test cannot be provoked
        // here. Use the other ways a path can be unreadable, which exercise
        // exactly the same "report, keep walking" path.
        let missing = tmp.path().join("does-not-exist");
        let (events, summary) = run_scan(&missing, ScanOptions::listing());
        let _ = summary;
        assert!(events.is_empty(), "an unreadable root yields no entries");

        let err = scan::scan(
            &missing,
            ScanOptions::listing(),
            &CancellationToken::new(),
            &mut |_| true,
        )
        .expect_err("missing root must be an error, not a panic");
        assert!(err.is_not_found());

        // A file used as a directory.
        let err = scan::scan(
            tmp.path().join("open.txt"),
            ScanOptions::listing(),
            &CancellationToken::new(),
            &mut |_| true,
        )
        .expect_err("file as directory must be an error");
        assert!(
            matches!(err, KestrelError::NotADirectory { .. }),
            "expected NotADirectory, got {err}"
        );
    } else {
        let locked = tmp.path().join("locked");
        fs::create_dir(&locked).expect("mkdir");
        fs::write(locked.join("secret.txt"), b"s").expect("write");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("chmod");

        let (events, summary) = run_scan(tmp.path(), ScanOptions::recursive());

        // No panic; the walk finished and reported the locked directory.
        let denied = scan_errors(&events)
            .iter()
            .any(|err| err.is_permission_denied() && err.path.ends_with("locked"));
        assert!(
            denied,
            "expected a permission-denied error event, got {:?}",
            scan_errors(&events)
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
        );
        assert!(summary.errors >= 1);

        // The rest of the scan still worked.
        assert!(entry_names(&events).contains(&"open.txt".to_string()));
        assert!(!summary.cancelled);

        // Restore so the TempDir cleanup can succeed.
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).expect("chmod back");
    }
}

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;

#[test]
fn natural_sort_places_file2_before_file10() {
    let tmp = tempfile::tempdir().expect("tempdir");
    for name in ["file10", "file2", "file1", "file20", "file3"] {
        fs::write(tmp.path().join(name), b"x").expect("write");
    }

    // Through the scan pipeline, with the default spec.
    let (events, _) = run_scan(tmp.path(), ScanOptions::listing());
    assert_eq!(
        entry_names(&events),
        vec!["file1", "file2", "file3", "file10", "file20"]
    );

    // And directly, so the requirement is pinned independently of scanning.
    let mut names = vec!["file10", "file2", "file1"];
    names.sort_by(|a, b| kestrel_fs::model::natural_cmp(a, b));
    assert_eq!(names, vec!["file1", "file2", "file10"]);

    // Plain byte order would put "file10" first; make sure we are not doing that.
    let mut byte_sorted = vec!["file10", "file2"];
    byte_sorted.sort();
    assert_eq!(byte_sorted, vec!["file10", "file2"]);
}

#[test]
fn all_sort_keys_work_on_a_real_listing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::write(tmp.path().join("small.txt"), vec![0u8; 10]).expect("write");
    fs::write(tmp.path().join("big.txt"), vec![0u8; 10_000]).expect("write");
    fs::create_dir(tmp.path().join("zdir")).expect("mkdir");

    for key in [
        SortKey::Name,
        SortKey::Size,
        SortKey::Modified,
        SortKey::Kind,
    ] {
        let options = ScanOptions {
            sort: Some(SortSpec {
                key,
                ascending: true,
                dirs_first: true,
            }),
            ..ScanOptions::listing()
        };
        let (events, _) = run_scan(tmp.path(), options);
        let names = entry_names(&events);
        assert_eq!(names.len(), 3, "sorting by {key:?} lost rows: {names:?}");
        assert_eq!(names[0], "zdir", "dirs first regardless of {key:?}");
    }

    // Largest first.
    let options = ScanOptions {
        sort: Some(SortSpec {
            key: SortKey::Size,
            ascending: false,
            dirs_first: false,
        }),
        ..ScanOptions::listing()
    };
    let (events, _) = run_scan(tmp.path(), options);
    assert_eq!(entry_names(&events)[0], "big.txt");
}

#[test]
fn size_computation_matches_known_bytes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    fs::write(root.join("a.bin"), vec![0u8; 100]).expect("write");
    fs::create_dir_all(root.join("sub/deep")).expect("mkdir");
    fs::write(root.join("sub/b.bin"), vec![0u8; 200]).expect("write");
    fs::write(root.join("sub/deep/c.bin"), vec![0u8; 300]).expect("write");
    // Sparse file: logical size counts, on-disk does not. Keep it out of the
    // fixture so the expected total stays obvious.
    let size = compute_blocking(
        root,
        SizeOptions::default(),
        &CancellationToken::new(),
        &mut |_| true,
    )
    .expect("walk")
    .expect("not cancelled");
    assert_eq!(size.logical, 600);
    assert_eq!(size.files, 3);
    assert_eq!(size.dirs, 3);
    assert_eq!(size.errors, 0);
    assert!(!size.truncated);
    assert!(size.on_disk >= size.logical);

    // The scan path must NOT have computed any of this: sizes come from lstat.
    let (events, _) = run_scan(root, ScanOptions::listing());
    let dir = entries_of_kind(&events, EntryKind::Directory)
        .into_iter()
        .find(|entry| entry.name == "sub")
        .expect("sub");
    assert_eq!(
        dir.size,
        Some(fs::symlink_metadata(root.join("sub")).expect("lstat").len()),
        "a directory row shows the directory's own length, never a recursive total"
    );
}

#[test]
fn size_computation_is_cached_and_cancellable() {
    let tmp = nested_fixture();
    let cache = SizeCache::new();
    assert!(cache.get(tmp.path()).is_none());

    let handle =
        kestrel_fs::size::start(tmp.path(), SizeOptions::default(), cache.clone()).expect("spawn");
    let mut progress_seen = false;
    let mut result = None;
    while let Ok(event) = handle.recv() {
        match event {
            SizeEvent::Progress { .. } => progress_seen = true,
            SizeEvent::Done(done) => {
                result = Some(done);
                break;
            }
            SizeEvent::Error(_) => {}
        }
    }
    let _ = progress_seen;
    let size = result
        .expect("done event")
        .expect("ok")
        .expect("not cancelled");
    assert_eq!(
        size.logical,
        ("alpha".len() + "beta".len() + "gamma".len()) as u64
    );
    assert_eq!(cache.get(tmp.path()).map(|s| s.logical), Some(size.logical));

    // Cached values survive, and a write invalidates them.
    assert!(cache.get(tmp.path()).is_some());
    cache.invalidate(tmp.path());
    assert!(cache.get(tmp.path()).is_none());

    // Cancelling a fresh run yields `Done(Ok(None))`.
    let handle =
        kestrel_fs::size::start(tmp.path(), SizeOptions::default(), cache.clone()).expect("spawn");
    handle.cancel();
    let mut cancelled = false;
    while let Ok(event) = handle.recv() {
        if let SizeEvent::Done(result) = event {
            cancelled = matches!(result, Ok(None));
            break;
        }
    }
    assert!(cancelled, "a cancelled walk must report cancellation");
}

#[test]
fn move_across_devices_falls_back_to_copy_then_delete() {
    // A real EXDEV needs a second mount, which a test cannot arrange. Both
    // halves are therefore checked explicitly: the fallback is what a
    // cross-device move executes, and it must be indistinguishable from a
    // rename.
    let tmp = tempfile::tempdir().expect("tempdir");
    let src = tmp.path().join("payload");
    let dst = tmp.path().join("landed");

    fs::create_dir_all(src.join("nested")).expect("mkdir");
    fs::write(src.join("file.bin"), vec![7u8; 5_000]).expect("write");
    fs::write(src.join("nested/deep.bin"), vec![9u8; 123]).expect("write");
    #[cfg(unix)]
    std::os::unix::fs::symlink("file.bin", src.join("link.bin")).expect("link");

    // `EXDEV` is what `rename` returns across filesystems; the engine keys off
    // exactly this and then takes the copy+delete path.
    assert!(kestrel_fs::ops::is_cross_device(
        &std::io::Error::from_raw_os_error(18)
    ));

    move_(
        &src,
        &dst,
        CopyOptions::default(),
        MoveStrategy::CopyThenDelete,
    )
    .expect("fallback move");

    assert!(!src.exists(), "source removed after the copy");
    assert_eq!(fs::read(dst.join("file.bin")).expect("read").len(), 5_000);
    assert_eq!(
        fs::read(dst.join("nested/deep.bin")).expect("read").len(),
        123
    );
    #[cfg(unix)]
    {
        let meta = fs::symlink_metadata(dst.join("link.bin")).expect("lstat");
        assert!(
            meta.file_type().is_symlink(),
            "link survived the move as a link"
        );
    }
}

#[test]
fn copy_refuses_to_overwrite_without_the_explicit_flag() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let src = tmp.path().join("source.txt");
    let dst = tmp.path().join("existing.txt");
    fs::write(&src, b"new contents").expect("write");
    fs::write(&dst, b"old contents").expect("write");

    let err = copy(&src, &dst, CopyOptions::default()).expect_err("must refuse");
    match err {
        KestrelError::AlreadyExists { ref path } => assert_eq!(path, &dst),
        other => panic!("expected AlreadyExists, got {other}"),
    }
    assert_eq!(
        fs::read(&dst).expect("read"),
        b"old contents",
        "the existing file must be untouched"
    );

    // Explicit permission succeeds.
    copy(
        &src,
        &dst,
        CopyOptions {
            collision: Collision::Overwrite,
            ..CopyOptions::default()
        },
    )
    .expect("overwrite allowed");
    assert_eq!(fs::read(&dst).expect("read"), b"new contents");
}

#[test]
fn copy_guard_refuses_self_nesting() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let src = tmp.path().join("folder");
    fs::create_dir_all(src.join("inner")).expect("mkdir");
    let err = copy(&src, src.join("inner/copy"), CopyOptions::default()).expect_err("must refuse");
    assert!(
        matches!(err, KestrelError::InvalidInput { .. }),
        "got {err}"
    );
}

#[test]
fn background_scan_streams_and_completes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    for i in 0..800 {
        fs::write(tmp.path().join(format!("f{i:04}")), b"x").expect("write");
    }

    let handle = scan::start(tmp.path(), ScanOptions::listing()).expect("spawn");
    let mut rows = 0;
    let mut batches = 0;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match handle.recv_timeout(remaining) {
            Ok(ScanEvent::Entry(_)) => rows += 1,
            Ok(ScanEvent::BatchEnd) => batches += 1,
            Ok(ScanEvent::Complete { total }) => {
                assert_eq!(total, rows);
                break;
            }
            Ok(ScanEvent::Error(err)) => panic!("unexpected error: {err}"),
            Err(e) => panic!("scan never completed: {e}"),
        }
    }
    assert_eq!(rows, 800);
    assert!(
        batches > 1,
        "entries should arrive in batches, got {batches}"
    );
}

#[test]
fn background_scan_cancellation_stops_work() {
    let tmp = tempfile::tempdir().expect("tempdir");
    for i in 0..2_000 {
        fs::create_dir(tmp.path().join(format!("d{i:04}"))).expect("mkdir");
    }
    let handle = scan::start(tmp.path(), ScanOptions::recursive()).expect("spawn");
    // Let it get going, then pull the plug, the way a user navigating away would.
    let _ = handle.recv_timeout(Duration::from_millis(20));
    handle.cancel();

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match handle.recv_timeout(remaining) {
            Ok(ScanEvent::Complete { .. }) => break,
            Ok(_) => {}
            Err(e) => panic!("worker kept running after cancel: {e}"),
        }
    }
}

#[test]
fn watcher_reports_coalesced_directory_changes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let sub = kestrel_fs::watcher::watch(tmp.path()).expect("watch");
    let watched: PathBuf = tmp.path().to_path_buf();

    for i in 0..20 {
        fs::write(tmp.path().join(format!("burst{i}")), b"x").expect("write");
    }

    let changes = next_change(&sub, Duration::from_secs(20));
    assert!(
        changes.touches(&watched),
        "the watched directory should be flagged: {changes:?}"
    );
}

#[test]
fn delete_recursive_removes_a_tree() {
    let tmp = nested_fixture();
    let victim = tmp.path().join("b");
    assert!(victim.is_dir());
    delete_recursive(&victim, None).expect("delete");
    assert!(!victim.exists());
    assert!(tmp.path().join("a.txt").exists(), "siblings untouched");
}

#[test]
fn the_scanner_resolves_symlink_targets_so_the_ui_never_has_to() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let sub = tmp.path().join("subdir");
    fs::create_dir(&sub).expect("mkdir");
    let file = tmp.path().join("file.txt");
    fs::write(&file, b"x").expect("write");

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&sub, tmp.path().join("to-dir")).expect("symlink");
        std::os::unix::fs::symlink(&file, tmp.path().join("to-file")).expect("symlink");
        std::os::unix::fs::symlink(tmp.path().join("nowhere"), tmp.path().join("dangling"))
            .expect("symlink");
    }

    let (events, _) = run_scan(tmp.path(), ScanOptions::listing());
    let entries: Vec<FileEntry> = events
        .iter()
        .filter_map(|e| match e {
            ScanEvent::Entry(entry) => Some(entry.clone()),
            _ => None,
        })
        .collect();
    let by_name = |n: &str| {
        entries
            .iter()
            .find(|e| e.name == n)
            .unwrap_or_else(|| panic!("{n} missing from the listing"))
    };

    // The whole point: the answer is on the row, so the UI does no I/O.
    let to_dir = by_name("to-dir");
    assert_eq!(to_dir.kind, EntryKind::Symlink);
    assert_eq!(to_dir.is_dir_target, Some(true));
    assert!(to_dir.is_descendable());

    let to_file = by_name("to-file");
    assert_eq!(to_file.is_dir_target, Some(false));
    assert!(!to_file.is_descendable());

    // A dangling link is *not* silently treated as "not a link": the row
    // survives, the field is honestly unknown, and Enter does nothing.
    let dangling = by_name("dangling");
    assert_eq!(dangling.kind, EntryKind::Symlink);
    assert_eq!(
        dangling.is_dir_target, None,
        "unresolvable target stays unknown"
    );
    assert!(!dangling.is_descendable());

    // Non-symlinks never carry a link target at all.
    for name in ["file.txt", "subdir"] {
        assert_eq!(by_name(name).is_dir_target, None, "{name}");
        assert!(by_name(name).is_descendable() == (name == "subdir"));
    }
}
