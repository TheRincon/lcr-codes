//! How much the wider version range costs per frame.
//!
//! The §6.4 ring detector correlates a candidate hull against **every** version's
//! sequence and geometry (`ring.rs`, `for version in Version::all()`), so raising the
//! ceiling from 32 to 64 doubles that loop. On a phone running it per preview frame that
//! is the change most likely to be felt, so it gets measured rather than assumed.
//!
//! Run with `cargo test -p lcr-decode --release --test detect_cost -- --nocapture`.

use lcr_core::{Compression, EccLevel, Profile, Version};
use lcr_decode::Decoder;
use lcr_decode::image::{ImageView, PixelFormat};
use lcr_encode::EncodeOptions;
use lcr_render::{RenderOptions, render_rgb};
use std::cell::RefCell;
use std::time::Instant;

thread_local! {
    static MAX_V: RefCell<Version> = const { RefCell::new(Version::MAX) };
}

fn timed(v: u8, module_px: u32, n: u32) -> (std::time::Duration, std::time::Duration, u32, u32) {
    let version = Version::new(v).unwrap();
    let opts = EncodeOptions {
        profile: Profile::C3,
        version: Some(version),
        ecc: EccLevel::DEFAULT,
        compression: Some(Compression::None),
        reference_border: true,
        mask: None,
    };
    let cap = lcr_encode::capacity_for_symbol(version, Profile::C3, EccLevel::DEFAULT, true);
    let data: Vec<u8> = (0..cap.body_bytes).map(|i| (i * 31 + 7) as u8).collect();
    let sym = lcr_encode::encode_bytes(&data, &opts).expect("encode");
    let r = render_rgb(
        &sym,
        &RenderOptions {
            module_px,
            ..RenderOptions::screen()
        },
    );
    let d = Decoder::new().with_options(lcr_decode::DecodeOptions {
        max_version: MAX_V.with(|m| *m.borrow()),
        ..Default::default()
    });

    // `probe` is §12.1–§12.3 only: detection, ring correlation and format decode, with no
    // payload work. The gap between it and a full decode is the LDPC cost.
    let view =
        ImageView::new(&r.data, r.width, r.height, r.width * 3, PixelFormat::Rgb8).expect("view");
    let t0 = Instant::now();
    for _ in 0..n {
        d.probe(&view).expect("probe");
    }
    let probe = t0.elapsed() / n;

    let t0 = Instant::now();
    for _ in 0..n {
        d.decode_rgb(&r.data, r.width, r.height, r.width * 3)
            .expect("decode");
    }
    let decode = t0.elapsed() / n;

    (probe, decode, r.width, version.side() as u32)
}

#[test]
fn lowering_the_ceiling_recovers_the_cost() {
    for ceiling in [Version::MAX, Version::MAX_CLASSIC] {
        MAX_V.with(|m| *m.borrow_mut() = ceiling);
        let (probe, decode, _, _) = timed(20, 8, 20);
        println!(
            "  max_version {:>2}: v20 probe {probe:.1?}  decode {decode:.1?}",
            ceiling.get()
        );
    }
    MAX_V.with(|m| *m.borrow_mut() = Version::MAX);
}

#[test]
fn per_frame_cost_across_the_version_range() {
    println!(
        "\n  {:<5} {:>7} {:>7} {:>10} {:>10} {:>10}",
        "ver", "modules", "px", "probe", "decode", "ldpc"
    );
    for v in [10u8, 20, 32] {
        let n = 20;
        let (probe, decode, px, side) = timed(v, 8, n);
        println!(
            "  v{:<4} {:>7} {:>7} {:>10.1?} {:>10.1?} {:>10.1?}",
            v,
            format!("{side}²"),
            px,
            probe,
            decode,
            decode.saturating_sub(probe)
        );
    }
    println!();
}
