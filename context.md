# Kestrel — handoff document

Written 2026-09-30, at the end of a long stabilisation session. This describes
**what is true right now**, including the parts that are still broken and the
things that were tried and rejected. A fresh session should be able to work on
this project from this file plus the repo without re-deriving any of it.

Read this before changing anything. Most of the value here is in the *why*.

---

## 1. What this is

A lightweight personal file manager for Linux, written in Rust with `egui`/`eframe`.
Lives at `/home/achraf/Projects/kestrel`. Installed at
`~/.local/share/kestrel/bin/kestrel`, launched from a `.desktop` entry, 17.72 MB.

The governing constraint was **"lightweight"**, and that constraint decided the
whole architecture. See §4 for what it ruled out.

### The two crates

| Crate | Purpose | Rule |
|---|---|---|
| `kestrel-fs` | Engine: scan, size, watch, copy/move/trash, MIME + handler resolution | **Zero UI dependencies.** Must never depend on `egui`/`eframe`/`winit`. |
| `kestrel` | The GUI | The only crate that touches a display server |

This split is the single most important structural decision in the project. It is
why the engine's 95 tests run headless in 0.72s with no display server, and why
a whole class of engine bug can be found without launching a window. Do not
merge them back together for convenience.

`docs/README.md` states four invariants. They are load-bearing; the repo has
already paid for breaking them once.

---

## 2. Current state — verified 2026-09-30

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace` | **443 passed, 0 failed** (311 + 95 + 18 + 19) |
| `cargo build --release` | exit 0, 18,585,768 bytes |
| `git status` | clean, all work committed |

Both crates set `missing_docs = "warn"`, `clippy::all = "warn"`, and
`unsafe_code = "forbid"`. The clippy gate is therefore not optional polish — it is
the reason `unsafe_code` stays at zero. 34 commits on `master`, pushed to
`https://github.com/kenjaku-dev/kestrel` (**public**).

Source is 32,242 lines across 29 files. No `TODO`, `FIXME`, `todo!`, or
`unimplemented!` anywhere in the tree. The only `panic!` calls in non-test code
are one in `packaging.rs:35` reading a file that must exist at that point.

---

## 3. Open bugs and unfinished work

Everything in this section is **verified as of the date above**, with a file
reference. Nothing here is speculation except where marked.

### 3.1 Real bugs

**Empty preview pane reserves a third of the window.**
Discovered from a live screenshot, not from the code. At ~972px window width the
sidebar takes 215px and the *empty* preview pane takes ~330px — width reserved
for an eye icon and the words "Nothing selected." The list is starved to ~410px,
which trips `columns_for()` (`kestrel/src/columns.rs:159`) and drops the
`Modified` timestamp column.

The column logic is **correct** and documented: it drops `Modified` first because
"a timestamp is the column a user can most afford to lose." The pane is what is
wrong. Fix the pane; the column will come back on its own.

**`visible_rows()` allocates per frame, and per row lookup.**
`kestrel/src/app.rs:1315` returns `Vec<usize>`, rebuilt on every call. Worse,
`visible_row(at)` at `app.rs:1328` calls `visible_rows()` and indexes **one**
element — so a single row lookup costs a full-directory allocation plus a full
filter pass. The filter additionally allocates two `String`s per row per frame
(`passes_filter`, `to_lowercase` on both the haystack and the needle). A large
directory will feel heavy. Cache the visible set per scan, not per frame.

### 3.2 Engine capability with no UI

**`handler_for()` returns a `Candidate` and nothing consumes it.**
`kestrel-fs/src/open.rs:462` resolves a file's registered handler, and
`Xdg::from_env()` (line 335) and `mime_of()` (line 624) both work. `grep` for
`Candidate` across `kestrel/src` returns **nothing**. So `Enter` opens only the
default application. The "Open With…" picker is a UI feature over a finished
engine. This is the cheapest real feature left in the project.

### 3.3 Spec-vs-code divergences

**`Ctrl+2` means two different things.**
`docs/design-tokens.md:861` says:

```
| View mode | `Ctrl+1` list, `Ctrl+2` grid, `Ctrl+3` details |
```

The code (`settings.rs:188-192`, `toolbar.rs:41-43`) says `Ctrl+1` = List,
`Ctrl+2` = **Tree**. The `View` enum has only two variants. So the spec table is
stale *and* grid/details were never built. Pick one direction:

- implement `View::Grid`, keep the spec as-is and fix the enum; **or**
- accept that List+Tree is the view set, and amend §4.11 plus the toolbar icon
  row at line 950.

An earlier message in this project claimed "`Ctrl+3` grid is specified and
missing." That was wrong on both counts — grid is `Ctrl+2` in the spec, and
`Ctrl+2` is bound to something real. The underlying gap is still real; the
framing was not.

**`§6.1 status.warning-text` (light) is internally inconsistent.**
Flagged, **not fixed, needs a second opinion.** Recomputing the row against its
own foreground/background cells disagrees in 6 of 8 cells. The minimum ratio
value is believed correct. Either the row or the cells are wrong; which one is
unknown. Do not "fix" this by editing numbers until the discrepancy is explained.

### 3.4 Missing features the spec implies

- **No context menu.** `grep` for `secondary_click` / `context_menu` /
  `right_click` in `kestrel/src` returns nothing. Right-click does nothing.
  Spec §4.11 promises `Shift+F10`, right-click, and `Menu`.
- **No `--version`.** `version = "0.1.0"` is hardcoded in the workspace
  `Cargo.toml` and has never been bumped across 34 commits. `--version` is an
  *unknown option*. `CARGO_PKG_VERSION` is referenced nowhere in the source. The
  binary cannot identify itself — which is exactly the tool you would want to ask
  "am I running the fixed one?" This is a 10-line fix with real diagnostic value.

### 3.5 Intentional, not bugs — do not "fix" these

- **`warn_if_rect_changes_id = true`** at `app.rs:696` and `app.rs:716` is
  **deliberately enabled diagnostics**, not an oversight. Any warnings it emits
  are the harness working.
- **Directories show `—` for size**, not a misleading number. Deliberate.
- **`Modified` drops before `Size`** on a narrow pane. Deliberate and documented.
- **A 90ms directory fade.** Opinionated, may be disliked, is not a bug.

### 3.6 Deferred, known, unstarted

- **TOCTOU hardening.** Copy/move validate a path, then act on it; a rename
  between check and use is still possible. The fix is `openat`-relative
  operation with `O_NOFOLLOW` (see rust-lang/rust#2688). Not attempted.
- **`JobHandle::drop` joins at teardown.** All *frame-thread* joins are gone
  (`9c7e5d6`); this one is at window close and is much less urgent.
- **`app.rs` is 4,912 lines.** It is the file every lane collides on. Splitting
  it is the change that would most improve parallel work on this project.
- **No `LICENSE` file** for Kestrel itself. The vendored fonts are fine — IBM Plex
  is OFL 1.1 and Phosphor is MIT, and both licence files are already in
  `assets/fonts/`. The repo is **public**, so the project's own licence should be
  a deliberate choice, not an omission.

---

## 4. Dead ends — do not re-litigate these

**CSS / webview / Tauri.** Considered and rejected. The entire reason this
project exists is the 17.72 MB binary. Measured: egui 17.5 MB, Electron 123 MB,
Tauri 120 MB binary plus ~140 MB of webkit2gtk. The user's requirement was
explicit and the size difference is not close. **Locked.**

**`mimeapps` crate for Open With.** Tried, rejected. It resolved `Enter` to
Okular instead of Kate on this machine. Replaced by hand-rolled resolution over
`xdg-mime` + desktop-entry parsing (`kestrel-fs/src/open.rs`). Works.

**egui 0.36.2 has no `StyleProvider` / `styling_engine`.** Those exist only on
`main` behind an `experimental` feature. Do not write code against them. 0.36
uses the widget-class model instead. This cost real time once.

**`cc!` macro does not exist in 0.36.** Fonts are passed as `Arc<FontData>`.

**`App::ui`, not `App::update`.** And `Panel::left`, not `SidePanel`. egui 0.36
renamed both. A reference that says otherwise is for a different version.

**The offscreen capture harness rendered glyphs as solid bricks for most of this
project's life.** Only fixed recently (`9478bd4`). **Every screenshot taken
before that fix is invalid** and must not be used as a visual baseline. If a
screenshot shows solid rectangles where text should be, that is the old harness,
not a new bug.

**`cargo-sweep` is maintained** — 0.8.0, 2025-10-11. An agent in this project
claimed the opposite. It was wrong.

**Fish, not bash.** The user's shell is fish: `set -gx GH_TOKEN …`, not
`export …`. Fish also cannot read a password from a piped `-S` as some other
shells can, which is why `sudo pacman` cannot be driven from an agent at all.

**`git filter-repo` is not installed** and cannot be installed (no `sudo`).
History rewrite used built-in `git filter-branch` instead. It works for this.

---

## 5. Mistakes made here — recorded so they are not repeated

These are real. They are here because a handoff that only lists successes
teaches the next session the wrong things.

**By the assistant:**
- Reported a working UI as working while it was rendering **solid brick
  glyphs**. Then separately called a breadcrumb "clipped" when it was not.
- Reported a red test suite that was really a **mid-edit snapshot** — the tree
  was changing under the test run. The timing was wrong.
- **Falsely alarmed about a leaked GitHub token.** The check was
  `git grep -l … | head` followed by `&&`, and `head` always exits 0, so the
  `&&` fired regardless of whether grep matched. The repo is clean; the check
  was broken. Correct form:
  `if git grep -lIE 'ghp_[A-Za-z0-9]{20,}' -- . ; then … ; fi`
- Repeatedly promised `context.md` and deferred it four times.

**By the agents:**
- One lane **shipped a red tree and claimed it was clean.**
- One lane **misattributed its own flaky test** to "4-core contention" when it
  failed 2-of-3 times *in isolation*. Contention cannot explain an isolated
  failure.
- One lane was briefed that `ops.rs` was dirty. It wasn't — the real dirt was a
  content-less mode change on `kestrel/packaging/kestrel.desktop`. It checked
  and said so, which was the right call and worth copying.

**The lesson that generalises:** every one of these was a *claim* that outran
its *evidence*. Verify from the tree, from a command, or from a file — never from
a report, including a report about your own prior claim.

---

## 6. The two bugs that were masking each other

Worth understanding, because it is the best argument for not stopping at "green".

`16aeb1b` fixed the watcher so it actually delivered filesystem events. That
unmasked a latent engine bug, found by `a74415c`: `debounce_loop`
(`kestrel-fs/src/watcher.rs:358`) delivered **every** inotify event kind except
`Other` as a directory change — including `EventKind::Access(Open/Read/Close)`.
The app's own `readdir` during scan, and its own preview reads, therefore
re-listed the directory, which cleared the selection, which meant the preview
never loaded. And each re-list started a new scan, which emitted another access
event — a genuine livelock, not a slow machine. One instrumented run showed
seven consecutive self-triggered re-lists.

The dead watcher had been hiding it. Fix one bug, and a different bug becomes
visible. **A green suite is not the same as a correct program** — it means the
current failures are the only ones still observable.

**Known residual risk:** a read that updates `atime` surfaces as `IN_ATTRIB`,
which notify may report as `Modify(Metadata(AccessTime))` rather than `Access`.
Eight instrumented runs on this machine showed only `Access(Open)` (relatime
does not bump atime on a just-written file), so the fix was kept minimal. On a
`strictatime` mount the next arm is `Modify(Metadata(AccessTime))`. The listing
displays mtime, never atime, so an access-time event is still not a real change.

---

## 7. How to work on this project

**Gates, in this order.** `fmt --check`, then `clippy -D warnings`, then
`cargo test --workspace`, then `cargo build --release`. All four are currently
green; keep them that way. The bin suite takes ~98s, so budget for it.

**Lanes.** Two agents worked this project in parallel and the pattern that worked
was: disjoint file ownership, explicit scope, and each lane required to
demonstrate its own bug — **a test that fails before the fix and passes after**.
One lane was asked to fix a bug and produced a plausible patch with no failing
test; that is not evidence.

**Never read the tree while another lane is mid-edit.** That mistake produced a
phantom regression here. Wait for the report, then diff.

**Ship one lane's changes at a time.** Force-with-lease pushes are only needed
after an intentional history rewrite.

**Docs are a separate lane's territory.** `docs/` is not to be edited by a code
lane, and a code lane is not to be judged on doc churn.

---

## 8. Environment

- Rust 1.98.1, Wayland under Hyprland, Intel HD 530, Mesa 26.2.3
- egui/eframe 0.36.2, `glow` renderer pinned 0.17 (**not** wgpu), winit 0.30
- Settings persist to `~/.local/share/dev.kestrel.app/app.ron`
- Repo: `https://github.com/kenjaku-dev/kestrel`, public, branch `master`
- All commits authored `Omega <kenjaku-dev@users.noreply.github.com>`. History was
  rewritten once (via `filter-branch`) off an agent identity; a
  `backup-pre-rewrite` branch still points at the pre-rewrite `991399d`.

**GitHub auth is via the system keyring** — `gh auth status` reports the account
with no token in the environment. Do not put a token in a chat, a file, or a
command line.

---

## 9. Where the design lives

`docs/design-tokens.md` (1,359 lines) is the locked design direction, "Kestrel."
It is the source of truth for every colour, size, radius, and motion value, and
its §8 decisions log records why each was chosen.

- §2 primitives, §3 semantics, §4 components, §5 icon mapping
- §6 accessibility audit — 17/17 contrast ratios independently verified
- §7 anti-goals — **read this before "improving" anything.** It lists the
  look-and-feel the project deliberately refused.

`kestrel/src/tokens.rs` (2,533 lines) is the code translation of that spec.

One place where code and spec were reconciled *the other way*: `surface.raised`
dark is `#2B2824`, which equals `neutral.800`. The spec was corrected to match
the code, because the code was already right and shipped.

---

## 10. Suggested order for the next session

1. **`--version` + bump to 0.2.0.** Ten minutes; makes every future "am I
   running the fixed build?" answerable.
2. **Fix the empty preview pane** (§3.1). Restores the `Modified` column and
   reclaims a third of the window.
3. **Decide the `Ctrl+2` divergence** (§3.3). Either implement grid or amend the
   spec. Do not leave the two documents contradicting each other.
4. **`visible_rows()` caching** (§3.1). The one real performance defect.
5. **Open With picker** (§3.2). Engine is done; this is UI only.
6. **`context menu`** (§3.4). The only pointer path to any operation.
7. **Split `app.rs`** (§3.6). Do this before opening more parallel lanes.
8. **Decide the `LICENSE`** (§3.6). The repo is public.
9. **Second opinion on §6.1** (§3.3). Needs recomputation, not an edit.
