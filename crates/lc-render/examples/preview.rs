//! Render sample symbols for eyeballing: `cargo run -p lc-render --example preview -- <dir>`
fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let opts = lc_encode::EncodeOptions { reference_border: true, ..Default::default() };
    for (name, profile) in [
        ("m", lc_core::Profile::M),
        ("c1", lc_core::Profile::C1),
        ("c2", lc_core::Profile::C2),
        ("c3", lc_core::Profile::C3),
    ] {
        let o = lc_encode::EncodeOptions { profile, ..opts.clone() };
        let sym = lc_encode::encode_json(
            r#"{"name":"Fred","loc":"45.00,55.0","note":"LC Code preview symbol"}"#,
            &o,
        )
        .unwrap();
        let png = lc_render::render_png(&sym, &lc_render::RenderOptions::screen());
        let path = format!("{dir}/preview-{name}-v{}.png", sym.format.version.get());
        std::fs::write(&path, png).unwrap();
        println!("{path}  v{} {:?}", sym.format.version.get(), profile);
    }
}
