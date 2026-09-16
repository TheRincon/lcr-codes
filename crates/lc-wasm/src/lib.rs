//! LC Code as a WebAssembly component (§14.2, §17.3).
//!
//! ## Why this is not a browser bundle
//!
//! §14.2 is written around a browser: `Uint8ClampedArray` from canvas `getImageData`, a
//! gzipped artifact served over the network. The consumer here is a **host application
//! embedding a wasm runtime** — Rust, Go, C or C++ — so the interface is declared in
//! `wit/lc.wit` against the Component Model, and the deliverable is a component rather
//! than a core module plus JS glue.
//!
//! That is a deviation from the spec, recorded as D8 in `docs/decisions.md`. It does not
//! weaken §17.3's "make embedding cheap" argument; it moves it. For a non-browser host a
//! typed, versioned WIT interface is a *better* answer than a JS-shaped ABI, because the
//! host's bindings are generated rather than hand-written against a private convention.
//!
//! ## Layering
//!
//! Everything in this module is ordinary Rust with no binding types in any signature, and
//! it is what the tests exercise. `component.rs` maps it to the generated WIT types and
//! compiles only for `wasm32`.
//!
//! That split is not tidiness. Binding types are often constructible only inside their
//! host runtime: the previous `wasm-bindgen` surface could not build a `JsError` outside a
//! JS engine, which made every failure branch untestable anywhere but a browser. Keeping
//! the fallible logic in plain Rust means error paths are testable on the host, and leaves
//! the binding layer thin enough to read.

use lc_core::{ContentType, Packet, Profile};
use lc_decode::image::{ImageView, PixelFormat};
use lc_decode::{Decoder, FrameAccumulator};

#[cfg(target_arch = "wasm32")]
mod component;

// QR generation for the density comparison. Off by default and in neither shipped
// artifact — see the `compare` feature in Cargo.toml.
#[cfg(feature = "compare")]
pub mod compare;

/// Crate version.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

// ---------------------------------------------------------------- errors

/// Everything that can go wrong on the Rust side of the boundary.
#[derive(Debug, Clone, PartialEq)]
pub enum WasmError {
    /// The §12 pipeline rejected the frame, or §8 rejected the payload.
    Lc(lc_core::LcError),
    /// `try_decode` before any frame was pushed.
    NoFrames,
    /// A module pitch below §13.1's normative floor of 8 device pixels per module.
    ModulePitchTooSmall(u32),
    /// A payload too large for any QR version, from the `compare` feature. Carries the
    /// byte count that did not fit.
    #[cfg(feature = "compare")]
    QrTooLong(u32),
}

impl core::fmt::Display for WasmError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            WasmError::Lc(e) => write!(f, "{e}"),
            WasmError::NoFrames => write!(f, "no frames pushed"),
            WasmError::ModulePitchTooSmall(n) => {
                write!(f, "module-px must be at least 8 (§13.1), got {n}")
            }
            #[cfg(feature = "compare")]
            WasmError::QrTooLong(n) => {
                write!(
                    f,
                    "{n} bytes exceeds the largest QR version at this ECC level"
                )
            }
        }
    }
}

impl From<lc_core::LcError> for WasmError {
    fn from(e: lc_core::LcError) -> WasmError {
        WasmError::Lc(e)
    }
}

// ---------------------------------------------------------------- decode types

/// A decoded packet, mirroring §14.3's `Packet`.
///
/// A variant rather than a struct with an optional body, so the §10.8 guarantee survives
/// the interface boundary: `Unverified` has no body to extract, and a `Partial` prefix
/// cannot be mistaken for one.
#[derive(Debug, Clone, PartialEq)]
pub enum DecodedPacket {
    /// Decoded and unsigned — the §1.2 common case.
    Unsigned { ctype: ContentType, body: Vec<u8> },
    /// Signature verified.
    Verified { ctype: ContentType, body: Vec<u8> },
    /// Well-formed COSE, not verified. No body, by construction (§10.8).
    Unverified,
    /// A luma-only prefix (§12.5). A diagnostic, not content (§8.1).
    Partial { prefix: Vec<u8> },
}

impl DecodedPacket {
    /// The body, for the two variants that have one.
    pub fn body(&self) -> Option<&[u8]> {
        match self {
            DecodedPacket::Unsigned { body, .. } | DecodedPacket::Verified { body, .. } => {
                Some(body)
            }
            _ => None,
        }
    }

    /// The body rendered as JSON, when it is CBOR.
    ///
    /// §9.4's boundary in the other direction: payloads travel as CBOR, hosts want JSON.
    pub fn as_json(&self) -> Option<String> {
        let (ctype, body) = match self {
            DecodedPacket::Unsigned { ctype, body } | DecodedPacket::Verified { ctype, body } => {
                (*ctype, body)
            }
            _ => return None,
        };
        if ctype != ContentType::Cbor {
            return None;
        }
        lc_core::cbor::Value::from_slice(body)
            .ok()
            .map(|v| lc_core::json::to_json(&v))
    }
}

/// Decode diagnostics (§14.3).
///
/// §14.3 keeps these because "colour codes fail for diagnosable reasons; callers need
/// them for user guidance". `snr_db` and `chroma_recovered` are what a scanning UI should
/// use to tell a user to move closer or tilt away from glare.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedDiagnostics {
    pub luma_iterations: u16,
    pub chroma_iterations: u16,
    pub chroma_recovered: bool,
    pub illum_residual_rms: f32,
    pub references_rejected: u8,
    pub snr_db: f32,
    pub frames_combined: u8,
    pub version: u8,
    pub profile: Profile,
    pub format_margin: f32,
    pub alignment_found: bool,
    pub capture_pitch_min: f32,
    pub capture_pitch_max: f32,
}

/// A successful decode.
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    pub packet: DecodedPacket,
    pub diagnostics: DecodedDiagnostics,
}

fn convert(out: lc_decode::DecodeOutput) -> Decoded {
    let d = out.diagnostics;
    let packet = match out.packet {
        Packet::Unsigned { body, ctype } => DecodedPacket::Unsigned { ctype, body },
        Packet::Verified { body, .. } => DecodedPacket::Verified {
            ctype: ContentType::Cbor,
            body,
        },
        Packet::Unverified { .. } => DecodedPacket::Unverified,
        Packet::Partial { prefix } => DecodedPacket::Partial { prefix },
        // `Packet` is `#[non_exhaustive]`. A variant added upstream must not silently
        // acquire a body here, so the fallback exposes nothing.
        _ => DecodedPacket::Unverified,
    };
    Decoded {
        packet,
        diagnostics: DecodedDiagnostics {
            luma_iterations: d.luma_iterations,
            chroma_iterations: d.chroma_iterations,
            chroma_recovered: d.chroma_recovered,
            illum_residual_rms: d.illum_residual_rms,
            references_rejected: d.references_rejected,
            snr_db: d.estimated_snr_db,
            frames_combined: d.frames_combined,
            version: d.format.version.get(),
            profile: d.format.profile,
            format_margin: d.format_margin,
            alignment_found: d.alignment_found,
            capture_pitch_min: d.capture_pitch_min,
            capture_pitch_max: d.capture_pitch_max,
        },
    }
}

// ---------------------------------------------------------------- decode

/// Decode one frame.
pub fn decode(
    pixels: &[u8],
    width: u32,
    height: u32,
    stride: u32,
    format: PixelFormat,
) -> Result<Decoded, WasmError> {
    let view = ImageView::new(pixels, width, height, stride, format)?;
    Ok(convert(Decoder::new().decode(&view)?))
}

/// Locate a symbol and measure how it was captured, without decoding it.
///
/// Succeeds whenever detection and the format region do, so it reports geometry for
/// frames whose payload does not decode — which is what separates "nothing in frame"
/// from "found it, move closer" in a scanning UI, and what lets a measurement bin its
/// failures.
pub fn probe(
    pixels: &[u8],
    width: u32,
    height: u32,
    stride: u32,
    format: PixelFormat,
) -> Result<Probed, WasmError> {
    let view = ImageView::new(pixels, width, height, stride, format)?;
    let p = Decoder::new().probe(&view)?;
    Ok(Probed {
        version: p.format.version.get(),
        profile: p.format.profile,
        format_margin: p.format_margin,
        alignment_found: p.alignment_found,
        illum_residual_rms: p.illum_residual_rms,
        references_rejected: p.references_rejected,
        capture_pitch_min: p.capture_pitch_min,
        capture_pitch_max: p.capture_pitch_max,
    })
}

/// What [`probe`] establishes about a capture.
#[derive(Debug, Clone, PartialEq)]
pub struct Probed {
    pub version: u8,
    pub profile: Profile,
    pub format_margin: f32,
    pub alignment_found: bool,
    pub illum_residual_rms: f32,
    pub references_rejected: u8,
    pub capture_pitch_min: f32,
    pub capture_pitch_max: f32,
}

/// Render a CBOR body as JSON text — the read side of §9.4.
///
/// The encoder converts JSON to CBOR because CBOR is 20–33% smaller on these shapes; a
/// scanner displaying a payload needs the inverse, and doing it here rather than in the
/// host keeps a second CBOR parser out of every embedder.
pub fn cbor_to_json(body: &[u8]) -> Result<String, WasmError> {
    let value = lc_core::cbor::Value::from_slice(body)?;
    Ok(lc_core::json::to_json(&value))
}

/// The exact bytes [`encode_json`] puts on the wire — §9.4's write side.
///
/// This is the same two steps `lc_encode::encode_json` performs before framing, so a
/// caller measuring what CBOR saves, or handing the wire bytes to another carrier for
/// comparison, gets what LC Code would really have transmitted rather than an
/// approximation of it.
pub fn json_to_cbor(json: &str) -> Result<Vec<u8>, WasmError> {
    Ok(lc_core::json::parse(json)?.to_vec())
}

/// Multi-frame LLR combining (§12.4), which the spec calls "the single best use of a live
/// camera preview": banding and glare move between frames, so accumulating soft
/// information across them is far cheaper than tightening the code rate.
#[derive(Default)]
pub struct Accumulator {
    inner: FrameAccumulator,
    decoder: Decoder,
}

impl Accumulator {
    pub fn new() -> Accumulator {
        Accumulator {
            inner: FrameAccumulator::new(),
            decoder: Decoder::new(),
        }
    }

    /// Add a frame.
    pub fn push(
        &mut self,
        pixels: &[u8],
        width: u32,
        height: u32,
        stride: u32,
        format: PixelFormat,
    ) -> Result<(), WasmError> {
        let view = ImageView::new(pixels, width, height, stride, format)?;
        self.inner.push(&view)?;
        Ok(())
    }

    /// How many frames have been combined.
    pub fn frames(&self) -> u8 {
        self.inner.frames()
    }

    /// Attempt a decode from everything pushed so far.
    pub fn try_decode(&self) -> Result<Decoded, WasmError> {
        if self.inner.frames() == 0 {
            return Err(WasmError::NoFrames);
        }
        Ok(convert(self.inner.try_decode(&self.decoder)?))
    }

    /// Discard accumulated frames.
    pub fn reset(&mut self) {
        self.inner.reset();
    }
}

// ---------------------------------------------------------------- encode

/// Encoding knobs, carried across the interface.
#[derive(Debug, Clone)]
pub struct EncodeOptions {
    pub profile: Profile,
    /// `None` picks the smallest version that fits (§6.1).
    pub version: Option<u8>,
    pub ecc_luma: u8,
    pub ecc_chroma: u8,
    /// `None` tries each scheme and keeps the smallest, which at these payload sizes is
    /// frequently no compression at all (§9.3).
    pub compression: Option<lc_core::Compression>,
    /// Emit the §6.4 screen reference border. Costs no payload capacity — it lives
    /// outside the symbol grid — and materially improves decoding under glare.
    pub reference_border: bool,
    /// `None` runs the §8.5 penalty search.
    pub mask: Option<u8>,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        EncodeOptions {
            profile: Profile::C1,
            version: None,
            ecc_luma: 1,
            ecc_chroma: 1,
            compression: None,
            reference_border: true,
            mask: None,
        }
    }
}

/// A rendered symbol.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    /// Row-major RGB triples, `width · height · 3` bytes.
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub version: u8,
    /// Grid side in modules, excluding the quiet zone.
    pub side: u32,
}

#[cfg(feature = "encode")]
mod enc {
    use super::{EncodeOptions, Rendered, WasmError};

    pub(super) fn options(o: &EncodeOptions) -> Result<lc_encode::EncodeOptions, WasmError> {
        Ok(lc_encode::EncodeOptions {
            profile: o.profile,
            version: match o.version {
                Some(v) => Some(lc_core::Version::new(v)?),
                None => None,
            },
            ecc: lc_core::EccLevel::new(o.ecc_luma, o.ecc_chroma)?,
            compression: o.compression,
            reference_border: o.reference_border,
            mask: o.mask,
        })
    }

    /// §13.1 point 3 makes 8 device pixels per module normative, and D6 records that the
    /// decoder now genuinely works at exactly 8. Below it a conforming reader is not
    /// expected to cope, so refusing beats emitting a symbol nothing can read.
    pub(super) fn pitch(module_px: u32) -> Result<u32, WasmError> {
        if module_px < 8 {
            return Err(WasmError::ModulePitchTooSmall(module_px));
        }
        Ok(module_px)
    }

    pub(super) fn opts(module_px: u32) -> lc_render::RenderOptions {
        lc_render::RenderOptions {
            module_px,
            ..lc_render::RenderOptions::screen()
        }
    }

    pub(super) fn render(sym: &lc_core::Symbol, module_px: u32) -> Rendered {
        let raster = lc_render::render_rgb(sym, &opts(module_px));
        Rendered {
            pixels: raster.data,
            width: raster.width,
            height: raster.height,
            version: sym.format.version.get(),
            side: sym.format.version.side() as u32,
        }
    }
}

/// Encode JSON, converting to CBOR first (§9.4).
#[cfg(feature = "encode")]
pub fn encode_json(json: &str, o: &EncodeOptions, module_px: u32) -> Result<Rendered, WasmError> {
    let px = enc::pitch(module_px)?;
    Ok(enc::render(
        &lc_encode::encode_json(json, &enc::options(o)?)?,
        px,
    ))
}

/// Encode UTF-8 text.
#[cfg(feature = "encode")]
pub fn encode_text(text: &str, o: &EncodeOptions, module_px: u32) -> Result<Rendered, WasmError> {
    let px = enc::pitch(module_px)?;
    Ok(enc::render(
        &lc_encode::encode_text(text, &enc::options(o)?)?,
        px,
    ))
}

/// Encode opaque bytes.
#[cfg(feature = "encode")]
pub fn encode_bytes(data: &[u8], o: &EncodeOptions, module_px: u32) -> Result<Rendered, WasmError> {
    let px = enc::pitch(module_px)?;
    Ok(enc::render(
        &lc_encode::encode_bytes(data, &enc::options(o)?)?,
        px,
    ))
}

/// Encode JSON and render to SVG rather than pixels.
#[cfg(feature = "encode")]
pub fn encode_json_svg(json: &str, o: &EncodeOptions, module_px: u32) -> Result<String, WasmError> {
    let px = enc::pitch(module_px)?;
    let sym = lc_encode::encode_json(json, &enc::options(o)?)?;
    Ok(lc_render::render_svg(&sym, &enc::opts(px)))
}
