//! Emit a raw RGBA test frame for the browser demo.
//!
//! `cargo run -p lc-decode --release --example frame -- web/frame.rgba`
//!
//! RGBA rather than RGB deliberately: that is the layout a canvas `getImageData` returns,
//! so the fixture exercises the same `pixel-format::rgba8` path the browser will, rather
//! than a shape only this repo produces.
//!
//! Layout: `u32` width, `u32` height, both little-endian, then `width * height * 4` bytes.

#![allow(dead_code)]

use lc_core::{Compression, Profile};
use lc_encode::EncodeOptions;
use lc_render::{RenderOptions, render_rgb};

fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "web/frame.rgba".into());
    let payload = br#"{"name":"Fred","loc":"45.00,55.0","note":"decoded in a browser"}"#;

    let sym = lc_encode::encode_bytes(
        payload,
        &EncodeOptions {
            profile: Profile::C1,
            reference_border: true,
            compression: Some(Compression::None),
            ..Default::default()
        },
    )
    .unwrap();
    let r = render_rgb(
        &sym,
        &RenderOptions {
            module_px: 12,
            ..RenderOptions::screen()
        },
    );

    let mut buf = Vec::with_capacity(8 + (r.width * r.height * 4) as usize);
    buf.extend_from_slice(&r.width.to_le_bytes());
    buf.extend_from_slice(&r.height.to_le_bytes());
    for px in r.data.as_chunks::<3>().0 {
        buf.extend_from_slice(&[px[0], px[1], px[2], 255]);
    }

    if let Some(dir) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(&out, &buf).unwrap();
    println!(
        "  {out}  {}x{} RGBA, {} bytes\n  payload: {}",
        r.width,
        r.height,
        buf.len(),
        std::str::from_utf8(payload).unwrap()
    );
}
