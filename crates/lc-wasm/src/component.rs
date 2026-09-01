//! The Component Model binding layer.
//!
//! Deliberately thin: every function here converts WIT types to the plain Rust in
//! `lib.rs`, calls it, and converts back. Nothing decides anything. That is what keeps
//! the logic testable on the host, where these generated types do not exist.

#![allow(clippy::needless_pass_by_value)]

// Two worlds, selected by feature. §14.1 keeps the encoder out of a scanner build so it
// cannot be acquired by accident; `scanner` exports only `decoder` to match.
#[cfg(feature = "encode")]
wit_bindgen::generate!({
    world: "lc-code",
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
use exports::lc::code::decoder::{Guest as DecoderGuest, GuestAccumulator};
use lc::code::types::{
    ContentType as WitContentType, DecodeResult, Diagnostics as WitDiagnostics, Image,
    Packet as WitPacket, Payload, PixelFormat as WitFormat, Profile as WitProfile,
};

struct Component;

// ---------------------------------------------------------------- conversions

fn format(f: WitFormat) -> lc_decode::image::PixelFormat {
    use lc_decode::image::PixelFormat as P;
    match f {
        WitFormat::Gray8 => P::Gray8,
        WitFormat::Rgb8 => P::Rgb8,
        WitFormat::Rgba8 => P::Rgba8,
    }
}

fn profile_out(p: lc_core::Profile) -> WitProfile {
    match p {
        lc_core::Profile::M => WitProfile::Mono,
        lc_core::Profile::C1 => WitProfile::C1,
        lc_core::Profile::C2 => WitProfile::C2,
        lc_core::Profile::C3 => WitProfile::C3,
    }
}

fn ctype_out(c: lc_core::ContentType) -> WitContentType {
    use WitContentType as W;
    match c {
        lc_core::ContentType::Opaque => W::Opaque,
        lc_core::ContentType::Text => W::Text,
        lc_core::ContentType::Json => W::Json,
        lc_core::ContentType::Cbor => W::Cbor,
        lc_core::ContentType::CoseSign1 => W::CoseSign1,
        lc_core::ContentType::Multipart => W::Multipart,
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

// ---------------------------------------------------------------- encoder

#[cfg(feature = "encode")]
mod encoder_impl {
    use super::exports;
    use super::lc;
    use super::{Component, err};
    use exports::lc::code::encoder::{Guest as EncoderGuest, Options, Rendered as WitRendered};
    use lc::code::types::{Compression as WitCompression, Profile as WitProfile};

    fn profile_in(p: WitProfile) -> lc_core::Profile {
        match p {
            WitProfile::Mono => lc_core::Profile::M,
            WitProfile::C1 => lc_core::Profile::C1,
            WitProfile::C2 => lc_core::Profile::C2,
            WitProfile::C3 => lc_core::Profile::C3,
        }
    }

    fn compression_in(c: WitCompression) -> lc_core::Compression {
        match c {
            WitCompression::None => lc_core::Compression::None,
            WitCompression::Deflate => lc_core::Compression::Deflate,
            WitCompression::Brotli => lc_core::Compression::Brotli,
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

export!(Component);
