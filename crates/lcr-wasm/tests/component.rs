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

const FULL: &str = "../../dist/lcr-code.wasm";
const SCANNER: &str = "../../dist/lcr-code-scanner.wasm";
/// The demo build, which `just test` builds alongside the shipped two.
#[cfg(feature = "compare")]
const COMPARE: &str = "../../dist/lcr-code-compare.wasm";

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
    instance
        .get_func(&mut *store, idx)
        .unwrap_or_else(|| panic!("{name} is not a function"))
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
        (
            "pixels".into(),
            Val::List(pixels.iter().map(|b| Val::U8(*b)).collect()),
        ),
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
    let Val::Record(fields) = inner.as_ref() else {
        panic!("expected record")
    };
    let get = |name: &str| {
        fields
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v)
            .unwrap()
    };
    let Val::List(px) = get("pixels") else {
        panic!("pixels")
    };
    let pixels = px
        .iter()
        .map(|b| match b {
            Val::U8(n) => *n,
            other => panic!("{other:?}"),
        })
        .collect();
    let (Val::U32(w), Val::U32(h), Val::U8(ver)) = (get("width"), get("height"), get("version"))
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
    let instance = linker
        .instantiate(&mut store, &component)
        .expect("instantiate");

    // --- encode
    let enc = export(&instance, &mut store, None, "lcr:code/encoder@0.4.0");
    let enc_json = func(&instance, &mut store, &enc, "encode-json");

    let mut out = [Val::Bool(false)];
    enc_json
        .call(
            &mut store,
            &[Val::String(JSON.into()), options_val("c1"), Val::U32(10)],
            &mut out,
        )
        .expect("encode-json call");

    let (pixels, width, height, version) = rendered_parts(&out[0]);
    // v2 with finders; the ring's reclaimed modules (D17) fit it in v1.
    assert_eq!(
        version, 1,
        "the §6.1 worked example is a v1 finder-less symbol"
    );
    assert_eq!(pixels.len(), (width * height * 3) as usize);

    // --- decode what we just encoded
    let dec = export(&instance, &mut store, None, "lcr:code/decoder@0.4.0");
    let decode = func(&instance, &mut store, &dec, "decode");

    let mut out = [Val::Bool(false)];
    decode
        .call(
            &mut store,
            &[image_val(&pixels, width, height, width * 3, "rgb8")],
            &mut out,
        )
        .expect("decode call");

    // --- the packet variant must arrive as `unsigned`, carrying CBOR
    let Val::Result(Ok(Some(res))) = &out[0] else {
        panic!("expected ok, got {:?}", out[0])
    };
    let Val::Record(fields) = res.as_ref() else {
        panic!("record")
    };
    let packet = fields
        .iter()
        .find(|(k, _)| k == "packet")
        .map(|(_, v)| v)
        .unwrap();
    let Val::Variant(case, payload) = packet else {
        panic!("variant")
    };
    assert_eq!(case, "unsigned", "a clean capture of an unsigned payload");

    let Some(payload) = payload else {
        panic!("unsigned carries a payload")
    };
    let Val::Record(p) = payload.as_ref() else {
        panic!("payload record")
    };
    let ctype = p
        .iter()
        .find(|(k, _)| k == "ctype")
        .map(|(_, v)| v)
        .unwrap();
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
    let value = lcr_core::cbor::Value::from_slice(&body).expect("valid CBOR out");
    assert_eq!(
        lcr_core::json::to_json(&value),
        JSON,
        "round-tripped through the ABI"
    );

    // --- diagnostics survive as f32/bool/u8, not just as a shape
    let diag = fields
        .iter()
        .find(|(k, _)| k == "diagnostics")
        .map(|(_, v)| v)
        .unwrap();
    let Val::Record(d) = diag else {
        panic!("diagnostics record")
    };
    let get = |name: &str| {
        d.iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v)
            .unwrap()
            .clone()
    };
    // v1: finder-less (D17), as asserted at encode above.
    assert_eq!(get("version"), Val::U8(1));
    assert_eq!(get("profile"), Val::Enum("c1".into()));
    assert_eq!(get("chroma-recovered"), Val::Bool(true));
    let Val::Float32(snr) = get("snr-db") else {
        panic!("snr-db")
    };
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
    let instance = linker
        .instantiate(&mut store, &component)
        .expect("instantiate");

    // Encode something to feed it.
    let enc = export(&instance, &mut store, None, "lcr:code/encoder@0.4.0");
    let f = func(&instance, &mut store, &enc, "encode-json");
    let mut out = [Val::Bool(false)];
    f.call(
        &mut store,
        &[Val::String(JSON.into()), options_val("c1"), Val::U32(10)],
        &mut out,
    )
    .unwrap();
    let (pixels, width, height, _) = rendered_parts(&out[0]);

    let dec = export(&instance, &mut store, None, "lcr:code/decoder@0.4.0");
    let ctor = func(&instance, &mut store, &dec, "[constructor]accumulator");
    let mut handle = [Val::Bool(false)];
    ctor.call(&mut store, &[], &mut handle).expect("construct");
    let Val::Resource(acc) = handle[0] else {
        panic!("expected a resource handle")
    };

    // frames() starts at zero.
    let frames = func(&instance, &mut store, &dec, "[method]accumulator.frames");
    let mut n = [Val::Bool(false)];
    frames
        .call(&mut store, &[Val::Resource(acc)], &mut n)
        .unwrap();
    assert_eq!(n[0], Val::U8(0));

    // Push three frames through the ABI.
    let push = func(&instance, &mut store, &dec, "[method]accumulator.push");
    for _ in 0..3 {
        let mut r = [Val::Bool(false)];
        push.call(
            &mut store,
            &[
                Val::Resource(acc),
                image_val(&pixels, width, height, width * 3, "rgb8"),
            ],
            &mut r,
        )
        .expect("push call");
        assert!(
            matches!(r[0], Val::Result(Ok(_))),
            "push failed: {:?}",
            r[0]
        );
    }

    frames
        .call(&mut store, &[Val::Resource(acc)], &mut n)
        .unwrap();
    assert_eq!(n[0], Val::U8(3), "state persisted across calls");

    // §12.4's combined decode.
    let try_decode = func(
        &instance,
        &mut store,
        &dec,
        "[method]accumulator.try-decode",
    );
    let mut r = [Val::Bool(false)];
    try_decode
        .call(&mut store, &[Val::Resource(acc)], &mut r)
        .expect("try-decode");
    let Val::Result(Ok(Some(res))) = &r[0] else {
        panic!("expected ok, got {:?}", r[0])
    };
    let Val::Record(fields) = res.as_ref() else {
        panic!("record")
    };
    let diag = fields
        .iter()
        .find(|(k, _)| k == "diagnostics")
        .map(|(_, v)| v)
        .unwrap();
    let Val::Record(d) = diag else {
        panic!("diagnostics")
    };
    let combined = d
        .iter()
        .find(|(k, _)| k == "frames-combined")
        .map(|(_, v)| v)
        .unwrap();
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
    let instance = linker
        .instantiate(&mut store, &component)
        .expect("instantiate");

    assert!(
        instance
            .get_export(&mut store, None, "lcr:code/decoder@0.4.0")
            .is_some(),
        "the scanner must still decode"
    );
    assert!(
        instance
            .get_export(&mut store, None, "lcr:code/encoder@0.4.0")
            .is_none(),
        "the scanner must not carry the encoder (§14.1)"
    );
    let _ = &instance;
}

/// Neither shipped artifact may carry QR.
///
/// The `compare` feature exists so `display.html` can put the two symbologies side by side
/// and let the README's density claim be checked. That is a demo, and a symbology crate
/// has no business shipping a competing symbology — so the QR build is a third artifact
/// (`dist/lcr-code-compare.wasm`, built only by `just web`) and this asserts the other two
/// stayed clean. Without it, "off by default" is a comment rather than a fact.
#[test]
fn the_shipped_components_export_no_qr() {
    for path in [SCANNER, FULL] {
        let (engine, component) = load(path);
        let linker: Linker<()> = Linker::new(&engine);
        let mut store = Store::new(&engine, ());
        let instance = linker
            .instantiate(&mut store, &component)
            .expect("instantiate");
        assert!(
            instance
                .get_export(&mut store, None, "lcr:code/compare@0.4.0")
                .is_none(),
            "{path} must not carry QR generation",
        );
    }
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
    let instance = linker
        .instantiate(&mut store, &component)
        .expect("instantiate");

    let dec = export(&instance, &mut store, None, "lcr:code/decoder@0.4.0");
    let decode = func(&instance, &mut store, &dec, "decode");

    let blank = vec![255u8; 120 * 120 * 3];
    let mut out = [Val::Bool(false)];
    decode
        .call(
            &mut store,
            &[image_val(&blank, 120, 120, 120 * 3, "rgb8")],
            &mut out,
        )
        .expect("the call itself must succeed — a trap here means the guest panicked");

    match &out[0] {
        Val::Result(Err(Some(msg))) => {
            let Val::String(s) = msg.as_ref() else {
                panic!("error should be a string")
            };
            assert!(!s.is_empty(), "the error should say something");
        }
        other => panic!("expected err(string), got {other:?}"),
    }
}

/// What the build's size pass costs at run time (D20).
///
/// D15 declined `opt-level = "z"` because it cost 11% median decode time, so a size win has
/// to be priced the same way. Run it against `dist/` as built, then against a `dist/` built
/// without wasm-opt, and compare. Under wasmtime rather than jco, so read the ratio, not the
/// absolute milliseconds.
#[test]
#[ignore]
fn decode_cost_through_the_component() {
    let (engine, component) = load(FULL);
    let linker: Linker<()> = Linker::new(&engine);
    let mut store = Store::new(&engine, ());
    let instance = linker
        .instantiate(&mut store, &component)
        .expect("instantiate");

    let enc = export(&instance, &mut store, None, "lcr:code/encoder@0.4.0");
    let enc_json = func(&instance, &mut store, &enc, "encode-json");
    let mut out = [Val::Bool(false)];
    enc_json
        .call(
            &mut store,
            &[Val::String(JSON.into()), options_val("c1"), Val::U32(12)],
            &mut out,
        )
        .expect("encode-json call");
    let (pixels, width, height, _) = rendered_parts(&out[0]);

    let dec = export(&instance, &mut store, None, "lcr:code/decoder@0.4.0");
    let decode = func(&instance, &mut store, &dec, "decode");
    let frame = image_val(&pixels, width, height, width * 3, "rgb8");

    // One warm-up, then time the rest.
    let mut out = [Val::Bool(false)];
    decode
        .call(&mut store, std::slice::from_ref(&frame), &mut out)
        .unwrap();

    const N: u32 = 20;
    let t0 = std::time::Instant::now();
    for _ in 0..N {
        decode
            .call(&mut store, std::slice::from_ref(&frame), &mut out)
            .unwrap();
    }
    let each = t0.elapsed() / N;
    println!("decode {width}x{height}: {each:?} per frame over {N} frames");
}

#[cfg(feature = "compare")]
fn bytes_val(b: &[u8]) -> Val {
    Val::List(b.iter().map(|x| Val::U8(*x)).collect())
}

#[cfg(feature = "compare")]
fn bytes_of(v: &Val) -> Vec<u8> {
    let Val::List(items) = v else {
        panic!("expected list<u8>, got {v:?}")
    };
    items
        .iter()
        .map(|b| match b {
            Val::U8(n) => *n,
            other => panic!("{other:?}"),
        })
        .collect()
}

/// The fountain is the demo's illustration, not a shipped feature: neither shipped
/// artifact may export it.
#[test]
fn shipped_artifacts_carry_no_fountain() {
    for path in [FULL, SCANNER] {
        let (engine, component) = load(path);
        let linker: Linker<()> = Linker::new(&engine);
        let mut store = Store::new(&engine, ());
        let instance = linker.instantiate(&mut store, &component).unwrap();
        assert!(
            instance
                .get_export(&mut store, None, "lcr:code/cft-fountain@0.4.0")
                .is_none(),
            "{path} exports cft-fountain"
        );
    }
}

/// A fountain transfer through the real ABI (CFT.md §21): the sender resource built by
/// `start-fountain`, frames taken from it with a third of them dropped, and the receiver
/// resource finishing — verified — from whatever arrived. Against the demo build, the one
/// that exports `cft-fountain`.
#[cfg(feature = "compare")]
#[test]
fn a_fountain_transfer_crosses_the_boundary() {
    let (engine, component) = load(COMPARE);
    let linker: Linker<()> = Linker::new(&engine);
    let mut store = Store::new(&engine, ());
    let instance = linker
        .instantiate(&mut store, &component)
        .expect("instantiate");
    let cft = export(&instance, &mut store, None, "lcr:code/cft@0.4.0");
    let fountain = export(&instance, &mut store, None, "lcr:code/cft-fountain@0.4.0");

    // Incompressible, so the chunk count is what the size says: 40 chunks of 300 bytes.
    let mut s = 0x9E37_79B9u32;
    let file: Vec<u8> = (0..12_000)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            s as u8
        })
        .collect();

    let start = func(&instance, &mut store, &fountain, "start-fountain");
    let mut out = [Val::Bool(false)];
    start
        .call(
            &mut store,
            &[
                bytes_val(&file),
                Val::String("noise.bin".into()),
                bytes_val(b"\x01\x02\x03\x04"),
                Val::U32(300),
            ],
            &mut out,
        )
        .expect("start-fountain call");
    let Val::Result(Ok(Some(handle))) = &out[0] else {
        panic!("start-fountain failed: {:?}", out[0])
    };
    let Val::Resource(tx) = **handle else {
        panic!("expected a fountain handle")
    };

    let count = func(
        &instance,
        &mut store,
        &fountain,
        "[method]fountain.source-count",
    );
    let mut k = [Val::Bool(false)];
    count
        .call(&mut store, &[Val::Resource(tx)], &mut k)
        .unwrap();
    let Val::U32(k) = k[0] else { panic!() };
    assert!(k > 30, "expected ~40 chunks, got {k}");

    let ctor = func(&instance, &mut store, &cft, "[constructor]receiver");
    let mut rx = [Val::Bool(false)];
    ctor.call(&mut store, &[], &mut rx).unwrap();
    let Val::Resource(rx) = rx[0] else { panic!() };

    let frame = func(&instance, &mut store, &fountain, "[method]fountain.frame");
    let accept = func(&instance, &mut store, &cft, "[method]receiver.accept");
    let mut done = false;
    for n in 0..20 * k as u64 {
        // Drop every third frame: a fixed pattern a plain carousel would also survive,
        // but only after looping round for the chunks it lost.
        if n % 3 == 2 {
            continue;
        }
        let mut msg = [Val::Bool(false)];
        frame
            .call(&mut store, &[Val::Resource(tx), Val::U64(n)], &mut msg)
            .unwrap();
        let mut st = [Val::Bool(false)];
        accept
            .call(&mut store, &[Val::Resource(rx), msg[0].clone()], &mut st)
            .unwrap();
        let Val::Result(Ok(Some(status))) = &st[0] else {
            panic!("accept failed at frame {n}: {:?}", st[0])
        };
        if matches!(**status, Val::Variant(ref tag, _) if tag == "complete") {
            done = true;
            break;
        }
    }
    assert!(done, "never completed");

    let finish = func(&instance, &mut store, &cft, "[method]receiver.finish");
    let mut got = [Val::Bool(false)];
    finish
        .call(&mut store, &[Val::Resource(rx)], &mut got)
        .unwrap();
    let Val::Result(Ok(Some(bytes))) = &got[0] else {
        panic!("finish failed: {:?}", got[0])
    };
    assert_eq!(bytes_of(bytes), file);

    tx.resource_drop(&mut store).expect("drop fountain");
    rx.resource_drop(&mut store).expect("drop receiver");
}
