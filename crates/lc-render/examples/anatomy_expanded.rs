//! Generate `examples/anatomy-expanded-c3.png`: every cell of a symbol coloured by the job
//! it does — the ring's fields, and where each layer's information and parity bits land.
//!
//! `just anatomy-expanded`
//!
//! The companion to `anatomy.rs`. That one points at features; this one colours the whole
//! symbol, in the spirit of Russ Cox's QArt diagrams: the interesting thing about a matrix
//! code is not that it *has* error correction but *where the error correction is*.
//!
//! Everything is computed, not drawn: module roles come from `Layout`, the ring's fields
//! from `border::format_slot`, and information-versus-parity from the real LDPC code and the
//! real §8.3 interleaver — `Interleaver::position` inverted, so each module reports the
//! codeword index that actually lands on it.

use lc_core::border;
use lc_core::ecc::ldpc::{CHROMA_RATE_INDEX, Code, LUMA_RATE_INDEX};
use lc_core::interleave::Interleaver;
use lc_core::layout::Layout;
use lc_core::{ModuleRole, Packet, Profile, Version};
use lc_decode::Decoder;
use lc_encode::EncodeOptions;
use lc_render::{Canvas, RenderOptions, render_png, render_rgb};
use std::fmt::Write as _;

/// Forced to v5: big enough that the interleaver's scatter is visible, small enough that a
/// module is still a legible square on the page.
const VERSION: u8 = 5;
const PX: f32 = 10.0;
const PAGE: (u32, u32) = (1340, 1700);

const INK: &str = "#e8e4dc";
const SUNK: &str = "#14120f";

/// One colour per job. Chosen to stay distinguishable against the dark page and each other.
const C_LUMA_INFO: &str = "#E2AC71";
const C_LUMA_PARITY: &str = "#8a5a22";
const C_CHROMA_INFO: &str = "#95B5FE";
const C_CHROMA_PARITY: &str = "#2E5FA3";
const C_REF: &str = "#F89ABD";
const C_SEQ: &str = "#81C58C";
const C_PROFILE: &str = "#D0021B";
const C_ECC: &str = "#FFB000";
const C_MASK: &str = "#7B4FBF";
const C_RESERVED: &str = "#5c5850";
const C_PARITY_RING: &str = "#0C4784";
const C_RINGPAY: &str = "#2E9E8F";
const C_EMPTY: &str = "#262019";

/// A patch to paint, in canvas coordinates. Fractional, because a chroma cell carries
/// more than one bit and each gets its own slice.
struct Cell {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    fill: &'static str,
}

impl Cell {
    fn module(x: i32, y: i32, fill: &'static str) -> Cell {
        Cell {
            x: x as f32,
            y: y as f32,
            w: 1.0,
            h: 1.0,
            fill,
        }
    }
}

fn main() {
    let dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/anatomy".into());
    std::fs::create_dir_all(&dir).expect("create output directory");

    let version = Version::new(VERSION).unwrap();
    let opts = EncodeOptions {
        profile: Profile::C3,
        reference_border: true,
        version: Some(version),
        compression: Some(lc_core::Compression::None),
        ..Default::default()
    };
    // Sized from the capacity rather than chosen: a diagram of where the bits go should
    // show a symbol with no spare room, or the last block reads as half empty.
    let bytes = lc_core::capacity::capacity_for_symbol(
        version,
        opts.profile,
        opts.ecc,
        opts.reference_border,
    )
    .body_bytes;
    let payload: Vec<u8> = (0..bytes).map(|i| (i.wrapping_mul(31) + 7) as u8).collect();
    let sym = lc_encode::encode_bytes(&payload, &opts).expect("encode");
    let render = RenderOptions {
        module_px: 12,
        ..RenderOptions::screen()
    };

    // Nothing is explained that does not decode.
    let raster = render_rgb(&sym, &render);
    let out = Decoder::new()
        .decode_rgb(&raster.data, raster.width, raster.height, raster.width * 3)
        .unwrap_or_else(|e| panic!("the expanded-anatomy symbol does not decode: {e}"));
    assert!(
        matches!(&out.packet, Packet::Unsigned { .. }),
        "expected an unsigned packet"
    );

    let f = sym.format;
    let side = f.version.side();
    let layout = Layout::for_symbol(f.version, f.profile, true);
    let cap = layout.capacity();
    let palette = sym.palette();
    let hues = palette.hues;
    let bpc = f.profile.chroma_bits_per_cell();
    let cell_side = f.profile.chroma_cell();
    let margin = Canvas::of(&sym).margin as i32;
    let canvas = side as i32 + 2 * margin;

    // ---------------------------------------------------------------- codes
    let luma_code = Code::new(LUMA_RATE_INDEX[f.ecc.luma as usize & 3], cap.luma_bits)
        .expect("luma code exists");
    let chroma_code = Code::new(
        CHROMA_RATE_INDEX[f.ecc.chroma as usize & 3],
        cap.chroma_bits,
    )
    .expect("chroma code exists");

    // Placement index -> codeword index, by inverting §8.3's permutation. A placement holds
    // an information bit when the codeword index it received is below `info_bits`.
    let invert = |n: usize| -> Vec<usize> {
        let it = Interleaver::new(n);
        let mut inv = vec![0usize; n];
        for j in 0..n {
            inv[it.position(j)] = j;
        }
        inv
    };
    let luma_at = invert(cap.luma_bits);
    let chroma_at = invert(cap.chroma_bits);
    let (luma_k, chroma_k) = (luma_code.info_bits(), chroma_code.info_bits());

    let ring_cells: Vec<(i32, i32)> = border::payload_cells(side).collect();

    // ---------------------------------------------------------------- panel: luma layer
    let mut luma_panel: Vec<Cell> = Vec::new();
    for (i, &(x, y)) in layout.payload.iter().enumerate() {
        let fill = if luma_at[i] < luma_k {
            C_LUMA_INFO
        } else {
            C_LUMA_PARITY
        };
        luma_panel.push(Cell::module(x as i32 + margin, y as i32 + margin, fill));
    }
    for (n, &(rx, ry)) in ring_cells.iter().enumerate() {
        let i = layout.payload.len() + n;
        if i < cap.luma_bits {
            let fill = if luma_at[i] < luma_k {
                C_LUMA_INFO
            } else {
                C_LUMA_PARITY
            };
            luma_panel.push(Cell::module(rx + margin, ry + margin, fill));
        }
    }
    let interior: Vec<(u16, u16)> = (0..side)
        .flat_map(|y| (0..side).map(move |x| (x, y)))
        .filter(|&(x, y)| matches!(layout.roles.at(x, y), ModuleRole::Reference))
        .collect();
    for &(x, y) in &interior {
        luma_panel.push(Cell::module(x as i32 + margin, y as i32 + margin, C_REF));
    }

    // ---------------------------------------------------------------- panel: chroma layer
    let mut chroma_panel: Vec<Cell> = Vec::new();
    let slice = cell_side as f32 / bpc as f32;
    for (c, &(cx, cy)) in layout.chroma_cells.iter().enumerate() {
        // A cell carries `bpc` bits and they need not agree, so each gets its own slice
        // rather than the cell taking the colour of its first bit.
        for b in 0..bpc {
            chroma_panel.push(Cell {
                x: (cx * cell_side) as f32 + margin as f32,
                y: (cy * cell_side) as f32 + margin as f32 + b as f32 * slice,
                w: cell_side as f32,
                h: slice,
                fill: if chroma_at[c * bpc + b] < chroma_k {
                    C_CHROMA_INFO
                } else {
                    C_CHROMA_PARITY
                },
            });
        }
    }
    for (n, &(rx, ry)) in ring_cells.iter().enumerate() {
        let c = layout.chroma_cells.len() + n;
        if c * bpc + bpc <= cap.chroma_bits {
            for b in 0..bpc {
                chroma_panel.push(Cell {
                    x: rx as f32 + margin as f32,
                    y: ry as f32 + margin as f32 + b as f32 / bpc as f32,
                    w: 1.0,
                    h: 1.0 / bpc as f32,
                    fill: if chroma_at[c * bpc + b] < chroma_k {
                        C_CHROMA_INFO
                    } else {
                        C_CHROMA_PARITY
                    },
                });
            }
        }
    }
    for &(x, y) in &interior {
        chroma_panel.push(Cell::module(x as i32 + margin, y as i32 + margin, C_REF));
    }

    // ---------------------------------------------------------------- panel: the ring
    //
    // Data bits 0–14 of the ring word (D21), then 15 parity. The field a bit belongs to is
    // what gets painted, so the page shows where "profile" physically is.
    let field = |bit: usize| -> &'static str {
        match bit {
            0..=3 => C_PROFILE,
            4..=7 => C_ECC,
            8..=10 => C_MASK,
            11..=14 => C_RESERVED,
            _ => C_PARITY_RING,
        }
    };
    let mut ring_panel: Vec<Cell> = Vec::new();
    for p in 0..border::positions(side) {
        let (ox, oy) = border::outer_cell(p, side);
        let fill = if border::is_reference(p) {
            C_REF
        } else {
            C_SEQ
        };
        ring_panel.push(Cell::module(ox + margin, oy + margin, fill));
        // A two-deep position owns its inner cell as well; leaving it dark would draw a
        // reference as half missing.
        let (ix, iy) = border::inner_cell(p, side);
        if border::is_two_deep(p) && border::position(ix, iy, side) == Some(p) {
            ring_panel.push(Cell::module(ix + margin, iy + margin, fill));
        }
    }
    for copy in 0..border::FORMAT_COPIES {
        for bit in 0..border::FORMAT_COPY_BITS {
            let (p, _) = border::format_slot(copy, bit, side, hues);
            let (ox, oy) = border::outer_cell(p, side);
            ring_panel.push(Cell::module(ox + margin, oy + margin, field(bit)));
        }
    }
    for &(rx, ry) in &ring_cells {
        ring_panel.push(Cell::module(rx + margin, ry + margin, C_RINGPAY));
    }

    // ---------------------------------------------------------------- page
    std::fs::write(
        format!("{dir}/expanded-symbol.png"),
        render_png(&sym, &render),
    )
    .unwrap();

    let panel = |cells: &[Cell]| -> String {
        let n = canvas as f32 * PX;
        let mut s = format!(
            r#"<svg width="{n}" height="{n}" viewBox="0 0 {n} {n}" xmlns="http://www.w3.org/2000/svg"><rect width="{n}" height="{n}" fill="{SUNK}"/>"#
        );
        // Every canvas cell gets a ground first, so unused ring slots and the quiet zone
        // read as "nothing here" rather than as holes.
        for y in 0..canvas {
            for x in 0..canvas {
                let _ = write!(
                    s,
                    r#"<rect x="{:.1}" y="{:.1}" width="{PX}" height="{PX}" fill="{C_EMPTY}"/>"#,
                    x as f32 * PX,
                    y as f32 * PX
                );
            }
        }
        for c in cells {
            let _ = write!(
                s,
                r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" fill="{}"/>"#,
                c.x * PX,
                c.y * PX,
                c.w * PX,
                c.h * PX,
                c.fill
            );
        }
        s.push_str("</svg>");
        s
    };

    let key = |items: &[(&str, String)]| -> String {
        let mut s = String::from(r#"<ul class="key">"#);
        for (colour, label) in items {
            let _ = write!(s, r#"<li><i style="background:{colour}"></i>{label}</li>"#);
        }
        s.push_str("</ul>");
        s
    };

    let luma_rate = luma_k as f32 / cap.luma_bits as f32;
    let chroma_rate = chroma_k as f32 / cap.chroma_bits as f32;
    let (pw, ph) = PAGE;
    let html = format!(
        r#"<!doctype html>
<html><head><meta charset="utf-8"><title>LC Code anatomy, expanded</title>
<style>
  html, body {{ margin: 0; background: #0d0b09; }}
  body {{ width: {pw}px; height: {ph}px; overflow: hidden; color: {INK};
         font-family: -apple-system, "Helvetica Neue", Helvetica, Arial, sans-serif; }}
  header {{ padding: 28px 44px 0; }}
  h1 {{ margin: 0; font-size: 30px; letter-spacing: -0.01em; }}
  .sub {{ margin: 6px 0 0; font-size: 16px; color: #9a948a; max-width: 1200px; }}
  main {{ display: grid; grid-template-columns: auto minmax(0, 1fr); gap: 22px 28px;
          padding: 18px 40px; }}
  section {{ display: contents; }}
  .panel {{ background: {SUNK}; border: 1px solid #2a2520; border-radius: 4px; padding: 8px;
            line-height: 0; }}
  .side {{ align-self: start; padding-top: 4px; }}
  h2 {{ margin: 0 0 6px; font-size: 19px; }}
  p.note {{ margin: 0 0 10px; font-size: 14.5px; line-height: 1.45; color: #b8b2a8; }}
  ul.key {{ list-style: none; margin: 0; padding: 0; font-size: 13.5px; }}
  ul.key li {{ margin: 0 0 5px; display: flex; align-items: center; gap: 8px;
               break-inside: avoid; }}
  ul.key i {{ width: 13px; height: 13px; border-radius: 2px; flex: none; }}
  code {{ font-family: "SF Mono", Menlo, monospace; font-size: 0.93em; }}
  img.real {{ float: right; width: 190px; height: 190px; margin: 0 0 12px 16px;
              image-rendering: pixelated; border: 1px solid #2a2520; border-radius: 3px; }}
</style></head><body>
<header>
  <h1>Where everything actually is</h1>
  <p class="sub">A v{ver} {prof:?} symbol, every cell coloured by the job it does. Module roles come from the layout, the ring's fields from the format map, and information-versus-parity from the real error-correcting code and interleaver — nothing here is drawn by hand.</p>
</header>
<main>
  <div class="panel">{ring_svg}</div>
  <div class="side">
    <h2>The ring</h2>
    <img class="real" src="expanded-symbol.png" alt="The symbol these diagrams describe, as actually rendered.">
    <p class="note">The ring locates the symbol and describes it. Its light/dark pattern is the position sequence — that is what gives the <b>version</b>, the rotation and the mirror, which is why no version field is needed in the word itself. Every third position is a fixed colour for the illumination fit. The remaining positions spend their <i>colour</i> choice on the format word, twice over, and their inner cell on payload.</p>
    {ring_key}
  </div>

  <div class="panel">{luma_svg}</div>
  <div class="side">
    <h2>The brightness layer</h2>
    <p class="note">One bit per module, {lk} of them information and {lp} parity — a rate of {lrate:.2}. The two are shuffled across the whole symbol by the interleaver, which is why they look mixed rather than banded: a scratch or a highlight then damages a little of every block instead of destroying one.</p>
    {luma_key}
  </div>

  <div class="panel">{chroma_svg}</div>
  <div class="side">
    <h2>The colour layer</h2>
    <p class="note">{ck} information and {cp} parity bits, a rate of {crate_:.2} — deliberately more protection than brightness gets, because colour is the layer defocus destroys first. {cbits} bits ride on each module here, since this profile keeps one hue decision per module.</p>
    {chroma_key}
  </div>
</main>
</body></html>
"#,
        ver = f.version.get(),
        prof = f.profile,
        ring_svg = panel(&ring_panel),
        luma_svg = panel(&luma_panel),
        chroma_svg = panel(&chroma_panel),
        lk = luma_k,
        lp = cap.luma_bits - luma_k,
        lrate = luma_rate,
        ck = chroma_k,
        cp = cap.chroma_bits - chroma_k,
        crate_ = chroma_rate,
        cbits = bpc,
        ring_key = key(&[
            (
                C_SEQ,
                "Position sequence — version, rotation, mirror".into()
            ),
            (
                C_REF,
                "Fixed-colour reference, for the illumination fit".into()
            ),
            (
                C_PROFILE,
                format!("Profile field — 4 bits, {:?} here, 16 possible", f.profile)
            ),
            (
                C_ECC,
                format!(
                    "Error-correction levels — luma {}, chroma {}",
                    f.ecc.luma, f.ecc.chroma
                )
            ),
            (C_MASK, format!("Mask id — {} of 8", f.mask)),
            (C_RESERVED, "Reserved, must read zero".into()),
            (C_PARITY_RING, "BCH parity for the format word".into()),
            (C_RINGPAY, "Ring payload cells — ordinary data".into()),
        ]),
        luma_key = key(&[
            (C_LUMA_INFO, format!("Information — {luma_k} bits")),
            (
                C_LUMA_PARITY,
                format!("Parity — {} bits", cap.luma_bits - luma_k)
            ),
            (
                C_REF,
                format!("Known-colour reference — {}", interior.len())
            ),
            (C_EMPTY, "Quiet zone and ring".into()),
        ]),
        chroma_key = key(&[
            (C_CHROMA_INFO, format!("Information — {chroma_k} bits")),
            (
                C_CHROMA_PARITY,
                format!("Parity — {} bits", cap.chroma_bits - chroma_k)
            ),
            (C_REF, "Known-colour reference".into()),
            (C_EMPTY, "Quiet zone and ring".into()),
        ]),
    );

    std::fs::write(format!("{dir}/anatomy-expanded-c3.html"), html).unwrap();
    println!(
        "expanded anatomy: v{} {:?} {side}x{side} — luma {}+{} parity, chroma {}+{} parity -> {dir}/anatomy-expanded-c3.html",
        f.version.get(),
        f.profile,
        luma_k,
        cap.luma_bits - luma_k,
        chroma_k,
        cap.chroma_bits - chroma_k
    );
}
