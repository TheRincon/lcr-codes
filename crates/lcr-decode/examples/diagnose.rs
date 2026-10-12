//! Print detection internals for a degraded capture.
//! `cargo run -p lcr-decode --release --example diagnose`

// The shared harness in `tests/support/ops.rs` carries every impairment; each consumer
// uses a subset.
#![allow(dead_code)]

use lcr_core::{Compression, Profile};
use lcr_decode::detect;
use lcr_decode::image::{ImageView, PixelFormat, binarize};
use lcr_encode::EncodeOptions;
use lcr_render::{Raster, RenderOptions, render_rgb};

include!("../tests/support/ops.rs");

fn main() {
    let opts = EncodeOptions {
        profile: Profile::C1,
        reference_border: true,
        compression: Some(Compression::None),
        ..Default::default()
    };
    let data: Vec<u8> = (0..96u8)
        .map(|i| i.wrapping_mul(101).wrapping_add(29))
        .collect();
    let sym = lcr_encode::encode_bytes(&data, &opts).unwrap();
    println!(
        "true version v{} side {}",
        sym.format.version.get(),
        sym.format.version.side()
    );
    let base = render_rgb(
        &sym,
        &RenderOptions {
            module_px: 12,
            ..RenderOptions::screen()
        },
    );

    for tilt in [0.0f32, 0.1, 0.2, 0.3, 0.4] {
        let r = perspective(&base, tilt, 0.0);
        let view =
            ImageView::new(&r.data, r.width, r.height, r.width * 3, PixelFormat::Rgb8).unwrap();
        let bin = binarize(&view.gray_buffer(), r.width, r.height).denoised();
        let cands = detect::find_finders(&bin);
        print!("tilt {tilt:>4}: {} candidates", cands.len());
        match detect::detect(&bin) {
            Some(d) => {
                let m: Vec<String> = d
                    .finders
                    .iter()
                    .map(|f| format!("{:.1}", f.module))
                    .collect();
                println!(
                    ", est v{}, modules [{}], alignment {:?}",
                    d.version.get(),
                    m.join(", "),
                    d.alignment.map(|a| (a.0 as i32, a.1 as i32))
                );
            }
            None => {
                if cands.len() >= 3 {
                    let o = detect::order_finders(cands[0], cands[1], cands[2]);
                    let [tl, tr, bl] = o;
                    let n = |a: detect::Finder, b: detect::Finder| {
                        ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt()
                    };
                    let (v1, v2) = ((tr.x - tl.x, tr.y - tl.y), (bl.x - tl.x, bl.y - tl.y));
                    let (n1, n2) = (n(tl, tr), n(tl, bl));
                    let cos = (v1.0 * v2.0 + v1.1 * v2.1) / (n1 * n2);
                    let mean = (tl.module + tr.module + bl.module) / 3.0;
                    let spread = [tl, tr, bl]
                        .iter()
                        .map(|f| (f.module - mean).abs())
                        .fold(0.0f32, f32::max);
                    println!(
                        ", DETECTION FAILED: |cos|={:.3} ratio={:.3} spread/mean={:.3} modules=[{:.1},{:.1},{:.1}]",
                        cos.abs(),
                        (n1 / n2).max(n2 / n1),
                        spread / mean,
                        tl.module,
                        tr.module,
                        bl.module
                    );
                } else {
                    println!(", DETECTION FAILED: only {} candidates", cands.len());
                }
            }
        }
    }
}
