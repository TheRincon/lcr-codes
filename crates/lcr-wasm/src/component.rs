//! The Component Model binding layer.
//!
//! Deliberately thin: every function here converts WIT types to the plain Rust in
//! `lib.rs`, calls it, and converts back. Nothing decides anything. That is what keeps
//! the logic testable on the host, where these generated types do not exist.

#![allow(clippy::needless_pass_by_value)]

// Two worlds, selected by feature. §14.1 keeps the encoder out of a scanner build so it
// cannot be acquired by accident; `scanner` exports only `decoder` to match.
// Three worlds now. `compare` adds QR generation for the demo build and is checked first,
// because it is a superset of `lcr-code` and both would otherwise match.
#[cfg(feature = "compare")]
wit_bindgen::generate!({
    world: "lcr-code-compare",
    path: "wit",
    generate_all,
});

#[cfg(all(feature = "encode", not(feature = "compare")))]
wit_bindgen::generate!({
    world: "lcr-code",
    path: "wit",
    generate_all,
});

#[cfg(not(feature = "encode"))]
wit_bindgen::generate!({
    world: "scanner",
    path: "wit",
    generate_all,
});

use crate::{DecodedPacket, WasmError};
// The shared `types` interface generates once, outside the per-export modules, because
// both `decoder` and `encoder` `use` it.
use exports::lcr::code::decoder::{Guest as DecoderGuest, GuestAccumulator};
use lcr::code::types::{
    ContentType as WitContentType, DecodeResult, Diagnostics as WitDiagnostics, Image,
    Packet as WitPacket, Payload, PixelFormat as WitFormat, ProbeResult, Profile as WitProfile,
};

struct Component;

// ---------------------------------------------------------------- conversions

fn format(f: WitFormat) -> lcr_decode::image::PixelFormat {
    use lcr_decode::image::PixelFormat as P;
    match f {
        WitFormat::Gray8 => P::Gray8,
        WitFormat::Rgb8 => P::Rgb8,
        WitFormat::Rgba8 => P::Rgba8,
    }
}

fn profile_out(p: lcr_core::Profile) -> WitProfile {
    match p {
        lcr_core::Profile::M => WitProfile::Mono,
        lcr_core::Profile::C1 => WitProfile::C1,
        lcr_core::Profile::C2 => WitProfile::C2,
        lcr_core::Profile::C3 => WitProfile::C3,
    }
}

fn ctype_out(c: lcr_core::ContentType) -> WitContentType {
    use WitContentType as W;
    match c {
        lcr_core::ContentType::Opaque => W::Opaque,
        lcr_core::ContentType::Text => W::Text,
        lcr_core::ContentType::Json => W::Json,
        lcr_core::ContentType::Cbor => W::Cbor,
        lcr_core::ContentType::CoseSign1 => W::CoseSign1,
        lcr_core::ContentType::Multipart => W::Multipart,
    }
}

fn decoded_out(d: crate::Decoded) -> DecodeResult {
    let packet = match d.packet {
        DecodedPacket::Unsigned { ctype, body } => WitPacket::Unsigned(Payload {
            ctype: ctype_out(ctype),
            body,
        }),
        DecodedPacket::Verified { ctype, body } => WitPacket::Verified(Payload {
            ctype: ctype_out(ctype),
            body,
        }),
        DecodedPacket::Unverified => WitPacket::Unverified,
        DecodedPacket::Partial { prefix } => WitPacket::Partial(prefix),
    };
    let g = d.diagnostics;
    DecodeResult {
        packet,
        diagnostics: WitDiagnostics {
            luma_iterations: g.luma_iterations,
            chroma_iterations: g.chroma_iterations,
            chroma_recovered: g.chroma_recovered,
            illum_residual_rms: g.illum_residual_rms,
            references_rejected: g.references_rejected,
            snr_db: g.snr_db,
            frames_combined: g.frames_combined,
            version: g.version,
            profile: profile_out(g.profile),
            format_margin: g.format_margin,
            alignment_found: g.alignment_found,
            capture_pitch_min: g.capture_pitch_min,
            capture_pitch_max: g.capture_pitch_max,
        },
    }
}

fn err(e: WasmError) -> String {
    e.to_string()
}

// ---------------------------------------------------------------- decoder

impl DecoderGuest for Component {
    type Accumulator = Acc;

    fn decode(img: Image) -> Result<DecodeResult, String> {
        crate::decode(
            &img.pixels,
            img.width,
            img.height,
            img.stride,
            format(img.format),
        )
        .map(decoded_out)
        .map_err(err)
    }

    fn probe(img: Image) -> Result<ProbeResult, String> {
        crate::probe(
            &img.pixels,
            img.width,
            img.height,
            img.stride,
            format(img.format),
        )
        .map(|p| ProbeResult {
            version: p.version,
            profile: profile_out(p.profile),
            format_margin: p.format_margin,
            alignment_found: p.alignment_found,
            illum_residual_rms: p.illum_residual_rms,
            references_rejected: p.references_rejected,
            capture_pitch_min: p.capture_pitch_min,
            capture_pitch_max: p.capture_pitch_max,
        })
        .map_err(err)
    }
}

/// Interior mutability because WIT resource methods take `&self`, while the underlying
/// accumulator needs `&mut` to fold a frame in.
struct Acc(core::cell::RefCell<crate::Accumulator>);

impl GuestAccumulator for Acc {
    fn new() -> Self {
        Acc(core::cell::RefCell::new(crate::Accumulator::new()))
    }

    fn push(&self, img: Image) -> Result<(), String> {
        self.0
            .borrow_mut()
            .push(
                &img.pixels,
                img.width,
                img.height,
                img.stride,
                format(img.format),
            )
            .map_err(err)
    }

    fn frames(&self) -> u8 {
        self.0.borrow().frames()
    }

    fn try_decode(&self) -> Result<DecodeResult, String> {
        self.0.borrow().try_decode().map(decoded_out).map_err(err)
    }

    fn reset(&self) {
        self.0.borrow_mut().reset();
    }
}

// ---------------------------------------------------------------- codec

/// Gated on `encode` because that is the feature selecting the `lcr-code` world, which is
/// the only world exporting `codec`. The dependency is on the *world*, not on encoding —
/// see the interface's own note in `wit/lcr.wit` for why it is not in the scanner.
#[cfg(feature = "encode")]
mod codec_impl {
    use super::exports;
    use super::{Component, err};
    use exports::lcr::code::codec::Guest as CodecGuest;

    impl CodecGuest for Component {
        fn json_to_cbor(json: String) -> Result<Vec<u8>, String> {
            crate::json_to_cbor(&json).map_err(err)
        }

        fn cbor_to_json(body: Vec<u8>) -> Result<String, String> {
            crate::cbor_to_json(&body).map_err(err)
        }
    }
}

// ---------------------------------------------------------------- cft

/// Chunked file transfer (docs/CFT.md). Gated like `codec`: on the *world*, not on
/// encoding — see the interface note in `wit/lcr.wit`.
#[cfg(feature = "encode")]
mod cft_impl {
    use super::Component;
    use super::exports;
    use core::cell::RefCell;
    use exports::lcr::code::cft::{
        Guest as CftGuest, GuestReceiver, Progress, Status as WitStatus, Transfer as WitTransfer,
    };
    use lcr_cft::{Compression, Reassembler, Status};

    /// **Deflate, not Zstandard.** `zstd-sys` compiles C and there is no C toolchain for
    /// `wasm32-unknown-unknown`, so the browser build uses the pure-Rust codec. The
    /// manifest records the choice, so a receiver is never guessing.
    pub(super) const CODEC: Compression = Compression::Deflate;

    fn status_out(s: Status) -> WitStatus {
        match s {
            Status::AwaitingManifest { held } => WitStatus::AwaitingManifest(held as u32),
            Status::Receiving { have, total } => WitStatus::Receiving(Progress { have, total }),
            Status::Complete => WitStatus::Complete,
        }
    }

    impl CftGuest for Component {
        type Receiver = Rx;

        fn split(
            file: Vec<u8>,
            filename: String,
            transfer_id: Vec<u8>,
            chunk_size: u32,
        ) -> Result<WitTransfer, String> {
            let name = (!filename.is_empty()).then_some(filename);
            let t = lcr_cft::split(&file, name, transfer_id, CODEC, chunk_size)
                .map_err(|e| e.to_string())?;
            Ok(WitTransfer {
                messages: t.messages(),
                total_chunks: t.manifest.total_chunks,
                original_size: t.manifest.original_size,
                compressed_size: t.manifest.compressed_size,
            })
        }
    }

    pub struct Rx(pub(super) RefCell<Reassembler>);

    impl GuestReceiver for Rx {
        fn new() -> Self {
            Rx(RefCell::new(Reassembler::default()))
        }

        fn accept(&self, bytes: Vec<u8>) -> Result<WitStatus, String> {
            self.0
                .borrow_mut()
                .accept(&bytes)
                .map(status_out)
                .map_err(|e| e.to_string())
        }

        fn status(&self) -> WitStatus {
            status_out(self.0.borrow().status())
        }

        fn missing(&self) -> Vec<u32> {
            self.0.borrow().missing()
        }

        fn filename(&self) -> Option<String> {
            self.0
                .borrow()
                .manifest()
                .and_then(|m| m.safe_filename().map(String::from))
        }

        fn original_size(&self) -> u64 {
            self.0.borrow().manifest().map_or(0, |m| m.original_size)
        }

        fn finish(&self) -> Result<Vec<u8>, String> {
            self.0.borrow().finish().map_err(|e| e.to_string())
        }
    }
}

// ---------------------------------------------------------------- cft-fountain

/// Fountain transfers (docs/CFT.md §21). The demo world only — see the interface note in
/// `wit/lcr.wit` — so it rides on `compare`, which is what builds that world.
#[cfg(feature = "compare")]
mod cft_fountain_impl {
    use super::Component;
    use super::cft_impl::{CODEC, Rx};
    use super::exports;
    use exports::lcr::code::cft::ReceiverBorrow;
    use exports::lcr::code::cft_fountain::{
        Fountain as WitFountain, Guest as FountainGuest, GuestFountain,
    };
    use lcr_cft::Fountain;

    impl FountainGuest for Component {
        type Fountain = Tx;

        fn start_fountain(
            file: Vec<u8>,
            filename: String,
            transfer_id: Vec<u8>,
            chunk_size: u32,
        ) -> Result<WitFountain, String> {
            let name = (!filename.is_empty()).then_some(filename);
            let t = lcr_cft::split(&file, name, transfer_id, CODEC, chunk_size)
                .map_err(|e| e.to_string())?;
            Ok(WitFountain::new(Tx(Fountain::new(t))))
        }

        fn pending(rx: ReceiverBorrow<'_>) -> u32 {
            rx.get::<Rx>().0.borrow().pending() as u32
        }
    }

    pub struct Tx(Fountain);

    impl GuestFountain for Tx {
        fn frame(&self, n: u64) -> Vec<u8> {
            self.0.frame(n)
        }

        fn source_count(&self) -> u32 {
            self.0.source_count()
        }

        fn original_size(&self) -> u64 {
            self.0.transfer().manifest.original_size
        }

        fn compressed_size(&self) -> u64 {
            self.0.transfer().manifest.compressed_size
        }
    }
}

// ---------------------------------------------------------------- encoder

#[cfg(feature = "encode")]
mod encoder_impl {
    use super::exports;
    use super::lcr;
    use super::{Component, err};
    use exports::lcr::code::encoder::{Guest as EncoderGuest, Options, Rendered as WitRendered};
    use lcr::code::types::{Compression as WitCompression, Profile as WitProfile};

    fn profile_in(p: WitProfile) -> lcr_core::Profile {
        match p {
            WitProfile::Mono => lcr_core::Profile::M,
            WitProfile::C1 => lcr_core::Profile::C1,
            WitProfile::C2 => lcr_core::Profile::C2,
            WitProfile::C3 => lcr_core::Profile::C3,
        }
    }

    fn compression_in(c: WitCompression) -> lcr_core::Compression {
        match c {
            WitCompression::None => lcr_core::Compression::None,
            WitCompression::Deflate => lcr_core::Compression::Deflate,
            WitCompression::Brotli => lcr_core::Compression::Brotli,
        }
    }

    fn options_in(o: Options) -> crate::EncodeOptions {
        crate::EncodeOptions {
            profile: profile_in(o.profile),
            version: o.version,
            ecc_luma: o.ecc_luma,
            ecc_chroma: o.ecc_chroma,
            compression: o.compression.map(compression_in),
            reference_border: o.reference_border,
            mask: o.mask,
        }
    }

    fn rendered_out(r: crate::Rendered) -> WitRendered {
        WitRendered {
            pixels: r.pixels,
            width: r.width,
            height: r.height,
            version: r.version,
            side: r.side,
        }
    }

    impl EncoderGuest for Component {
        fn capacity(
            profile: WitProfile,
            version: u8,
            ecc_luma: u8,
            ecc_chroma: u8,
            reference_border: bool,
        ) -> Result<u32, String> {
            let version = lcr_core::Version::new(version).map_err(|e| e.to_string())?;
            let ecc = lcr_core::EccLevel::new(ecc_luma, ecc_chroma).map_err(|e| e.to_string())?;
            let cap = lcr_encode::capacity_for_symbol(
                version,
                profile_in(profile),
                ecc,
                reference_border,
            );
            Ok(cap.body_bytes as u32)
        }

        fn encode_json(json: String, opts: Options, module_px: u32) -> Result<WitRendered, String> {
            crate::encode_json(&json, &options_in(opts), module_px)
                .map(rendered_out)
                .map_err(err)
        }

        fn encode_text(text: String, opts: Options, module_px: u32) -> Result<WitRendered, String> {
            crate::encode_text(&text, &options_in(opts), module_px)
                .map(rendered_out)
                .map_err(err)
        }

        fn encode_bytes(
            data: Vec<u8>,
            opts: Options,
            module_px: u32,
        ) -> Result<WitRendered, String> {
            crate::encode_bytes(&data, &options_in(opts), module_px)
                .map(rendered_out)
                .map_err(err)
        }

        fn encode_json_svg(json: String, opts: Options, module_px: u32) -> Result<String, String> {
            crate::encode_json_svg(&json, &options_in(opts), module_px).map_err(err)
        }
    }
}

// ---------------------------------------------------------------- compare

/// QR generation, demo build only. See the `compare` interface in wit/lcr.wit.
#[cfg(feature = "compare")]
mod cmp {
    use super::{Component, err};
    use crate::compare::QrEcc;
    use exports::lcr::code::compare::{Guest as CompareGuest, QrEcc as WitQrEcc, QrSymbol};

    use super::*;

    fn ecc_in(e: WitQrEcc) -> QrEcc {
        match e {
            WitQrEcc::Low => QrEcc::Low,
            WitQrEcc::Medium => QrEcc::Medium,
            WitQrEcc::Quartile => QrEcc::Quartile,
            WitQrEcc::High => QrEcc::High,
        }
    }

    impl CompareGuest for Component {
        fn qr_encode(data: Vec<u8>, ecl: WitQrEcc) -> Result<QrSymbol, String> {
            let payload_bytes = data.len() as u32;
            crate::compare::qr_encode(&data, ecc_in(ecl))
                .map(|s| QrSymbol {
                    modules: s.modules,
                    size: s.size,
                    version: s.version,
                    // A constant in `lcr-render`, since ISO/IEC 18004 §6.3.8 fixes it at 4
                    // and a QR decoder in the wild genuinely fails without it.
                    quiet_zone: lcr_render::qr::QR_QUIET,
                    payload_bytes,
                })
                .map_err(err)
        }
    }
}

export!(Component);
