//! Generate `examples/anatomy-ldpc-c3.png`: how the error correction works, shown on one
//! real symbol — what LDPC adds when encoding, what the decoder reads back from a degraded
//! capture, and how belief propagation turns that into the right bits.
//!
//! `just anatomy-ldpc`
//!
//! The third of the anatomy figures. `anatomy` points at features and `anatomy_expanded`
//! shows where information and parity bits land; this one follows the luma layer's bits
//! through an actual decode. Nothing is illustrative: the parity check drawn is a real row
//! of the real code, the confidences are the decoder's own LLRs for a degraded render, and
//! each iteration panel is the decoder stopped after that many iterations.

use lc_core::border::RingMode;
use lc_core::ecc::ldpc::{Code, LUMA_RATE_INDEX};
use lc_core::interleave::{Interleaver, multiplier};
use lc_core::layout::Layout;
use lc_core::palette::Palette;
use lc_core::{Compression, Packet, Profile, Version};
use lc_decode::Decoder;
use lc_decode::demod;
use lc_decode::illum::IlluminationModel;
use lc_decode::image::{ImageView, PixelFormat};
use lc_decode::ring;
use lc_encode::EncodeOptions;
use lc_render::{Canvas, Raster, RenderOptions, png_encode, render_png, render_rgb};
use std::fmt::Write as _;

/// Forced to v4: 33 × 33 keeps a module legible in the small iteration panels.
const VERSION: u8 = 4;
const PX: f32 = 11.0;
const PX_SMALL: f32 = 4.0;
const PAGE: (u32, u32) = (1340, 3000);

const INK: &str = "#e8e4dc";
const SUNK: &str = "#14120f";
const C_EMPTY: &str = "#262019";
const C_LUMA: &str = "#4a3d2a";
const C_MEMBER: &str = "#FFB000";
const C_WRONG: &str = "#ff3b30";
const C_RIGHT: &str = "#6b5a3e";

/// What the luma layer looked like to the decoder for one capture.
struct Read {
    /// Unmasked channel LLRs, one per luma position, in placement order.
    llrs: Vec<f32>,
    /// The same, deinterleaved into codeword order — what LDPC receives.
    codeword_llrs: Vec<f32>,
}

/// Run the real pipeline's stages up to the luma LLRs.
fn read_luma(r: &Raster) -> Option<(Read, Layout, u8)> {
    let img = ImageView::packed(&r.data, r.width, r.height, PixelFormat::Rgb8).ok()?;
    let fix = ring::detect_ring(&img.gray_buffer(), r.width, r.height, Version::MAX)?;
    let (format, _) = ring::read_ring_format(&img, &fix.transform, fix.version).ok()?;
    let layout = Layout::for_symbol(format.version, format.profile, format.reference_border);
    let palette = Palette::for_profile(format.profile);
    let mode = Some(RingMode::Data(format.to_ring_codeword()));
    let refs = demod::collect_references(&img, &fix.transform, &layout, &palette, mode);
    let illum = IlluminationModel::fit(&refs, layout.side, format.version.get());
    let stats = demod::measure(&refs, &illum, &palette);
    let d = demod::demodulate(
        &img,
        &fix.transform,
        &layout,
        &palette,
        &illum,
        &stats,
        true,
    );
    let mut llrs = d.luma;
    demod::unmask_luma(&mut llrs, &layout, format.mask);
    let codeword_llrs = Interleaver::new(llrs.len()).deinterleave(&llrs);
    Some((
        Read {
            llrs,
            codeword_llrs,
        },
        layout,
        format.ecc.luma,
    ))
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
        compression: Some(Compression::None),
        ..Default::default()
    };
    let bytes =
        lc_core::capacity::capacity_for_symbol(version, opts.profile, opts.ecc, true).body_bytes;
    let payload: Vec<u8> = (0..bytes).map(|i| (i.wrapping_mul(31) + 7) as u8).collect();
    let sym = lc_encode::encode_bytes(&payload, &opts).expect("encode");
    let render = RenderOptions {
        module_px: 6,
        ..RenderOptions::screen()
    };
    let clean = render_rgb(&sym, &render);

    // ---------------------------------------------------------------- the truth
    let (truth_read, layout, ecc_luma) = read_luma(&clean).expect("clean render reads");
    let cap = layout.capacity();
    let code = Code::new(LUMA_RATE_INDEX[ecc_luma as usize & 3], cap.luma_bits).expect("code");
    let truth = code.decode(&truth_read.codeword_llrs, 50).expect("decode");
    assert!(truth.converged, "the clean render's luma layer converges");
    let truth = truth.codeword;
    assert_eq!(code.unsatisfied(&truth), 0);

    // ---------------------------------------------------------------- the capture
    //
    // Chosen, not hand-picked: the mildest defocus-plus-noise from a fixed list that gives
    // the decoder real work — at least 1.5% of luma bits wrong going in — and still decodes.
    let candidates: Vec<(u32, f32)> = (2..=5)
        .flat_map(|r| [0.06f32, 0.08, 0.10, 0.12, 0.14].map(|s| (r, s)))
        .collect();
    let wrong_of = |word: &[bool]| -> Vec<usize> {
        (0..word.len()).filter(|&j| word[j] != truth[j]).collect()
    };
    let mut chosen = None;
    for &(radius, sigma) in &candidates {
        let degraded = noise(&blur(&clean, radius), sigma, 0x1d9c);
        let decodes = matches!(
            Decoder::new()
                .decode_rgb(&degraded.data, degraded.width, degraded.height, degraded.width * 3)
                .map(|o| o.packet),
            Ok(Packet::Unsigned { ref body, .. }) if *body == payload
        );
        let Some((read, _, _)) = read_luma(&degraded) else {
            continue;
        };
        let start = code.decode(&read.codeword_llrs, 0).expect("decode");
        let full = code.decode(&read.codeword_llrs, 50).expect("decode");
        let wrong = wrong_of(&start.codeword).len();
        if decodes && full.converged && wrong * 200 >= 3 * cap.luma_bits && full.iterations >= 3 {
            chosen = Some((radius, sigma, degraded, read, full.iterations));
            break;
        }
    }
    let (radius, sigma, degraded, read, iterations) =
        chosen.expect("one candidate capture needs the decoder and decodes");

    // Every iteration up to convergence: the decoder stopped after `i` of them.
    let steps: Vec<(u16, Vec<usize>, usize)> = (0..=iterations)
        .map(|i| {
            let out = code.decode(&read.codeword_llrs, i).expect("decode");
            (i, wrong_of(&out.codeword), code.unsatisfied(&out.codeword))
        })
        .collect();

    // ---------------------------------------------------------------- geometry
    let positions: Vec<(i32, i32)> = layout.luma_positions().collect();
    let interleaver = Interleaver::new(cap.luma_bits);
    // Codeword index -> module, through the §8.3 interleaver.
    let module_of = |j: usize| positions[interleaver.position(j)];
    let margin = Canvas::of(&sym).margin as i32;
    let canvas = layout.side as i32 + 2 * margin;

    // One parity check to draw: the first whose members all sit inside the grid, so it reads
    // as "these modules" without the ring complicating the picture.
    let side = layout.side as i32;
    let in_grid = |(x, y): (i32, i32)| (0..side).contains(&x) && (0..side).contains(&y);
    let check = (0..code.checks())
        .find(|&c| {
            let m = code.check_members(c);
            m.len() >= 5 && m.iter().all(|&j| in_grid(module_of(j)))
        })
        .expect("a check inside the grid");
    let members = code.check_members(check);
    let member_bits: Vec<u8> = members.iter().map(|&j| truth[j] as u8).collect();

    // ---------------------------------------------------------------- panels
    let panel = |px: f32, paint: &dyn Fn(i32, i32) -> Option<&'static str>, extra: &str| {
        let n = canvas as f32 * px;
        let mut s = format!(
            r#"<svg width="{n}" height="{n}" viewBox="0 0 {n} {n}" xmlns="http://www.w3.org/2000/svg"><rect width="{n}" height="{n}" fill="{SUNK}"/>"#
        );
        for y in 0..canvas {
            for x in 0..canvas {
                let fill = paint(x - margin, y - margin).unwrap_or(C_EMPTY);
                let _ = write!(
                    s,
                    r#"<rect x="{:.1}" y="{:.1}" width="{px}" height="{px}" fill="{fill}"/>"#,
                    x as f32 * px,
                    y as f32 * px
                );
            }
        }
        s.push_str(extra);
        s.push_str("</svg>");
        s
    };
    let is_luma = |x: i32, y: i32| positions.contains(&(x, y));

    // Panel 1: one check's members.
    let member_modules: Vec<(i32, i32)> = members.iter().map(|&j| module_of(j)).collect();
    let mut lines = String::new();
    let centre = |(x, y): (i32, i32)| {
        (
            (x + margin) as f32 * PX + PX / 2.0,
            (y + margin) as f32 * PX + PX / 2.0,
        )
    };
    // A star from the first member to the rest: drawn as the check node it is.
    let hub = {
        let n = member_modules.len() as f32;
        let (sx, sy) = member_modules
            .iter()
            .map(|&m| centre(m))
            .fold((0.0, 0.0), |a, c| (a.0 + c.0 / n, a.1 + c.1 / n));
        (sx, sy)
    };
    for &m in &member_modules {
        let (x, y) = centre(m);
        let _ = write!(
            lines,
            r#"<line x1="{:.1}" y1="{:.1}" x2="{x:.1}" y2="{y:.1}" stroke="{C_MEMBER}" stroke-width="1.6" stroke-opacity="0.75"/>"#,
            hub.0, hub.1
        );
    }
    let _ = write!(
        lines,
        r#"<rect x="{:.1}" y="{:.1}" width="16" height="16" fill="{SUNK}" stroke="{C_MEMBER}" stroke-width="2.5"/><text x="{:.1}" y="{:.1}" fill="{C_MEMBER}" font-family="-apple-system,Helvetica,Arial,sans-serif" font-size="13" font-weight="700" text-anchor="middle">+</text>"#,
        hub.0 - 8.0,
        hub.1 - 8.0,
        hub.0,
        hub.1 + 4.5
    );
    let check_svg = panel(
        PX,
        &|x, y| {
            if member_modules.contains(&(x, y)) {
                Some(C_MEMBER)
            } else if is_luma(x, y) {
                Some(C_LUMA)
            } else {
                None
            }
        },
        &lines,
    );

    // Panel 2: the capture as the decoder saw it — confidence, and the guesses that are
    // wrong. Confidence is |LLR| scaled to the clamp.
    let wrong0: Vec<(i32, i32)> = steps[0].1.iter().map(|&j| module_of(j)).collect();
    let conf_colour = |c: f32| -> &'static str {
        // Five steps of warm grey, dim (unsure) to bright (sure).
        match (c * 5.0) as usize {
            0 => "#2b241c",
            1 => "#4d4233",
            2 => "#77684f",
            3 => "#a8977a",
            _ => "#e0d2b6",
        }
    };
    let conf_at: std::collections::HashMap<(i32, i32), f32> = positions
        .iter()
        .zip(&read.llrs)
        .map(|(&p, &l)| (p, (l.abs() / demod::LLR_CLAMP).min(1.0)))
        .collect();
    let conf_svg = panel(
        PX,
        &|x, y| {
            if wrong0.contains(&(x, y)) {
                Some(C_WRONG)
            } else {
                conf_at.get(&(x, y)).map(|&c| conf_colour(c))
            }
        },
        "",
    );

    // Panel 3: one small panel per iteration.
    let mut strip = String::new();
    for (i, wrong, failing) in &steps {
        let w: Vec<(i32, i32)> = wrong.iter().map(|&j| module_of(j)).collect();
        let svg = panel(
            PX_SMALL,
            &|x, y| {
                if w.contains(&(x, y)) {
                    Some(C_WRONG)
                } else if is_luma(x, y) {
                    Some(C_RIGHT)
                } else {
                    None
                }
            },
            "",
        );
        let label = if *i == 0 {
            "before decoding".to_string()
        } else {
            format!("after {i} iteration{}", if *i == 1 { "" } else { "s" })
        };
        let _ = write!(
            strip,
            r#"<figure><div class="panel small">{svg}</div><figcaption><b>{label}</b><br>{} wrong bits<br>{} of {} checks failing</figcaption></figure>"#,
            wrong.len(),
            failing,
            code.checks()
        );
    }

    // ---------------------------------------------------------------- page
    std::fs::write(format!("{dir}/ldpc-clean.png"), render_png(&sym, &render)).unwrap();
    std::fs::write(format!("{dir}/ldpc-degraded.png"), png_encode(&degraded)).unwrap();

    let k = code.info_bits();
    let n = cap.luma_bits;
    let bits_str: String = member_bits
        .iter()
        .map(|b| b.to_string())
        .collect::<Vec<_>>()
        .join(" ⊕ ");
    let ones = member_bits.iter().filter(|&&b| b == 1).count();
    let first_wrong = steps[0].1.len();

    // The code's shape, counted rather than asserted: how many checks each bit sits in and
    // how many bits each check covers.
    let mut degree = vec![0usize; n];
    let mut sizes = Vec::new();
    for c in 0..code.checks() {
        let m = code.check_members(c);
        sizes.push(m.len());
        for j in m {
            degree[j] += 1;
        }
    }
    let span = |v: &[usize]| {
        let (lo, hi) = (v.iter().min().unwrap(), v.iter().max().unwrap());
        if lo == hi {
            format!("{lo}")
        } else {
            format!("{lo} to {hi}")
        }
    };
    let (bit_degree, check_size) = (span(&degree), span(&sizes));

    // Whether the decoder knew which guesses were shaky: median confidence of the wrong
    // guesses against the right ones, and how many wrong ones sit on the outer ring.
    let confidence =
        |j: usize| (read.llrs[interleaver.position(j)].abs() / demod::LLR_CLAMP).min(1.0);
    let median = |mut v: Vec<f32>| {
        v.sort_by(f32::total_cmp);
        v[v.len() / 2]
    };
    let wrong_conf = median(steps[0].1.iter().map(|&j| confidence(j)).collect());
    let right_conf = median(
        (0..n)
            .filter(|j| !steps[0].1.contains(j))
            .map(confidence)
            .collect(),
    );
    let on_ring = steps[0]
        .1
        .iter()
        .filter(|&&j| interleaver.position(j) >= layout.payload.len())
        .count();

    // ---------------------------------------------------------------- where checks land
    //
    // The drawn check rebuilt by hand from the base graph, lift and interleaver, and checked
    // against the code's own adjacency: the recipe on the page is the whole recipe.
    let z = code.lift();
    let (n_b, k_b, m_b) = code.base_shape();
    let edges = code.base_edges();
    let (row, t) = (check / z, check % z);
    let a = multiplier(n);
    let mut trace = String::new();
    let mut rebuilt = Vec::new();
    for &(_, col, shift) in edges.iter().filter(|e| e.0 == row) {
        let v = col * z + (t + shift as usize) % z;
        let j = if v < k {
            Some(v)
        } else if v < k_b * z {
            None
        } else {
            Some(k + v - k_b * z)
        };
        let kind = if col < k_b { "data" } else { "parity" };
        let rest = match j {
            Some(j) => {
                rebuilt.push(j);
                let slot = interleaver.position(j);
                assert_eq!(slot, a * j % n, "the interleaver is a·j mod n");
                let (x, y) = positions[slot];
                format!("<td>{j}</td><td>{slot}</td><td>({x}, {y})</td>")
            }
            None => r#"<td colspan="3">shortened: a known 0, not drawn</td>"#.into(),
        };
        let _ = write!(
            trace,
            "<tr><td>{col}</td><td>{kind}</td><td>{shift}</td><td>{}</td><td>{v}</td>{rest}</tr>",
            shift as usize % z
        );
    }
    rebuilt.sort_unstable();
    let mut expected = members.clone();
    expected.sort_unstable();
    assert_eq!(rebuilt, expected, "hand-built check matches the code's");

    // The base graph itself: one cell per connection, labelled with its shift mod Z.
    let cell = 19.0f32;
    let (left, top) = (30.0f32, 18.0f32);
    let (gw, gh) = (left + n_b as f32 * cell, top + m_b as f32 * cell);
    let mut base_svg = format!(
        r#"<svg width="{gw}" height="{gh}" viewBox="0 0 {gw} {gh}" xmlns="http://www.w3.org/2000/svg" font-family="-apple-system,Helvetica,Arial,sans-serif">"#
    );
    for col in 0..n_b {
        let x = left + col as f32 * cell;
        let fill = if col < k_b { "#7a6644" } else { "#4d6a7a" };
        let _ = write!(
            base_svg,
            r#"<rect x="{:.1}" y="2" width="{:.1}" height="9" fill="{fill}"/>"#,
            x + 1.0,
            cell - 2.0
        );
        // Shortened bits are the tail of the data columns.
        let lo = (col * z).max(k);
        let hi = ((col + 1) * z).min(k_b * z);
        if hi > lo {
            let frac = (hi - lo) as f32 / z as f32;
            let _ = write!(
                base_svg,
                r##"<rect x="{:.1}" y="2" width="{:.1}" height="9" fill="#2b2620"/>"##,
                x + 1.0 + (cell - 2.0) * (1.0 - frac),
                (cell - 2.0) * frac
            );
        }
        for r in 0..m_b {
            let _ = write!(
                base_svg,
                r##"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" fill="#1c1814"/>"##,
                x + 1.0,
                top + r as f32 * cell + 1.0,
                cell - 2.0,
                cell - 2.0
            );
        }
    }
    for r in 0..m_b {
        let _ = write!(
            base_svg,
            r##"<text x="{:.1}" y="{:.1}" fill="#9a948a" font-size="10" text-anchor="end">{r}</text>"##,
            left - 6.0,
            top + r as f32 * cell + cell / 2.0 + 3.5
        );
    }
    for &(r, col, shift) in &edges {
        let (fill, ink) = if r == row {
            (C_MEMBER, SUNK)
        } else {
            ("#5a4c38", INK)
        };
        let (x, y) = (left + col as f32 * cell, top + r as f32 * cell);
        let _ = write!(
            base_svg,
            r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" fill="{fill}"/><text x="{:.1}" y="{:.1}" fill="{ink}" font-size="9" text-anchor="middle">{}</text>"#,
            x + 1.0,
            y + 1.0,
            cell - 2.0,
            cell - 2.0,
            x + cell / 2.0,
            y + cell / 2.0 + 3.2,
            shift as usize % z
        );
    }
    let _ = write!(
        base_svg,
        r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{cell}" fill="none" stroke="{C_MEMBER}" stroke-width="1.5"/></svg>"#,
        left,
        top + row as f32 * cell,
        n_b as f32 * cell
    );

    // The same recipe at other sizes.
    let rate_index = LUMA_RATE_INDEX[ecc_luma as usize & 3];
    let mut versions = String::new();
    for v in [1u8, 4, 10, 20, 40, 64] {
        let l = Layout::for_symbol(Version::new(v).unwrap(), Profile::C3, true);
        let nv = l.capacity().luma_bits;
        let c = Code::new(rate_index, nv).expect("code");
        let (nb, _, mb) = c.base_shape();
        let _ = write!(
            versions,
            "<tr{}><td>v{v}</td><td>{s}×{s}</td><td>{nv}</td><td>{nb} × {mb}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            if v == VERSION { r#" class="here""# } else { "" },
            c.lift(),
            c.shortened_bits(),
            c.info_bits(),
            c.checks(),
            multiplier(nv),
            s = l.side,
        );
    }

    let (pw, ph) = PAGE;
    let html = format!(
        r#"<!doctype html>
<html><head><meta charset="utf-8"><title>LC Code anatomy: error correction</title>
<style>
  html, body {{ margin: 0; background: #0d0b09; }}
  body {{ width: {pw}px; height: {ph}px; overflow: hidden; color: {INK};
         font-family: -apple-system, "Helvetica Neue", Helvetica, Arial, sans-serif; }}
  header {{ padding: 28px 44px 0; }}
  h1 {{ margin: 0; font-size: 30px; letter-spacing: -0.01em; }}
  .sub {{ margin: 6px 0 0; font-size: 16px; color: #9a948a; max-width: 1220px; }}
  main {{ display: grid; grid-template-columns: auto minmax(0, 1fr); gap: 26px 30px;
          padding: 20px 40px 0; }}
  .panel {{ background: {SUNK}; border: 1px solid #2a2520; border-radius: 4px; padding: 7px;
            line-height: 0; display: inline-block; }}
  .side {{ align-self: start; }}
  h2 {{ margin: 0 0 8px; font-size: 20px; }}
  h2 .n {{ color: {C_MEMBER}; margin-right: 6px; }}
  p {{ margin: 0 0 10px; font-size: 15px; line-height: 1.5; color: #c4beb3; }}
  b {{ color: {INK}; }}
  .rule {{ font-family: "SF Mono", Menlo, monospace; font-size: 15px; color: {C_MEMBER};
           background: #1c1814; padding: 8px 12px; border-radius: 4px; display: inline-block;
           margin: 2px 0 12px; }}
  .strip {{ grid-column: 1 / -1; display: flex; flex-wrap: wrap; gap: 14px 18px;
            align-items: flex-start; }}
  figure {{ margin: 0; }}
  figcaption {{ font-size: 13px; line-height: 1.4; color: #b8b2a8; margin-top: 6px; }}
  .wide {{ grid-column: 1 / -1; }}
  .swatch {{ display: inline-block; width: 12px; height: 12px; border-radius: 2px;
             vertical-align: -1px; margin: 0 4px 0 2px; }}
  ol.steps {{ margin: 0 0 4px; padding-left: 22px; }}
  ol.steps li {{ font-size: 15px; line-height: 1.5; color: #c4beb3; margin-bottom: 6px; }}
  code {{ font-family: "SF Mono", Menlo, monospace; font-size: 13.5px; color: {C_MEMBER}; }}
  .tables {{ display: flex; flex-wrap: wrap; gap: 18px 36px; align-items: flex-start; }}
  table {{ border-collapse: collapse; font-size: 13px; color: #c4beb3; }}
  th, td {{ padding: 3px 8px; text-align: right; border-bottom: 1px solid #2a2520;
            font-variant-numeric: tabular-nums; white-space: nowrap; }}
  th {{ color: {INK}; font-weight: 600; vertical-align: bottom; }}
  tr.here td {{ color: {C_MEMBER}; }}
  img.cap {{ width: 150px; height: 150px; image-rendering: pixelated; float: right;
             margin: 0 0 10px 16px; border: 1px solid #2a2520; border-radius: 3px; }}
</style></head><body>
<header>
  <h1>How the error correction works</h1>
  <p class="sub">One real v{ver} C3 symbol, followed through the brightness (luma) layer's LDPC code: what the encoder adds, what the decoder reads back from a damaged picture, and how it repairs the mistakes. The colour layer works the same way with its own, stronger code. Every check, confidence and count below comes from running the actual encoder and decoder.</p>
</header>
<main>
  <div class="panel">{check_svg}</div>
  <div class="side">
    <h2><span class="n">1</span>Encoding: bits plus parity</h2>
    <p>The luma layer has room for <b>{n} bits</b>, one per module. The encoder fills <b>{k}</b> of them with your data and computes the other <b>{p}</b> as parity, a rate of {rate:.2}. The interleaver then scatters all {n} across the symbol, so neighbouring modules are unrelated bits.</p>
    <p>Parity is defined by <b>{checks} checks</b>. Each check is a small group of bits whose XOR must be 0 (an even number of 1s). The <span class="swatch" style="background:{C_MEMBER}"></span>highlighted modules are one real check, number {check}, with {mc} members. In this symbol they read:</p>
    <div class="rule">{bits_str} = 0 &nbsp;({ones} ones: even)</div>
    <p>Every bit belongs to only a few checks ({bit_degree}, in this code) and every check covers only a handful of bits ({check_size}). That sparseness is the "low-density" in LDPC, and it is what makes decoding cheap. The {checks} checks all together are what a valid codeword satisfies; a picture that breaks some of them has errors, and <i>which</i> checks break says roughly where.</p>
  </div>

  <div class="panel">{conf_svg}</div>
  <div class="side">
    <h2><span class="n">2</span>Reading: guesses with confidence</h2>
    <img class="cap" src="ldpc-degraded.png" alt="The degraded capture">
    <p>The decoder does not read 0s and 1s. For each module it computes a <b>log-likelihood ratio (LLR)</b>: its sign is the guess (dark or light) and its size is how sure it is, from how close the module's brightness sits to the dark and light levels, measured against the noise seen on the symbol's own reference cells.</p>
    <p>Here the picture was damaged on purpose: defocus of radius {radius} px plus sensor noise σ {sigma} (inset). Brighter modules are confident guesses, dimmer ones unsure. <span class="swatch" style="background:{C_WRONG}"></span><b>{first_wrong} of {n}</b> guesses are simply wrong. That is too many to read directly; the code is there to fix them.</p>
    <p>The wrong guesses were mostly unsure ones: their median confidence is <b>{wrong_pct:.0}%</b> of the maximum, against <b>{right_pct:.0}%</b> for the right ones. The decoder usually knows when it is unsure, and that is the whole reason for passing confidences rather than bits. {on_ring} of the {first_wrong} are on the outer ring, which holds only {ring_pct:.0}% of the bits: in this capture the edge was the hardest part to read.</p>
  </div>

  <div class="wide">
    <h2><span class="n">3</span>Decoding: checks vote, bits update</h2>
    <p>Belief propagation (normalised min-sum) passes messages back and forth along those check memberships. Each check tells each of its members: <i>"given what the others in my group currently believe, you should be this, with this much confidence"</i>, which in min-sum means the XOR of the others' signs at the confidence of the <i>least</i> sure of them. Each bit adds those messages to its own reading from the picture and updates its guess. A confidently-read bit outvotes a few weak suggestions; an unsure bit is pulled into line by the checks around it.</p>
    <p>Repeat until every check is satisfied. Below, the decoder is stopped after each iteration; <span class="swatch" style="background:{C_WRONG}"></span>red is a bit still wrong.</p>
  </div>
  <div class="strip">{strip}</div>

  <div class="wide">
    <h2><span class="n">4</span>Knowing it worked</h2>
    <p><b>All {checks} checks satisfied</b> is how the decoder knows it has <i>a</i> valid codeword, and it stops there: {iterations} iterations here. It is not yet proof it has <i>the</i> codeword. After both layers decode, the payload's frame carries a CRC, and only a CRC match counts as a successful decode. If a layer does not converge, or the CRC fails, the decoder retries before giving up: it re-reads the picture with blur from neighbouring modules subtracted, and refits the colour correction using the bits it did recover (D24–D27).</p>
  </div>

  <div class="wide">
    <h2><span class="n">5</span>Where the checks land, from the version alone</h2>
    <p>Nothing about the checks is stored in the symbol or chosen per message. The decoder reads the version, profile and ECC level from the format word and rebuilds every check with the same arithmetic the encoder used. For this v{ver} symbol:</p>
    <ol class="steps">
      <li><b>Count the modules.</b> v{ver} C3 is {side} × {side}. Besides the reference cells, that leaves {grid_bits} payload modules in the grid and {ring_bits} on the ring: <b>n = {n}</b>.</li>
      <li><b>Pick a base graph.</b> The ECC level sets the rate, {nominal:.2}. Each rate has three stored tables (60, 20 and 10 columns), and the widest one that fits is used: here <b>{n_b} columns × {m_b} rows</b>. Below, each filled cell is a connection, numbered with its shift. The {k_b} <span class="swatch" style="background:#7a6644"></span>data columns join exactly 3 rows each; the {m_b} <span class="swatch" style="background:#4d6a7a"></span>parity columns form a fixed staircase down the right.</li>
      <li><b>Scale it up.</b> The lift <b>Z</b> is the smallest whole number with {n_b} × Z ≥ {n}: {n} ÷ {n_b} = {zf:.2}, rounded up, so <b>Z = {z}</b>. Every cell becomes {z} connections: check <code>row·Z + t</code> covers bit <code>column·Z + (t + shift) mod Z</code>. That gives {m_b} × {z} = <b>{checks} checks</b> over {n_b} × {z} = {nz} bits. That is {short} more than the symbol holds, so the last {short} data bits (<span class="swatch" style="background:#2b2620"></span>dark in the column header) are fixed to 0 and never drawn, leaving k = {k}.</li>
      <li><b>Scatter.</b> Codeword bit j goes to slot <code>{a} · j mod {n}</code>: {a} is n/φ (φ the golden ratio) rounded, then nudged until it shares no factor with n. Slots are numbered along the grid's zigzag walk (pairs of columns, up then down, starting at the bottom right, as QR does), then around the ring.</li>
    </ol>
  </div>
  <div class="wide"><div class="panel">{base_svg}</div></div>
  <div class="wide tables">
    <figure>
      <table><tr><th>base<br>column</th><th></th><th>stored<br>shift</th><th>shift<br>mod Z</th><th>bit<br>c·Z+(t+s) mod Z</th><th>codeword<br>index j</th><th>slot<br>{a}·j mod {n}</th><th>module<br>(x, y)</th></tr>{trace}</table>
      <figcaption>Check {check} is row {row} (the outlined row) at t = {t}, rebuilt step by step. The result is exactly the {mc} modules highlighted in panel 1.</figcaption>
    </figure>
    <figure>
      <table><tr><th>version</th><th>size</th><th>n</th><th>base graph</th><th>Z</th><th>shortened</th><th>k</th><th>checks</th><th>a</th></tr>{versions}</table>
      <figcaption>The same recipe for C3 at other versions and the same ECC level. Every version has a different n, so a different Z: the shifts wrap differently (shift mod Z), the checks connect different bits, and the interleaver scatters them differently.</figcaption>
    </figure>
  </div>
</main>
</body></html>
"#,
        ver = VERSION,
        p = n - k,
        rate = k as f32 / n as f32,
        checks = code.checks(),
        mc = members.len(),
        wrong_pct = wrong_conf * 100.0,
        right_pct = right_conf * 100.0,
        ring_pct = (n - layout.payload.len()) as f32 * 100.0 / n as f32,
        side = layout.side,
        grid_bits = layout.payload.len(),
        ring_bits = n - layout.payload.len(),
        nominal = lc_core::ecc::ldpc::RATES[rate_index],
        nz = n_b * z,
        zf = n as f32 / n_b as f32,
        short = code.shortened_bits(),
    );
    std::fs::write(format!("{dir}/anatomy-ldpc-c3.html"), html).unwrap();
    println!(
        "ldpc anatomy: v{VERSION} C3 luma n={n} k={k} checks={} — capture blur {radius} σ {sigma}: {first_wrong} wrong, converged in {iterations} -> {dir}/anatomy-ldpc-c3.html",
        code.checks()
    );
}

// The two capture impairments, as the decoder's test harness defines them
// (`lc-decode/tests/support/ops.rs`), so the damage here means the same as in a sweep.

/// Deterministic xorshift, so the capture is reproducible from its seed.
struct Rng(u64);

impl Rng {
    fn unit(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f32 / (1u64 << 53) as f32
    }
    fn normal(&mut self) -> f32 {
        let u1 = self.unit().max(1e-9);
        (-2.0 * u1.ln()).sqrt() * (std::f32::consts::TAU * self.unit()).cos()
    }
}

fn srgb_to_linear(v: u8) -> f32 {
    lc_core::color::srgb_to_linear_f(v as f32 / 255.0)
}

fn linear_to_srgb(v: f32) -> u8 {
    (lc_core::color::linear_to_srgb_f(v.clamp(0.0, 1.0)) * 255.0).round() as u8
}

/// Additive per-channel sensor noise, σ in linear light.
fn noise(r: &Raster, sigma: f32, seed: u64) -> Raster {
    let mut rng = Rng(seed | 1);
    let mut out = r.clone();
    for (o, &v) in out.data.iter_mut().zip(&r.data) {
        *o = linear_to_srgb(srgb_to_linear(v) + sigma * rng.normal());
    }
    out
}

/// Box blur in linear light, approximating defocus. `radius` is in pixels.
fn blur(r: &Raster, radius: u32) -> Raster {
    let mut out = r.clone();
    let rad = radius as i32;
    let n = (2 * radius + 1) as f32;
    for y in 0..r.height as i32 {
        for x in 0..r.width as i32 {
            let mut acc = [0f32; 3];
            for dy in -rad..=rad {
                for dx in -rad..=rad {
                    let sx = (x + dx).clamp(0, r.width as i32 - 1);
                    let sy = (y + dy).clamp(0, r.height as i32 - 1);
                    let i = ((sy * r.width as i32 + sx) * 3) as usize;
                    for (c, a) in acc.iter_mut().enumerate() {
                        *a += srgb_to_linear(r.data[i + c]);
                    }
                }
            }
            let i = ((y * r.width as i32 + x) * 3) as usize;
            for (c, &a) in acc.iter().enumerate() {
                out.data[i + c] = linear_to_srgb(a / (n * n));
            }
        }
    }
    out
}
