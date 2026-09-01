//! The component, actually executed.
//!
//! `surface.rs` tests the logic natively, which leaves the part that only exists inside a
//! runtime untested: the canonical ABI lifting and lowering, and the `accumulator`
//! resource's lifetime across the boundary. This loads the built artifact into wasmtime
//! and calls it the way a host would.
//!
//! The most valuable assertion here is the cheapest one:
//! [`instantiates_with_an_empty_linker`]. Everything else in this project claims the
//! component imports nothing from its host — the README says so, D8 says so, and both were
//! inferred from reading `wasm-tools` output. Instantiating with a linker that provides
//! literally nothing turns that inference into a fact.
//!
//! Requires `just component` to have run. The test says so rather than failing obscurely.

use wasmtime::component::{Component, ComponentExportIndex, Instance, Linker, ResourceAny, Val};
use wasmtime::{Config, Engine, Store};

const FULL: &str = "../../dist/lc-code.wasm";
const SCANNER: &str = "../../dist/lc-code-scanner.wasm";

const JSON: &str = r#"{"name":"Fred","loc":"45.00,55.0"}"#;

/// `get_export` yields `(ComponentItem, ComponentExportIndex)`; every call site wants the
/// index. Wrapping it once keeps the tests about the component rather than about wasmtime.
fn export(
    instance: &Instance,
    store: &mut Store<()>,
    parent: Option<&ComponentExportIndex>,
    name: &str,
) -> ComponentExportIndex {
    instance
        .get_export(&mut *store, parent, name)
        .unwrap_or_else(|| panic!("no export {name:?}"))
        .1
}

/// The exported function `name`, looked up under `parent`.
fn func(
    instance: &Instance,
    store: &mut Store<()>,
    parent: &ComponentExportIndex,
    name: &str,
) -> wasmtime::component::Func {
    let idx = export(instance, store, Some(parent), name);
    instance.get_func(&mut *store, idx).unwrap_or_else(|| panic!("{name} is not a function"))
}

fn engine() -> Engine {
    let mut cfg = Config::new();
    cfg.wasm_component_model(true);
    Engine::new(&cfg).expect("engine")
}

fn load(path: &str) -> (Engine, Component) {
    let engine = engine();
    let bytes = std::fs::read(path).unwrap_or_else(|e| {
        panic!("cannot read {path}: {e}\n\nRun `just component` first — these tests exercise the built artifact, not the source.")
    });
    let component = Component::new(&engine, &bytes).expect("component parses");
    (engine, component)
}

/// The claim the whole "runs anywhere" argument rests on: nothing has to be provided.
///
/// No WASI, no clock, no randomness, no filesystem — a linker with nothing in it at all.
/// If this ever fails, the component has acquired a host dependency and D8's central
/// promise is broken.
#[test]
fn instantiates_with_an_empty_linker() {
    for path in [FULL, SCANNER] {
        let (engine, component) = load(path);
        let linker: Linker<()> = Linker::new(&engine);
        let mut store = Store::new(&engine, ());
        linker
            .instantiate(&mut store, &component)
            .unwrap_or_else(|e| panic!("{path} demanded something from its host: {e:#}"));
    }
}

/// Build an `image` record as the WIT interface declares it.
fn image_val(pixels: &[u8], width: u32, height: u32, stride: u32, format: &str) -> Val {
    Val::Record(vec![
        ("pixels".into(), Val::List(pixels.iter().map(|b| Val::U8(*b)).collect())),
        ("width".into(), Val::U32(width)),
        ("height".into(), Val::U32(height)),
        ("stride".into(), Val::U32(stride)),
        ("format".into(), Val::Enum(format.into())),
    ])
}

fn options_val(profile: &str) -> Val {
    Val::Record(vec![
        ("profile".into(), Val::Enum(profile.into())),
        ("version".into(), Val::Option(None)),
        ("ecc-luma".into(), Val::U8(1)),
        ("ecc-chroma".into(), Val::U8(1)),
        ("compression".into(), Val::Option(None)),
        ("reference-border".into(), Val::Bool(true)),
        ("mask".into(), Val::Option(None)),
    ])
}

/// Pull `pixels`, `width`, `height` out of a `rendered` record.
fn rendered_parts(v: &Val) -> (Vec<u8>, u32, u32, u8) {
    let Val::Result(Ok(Some(inner))) = v else {
        panic!("expected ok(rendered), got {v:?}");
    };
    let Val::Record(fields) = inner.as_ref() else { panic!("expected record") };
    let get = |name: &str| fields.iter().find(|(k, _)| k == name).map(|(_, v)| v).unwrap();
    let Val::List(px) = get("pixels") else { panic!("pixels") };
    let pixels = px
        .iter()
        .map(|b| match b {
            Val::U8(n) => *n,
            other => panic!("{other:?}"),
        })
        .collect();
    let (Val::U32(w), Val::U32(h), Val::U8(ver)) =
        (get("width"), get("height"), get("version"))
    else {
        panic!("width/height/version")
    };
    (pixels, *w, *h, *ver)
}

/// Encode and decode across the boundary, in one process, through the real ABI.
#[test]
fn encodes_and_decodes_through_the_canonical_abi() {
    let (engine, component) = load(FULL);
    let linker: Linker<()> = Linker::new(&engine);
    let mut store = Store::new(&engine, ());
    let instance = linker.instantiate(&mut store, &component).expect("instantiate");

    // --- encode
    let enc = export(&instance, &mut store, None, "lc:code/encoder@0.4.0");
    let enc_json = func(&instance, &mut store, &enc, "encode-json");

    let mut out = [Val::Bool(false)];
    enc_json
        .call(
            &mut store,
            &[Val::String(JSON.into()), options_val("c1"), Val::U32(10)],
            &mut out,
        )
        .expect("encode-json call");
    enc_json.post_return(&mut store).unwrap();

    let (pixels, width, height, version) = rendered_parts(&out[0]);
    assert_eq!(version, 2, "the §6.1 worked example is a v2 symbol");
    assert_eq!(pixels.len(), (width * height * 3) as usize);

    // --- decode what we just encoded
    let dec = export(&instance, &mut store, None, "lc:code/decoder@0.4.0");
    let decode = func(&instance, &mut store, &dec, "decode");

    let mut out = [Val::Bool(false)];
    decode
        .call(&mut store, &[image_val(&pixels, width, height, width * 3, "rgb8")], &mut out)
        .expect("decode call");
    decode.post_return(&mut store).unwrap();

    // --- the packet variant must arrive as `unsigned`, carrying CBOR
    let Val::Result(Ok(Some(res))) = &out[0] else { panic!("expected ok, got {:?}", out[0]) };
    let Val::Record(fields) = res.as_ref() else { panic!("record") };
    let packet = fields.iter().find(|(k, _)| k == "packet").map(|(_, v)| v).unwrap();
    let Val::Variant(case, payload) = packet else { panic!("variant") };
    assert_eq!(case, "unsigned", "a clean capture of an unsigned payload");

    let Some(payload) = payload else { panic!("unsigned carries a payload") };
    let Val::Record(p) = payload.as_ref() else { panic!("payload record") };
    let ctype = p.iter().find(|(k, _)| k == "ctype").map(|(_, v)| v).unwrap();
    assert_eq!(*ctype, Val::Enum("cbor".into()), "§9.4: transmit CBOR");

    let Val::List(body) = p.iter().find(|(k, _)| k == "body").map(|(_, v)| v).unwrap() else {
        panic!("body")
    };
    let body: Vec<u8> = body
        .iter()
        .map(|b| match b {
            Val::U8(n) => *n,
            other => panic!("{other:?}"),
        })
        .collect();
    let value = lc_core::cbor::Value::from_slice(&body).expect("valid CBOR out");
    assert_eq!(lc_core::json::to_json(&value), JSON, "round-tripped through the ABI");

    // --- diagnostics survive as f32/bool/u8, not just as a shape
    let diag = fields.iter().find(|(k, _)| k == "diagnostics").map(|(_, v)| v).unwrap();
    let Val::Record(d) = diag else { panic!("diagnostics record") };
    let get = |name: &str| d.iter().find(|(k, _)| k == name).map(|(_, v)| v).unwrap().clone();
    assert_eq!(get("version"), Val::U8(2));
    assert_eq!(get("profile"), Val::Enum("c1".into()));
    assert_eq!(get("chroma-recovered"), Val::Bool(true));
    let Val::Float32(snr) = get("snr-db") else { panic!("snr-db") };
    assert!(snr > 10.0, "clean render SNR {snr}");
}

/// The `accumulator` resource: construct, push across the boundary, decode, drop.
///
/// Resources are the part of the Component Model most likely to be wrong in a way native
/// tests cannot see — the handle table is entirely a runtime construct.
#[test]
fn accumulator_resource_survives_the_boundary() {
    let (engine, component) = load(FULL);
    let linker: Linker<()> = Linker::new(&engine);
    let mut store = Store::new(&engine, ());
    let instance = linker.instantiate(&mut store, &component).expect("instantiate");

    // Encode something to feed it.
    let enc = export(&instance, &mut store, None, "lc:code/encoder@0.4.0");
    let f = func(&instance, &mut store, &enc, "encode-json");
    let mut out = [Val::Bool(false)];
    f.call(&mut store, &[Val::String(JSON.into()), options_val("c1"), Val::U32(10)], &mut out)
        .unwrap();
    f.post_return(&mut store).unwrap();
    let (pixels, width, height, _) = rendered_parts(&out[0]);

    let dec = export(&instance, &mut store, None, "lc:code/decoder@0.4.0");
    let ctor = func(&instance, &mut store, &dec, "[constructor]accumulator");
    let mut handle = [Val::Bool(false)];
    ctor.call(&mut store, &[], &mut handle).expect("construct");
    ctor.post_return(&mut store).unwrap();
    let Val::Resource(acc) = handle[0] else { panic!("expected a resource handle") };

    // frames() starts at zero.
    let frames = func(&instance, &mut store, &dec, "[method]accumulator.frames");
    let mut n = [Val::Bool(false)];
    frames.call(&mut store, &[Val::Resource(acc)], &mut n).unwrap();
    frames.post_return(&mut store).unwrap();
    assert_eq!(n[0], Val::U8(0));

    // Push three frames through the ABI.
    let push = func(&instance, &mut store, &dec, "[method]accumulator.push");
    for _ in 0..3 {
        let mut r = [Val::Bool(false)];
        push.call(
            &mut store,
            &[Val::Resource(acc), image_val(&pixels, width, height, width * 3, "rgb8")],
            &mut r,
        )
        .expect("push call");
        push.post_return(&mut store).unwrap();
        assert!(matches!(r[0], Val::Result(Ok(_))), "push failed: {:?}", r[0]);
    }

    frames.call(&mut store, &[Val::Resource(acc)], &mut n).unwrap();
    frames.post_return(&mut store).unwrap();
    assert_eq!(n[0], Val::U8(3), "state persisted across calls");

    // §12.4's combined decode.
    let try_decode = func(&instance, &mut store, &dec, "[method]accumulator.try-decode");
    let mut r = [Val::Bool(false)];
    try_decode.call(&mut store, &[Val::Resource(acc)], &mut r).expect("try-decode");
    try_decode.post_return(&mut store).unwrap();
    let Val::Result(Ok(Some(res))) = &r[0] else { panic!("expected ok, got {:?}", r[0]) };
    let Val::Record(fields) = res.as_ref() else { panic!("record") };
    let diag = fields.iter().find(|(k, _)| k == "diagnostics").map(|(_, v)| v).unwrap();
    let Val::Record(d) = diag else { panic!("diagnostics") };
    let combined = d.iter().find(|(k, _)| k == "frames-combined").map(|(_, v)| v).unwrap();
    assert_eq!(*combined, Val::U8(3), "§12.4 combined all three frames");

    // Drop it. A leaked handle would show up as an error here.
    let acc: ResourceAny = acc;
    acc.resource_drop(&mut store).expect("drop");
}

/// §14.1: a scanner build must not be able to encode, even by accident.
#[test]
fn the_scanner_component_exports_no_encoder() {
    let (engine, component) = load(SCANNER);
    let linker: Linker<()> = Linker::new(&engine);
    let mut store = Store::new(&engine, ());
    let instance = linker.instantiate(&mut store, &component).expect("instantiate");

    assert!(
        instance.get_export(&mut store, None, "lc:code/decoder@0.4.0").is_some(),
        "the scanner must still decode"
    );
    assert!(
        instance.get_export(&mut store, None, "lc:code/encoder@0.4.0").is_none(),
        "the scanner must not carry the encoder (§14.1)"
    );
    let _ = &instance;
}

/// Errors cross the boundary as `result::err`, not as a trap.
///
/// A trap would tear down the instance and give the host nothing to report; §14.2 asks for
/// panics kept out of the hot path and `Result` returned instead.
#[test]
fn a_blank_frame_returns_an_error_not_a_trap() {
    let (engine, component) = load(SCANNER);
    let linker: Linker<()> = Linker::new(&engine);
    let mut store = Store::new(&engine, ());
    let instance = linker.instantiate(&mut store, &component).expect("instantiate");

    let dec = export(&instance, &mut store, None, "lc:code/decoder@0.4.0");
    let decode = func(&instance, &mut store, &dec, "decode");

    let blank = vec![255u8; 120 * 120 * 3];
    let mut out = [Val::Bool(false)];
    decode
        .call(&mut store, &[image_val(&blank, 120, 120, 120 * 3, "rgb8")], &mut out)
        .expect("the call itself must succeed — a trap here means the guest panicked");
    decode.post_return(&mut store).unwrap();

    match &out[0] {
        Val::Result(Err(Some(msg))) => {
            let Val::String(s) = msg.as_ref() else { panic!("error should be a string") };
            assert!(!s.is_empty(), "the error should say something");
        }
        other => panic!("expected err(string), got {other:?}"),
    }
}
