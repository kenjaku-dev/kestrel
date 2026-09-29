# Kestrel

A lightweight two-pane file manager. Rust, [egui](https://www.egui.rs/) / eframe,
two crates, ~24k lines.

Dense on purpose: 26px rows, 13px type, a near-neutral warm-graphite chrome, and
exactly one saturated colour in the interface — the accent, and only on the thing you
are acting on. Everything else the eye has to parse is file-type hue. The full design
system, including a machine-verified accessibility audit, is
**[`design-tokens.md`](design-tokens.md)**; read that before changing anything visual.

---

## The two crates, and why they are split

```
kestrel-fs/    the filesystem engine — no UI dependency of any kind
kestrel/       the egui/eframe shell — depends on kestrel-fs
```

`kestrel-fs` is a plain library with **zero UI dependencies**. It knows nothing about
egui, a GPU, a window, or an event loop, which means every behaviour in it is testable
headless — `cargo test -p kestrel-fs` runs in a fraction of a second with no display
server, and the tests use real temporary directories rather than mocks, because a
collision, an overwrite and a cancel all need an actual filesystem to happen on.

`kestrel` is the only member that knows a GPU exists. It depends on `kestrel-fs` and
nothing depends on it. The split is not tidiness: it is what makes the parts of the
system that can be *wrong* — traversal, symlink resolution, cross-device moves,
cancellation, trash — testable without a window, and it means a filesystem bug can never
be an event-loop bug.

---

## The four invariants

These are load-bearing, not stylistic. Each one has cost real time, and each is
enforced in the code rather than left to good intentions.

**1. Nothing blocks the frame loop.**
`KestrelApp::ui()` contains no `std::fs` at all. Directory reads go through
`kestrel_fs::scan::start` on a worker thread, filesystem *changes* through
`kestrel_fs::watcher` on its own, free space through a `SpaceProbe` on a worker.
Symlink targets are resolved by the *scanner* and ride along on `FileEntry::is_dir_target`,
so deciding whether `Enter` descends is a field read rather than a `stat`. Every long
job hands out a `CancellationToken` so the user can abandon it. A dropped frame in a
file manager is a dropped frame the user cannot explain.

**2. No `unwrap` or `expect` in non-test code.**
Both crates carry `#![warn(missing_docs)]` and `clippy::all`; this one is a review
invariant rather than a lint. Every fallible call — `scan::start`, `watcher::watch`,
`Response::clicked`, `Vec::get` — is matched explicitly. There is exactly one legitimate
`unwrap` shape left in either crate: on a `const` expression that cannot fail.

**3. Filesystem errors are data, not panics.**
A file manager meets hostile input constantly: half of `/home` is unreadable, paths
exceed `PATH_MAX`, symlinks dangle, mounts disappear mid-traverse. None of that is
exceptional. Every fallible operation returns a `KestrelError` that keeps the offending
`PathBuf` and the original `io::Error`; an unreadable directory is one row in an error
list, never a panic. See `kestrel-fs/src/error.rs`.

**4. Trash by default; never a silent overwrite.**
`Delete` means trash. Permanent deletion is the explicit `Shift+Delete` and requires a
confirmation dialog whose Enter is *not* bound to it. A copy or move whose destination
already exists fails with `AlreadyExists` unless the caller passed an explicit
overwrite; the UI resolves collisions, `ops.rs` does not guess. `fs::rename` failing
with `EXDEV` across a mount point is handled as the normal case it is — copy, then
delete — because dragging a folder to a USB stick is not an error.

Also structural, and load-bearing in the same way: **every sticky band is a layout
sibling with a 1px border, never an overlay** (§2.12). That is what makes WCAG 2.2
"Focus Not Obscured" a property of the layout rather than something a scroll offset has
to remember.

---

## Build, test, run

There is no `Makefile` or `justfile` yet — the packaging targets land in Phase 5. The
commands are plain Cargo and there is no build step to learn.

```sh
# build (workspace, debug)
cargo build

# build optimised — `lto = "thin"` is set in the root profile
cargo build --release

# the whole test suite
cargo test

# just the engine: headless, no display server, sub-second
cargo test -p kestrel-fs

# just the UI crate
cargo test -p kestrel

# lint. The crates are `missing_docs = "warn"` and `clippy::all = "warn"`,
# and `unsafe_code = "forbid"` in both, so this is the gate.
cargo clippy --workspace --all-targets

# run
cargo run --release
cargo run --release -- /some/directory
```

Start directory: an explicit `DIR` argument wins, then `$KESTREL_HOME`, then `$HOME`,
then `/`. A single directory may be given; two is a usage error, not a silent
last-one-wins.

**Build profile note.** `kestrel/Cargo.toml` sets `eframe` `default-features = false`
with an explicit feature list, which keeps `wgpu` and its ~200-crate graph out of the
build entirely — this renders with `glow` on an Intel HD 530. Do **not** add
`winit/default`: Cargo hard-errors on a `dep/feat` string in a dependency feature list,
and losing `wayland-csd-adwaita` is deliberate (Hyprland draws its own decorations).

`--help` prints the full flag list and is the authority on it; this file is the
orientation, the binary is the reference.

---

## The offscreen capture harness

The design system has to be *seen* before it is built on, and a screenshot grabbed off
the desktop does not survive contact with a real session — other windows cover it, the
compositor may be scaling it, and the capture depends on what else happened to be on
screen. `--screenshot` renders the real app through egui's own tessellation into an
in-memory framebuffer and writes a PNG. No window, no compositor, no timing.

```sh
# the whole design-system gallery, every component at every state
cargo run --release -- --gallery --screenshot /tmp/gallery.png

# the file manager, light and dark
cargo run --release -- --screenshot /tmp/light.png --light
cargo run --release -- --screenshot /tmp/dark.png  --dark

# a specific state at a specific size
cargo run --release -- --screenshot /tmp/dlg.png --scene confirm-trash --size 900x700
```

| flag | effect |
|---|---|
| `--screenshot PATH` | render the UI, write a PNG to `PATH`, exit. Replaces the whole run. |
| `--scene NAME` | drive the app into a named state *before* capturing, then capture. One of `browser`, `confirm-permanent`, `confirm-trash`, `collision`, `progress`, `failed`, `preview-empty`, `preview-text`, `preview-image`, `preview-too-large`. An unknown name is an error listing the real ones — never a silent fallback to a default state, which would then produce a perfectly reviewable PNG of the wrong thing. |
| `--size WxH` | capture at an explicit size, e.g. `900x700`. A layout that only works at the default width is a layout that does not work. Minimum 200×200. Defaults are 1200×800 for the file manager and 1200×2600 for the gallery. |

Dialogs and the populated preview pane are *states*, not screens: they exist only as
the result of a keystroke or a background answer, and there is no flag that reaches
them. `--scene` is therefore the only way to review §4.7, and any non-`browser` scene
runs against a generated fixture directory (`$TMPDIR/kestrel-shot-fixture`, rebuilt
each run) containing one of everything the pane has a case for — a multi-line text
file, a real 320×240 PNG, a file over the preview size cap, a folder, and a file with
no extension. Capturing your `$HOME` proves the layout; capturing a directory chosen
for what is in it proves the components.

It is a **review tool, not a test harness**. It renders a fixed number of frames at a
fixed size with no input, so it proves the theme composes and the layout is sane. It
does not prove interaction works. For that, run the app.

### ⚠️ Every screenshot taken before the brick-glyph fix is invalid

**This is the single most important thing to know about comparing against old
captures.**

For most of this project's life, the capture harness rasterised **every glyph as a
solid block** — captions and filenames rendered as a row of little bricks. It looked
exactly like a broken renderer, and it is not: the cause was in the harness, not the
theme.

The font atlas is *built* by the frames that lay the UI out, and egui delivers the
texture delta **one frame late**. The settling loop was discarding the settling frames'
deltas instead of folding them into the texture book it later drew with — so by the time
the drawing passes ran, the book was empty and every glyph sampled nothing. Because the
symptom is visually plausible ("the font didn't load"), this hid itself for a long
time, and the fix is the shared `TextureBook` in `shot.rs`'s `settle_until`.

**Consequences for anyone comparing against historical captures:**

* Any PNG from before the fix is not a valid rendering of any theme. Do not diff
  against it, do not use it as a baseline, and do not treat a difference as a
  regression. Re-capture the "before" state.
* The same bug also produced a second, quieter one: a **constant** capture clock pins
  every dialog at zero opacity forever, because egui computes a fade as
  `time - last_became_visible_at` and an `Area` stamps that on the frame it first
  appears. Captures taken with a frozen clock show dialogs and scrims at a fraction of
  their real alpha. The clock now starts at a synthetic 10s and advances 50ms per
  frame — late enough to be past every animation the app has, so a capture is the
  *settled* frame. Captures from before that are wrong in colour as well as in glyphs.

If a capture looks wrong, suspect the harness before the theme. The harness's own
module docs are the record of both bugs and of how they were found.

---

## Where things live

| path | what |
|---|---|
| `docs/design-tokens.md` | the design system. Single source of truth. Read §8 (the decisions log) before implementing against any value — it records where the code was right and this document was behind. |
| `kestrel/src/tokens.rs` | the transcription: primitives → semantics → components, plus the light/dark `Theme`. Every §2.1/§2.2/§2.3 rung is present whether or not §3 uses it today. |
| `kestrel/src/shot.rs` | the capture harness and its own hand-written rasteriser. |
| `kestrel/src/gallery.rs` | `--gallery`: every §4 component at every state, with each colour annotated by its spec hex. |
| `kestrel-fs/src/` | scan, size, watcher, ops, model, error. No UI. |

**Note on `unsafe_code`.** Both crates set `unsafe_code = "forbid"`. `kestrel/Cargo.toml`
uses `rustix`'s safe `fs::statvfs` rather than `libc` partly for this reason — it was
already in the graph via `notify`, so it also adds zero packages.
