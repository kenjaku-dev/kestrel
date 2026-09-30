# Kestrel → Tauri + CSS: strangler migration plan

Decided 2026-09-30. Written by four research lanes (librarian ×1, explorer ×1,
oracle ×1) plus the orchestrator, all reconciled.

**This is not a rewrite.** `kestrel-fs` and the egui `kestrel` app stay exactly
where they are, keep passing their tests, and stay installable for the whole
migration. A new Tauri + CSS frontend is built alongside. You switch to it when
it's good enough — not when it's finished.

---

## 1. Why this shape

| Question | Answer | Why |
|---|---|---|
| Does the engine have to change? | **No.** `kestrel-fs` gets one additive progress hook. | Zero UI deps confirmed, `unsafe_code = forbid`, 121 tests all headless. Built UI-free in hour one specifically so it could outlive the UI. |
| Does the engine need `serde`? | **No.** DTOs live in `src-tauri`. | `SystemTime`, `Duration`, `io::Error` and `#[non_exhaustive]` all need hand-written wire forms anyway. A derive buys ~30 lines and costs format-agnosticism. |
| Keep egui behind a feature flag? | **No.** Separate crate. | Flags entangle two binaries' dep graphs. `cargo build -p kestrel` stays identical to today. |
| Frontend framework? | **Svelte 5 + TypeScript + Vite**, `safari13` target | 2–7 KB runtime, keyed `{#each}` for large lists, single-file components. Vanilla TS is smaller but forces hand-rolled reactivity for multi-select/sort/history. |

## 2. Target layout

```
kestrel-fs/          engine — untouched (one additive field)
kestrel/             egui app — the safety net, untouched
src-tauri/           new Tauri backend: commands + DTO + state
ui/                  new Svelte 5 + TS frontend
justfile             + run-tauri / install-tauri / perf-fixture
```

`just install` still installs the egui app to exactly the same paths. A new
`just install-tauri` is added alongside. Neither touches the other.

---

## 3. IPC contract

State payload, managed once at startup:

```rust
pub struct Backend {
    next_id: AtomicU64,
    scans:  Mutex<HashMap<JobId, Arc<Mutex<ScanHandle>>>>,
    sizes:  Mutex<HashMap<JobId, Arc<Mutex<SizeHandle>>>>,
    ops:    Mutex<HashMap<JobId, Arc<OpJob>>>,
    watcher: DirWatcher,                              // ONE, long-lived
    watches: Mutex<HashMap<JobId, WatchRelay>>,
}
pub type JobId = u64;   // backend-minted; the frontend never invents one
```

**The `!Sync` crux.** `ScanHandle`, `SizeHandle`, `WatchSubscription`,
`DirWatcher`, `Opening` each own an `mpsc::Receiver`, so they are `Send` but
**not** `Sync` — and `tauri::State<T>` requires `Send + Sync + 'static`. The
`Mutex` is not for locking `recv()` (those take `&self`); it is the mechanical
`!Sync → Sync` adapter. Per-job `Arc<Mutex<Handle>>` inside the outer map so
cancelling one job doesn't serialise all of them. **Do not change the engine to
"fix" this.**

Commands. Phase 1 set only; later phases add, never change:

```rust
scan_start(path, options, events: Channel<ScanEventDto>) -> JobId
scan_cancel(id)
stat(path) -> FileEntryDto
open_path(path) -> { entered_dir: bool }
```

No blocking `list_dir`. The streaming `scan_start` with `recursive: false`
**is** the listing — one code path, so the two UIs can't drift.

**Backpressure is load-bearing.** The engine cancels a worker when a `send`
fails (`scan.rs:311-318`, `size.rs:214-221`) — correct for egui's pump loop,
**wrong for IPC**, where the frontend may merely be busy. The pump task must
drain with `try_recv`, coalesce entry bursts at `BatchEnd` boundaries, and never
block on the frontend. A slow browser must not stop files from listing.

**The watcher is the exception:** one long-lived `DirWatcher` with
`subscribe()` / `add_path()` / `remove_path()`. Each `WatchSubscription` spawns
its own thread, and `DirWatcher::drop` **joins** that thread — N short-lived
watchers means N threads and blocking drops.

---

## 4. Progress reporting — the one engine change

`ops.rs` reports **nothing** today. The progress bar in the egui app is
invented by `kestrel/src/job.rs`. So it has to be added, and the engine is the
right layer — watching the destination from the backend would double-walk, race,
and duplicate I/O.

```rust
// kestrel-fs/src/ops.rs — additive, default None = today's behaviour
pub struct CopyProgress { pub current_src, pub current_dst: PathBuf,
                          pub bytes_copied: u64, pub files_done: usize }
pub struct CopyOptions { /* … */ pub progress: Option<Arc<dyn Fn(CopyProgress) + Send + Sync>> }
```

Emitted per 64 KiB chunk, where cancellation is already checked, plus per file.
New headless engine tests. Both UIs benefit — egui's `job.rs` can later drop its
invented progress for the real thing.

---

## 5. Phases — each ends with something you can run

| | Delivers | You can | Exit gate |
|---|---|---|---|
| **1 — Browse**<br>days, not weeks | Tauri shell + IPC + DTO layer + Svelte list with hand-rolled fixed-row windower + navigation + `open_path` + error rows | Browse `$HOME`, enter directories, <kbd>Enter</kbd> opens a file, permission errors show as rows | Cold-open `$HOME` and a 10k-file directory, navigate, open a text and an image file. `cargo test --workspace` green, `just install` untouched. **A scaffold is a failure.** |
| **2 — Live + sort/filter** | `watch_subscribe`, sort, filter-as-you-type, hidden toggle, breadcrumb | Leave it open while you `touch`/`mv` files and watch it update; sort a 10k directory | `mkdir` in a watched directory appears <500 ms with no rescan. Sort/filter <100 ms keystroke-to-paint on 10k. |
| **3 — Mutate** | Engine progress hook + copy/move/trash/delete + progress bar + cancel + collision dialog | Copy, move, trash with real progress, cancel mid-flight, resolve one collision | 1 GB copy shows monotonic progress; cancel leaves a partial and reports `Cancelled`; a cancelled move never deletes the source. **First phase you could switch to.** |
| **4 — Match + ship** | 1,359-line token spec → CSS custom properties, keyboard/a11y, size + preview + open-with, perf gate, AUR `-bin` package | Daily-drive Tauri, install from AUR | Perf gate passes; AUR install works on clean Arch; all Rust gates + `npm run build` green. **You switch when good enough.** |

The egui app stays your default until Phase 3 passes.

---

## 6. Performance gate — measured, not assumed

No published benchmark exists for a 10k-row list on WebKitGTK. So we measure
on this machine (Intel HD 530, Mesa, webkit2gtk-4.1 2.52.6).

- **Fixture:** `just perf-fixture` generates `/tmp/kestrel-perf/{10k,50k}`.
- **Interactions:** cold open → first paint; 5 s continuous scroll; one
  keystroke filter; sort-by-size click.
- **Thresholds** (windowed, fixed-height rows, overscan ~10): first paint
  <500 ms after `Complete`; scroll p95 frame <33 ms; keystroke-to-paint <100 ms;
  sort <200 ms.
- **Fallback rule: 2-day timebox.** If 10k scroll p95 is still >100 ms after
  windowing, the `safari13` target, and no per-row reactivity — **freeze Tauri
  list work.** egui stays your daily driver. No TanStack, no grid view, no
  further frontend phases until a new approach is proposed. **Do not tune for
  weeks.**

---

## 7. Skills per phase

**Phase 1**
- `modern-web-guidance` — **run first, mandatory.** Vite/WebKitGTK patterns rot
  fast; training data contains obsolete ones. Verify `Channel`, capabilities,
  and the `safari13` target rather than trusting recall.
- `api-and-interface-design` — load-bearing. The IPC/DTO/`JobId` contract is the
  thing everything else is built on.
- `frontend-ui-engineering` — the windower and Svelte reactivity.
- `verification-planning` — the prove-it script for the exit gate.

**Phase 2** — `frontend-ui-engineering`; add `systematic-debugging` if the
watcher self-trigger livelock ever recurs.

**Phase 3** — `test-driven-development` first (engine hook and collision tests),
then `api-and-interface-design` for the progress DTO, then
`code-review-and-quality` on the cancel path.

**Phase 4** — `design-system` (tokens → CSS custom properties: mechanical and
load-bearing), `frontend-ui-engineering` (keyboard/a11y), `verification-planning`
(perf gate), `code-review-and-quality` as the pre-switch audit. `simplify` after
each phase to kill abstractions that aren't earning their keep.

**Deliberately not used:** `tui-design` (no terminal UI). `ui-styling` and
`ui-ux-pro-max` are deferred until after the perf gate passes — the token spec
already decides the look, and taste work before performance is proven is waste.
Doc hunts only to verify a Tauri 2.12 signature at build time.

---

## 8. What could kill this

**1. Backpressure or lock ordering in the pump.** `Mutex<HashMap>` plus per-job
`Mutex` invites ordering bugs and leaked `JobId`s. *Week-one sign:* thread count
in `/proc` grows while you navigate, or rapid directory switching stalls scans.

**2. WebKitGTK list performance is genuinely insufficient.** Oldest of the three
Tauri backends, missing newest CSS/JS, no published 10k number. *Week-one sign:*
a windowed 10k list still scrolls visibly chunked. Enforce the two-day timebox.

**3. Split attention.** Rebuilding preview/collision/settings to "finish" Tauri
instead of strangling slice-by-slice, while egui rots from neglect. *Week-one
sign:* a Phase 1 diff touching `kestrel/` beyond additive changes, or pulling in
libraries Phase 1 doesn't need. **Rule: a Phase 1 diff outside `src-tauri/`,
`ui/`, and `justfile` is a reject.**

---

## 9. Gates — both UIs, every phase

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace          # 464 today, must never go down
cargo build --release           # egui release, unchanged
npm run build                   # frontend, SEPARATE gate
```

The frontend gate is deliberately separate so broken CSS can never redden the
Rust suite. Existing tests must pass in every phase — that is the guarantee the
egui app stays usable.