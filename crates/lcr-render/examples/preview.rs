//! Render sample symbols for eyeballing: `cargo run -p lcr-render --example preview -- <dir>`
fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let opts = lcr_encode::EncodeOptions {
        reference_border: true,
        ..Default::default()
    };
    for (name, profile) in [
        ("m", lcr_core::Profile::M),
        ("c1", lcr_core::Profile::C1),
        ("c2", lcr_core::Profile::C2),
        ("c3", lcr_core::Profile::C3),
    ] {
        let o = lcr_encode::EncodeOptions {
            profile,
            ..opts.clone()
        };
        let sym = lcr_encode::encode_json(
            r#"{"name":"Fred","loc":"45.00,55.0","note":"LCR Code preview symbol"}"#,
            &o,
        )
        .unwrap();
        let png = lcr_render::render_png(&sym, &lcr_render::RenderOptions::screen());
        let path = format!("{dir}/preview-{name}-v{}.png", sym.format.version.get());
        std::fs::write(&path, png).unwrap();
        println!("{path}  v{} {:?}", sym.format.version.get(), profile);
    }
}
