//! The C ABI, exercised through the exported functions exactly as a C caller would —
//! raw pointers, status codes, out-parameters and explicit frees.
//!
//! `just ffi-test` covers the other half: that `include/lcr.h` describes these functions
//! correctly, by compiling and linking a C program against it.

use core::ffi::{CStr, c_char};
use core::ptr;
use lcr_ffi::*;

const JSON: &str = r#"{"name":"Fred","loc":"45.00,55.0"}"#;

fn last_error() -> String {
    let p = lcr_last_error();
    assert!(!p.is_null(), "a failing call must leave a message");
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

fn bytes(b: &LcrBytes) -> &[u8] {
    if b.data.is_null() {
        &[]
    } else {
        unsafe { core::slice::from_raw_parts(b.data, b.len) }
    }
}

fn take_string(p: *mut c_char) -> String {
    assert!(!p.is_null());
    let s = unsafe { CStr::from_ptr(p) }.to_str().unwrap().to_owned();
    unsafe { lcr_string_free(p) };
    s
}

fn encode_json(json: &str, opts: Option<&LcrEncodeOptions>) -> LcrRendered {
    let mut out = core::mem::MaybeUninit::<LcrRendered>::uninit();
    let st = unsafe {
        lcr_encode_json(
            json.as_ptr().cast(),
            json.len(),
            opts.map_or(ptr::null(), |o| o as *const _),
            12,
            out.as_mut_ptr(),
        )
    };
    assert_eq!(st, LCR_OK, "{}", last_error());
    unsafe { out.assume_init() }
}

fn image(r: &LcrRendered) -> LcrImage {
    LcrImage {
        pixels: r.pixels.data,
        len: r.pixels.len,
        width: r.width,
        height: r.height,
        stride: r.width * 3,
        format: LCR_PIXEL_RGB8,
    }
}

fn decode(img: &LcrImage) -> Result<LcrDecodeResult, (LcrStatus, String)> {
    let mut out = core::mem::MaybeUninit::<LcrDecodeResult>::uninit();
    match unsafe { lcr_decode(img, out.as_mut_ptr()) } {
        LCR_OK => Ok(unsafe { out.assume_init() }),
        st => Err((st, last_error())),
    }
}

fn json_to_cbor(json: &str) -> Vec<u8> {
    let mut out = LcrBytes {
        data: ptr::null_mut(),
        len: 0,
    };
    assert_eq!(
        unsafe { lcr_json_to_cbor(json.as_ptr().cast(), json.len(), &mut out) },
        LCR_OK
    );
    let v = bytes(&out).to_vec();
    unsafe { lcr_bytes_free(&mut out) };
    assert!(out.data.is_null() && out.len == 0, "free resets the buffer");
    v
}

#[test]
fn json_round_trips_through_encode_and_decode() {
    let mut sym = encode_json(JSON, None);
    assert_eq!(sym.pixels.len, (sym.width * sym.height * 3) as usize);

    let mut res = decode(&image(&sym)).expect("decodes");
    assert_eq!(res.kind, LCR_PACKET_UNSIGNED);
    assert_eq!(res.ctype, LCR_CONTENT_CBOR);
    assert_eq!(
        bytes(&res.body),
        json_to_cbor(JSON),
        "the wire bytes json_to_cbor promised"
    );
    assert_eq!(bytes(&res.prefix), b"", "no prefix on a complete decode");
    assert_eq!(res.diagnostics.version, sym.version);
    assert_eq!(res.diagnostics.profile, LCR_PROFILE_C1);

    let mut json = ptr::null_mut();
    let st = unsafe { lcr_cbor_to_json(res.body.data, res.body.len, &mut json) };
    assert_eq!(st, LCR_OK);
    assert_eq!(take_string(json), JSON);

    unsafe {
        lcr_decode_result_free(&mut res);
        lcr_rendered_free(&mut sym);
        // Every free tolerates a second call and a null.
        lcr_decode_result_free(&mut res);
        lcr_rendered_free(&mut sym);
        lcr_decode_result_free(ptr::null_mut());
        lcr_rendered_free(ptr::null_mut());
        lcr_bytes_free(ptr::null_mut());
        lcr_string_free(ptr::null_mut());
    }
}

#[test]
fn text_and_bytes_keep_their_content_types() {
    let text = "héllo, wörld";
    let mut out = core::mem::MaybeUninit::<LcrRendered>::uninit();
    let st = unsafe {
        lcr_encode_text(
            text.as_ptr().cast(),
            text.len(),
            ptr::null(),
            10,
            out.as_mut_ptr(),
        )
    };
    assert_eq!(st, LCR_OK);
    let mut sym = unsafe { out.assume_init() };
    let mut res = decode(&image(&sym)).unwrap();
    assert_eq!(
        (res.kind, res.ctype),
        (LCR_PACKET_UNSIGNED, LCR_CONTENT_TEXT)
    );
    assert_eq!(bytes(&res.body), text.as_bytes());
    unsafe {
        lcr_decode_result_free(&mut res);
        lcr_rendered_free(&mut sym);
    }

    let data = [0u8, 1, 2, 3, 0xff, 0xfe];
    let mut out = core::mem::MaybeUninit::<LcrRendered>::uninit();
    let st =
        unsafe { lcr_encode_bytes(data.as_ptr(), data.len(), ptr::null(), 10, out.as_mut_ptr()) };
    assert_eq!(st, LCR_OK);
    let mut sym = unsafe { out.assume_init() };
    let mut res = decode(&image(&sym)).unwrap();
    assert_eq!(
        (res.kind, res.ctype),
        (LCR_PACKET_UNSIGNED, LCR_CONTENT_OPAQUE)
    );
    assert_eq!(bytes(&res.body), data);
    unsafe {
        lcr_decode_result_free(&mut res);
        lcr_rendered_free(&mut sym);
    }
}

#[test]
fn options_are_honoured() {
    let opts = LcrEncodeOptions {
        profile: LCR_PROFILE_C2,
        version: 6,
        compression: LCR_COMPRESSION_NONE,
        reference_border: false,
        ..lcr_encode_options_default()
    };
    let mut sym = encode_json(JSON, Some(&opts));
    assert_eq!(sym.version, 6);
    let mut res = decode(&image(&sym)).unwrap();
    assert_eq!(res.diagnostics.profile, LCR_PROFILE_C2);
    assert_eq!(res.diagnostics.version, 6);
    unsafe {
        lcr_decode_result_free(&mut res);
        lcr_rendered_free(&mut sym);
    }
}

#[test]
fn bad_arguments_are_rejected_not_trusted() {
    let mut out = core::mem::MaybeUninit::<LcrRendered>::uninit();
    let call = |opts: &LcrEncodeOptions, px: u32, out: *mut LcrRendered| unsafe {
        lcr_encode_json(JSON.as_ptr().cast(), JSON.len(), opts, px, out)
    };
    let d = lcr_encode_options_default();

    assert_eq!(call(&d, 12, ptr::null_mut()), LCR_ERR_ARGUMENT);
    assert!(last_error().contains("`out` is null"), "{}", last_error());

    assert_eq!(
        call(&LcrEncodeOptions { profile: 9, ..d }, 12, out.as_mut_ptr()),
        LCR_ERR_ARGUMENT
    );
    assert!(last_error().contains("profile"));

    assert_eq!(
        call(
            &LcrEncodeOptions {
                compression: 77,
                ..d
            },
            12,
            out.as_mut_ptr()
        ),
        LCR_ERR_ARGUMENT
    );
    assert_eq!(
        call(&LcrEncodeOptions { version: 65, ..d }, 12, out.as_mut_ptr()),
        LCR_ERR_ARGUMENT
    );
    assert_eq!(
        call(&LcrEncodeOptions { mask: 300, ..d }, 12, out.as_mut_ptr()),
        LCR_ERR_ARGUMENT
    );

    assert_eq!(
        call(&d, 7, out.as_mut_ptr()),
        LCR_ERR_ARGUMENT,
        "§13.1's 8 px floor"
    );
    assert!(last_error().contains("§13.1"));

    let not_utf8 = [0xffu8, 0xfe];
    let st = unsafe {
        lcr_encode_text(
            not_utf8.as_ptr().cast(),
            2,
            ptr::null(),
            10,
            out.as_mut_ptr(),
        )
    };
    assert_eq!(st, LCR_ERR_ARGUMENT);
    assert!(last_error().contains("UTF-8"));

    // A null pointer with a non-zero length is refused; with zero length it is empty input.
    let st = unsafe { lcr_encode_bytes(ptr::null(), 4, ptr::null(), 10, out.as_mut_ptr()) };
    assert_eq!(st, LCR_ERR_ARGUMENT);
}

#[test]
fn decode_failures_are_failures_with_a_reason() {
    let px = vec![128u8; 64 * 64 * 3];
    let img = LcrImage {
        pixels: px.as_ptr(),
        len: px.len(),
        width: 64,
        height: 64,
        stride: 64 * 3,
        format: LCR_PIXEL_RGB8,
    };
    let (st, msg) = decode(&img).err().expect("a grey frame holds no symbol");
    assert_eq!(st, LCR_ERR_FAILED);
    assert!(!msg.is_empty());

    let mut probe = core::mem::MaybeUninit::<LcrProbeResult>::uninit();
    assert_eq!(
        unsafe { lcr_probe(&img, probe.as_mut_ptr()) },
        LCR_ERR_FAILED
    );

    // A buffer shorter than the geometry claims is the caller's mistake, not a decode
    // failure, and is caught before anything reads past the end.
    let short = LcrImage { len: 100, ..img };
    assert_eq!(decode(&short).err().unwrap().0, LCR_ERR_ARGUMENT);
    let bad_format = LcrImage { format: 3, ..img };
    assert_eq!(decode(&bad_format).err().unwrap().0, LCR_ERR_ARGUMENT);
}

#[test]
fn probe_reports_geometry() {
    let mut sym = encode_json(JSON, None);
    let mut p = core::mem::MaybeUninit::<LcrProbeResult>::uninit();
    assert_eq!(unsafe { lcr_probe(&image(&sym), p.as_mut_ptr()) }, LCR_OK);
    let p = unsafe { p.assume_init() };
    assert_eq!(p.version, sym.version);
    assert_eq!(p.profile, LCR_PROFILE_C1);
    assert!(p.capture_pitch_min > 0.0 && p.capture_pitch_max >= p.capture_pitch_min);
    unsafe { lcr_rendered_free(&mut sym) };
}

#[test]
fn accumulator_combines_frames() {
    let mut sym = encode_json(JSON, None);
    let acc = lcr_accumulator_new();
    let mut out = core::mem::MaybeUninit::<LcrDecodeResult>::uninit();
    unsafe {
        assert_eq!(
            lcr_accumulator_try_decode(acc, out.as_mut_ptr()),
            LCR_ERR_FAILED
        );
        assert!(last_error().contains("no frames"));
        for _ in 0..3 {
            assert_eq!(lcr_accumulator_push(acc, &image(&sym)), LCR_OK);
        }
        assert_eq!(lcr_accumulator_frames(acc), 3);
        assert_eq!(lcr_accumulator_try_decode(acc, out.as_mut_ptr()), LCR_OK);
        let mut res = out.assume_init();
        assert_eq!(res.kind, LCR_PACKET_UNSIGNED);
        assert_eq!(res.diagnostics.frames_combined, 3);
        lcr_decode_result_free(&mut res);

        lcr_accumulator_reset(acc);
        assert_eq!(lcr_accumulator_frames(acc), 0);
        lcr_accumulator_free(acc);
        lcr_accumulator_free(ptr::null_mut());
        assert_eq!(lcr_accumulator_frames(ptr::null()), 0);
        lcr_rendered_free(&mut sym);
    }
}

#[test]
fn svg_and_capacity() {
    let mut svg = ptr::null_mut();
    let st =
        unsafe { lcr_encode_json_svg(JSON.as_ptr().cast(), JSON.len(), ptr::null(), 10, &mut svg) };
    assert_eq!(st, LCR_OK);
    let svg = take_string(svg);
    assert!(
        svg.starts_with("<svg") || svg.starts_with("<?xml"),
        "{}",
        &svg[..40]
    );

    let mut small = 0u32;
    let mut large = 0u32;
    unsafe {
        assert_eq!(
            lcr_capacity(LCR_PROFILE_C1, 4, 1, 1, true, &mut small),
            LCR_OK
        );
        assert_eq!(
            lcr_capacity(LCR_PROFILE_C3, 4, 1, 1, true, &mut large),
            LCR_OK
        );
        assert_eq!(
            lcr_capacity(LCR_PROFILE_C1, 0, 1, 1, true, &mut small),
            LCR_ERR_ARGUMENT
        );
    }
    assert!(
        small > 0 && large > small,
        "C3 carries more than C1 at the same version"
    );
}

#[test]
fn cft_transfers_a_file_across_symbols() {
    // xorshift: deterministic, and incompressible enough that deflate cannot fold 6000
    // bytes into one symbol.
    let mut s = 0x9e37_79b9u32;
    let file: Vec<u8> = (0..6000)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            s as u8
        })
        .collect();
    let id = [9u8, 8, 7, 6, 5, 4, 3, 2];
    let mut cap = 0u32;
    assert_eq!(
        unsafe { lcr_capacity(LCR_PROFILE_C1, 20, 1, 1, true, &mut cap) },
        LCR_OK
    );
    // Headroom for the chunk's own CBOR framing around the payload.
    let chunk = cap - 32;

    let mut t = ptr::null_mut();
    let st = unsafe {
        lcr_cft_split(
            file.as_ptr(),
            file.len(),
            c"data.bin".as_ptr(),
            id.as_ptr(),
            id.len(),
            chunk,
            &mut t,
        )
    };
    assert_eq!(st, LCR_OK, "{}", last_error());
    let n = unsafe { lcr_cft_transfer_message_count(t) };
    assert!(
        n > 2,
        "6000 bytes of poorly compressible data needs several symbols"
    );
    assert_eq!(
        unsafe { lcr_cft_transfer_original_size(t) },
        file.len() as u64
    );

    let rx = lcr_cft_receiver_new();
    // Out of order, manifest last: the receiver must not care.
    let order: Vec<usize> = (1..n).rev().chain([0]).collect();
    let mut last = unsafe { lcr_cft_receiver_status(rx) };
    assert_eq!(last.state, LCR_CFT_AWAITING_MANIFEST);
    for i in order {
        let (mut data, mut len) = (ptr::null(), 0usize);
        assert_eq!(
            unsafe { lcr_cft_transfer_message(t, i, &mut data, &mut len) },
            LCR_OK
        );
        // Through a real symbol: encode the message, decode the pixels, accept the body.
        let mut out = core::mem::MaybeUninit::<LcrRendered>::uninit();
        let opts = LcrEncodeOptions {
            version: 20,
            ..lcr_encode_options_default()
        };
        let st = unsafe { lcr_encode_bytes(data, len, &opts, 8, out.as_mut_ptr()) };
        assert_eq!(st, LCR_OK, "message {i}: {}", last_error());
        let mut sym = unsafe { out.assume_init() };
        let mut res = decode(&image(&sym)).expect("each symbol decodes");
        let st = unsafe { lcr_cft_receiver_accept(rx, res.body.data, res.body.len, &mut last) };
        assert_eq!(st, LCR_OK, "{}", last_error());
        unsafe {
            lcr_decode_result_free(&mut res);
            lcr_rendered_free(&mut sym);
        }
        if i == n - 1 {
            assert_eq!(last.state, LCR_CFT_AWAITING_MANIFEST);
            assert_eq!(last.held, 1);
        }
    }
    assert_eq!(last.state, LCR_CFT_COMPLETE);
    assert_eq!(
        unsafe { lcr_cft_receiver_missing(rx, ptr::null_mut(), 0) },
        0
    );
    assert_eq!(
        take_string(unsafe { lcr_cft_receiver_filename(rx) }),
        "data.bin"
    );
    assert_eq!(
        unsafe { lcr_cft_receiver_original_size(rx) },
        file.len() as u64
    );

    let mut got = LcrBytes {
        data: ptr::null_mut(),
        len: 0,
    };
    assert_eq!(unsafe { lcr_cft_receiver_finish(rx, &mut got) }, LCR_OK);
    assert_eq!(bytes(&got), file);
    unsafe {
        lcr_bytes_free(&mut got);
        lcr_cft_receiver_free(rx);
        lcr_cft_transfer_free(t);
    }
}

#[test]
fn cft_reports_what_is_missing() {
    let file = [42u8; 50];
    let id = [1u8; 8];
    let mut t = ptr::null_mut();
    // A tiny chunk size and store-only content would still compress; uncompressible-ish
    // input at chunk 4 guarantees several chunks.
    let file: Vec<u8> = file
        .iter()
        .enumerate()
        .map(|(i, _)| (i * 97 % 251) as u8)
        .collect();
    assert_eq!(
        unsafe {
            lcr_cft_split(
                file.as_ptr(),
                file.len(),
                ptr::null(),
                id.as_ptr(),
                8,
                4,
                &mut t,
            )
        },
        LCR_OK
    );
    let n = unsafe { lcr_cft_transfer_message_count(t) };
    let rx = lcr_cft_receiver_new();
    // Manifest plus every second chunk.
    for i in (0..n).filter(|i| *i == 0 || i % 2 == 1) {
        let (mut d, mut l) = (ptr::null(), 0usize);
        unsafe {
            lcr_cft_transfer_message(t, i, &mut d, &mut l);
            assert_eq!(lcr_cft_receiver_accept(rx, d, l, ptr::null_mut()), LCR_OK);
        }
    }
    let total = unsafe { lcr_cft_receiver_missing(rx, ptr::null_mut(), 0) };
    assert!(total > 0);
    let mut buf = vec![0u32; total];
    assert_eq!(
        unsafe { lcr_cft_receiver_missing(rx, buf.as_mut_ptr(), buf.len()) },
        total
    );
    // Message i (i >= 1) carries chunk i - 1, so the odd messages delivered the even chunks.
    assert!(buf.iter().all(|c| c % 2 == 1), "{buf:?}");

    let mut got = LcrBytes {
        data: ptr::null_mut(),
        len: 0,
    };
    assert_eq!(
        unsafe { lcr_cft_receiver_finish(rx, &mut got) },
        LCR_ERR_FAILED
    );
    assert!(got.data.is_null(), "out untouched on failure");
    assert!(
        unsafe { lcr_cft_receiver_filename(rx) }.is_null(),
        "no filename was sent"
    );
    let mut idx = (ptr::null(), 0usize);
    assert_eq!(
        unsafe { lcr_cft_transfer_message(t, n, &mut idx.0, &mut idx.1) },
        LCR_ERR_ARGUMENT
    );
    unsafe {
        lcr_cft_receiver_free(rx);
        lcr_cft_transfer_free(t);
    }
}

#[test]
fn version_is_the_crate_version() {
    let v = unsafe { CStr::from_ptr(lcr_version()) }.to_str().unwrap();
    assert_eq!(v, env!("CARGO_PKG_VERSION"));
}
