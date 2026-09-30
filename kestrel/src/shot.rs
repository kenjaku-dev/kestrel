//! `--screenshot`: render the UI headlessly and write a PNG.
//!
//! # Why this exists
//!
//! The design system has to be *seen* before it is built on, and a screenshot
//! grabbed off the desktop does not survive contact with a real session — other
//! windows cover it, the compositor may be scaling it, and the capture depends on
//! what else happened to be on screen. This renders the same [`KestrelApp`]
//! through [`egui::Context::run_ui`], tessellates it with egui's own code, and
//! rasterises the result to a PNG. No window, no compositor, no timing.
//!
//! That makes theme review a diffable artifact: `--gallery --screenshot` before
//! and after a token change gives two files that can be compared directly.
//!
//! # What it is not
//!
//! It is a *review* tool, not a test harness. It renders a fixed number of
//! frames at a fixed size with no input, so it proves the theme composes and the
//! layout is sane; it does not prove interaction works. For that, run the app.
//!
//! # The rasteriser
//!
//! egui returns a `FullOutput` of `Shape`s; `Context::tessellate` turns those
//! into triangle `Mesh`es that reference the font atlas by UV. Compositing them
//! means: collect the texture deltas, then for each triangle, scanline-fill with
//! barycentric UV interpolation and bilinear texture sampling, in premultiplied
//! sRGB — which is what `ecolor` stores. That is the whole reason this file
//! exists rather than a screenshot crate, and it is a deliberate zero-dependency
//! trade: the alternative is a `save_file` feature on the `image` crate that
//! `egui_extras` already pulls in, which would be three lines instead of a
//! hundred and one more thing in the dependency graph.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use egui::epaint::{ClippedPrimitive, ImageData, Primitive, TextureId, Vertex};
use egui::{Color32, ColorImage, Context, Pos2, Rect, Vec2, vec2};

use crate::app::{KestrelApp, Options, Scene};

/// The size the file-manager capture is rendered at, in logical points.
///
/// Deliberately not the window's size: a capture must be reproducible, and a
/// layout that only works at 1906px wide is a layout that does not work.
const BROWSER_SIZE: Vec2 = vec2(1200.0, 800.0);

/// The size the gallery capture is rendered at.
///
/// Tall, because the gallery is a scrolling document and a truncated capture
/// only shows the first third of the design system.
const GALLERY_SIZE: Vec2 = vec2(1200.0, 2600.0);

/// How many passes to run before capturing.
///
/// Pass 1 lays the UI out and builds the font atlas, so pass 2 is the first whose
/// text is guaranteed correct. Pass 3 settles anything that only resolves on the
/// second — most importantly the file list, whose first scan drains in pass 1 and
/// whose rows are painted in pass 2.
const PASSES: usize = 3;

/// How long a scene may spend settling, in milliseconds.
///
/// A scene that cannot settle is a bug in the scene, not something to paper over
/// with a longer sleep: the bound is here so a broken capture fails loudly
/// instead of hanging the review.
const SETTLE_BUDGET_MS: u64 = 5_000;

/// Where the capture's synthetic clock starts, in seconds.
///
/// Arbitrary but *late*: long enough to be past the longest animation the app
/// has (the 380ms §2.11 `motion.deliberate`), so a capture is the settled frame
/// and never a frame caught mid-fade. See [`run_frame`].
const CAPTURE_TIME_BASE: f64 = 10.0;

/// How far the capture's clock advances per frame, in seconds.
///
/// 50ms: long enough that the settling loop gets past every animation in a few
/// frames, short enough that fifty of them is 2.5s of virtual time for a run
/// that takes a few hundred milliseconds of real time.
const CAPTURE_STEP: f64 = 0.05;

/// Renders the app and writes a PNG to `path`.
///
/// # Errors
///
/// Returns the I/O error from writing `path`, or a message describing why the
/// scene could not be driven into the state it names. Rendering a settled frame
/// itself cannot fail: it allocates an in-memory framebuffer and touches no
/// display server.
pub fn capture(
    opts: Options,
    scene: Scene,
    size: Option<Vec2>,
    path: &Path,
) -> std::io::Result<()> {
    let image = render(opts, scene, size)?;
    save_png(&image, path)
}

/// Renders the app offscreen and returns the framebuffer.
///
/// Split out from [`capture`] so a test can compare two renders *in memory*,
/// without writing files and without shelling out to the binary. That matters
/// for the `--light`/`--dark` regression: the failure mode being guarded against
/// is "the two flags produce the same picture", and the cheapest honest check is
/// to render both and compare the pixels.
///
/// # Errors
///
/// Returns `io::ErrorKind::TimedOut` when a scene never settles. A capture that
/// gives up loudly is worth more than a PNG of a pane that still says
/// "Loading…", which is the failure this whole module exists to make impossible.
pub fn render(opts: Options, scene: Scene, size: Option<Vec2>) -> std::io::Result<ColorImage> {
    let size = size.unwrap_or(if opts.gallery {
        GALLERY_SIZE
    } else {
        BROWSER_SIZE
    });
    let ctx = Context::default();

    // The font set is installed before the first pass, exactly as the real
    // `AppCreator` does it, or the capture would be typeset in egui's default
    // families rather than the ones `fonts::install` selects.
    crate::tokens::fonts::install(&ctx);
    egui_extras::install_image_loaders(&ctx);

    let mut opts = opts;
    // Bound outside the `if` below on purpose: the app watches this directory
    // for the whole capture, and a block-scoped guard would delete it before
    // `for_capture` runs — an empty listing, no dialog, no scrim, and a watch
    // that fails with "no path found".
    let _fixture_guard = if !opts.gallery && scene != Scene::Browser {
        // A scene is only meaningful against a known listing, so the start
        // directory becomes the fixture unless the caller named one. Without
        // this, `--scene preview-image` in the user's home directory renders an
        // empty preview pane and reviews nothing.
        let fixture = fixture()?;
        if opts.start_dir.is_none() {
            opts.start_dir = Some(fixture.path().to_path_buf());
        }
        Some(fixture)
    } else {
        None
    };

    let mut app = KestrelApp::for_capture(ctx.clone(), opts);
    let mut textures = TextureBook::default();
    let mut image = blank(size, 1.0);

    // Two settling phases, because a scene names a *row* and a row only exists
    // once the scan has delivered. Applying the scene before the listing arrives
    // would silently select nothing — and a capture that quietly reviews the
    // wrong state is worse than one that fails.
    let mut clock = CAPTURE_TIME_BASE;
    if !settle_until(&ctx, &mut app, size, &mut textures, &mut clock, |a| {
        a.is_listed()
    }) {
        return Err(timed_out(scene, "the listing"));
    }
    if scene != Scene::Browser {
        app.apply_scene(scene);
    }
    if !settle_until(&ctx, &mut app, size, &mut textures, &mut clock, |a| {
        a.is_settled()
    }) {
        return Err(timed_out(scene, "the scene"));
    }

    for _ in 0..PASSES {
        let mut output = run_frame(&ctx, &mut app, size, &mut clock);
        textures.absorb(&output.textures_delta);
        let shapes = std::mem::take(&mut output.shapes);
        let ppp = output.pixels_per_point;
        output.drop_without_applying_deltas();
        // Every pass is rasterized and the last one kept. The earlier passes are
        // wasted work, but they cost a few hundred microseconds and they make the
        // loop obviously correct: what ends up in the file is exactly what the
        // final pass painted.
        image = rasterize(&ctx, shapes, ppp, size, &textures);
    }

    Ok(image)
}

/// One frame of the app, with the input a capture always supplies.
///
/// `clock` advances by a fixed step per frame rather than tracking the wall
/// clock, so the capture is reproducible *and* animations are settled. See
/// [`CAPTURE_TIME_BASE`].
fn run_frame(ctx: &Context, app: &mut KestrelApp, size: Vec2, clock: &mut f64) -> egui::FullOutput {
    let now = *clock;
    *clock += CAPTURE_STEP;
    let raw = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
        // §4.7 gives a dialog a 140ms enter animation and a 260ms scrim fade,
        // and egui's `Area` fades whatever it owns. A capture is three frames
        // drawn in microseconds, so with the clock at zero every dialog rendered
        // at roughly a tenth of its opacity — the fill measured 232 where
        // `surface.raised` is 255, and the scrim measured 209 where the spec's
        // 38% over the list is 165. Every colour anyone reviewed in a dialog
        // capture was wrong, and it looked like a theming problem rather than a
        // timing one.
        //
        // A *constant* clock is the other half of the bug and just as
        // confusing: egui computes a fade as `time - last_became_visible_at`,
        // and an `Area` stamps that on the frame it first appears — so a clock
        // that never moves leaves every dialog pinned at zero opacity for ever.
        // The clock therefore has to move, and it moves by a fixed step from a
        // base past every animation the app has. Deterministic, and settled.
        time: Some(now),
        // No `pixels_per_point` override: egui 0.36's `RawInput` has no such
        // field — the scale comes from `Context::pixels_per_point`, which
        // defaults to 1.0 for a `Context::default()`. That is exactly what
        // this wants: a capture at 1.0 is the size the tokens are written
        // in, so a 26px row is 26 pixels and the density decisions can be
        // checked directly.
        ..Default::default()
    };
    ctx.run_ui(raw, |ui| app.draw(ui))
}

/// Runs frames until `ready` says so, or the budget runs out.
///
/// Returns `true` when it settled. Sleeping between frames is not a stall in the
/// product — this is the review tool, it owns the process, and there is no user
/// to be responsive to. The frame loop *itself* still never blocks; that is a
/// property of [`KestrelApp::draw`] and is unaffected by how often this calls it.
fn settle_until(
    ctx: &Context,
    app: &mut KestrelApp,
    size: Vec2,
    textures: &mut TextureBook,
    clock: &mut f64,
    ready: impl Fn(&KestrelApp) -> bool,
) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(SETTLE_BUDGET_MS);
    loop {
        let output = run_frame(ctx, app, size, clock);
        // The texture book is shared with the drawing passes, and it has to be:
        // the font atlas is *built* by the frames that lay the UI out, and it is
        // delivered one frame late. Discarding the settling frames' deltas
        // therefore leaves the book empty for every drawing frame too, and every
        // glyph in the capture rasterises as a solid block — a caption that
        // looks like a row of little bricks, which is not an error anyone can
        // read past as "the renderer is broken".
        textures.absorb(&output.textures_delta);
        output.drop_without_applying_deltas();
        if ready(app) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(4));
    }
}

/// The error a capture returns when a scene never reaches its state.
fn timed_out(scene: Scene, what: &str) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        format!("scene {scene:?} never finished settling {what} within {SETTLE_BUDGET_MS}ms"),
    )
}

// ----------------------------------------------------------------------------
// Scene fixtures
// ----------------------------------------------------------------------------

/// A unique scratch directory that removes itself.
///
/// `tempfile` is a dev-dependency and the fixture backs the real `--screenshot`
/// path, so this is the small version rather than a new production dependency:
/// pid plus a process-wide counter, cleaned up on drop.
struct FixtureDir {
    path: PathBuf,
}

/// Distinguishes one fixture from another within this process.
static FIXTURE_SEQ: AtomicU64 = AtomicU64::new(0);

impl FixtureDir {
    /// Creates the directory, with the `assets` subfolder the pane has a case for.
    fn new() -> std::io::Result<Self> {
        let n = FIXTURE_SEQ.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("kestrel-shot-fixture-{}-{n}", std::process::id()));
        std::fs::create_dir_all(path.join("assets"))?;
        Ok(Self { path })
    }

    /// Where the fixture files live.
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for FixtureDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Builds the directory a scene is captured against.
///
/// The point of a fixture is that it contains **one of everything the pane has a
/// case for**: a text file with several lines and a comment, an image, a file
/// over [`crate::preview::MAX_PREVIEW_BYTES`], a folder, a file with no
/// extension. A capture of `$HOME` proves the layout; a capture of a directory
/// chosen for what is in it proves the components.
///
/// A fresh [`FixtureDir`] every time, so a scene never captures a
/// listing that a previous run left behind — the failure mode being a
/// screenshot that changes when nothing in the app changed — and so parallel
/// tests never share one mutable directory: rebuilding a shared path while
/// another test's watcher is settled on it reads as an endless stream of
/// external changes, and a working watcher re-lists forever.
///
/// # Errors
///
/// Returns the I/O error from creating the directory or writing a fixture file.
fn fixture() -> std::io::Result<FixtureDir> {
    let dir = FixtureDir::new()?;

    std::fs::write(
        dir.path().join("notes.md"),
        "# Kestrel\n\nThe design tokens live in /tmp/opencode/kestrel-tokens.md.\n\n\
         - density is 26px rows\n- the accent is pine teal\n- selection is a flat tint\n\n\
         // a comment, so the highlighter has something to colour\nlet width = 720.0;\n\
         let label = \"Modified\";\n",
    )?;
    std::fs::write(dir.path().join("diagram.png"), sample_png())?;
    // Comfortably over the cap, so `PreviewTooLarge` has a file to degrade on
    // *and* the pane's two size numbers are distinguishable. A file 4 KiB over
    // the limit formats as "1.0 MB" and the limit formats as "1.0 MB", which
    // makes the degradation sentence look self-contradictory in a capture.
    std::fs::write(
        dir.path().join("archive.tar"),
        vec![0u8; (crate::preview::MAX_PREVIEW_BYTES + 512 * 1024) as usize],
    )?;
    std::fs::write(dir.path().join("LICENSE"), b"MIT\n")?;
    std::fs::write(dir.path().join("run.sh"), b"#!/bin/sh\necho hello\n")?;
    Ok(dir)
}

/// A small, real PNG: 320x240, an accent-tinted field in three bands with a
/// left-to-right ramp.
///
/// Written from this file's own encoder rather than pasted in as hex, because a
/// hand-copied byte string is exactly the kind of fixture that is one wrong
/// nibble away from testing the *decoder's* error path instead of the preview
/// pane's success path. The size is chosen so the image is **larger** than the
/// pane's body and therefore has to be scaled down: a 1x1 or 64x48 fixture proves
/// the decoder works and nothing about whether the scaling, the aspect ratio or
/// the centring is right, which is the thing worth looking at.
fn sample_png() -> Vec<u8> {
    let (w, h) = (320u32, 240u32);
    let image = ColorImage::new([w as usize, h as usize], {
        let mut px = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                let band = y / 12;
                let tint = match band % 3 {
                    0 => Color32::from_rgb(0x0E, 0x6B, 0x5F),
                    1 => Color32::from_rgb(0x33, 0xA8, 0x94),
                    _ => Color32::from_rgb(0xC9, 0xE6, 0xDF),
                };
                // A left-to-right ramp so scaling artefacts (a wrong aspect
                // ratio, a nearest-neighbour magnify) are visible in the capture.
                let t = x as f32 / w as f32;
                let mix = |c: u8, f: u8| {
                    let v = f32::from(c) * (1.0 - t) + f32::from(f) * t;
                    (v.round().clamp(0.0, 255.0)) as u8
                };
                px.push(Color32::from_rgb(
                    mix(tint.r(), 0xFC),
                    mix(tint.g(), 0xFB),
                    mix(tint.b(), 0xF9),
                ));
            }
        }
        px
    });
    encode_png(&image)
}

/// The PNG byte stream for `image`.
///
/// The same stored-deflate scheme [`save_png`] uses, factored out so a fixture
/// and a capture are encoded by one piece of code.
fn encode_png(image: &ColorImage) -> Vec<u8> {
    let [w, h] = image.size;
    let mut raw = Vec::with_capacity(h * (1 + w * 4));
    for y in 0..h {
        raw.push(0u8);
        for x in 0..w {
            raw.extend_from_slice(&image.pixels[y * w + x].to_array());
        }
    }
    let mut png = Vec::new();
    png.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &zlib_stored(&raw));
    chunk(&mut png, b"IEND", &[]);
    png
}

// ----------------------------------------------------------------------------
// Texture collection
// ----------------------------------------------------------------------------

/// The textures a frame's meshes reference, keyed by id.
///
/// egui hands them over as a delta against whatever the previous frame left in
/// the GPU. There is no GPU here, so the book accumulates them across passes: a
/// glyph rasterised in pass 1 is still sampled in pass 3.
#[derive(Default)]
struct TextureBook {
    images: HashMap<TextureId, ImageData>,
}

impl TextureBook {
    /// Folds one frame's texture deltas into the book.
    ///
    /// # Both kinds of delta are applied
    ///
    /// `pos: None` is a full rewrite — the atlas's first frame, and every time
    /// it doubles in height. `pos: Some(_)` is a **partial** update, and that is
    /// the common case: `TextureAtlas::take_delta` only reports `EVERYTHING`
    /// when the atlas has been reallocated, so once it has stopped growing every
    /// new glyph arrives as a small dirty rectangle.
    ///
    /// The previous version dropped partial deltas on the grounds that "nothing
    /// in this app uses a moving texture". That is true of *images* and false of
    /// the **font atlas**, which is exactly the texture that grows by accretion:
    /// drop its partial deltas and the book keeps whatever the last reallocation
    /// looked like, so every glyph rasterised after that point samples the wrong
    /// texels. It does not look like missing text — it looks like *solid boxes*,
    /// which is a far more convincing lie, and every icon in every capture was
    /// rendering as a filled rectangle.
    fn absorb(&mut self, delta: &egui::TexturesDelta) {
        for (id, deltas) in &delta.set {
            for image_delta in deltas {
                match image_delta.pos {
                    None => {
                        self.images.insert(*id, image_delta.image.clone());
                    }
                    Some(pos) => {
                        // A partial update is a blit. The destination has to
                        // exist already: a partial delta before any full one
                        // would be epaint writing into a texture this book has
                        // never been told the size of, and guessing the size is
                        // how a capture ends up sampling off the end of an image.
                        let Some(existing) = self.images.get_mut(id) else {
                            continue;
                        };
                        // `ImageData` is a single-variant enum in epaint 0.36,
                        // so these two patterns are irrefutable today. The
                        // `#[allow]` documents that as a version fact rather than
                        // as an oversight — the same reasoning as `sample`.
                        #[allow(irrefutable_let_patterns)]
                        let ImageData::Color(dst) = existing;
                        #[allow(irrefutable_let_patterns)]
                        let ImageData::Color(src) = &image_delta.image;
                        // The atlas is shared with nobody here, but it is handed
                        // over behind an `Arc`, so the copy-on-write is what gets
                        // the mutation in without cloning the whole atlas.
                        blit(Arc::make_mut(dst), src, pos);
                    }
                }
            }
        }
    }

    /// Samples a texture at normalised `uv`, bilinearly.
    ///
    /// Returns premultiplied sRGBA. `WHITE_UV` — the 1x1 white pixel every
    /// untextured shape points at — resolves to opaque white, which is what
    /// makes `rect_filled` work without a special case.
    fn sample(&self, id: TextureId, uv: Vec2) -> [f32; 4] {
        let Some(image) = self.images.get(&id) else {
            // An unknown texture is white, not transparent: a missing texture
            // showing as a hole would read as "the theme is broken" when in fact
            // the book is.
            return [1.0, 1.0, 1.0, 1.0];
        };
        // `ImageData` is a single-variant enum in epaint 0.36, so this match is
        // irrefutable today; the `else` arm is what keeps it compiling if a
        // second variant is ever added, and documents the intended fallback.
        #[allow(irrefutable_let_patterns)]
        let ImageData::Color(color) = image else {
            return [1.0, 1.0, 1.0, 1.0];
        };
        let [w, h] = color.size;
        if w == 0 || h == 0 {
            return [1.0, 1.0, 1.0, 1.0];
        }
        // Bilinear: text at 11-13px is exactly where nearest-neighbour sampling
        // looks wrong, and a theme review screenshot with crunchy text is worse
        // than no screenshot.
        let x = uv.x * w as f32 - 0.5;
        let y = uv.y * h as f32 - 0.5;
        let x0 = x.floor();
        let y0 = y.floor();
        let fx = x - x0;
        let fy = y - y0;
        let fetch = |ix: i32, iy: i32| {
            let ix = ix.clamp(0, w as i32 - 1) as usize;
            let iy = iy.clamp(0, h as i32 - 1) as usize;
            texel(color, ix, iy)
        };
        let c00 = fetch(x0 as i32, y0 as i32);
        let c10 = fetch(x0 as i32 + 1, y0 as i32);
        let c01 = fetch(x0 as i32, y0 as i32 + 1);
        let c11 = fetch(x0 as i32 + 1, y0 as i32 + 1);
        let mut out = [0.0f32; 4];
        for ch in 0..4 {
            let top = c00[ch] + (c10[ch] - c00[ch]) * fx;
            let bottom = c01[ch] + (c11[ch] - c01[ch]) * fx;
            out[ch] = top + (bottom - top) * fy;
        }
        out
    }
}

/// Copies `src` into `dst` at `pos`, clipping at the destination's edges.
///
/// Every index is bounds-checked rather than asserted: a partial delta whose
/// rectangle runs off the edge of a texture that was resized since the book last
/// saw it is a possibility, not an impossibility, and a screenshot tool that
/// panics is a screenshot tool that produces no screenshot.
fn blit(dst: &mut ColorImage, src: &ColorImage, pos: [usize; 2]) {
    for y in 0..src.size[1] {
        for x in 0..src.size[0] {
            let Some(pixel) = src.pixels.get(y * src.size[0] + x) else {
                continue;
            };
            let tx = pos[0] + x;
            let ty = pos[1] + y;
            if let Some(slot) = dst.pixels.get_mut(ty * dst.size[0] + tx) {
                *slot = *pixel;
            }
        }
    }
}

/// One pixel of a `ColorImage`, as normalised premultiplied sRGBA.
///
/// `pixels` is indexed by *element*, not by byte — `Color32` is a 4-byte type
/// and `Vec<Color32>` already counts in units of one.
fn texel(image: &ColorImage, x: usize, y: usize) -> [f32; 4] {
    let Some(px) = image.pixels.get(y * image.size[0] + x) else {
        // Unreachable for any in-range x/y, which `fetch` clamps. Returning
        // transparent rather than panicking keeps a malformed texture from
        // taking down a screenshot that exists to be looked at.
        return [0.0; 4];
    };
    [
        f32::from(px.r()) / 255.0,
        f32::from(px.g()) / 255.0,
        f32::from(px.b()) / 255.0,
        f32::from(px.a()) / 255.0,
    ]
}

// ----------------------------------------------------------------------------
// Rasteriser
// ----------------------------------------------------------------------------

/// A blank framebuffer of the right size.
fn blank(size: Vec2, ppp: f32) -> ColorImage {
    let [w, h] = image_px(size, ppp);
    ColorImage::new([w, h], vec![Color32::TRANSPARENT; w * h])
}

fn image_px(size: Vec2, ppp: f32) -> [usize; 2] {
    [
        (size.x * ppp).round().max(1.0) as usize,
        (size.y * ppp).round().max(1.0) as usize,
    ]
}

/// Tessellates and composites one frame into an RGBA image.
fn rasterize(
    ctx: &Context,
    shapes: Vec<egui::epaint::ClippedShape>,
    ppp: f32,
    size: Vec2,
    textures: &TextureBook,
) -> ColorImage {
    let mut image = blank(size, ppp);
    for primitive in ctx.tessellate(shapes, ppp) {
        // `Callback` primitives are user shader hooks. This app paints none —
        // every shape comes from a token and a `Painter` call — so skipping them
        // is complete, not a shortcut.
        // Borrow rather than move: `clip_rect` lives on the same struct.
        let Primitive::Mesh(mesh) = &primitive.primitive else {
            continue;
        };
        draw_mesh(&mut image, &primitive, mesh, ppp, textures);
    }
    image
}

/// Draws one mesh, scissored to its clip rectangle.
#[allow(clippy::too_many_arguments)]
fn draw_mesh(
    image: &mut ColorImage,
    clipped: &ClippedPrimitive,
    mesh: &egui::epaint::Mesh,
    ppp: f32,
    textures: &TextureBook,
) {
    debug_assert_eq!(mesh.indices.len() % 3, 0, "egui meshes are triangle lists");
    let [w, h] = image.size;
    let clip = clipped.clip_rect;
    // egui's clip rect is in points, inclusive-exclusive, already snapped to
    // integers by the tessellator.
    let cx0 = (clip.min.x * ppp).floor().max(0.0) as i64;
    let cy0 = (clip.min.y * ppp).floor().max(0.0) as i64;
    let cx1 = ((clip.max.x * ppp).ceil() as i64).min(w as i64);
    let cy1 = ((clip.max.y * ppp).ceil() as i64).min(h as i64);
    if cx0 >= cx1 || cy0 >= cy1 {
        return;
    }

    for tri in mesh.indices.chunks_exact(3) {
        let (Some(a), Some(b), Some(c)) = (
            mesh.vertices.get(tri[0] as usize),
            mesh.vertices.get(tri[1] as usize),
            mesh.vertices.get(tri[2] as usize),
        ) else {
            continue;
        };
        // to pixels
        let a = (a.pos * ppp, a.uv, vertex_color(a));
        let b = (b.pos * ppp, b.uv, vertex_color(b));
        let c = (c.pos * ppp, c.uv, vertex_color(c));

        let min_x = (a.0.x.min(b.0.x).min(c.0.x).floor() as i64).max(cx0);
        let max_x = ((a.0.x.max(b.0.x).max(c.0.x).ceil() as i64) + 1).min(cx1);
        let min_y = (a.0.y.min(b.0.y).min(c.0.y).floor() as i64).max(cy0);
        let max_y = ((a.0.y.max(b.0.y).max(c.0.y).ceil() as i64) + 1).min(cy1);

        let area = edge(a.0, b.0, c.0);
        if area.abs() < 1e-9 {
            // Degenerate triangle. egui emits these for collapsed rects; drawing
            // them would divide by zero.
            continue;
        }

        for y in min_y..max_y {
            for x in min_x..max_x {
                // Sample at the pixel centre, which is the standard
                // pixel-centre rasterisation convention and what egui's own GPU
                // backend does.
                let p = Pos2::new(x as f32 + 0.5, y as f32 + 0.5);
                // Barycentric coordinates, normalised by the signed area. The
                // sign is folded in so clockwise and counter-clockwise winding
                // both work — egui explicitly does not guarantee winding order.
                let w0 = edge(b.0, c.0, p) / area;
                let w1 = edge(c.0, a.0, p) / area;
                let w2 = edge(a.0, b.0, p) / area;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                // egui's shapes carry their own feathered alpha in the vertex
                // colours, so there is no coverage term here: the triangle is
                // the coverage, and the edges are already soft.
                // `Pos2 * f32` yields a `Vec2`; the UV is a direction, not a
                // point, so `Vec2` is the right home for the interpolated value.
                let uv = a.1.to_vec2() * w0 + b.1.to_vec2() * w1 + c.1.to_vec2() * w2;
                let vc = [
                    a.2[0] * w0 + b.2[0] * w1 + c.2[0] * w2,
                    a.2[1] * w0 + b.2[1] * w1 + c.2[1] * w2,
                    a.2[2] * w0 + b.2[2] * w1 + c.2[2] * w2,
                    a.2[3] * w0 + b.2[3] * w1 + c.2[3] * w2,
                ];
                blend(
                    image,
                    x as usize,
                    y as usize,
                    vc,
                    uv,
                    textures,
                    mesh.texture_id,
                );
            }
        }
    }
}

/// Twice the signed area of the triangle `a b p`.
fn edge(a: Pos2, b: Pos2, p: Pos2) -> f32 {
    (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x)
}

fn vertex_color(v: &Vertex) -> [f32; 4] {
    [
        f32::from(v.color.r()) / 255.0,
        f32::from(v.color.g()) / 255.0,
        f32::from(v.color.b()) / 255.0,
        f32::from(v.color.a()) / 255.0,
    ]
}

/// Source-over composite of one premultiplied source pixel onto the framebuffer.
///
/// `ecolor` stores premultiplied sRGBA, so the blend is the short one:
/// `dst = src + dst * (1 - src.a)`. The texture sample is multiplied by the
/// interpolated vertex colour, which is where a mesh's own feathered alpha and
/// its `text.primary` / `state.selected` tint both come from.
fn blend(
    image: &mut ColorImage,
    x: usize,
    y: usize,
    vertex: [f32; 4],
    uv: Vec2,
    textures: &TextureBook,
    texture_id: TextureId,
) {
    let sampled = textures.sample(texture_id, uv);
    let mut src = [0.0f32; 4];
    for ch in 0..4 {
        src[ch] = sampled[ch] * vertex[ch];
    }
    // Premultiplied sources can only *darken* their own RGB; an unpremultiplied
    // texel (alpha 0, rgb 255) would otherwise punch white holes through the
    // rounded corners of a `rect_filled`.
    for ch in 0..3 {
        src[ch] = src[ch].min(src[3]);
    }
    let alpha = src[3].clamp(0.0, 1.0);
    if alpha <= 0.0 {
        return;
    }
    let index = y * image.size[0] + x;
    let dst = image.pixels[index];
    let inv = 1.0 - alpha;
    let out = [
        clamp_u8(src[0] + f32::from(dst.r()) / 255.0 * inv),
        clamp_u8(src[1] + f32::from(dst.g()) / 255.0 * inv),
        clamp_u8(src[2] + f32::from(dst.b()) / 255.0 * inv),
        clamp_u8(alpha + f32::from(dst.a()) / 255.0 * inv),
    ];
    image.pixels[index] = Color32::from_rgba_premultiplied(out[0], out[1], out[2], out[3]);
}

fn clamp_u8(v: f32) -> u8 {
    (v * 255.0).round().clamp(0.0, 255.0) as u8
}

// ----------------------------------------------------------------------------
// PNG output
// ----------------------------------------------------------------------------

/// Writes a `ColorImage` as an 8-bit RGBA PNG.
///
/// A hand-rolled encoder with *stored* (uncompressed) deflate blocks: the point
/// of this file is reviewing a theme, not shipping an image, and an
/// uncompressed PNG needs only CRC-32 and Adler-32 — neither of which justifies
/// a dependency. A 1200x1000 capture is a few MB on disk, which is fine.
pub(crate) fn save_png(image: &ColorImage, path: &Path) -> std::io::Result<()> {
    std::fs::write(path, encode_png(image))
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

/// A zlib stream of stored deflate blocks.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01]; // CMF/FLG: deflate, 32K window, no preset dict
    if data.is_empty() {
        out.extend_from_slice(&[0x01, 0x00, 0x00, 0xFF, 0xFF]);
    } else {
        let mut chunks = data.chunks(0xFFFF).peekable();
        while let Some(chunk) = chunks.next() {
            let final_block = chunks.peek().is_none();
            out.push(u8::from(final_block));
            let len = chunk.len() as u16;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&(!len).to_le_bytes());
            out.extend_from_slice(chunk);
        }
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65_521;
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + u32::from(byte)) % MOD;
        b = (b + a) % MOD;
    }
    (b << 16) | a
}

/// A `Context` with the real font set installed **and** at least one pass run,
/// which is what `Context::fonts` requires before it will hand out a
/// `FontsView`.
///
/// Lives here because `shot` already owns the "build a headless context"
/// machinery; every module that needs to inspect the font atlas in a test uses
/// this rather than each rolling its own.
#[cfg(test)]
pub(crate) fn ctx_with_fonts() -> egui::Context {
    let ctx = egui::Context::default();
    crate::tokens::fonts::install(&ctx);
    // One empty pass: lays nothing out, but populates the font atlas and the
    // `fonts` slot that `Context::fonts`/`fonts_mut` read from. The output's
    // texture deltas must be applied or explicitly discarded, or epaint panics
    // on drop — a headless test has no renderer to apply them.
    let output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(16.0, 16.0),
            )),
            ..Default::default()
        },
        |_ui| {},
    );
    output.drop_without_applying_deltas();
    ctx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app;

    /// The leftmost interior column of the preview pane, measured from the
    /// picture.
    ///
    /// The search is for the pane's own fill colour down the pane's full height,
    /// and a *majority* of that height rather than a single pixel of it. The
    /// majority is what makes this work: `surface.panel` also paints the
    /// toolbar, the status bar and the column header, so a plain
    /// "first pixel of this colour to the right of centre" finds the column
    /// header at x = w/2 and calls a 600px pane. The pane is the only region in
    /// the right-hand half that is this colour for nearly every row of the band.
    ///
    /// The result is the first such column, which is one or two pixels inside
    /// the pane's stroke and separator. That is deliberate — a caller gets a
    /// column that is definitely the pane and definitely not a border.
    ///
    /// Walking in from the window's right edge — which is what the older version
    /// of these tests did — measures the pane's 1px frame stroke instead,
    /// because that stroke is the first pixel from the right that is not the
    /// fill. Every assertion built on that number was therefore checking a
    /// 20px sliver at the extreme edge of the window and calling it the pane;
    /// the list-vs-pane test was, in that reading, checking that the *pane* did
    /// not paint into the *pane*.
    fn pane_left(image: &ColorImage) -> usize {
        let w = image.width();
        let h = image.height();
        let px = |x: usize, y: usize| {
            let p = image.pixels[y * w + x];
            (p.r(), p.g(), p.b())
        };
        let top = crate::tokens::metric::TOOLBAR as usize;
        let bottom = h - crate::tokens::component::STATUSBAR_HEIGHT as usize;
        // Well inside the pane vertically, and well inside it horizontally: the
        // pane is at its narrowest when it is a rail, and 20px from the right
        // edge is inside all three states.
        let fill = px(w - 20, top + 20);
        let majority = (bottom - top) / 2;
        (w / 2..w)
            .find(|&x| ((top..bottom).filter(|&y| px(x, y) == fill).count()) > majority)
            .unwrap_or_else(|| panic!("no preview pane fill found in {w}x{h}"))
    }

    /// An unselected pane is a rail — and the list gets its columns back.
    ///
    /// This is the bug this whole state exists for. The pane used to cost its
    /// full configured width (280 by default, more after a drag) to hold one eye
    /// icon and the words "Nothing selected", which starved a 972px window's
    /// list down to ~410px — and at that width [`columns::columns_for`] drops
    /// the `Modified` timestamp. The column logic is right; the pane was wrong.
    ///
    /// So this measures the picture, not the layout function, and then feeds the
    /// width it measured back into `columns_for`. The second half is the payoff:
    /// it says the rail is not merely prettier, it is what puts a column back
    /// into the list.
    #[test]
    fn an_unselected_pane_is_a_narrow_rail() {
        let strip = crate::dialog::preview_metrics::STRIP_WIDTH;
        // `Scene::Browser` *is* the no-selection scene — see `apply_scene` — so
        // it is here as well as `PreviewEmpty` to prove the two agree.
        for scene in [Scene::Browser, Scene::PreviewEmpty] {
            let image = render(app::Options::default(), scene, Some(BROWSER_SIZE)).expect("render");
            let left = pane_left(&image);
            let fill = (image.width() - left) as f32;
            assert!(
                fill <= strip,
                "{scene:?}: the pane is {fill}px wide with nothing selected, so it is \
                 still paying full width for an empty state (strip is {strip})"
            );
            assert!(
                fill >= strip - 6.0,
                "{scene:?}: the pane collapsed to {fill}px, below the {strip}px rail — the \
                 icon would be clipped rather than centred"
            );
            // The point of the rail. The list is what is left of the window once
            // the sidebar has taken its 200.
            let list = left as f32 - crate::tokens::metric::SIDEBAR_WIDTH;
            for show_kind in [false, true] {
                assert!(
                    crate::columns::columns_for(list, show_kind).modified,
                    "{scene:?}: at {list}px of list the Modified column is still dropped — \
                     the rail did not buy it back (show_kind: {show_kind})"
                );
            }
        }
    }

    /// A selected row brings the pane straight back to its configured width.
    ///
    /// The other half of the transition, and the half that a strip-only test
    /// would leave uncovered: a pane that only ever collapsed would be a pane
    /// that could not be read. `PreviewText` has a focused row, so the plan is
    /// [`PanePlan::Full`], and 280 is `PREVIEW_DEFAULT` — which is also the
    /// check that the settings stepper's default is what the pane actually uses.
    #[test]
    fn a_selected_pane_is_its_configured_width() {
        let full = crate::settings::PREVIEW_DEFAULT;
        let image = render(
            app::Options::default(),
            Scene::PreviewText,
            Some(BROWSER_SIZE),
        )
        .expect("render");
        let fill = image.width() - pane_left(&image);
        assert!(
            (fill as f32 - full).abs() <= 4.0,
            "a selected row gives a {fill}px pane, not the configured {full}px"
        );
    }

    /// Every scene must render, in both themes, without hanging.
    ///
    /// The dialogs had never been rendered at all, which is the whole reason
    /// [`Scene`] exists. A scene that cannot be driven into its state is a
    /// regression in the state machine, and the cheapest place to notice is a
    /// test that walks every variant rather than a person remembering to open
    /// each dialog by hand.
    ///
    /// Rendered small on purpose: this asks "can the app be *driven* here?", not
    /// "does it look right", and the answer to the second is what a human is for.
    /// At the default size the software rasteriser turns twenty renders into two
    /// minutes of test time, which is how a test gets deleted.
    #[test]
    fn every_scene_renders() {
        let size = vec2(640.0, 480.0);
        for scene in [
            Scene::Browser,
            Scene::ConfirmPermanent,
            Scene::ConfirmTrash,
            Scene::Collision,
            Scene::Progress,
            Scene::Failed,
            Scene::CannotOpen,
            Scene::PreviewEmpty,
            Scene::PreviewText,
            Scene::PreviewImage,
            Scene::PreviewTooLarge,
            Scene::Settings,
            Scene::Help,
            Scene::Empty,
            Scene::Denied,
            Scene::Gone,
            Scene::NoWatch,
        ] {
            for theme in [
                crate::tokens::ThemeMode::Light,
                crate::tokens::ThemeMode::Dark,
            ] {
                let image = render(
                    app::Options {
                        theme,
                        ..app::Options::default()
                    },
                    scene,
                    Some(size),
                )
                .unwrap_or_else(|e| panic!("{scene:?} / {theme:?} did not render: {e}"));
                assert_eq!(
                    (image.width(), image.height()),
                    (640, 480),
                    "{scene:?} rendered at the wrong size"
                );
            }
        }
    }

    /// A dialog renders at full opacity, and only one scrim is painted.
    ///
    /// Two separate bugs, both invisible in a screenshot a person is *looking*
    /// at rather than measuring, and both of which made every colour in every
    /// dialog review wrong:
    ///
    /// * the app painted `ui.max_rect()` as a scrim **and** left `egui::Modal`'s
    ///   own default backdrop in place, so the window was darkened twice (0.62 ×
    ///   0.61 ≈ 0.38 of the original) — and the hand-painted rect was the *list*
    ///   rectangle, so it never covered the toolbar or the status bar anyway;
    /// * a capture drew three frames in microseconds with the clock at zero, and
    ///   `egui::Area` fades what it owns, so the dialog itself rendered at about
    ///   a tenth of its opacity. `surface.raised` in light is white; it measured
    ///   232.
    ///
    /// So: the dialog's own fill must be `surface.raised`, and the surface behind
    /// it must be the spec's scrim over the list — which is darker than the raw
    /// list and lighter than a doubly-scrimmed one.
    #[test]
    fn a_dialog_renders_at_full_opacity_over_one_scrim() {
        let image = render(
            app::Options {
                theme: crate::tokens::ThemeMode::Light,
                ..app::Options::default()
            },
            Scene::ConfirmPermanent,
            Some(BROWSER_SIZE),
        )
        .expect("render");
        let w = image.width();
        let h = image.height();
        let px = |x: usize, y: usize| {
            let p = image.pixels[y * w + x];
            (p.r(), p.g(), p.b())
        };
        // `surface.raised`, light: `#FFFFFF`. The rasteriser's bilinear tap of
        // `WHITE_UV` costs a few levels on a large flat fill, hence the slack.
        let dialog = px(w / 2, h / 2 - 20);
        assert!(
            dialog.0 > 245 && dialog.1 > 245 && dialog.2 > 245,
            "the dialog's own surface is {dialog:?}, not `surface.raised` — the \
             dialog is being drawn at less than full opacity"
        );
        // The scrim over the list: §3.1's `#1C1A17` at 38%, so roughly 0.62 of
        // `surface.list` (`#FCFBF9`) plus a little premultiplied ink.
        let behind = px(w / 2 - 300, h / 2);
        assert!(
            (150..200).contains(&behind.0),
            "the surface behind the dialog is {behind:?}, which is not `surface.scrim` \
             over `surface.list`"
        );
    }

    /// The list must never draw into the preview pane.
    ///
    /// The reported defect was the `Modified` column being clipped at the pane's
    /// left edge. The cause was structural rather than arithmetic: the column set
    /// was chosen from the *panel's* width while the rows were laid out from the
    /// width read **inside** the `ScrollArea`, so the header and the values could
    /// resolve to different columns, and any disagreement shows up as a value
    /// sitting under the wrong header or running under the divider.
    ///
    /// This measures the rendered picture instead of the layout: it finds the
    /// pane's left edge from the pixels, then requires every row band's ink to
    /// stop short of it. It holds at any width, which is the property that
    /// matters — a check at one width would only pin one width.
    #[test]
    fn the_list_never_draws_into_the_preview_pane() {
        for width in [900.0_f32, 1200.0, 1600.0] {
            let image = render(
                app::Options::default(),
                Scene::Browser,
                Some(vec2(width, 800.0)),
            )
            .unwrap_or_else(|e| panic!("render at {width} failed: {e}"));
            let w = image.width();
            let h = image.height();
            let px = |x: usize, y: usize| {
                let p = image.pixels[y * w + x];
                (p.r(), p.g(), p.b())
            };
            // The pane's left edge, from the picture's own fill rather than from
            // the window's edge — see `pane_left`. Walking in from `w - 1` stopped
            // on the pane's own 1px frame stroke, so the old version scanned
            // 20px of *pane* looking for list ink. It could not fail.
            //
            // `Scene::Browser` focuses nothing, so the pane is one flat colour,
            // and since it is now a rail that colour is a narrow band rather than
            // a third of the window. Which is the point: an 80px rail is a
            // sharper test of "the list stops at the pane's edge" than a 280px
            // pane was.
            let pane_left = pane_left(&image);
            // The list's own surface, sampled well inside the list body and on a
            // row, so it is between two rows rather than on one.
            let list_surface = px(400, 110);
            // The list band: below the column header, above the status bar.
            let band = 100..(h - 40);
            let mut worst = 0usize;
            for y in band {
                // The window stops two columns short of `pane_left`: egui draws a
                // hairline frame stroke and a dim separator there, and neither is
                // list ink. A list row that reached *them* would still be a bug,
                // but this test is about the 78 columns beside them.
                for x in pane_left.saturating_sub(80)..pane_left.saturating_sub(2) {
                    if px(x, y) != list_surface {
                        worst = worst.max(x);
                    }
                }
            }
            assert!(
                worst < pane_left,
                "at width {width}: the list paints ink at x={worst}, inside the \
                 preview pane that starts at {pane_left}"
            );
        }
    }

    /// The preview empty state is centred in the part of the pane the user can
    /// see — and when nothing is selected, that part is a rail.
    ///
    /// # Why this grew a second axis
    ///
    /// With nothing selected the pane is [`PanePlan::Strip`], so "centred in the
    /// pane" now has two readings and both are defects worth catching: a block
    /// that is vertically centred in a 776px rail but shoved to one side reads
    /// as a layout bug, and a block that is horizontally centred in a 280px pane
    /// reads fine while the pane is 200px too wide. So the test measures the
    /// block's ink in both directions against the rail's own rect, and the rail
    /// width is asserted to be a rail at all.
    ///
    /// §4.2's "Never blank" is the property being protected and it has not been
    /// weakened: the rail still has to draw something, and the first assertion is
    /// still "the empty state drew nothing in the pane" failing. What changed is
    /// the thing being centred — a 48px glyph rather than a glyph with a title
    /// and a sentence under it — because at 80px wide there is no room for the
    /// other two and §2 has no smaller font to offer.
    ///
    /// The original reason for the test is unchanged and still the reason it
    /// measures the picture: the pane used to be *sized* before the status bar
    /// existed — an egui panel is measured against whatever the root `Ui` has
    /// left when it is shown, and the status bar is a full-width band that had
    /// not been reserved yet — so its rectangle ran 25px under the status bar.
    /// Nothing about the fill showed it, because the status bar and the pane
    /// share `surface.panel`. What it did show was anything centred in the pane
    /// sitting 15px below the middle of the visible area, which is a defect no
    /// unit test on the layout function could have found.
    ///
    /// So this measures the picture: the block's ink, against the band between
    /// the column header and the status bar. The band's edges are the tokens:
    /// toolbar 34 + breadcrumb 28 + column header 24 at the top, status bar 24 at
    /// the bottom.
    #[test]
    fn the_preview_empty_state_is_centred_in_the_pane() {
        // The pane is a full-height side panel, so its visible band runs from
        // under the toolbar to the top of the status bar — *not* under the
        // column header, which only spans the list column. Getting that wrong is
        // how a test can pass a layout that is 25px low.
        let top = crate::tokens::metric::TOOLBAR as usize;
        let bottom_gap = crate::tokens::component::STATUSBAR_HEIGHT as usize;
        for theme in [
            crate::tokens::ThemeMode::Light,
            crate::tokens::ThemeMode::Dark,
        ] {
            let image = render(
                app::Options {
                    theme,
                    ..app::Options::default()
                },
                Scene::PreviewEmpty,
                Some(BROWSER_SIZE),
            )
            .expect("render");
            let w = image.width();
            let h = image.height();
            let px = |x: usize, y: usize| {
                let p = image.pixels[y * w + x];
                (p.r(), p.g(), p.b())
            };
            // The pane's fill, from its own middle.
            let pane_fill = px(w - 20, h / 2);
            // The pane's left edge, from the picture's own fill rather than from
            // the window's edge — see `pane_left`. Walking in from `w - 1` stopped
            // on the pane's own 1px frame stroke, which made the box below 20px
            // wide and the whole measurement a description of the border.
            let pane_left = pane_left(&image);
            // `top + 1`: the toolbar's bottom border is a hairline that spans the
            // whole window, so it lands on the band's first row inside the
            // pane's own x range. Everything below it in the pane belongs to the
            // block, so the bounding box is min..max of the rest.
            let pane_x: Vec<usize> = (pane_left + 4..w - 4).collect();
            let rows: Vec<usize> = (top + 1..h - bottom_gap)
                .filter(|y| pane_x.iter().any(|x| px(*x, *y) != pane_fill))
                .collect();
            assert!(
                !rows.is_empty(),
                "{theme:?}: the empty state drew nothing in the pane"
            );
            let start = rows[0];
            let end = rows[rows.len() - 1];
            let centre = (start + end) as f32 / 2.0;
            let band_centre = (top + h - bottom_gap) as f32 / 2.0;
            assert!(
                (centre - band_centre).abs() < 8.0,
                "{theme:?}: the empty state is centred at {centre} (rows {start}..\
                 {end}) but the visible pane is centred at {band_centre}"
            );
            // The rail, not a pane. `pane_policy` asserts the same width as a
            // plan; only the picture proves the panel honoured it.
            let rail = (w - pane_left) as f32;
            assert!(
                rail <= crate::dialog::preview_metrics::STRIP_WIDTH,
                "{theme:?}: nothing is selected, yet the pane is {rail}px wide"
            );
            // And the glyph is centred *in the rail*, which is the new part.
            // `pane_x` is already the rail's interior, so this is the ink's
            // middle against the rail's.
            let cols: Vec<usize> = pane_x
                .iter()
                .copied()
                .filter(|x| ((top + 1)..(h - bottom_gap)).any(|y| px(*x, y) != pane_fill))
                .collect();
            assert!(
                !cols.is_empty(),
                "{theme:?}: the rail drew rows but no columns — the glyph is a smear"
            );
            let first = cols[0];
            let last = cols[cols.len() - 1];
            let glyph_centre = (first + last) as f32 / 2.0;
            let rail_centre = (pane_left as f32 + w as f32) / 2.0;
            assert!(
                (glyph_centre - rail_centre).abs() < 8.0,
                "{theme:?}: the glyph is centred at {glyph_centre} (columns {first}..\
                 {last}) but the rail is centred at {rail_centre}"
            );
        }
    }

    /// A text preview must actually put text on the pane.
    ///
    /// Without this, `PreviewText` renders happily with a pane that says
    /// "Loading…" forever and the test above passes — which is exactly the bug
    /// this capture path exists to make visible, so it needs an assertion of its
    /// own rather than only a "it did not hang".
    #[test]
    fn a_text_preview_lands_content() {
        let dir = fixture().expect("fixture");
        let ctx = Context::default();
        crate::tokens::fonts::install(&ctx);
        let mut app = KestrelApp::for_capture(
            ctx.clone(),
            app::Options {
                theme: crate::tokens::ThemeMode::Dark,
                start_dir: Some(dir.path().to_path_buf()),
                ..app::Options::default()
            },
        );
        let mut textures = TextureBook::default();
        let mut clock = CAPTURE_TIME_BASE;
        assert!(
            settle_until(
                &ctx,
                &mut app,
                BROWSER_SIZE,
                &mut textures,
                &mut clock,
                |a| a.is_listed(),
            ),
            "the fixture listing never arrived"
        );
        app.apply_scene(Scene::PreviewText);
        assert!(
            settle_until(
                &ctx,
                &mut app,
                BROWSER_SIZE,
                &mut textures,
                &mut clock,
                |a| a.is_settled(),
            ),
            "the preview scene never settled"
        );
        assert!(
            matches!(
                app.preview_state(),
                Some(crate::preview::Loaded::Text { .. })
            ),
            "the preview pane never received its content: {:?}",
            app.preview_state()
        );
    }

    /// A capture at an explicit size is that size, not the default.
    ///
    /// The narrow-pane bug was invisible at 1200px, so the ability to render
    /// narrower is not a convenience: without it the next one cannot be caught.
    #[test]
    fn an_explicit_size_is_honoured() {
        let image = render(
            app::Options::default(),
            Scene::Browser,
            Some(vec2(640.0, 480.0)),
        )
        .expect("render");
        assert_eq!((image.width(), image.height()), (640, 480));
    }

    /// `--light` and `--dark` must produce **different pictures**.
    ///
    /// The reported failure was "`--light` produces a dark screenshot", and the
    /// evidence offered was that two files had the same size. That turned out to
    /// be a coincidence — the two files differ in 2.8 million bytes — which is
    /// exactly the kind of evidence that is easy to check and easy to get wrong,
    /// so it is now checked properly: render both in memory and compare pixels.
    ///
    /// Two assertions, because either alone is too weak:
    ///
    /// * the images are not equal — which catches "the flag is ignored", and
    /// * a light render really is *lighter* than a dark one at the list body —
    ///   which catches "both flags work but both resolve to the same theme",
    ///   which equality alone would miss.
    #[test]
    fn light_and_dark_renders_actually_differ() {
        let dir = std::env::temp_dir().join("kestrel-shot-theme-test");
        let _ = std::fs::create_dir_all(&dir);
        let base = app::Options {
            gallery: false,
            start_dir: Some(dir.clone()),
            ..app::Options::default()
        };
        let dark = render(
            app::Options {
                theme: crate::tokens::ThemeMode::Dark,
                ..base.clone()
            },
            Scene::Browser,
            None,
        )
        .expect("dark render");
        let light = render(
            app::Options {
                theme: crate::tokens::ThemeMode::Light,
                ..base
            },
            Scene::Browser,
            None,
        )
        .expect("light render");
        assert_eq!(
            (light.width(), light.height()),
            (dark.width(), dark.height()),
            "the two renders must be the same size, or the comparison is meaningless"
        );
        assert_ne!(
            light.pixels, dark.pixels,
            "`--light` and `--dark` produced identical pixels: one of the two \
             theme flags is being ignored"
        );

        // Sampled mid-height, in the list body, where `surface.list` shows through
        // between rows: #FCFBF9 light, #1E1C1A dark.
        let x = (light.width() / 2) as usize;
        let y = ((light.height() as f32) * 0.5) as usize;
        let lw = light.width() as usize;
        let dw = dark.width() as usize;
        let l = light.pixels[(y * lw) + x];
        let d = dark.pixels[(y * dw) + x];
        // `Color32` components are already `u8`; summing them is the luminance
        // proxy, good enough to order two themes.
        let lum = |p: Color32| f32::from(p.r()) + f32::from(p.g()) + f32::from(p.b());
        assert!(
            lum(l) > lum(d),
            "the light render is not lighter than the dark one at the list body: \
             light {l:?} dark {d:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    use egui::epaint::Color32;

    #[test]
    fn crc32_matches_the_known_check_value() {
        // The standard "123456789" check value. 0xCBF43926 is the *reflected*
        // CRC-32 that PNG and zlib both use (poly 0xEDB88320 reflected, init
        // 0xFFFFFFFF, final XOR). 0xC76C4163 — the value that looks right if you
        // are thinking of CRC-32/MPEG-2 — is the non-reflected variant and would
        // make every chunk in every capture unreadable to a strict decoder.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    /// The fixture's PNG has to be a real PNG, or every image capture in the
    /// suite is really testing the decoder's failure path.
    #[test]
    fn the_fixture_image_is_a_decodable_png() {
        let bytes = sample_png();
        let kinds = read_chunks(&bytes);
        assert_eq!(
            kinds,
            vec![*b"IHDR", *b"IDAT", *b"IEND"],
            "the fixture must be a complete PNG, CRC and all"
        );
        let image =
            egui_extras::image::load_image_bytes(&bytes).expect("the fixture PNG must decode");
        assert_eq!((image.width(), image.height()), (320, 240));
    }

    #[test]
    fn adler32_matches_the_known_check_value() {
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn a_stored_zlib_stream_starts_and_ends_correctly() {
        let z = zlib_stored(b"hello");
        assert_eq!(&z[..2], &[0x78, 0x01]);
        // final block, stored type
        assert_eq!(z[2] & 0b0000_0111, 0b0000_0001);
        assert_eq!(u16::from_le_bytes([z[3], z[4]]), 5);
        assert_eq!(u16::from_le_bytes([z[5], z[6]]), !5u16);
        assert_eq!(&z[7..12], b"hello");
    }

    #[test]
    fn an_empty_input_still_produces_a_valid_stream() {
        // A PNG with zero rows would be invalid, but the helper must not panic.
        let z = zlib_stored(&[]);
        assert_eq!(&z[..2], &[0x78, 0x01]);
    }

    /// Walks the PNG chunk stream, returning each chunk's type and verifying
    /// its CRC as it goes — which is what a strict decoder does, and what makes
    /// this test able to catch a wrong CRC rather than only a missing chunk.
    fn read_chunks(bytes: &[u8]) -> Vec<[u8; 4]> {
        assert_eq!(
            &bytes[..8],
            &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
            "PNG signature"
        );
        let mut out = Vec::new();
        let mut i = 8usize;
        while i + 8 <= bytes.len() {
            let len =
                u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]) as usize;
            let mut kind = [0u8; 4];
            kind.copy_from_slice(&bytes[i + 4..i + 8]);
            let data = &bytes[i + 8..i + 8 + len];
            let stored = u32::from_be_bytes([
                bytes[i + 8 + len],
                bytes[i + 9 + len],
                bytes[i + 10 + len],
                bytes[i + 11 + len],
            ]);
            let mut crc_input = Vec::with_capacity(4 + len);
            crc_input.extend_from_slice(&kind);
            crc_input.extend_from_slice(data);
            assert_eq!(
                crc32(&crc_input),
                stored,
                "CRC mismatch in chunk {}",
                String::from_utf8_lossy(&kind)
            );
            out.push(kind);
            i += 12 + len;
            if &kind == b"IEND" {
                break;
            }
        }
        out
    }

    #[test]
    fn a_written_png_has_a_valid_chunk_stream() {
        let fill = Color32::from_rgb(0xFC, 0xFB, 0xF9);
        let image = ColorImage::new([2, 2], vec![fill; 4]);
        let path = std::env::temp_dir().join("kestrel-shot-png-test.png");
        save_png(&image, &path).expect("save");

        let bytes = std::fs::read(&path).expect("read back");
        let kinds = read_chunks(&bytes);
        assert_eq!(
            kinds,
            vec![*b"IHDR", *b"IDAT", *b"IEND"],
            "a minimal PNG is exactly IHDR, IDAT, IEND"
        );

        // IHDR: 2x2, 8-bit, colour type 6 (RGBA).
        let ihdr = &bytes[16..16 + 13];
        assert_eq!(&ihdr[..4], &2u32.to_be_bytes());
        assert_eq!(&ihdr[4..8], &2u32.to_be_bytes());
        assert_eq!(ihdr[8], 8, "bit depth");
        assert_eq!(ihdr[9], 6, "colour type RGBA");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn edge_gives_a_consistent_sign_for_winding() {
        let a = Pos2::new(0.0, 0.0);
        let b = Pos2::new(1.0, 0.0);
        let c = Pos2::new(0.0, 1.0);
        // Clockwise and counter-clockwise must both be non-degenerate, only the
        // sign differs.
        assert!(edge(a, b, c) > 0.0);
        assert!(edge(c, b, a) < 0.0);
        // A point on the edge evaluates to ~0, which is what the barycentric
        // inside test relies on.
        assert!(edge(a, b, Pos2::new(0.5, 0.0)).abs() < 1e-6);
    }
}
