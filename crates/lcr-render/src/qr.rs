//! §17.2 dual-render: an LCR symbol beside a QR carrying a fallback URL.
//!
//! Behind the `qr` feature, which is **off by default** — see the feature comment in
//! `Cargo.toml`.
//!
//! ## What this is for, and what §17.2 thinks of it
//!
//! §17 is blunt that adoption, not density, is the binding constraint: every phone decodes
//! QR natively and nothing decodes LCR Code. Dual-render is the second of its three
//! responses — put a standard QR alongside carrying a fallback URL, so a stranger with no
//! app still gets *something*.
//!
//! §17.2 is equally blunt that this "reintroduces the network dependency the format exists
//! to remove — so it is a bridge, not an answer." That judgement is worth keeping in view:
//! a deployment reaching for this should know it is trading the offline property for
//! reach, and it should be a deliberate trade rather than a default. Hence opt-in.
//!
//! ## Generation is a library
//!
//! QR comes from [`qrcodegen`], not from code here. LCR Code has no business implementing a
//! second symbology, and a hand-rolled encoder's masking, interleaving and format info are
//! not provable without a decoder — tests read the output back with `rqrr`, an independent
//! implementation, so a rendered dual is verified rather than asserted.

// `alloc`, not `std`: this crate is `no_std + alloc` when `std` is off, and a dependent
// taking it with `default-features = false` — which `lcr-wasm` does — must still build.
use alloc::vec::Vec;

use crate::{Raster, RenderOptions, render_rgb};
use lcr_core::Symbol;
use lcr_core::color::Srgb8;
use qrcodegen::{QrCode, QrCodeEcc};

/// White, matching the quiet zone §13.1 point 5 requires.
const WHITE: Srgb8 = Srgb8 {
    r: 255,
    g: 255,
    b: 255,
};
const BLACK: Srgb8 = Srgb8 { r: 0, g: 0, b: 0 };

/// Quiet zone QR requires on every side, in modules (ISO/IEC 18004 §6.3.8).
///
/// Not negotiable the way LCR Code's §6.1 two is: a QR decoder in the wild genuinely fails
/// without it, and the whole point of a dual-render is that a stranger's stock camera app
/// reads the QR half.
pub const QR_QUIET: u32 = 4;

/// QR error-correction level, named as ISO/IEC 18004 names them.
///
/// Deliberately not mapped onto LCR Code's 0–3 scale: the two do not correspond.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QrEcc {
    /// ~7% of codewords recoverable.
    Low,
    /// ~15%. The default here — a fallback URL is short, so the extra redundancy is nearly
    /// free in symbol size and the QR half is the one being read in bad conditions by
    /// someone who does not know what they are looking at.
    Medium,
    /// ~25%.
    Quartile,
    /// ~30%.
    High,
}

impl QrEcc {
    fn to_qrcodegen(self) -> QrCodeEcc {
        match self {
            QrEcc::Low => QrCodeEcc::Low,
            QrEcc::Medium => QrCodeEcc::Medium,
            QrEcc::Quartile => QrCodeEcc::Quartile,
            QrEcc::High => QrCodeEcc::High,
        }
    }
}

/// A generated QR symbol as a module grid.
#[derive(Debug, Clone)]
pub struct QrSymbol {
    /// Row-major, one byte per module, 1 = dark.
    pub modules: Vec<u8>,
    /// Grid side in modules, excluding the quiet zone.
    pub size: u32,
    /// QR version, 1..=40.
    pub version: u8,
    /// Error-correction level it was generated at.
    pub ecl: QrEcc,
}

impl QrSymbol {
    /// Dark at `(x, y)`, or `false` outside the grid.
    pub fn get(&self, x: u32, y: u32) -> bool {
        if x >= self.size || y >= self.size {
            return false;
        }
        self.modules[(y * self.size + x) as usize] == 1
    }

    /// Side including the required quiet zone, in modules.
    pub fn outer(&self) -> u32 {
        self.size + 2 * QR_QUIET
    }
}

/// The payload does not fit any QR version at the requested level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QrTooLong {
    /// Size of the payload that did not fit.
    pub bytes: usize,
}

impl core::fmt::Display for QrTooLong {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{} bytes exceeds the largest QR version at this ECC level",
            self.bytes
        )
    }
}

/// Encode text — a URL, in the dual-render case.
///
/// Mode is chosen by `qrcodegen` for the smallest symbol, which for a URL usually means
/// byte mode but may mean alphanumeric. That is the right choice *here*: this is a real
/// fallback meant to be as small as possible, not a controlled comparison.
pub fn qr_encode_text(text: &str, ecl: QrEcc) -> Result<QrSymbol, QrTooLong> {
    let qr = QrCode::encode_text(text, ecl.to_qrcodegen())
        .map_err(|_| QrTooLong { bytes: text.len() })?;
    Ok(collect(&qr, ecl))
}

/// Encode raw bytes in QR byte mode.
///
/// Byte mode unconditionally, which is the fair choice when comparing carriers: LCR Code
/// carries arbitrary bytes natively and §9.6 refuses a text-safe encoding layer precisely
/// because QR's alphanumeric restriction forces one on comparable systems. Handing QR an
/// alphanumeric payload would measure a restriction LCR Code does not have.
pub fn qr_encode_bytes(data: &[u8], ecl: QrEcc) -> Result<QrSymbol, QrTooLong> {
    let qr = QrCode::encode_binary(data, ecl.to_qrcodegen())
        .map_err(|_| QrTooLong { bytes: data.len() })?;
    Ok(collect(&qr, ecl))
}

fn collect(qr: &QrCode, ecl: QrEcc) -> QrSymbol {
    let size = qr.size();
    let mut modules = Vec::with_capacity((size * size) as usize);
    for y in 0..size {
        for x in 0..size {
            modules.push(u8::from(qr.get_module(x, y)));
        }
    }
    QrSymbol {
        modules,
        size: size as u32,
        version: qr.version().value(),
        ecl,
    }
}

/// Where the QR sits relative to the LCR symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DualLayout {
    /// QR to the right. The default: most scanning surfaces are wider than they are tall.
    Beside,
    /// QR underneath.
    Below,
}

/// How to compose a dual render.
#[derive(Clone, Debug)]
pub struct DualOptions {
    /// Rasterisation of the LCR half. The QR is drawn at the **same** `module_px`.
    pub render: RenderOptions,
    /// Error correction for the QR half.
    pub ecl: QrEcc,
    /// Where the QR sits relative to the LCR symbol.
    pub layout: DualLayout,
    /// Extra separation between the two symbols, in modules, on top of the quiet zones
    /// each already carries.
    pub gap: u32,
}

impl Default for DualOptions {
    fn default() -> Self {
        DualOptions {
            render: RenderOptions::screen(),
            ecl: QrEcc::Medium,
            layout: DualLayout::Beside,
            // Both symbols already carry their own quiet zone, so the white between them is
            // at least 6 modules before this is added. Four more buys visual separation and
            // makes it obvious to a human that these are two symbols and not one.
            gap: 4,
        }
    }
}

/// Render an LCR symbol beside a QR carrying `url`, as one raster.
///
/// **Both halves are drawn at the same module pitch**, which is the property that makes a
/// dual-render useful rather than decorative: a symbol's scanning distance is set by how
/// many sensor pixels land on a module, so matched pitch means both halves become readable
/// at roughly the same distance. Rendering the QR smaller because it has fewer modules
/// would produce a fallback that stops working before the thing it is a fallback for.
pub fn render_dual_rgb(
    symbol: &Symbol,
    url: &str,
    opts: &DualOptions,
) -> Result<Raster, QrTooLong> {
    let lcr = render_rgb(symbol, &opts.render);
    let qr = qr_encode_text(url, opts.ecl)?;
    Ok(compose(&lcr, &qr, opts))
}

/// [`render_dual_rgb`], encoded as a PNG.
#[cfg(feature = "png")]
pub fn render_dual_png(
    symbol: &Symbol,
    url: &str,
    opts: &DualOptions,
) -> Result<Vec<u8>, QrTooLong> {
    Ok(crate::png_encode(&render_dual_rgb(symbol, url, opts)?))
}

fn compose(lcr: &Raster, qr: &QrSymbol, opts: &DualOptions) -> Raster {
    let px = opts.render.module_px.max(1);
    let qr_px = qr.outer() * px;
    let gap_px = opts.gap * px;

    let (w, h) = match opts.layout {
        DualLayout::Beside => (lcr.width + gap_px + qr_px, lcr.height.max(qr_px)),
        DualLayout::Below => (lcr.width.max(qr_px), lcr.height + gap_px + qr_px),
    };

    // White ground: the gap and any centring slack are quiet zone as far as either
    // detector is concerned, so they must be the same white (§13.1 point 5).
    let mut data = Vec::with_capacity((w * h * 3) as usize);
    for _ in 0..(w * h) {
        data.extend_from_slice(&[WHITE.r, WHITE.g, WHITE.b]);
    }
    let mut out = Raster {
        width: w,
        height: h,
        data,
    };

    // Centre each symbol on the cross axis, so neither sits against an edge.
    let (lcr_x, lcr_y, qr_x, qr_y) = match opts.layout {
        DualLayout::Beside => (0, (h - lcr.height) / 2, lcr.width + gap_px, (h - qr_px) / 2),
        DualLayout::Below => ((w - lcr.width) / 2, 0, (w - qr_px) / 2, lcr.height + gap_px),
    };

    blit(&mut out, lcr, lcr_x, lcr_y);
    draw_qr(&mut out, qr, qr_x, qr_y, px);
    out
}

fn blit(dst: &mut Raster, src: &Raster, at_x: u32, at_y: u32) {
    for y in 0..src.height {
        let s = ((y * src.width) * 3) as usize;
        let d = (((at_y + y) * dst.width + at_x) * 3) as usize;
        let n = (src.width * 3) as usize;
        dst.data[d..d + n].copy_from_slice(&src.data[s..s + n]);
    }
}

fn draw_qr(dst: &mut Raster, qr: &QrSymbol, at_x: u32, at_y: u32, px: u32) {
    // `at_x`/`at_y` address the outer edge of the quiet zone, which is already white, so
    // only the dark modules need painting.
    for my in 0..qr.size {
        for mx in 0..qr.size {
            if !qr.get(mx, my) {
                continue;
            }
            let x0 = at_x + (mx + QR_QUIET) * px;
            let y0 = at_y + (my + QR_QUIET) * px;
            for y in y0..y0 + px {
                let row = ((y * dst.width + x0) * 3) as usize;
                for i in 0..px as usize {
                    dst.data[row + i * 3] = BLACK.r;
                    dst.data[row + i * 3 + 1] = BLACK.g;
                    dst.data[row + i * 3 + 2] = BLACK.b;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lcr_core::{Compression, EccLevel, Profile};
    use lcr_encode::EncodeOptions;

    fn symbol() -> Symbol {
        lcr_encode::encode_bytes(
            br#"{"id":"abc","n":7}"#,
            &EncodeOptions {
                profile: Profile::C1,
                compression: Some(Compression::None),
                ecc: EccLevel::DEFAULT,
                ..Default::default()
            },
        )
        .expect("encode")
    }

    /// Read the QR half back with an independent decoder.
    ///
    /// This is what makes a dual-render worth shipping: the entire point is that a
    /// stranger's stock camera app reads that half, and nothing else here can establish
    /// that it will. `rqrr` is a different implementation by a different author.
    fn read_qr_half(raster: &Raster) -> String {
        let mut prepared = rqrr::PreparedImage::prepare_from_greyscale(
            raster.width as usize,
            raster.height as usize,
            |x, y| {
                let p = raster.pixel(x as u32, y as u32);
                // Rec.709 luma, so the LCR half's colours reduce sensibly rather than
                // thresholding into noise that the detector might latch onto.
                (0.2126 * p.r as f32 + 0.7152 * p.g as f32 + 0.0722 * p.b as f32) as u8
            },
        );
        let grids = prepared.detect_grids();
        assert!(!grids.is_empty(), "no QR grid found in the dual render");
        let (_, text) = grids
            .iter()
            .find_map(|g| g.decode().ok())
            .expect("no grid decoded");
        text
    }

    #[test]
    fn the_qr_half_of_a_dual_render_decodes() {
        let url = "https://example.org/lcr/abc123";
        for layout in [DualLayout::Beside, DualLayout::Below] {
            let opts = DualOptions {
                layout,
                ..Default::default()
            };
            let raster = render_dual_rgb(&symbol(), url, &opts).expect("dual render");
            assert_eq!(read_qr_half(&raster), url, "layout {layout:?}");
        }
    }

    /// The property that makes the fallback a fallback: matched module pitch, so the QR
    /// half does not stop working before the LCR half it backs up.
    #[test]
    fn both_halves_share_a_module_pitch() {
        let opts = DualOptions {
            render: RenderOptions::screen().with_module_px(12),
            ..Default::default()
        };
        let lcr = render_rgb(&symbol(), &opts.render);
        let qr = qr_encode_text("https://example.org/x", opts.ecl).unwrap();
        let dual = render_dual_rgb(&symbol(), "https://example.org/x", &opts).unwrap();

        // Beside: width is the two symbols plus the gap, all at the same pitch.
        assert_eq!(
            dual.width,
            lcr.width + opts.gap * 12 + qr.outer() * 12,
            "the composed width must be exactly both symbols at one pitch",
        );
        assert_eq!(dual.height, lcr.height.max(qr.outer() * 12));
    }

    /// The LCR half must survive composition — a dual render that broke the thing it exists
    /// to carry would be worse than no dual render.
    #[test]
    fn the_lcr_half_of_a_dual_render_still_decodes() {
        let sym = symbol();
        let opts = DualOptions {
            render: RenderOptions::screen().with_module_px(12),
            ..Default::default()
        };
        let raster = render_dual_rgb(&sym, "https://example.org/lcr/abc123", &opts).unwrap();
        let out = lcr_decode::Decoder::new()
            .decode_rgb(&raster.data, raster.width, raster.height, raster.width * 3)
            .expect("the LCR half must decode out of the composed image");
        match out.packet {
            lcr_core::Packet::Unsigned { body, .. } => {
                assert_eq!(body, br#"{"id":"abc","n":7}"#);
            }
            other => panic!("unexpected packet {other:?}"),
        }
    }

    #[test]
    fn a_url_too_long_for_any_version_is_an_error_not_a_panic() {
        let huge = "x".repeat(3000);
        assert!(qr_encode_text(&huge, QrEcc::High).is_err());
    }
}
