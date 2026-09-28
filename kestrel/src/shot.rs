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
use std::path::Path;

use egui::epaint::{ClippedPrimitive, ImageData, Primitive, TextureId, Vertex};
use egui::{Color32, ColorImage, Context, Pos2, Rect, Vec2, vec2};

use crate::app::{KestrelApp, Options};

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

/// Renders the app and writes a PNG to `path`.
///
/// # Errors
///
/// Returns the I/O error from writing `path`. Rendering itself cannot fail: it
/// allocates an in-memory framebuffer and touches no filesystem, GPU, or display
/// server.
pub fn capture(opts: Options, path: &Path) -> std::io::Result<()> {
    let image = render(opts);
    save_png(&image, path)
}

/// Renders the app offscreen and returns the framebuffer.
///
/// Split out from [`capture`] so a test can compare two renders *in memory*,
/// without writing files and without shelling out to the binary. That matters
/// for the `--light`/`--dark` regression: the failure mode being guarded against
/// is "the two flags produce the same picture", and the cheapest honest check is
/// to render both and compare the pixels.
#[must_use]
pub fn render(opts: Options) -> ColorImage {
    let size = if opts.gallery {
        GALLERY_SIZE
    } else {
        BROWSER_SIZE
    };
    let ctx = Context::default();

    // The font set is installed before the first pass, exactly as the real
    // `AppCreator` does it, or the capture would be typeset in egui's default
    // families rather than the ones `fonts::install` selects.
    crate::tokens::fonts::install(&ctx);
    egui_extras::install_image_loaders(&ctx);

    let mut app = KestrelApp::for_capture(ctx.clone(), opts);
    let mut textures = TextureBook::default();
    let mut image = blank(size, 1.0);

    for pass in 0..PASSES {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            // No `pixels_per_point` override: egui 0.36's `RawInput` has no such
            // field — the scale comes from `Context::pixels_per_point`, which
            // defaults to 1.0 for a `Context::default()`. That is exactly what
            // this wants: a capture at 1.0 is the size the tokens are written
            // in, so a 26px row is 26 pixels and the density decisions can be
            // checked directly.
            ..Default::default()
        };
        let mut output = ctx.run_ui(raw, |ui| app.draw(ui));

        textures.absorb(&output.textures_delta);
        let shapes = std::mem::take(&mut output.shapes);
        let ppp = output.pixels_per_point;
        // The platform output is dropped on the floor: there is no window to act
        // on it. The texture deltas are *not* dropped — `absorb` above has
        // already taken what the rasteriser needs.
        output.drop_without_applying_deltas();

        // Every pass is rasterized and the last one kept. The earlier passes are
        // wasted work, but they cost a few hundred microseconds and they make the
        // loop obviously correct: what ends up in the file is exactly what the
        // final pass painted. (A `--preview` / `--collision` capture uses the
        // extra passes to settle the debounce and the modal, so they are not
        // free of purpose.)
        let _ = pass;
        image = rasterize(&ctx, shapes, ppp, size, &textures);
    }

    image
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
    /// A delta with a `pos` is a *partial* update. The font atlas grows by
    /// rewriting its whole image, and nothing in this app uses a moving
    /// texture, so a partial delta is skipped rather than mis-composited —
    /// recorded here because silently dropping one would be a rendering bug
    /// that only shows up as missing glyphs.
    fn absorb(&mut self, delta: &egui::TexturesDelta) {
        for (id, deltas) in &delta.set {
            for image_delta in deltas {
                if image_delta.pos.is_none() {
                    self.images.insert(*id, image_delta.image.clone());
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
fn save_png(image: &ColorImage, path: &Path) -> std::io::Result<()> {
    let [w, h] = image.size;
    let mut raw = Vec::with_capacity(h * (1 + w * 4));
    for y in 0..h {
        // Filter type 0 (None) per scanline, which is what the zlib stream
        // expects before each row of pixels.
        raw.push(0u8);
        for x in 0..w {
            let px = image.pixels[y * w + x];
            raw.extend_from_slice(&px.to_array());
        }
    }

    let mut png = Vec::new();
    png.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);

    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit, RGBA, deflate, no filter, no interlace
    chunk(&mut png, b"IHDR", &ihdr);

    chunk(&mut png, b"IDAT", &zlib_stored(&raw));
    chunk(&mut png, b"IEND", &[]);

    std::fs::write(path, png)
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
        let dark = render(app::Options {
            theme: crate::tokens::ThemeMode::Dark,
            ..base.clone()
        });
        let light = render(app::Options {
            theme: crate::tokens::ThemeMode::Light,
            ..base
        });
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
