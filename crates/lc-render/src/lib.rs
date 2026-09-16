//! # lc-render
//!
//! Turning a [`Symbol`] into something a camera can see: an RGB raster, a PNG, or an
//! SVG.
//!
//! ## The canvas, and where the reference border lives
//!
//! §6.4 puts the optional reference border **outside** the quiet zone, which means the
//! rendered canvas is larger than `side + 2·quiet` whenever it is present, and the border
//! is not part of the module grid. Shells, counted outward from the symbol edge:
//!
//! ```text
//! d = 0        symbol
//! d = 1, 2     quiet zone (§6.1)
//! d = 3, 4     reference border (§6.4, when signalled by format bit 14)
//! d = 5, 6     outer quiet zone
//! ```
//!
//! The outer quiet zone is not in the spec but is not optional in practice: without it,
//! the border's dark cells run straight into whatever the symbol is displayed on, and
//! the finder detector's run-ratio scan has no clean termination.
//!
//! ## Pinned: the border colour sequence
//!
//! §6.4 says the border cycles "through the full palette plus white and black" without
//! fixing the order or the indexing. Pinned here as: the cycle is the palette in index
//! order followed by white then black, and a border cell's position in the cycle is its
//! coordinate **along** the edge it sits on — `x` on the top and bottom runs, `y` on the
//! left and right, corners resolved to whichever axis is further out. Both radial layers
//! of the border take the same colour, so every entry presents a 2-module-deep target to
//! the sampler rather than a 1-module sliver.
//!
//! ## §13.1 conformance
//!
//! [`RenderOptions::screen`] defaults to 10 device pixels per module and a pure white
//! quiet zone, both of which §13.1 makes normative for a conforming display application.
//! `module_px` is an integer everywhere in this crate — there is no code path that can
//! produce a fractional module size, because §13.1 point 3 says non-integer module sizes
//! alias badly against the capturing sensor.

#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use lc_core::border;
use lc_core::color::Srgb8;
use lc_core::layout::Layout;
use lc_core::palette::{BORDER_WHITE, Palette};
use lc_core::{Profile, Symbol};

/// §17.2 dual-render: an LC symbol beside a QR carrying a fallback URL.
///
/// Off by default — see the `qr` feature in `Cargo.toml`, and the module docs on why
/// §17.2 calls this "a bridge, not an answer".
#[cfg(feature = "qr")]
pub mod qr;

/// Quiet-zone width in modules (§6.1).
pub const QUIET: u16 = border::QUIET as u16;
/// Modules of white between the grid and the ring (§6.4, D18).
pub const GAP: u16 = border::GAP as u16;
/// Reference-border width in modules (§6.4).
pub const BORDER: u16 = border::WIDTH as u16;

/// How to rasterise.
#[derive(Clone, Debug)]
pub struct RenderOptions {
    /// Device pixels per module. §13.1 requires ≥ 8 and prefers 10–12.
    pub module_px: u32,
    /// Colour of the quiet zone. §13.1 point 5 requires pure white on screen.
    pub quiet_color: Srgb8,
}

impl Default for RenderOptions {
    fn default() -> Self {
        RenderOptions::screen()
    }
}

impl RenderOptions {
    /// §13.1-conforming screen defaults: 10 device pixels per module, white quiet zone.
    pub fn screen() -> RenderOptions {
        RenderOptions {
            module_px: 10,
            quiet_color: BORDER_WHITE,
        }
    }

    /// Print defaults. 8 px/module at 600 dpi is ≈ 0.34 mm, near §13.3's floor.
    pub fn print() -> RenderOptions {
        RenderOptions {
            module_px: 8,
            quiet_color: BORDER_WHITE,
        }
    }

    /// Set the module pitch, clamped to the §13.1 floor of 8 device pixels.
    pub fn with_module_px(mut self, px: u32) -> RenderOptions {
        self.module_px = px.max(8);
        self
    }
}

/// The rendered canvas geometry, in modules.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Canvas {
    /// Symbol side.
    pub side: u16,
    /// Modules between the canvas edge and the symbol on every side.
    pub margin: u16,
    /// Whether the §6.4 border is drawn.
    pub border: bool,
    /// What the ring's colour choices carry: nothing, or the format codeword of a
    /// finder-less symbol (D17).
    pub ring: border::RingMode,
}

impl Canvas {
    /// Geometry for a symbol.
    pub fn of(symbol: &Symbol) -> Canvas {
        let border = symbol.format.reference_border;
        let ring = if Layout::is_finderless(symbol.format.profile, border) {
            border::RingMode::Data(symbol.format.to_codeword())
        } else {
            border::RingMode::Plain
        };
        Canvas {
            side: symbol.side(),
            margin: if border { QUIET + BORDER + GAP } else { QUIET },
            border,
            ring,
        }
    }

    /// Canvas side in modules.
    #[inline]
    pub const fn modules(&self) -> u16 {
        self.side + 2 * self.margin
    }

    /// Canvas side in device pixels at a given module pitch.
    #[inline]
    pub const fn pixels(&self, module_px: u32) -> u32 {
        self.modules() as u32 * module_px
    }

    /// How far outside the symbol a canvas cell sits, in modules. `0` is inside.
    #[inline]
    fn shell(&self, cx: u16, cy: u16) -> u16 {
        border::shell(
            cx as i32 - self.margin as i32,
            cy as i32 - self.margin as i32,
            self.side,
        ) as u16
    }
}

/// The §6.4 border cycle: the palette in index order, then white, then black.
pub fn border_cycle(palette: &Palette) -> Vec<Srgb8> {
    (0..border::cycle_len(palette.len()))
        .map(|i| border::cycle_color(palette, i))
        .collect()
}

/// Colour of a border cell in *canvas* coordinates, or `None` if it is not on the border.
///
/// A thin adapter over [`lc_core::border::cell_color`], which works in module space. The
/// decoder addresses the same cells directly in module space, through the homography —
/// keeping one definition of the rule is what stops renderer and decoder from drifting
/// apart over what the border says.
pub fn border_color(canvas: &Canvas, palette: &Palette, cx: u16, cy: u16) -> Option<Srgb8> {
    if !canvas.border {
        return None;
    }
    border::cell_color_in(
        cx as i32 - canvas.margin as i32,
        cy as i32 - canvas.margin as i32,
        canvas.side,
        palette,
        canvas.ring,
    )
}

/// Colour of any canvas cell: symbol module, border, or quiet zone.
pub fn canvas_color(
    symbol: &Symbol,
    canvas: &Canvas,
    palette: &Palette,
    opts: &RenderOptions,
    cx: u16,
    cy: u16,
) -> Srgb8 {
    if canvas.shell(cx, cy) == 0 {
        return symbol.color_at(palette, cx - canvas.margin, cy - canvas.margin);
    }
    // A ring payload cell (D19) takes its colour from the symbol, not from the ring's own
    // rule: it is an ordinary payload module that happens to live in the border.
    if !symbol.ring.is_empty() {
        let (x, y) = (
            cx as i32 - canvas.margin as i32,
            cy as i32 - canvas.margin as i32,
        );
        if let Some(i) = border::payload_cell_index(x, y, canvas.side)
            && let Some(&idx) = symbol.ring.get(i)
        {
            return palette.colors[idx as usize];
        }
    }
    border_color(canvas, palette, cx, cy).unwrap_or(opts.quiet_color)
}

/// An RGB8 raster.
#[derive(Clone, Debug)]
pub struct Raster {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Row-major `RGB` triples, `width · height · 3` bytes.
    pub data: Vec<u8>,
}

impl Raster {
    /// Read a pixel.
    pub fn pixel(&self, x: u32, y: u32) -> Srgb8 {
        let i = ((y * self.width + x) * 3) as usize;
        Srgb8::new(self.data[i], self.data[i + 1], self.data[i + 2])
    }

    /// Convert to RGBA8, as a browser canvas expects.
    pub fn to_rgba(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.data.len() / 3 * 4);
        for px in self.data.as_chunks::<3>().0 {
            out.extend_from_slice(px);
            out.push(0xFF);
        }
        out
    }
}

/// Rasterise a symbol to RGB8.
pub fn render_rgb(symbol: &Symbol, opts: &RenderOptions) -> Raster {
    let canvas = Canvas::of(symbol);
    let palette = symbol.palette();
    let px = opts.module_px.max(1);
    let side_px = canvas.pixels(px);
    let mut data = vec![0u8; (side_px * side_px * 3) as usize];

    for cy in 0..canvas.modules() {
        // One module row is identical across its `px` pixel rows; build it once.
        let mut row = Vec::with_capacity((side_px * 3) as usize);
        for cx in 0..canvas.modules() {
            let c = canvas_color(symbol, &canvas, &palette, opts, cx, cy);
            for _ in 0..px {
                row.extend_from_slice(&[c.r, c.g, c.b]);
            }
        }
        for sub in 0..px {
            let y = cy as u32 * px + sub;
            let off = (y * side_px * 3) as usize;
            data[off..off + row.len()].copy_from_slice(&row);
        }
    }
    Raster {
        width: side_px,
        height: side_px,
        data,
    }
}

/// Render a symbol as a standalone SVG document.
///
/// Runs of identical colour within a module row are merged into one `<rect>`, which
/// typically cuts the file size by 3–5× on real symbols and costs nothing to do.
pub fn render_svg(symbol: &Symbol, opts: &RenderOptions) -> String {
    let canvas = Canvas::of(symbol);
    let palette = symbol.palette();
    let px = opts.module_px.max(1);
    let n = canvas.modules();
    let size = n as u32 * px;

    let mut s = String::with_capacity(4096);
    s.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{size}\" height=\"{size}\" \
         viewBox=\"0 0 {n} {n}\" shape-rendering=\"crispEdges\">\n"
    ));
    s.push_str(&format!(
        "<rect width=\"{n}\" height=\"{n}\" fill=\"#{:06X}\"/>\n",
        opts.quiet_color.to_hex()
    ));

    for cy in 0..n {
        let mut run_start = 0u16;
        let mut run_color = canvas_color(symbol, &canvas, &palette, opts, 0, cy);
        for cx in 1..=n {
            let c = if cx < n {
                Some(canvas_color(symbol, &canvas, &palette, opts, cx, cy))
            } else {
                None
            };
            if c != Some(run_color) {
                if run_color != opts.quiet_color {
                    s.push_str(&format!(
                        "<rect x=\"{run_start}\" y=\"{cy}\" width=\"{}\" height=\"1\" fill=\"#{:06X}\"/>\n",
                        cx - run_start,
                        run_color.to_hex()
                    ));
                }
                run_start = cx;
                if let Some(c) = c {
                    run_color = c;
                }
            }
        }
    }
    s.push_str("</svg>\n");
    s
}

/// Encode a raster as a PNG.
///
/// A minimal encoder rather than a dependency: `image` is explicitly gated out of the
/// wasm path by §14.2, and the alternative is a C-backed PNG library that would violate
/// the same rule.
#[cfg(feature = "png")]
pub fn render_png(symbol: &Symbol, opts: &RenderOptions) -> Vec<u8> {
    png_encode(&render_rgb(symbol, opts))
}

/// Encode an existing raster as a PNG.
#[cfg(feature = "png")]
pub fn png_encode(raster: &Raster) -> Vec<u8> {
    use lc_core::crc::crc32_iso_hdlc;

    // Filter type 0 (None) on every scanline.
    let mut raw = Vec::with_capacity(raster.data.len() + raster.height as usize);
    for row in raster.data.chunks_exact((raster.width * 3) as usize) {
        raw.push(0u8);
        raw.extend_from_slice(row);
    }
    let idat = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 8);

    let mut png = Vec::with_capacity(idat.len() + 128);
    png.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);

    let chunk = |kind: &[u8; 4], body: &[u8], out: &mut Vec<u8>| {
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        let mut crc_input = Vec::with_capacity(4 + body.len());
        crc_input.extend_from_slice(kind);
        crc_input.extend_from_slice(body);
        out.extend_from_slice(&crc32_iso_hdlc(&crc_input).to_be_bytes());
    };

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&raster.width.to_be_bytes());
    ihdr.extend_from_slice(&raster.height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit, truecolour RGB
    chunk(b"IHDR", &ihdr, &mut png);
    chunk(b"IDAT", &idat, &mut png);
    chunk(b"IEND", &[], &mut png);
    png
}

/// Render the symbol as ANSI-coloured text, two modules per character cell.
///
/// Useful in the CLI and, more to the point, in test failure output: a broken layout is
/// obvious at a glance and invisible in a hex dump.
pub fn render_ansi(symbol: &Symbol) -> String {
    let canvas = Canvas::of(symbol);
    let palette = symbol.palette();
    let opts = RenderOptions::screen();
    let n = canvas.modules();
    let mut s = String::new();
    for cy in (0..n).step_by(2) {
        for cx in 0..n {
            let top = canvas_color(symbol, &canvas, &palette, &opts, cx, cy);
            let bot = if cy + 1 < n {
                canvas_color(symbol, &canvas, &palette, &opts, cx, cy + 1)
            } else {
                opts.quiet_color
            };
            s.push_str(&format!(
                "\x1b[38;2;{};{};{}m\x1b[48;2;{};{};{}m▀",
                top.r, top.g, top.b, bot.r, bot.g, bot.b
            ));
        }
        s.push_str("\x1b[0m\n");
    }
    s
}

/// Convenience: profile-appropriate render options.
pub fn options_for(profile: Profile) -> RenderOptions {
    match profile {
        Profile::C2 | Profile::M => RenderOptions::print(),
        _ => RenderOptions::screen(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lc_core::{EccLevel, Version, layout::Layout, modulate::modulate};

    fn sample(profile: Profile, border: bool) -> Symbol {
        let format = lc_core::FormatInfo {
            version: Version::new(4).unwrap(),
            profile,
            ecc: EccLevel::DEFAULT,
            mask: 2,
            reference_border: border,
        };
        // The layout the format actually implies (D17) — a bordered colour symbol is
        // finder-less, and a fixture that disagreed would build an impossible symbol.
        let layout = Layout::for_symbol(format.version, profile, border);
        let cap = layout.capacity();
        let bits: Vec<bool> = (0..cap.luma_bits).map(|i| i % 3 == 0).collect();
        let ch: Vec<bool> = (0..cap.chroma_bits).map(|i| i % 5 < 2).collect();
        modulate(&layout, format, &bits, &ch)
    }

    #[test]
    fn canvas_geometry_matches_the_shell_table() {
        let plain = Canvas::of(&sample(Profile::C1, false));
        assert_eq!(plain.margin, 2);
        assert_eq!(plain.modules(), 33 + 4);

        let bordered = Canvas::of(&sample(Profile::C1, true));
        assert_eq!(bordered.margin, 5);
        assert_eq!(bordered.modules(), 33 + 2 * (QUIET + BORDER + GAP));
    }

    #[test]
    fn the_border_only_occupies_its_own_shells() {
        let sym = sample(Profile::C1, true);
        let canvas = Canvas::of(&sym);
        let palette = sym.palette();
        for cy in 0..canvas.modules() {
            for cx in 0..canvas.modules() {
                let d = canvas.shell(cx, cy);
                let has = border_color(&canvas, &palette, cx, cy).is_some();
                let ring = (GAP + 1)..=(GAP + BORDER);
                assert_eq!(has, ring.contains(&d), "({cx},{cy}) shell {d}");
            }
        }
    }

    #[test]
    fn the_border_shows_every_reference_colour() {
        let sym = sample(Profile::C3, true);
        let canvas = Canvas::of(&sym);
        let palette = sym.palette();
        let mut seen = alloc::collections::BTreeSet::new();
        for cy in 0..canvas.modules() {
            for cx in 0..canvas.modules() {
                if let Some(c) = border_color(&canvas, &palette, cx, cy) {
                    seen.insert(c.to_hex());
                }
            }
        }
        for c in border_cycle(&palette) {
            assert!(seen.contains(&c.to_hex()), "border omits {c:?}");
        }
    }

    #[test]
    fn both_border_layers_share_a_colour() {
        let sym = sample(Profile::C1, true);
        let canvas = Canvas::of(&sym);
        let palette = sym.palette();
        // Walk the top edge; the ring's two shells at the same x must match.
        let m = canvas.margin;
        for cx in m..(m + canvas.side) {
            let a = border_color(&canvas, &palette, cx, m - (GAP + 1)).unwrap();
            let b = border_color(&canvas, &palette, cx, m - (GAP + BORDER)).unwrap();
            assert_eq!(a, b, "column {cx}");
        }
    }

    #[test]
    fn raster_is_square_and_module_aligned() {
        for border in [false, true] {
            let sym = sample(Profile::C1, border);
            let opts = RenderOptions::screen();
            let r = render_rgb(&sym, &opts);
            let canvas = Canvas::of(&sym);
            assert_eq!(r.width, canvas.pixels(opts.module_px));
            assert_eq!(r.width, r.height);
            assert_eq!(r.data.len(), (r.width * r.height * 3) as usize);

            // Every pixel of a module must be identical — §13.1 point 3.
            let px = opts.module_px;
            for cy in 0..canvas.modules() {
                for cx in 0..canvas.modules() {
                    let want = r.pixel(cx as u32 * px, cy as u32 * px);
                    for dy in 0..px {
                        for dx in 0..px {
                            assert_eq!(r.pixel(cx as u32 * px + dx, cy as u32 * px + dy), want);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn quiet_zone_is_pure_white() {
        let sym = sample(Profile::C1, false);
        let r = render_rgb(&sym, &RenderOptions::screen());
        assert_eq!(r.pixel(0, 0), BORDER_WHITE);
        assert_eq!(r.pixel(r.width - 1, r.height - 1), BORDER_WHITE);
    }

    #[test]
    fn module_pitch_respects_the_spec_floor() {
        assert_eq!(RenderOptions::screen().with_module_px(3).module_px, 8);
        assert_eq!(RenderOptions::screen().with_module_px(12).module_px, 12);
        assert!(RenderOptions::screen().module_px >= 8);
        assert!(RenderOptions::print().module_px >= 8);
    }

    #[test]
    fn svg_is_well_formed_and_covers_the_canvas() {
        let sym = sample(Profile::C1, true);
        let svg = render_svg(&sym, &RenderOptions::screen());
        assert!(svg.starts_with("<svg"));
        assert!(svg.trim_end().ends_with("</svg>"));
        assert_eq!(svg.matches("<rect").count(), svg.matches("/>").count());
        let n = Canvas::of(&sym).modules();
        assert!(svg.contains(&format!("viewBox=\"0 0 {n} {n}\"")));
    }

    #[test]
    fn svg_merges_runs() {
        let sym = sample(Profile::M, false);
        let svg = render_svg(&sym, &RenderOptions::screen());
        let rects = svg.matches("<rect").count();
        let cells = Canvas::of(&sym).modules() as usize;
        assert!(
            rects < cells * cells / 2,
            "{rects} rects for {} cells",
            cells * cells
        );
    }

    #[cfg(feature = "png")]
    #[test]
    fn png_has_a_valid_signature_and_chunks() {
        let sym = sample(Profile::C1, false);
        let png = render_png(&sym, &RenderOptions::screen());
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        assert_eq!(&png[12..16], b"IHDR");
        assert!(png.windows(4).any(|w| w == b"IDAT"));
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");

        // The decompressed IDAT must be exactly one filter byte plus RGB per row.
        let raster = render_rgb(&sym, &RenderOptions::screen());
        let start = png.windows(4).position(|w| w == b"IDAT").unwrap() + 4;
        let len = u32::from_be_bytes(png[start - 8..start - 4].try_into().unwrap()) as usize;
        let raw = miniz_oxide::inflate::decompress_to_vec_zlib(&png[start..start + len]).unwrap();
        assert_eq!(
            raw.len(),
            raster.height as usize * (1 + raster.width as usize * 3)
        );
    }

    #[test]
    fn rgba_conversion_is_opaque() {
        let sym = sample(Profile::C1, false);
        let r = render_rgb(
            &sym,
            &RenderOptions {
                module_px: 8,
                ..Default::default()
            },
        );
        let rgba = r.to_rgba();
        assert_eq!(rgba.len(), r.data.len() / 3 * 4);
        assert!(rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 0xFF));
    }

    #[test]
    fn ansi_render_has_one_line_per_two_module_rows() {
        let sym = sample(Profile::C1, false);
        let n = Canvas::of(&sym).modules() as usize;
        assert_eq!(render_ansi(&sym).lines().count(), n.div_ceil(2));
    }
}
