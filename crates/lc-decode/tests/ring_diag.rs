//! Stage-by-stage trace of the finder-less ring detector (D17) on the cases it fails.
//!
//! Diagnostic, not a regression test: `#[ignore]`d, and it prints rather than asserts. Run
//!
//! ```text
//! cargo test -p lc-decode --release --test ring_diag -- --ignored --nocapture
//! ```
//!
//! Built on `ring::trace`, which `ring::detect_ring` is itself built on, so what this shows
//! is what detection actually decided from.

#![allow(dead_code)]

use lc_core::{Compression, Packet, Profile};
use lc_decode::image::{ImageView, PixelFormat};
use lc_decode::{Decoder, ring};
use lc_encode::EncodeOptions;
use lc_render::{Raster, RenderOptions, render_rgb};

include!("support/ops.rs");

/// The degradation suite's payload, so these traces explain *its* failures.
fn payload() -> Vec<u8> {
    (0..96u8)
        .map(|i| i.wrapping_mul(101).wrapping_add(29))
        .collect()
}

fn symbol() -> lc_core::Symbol {
    let opts = EncodeOptions {
        profile: Profile::C1,
        reference_border: true,
        compression: Some(Compression::None),
        ..Default::default()
    };
    lc_encode::encode_bytes(&payload(), &opts).unwrap()
}

fn render_at(sym: &lc_core::Symbol, px: u32) -> Raster {
    render_rgb(
        sym,
        &RenderOptions {
            module_px: px,
            ..RenderOptions::screen()
        },
    )
}

/// Pad white on the top-left: same pitch, different phase against the binariser's grid.
fn shifted(r: &Raster, off: u32) -> Raster {
    let (w, h) = (r.width + off, r.height + off);
    let mut data = vec![255u8; (w * h * 3) as usize];
    for y in 0..r.height {
        let s = (y * r.width * 3) as usize;
        let d = (((y + off) * w + off) * 3) as usize;
        let n = (r.width * 3) as usize;
        data[d..d + n].copy_from_slice(&r.data[s..s + n]);
    }
    Raster {
        width: w,
        height: h,
        data,
    }
}

fn side_by_side(a: &Raster, b: &Raster, gap: u32) -> Raster {
    let (w, h) = (a.width + gap + b.width, a.height.max(b.height));
    let mut data = vec![255u8; (w * h * 3) as usize];
    for (src, x0) in [(a, 0), (b, a.width + gap)] {
        for y in 0..src.height {
            let s = (y * src.width * 3) as usize;
            let d = ((y * w + x0) * 3) as usize;
            let n = (src.width * 3) as usize;
            data[d..d + n].copy_from_slice(&src.data[s..s + n]);
        }
    }
    Raster {
        width: w,
        height: h,
        data,
    }
}

/// Ring outer edges of a clean render: canvas margin is 6 modules and the ring's outer layer
/// sits at shell 4, so its outer edge is 2 modules in from the canvas edge on every side.
fn true_ring(side: u16, px: u32, off: u32) -> ((f32, f32), (f32, f32)) {
    let lo = 2.0 * px as f32 + off as f32;
    let hi = (side as f32 + 10.0) * px as f32 + off as f32;
    ((lo, lo), (hi, hi))
}

fn report(name: &str, r: &Raster, truth: Option<((f32, f32), (f32, f32))>, true_v: u8) {
    let view = ImageView::new(&r.data, r.width, r.height, r.width * 3, PixelFormat::Rgb8).unwrap();
    let gray = view.gray_buffer();
    let t = ring::trace(&gray, r.width, r.height);

    let quad = t.quad.map_or("NO QUAD".to_string(), |q| {
        q.iter()
            .map(|p| format!("({:.0},{:.0})", p.0, p.1))
            .collect::<Vec<_>>()
            .join(" ")
    });
    let truth = truth.map_or(String::new(), |((a, b), (c, d))| {
        format!(" | true edge ({a:.0},{b:.0})..({c:.0},{d:.0})")
    });
    println!(
        "== {name} [{}x{}]  regions {} hull {} quad {quad}{truth}",
        r.width, r.height, t.regions, t.hull_len
    );

    let mut ranked = t.scores.clone();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    let top: Vec<String> = ranked
        .iter()
        .take(3)
        .map(|(v, a, _, m)| format!("v{}{} {a:.3}", v.get(), if *m { "m" } else { "" }))
        .collect();
    let true_score = t
        .scores
        .iter()
        .find(|s| s.0.get() == true_v)
        .map_or("untried".to_string(), |s| format!("{:.3}", s.1));

    let stage = match &t.fix {
        None => "no fix".to_string(),
        Some(f) => match ring::read_ring_format(&view, &f.transform, f.version) {
            Ok((info, m)) => format!(
                "fix v{} -> format {:?} margin {m:.1}",
                f.version.get(),
                info.profile
            ),
            Err(e) => format!("fix v{} -> format ERR {e}", f.version.get()),
        },
    };
    let decode = match Decoder::new().decode_rgb(&r.data, r.width, r.height, r.width * 3) {
        Ok(o) => match o.packet {
            Packet::Unsigned { ref body, .. } if *body == payload() => "OK".to_string(),
            p => format!("WRONG {p:?}"),
        },
        Err(e) => format!("ERR {e}"),
    };
    println!(
        "   top {} | true v{true_v} {true_score} | {stage} | decode {decode}",
        top.join("  ")
    );
}

#[test]
#[ignore]
fn trace_failing_cases() {
    let sym = symbol();
    let (side, v) = (sym.side(), sym.format.version.get());
    println!("symbol: v{v} side {side}");

    for (px, off) in [
        (12, 0),
        (8, 0),
        (8, 1),
        (20, 0),
        (20, 1),
        (20, 2),
        (64, 0),
        (80, 0),
    ] {
        let r = shifted(&render_at(&sym, px), off);
        report(
            &format!("clean px{px} off{off}"),
            &r,
            Some(true_ring(side, px, off)),
            v,
        );
    }

    let base = render_at(&sym, 12);
    for radius in [3u32, 4] {
        report(
            &format!("blur r={radius}"),
            &blur(&base, radius),
            Some(true_ring(side, 12, 0)),
            v,
        );
    }
    for seed in 1..=12u64 {
        let r = noise(&base, 0.10, seed * 7919);
        report(
            &format!("noise 0.10 seed {}", seed * 7919),
            &r,
            Some(true_ring(side, 12, 0)),
            v,
        );
    }

    let r = perspective(&base, 0.15, 12.0);
    let r = shading(&r, 0.25);
    let r = color_cast(&r, [1.0, 0.88, 0.62]);
    let r = specular(&r, 0.7, 0.35, 0.15, 0.35);
    let r = chroma_subsample(&r);
    let r = noise(&r, 0.03, 0xBEEF);
    report("realistic combination", &r, None, v);

    report(
        "two symbols side by side",
        &side_by_side(&base, &base, 96),
        None,
        v,
    );
}

fn decodes(r: &Raster) -> bool {
    matches!(
        Decoder::new().decode_rgb(&r.data, r.width, r.height, r.width * 3),
        Ok(o) if matches!(&o.packet, Packet::Unsigned { body, .. } if *body == payload())
    )
}

/// Where the single-frame noise limit sits, and whether twelve accumulated frames still beat
/// it — the degradation suite's multi-frame test needs a σ between the two.
#[test]
#[ignore]
fn noise_limits() {
    let base = render_at(&symbol(), 12);
    for sigma in [0.35f32, 0.36, 0.37, 0.38, 0.39, 0.40] {
        let single = (1..=6u64)
            .filter(|&s| decodes(&noise(&base, sigma, s)))
            .count();
        let mut acc = lc_decode::FrameAccumulator::new();
        let mut pushed = 0;
        for seed in 1..=12u64 {
            let f = noise(&base, sigma, seed * 7919);
            if acc
                .push_rgb(&f.data, f.width, f.height, f.width * 3)
                .is_ok()
            {
                pushed += 1;
            }
        }
        let multi = matches!(
            acc.try_decode(&Decoder::new()),
            Ok(o) if matches!(&o.packet, Packet::Unsigned { body, .. } if *body == payload())
        );
        println!("σ={sigma:.2}: single {single}/6 | multi located {pushed}/12, decodes {multi}");
    }
}

fn accumulate(frames: impl Iterator<Item = Raster>) -> (usize, bool) {
    let mut acc = lc_decode::FrameAccumulator::new();
    let mut pushed = 0;
    for f in frames {
        if acc
            .push_rgb(&f.data, f.width, f.height, f.width * 3)
            .is_ok()
        {
            pushed += 1;
        }
    }
    let ok = matches!(
        acc.try_decode(&Decoder::new()),
        Ok(o) if matches!(&o.packet, Packet::Unsigned { body, .. } if *body == payload())
    );
    (pushed, ok)
}

/// Candidate impairments for the multi-frame test, now the ring has moved the single-frame
/// noise limit from σ ≈ 0.06 to ≈ 0.36.
#[test]
#[ignore]
fn multi_frame_designs() {
    let ring = render_at(&symbol(), 12);
    let classic_sym = lc_encode::encode_bytes(
        &payload(),
        &EncodeOptions {
            profile: Profile::C1,
            reference_border: false,
            compression: Some(Compression::None),
            ..Default::default()
        },
    )
    .unwrap();
    let classic = render_at(&classic_sym, 12);

    for (name, base, sigma) in [
        ("classic", &classic, 0.10f32),
        ("classic", &classic, 0.14),
        ("ring", &ring, 0.37),
        ("ring", &ring, 0.38),
    ] {
        let single = (1..=12u64)
            .filter(|&s| decodes(&noise(base, sigma, s)))
            .count();
        let (pushed, multi) = accumulate((1..=12u64).map(|k| noise(base, sigma, k * 7919)));
        println!(
            "{name} noise σ={sigma:.2}: single {single}/12 | multi located {pushed}/12, decodes {multi}"
        );
    }

    for radius in [0.2f32, 0.3] {
        for strength in [0.6f32, 0.8, 1.0] {
            let frame = |k: u64| {
                let a = k as f32 * 2.399;
                specular(
                    &ring,
                    0.5 + 0.25 * a.cos(),
                    0.5 + 0.25 * a.sin(),
                    radius,
                    strength,
                )
            };
            let single = (1..=12u64).filter(|&k| decodes(&frame(k))).count();
            let (pushed, multi) = accumulate((1..=12u64).map(frame));
            println!(
                "ring moving glare r={radius} s={strength}: single {single}/12 | multi located {pushed}/12, decodes {multi}"
            );
        }
    }
}

/// Where defocus and moving glare now bite, after D18 narrowed the gap between ring and
/// grid. Blur radius is in pixels, so what matters is its size *in modules*.
#[test]
#[ignore]
fn defocus_and_glare_envelope() {
    let sym = symbol();
    for px in [12u32, 14, 16, 20] {
        let base = render_at(&sym, px);
        let row: Vec<String> = [2u32, 3, 4, 5, 6]
            .iter()
            .map(|&r| {
                let ok = decodes(&blur(&base, r));
                format!(
                    "r={r} ({:.2} mod) {}",
                    r as f32 / px as f32,
                    if ok { "ok" } else { "--" }
                )
            })
            .collect();
        println!("defocus px={px}: {}", row.join(" | "));
    }

    let base = render_at(&sym, 12);
    for radius in [0.3f32, 0.35, 0.4] {
        for strength in [1.0f32, 1.2] {
            let frame = |k: u64| {
                let a = k as f32 * 2.399;
                specular(
                    &base,
                    0.5 + 0.25 * a.cos(),
                    0.5 + 0.25 * a.sin(),
                    radius,
                    strength,
                )
            };
            let single = (1..=12u64).filter(|&k| decodes(&frame(k))).count();
            let (pushed, multi) = accumulate((1..=12u64).map(frame));
            println!(
                "glare r={radius} s={strength}: single {single}/12 | multi located {pushed}/12, decodes {multi}"
            );
        }
    }
}

/// What the ring's unused colour slots are actually worth, before building anything.
///
/// Every non-reference ring position carries `log2(hues)` bits of free choice; the format
/// codeword uses 60 of those slots. The rest are spare. This prints what adding them to the
/// chroma layer would buy in real information bits, through the same LDPC code the layer
/// already uses.
#[test]
#[ignore]
fn ring_spare_capacity() {
    use lc_core::ecc::ldpc::{CHROMA_RATE_INDEX, Code};
    use lc_core::layout::Layout;
    use lc_core::{EccLevel, Version};

    let ecc = EccLevel::DEFAULT;
    for profile in [Profile::C1, Profile::C3] {
        let hues = lc_core::Palette::for_profile(profile).hues;
        let bpp = lc_core::border::bits_per_position(hues);
        println!("\n{profile:?}: {hues} hues, {bpp} format bit(s) per ring position");
        println!(
            "{:>4} {:>5} {:>7} {:>7} {:>7} {:>8} {:>9} {:>8} {:>7}",
            "v", "side", "ring", "slots", "spare", "grid ch", "ch bits+", "body B", "body+"
        );
        for v in [1u8, 2, 3, 5, 10, 20, 32] {
            let version = Version::new(v).unwrap();
            let side = version.side();
            let layout = Layout::for_symbol(version, profile, true);
            let cap = layout.capacity();
            let slots = lc_core::border::data_positions(side) * bpp;
            let spare = slots.saturating_sub(60);
            let rate = CHROMA_RATE_INDEX[ecc.chroma as usize & 3];
            let info = |bits: usize| Code::new(rate, bits).map(|c| c.info_bits()).unwrap_or(0);
            let (now, with) = (info(cap.chroma_bits), info(cap.chroma_bits + spare));
            let body =
                lc_core::capacity::capacity_for_symbol(version, profile, ecc, true).body_bytes;
            let body_with = lc_core::capacity::body_bytes_for(
                lc_core::capacity::capacity_for_symbol(version, profile, ecc, true).luma_info_bits
                    + with,
            );
            println!(
                "{v:>4} {side:>5} {:>7} {slots:>7} {spare:>7} {:>8} {:>9} {body:>8} {:>+7}",
                lc_core::border::positions(side),
                cap.chroma_bits,
                with as i64 - now as i64,
                body_with as i64 - body as i64,
            );
        }
    }
}

/// What the ring's *inner* layer would be worth as ordinary payload.
///
/// Both radial layers of a ring position currently carry the same value (P12), which is what
/// gives the detector a 2-module-deep sampling target. If the inner layer became data, each
/// of its cells could carry a luma bit and the profile's chroma bits, like any grid module.
#[test]
#[ignore]
fn inner_layer_capacity() {
    use lc_core::ecc::ldpc::{CHROMA_RATE_INDEX, Code, LUMA_RATE_INDEX};
    use lc_core::layout::Layout;
    use lc_core::{EccLevel, Version};

    let ecc = EccLevel::DEFAULT;
    for profile in [Profile::C1, Profile::C3] {
        let bpc = profile.chroma_bits_per_cell();
        println!("\n{profile:?}: 1 luma + {bpc} chroma bits per inner cell");
        println!(
            "{:>4} {:>5} {:>7} {:>9} {:>9} {:>8} {:>8} {:>7}",
            "v", "side", "free", "grid luma", "grid chr", "body B", "with", "gain"
        );
        for v in [1u8, 2, 3, 5, 10, 20, 32] {
            let version = Version::new(v).unwrap();
            let side = version.side();
            let layout = Layout::for_symbol(version, profile, true);
            let cap = layout.capacity();
            // Inner layer: the square one step in from the ring's outer square.
            let all_inner = 4 * (side as usize + 4) - 4;
            // The hybrid: reference positions and corners stay 2-deep, so only a data
            // position's inner cell is free.
            let inner = (0..lc_core::border::positions(side))
                .filter(|&p| {
                    let (ix, iy) = lc_core::border::inner_cell(p, side);
                    !lc_core::border::is_reference(p)
                        && lc_core::border::position(ix, iy, side) == Some(p)
                })
                .count();
            let _ = all_inner;
            let info = |idx: [usize; 4], bits: usize| {
                Code::new(idx[ecc.luma as usize & 3], bits)
                    .map(|c| c.info_bits())
                    .unwrap_or(0)
            };
            let now =
                info(LUMA_RATE_INDEX, cap.luma_bits) + info(CHROMA_RATE_INDEX, cap.chroma_bits);
            let with = info(LUMA_RATE_INDEX, cap.luma_bits + inner)
                + info(CHROMA_RATE_INDEX, cap.chroma_bits + inner * bpc);
            let (b0, b1) = (
                lc_core::capacity::body_bytes_for(now),
                lc_core::capacity::body_bytes_for(with),
            );
            println!(
                "{v:>4} {side:>5} {inner:>7} {:>9} {:>9} {b0:>8} {b1:>8} {:>6.1}%",
                cap.luma_bits,
                cap.chroma_bits,
                100.0 * (b1 as f64 - b0 as f64) / b0 as f64
            );
        }
    }
}

/// Repaint the inner cell of every data position with an unrelated palette colour: what the
/// ring would look like if those cells carried payload, with reference positions left 2-deep.
fn repaint_inner(r: &Raster, sym: &lc_core::Symbol, px: u32) -> Raster {
    let side = sym.side();
    let margin = lc_render::Canvas::of(sym).margin as i32;
    let palette = sym.palette();
    let mut out = r.clone();
    let mut rng = 0x1234_5678u64;
    for p in 0..lc_core::border::positions(side) {
        if lc_core::border::is_reference(p) {
            continue;
        }
        let (ix, iy) = lc_core::border::inner_cell(p, side);
        if lc_core::border::position(ix, iy, side) != Some(p) {
            continue;
        }
        rng = rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let c = palette.colors[(rng >> 33) as usize % palette.colors.len()];
        let (x0, y0) = (((ix + margin) as u32) * px, ((iy + margin) as u32) * px);
        for y in y0..y0 + px {
            for x in x0..x0 + px {
                let i = ((y * out.width + x) * 3) as usize;
                out.data[i] = c.r;
                out.data[i + 1] = c.g;
                out.data[i + 2] = c.b;
            }
        }
    }
    out
}

/// Can the ring give up the inner layer of its data positions?
#[test]
#[ignore]
fn inner_layer_as_payload() {
    let sym = symbol();
    let v = sym.format.version.get();
    let base = repaint_inner(&render_at(&sym, 12), &sym, 12);
    report("inner-as-payload clean", &base, None, v);
    report("inner-as-payload blur r=3", &blur(&base, 3), None, v);
    report("inner-as-payload blur r=4", &blur(&base, 4), None, v);
    report(
        "inner-as-payload noise 0.10",
        &noise(&base, 0.10, 7919),
        None,
        v,
    );
    report(
        "inner-as-payload noise 0.20",
        &noise(&base, 0.20, 7919),
        None,
        v,
    );
    let r = perspective(&base, 0.15, 12.0);
    let r = shading(&r, 0.25);
    let r = color_cast(&r, [1.0, 0.88, 0.62]);
    let r = specular(&r, 0.7, 0.35, 0.15, 0.35);
    let r = chroma_subsample(&r);
    let r = noise(&r, 0.03, 0xBEEF);
    report("inner-as-payload realistic", &r, None, v);
}
