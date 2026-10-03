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

use lc_core::{Compression, Packet, Profile, Version};
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
    let t = ring::trace(&gray, r.width, r.height, Version::MAX);

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

/// What deferring binarisation behind the ring path actually saves per frame.
///
/// The ring detector reads greyscale; only the finder fallback needs the binary. This times
/// a full decode of a ringed symbol (ring path, no binarisation) against the binarise-and-
/// denoise pass it used to pay for regardless.
#[test]
#[ignore]
fn decode_cost_by_path() {
    use lc_decode::image::binarize;
    use std::time::Instant;

    let sym = symbol();
    let r = render_at(&sym, 12);
    let view = ImageView::new(&r.data, r.width, r.height, r.width * 3, PixelFormat::Rgb8).unwrap();
    let gray = view.gray_buffer();

    const N: u32 = 20;
    let t0 = Instant::now();
    for _ in 0..N {
        let _ = binarize(&gray, r.width, r.height).denoised();
    }
    let binarise = t0.elapsed() / N;

    let t0 = Instant::now();
    for _ in 0..N {
        Decoder::new()
            .decode_rgb(&r.data, r.width, r.height, r.width * 3)
            .expect("decodes");
    }
    let decode = t0.elapsed() / N;

    println!(
        "{}x{}: decode {decode:?}, binarise+denoise {binarise:?} ({:.1}% of a decode, now skipped on the ring path)",
        r.width,
        r.height,
        100.0 * binarise.as_secs_f64() / decode.as_secs_f64()
    );
}

/// Numbers the README and the explainer quote, recomputed.
///
/// Both documents predate D17–D19 and quote the classic layout's capacity, which is now
/// only what a print or border-less symbol carries. This prints the ringed figures beside
/// them, and the QR side measured with `qrcodegen` rather than transcribed.
#[test]
#[ignore]
fn doc_numbers() {
    use lc_core::capacity::{capacity_for_symbol, smallest_version_for_symbol};
    use lc_core::{EccLevel, Version};
    use lc_render::qr::{QrEcc, qr_encode_bytes};

    let ecc = EccLevel::DEFAULT;
    let profiles = [Profile::M, Profile::C1, Profile::C2, Profile::C3];

    println!("\n== capacity at a fixed grid, ECC default (ring / classic) ==");
    for v in [1u8, 2, 3, 4] {
        let version = Version::new(v).unwrap();
        print!("  v{v} {:>2}x{:<2} ", version.side(), version.side());
        for p in profiles {
            let ring = capacity_for_symbol(version, p, ecc, true).body_bytes;
            let classic = capacity_for_symbol(version, p, ecc, false).body_bytes;
            print!("| {p:?} {ring:>4} / {classic:<4} ");
        }
        println!();
    }

    println!("\n== QR byte capacity at the same grids ==");
    for (label, ecl) in [
        ("L", QrEcc::Low),
        ("M", QrEcc::Medium),
        ("Q", QrEcc::Quartile),
        ("H", QrEcc::High),
    ] {
        print!("  QR-{label} ");
        for maxv in [1u8, 2, 3, 4] {
            let mut best = 0;
            for n in 1..500usize {
                match qr_encode_bytes(&vec![0x41u8; n], ecl) {
                    Ok(q) if q.version <= maxv => best = n,
                    _ => break,
                }
            }
            let side = 17 + 4 * maxv as u32;
            print!("| v{maxv} {side}x{side} {best:>4} B ");
        }
        println!();
    }

    println!("\n== v4 33x33, ring on, by ECC level (parity share of each layer) ==");
    let rates = lc_core::ecc::ldpc::RATES;
    let (li, ci) = (
        lc_core::ecc::ldpc::LUMA_RATE_INDEX,
        lc_core::ecc::ldpc::CHROMA_RATE_INDEX,
    );
    let v4 = Version::new(4).unwrap();
    for level in 0..4u8 {
        let e = EccLevel {
            luma: level,
            chroma: level,
        };
        print!(
            "  level {level}: luma rate {:.2} ({:.0}% parity), chroma rate {:.2} ({:.0}% parity) ",
            rates[li[level as usize]],
            100.0 * (1.0 - rates[li[level as usize]]),
            rates[ci[level as usize]],
            100.0 * (1.0 - rates[ci[level as usize]])
        );
        for p in profiles {
            print!(
                "| {p:?} {:>4} B ",
                capacity_for_symbol(v4, p, e, true).body_bytes
            );
        }
        println!();
    }

    println!("\n== same payload: who needs what (LC ring on) ==");
    for n in [34usize, 50, 96, 144] {
        let data = vec![0x41u8; n];
        let lc = |p| {
            smallest_version_for_symbol(n, p, ecc, true)
                .map(|v| format!("v{} {}x{}", v.get(), v.side(), v.side()))
                .unwrap_or_else(|_| "—".into())
        };
        let qr = |e| {
            qr_encode_bytes(&data, e)
                .map(|q| format!("v{} {}x{}", q.version, q.size, q.size))
                .unwrap_or_else(|_| "—".into())
        };
        println!(
            "  {n:>3} B: C1 {:<12} C3 {:<12} | QR-L {:<12} QR-M {:<12} QR-Q {:<12} QR-H {}",
            lc(Profile::C1),
            lc(Profile::C3),
            qr(QrEcc::Low),
            qr(QrEcc::Medium),
            qr(QrEcc::Quartile),
            qr(QrEcc::High)
        );
    }
}

/// Does tilt tolerance still depend on which way you lean the symbol?
///
/// The explainer's §05 blamed an asymmetric layout: bullseyes at three corners and a small
/// alignment marker at the fourth, so leaning on the weak corner gave the decoder less to
/// work with. A finder-less symbol (D17) has four identical ring corners, so that argument
/// no longer applies — this measures whether the effect survived it.
#[test]
#[ignore]
fn tilt_by_azimuth() {
    let base = render_at(&symbol(), 12);
    for tilt in [0.15f32, 0.25, 0.35, 0.45, 0.55, 0.65, 0.75, 0.85] {
        let row: Vec<String> = (0..8)
            .map(|k| {
                let az = k as f32 * 45.0;
                let ok = decodes(&perspective(&base, tilt, az));
                format!("{az:>3.0}\u{b0} {}", if ok { "ok" } else { "--" })
            })
            .collect();
        println!("tilt {tilt:.2}: {}", row.join(" | "));
    }
}

/// The biggest symbol the format can actually carry — encoded, rendered and decoded.
///
/// Capacity tables say what fits; this says what comes back. Timed, because a v32 symbol is
/// 145 modules across and the decode cost scales with pixels, not modules.
#[test]
#[ignore]
fn largest_symbol() {
    use lc_core::capacity::capacity_for_symbol;
    use lc_core::{EccLevel, Version};
    use std::time::Instant;

    let v32 = Version::new(32).unwrap();
    println!(
        "\nv32: {} x {} modules, {} in the grid",
        v32.side(),
        v32.side(),
        v32.side() as usize * v32.side() as usize
    );
    for profile in [Profile::C1, Profile::C3] {
        let layout = lc_core::layout::Layout::for_symbol(v32, profile, true);
        let cap = layout.capacity();
        println!(
            "  {profile:?}: {} luma + {} chroma = {} raw channel bits ({} ring cells)",
            cap.luma_bits,
            cap.chroma_bits,
            cap.luma_bits + cap.chroma_bits,
            layout.ring_cells
        );
    }
    for profile in [Profile::C1, Profile::C3] {
        for ecc in [0u8, 1] {
            let e = EccLevel {
                luma: ecc,
                chroma: ecc,
            };
            let bytes = capacity_for_symbol(v32, profile, e, true).body_bytes;
            let payload: Vec<u8> = (0..bytes).map(|i| (i * 31 + 7) as u8).collect();
            let opts = EncodeOptions {
                profile,
                version: Some(v32),
                reference_border: true,
                compression: Some(Compression::None),
                ecc: e,
                ..Default::default()
            };
            let t0 = Instant::now();
            let sym = match lc_encode::encode_bytes(&payload, &opts) {
                Ok(s) => s,
                Err(e) => {
                    println!("  {profile:?} ecc {ecc}: {bytes} B did not encode: {e}");
                    continue;
                }
            };
            let enc = t0.elapsed();
            for px in [8u32, 12] {
                let r = render_at(&sym, px);
                let t0 = Instant::now();
                let got = Decoder::new()
                    .decode_rgb(&r.data, r.width, r.height, r.width * 3)
                    .map(|o| matches!(&o.packet, Packet::Unsigned { body, .. } if *body == payload))
                    .unwrap_or(false);
                println!(
                    "  {profile:?} ecc {ecc}: {bytes:>5} B | {px:>2} px/module -> {}x{} px | encode {enc:>8.1?} decode {:>8.1?} | {}",
                    r.width,
                    r.height,
                    t0.elapsed(),
                    if got { "decodes" } else { "FAILED" }
                );
            }
        }
    }
}

/// How big a *document* one symbol can carry, once compression is allowed.
///
/// The raw ceiling is v32's capacity; the useful ceiling is how much source text fits
/// through §9.4's compression first. Random bytes are the worst case and JSON the realistic
/// one, so both are measured rather than assumed.
#[test]
#[ignore]
fn how_much_can_one_symbol_carry() {
    use lc_core::Version;

    // A repetitive JSON document, the shape a big payload actually takes.
    let json = |n: usize| -> Vec<u8> {
        let mut s = String::from("{\"records\":[");
        let mut i = 0;
        while s.len() < n {
            if i > 0 {
                s.push(',');
            }
            s.push_str(&format!(
                "{{\"id\":{i},\"name\":\"record {i}\",\"tags\":[\"alpha\",\"beta\"],\"score\":{}.{}}}",
                i % 97,
                i % 10
            ));
            i += 1;
        }
        s.push_str("]}");
        s.into_bytes()
    };
    let random = |n: usize| -> Vec<u8> {
        let mut x = 0x2545_F491_4F6C_DD1Du64;
        (0..n)
            .map(|_| {
                x ^= x >> 12;
                x ^= x << 25;
                x ^= x >> 27;
                (x >> 33) as u8
            })
            .collect()
    };

    for (label, make) in [
        ("JSON", &json as &dyn Fn(usize) -> Vec<u8>),
        ("random", &random),
    ] {
        for kb in [4usize, 8, 16, 32, 50] {
            let data = make(kb * 1024);
            let opts = EncodeOptions {
                profile: Profile::C3,
                reference_border: true,
                ecc: lc_core::EccLevel { luma: 0, chroma: 0 },
                ..Default::default() // compression: auto
            };
            match lc_encode::encode_bytes(&data, &opts) {
                Ok(sym) => {
                    let v = sym.format.version;
                    let px = 8;
                    let r = render_at(&sym, px);
                    let ok = Decoder::new()
                        .decode_rgb(&r.data, r.width, r.height, r.width * 3)
                        .map(
                            |o| matches!(&o.packet, Packet::Unsigned { body, .. } if *body == data),
                        )
                        .unwrap_or(false);
                    println!(
                        "  {label:>6} {kb:>3} KB -> v{:<2} {}x{} modules, {}x{} px at {px}/module | {}",
                        v.get(),
                        v.side(),
                        v.side(),
                        r.width,
                        r.height,
                        if ok { "decodes" } else { "FAILED" }
                    );
                }
                Err(e) => println!("  {label:>6} {kb:>3} KB -> does not fit any version ({e})"),
            }
        }
    }
    let v32 = Version::new(32).unwrap();
    println!(
        "\n  the cap is the format word: version is 5 bits, so v{} ({}x{}) is the largest that exists",
        v32.get(),
        v32.side(),
        v32.side()
    );
}

/// The exact ceiling, both layouts: the largest JSON document that fits, found by search.
#[test]
#[ignore]
fn the_real_ceiling() {
    use lc_core::Version;

    let json = |n: usize| -> Vec<u8> {
        let mut s = String::from("{\"records\":[");
        let mut i = 0;
        while s.len() < n {
            if i > 0 {
                s.push(',');
            }
            s.push_str(&format!(
                "{{\"id\":{i},\"name\":\"record {i}\",\"tags\":[\"alpha\",\"beta\"],\"score\":{}.{}}}",
                i % 97,
                i % 10
            ));
            i += 1;
        }
        s.push_str("]}");
        s.into_bytes()
    };

    for border in [true, false] {
        let layout = if border {
            "ring, finder-less"
        } else {
            "classic, 3 finders"
        };
        let opts = |ecc| EncodeOptions {
            profile: Profile::C3,
            reference_border: border,
            ecc,
            ..Default::default()
        };
        let e0 = lc_core::EccLevel { luma: 0, chroma: 0 };

        // Raw ceiling: incompressible bytes at v32.
        let raw = lc_core::capacity::capacity_for_symbol(
            Version::new(32).unwrap(),
            Profile::C3,
            e0,
            border,
        )
        .body_bytes;

        // Source ceiling: binary search the largest JSON that still encodes.
        let (mut lo, mut hi) = (1024usize, 200 * 1024);
        while lo + 256 < hi {
            let mid = (lo + hi) / 2;
            if lc_encode::encode_bytes(&json(mid), &opts(e0)).is_ok() {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let data = json(lo);
        let sym = lc_encode::encode_bytes(&data, &opts(e0)).expect("the found size encodes");
        let r = render_at(&sym, 8);
        let ok = Decoder::new()
            .decode_rgb(&r.data, r.width, r.height, r.width * 3)
            .map(|o| matches!(&o.packet, Packet::Unsigned { body, .. } if *body == data))
            .unwrap_or(false);
        println!(
            "  {layout:<20} raw {raw:>5} B | JSON {:>6} B ({:.1} KB, {:.1}x compression) -> v{} {}x{} | {}",
            data.len(),
            data.len() as f32 / 1024.0,
            data.len() as f32 / raw as f32,
            sym.format.version.get(),
            sym.side(),
            sym.side(),
            if ok { "decodes" } else { "FAILED" }
        );
    }
}

/// What actually stops a 50 KB payload: the container's length field, not the symbol.
///
/// `Frame` checks the **uncompressed** body against `EXTENDED_MAX` (16383) before it
/// compresses anything, so a payload that would compress to a few hundred bytes is rejected
/// for being long, not for being big. This measures both sides of that.
#[test]
#[ignore]
fn what_blocks_fifty_kilobytes() {
    use lc_core::frame::{Compression as C, EXTENDED_MAX, compress};

    // Repetitive JSON, the shape the photographed test payload had.
    let doc = |n: usize| -> String {
        let mut s = String::from("{\"records\":[");
        let mut i = 0;
        while s.len() < n {
            if i > 0 {
                s.push(',');
            }
            s.push_str(&format!(
                "{{\"id\":{i},\"name\":\"record {i}\",\"note\":\"the quick brown fox\",\"ok\":true}}"
            ));
            i += 1;
        }
        s.push_str("]}");
        s
    };

    println!(
        "\n  container limit: EXTENDED_MAX = {EXTENDED_MAX} B, checked on the *uncompressed* body"
    );
    for kb in [8usize, 16, 32, 50, 64] {
        let json = doc(kb * 1024);
        let cbor = lc_core::json::parse(&json).unwrap().to_vec();
        let deflate = compress(C::Deflate, &cbor).map(|v| v.len()).unwrap_or(0);
        let brotli = compress(C::Brotli, &cbor).map(|v| v.len()).unwrap_or(0);
        let best = deflate.min(if brotli > 0 { brotli } else { usize::MAX });
        // Would the compressed form fit a symbol, if the container allowed the body?
        let fits_v32 = best
            <= lc_core::capacity::capacity_for_symbol(
                lc_core::Version::new(32).unwrap(),
                Profile::C3,
                lc_core::EccLevel { luma: 0, chroma: 0 },
                true,
            )
            .body_bytes;
        println!(
            "  {kb:>3} KB JSON -> {:>6} B CBOR -> {:>5} B compressed ({:.1}x) | container: {} | fits v32 once framed: {}",
            cbor.len(),
            best,
            json.len() as f32 / best as f32,
            if cbor.len() <= EXTENDED_MAX {
                "ok"
            } else {
                "REJECTED"
            },
            if fits_v32 { "yes" } else { "no" }
        );
    }
}

/// What compression actually buys, by payload shape — because the synthetic figure lied.
///
/// The first generator here repeated one literal string thousands of times, which DEFLATE
/// reduces ~15x. No real payload does that. These are the shapes a real one takes, from
/// most compressible to least.
#[test]
#[ignore]
fn compression_reality_check() {
    use lc_core::frame::{Compression as C, compress};

    fn rnd(state: &mut u64) -> u64 {
        *state ^= *state >> 12;
        *state ^= *state << 25;
        *state ^= *state >> 27;
        state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    const WORDS: [&str; 18] = [
        "alpha", "bravo", "delta", "echo", "foxtrot", "golf", "hotel", "india", "juliet", "kilo",
        "lima", "mike", "north", "oscar", "papa", "quebec", "romeo", "sierra",
    ];

    /// What the earlier probe generated: one literal, repeated.
    fn repetitive(n: usize) -> String {
        let mut s = String::from("{\"records\":[");
        let mut i = 0;
        while s.len() < n {
            if i > 0 {
                s.push(',');
            }
            s.push_str(&format!(
                "{{\"id\":{i},\"name\":\"record {i}\",\"note\":\"the quick brown fox\",\"ok\":true}}"
            ));
            i += 1;
        }
        s.push_str("]}");
        s
    }

    /// The same schema, with values that actually vary.
    fn varied(n: usize) -> String {
        let mut st = 0x1234_5678_9ABC_DEF0u64;
        let mut s = String::from("{\"records\":[");
        let mut first = true;
        while s.len() < n {
            if !first {
                s.push(',');
            }
            first = false;
            let x = rnd(&mut st);
            s.push_str(&format!(
                "{{\"id\":{},\"name\":\"{}-{}\",\"lat\":{}.{},\"lon\":-{}.{},\"tag\":\"{}\"}}",
                x % 1_000_000,
                WORDS[(x >> 7) as usize % WORDS.len()],
                x % 9973,
                30 + x % 20,
                x % 1_000_000,
                80 + (x >> 11) % 40,
                (x >> 17) % 1_000_000,
                WORDS[(x >> 23) as usize % WORDS.len()]
            ));
        }
        s.push_str("]}");
        s
    }

    /// Natural-ish prose from a small vocabulary.
    fn prose(n: usize) -> String {
        let mut st = 0x0BAD_C0FF_EE12_3456u64;
        let mut s = String::from("{\"text\":\"");
        while s.len() < n {
            s.push_str(WORDS[rnd(&mut st) as usize % WORDS.len()]);
            s.push(' ');
        }
        s.push_str("\"}");
        s
    }

    /// Base64 of random bytes: a key, a certificate, a thumbnail.
    fn b64ish(n: usize) -> String {
        const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut st = 0xFEED_FACE_CAFE_D00Du64;
        let mut s = String::from("{\"blob\":\"");
        while s.len() < n {
            s.push(A[rnd(&mut st) as usize % 64] as char);
        }
        s.push_str("\"}");
        s
    }

    let cap = lc_core::capacity::capacity_for_symbol(
        lc_core::Version::new(32).unwrap(),
        Profile::C3,
        lc_core::EccLevel { luma: 0, chroma: 0 },
        true,
    )
    .body_bytes;
    println!("\n  50 KB of each shape. v32 C3 at the weakest ECC holds {cap} B.\n");
    println!("  shape                         CBOR  deflate   brotli   ratio  fits v32");
    for (name, doc) in [
        ("one literal repeated", repetitive(50 * 1024)),
        ("varying field values", varied(50 * 1024)),
        ("prose, 18-word vocab", prose(50 * 1024)),
        ("base64 random bytes", b64ish(50 * 1024)),
    ] {
        let cbor = lc_core::json::parse(&doc).unwrap().to_vec();
        let d = compress(C::Deflate, &cbor)
            .map(|v| v.len())
            .unwrap_or(usize::MAX);
        let b = compress(C::Brotli, &cbor)
            .map(|v| v.len())
            .unwrap_or(usize::MAX);
        let best = d.min(b);
        println!(
            "  {name:<26} {:>7} {:>8} {:>8} {:>6.1}x  {}",
            cbor.len(),
            d,
            b,
            doc.len() as f32 / best as f32,
            if best <= cap { "yes" } else { "NO" }
        );
    }
    println!("\n  the repetitive document, first 180 bytes — this is why it compressed 15x:");
    println!("  {}", &repetitive(50 * 1024)[..180]);

    // The honest ceiling: largest source of each shape that still encodes AND decodes.
    println!("\n  largest single symbol, by shape (C3, weakest ECC, ring):");
    for (name, make) in [
        ("one literal repeated", repetitive as fn(usize) -> String),
        ("varying field values", varied),
        ("prose, 18-word vocab", prose),
        ("base64 random bytes", b64ish),
    ] {
        let opts = EncodeOptions {
            profile: Profile::C3,
            reference_border: true,
            ecc: lc_core::EccLevel { luma: 0, chroma: 0 },
            ..Default::default()
        };
        let (mut lo, mut hi) = (512usize, 512 * 1024);
        while lo + 512 < hi {
            let mid = (lo + hi) / 2;
            if lc_encode::encode_json(&make(mid), &opts).is_ok() {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let doc = make(lo);
        match lc_encode::encode_json(&doc, &opts) {
            Ok(sym) => {
                let r = render_at(&sym, 8);
                let ok = Decoder::new()
                    .decode_rgb(&r.data, r.width, r.height, r.width * 3)
                    .is_ok();
                println!(
                    "  {name:<26} {:>6} B ({:>5.1} KB) -> v{} {}x{} at 8px = {}x{} px | {}",
                    doc.len(),
                    doc.len() as f32 / 1024.0,
                    sym.format.version.get(),
                    sym.side(),
                    sym.side(),
                    r.width,
                    r.height,
                    if ok { "decodes" } else { "FAILED" }
                );
            }
            Err(e) => println!("  {name:<26} nothing fit ({e})"),
        }
    }
}
