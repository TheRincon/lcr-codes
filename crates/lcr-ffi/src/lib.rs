//! C ABI for LCR Code.
//!
//! The same surface as the `lcr-code` WebAssembly component (`crates/lcr-wasm/wit/lcr.wit`),
//! minus QR comparison, for a native host that would rather link a library than embed a
//! wasm runtime. The header is `include/lcr.h`, generated from this file by
//! `just ffi-header`; `just ffi-test` builds a C program against it and runs it.
//!
//! ## Conventions
//!
//! - **Status codes.** Every fallible function returns an `LcrStatus`. `LCR_OK` is zero; on
//!   anything else `lcr_last_error()` says what went wrong. Out-parameters are written only
//!   on success, so a failed call leaves the caller's struct as it was.
//! - **Enums are integers.** Profiles, pixel formats and the rest cross the boundary as
//!   `uint32_t` with `#define`d values rather than as C `enum`s. A C enum holding a value
//!   Rust does not know is undefined behaviour on the Rust side; an integer is just checked,
//!   and an unknown one is `LCR_ERR_ARGUMENT`.
//! - **Ownership.** Anything the library returns belongs to the caller and is released by
//!   the matching `_free` function. Anything the caller passes in is borrowed for the
//!   duration of the call and never retained. Every `_free` function accepts null.
//! - **Threads.** Free functions are safe to call from any thread. A handle may move
//!   between threads but must not be used from two at once. `lcr_last_error()` is per
//!   thread.
//! - **Panics.** A panic inside the library aborts the process. Nothing unwinds across the
//!   boundary — the workspace builds release with `panic = "abort"`, and an `extern "C"`
//!   function aborts on unwind regardless.
//!
//! ## Why the conversions are restated here rather than shared with `lcr-wasm`
//!
//! `lcr-wasm`'s plain-Rust layer does the same mapping from `lcr_decode`'s types to
//! interface types, but depending on it would drag `wit-bindgen` into every native build
//! for the sake of forty lines. The two are kept in step by the tests: both round-trip the
//! same encoder through the same decoder.

use core::ffi::c_char;
use core::ptr;
use std::cell::RefCell;
use std::ffi::{CStr, CString};

use lcr_core::{ContentType, Packet, Profile};
use lcr_decode::image::{ImageView, PixelFormat};
use lcr_decode::{Decoder, FrameAccumulator};

// ---------------------------------------------------------------- status and errors

/// Result of a fallible call. `LCR_OK` is zero; see `lcr_last_error()` otherwise.
pub type LcrStatus = u32;
/// Success.
pub const LCR_OK: LcrStatus = 0;
/// A required pointer was null, text was not UTF-8, or an enum or option was out of range.
/// The call was rejected before doing any work.
pub const LCR_ERR_ARGUMENT: LcrStatus = 1;
/// The operation itself failed: no symbol in the frame, a payload too large for any
/// version, a malformed or incomplete transfer.
pub const LCR_ERR_FAILED: LcrStatus = 2;

thread_local! {
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

fn fail(status: LcrStatus, msg: impl core::fmt::Display) -> LcrStatus {
    // Interior NULs cannot occur in any message this crate builds, but the text of a
    // lower-level error is not ours to promise about, and a lost message beats a panic.
    let text = msg.to_string().replace('\0', "\u{FFFD}");
    let c = CString::new(text).unwrap_or_default();
    LAST_ERROR.with(|e| *e.borrow_mut() = Some(c));
    status
}

fn failed(e: impl core::fmt::Display) -> LcrStatus {
    fail(LCR_ERR_FAILED, e)
}

/// Describe the most recent failure on this thread, or null if there has been none.
///
/// The string is owned by the library and stays valid until the next failing call on
/// the same thread. It is only meaningful straight after a call returned something other
/// than `LCR_OK`: successful calls do not clear it.
#[unsafe(no_mangle)]
pub extern "C" fn lcr_last_error() -> *const c_char {
    LAST_ERROR.with(|e| e.borrow().as_ref().map_or(ptr::null(), |c| c.as_ptr()))
}

/// The library version, e.g. `"0.4.0"`. Static; never free it.
#[unsafe(no_mangle)]
pub extern "C" fn lcr_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast()
}

/// Reject a null pointer argument, naming it.
macro_rules! nonnull {
    ($($p:ident),+) => {
        $(
            if $p.is_null() {
                return fail(LCR_ERR_ARGUMENT, concat!("`", stringify!($p), "` is null"));
            }
        )+
    };
}

// ---------------------------------------------------------------- enums

/// Colour configuration (§5). One of the `LCR_PROFILE_*` values.
pub type LcrProfile = u32;
/// Monochrome fallback, 1.00 bits/module.
pub const LCR_PROFILE_MONO: LcrProfile = 0;
/// Screen default: 4 colours, co-sited chroma.
pub const LCR_PROFILE_C1: LcrProfile = 1;
/// Print default: 4 colours, 2×2 chroma cells.
pub const LCR_PROFILE_C2: LcrProfile = 2;
/// 8 colours, co-sited chroma. Optional, and the least robust.
pub const LCR_PROFILE_C3: LcrProfile = 3;

fn profile_in(p: LcrProfile) -> Option<Profile> {
    Some(match p {
        LCR_PROFILE_MONO => Profile::M,
        LCR_PROFILE_C1 => Profile::C1,
        LCR_PROFILE_C2 => Profile::C2,
        LCR_PROFILE_C3 => Profile::C3,
        _ => return None,
    })
}

fn profile_out(p: Profile) -> LcrProfile {
    match p {
        Profile::M => LCR_PROFILE_MONO,
        Profile::C1 => LCR_PROFILE_C1,
        Profile::C2 => LCR_PROFILE_C2,
        Profile::C3 => LCR_PROFILE_C3,
    }
}

/// Wire content type (§9.2). One of the `LCR_CONTENT_*` values.
pub type LcrContentType = u32;
/// Opaque bytes, no interpretation.
pub const LCR_CONTENT_OPAQUE: LcrContentType = 0;
/// UTF-8 text.
pub const LCR_CONTENT_TEXT: LcrContentType = 1;
/// JSON. Discouraged on the wire (§9.4); `lcr_encode_json` sends CBOR instead.
pub const LCR_CONTENT_JSON: LcrContentType = 2;
/// CBOR — what `lcr_encode_json` produces. `lcr_cbor_to_json` renders it.
pub const LCR_CONTENT_CBOR: LcrContentType = 3;
/// COSE_Sign1, the optional signed packet (§10).
pub const LCR_CONTENT_COSE_SIGN1: LcrContentType = 4;
/// Multipart (§9.5).
pub const LCR_CONTENT_MULTIPART: LcrContentType = 5;

fn ctype_out(c: ContentType) -> LcrContentType {
    match c {
        ContentType::Opaque => LCR_CONTENT_OPAQUE,
        ContentType::Text => LCR_CONTENT_TEXT,
        ContentType::Json => LCR_CONTENT_JSON,
        ContentType::Cbor => LCR_CONTENT_CBOR,
        ContentType::CoseSign1 => LCR_CONTENT_COSE_SIGN1,
        ContentType::Multipart => LCR_CONTENT_MULTIPART,
    }
}

/// Pixel layout of a supplied frame. One of the `LCR_PIXEL_*` values.
pub type LcrPixelFormat = u32;
/// One byte per pixel. Chroma is unavailable, so only the luma layer decodes.
pub const LCR_PIXEL_GRAY8: LcrPixelFormat = 0;
/// Three bytes per pixel.
pub const LCR_PIXEL_RGB8: LcrPixelFormat = 1;
/// Four bytes per pixel; the fourth is ignored.
pub const LCR_PIXEL_RGBA8: LcrPixelFormat = 2;

/// Compression applied to the framed body (§9.3). One of the `LCR_COMPRESSION_*` values.
pub type LcrCompression = u32;
/// Try each scheme and keep the smallest — frequently none at these sizes. Zero, so a
/// zero-initialised `LcrEncodeOptions` asks for it.
pub const LCR_COMPRESSION_AUTO: LcrCompression = 0;
/// No compression.
pub const LCR_COMPRESSION_NONE: LcrCompression = 1;
/// DEFLATE (RFC 1951).
pub const LCR_COMPRESSION_DEFLATE: LcrCompression = 2;
/// Brotli. Not compiled into this build, so encoding with it fails; listed so the values
/// match the format rather than this build.
pub const LCR_COMPRESSION_BROTLI: LcrCompression = 3;

/// What a decode produced. One of the `LCR_PACKET_*` values.
pub type LcrPacketKind = u32;
/// Decoded and carries no signature — the §1.2 common case. `body` holds the payload.
pub const LCR_PACKET_UNSIGNED: LcrPacketKind = 0;
/// Signature verified. `body` holds the payload.
pub const LCR_PACKET_VERIFIED: LcrPacketKind = 1;
/// Well-formed COSE, not verified. `body` is empty by construction (§10.8): there is no
/// unverified content to read by forgetting to check this field.
pub const LCR_PACKET_UNVERIFIED: LcrPacketKind = 2;
/// A luma-only prefix (§12.5), in `prefix`. A **diagnostic**, not content: it has no CRC
/// to check until the whole frame arrives, and §8.1 forbids acting on it.
pub const LCR_PACKET_PARTIAL: LcrPacketKind = 3;

// ---------------------------------------------------------------- owned buffers

/// A byte buffer owned by the caller once returned. Release with `lcr_bytes_free`.
///
/// An empty buffer has `data == NULL` and `len == 0`.
#[repr(C)]
pub struct LcrBytes {
    pub data: *mut u8,
    pub len: usize,
}

impl LcrBytes {
    const EMPTY: LcrBytes = LcrBytes {
        data: ptr::null_mut(),
        len: 0,
    };

    fn from_vec(v: Vec<u8>) -> LcrBytes {
        if v.is_empty() {
            return LcrBytes::EMPTY;
        }
        let b = v.into_boxed_slice();
        let len = b.len();
        LcrBytes {
            data: Box::into_raw(b).cast(),
            len,
        }
    }

    /// Release the buffer and reset to empty, so a second free is harmless.
    ///
    /// # Safety
    ///
    /// `data` and `len` must be as `from_vec` produced them.
    unsafe fn release(&mut self) {
        if !self.data.is_null() {
            // SAFETY: `from_vec` produced this pointer from a boxed slice of `len` bytes.
            drop(unsafe { Box::from_raw(ptr::slice_from_raw_parts_mut(self.data, self.len)) });
        }
        *self = LcrBytes::EMPTY;
    }
}

/// Release a buffer the library returned and reset it to empty. Null is ignored.
///
/// # Safety
///
/// `bytes` must be null or point to an `LcrBytes` the library filled in and that has not
/// been modified since.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_bytes_free(bytes: *mut LcrBytes) {
    if let Some(b) = unsafe { bytes.as_mut() } {
        // SAFETY: the caller promises the buffer came from this library unmodified.
        unsafe { b.release() };
    }
}

/// Release a string the library returned. Null is ignored.
///
/// # Safety
///
/// `s` must be null or a string returned by this library and not yet freed. Never pass
/// the result of `lcr_last_error()` or `lcr_version()`, which the library owns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_string_free(s: *mut c_char) {
    if !s.is_null() {
        // SAFETY: the caller promises `s` came from `CString::into_raw` below.
        drop(unsafe { CString::from_raw(s) });
    }
}

fn string_out(s: String) -> Result<*mut c_char, LcrStatus> {
    CString::new(s)
        .map(CString::into_raw)
        .map_err(|_| failed("result contains a NUL byte and cannot be returned as a C string"))
}

/// Borrow `len` bytes at `data`. Null is allowed exactly when `len` is zero.
///
/// # Safety
///
/// When non-null, `data` must be valid for reads of `len` bytes for `'a`.
unsafe fn borrow<'a>(data: *const u8, len: usize, what: &str) -> Result<&'a [u8], LcrStatus> {
    if len == 0 {
        return Ok(&[]);
    }
    if data.is_null() {
        return Err(fail(LCR_ERR_ARGUMENT, format!("`{what}` is null")));
    }
    // SAFETY: non-null, and the caller vouches for `len` readable bytes.
    Ok(unsafe { core::slice::from_raw_parts(data, len) })
}

/// Borrow `len` bytes at `data` as UTF-8.
///
/// # Safety
///
/// As [`borrow`].
unsafe fn borrow_str<'a>(
    data: *const c_char,
    len: usize,
    what: &str,
) -> Result<&'a str, LcrStatus> {
    // SAFETY: forwarded to the caller.
    let bytes = unsafe { borrow(data.cast(), len, what) }?;
    core::str::from_utf8(bytes)
        .map_err(|e| fail(LCR_ERR_ARGUMENT, format!("`{what}` is not UTF-8: {e}")))
}

/// Collapse a `Result<(), LcrStatus>` into the status a C caller sees.
fn status(r: Result<(), LcrStatus>) -> LcrStatus {
    r.err().unwrap_or(LCR_OK)
}

// ---------------------------------------------------------------- decode types

/// A captured frame, borrowed for the duration of a call.
#[repr(C)]
pub struct LcrImage {
    /// Pixel data, at least `(height - 1) * stride + width * channels` bytes.
    pub pixels: *const u8,
    /// Length of `pixels` in bytes.
    pub len: usize,
    pub width: u32,
    pub height: u32,
    /// Bytes per row. May exceed `width * channels` for a padded camera plane.
    pub stride: u32,
    /// One of the `LCR_PIXEL_*` values.
    pub format: LcrPixelFormat,
}

/// Why a decode went the way it did (§14.3).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LcrDiagnostics {
    pub luma_iterations: u16,
    pub chroma_iterations: u16,
    /// False means the chroma layer was unrecoverable.
    pub chroma_recovered: bool,
    /// RMS residual of the §12.3 illumination fit, in linear light.
    pub illum_residual_rms: f32,
    /// References discarded by §12.3's robust refit. High means glare.
    pub references_rejected: u8,
    /// Per-module luma SNR. The most useful single number for user guidance.
    pub snr_db: f32,
    /// Frames combined before decoding (§12.4).
    pub frames_combined: u8,
    /// Symbol version, 1..=64.
    pub version: u8,
    /// One of the `LCR_PROFILE_*` values.
    pub profile: LcrProfile,
    /// Confidence margin of the winning format-region copy.
    pub format_margin: f32,
    /// Whether a full homography was fitted rather than an affine fallback.
    pub alignment_found: bool,
    /// Module pitch as captured, in sensor pixels, at the symbol's finest corner.
    pub capture_pitch_min: f32,
    /// Module pitch as captured at the coarsest corner. The ratio against
    /// `capture_pitch_min` measures viewing obliquity.
    pub capture_pitch_max: f32,
}

/// The result of a decode (§14.3). Release with `lcr_decode_result_free`.
#[repr(C)]
pub struct LcrDecodeResult {
    /// One of the `LCR_PACKET_*` values. Read it before reading anything else.
    pub kind: LcrPacketKind,
    /// One of the `LCR_CONTENT_*` values. Meaningful for `UNSIGNED` and `VERIFIED` only.
    pub ctype: LcrContentType,
    /// The payload, for `UNSIGNED` and `VERIFIED`. Empty otherwise.
    pub body: LcrBytes,
    /// The luma-only prefix, for `PARTIAL`. Empty otherwise. Never content (§8.1).
    pub prefix: LcrBytes,
    pub diagnostics: LcrDiagnostics,
}

/// Where a symbol is and how it was captured, established without decoding it.
///
/// Every field also appears in `LcrDiagnostics`. These are the ones that survive a decode
/// failure: they come from §12.1–§12.3, which run before any payload is touched.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LcrProbeResult {
    pub version: u8,
    /// One of the `LCR_PROFILE_*` values.
    pub profile: LcrProfile,
    pub format_margin: f32,
    pub alignment_found: bool,
    pub illum_residual_rms: f32,
    pub references_rejected: u8,
    pub capture_pitch_min: f32,
    pub capture_pitch_max: f32,
}

/// Release the buffers inside a decode result and reset them to empty. Null is ignored.
///
/// # Safety
///
/// `result` must be null or point to a result the library filled in.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_decode_result_free(result: *mut LcrDecodeResult) {
    if let Some(r) = unsafe { result.as_mut() } {
        // SAFETY: both buffers came from `LcrBytes::from_vec`.
        unsafe {
            r.body.release();
            r.prefix.release();
        }
    }
}

/// Borrow an `LcrImage` as a validated view.
///
/// # Safety
///
/// `img` must be non-null and its `pixels` valid for `len` bytes for `'a`.
unsafe fn view<'a>(img: *const LcrImage) -> Result<ImageView<'a>, LcrStatus> {
    // SAFETY: the caller checked `img` for null and vouches for it.
    let img = unsafe { &*img };
    let format = match img.format {
        LCR_PIXEL_GRAY8 => PixelFormat::Gray8,
        LCR_PIXEL_RGB8 => PixelFormat::Rgb8,
        LCR_PIXEL_RGBA8 => PixelFormat::Rgba8,
        other => {
            return Err(fail(
                LCR_ERR_ARGUMENT,
                format!("unknown pixel format {other}"),
            ));
        }
    };
    // SAFETY: forwarded to the caller.
    let pixels = unsafe { borrow(img.pixels, img.len, "pixels") }?;
    ImageView::new(pixels, img.width, img.height, img.stride, format)
        .map_err(|e| fail(LCR_ERR_ARGUMENT, e))
}

fn result_out(out: lcr_decode::DecodeOutput) -> LcrDecodeResult {
    let d = out.diagnostics;
    let (kind, ctype, body, prefix) = match out.packet {
        Packet::Unsigned { body, ctype } => (LCR_PACKET_UNSIGNED, ctype_out(ctype), body, vec![]),
        // A verified packet's body is the CWT payload, which §10 makes CBOR.
        Packet::Verified { body, .. } => (LCR_PACKET_VERIFIED, LCR_CONTENT_CBOR, body, vec![]),
        Packet::Partial { prefix } => (LCR_PACKET_PARTIAL, LCR_CONTENT_OPAQUE, vec![], prefix),
        // `Unverified`, and anything `#[non_exhaustive]` adds later: a variant this crate
        // does not know must not acquire a body by falling through.
        _ => (LCR_PACKET_UNVERIFIED, LCR_CONTENT_OPAQUE, vec![], vec![]),
    };
    LcrDecodeResult {
        kind,
        ctype,
        body: LcrBytes::from_vec(body),
        prefix: LcrBytes::from_vec(prefix),
        diagnostics: LcrDiagnostics {
            luma_iterations: d.luma_iterations,
            chroma_iterations: d.chroma_iterations,
            chroma_recovered: d.chroma_recovered,
            illum_residual_rms: d.illum_residual_rms,
            references_rejected: d.references_rejected,
            snr_db: d.estimated_snr_db,
            frames_combined: d.frames_combined,
            version: d.format.version.get(),
            profile: profile_out(d.format.profile),
            format_margin: d.format_margin,
            alignment_found: d.alignment_found,
            capture_pitch_min: d.capture_pitch_min,
            capture_pitch_max: d.capture_pitch_max,
        },
    }
}

// ---------------------------------------------------------------- decode

/// Decode a single frame.
///
/// # Safety
///
/// `img` must point to a valid `LcrImage` whose `pixels` are readable for `len` bytes.
/// `out` must be valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_decode(img: *const LcrImage, out: *mut LcrDecodeResult) -> LcrStatus {
    nonnull!(img, out);
    status((|| {
        // SAFETY: checked non-null above; the caller vouches for the rest.
        let view = unsafe { view(img) }?;
        let decoded = Decoder::new().decode(&view).map_err(failed)?;
        // SAFETY: checked non-null; `write` does not drop the uninitialised old value.
        unsafe { out.write(result_out(decoded)) };
        Ok(())
    })())
}

/// Locate a symbol and measure the capture geometry, without decoding it.
///
/// §12.1–§12.3 only. Succeeding means a symbol was found and its format region read, and
/// says nothing about the payload — which is what separates "nothing in frame" from
/// "found it, move closer" in a scanning UI.
///
/// # Safety
///
/// As `lcr_decode`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_probe(img: *const LcrImage, out: *mut LcrProbeResult) -> LcrStatus {
    nonnull!(img, out);
    status((|| {
        // SAFETY: checked non-null above; the caller vouches for the rest.
        let view = unsafe { view(img) }?;
        let p = Decoder::new().probe(&view).map_err(failed)?;
        let r = LcrProbeResult {
            version: p.format.version.get(),
            profile: profile_out(p.format.profile),
            format_margin: p.format_margin,
            alignment_found: p.alignment_found,
            illum_residual_rms: p.illum_residual_rms,
            references_rejected: p.references_rejected,
            capture_pitch_min: p.capture_pitch_min,
            capture_pitch_max: p.capture_pitch_max,
        };
        // SAFETY: checked non-null.
        unsafe { out.write(r) };
        Ok(())
    })())
}

// ---------------------------------------------------------------- accumulator

/// Multi-frame LLR combining (§12.4). Opaque; create with `lcr_accumulator_new`.
///
/// The spec calls this "the single best use of a live camera preview": banding and glare
/// move between frames, so accumulating soft information across them is far cheaper than
/// tightening the code rate.
pub struct LcrAccumulator {
    inner: FrameAccumulator,
    decoder: Decoder,
}

/// Create an empty accumulator. Release with `lcr_accumulator_free`.
#[unsafe(no_mangle)]
pub extern "C" fn lcr_accumulator_new() -> *mut LcrAccumulator {
    Box::into_raw(Box::new(LcrAccumulator {
        inner: FrameAccumulator::new(),
        decoder: Decoder::new(),
    }))
}

/// Release an accumulator. Null is ignored.
///
/// # Safety
///
/// `acc` must be null or a live handle from `lcr_accumulator_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_accumulator_free(acc: *mut LcrAccumulator) {
    if !acc.is_null() {
        // SAFETY: the caller promises a live handle from `Box::into_raw`.
        drop(unsafe { Box::from_raw(acc) });
    }
}

/// Add a frame.
///
/// # Safety
///
/// `acc` must be a live handle; `img` as for `lcr_decode`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_accumulator_push(
    acc: *mut LcrAccumulator,
    img: *const LcrImage,
) -> LcrStatus {
    nonnull!(acc, img);
    status((|| {
        // SAFETY: checked non-null; the caller vouches for both.
        let (acc, view) = unsafe { (&mut *acc, view(img)?) };
        acc.inner.push(&view).map_err(failed)
    })())
}

/// How many frames have been combined. Zero for a null handle.
///
/// # Safety
///
/// `acc` must be null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_accumulator_frames(acc: *const LcrAccumulator) -> u8 {
    // SAFETY: the caller promises null or a live handle.
    unsafe { acc.as_ref() }.map_or(0, |a| a.inner.frames())
}

/// Attempt a decode from everything pushed so far.
///
/// # Safety
///
/// `acc` must be a live handle and `out` valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_accumulator_try_decode(
    acc: *const LcrAccumulator,
    out: *mut LcrDecodeResult,
) -> LcrStatus {
    nonnull!(acc, out);
    // SAFETY: checked non-null.
    let acc = unsafe { &*acc };
    if acc.inner.frames() == 0 {
        return failed("no frames pushed");
    }
    match acc.inner.try_decode(&acc.decoder) {
        Ok(decoded) => {
            // SAFETY: checked non-null.
            unsafe { out.write(result_out(decoded)) };
            LCR_OK
        }
        Err(e) => failed(e),
    }
}

/// Discard accumulated frames. Null is ignored.
///
/// # Safety
///
/// `acc` must be null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_accumulator_reset(acc: *mut LcrAccumulator) {
    // SAFETY: the caller promises null or a live handle.
    if let Some(a) = unsafe { acc.as_mut() } {
        a.inner.reset();
    }
}

// ---------------------------------------------------------------- encode

/// Encoding knobs. Start from `lcr_encode_options_default()` rather than zeroing the
/// struct: zero is a valid value for every field, but it means monochrome, ECC level 0, no
/// border and the fixed mask 0 — a legal symbol, and not the one anyone wants by accident.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LcrEncodeOptions {
    /// One of the `LCR_PROFILE_*` values.
    pub profile: LcrProfile,
    /// Symbol version 1..=64, or 0 for the smallest that fits (§6.1). Versions above 32 are
    /// ring-only (D21): §7's classic format word has five bits for the version.
    pub version: u8,
    /// Luma error-correction level.
    pub ecc_luma: u8,
    /// Chroma error-correction level.
    pub ecc_chroma: u8,
    /// One of the `LCR_COMPRESSION_*` values.
    pub compression: LcrCompression,
    /// Emit the §6.4 screen reference border. Costs no payload capacity — it lives outside
    /// the symbol grid — and materially improves decoding under glare.
    pub reference_border: bool,
    /// Data mask, or -1 for the §8.5 penalty search.
    pub mask: i16,
}

/// The defaults the WebAssembly component uses: C1, automatic version, ECC level 1 on both
/// layers, automatic compression, reference border on, automatic mask.
#[unsafe(no_mangle)]
pub extern "C" fn lcr_encode_options_default() -> LcrEncodeOptions {
    LcrEncodeOptions {
        profile: LCR_PROFILE_C1,
        version: 0,
        ecc_luma: 1,
        ecc_chroma: 1,
        compression: LCR_COMPRESSION_AUTO,
        reference_border: true,
        mask: -1,
    }
}

/// A rendered symbol. Release with `lcr_rendered_free`.
#[repr(C)]
pub struct LcrRendered {
    /// Row-major RGB triples, `width * height * 3` bytes.
    pub pixels: LcrBytes,
    pub width: u32,
    pub height: u32,
    /// Symbol version actually used.
    pub version: u8,
    /// Grid side in modules, excluding the quiet zone.
    pub side: u32,
}

/// Release the pixels inside a rendered symbol. Null is ignored.
///
/// # Safety
///
/// `r` must be null or point to a symbol the library rendered.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_rendered_free(r: *mut LcrRendered) {
    if let Some(r) = unsafe { r.as_mut() } {
        // SAFETY: the buffer came from `LcrBytes::from_vec`.
        unsafe { r.pixels.release() };
    }
}

/// Resolve options: null means the defaults.
///
/// # Safety
///
/// `o` must be null or valid for reads.
unsafe fn options_in(o: *const LcrEncodeOptions) -> Result<lcr_encode::EncodeOptions, LcrStatus> {
    // SAFETY: forwarded to the caller.
    let o = unsafe { o.as_ref() }
        .copied()
        .unwrap_or_else(|| lcr_encode_options_default());
    let arg = |e: lcr_core::LcrError| fail(LCR_ERR_ARGUMENT, e);
    Ok(lcr_encode::EncodeOptions {
        profile: profile_in(o.profile)
            .ok_or_else(|| fail(LCR_ERR_ARGUMENT, format!("unknown profile {}", o.profile)))?,
        version: match o.version {
            0 => None,
            v => Some(lcr_core::Version::new(v).map_err(arg)?),
        },
        ecc: lcr_core::EccLevel::new(o.ecc_luma, o.ecc_chroma).map_err(arg)?,
        compression: match o.compression {
            LCR_COMPRESSION_AUTO => None,
            LCR_COMPRESSION_NONE => Some(lcr_core::Compression::None),
            LCR_COMPRESSION_DEFLATE => Some(lcr_core::Compression::Deflate),
            LCR_COMPRESSION_BROTLI => Some(lcr_core::Compression::Brotli),
            other => {
                return Err(fail(
                    LCR_ERR_ARGUMENT,
                    format!("unknown compression {other}"),
                ));
            }
        },
        reference_border: o.reference_border,
        mask: match o.mask {
            -1 => None,
            m => Some(
                u8::try_from(m)
                    .map_err(|_| fail(LCR_ERR_ARGUMENT, format!("mask {m} out of range")))?,
            ),
        },
    })
}

/// §13.1 point 3 makes 8 device pixels per module normative. Below it a conforming reader
/// is not expected to cope, so refusing beats emitting a symbol nothing can read.
fn render_options(module_px: u32) -> Result<lcr_render::RenderOptions, LcrStatus> {
    if module_px < 8 {
        return Err(fail(
            LCR_ERR_ARGUMENT,
            format!("module_px must be at least 8 (§13.1), got {module_px}"),
        ));
    }
    Ok(lcr_render::RenderOptions {
        module_px,
        ..lcr_render::RenderOptions::screen()
    })
}

/// Rasterise `sym` into `out`.
///
/// # Safety
///
/// `out` must be valid for writes.
unsafe fn rendered_out(
    sym: Result<lcr_core::Symbol, lcr_core::LcrError>,
    module_px: u32,
    out: *mut LcrRendered,
) -> Result<(), LcrStatus> {
    let ropts = render_options(module_px)?;
    let sym = sym.map_err(failed)?;
    let raster = lcr_render::render_rgb(&sym, &ropts);
    let r = LcrRendered {
        pixels: LcrBytes::from_vec(raster.data),
        width: raster.width,
        height: raster.height,
        version: sym.format.version.get(),
        side: sym.format.version.side() as u32,
    };
    // SAFETY: forwarded to the caller.
    unsafe { out.write(r) };
    Ok(())
}

/// Encode JSON and render it as RGB pixels. The JSON is converted to CBOR first — §9.4's
/// "author in JSON, transmit CBOR" — so it decodes as `LCR_CONTENT_CBOR`.
///
/// `json` need not be NUL-terminated. `opts` may be null for the defaults. `module_px` is
/// the pitch in pixels per module, at least 8.
///
/// # Safety
///
/// `json` must be readable for `len` bytes, `opts` null or valid, `out` valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_encode_json(
    json: *const c_char,
    len: usize,
    opts: *const LcrEncodeOptions,
    module_px: u32,
    out: *mut LcrRendered,
) -> LcrStatus {
    nonnull!(out);
    status((|| {
        // SAFETY: forwarded to the caller.
        let (json, o) = unsafe { (borrow_str(json, len, "json")?, options_in(opts)?) };
        // SAFETY: checked non-null.
        unsafe { rendered_out(lcr_encode::encode_json(json, &o), module_px, out) }
    })())
}

/// Encode UTF-8 text and render it as RGB pixels.
///
/// # Safety
///
/// As `lcr_encode_json`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_encode_text(
    text: *const c_char,
    len: usize,
    opts: *const LcrEncodeOptions,
    module_px: u32,
    out: *mut LcrRendered,
) -> LcrStatus {
    nonnull!(out);
    status((|| {
        // SAFETY: forwarded to the caller.
        let (text, o) = unsafe { (borrow_str(text, len, "text")?, options_in(opts)?) };
        // SAFETY: checked non-null.
        unsafe { rendered_out(lcr_encode::encode_text(text, &o), module_px, out) }
    })())
}

/// Encode opaque bytes and render them as RGB pixels.
///
/// # Safety
///
/// `data` must be readable for `len` bytes; otherwise as `lcr_encode_json`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_encode_bytes(
    data: *const u8,
    len: usize,
    opts: *const LcrEncodeOptions,
    module_px: u32,
    out: *mut LcrRendered,
) -> LcrStatus {
    nonnull!(out);
    status((|| {
        // SAFETY: forwarded to the caller.
        let (data, o) = unsafe { (borrow(data, len, "data")?, options_in(opts)?) };
        // SAFETY: checked non-null.
        unsafe { rendered_out(lcr_encode::encode_bytes(data, &o), module_px, out) }
    })())
}

/// Encode JSON and render it as an SVG document. Release `*out_svg` with
/// `lcr_string_free`.
///
/// # Safety
///
/// As `lcr_encode_json`, with `out_svg` valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_encode_json_svg(
    json: *const c_char,
    len: usize,
    opts: *const LcrEncodeOptions,
    module_px: u32,
    out_svg: *mut *mut c_char,
) -> LcrStatus {
    nonnull!(out_svg);
    status((|| {
        // SAFETY: forwarded to the caller.
        let (json, o) = unsafe { (borrow_str(json, len, "json")?, options_in(opts)?) };
        let ropts = render_options(module_px)?;
        let sym = lcr_encode::encode_json(json, &o).map_err(failed)?;
        let svg = string_out(lcr_render::render_svg(&sym, &ropts))?;
        // SAFETY: checked non-null.
        unsafe { out_svg.write(svg) };
        Ok(())
    })())
}

/// Largest framed body a symbol with these settings holds, in bytes.
///
/// For sizing CFT chunks from the carrier: the alternative — encode, catch the failure,
/// try a smaller chunk — turns a lookup into a search.
///
/// # Safety
///
/// `out` must be valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_capacity(
    profile: LcrProfile,
    version: u8,
    ecc_luma: u8,
    ecc_chroma: u8,
    reference_border: bool,
    out: *mut u32,
) -> LcrStatus {
    nonnull!(out);
    status((|| {
        let arg = |e: lcr_core::LcrError| fail(LCR_ERR_ARGUMENT, e);
        let profile = profile_in(profile)
            .ok_or_else(|| fail(LCR_ERR_ARGUMENT, format!("unknown profile {profile}")))?;
        let version = lcr_core::Version::new(version).map_err(arg)?;
        let ecc = lcr_core::EccLevel::new(ecc_luma, ecc_chroma).map_err(arg)?;
        let cap = lcr_encode::capacity_for_symbol(version, profile, ecc, reference_border);
        // SAFETY: checked non-null.
        unsafe { out.write(cap.body_bytes as u32) };
        Ok(())
    })())
}

// ---------------------------------------------------------------- codec

/// Render a CBOR body as JSON text — the read side of §9.4. Release `*out_json` with
/// `lcr_string_free`.
///
/// # Safety
///
/// `cbor` must be readable for `len` bytes and `out_json` valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_cbor_to_json(
    cbor: *const u8,
    len: usize,
    out_json: *mut *mut c_char,
) -> LcrStatus {
    nonnull!(out_json);
    status((|| {
        // SAFETY: forwarded to the caller.
        let cbor = unsafe { borrow(cbor, len, "cbor") }?;
        let value = lcr_core::cbor::Value::from_slice(cbor).map_err(failed)?;
        let json = string_out(lcr_core::json::to_json(&value))?;
        // SAFETY: checked non-null.
        unsafe { out_json.write(json) };
        Ok(())
    })())
}

/// The exact bytes `lcr_encode_json` puts on the wire — §9.4's write side. Release
/// `out` with `lcr_bytes_free`.
///
/// # Safety
///
/// `json` must be readable for `len` bytes and `out` valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_json_to_cbor(
    json: *const c_char,
    len: usize,
    out: *mut LcrBytes,
) -> LcrStatus {
    nonnull!(out);
    status((|| {
        // SAFETY: forwarded to the caller.
        let json = unsafe { borrow_str(json, len, "json") }?;
        let cbor = lcr_core::json::parse(json).map_err(failed)?.to_vec();
        // SAFETY: checked non-null.
        unsafe { out.write(LcrBytes::from_vec(cbor)) };
        Ok(())
    })())
}

// ---------------------------------------------------------------- cft

/// **Deflate, not Zstandard**, to match the WebAssembly component: a transfer split here
/// must be receivable by a browser, which cannot have zstd. The manifest names the codec,
/// so a receiver is never guessing.
const CFT_CODEC: lcr_cft::Compression = lcr_cft::Compression::Deflate;

/// A file, compressed and split into CFT messages (docs/CFT.md). Opaque; create with
/// `lcr_cft_split`, release with `lcr_cft_transfer_free`.
pub struct LcrCftTransfer {
    messages: Vec<Vec<u8>>,
    total_chunks: u32,
    original_size: u64,
    compressed_size: u64,
}

/// Compress `file` and split it into chunks of at most `chunk_size` payload bytes, one
/// symbol's worth each. Message 0 is the manifest.
///
/// `filename` is a NUL-terminated UTF-8 string, or null for none. `transfer_id`
/// identifies the transfer and should be random enough not to collide with another;
/// this library has no RNG of its own. Size `chunk_size` with `lcr_capacity`.
///
/// # Safety
///
/// `file` and `transfer_id` must be readable for their lengths, `filename` null or a valid
/// C string, and `out` valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_cft_split(
    file: *const u8,
    file_len: usize,
    filename: *const c_char,
    transfer_id: *const u8,
    transfer_id_len: usize,
    chunk_size: u32,
    out: *mut *mut LcrCftTransfer,
) -> LcrStatus {
    nonnull!(out);
    status((|| {
        // SAFETY: forwarded to the caller.
        let (file, id) = unsafe {
            (
                borrow(file, file_len, "file")?,
                borrow(transfer_id, transfer_id_len, "transfer_id")?,
            )
        };
        // SAFETY: forwarded to the caller.
        let name = unsafe { cft_filename(filename) }?;
        let t = lcr_cft::split(file, name, id.to_vec(), CFT_CODEC, chunk_size).map_err(failed)?;
        let handle = Box::new(LcrCftTransfer {
            messages: t.messages(),
            total_chunks: t.manifest.total_chunks,
            original_size: t.manifest.original_size,
            compressed_size: t.manifest.compressed_size,
        });
        // SAFETY: checked non-null.
        unsafe { out.write(Box::into_raw(handle)) };
        Ok(())
    })())
}

/// A CFT filename argument: null or empty for none, otherwise UTF-8.
///
/// # Safety
///
/// `filename` must be null or a valid C string.
unsafe fn cft_filename(filename: *const c_char) -> Result<Option<String>, LcrStatus> {
    if filename.is_null() {
        return Ok(None);
    }
    // SAFETY: the caller promises a valid C string.
    let s = unsafe { CStr::from_ptr(filename) }
        .to_str()
        .map_err(|e| fail(LCR_ERR_ARGUMENT, format!("`filename` is not UTF-8: {e}")))?;
    Ok((!s.is_empty()).then(|| s.to_owned()))
}

/// A fountain transfer (CFT.md §21): the source chunks once, then repair symbols without
/// end, any slightly-more-than-`source_count` of which rebuild the file. Opaque; create
/// with `lcr_cft_fountain_new`, release with `lcr_cft_fountain_free`.
#[cfg(feature = "fountain")]
pub struct LcrCftFountain(lcr_cft::Fountain);

/// Compress and split `file` as `lcr_cft_split` does, and wrap the chunks as a fountain.
/// Arguments are as for `lcr_cft_split`.
///
/// # Safety
///
/// As for `lcr_cft_split`.
#[cfg(feature = "fountain")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_cft_fountain_new(
    file: *const u8,
    file_len: usize,
    filename: *const c_char,
    transfer_id: *const u8,
    transfer_id_len: usize,
    chunk_size: u32,
    out: *mut *mut LcrCftFountain,
) -> LcrStatus {
    nonnull!(out);
    status((|| {
        // SAFETY: forwarded to the caller.
        let (file, id, name) = unsafe {
            (
                borrow(file, file_len, "file")?,
                borrow(transfer_id, transfer_id_len, "transfer_id")?,
                cft_filename(filename)?,
            )
        };
        let t = lcr_cft::split(file, name, id.to_vec(), CFT_CODEC, chunk_size).map_err(failed)?;
        let handle = Box::new(LcrCftFountain(lcr_cft::Fountain::new(t)));
        // SAFETY: checked non-null.
        unsafe { out.write(Box::into_raw(handle)) };
        Ok(())
    })())
}

/// Release a fountain. Null is ignored.
///
/// # Safety
///
/// `f` must be null or a live handle from `lcr_cft_fountain_new`.
#[cfg(feature = "fountain")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_cft_fountain_free(f: *mut LcrCftFountain) {
    if !f.is_null() {
        // SAFETY: the caller promises a live handle from `Box::into_raw`.
        drop(unsafe { Box::from_raw(f) });
    }
}

/// The message for frame `n` of the carousel, counting from 0: the manifest every eighth
/// frame, otherwise the source chunks once and then repair symbols. Encode each with
/// `lcr_encode_bytes` and show them in order. Free `out` with `lcr_bytes_free`.
///
/// # Safety
///
/// `f` must be a live handle and `out` valid for writes.
#[cfg(feature = "fountain")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_cft_fountain_frame(
    f: *const LcrCftFountain,
    n: u64,
    out: *mut LcrBytes,
) -> LcrStatus {
    nonnull!(f, out);
    // SAFETY: checked non-null.
    unsafe { out.write(LcrBytes::from_vec((*f).0.frame(n))) };
    LCR_OK
}

/// Source chunk count, `K`. Zero for null.
///
/// # Safety
///
/// `f` must be null or a live handle.
#[cfg(feature = "fountain")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_cft_fountain_source_count(f: *const LcrCftFountain) -> u32 {
    // SAFETY: the caller promises null or a live handle.
    unsafe { f.as_ref() }.map_or(0, |f| f.0.source_count())
}

/// The file's size before compression. Zero for null.
///
/// # Safety
///
/// `f` must be null or a live handle.
#[cfg(feature = "fountain")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_cft_fountain_original_size(f: *const LcrCftFountain) -> u64 {
    // SAFETY: the caller promises null or a live handle.
    unsafe { f.as_ref() }.map_or(0, |f| f.0.transfer().manifest.original_size)
}

/// The compressed stream's size. Zero for null.
///
/// # Safety
///
/// `f` must be null or a live handle.
#[cfg(feature = "fountain")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_cft_fountain_compressed_size(f: *const LcrCftFountain) -> u64 {
    // SAFETY: the caller promises null or a live handle.
    unsafe { f.as_ref() }.map_or(0, |f| f.0.transfer().manifest.compressed_size)
}

/// Release a transfer. Null is ignored.
///
/// # Safety
///
/// `t` must be null or a live handle from `lcr_cft_split`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_cft_transfer_free(t: *mut LcrCftTransfer) {
    if !t.is_null() {
        // SAFETY: the caller promises a live handle from `Box::into_raw`.
        drop(unsafe { Box::from_raw(t) });
    }
}

/// How many messages the transfer takes, manifest included. Zero for null.
///
/// # Safety
///
/// `t` must be null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_cft_transfer_message_count(t: *const LcrCftTransfer) -> usize {
    // SAFETY: the caller promises null or a live handle.
    unsafe { t.as_ref() }.map_or(0, |t| t.messages.len())
}

/// Borrow message `index`. The bytes stay owned by the transfer and valid until it is
/// freed; encode each with `lcr_encode_bytes` to get one symbol per message.
///
/// # Safety
///
/// `t` must be a live handle and `data` and `len` valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_cft_transfer_message(
    t: *const LcrCftTransfer,
    index: usize,
    data: *mut *const u8,
    len: *mut usize,
) -> LcrStatus {
    nonnull!(t, data, len);
    // SAFETY: checked non-null.
    let t = unsafe { &*t };
    let Some(m) = t.messages.get(index) else {
        return fail(
            LCR_ERR_ARGUMENT,
            format!(
                "message {index} out of range ({} messages)",
                t.messages.len()
            ),
        );
    };
    // SAFETY: checked non-null.
    unsafe {
        data.write(m.as_ptr());
        len.write(m.len());
    }
    LCR_OK
}

/// Chunks after the manifest. 1 when the whole file fits in the manifest itself. Zero for
/// null.
///
/// # Safety
///
/// `t` must be null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_cft_transfer_total_chunks(t: *const LcrCftTransfer) -> u32 {
    // SAFETY: the caller promises null or a live handle.
    unsafe { t.as_ref() }.map_or(0, |t| t.total_chunks)
}

/// The file's size before compression. Zero for null.
///
/// # Safety
///
/// `t` must be null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_cft_transfer_original_size(t: *const LcrCftTransfer) -> u64 {
    // SAFETY: the caller promises null or a live handle.
    unsafe { t.as_ref() }.map_or(0, |t| t.original_size)
}

/// The compressed stream's size. Zero for null.
///
/// # Safety
///
/// `t` must be null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_cft_transfer_compressed_size(t: *const LcrCftTransfer) -> u64 {
    // SAFETY: the caller promises null or a live handle.
    unsafe { t.as_ref() }.map_or(0, |t| t.compressed_size)
}

/// Where a receive has got to. One of the `LCR_CFT_*` values.
pub type LcrCftState = u32;
/// No manifest yet, so chunks cannot be counted. `held` says how many are waiting.
pub const LCR_CFT_AWAITING_MANIFEST: LcrCftState = 0;
/// Manifest in hand; `have` of `total` chunks arrived.
pub const LCR_CFT_RECEIVING: LcrCftState = 1;
/// Every chunk has arrived; call `lcr_cft_receiver_finish`.
pub const LCR_CFT_COMPLETE: LcrCftState = 2;

/// How much of a transfer has arrived.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LcrCftStatus {
    /// One of the `LCR_CFT_*` values.
    pub state: LcrCftState,
    /// Chunks held before the manifest arrived. `AWAITING_MANIFEST` only.
    pub held: u32,
    /// Chunks arrived. `RECEIVING` only.
    pub have: u32,
    /// Chunks in the transfer. `RECEIVING` only.
    pub total: u32,
}

fn cft_status_out(s: lcr_cft::Status) -> LcrCftStatus {
    let mut out = LcrCftStatus {
        state: LCR_CFT_COMPLETE,
        held: 0,
        have: 0,
        total: 0,
    };
    match s {
        lcr_cft::Status::AwaitingManifest { held } => {
            out.state = LCR_CFT_AWAITING_MANIFEST;
            out.held = u32::try_from(held).unwrap_or(u32::MAX);
        }
        lcr_cft::Status::Receiving { have, total } => {
            out.state = LCR_CFT_RECEIVING;
            out.have = have;
            out.total = total;
        }
        lcr_cft::Status::Complete => {}
    }
    out
}

/// Collects CFT messages in any order and reconstructs the file. Opaque; create with
/// `lcr_cft_receiver_new`, release with `lcr_cft_receiver_free`.
pub struct LcrCftReceiver(lcr_cft::Reassembler);

/// Create an empty receiver, with the default size limits (64 MiB).
#[unsafe(no_mangle)]
pub extern "C" fn lcr_cft_receiver_new() -> *mut LcrCftReceiver {
    Box::into_raw(Box::new(LcrCftReceiver(lcr_cft::Reassembler::default())))
}

/// Release a receiver. Null is ignored.
///
/// # Safety
///
/// `r` must be null or a live handle from `lcr_cft_receiver_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_cft_receiver_free(r: *mut LcrCftReceiver) {
    if !r.is_null() {
        // SAFETY: the caller promises a live handle from `Box::into_raw`.
        drop(unsafe { Box::from_raw(r) });
    }
}

/// Feed one decoded CFT message — manifest, chunk, or with the `fountain` feature a repair
/// symbol — in any order.
/// `out` may be null; otherwise it receives the status after this message.
///
/// # Safety
///
/// `r` must be a live handle, `bytes` readable for `len` bytes, `out` null or valid for
/// writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_cft_receiver_accept(
    r: *mut LcrCftReceiver,
    bytes: *const u8,
    len: usize,
    out: *mut LcrCftStatus,
) -> LcrStatus {
    nonnull!(r);
    status((|| {
        // SAFETY: checked non-null; the caller vouches for the rest.
        let (r, bytes) = unsafe { (&mut *r, borrow(bytes, len, "bytes")?) };
        let s = r.0.accept(bytes).map_err(failed)?;
        if !out.is_null() {
            // SAFETY: checked non-null.
            unsafe { out.write(cft_status_out(s)) };
        }
        Ok(())
    })())
}

/// The receiver's current status. A null handle reads as awaiting a manifest.
///
/// # Safety
///
/// `r` must be null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_cft_receiver_status(r: *const LcrCftReceiver) -> LcrCftStatus {
    // SAFETY: the caller promises null or a live handle.
    match unsafe { r.as_ref() } {
        Some(r) => cft_status_out(r.0.status()),
        None => cft_status_out(lcr_cft::Status::AwaitingManifest { held: 0 }),
    }
}

/// Fountain repair symbols held that have not yet resolved to a chunk. Zero for null.
///
/// # Safety
///
/// `r` must be null or a live handle.
#[cfg(feature = "fountain")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_cft_receiver_pending(r: *const LcrCftReceiver) -> u32 {
    // SAFETY: the caller promises null or a live handle.
    unsafe { r.as_ref() }.map_or(0, |r| u32::try_from(r.0.pending()).unwrap_or(u32::MAX))
}

/// Chunk indices still outstanding (CFT.md §10) — what a receiver shows so the sender
/// knows which symbols to display again.
///
/// Writes up to `cap` indices to `out` and returns how many are missing in total, so a
/// caller can size a buffer with a first call passing `cap = 0`. Zero for a null handle,
/// and zero before the manifest arrives, since until then nothing can be counted.
///
/// # Safety
///
/// `r` must be null or a live handle; `out` must be valid for `cap` writes, or may be null
/// when `cap` is zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_cft_receiver_missing(
    r: *const LcrCftReceiver,
    out: *mut u32,
    cap: usize,
) -> usize {
    // SAFETY: the caller promises null or a live handle.
    let Some(r) = (unsafe { r.as_ref() }) else {
        return 0;
    };
    let missing = r.0.missing();
    if !out.is_null() {
        let n = missing.len().min(cap);
        // SAFETY: the caller promises `cap` writable slots, and `n <= cap`.
        unsafe { ptr::copy_nonoverlapping(missing.as_ptr(), out, n) };
    }
    missing.len()
}

/// The manifest's filename, if there is a manifest and its name is safe to use as one.
/// Null otherwise. Release with `lcr_string_free`.
///
/// # Safety
///
/// `r` must be null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_cft_receiver_filename(r: *const LcrCftReceiver) -> *mut c_char {
    // SAFETY: the caller promises null or a live handle.
    unsafe { r.as_ref() }
        .and_then(|r| r.0.manifest())
        .and_then(|m| m.safe_filename())
        .and_then(|s| CString::new(s).ok())
        .map_or(ptr::null_mut(), CString::into_raw)
}

/// Bytes the manifest promised, for a progress readout. Zero before the manifest arrives.
///
/// # Safety
///
/// `r` must be null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_cft_receiver_original_size(r: *const LcrCftReceiver) -> u64 {
    // SAFETY: the caller promises null or a live handle.
    unsafe { r.as_ref() }
        .and_then(|r| r.0.manifest())
        .map_or(0, |m| m.original_size)
}

/// Reassemble, decompress, and verify against the manifest's hash. Release `out` with
/// `lcr_bytes_free`.
///
/// Succeeds only when the digest matches, so a caller cannot hand a user a file that did
/// not survive the trip.
///
/// # Safety
///
/// `r` must be a live handle and `out` valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lcr_cft_receiver_finish(
    r: *const LcrCftReceiver,
    out: *mut LcrBytes,
) -> LcrStatus {
    nonnull!(r, out);
    // SAFETY: checked non-null.
    match unsafe { &*r }.0.finish() {
        Ok(file) => {
            // SAFETY: checked non-null.
            unsafe { out.write(LcrBytes::from_vec(file)) };
            LCR_OK
        }
        Err(e) => failed(e),
    }
}
