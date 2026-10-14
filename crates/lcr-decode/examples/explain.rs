//! Generate the figures used by the explainer page, into `docs/img/`.
//!
//! `cargo run -p lcr-decode --release --example explain`
//!
//! Everything here is rendered by the real encoder and degraded by the real §15 harness,
//! so the pictures are the thing itself rather than an illustration of it.

#![allow(dead_code)]

use lcr_core::color::{self, Lab};
use lcr_core::{Compression, Profile};
use lcr_decode::Decoder;
use lcr_encode::EncodeOptions;
use lcr_render::qr::{QrEcc, QrSymbol, qr_encode_bytes};
use lcr_render::{Raster, RenderOptions, png_encode, render_rgb};

include!("../tests/support/ops.rs");

const DIR: &str = "docs/img";
const PX: u32 = 12;

fn save(name: &str, r: &Raster) {
    std::fs::write(format!("{DIR}/{name}.png"), png_encode(r)).unwrap();
    let bytes = std::fs::metadata(format!("{DIR}/{name}.png"))
        .unwrap()
        .len();
    let ok = Decoder::new()
        .decode_rgb(&r.data, r.width, r.height, r.width * 3)
        .is_ok();
    println!(
        "  {name:<22} {}x{}  {bytes:>6} B  decodes={ok}",
        r.width, r.height
    );
}

/// Write a figure that is not an LCR Code, so the decode check does not apply.
fn save_plain(name: &str, r: &Raster) {
    std::fs::write(format!("{DIR}/{name}.png"), png_encode(r)).unwrap();
    println!("  {name:<22} {}x{}", r.width, r.height);
}

/// A QR symbol as a raster, at the same module pitch as the LCR figures, with its
/// mandatory 4-module quiet zone.
fn qr_raster(qr: &QrSymbol, px: u32) -> Raster {
    let side = qr.outer() * px;
    let mut data = vec![255u8; (side * side * 3) as usize];
    let quiet = (qr.outer() - qr.size) / 2;
    for y in 0..qr.size {
        for x in 0..qr.size {
            if !qr.get(x, y) {
                continue;
            }
            for dy in 0..px {
                for dx in 0..px {
                    let (px_x, px_y) = ((x + quiet) * px + dx, (y + quiet) * px + dy);
                    let i = ((px_y * side + px_x) * 3) as usize;
                    data[i..i + 3].copy_from_slice(&[0, 0, 0]);
                }
            }
        }
    }
    Raster {
        width: side,
        height: side,
        data,
    }
}

/// Rec.709 luma, as a grey image — literally what a monochrome sensor records.
fn luma_only(r: &Raster) -> Raster {
    let mut out = r.clone();
    for i in (0..r.data.len()).step_by(3) {
        let y = (0.2126 * r.data[i] as f32
            + 0.7152 * r.data[i + 1] as f32
            + 0.0722 * r.data[i + 2] as f32)
            .round() as u8;
        out.data[i] = y;
        out.data[i + 1] = y;
        out.data[i + 2] = y;
    }
    out
}

/// The hue axis with the brightness axis flattened: every module forced to one `L*`,
/// keeping only `a*b*`. This is the chroma layer, made visible.
fn chroma_only(r: &Raster, l_fixed: f32) -> Raster {
    let mut out = r.clone();
    for i in (0..r.data.len()).step_by(3) {
        let src = color::Srgb8::new(r.data[i], r.data[i + 1], r.data[i + 2]);
        let lab = color::srgb8_to_lab(src);
        let flat =
            color::linear_to_srgb8(color::lab_to_linear(Lab::new(l_fixed, lab.a, lab.b)).clamped());
        out.data[i] = flat.r;
        out.data[i + 1] = flat.g;
        out.data[i + 2] = flat.b;
    }
    out
}

fn main() {
    std::fs::create_dir_all(DIR).unwrap();
    // Deliberately the same configuration the `envelope` sweep measures — 96 bytes, C1,
    // reference border on — so a figure captioned with a success rate is a picture of the
    // thing that rate was measured on. A smaller borderless symbol looks tidier and
    // behaves differently: at v2 there are barely any scattered references, so the §12.3
    // illumination fit is starved and decodability stops being monotone in noise.
    let payload = trial_payload(96, 1);
    let payload = &payload[..];
    let opts = EncodeOptions {
        profile: Profile::C1,
        reference_border: true,
        compression: Some(Compression::None),
        ..Default::default()
    };
    let sym = lcr_encode::encode_bytes(payload, &opts).unwrap();
    let ropts = RenderOptions {
        module_px: PX,
        ..RenderOptions::screen()
    };
    let base = render_rgb(&sym, &ropts);

    println!("figures:");
    save("symbol", &base);
    save("layer-luma", &luma_only(&base));
    save("layer-chroma", &chroma_only(&base, 62.0));

    // The monochrome profile, for "the luma layer alone is a complete code".
    let mono = lcr_encode::encode_bytes(
        payload,
        &EncodeOptions {
            profile: Profile::M,
            ..opts.clone()
        },
    )
    .unwrap();
    save("symbol-mono", &render_rgb(&mono, &ropts));

    // C3: eight colours, four hue states, one hue symbol per module (D11).
    //
    // Rendered from the *same* payload at the *same* module pitch as `symbol`, so the two
    // are directly comparable on the page: C3 fits this data at v3 where C1 needs v4, and
    // the figure is physically smaller as a result. That is the density argument made
    // visible, rather than asserted in a table.
    let c3 = lcr_encode::encode_bytes(
        payload,
        &EncodeOptions {
            profile: Profile::C3,
            ..opts.clone()
        },
    )
    .unwrap();
    let c3r = render_rgb(&c3, &ropts);
    save("symbol-c3", &c3r);
    save("layer-chroma-c3", &chroma_only(&c3r, 62.0));

    // QR carrying the *same* payload at the *same* module pitch, for the density
    // comparison on the page. Generated, not transcribed — `qrcodegen` encodes it and the
    // test suite reads it back with an independent decoder (`rqrr`).
    let qr = qr_encode_bytes(payload, QrEcc::Medium).expect("96 bytes fits a QR");
    println!(
        "  qr-medium              QR v{} {}x{} modules",
        qr.version, qr.size, qr.size
    );
    save_plain("qr-medium", &qr_raster(&qr, PX));

    // Degradations, at magnitudes that bracket the failure knee measured by `envelope`.
    for r in [1u32, 4, 5] {
        let n = r;
        save(&format!("blur-{n}"), &blur(&base, r));
    }
    for (label, s) in [("low", 0.04f32), ("high", 0.14)] {
        save(&format!("noise-{label}"), &noise(&base, s, 7));
    }
    // Azimuth 0 for both, so the pair differs only in how far the symbol leans.
    //
    // 0.10 and 0.50 are chosen because they are honest: this sweep is NOT monotone at
    // azimuth 0 (0.10 decodes, 0.15 and 0.20 do not, 0.25 and 0.30 do again), which is
    // the bilinear-resampling artefact documented on `Impair::Tilt`. Any pair taken from
    // the middle of that range would illustrate the resampler rather than perspective.
    for (label, t) in [("small", 0.10f32), ("large", 0.5)] {
        save(&format!("tilt-{label}"), &perspective(&base, t, 0.0));
    }
    save("glare", &specular(&base, 0.68, 0.32, 0.28, 0.75));
    save("shading", &shading(&base, 0.7));
}
