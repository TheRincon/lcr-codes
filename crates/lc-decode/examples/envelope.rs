//! Sweep each degradation to find where decoding stops working.
//! `cargo run -p lc-decode --release --example envelope`

// The shared harness in `tests/support/ops.rs` carries every impairment; each consumer
// uses a subset.
#![allow(dead_code)]

use lc_core::{Compression, Packet, Profile};
use lc_decode::Decoder;
use lc_encode::EncodeOptions;
use lc_render::{Raster, RenderOptions, render_rgb};

include!("../tests/support/ops.rs");

fn payload() -> Vec<u8> { (0..96u8).map(|i| i.wrapping_mul(101).wrapping_add(29)).collect() }

fn base(profile: Profile) -> Raster {
    let opts = EncodeOptions { profile, reference_border: true, compression: Some(Compression::None), ..Default::default() };
    let sym = lc_encode::encode_bytes(&payload(), &opts).unwrap();
    render_rgb(&sym, &RenderOptions { module_px: 12, ..RenderOptions::screen() })
}

fn ok(r: &Raster) -> bool {
    matches!(Decoder::new().decode_rgb(&r.data, r.width, r.height, r.width * 3).map(|o| o.packet),
             Ok(Packet::Unsigned { ref body, .. }) if *body == payload())
}

fn sweep(name: &str, values: &[f32], f: impl Fn(f32) -> Raster) {
    let marks: String = values.iter().map(|&v| if ok(&f(v)) { '+' } else { '.' }).collect();
    let labels: String = values.iter().map(|v| format!("{v:>5}")).collect::<Vec<_>>().join("");
    println!("{name:<22} {marks}\n{:<22} {labels}", "");
}

fn main() {
    let b = base(Profile::C1);
    sweep("perspective tilt", &[0.1,0.2,0.3,0.4,0.5,0.6,0.7], |v| perspective(&b, v, 0.0));
    sweep("in-plane rotation", &[5.0,15.0,30.0,45.0,60.0,90.0], |v| perspective(&b, 0.0, v));
    sweep("shading", &[0.2,0.3,0.4,0.5,0.6,0.7,0.8], |v| shading(&b, v));
    sweep("specular strength", &[0.3,0.4,0.5,0.6,0.7,0.8,0.9], |v| specular(&b, 0.75, 0.3, 0.30, v));
    sweep("specular radius", &[0.15,0.20,0.25,0.30,0.35,0.40], |v| specular(&b, 0.5, 0.5, v, 0.6));
    sweep("noise sigma", &[0.02,0.04,0.06,0.08,0.10,0.12,0.14], |v| noise(&b, v, 7));
    sweep("blur radius", &[1.0,2.0,3.0,4.0,5.0,6.0], |v| blur(&b, v as u32));
    sweep("occlusion frac", &[0.10,0.15,0.20,0.25,0.30,0.35], |v| occlude(&b, 0.5, 0.5, v, v));
    for rot in [15.0f32, 30.0, 45.0] {
        sweep(&format!("tilt @ rot {rot:.0}"), &[0.1,0.2,0.3,0.4,0.5], |v| perspective(&b, v, rot));
    }
}
