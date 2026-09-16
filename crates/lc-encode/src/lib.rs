//! # lc-encode
//!
//! The §8 encoding pipeline, end to end:
//!
//! ```text
//! application data
//!   → §9  container framing (typed, compressed)
//!   → §8.1 layer split (luma-first)
//!   → §8.2 per-layer LDPC encode (luma rate ≠ chroma rate)
//!   → §8.3 interleave
//!   → §8.4 module mapping
//!   → §8.5 masking
//!   → §8.6 colourisation
//! ```
//!
//! ## Defaults follow §1.2, not §10
//!
//! An unsigned CBOR payload is the expected case, and nothing here charges it for
//! capability it does not use: [`EncodeOptions::default`] selects the smallest version
//! that fits, the compact frame, and no compression unless compression actually shrinks
//! the body. The v5 floor of §6.1 applies to *signed* packets and lives in
//! [`sign::SignOptions`], not in the general path.

#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

use alloc::vec::Vec;
use lc_core::bits::BitVec;
use lc_core::capacity::{self, SymbolCapacity};
use lc_core::ecc::ldpc::{CHROMA_RATE_INDEX, Code, LUMA_RATE_INDEX};
use lc_core::interleave::Interleaver;
use lc_core::layout::Layout;
use lc_core::modulate;
use lc_core::{
    Compression, ContentType, EccLevel, FormatInfo, Frame, LcError, Profile, Symbol, Version, mask,
};

/// Knobs for [`encode`].
#[derive(Clone, Debug)]
pub struct EncodeOptions {
    /// Colour configuration. Defaults to `C1`, the §5 screen default.
    pub profile: Profile,
    /// Force a version. `None` picks the smallest that fits (§6.1).
    pub version: Option<Version>,
    /// Per-layer ECC levels (§8.2).
    pub ecc: EccLevel,
    /// Force a compression scheme. `None` tries each and keeps whichever is smallest,
    /// including none — which §9.3 warns is often the winner at these sizes.
    pub compression: Option<Compression>,
    /// Emit the §6.4 screen reference border.
    pub reference_border: bool,
    /// Force a luma mask. `None` runs the §8.5 penalty search.
    pub mask: Option<u8>,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        EncodeOptions {
            profile: Profile::C1,
            version: None,
            ecc: EccLevel::DEFAULT,
            compression: None,
            // Mandatory as of 2026-09-13, where it used to be opt-in.
            //
            // §6.4 made the border optional because on print the overhead is unacceptable,
            // and on screen it is free — "it costs pixels nobody was using". Once the ring
            // also carries the position sequence (`border::position`) that reasoning
            // hardens into a requirement: the sequence is what a decoder localises from,
            // and a symbol that omits it is a symbol that cannot be found that way. QR
            // spends the same area on a quiet zone and gets nothing back for it.
            //
            // Print keeps the choice — `EncodeOptions::print` still turns it off — so this
            // is a screen-profile default, not a format-wide mandate.
            reference_border: true,
            mask: None,
        }
    }
}

impl EncodeOptions {
    /// Screen defaults: `C1`, reference border on (§6.4 costs pixels nobody was using).
    pub fn screen() -> EncodeOptions {
        EncodeOptions {
            profile: Profile::C1,
            reference_border: true,
            ..Default::default()
        }
    }

    /// Print defaults: `C2`, no reference border (§5, §13.3).
    pub fn print() -> EncodeOptions {
        EncodeOptions {
            profile: Profile::C2,
            reference_border: false,
            ..Default::default()
        }
    }

    /// Monochrome fallback for worst-case media.
    pub fn monochrome() -> EncodeOptions {
        EncodeOptions {
            profile: Profile::M,
            ..Default::default()
        }
    }
}

/// Encode a typed body into a symbol.
pub fn encode(data: &[u8], ctype: ContentType, opts: &EncodeOptions) -> Result<Symbol, LcError> {
    let frame = match opts.compression {
        None => Frame::compressed(ctype, data)?,
        Some(Compression::None) => Frame::new(ctype, data.to_vec())?,
        Some(c) => Frame {
            ctype,
            cflag: c,
            body: lc_core::frame::compress(c, data)?,
        },
    };
    encode_frame(&frame, opts)
}

/// Encode opaque bytes (`ctype = 0x0`).
pub fn encode_bytes(data: &[u8], opts: &EncodeOptions) -> Result<Symbol, LcError> {
    encode(data, ContentType::Opaque, opts)
}

/// Encode UTF-8 text (`ctype = 0x1`).
pub fn encode_text(text: &str, opts: &EncodeOptions) -> Result<Symbol, LcError> {
    encode(text.as_bytes(), ContentType::Text, opts)
}

/// Encode a CBOR body (`ctype = 0x3`).
pub fn encode_cbor(cbor: &[u8], opts: &EncodeOptions) -> Result<Symbol, LcError> {
    encode(cbor, ContentType::Cbor, opts)
}

/// Encode JSON by converting it to CBOR first (§9.4).
///
/// The conversion is the point: JSON on the wire carries key strings, punctuation and
/// whitespace that compress poorly at LC Code payload sizes. This is the API boundary
/// §9.4 asks for — author in JSON, transmit CBOR.
pub fn encode_json(json: &str, opts: &EncodeOptions) -> Result<Symbol, LcError> {
    let cbor = lc_core::json::parse(json)?.to_vec();
    encode(&cbor, ContentType::Cbor, opts)
}

/// Encode a §9.5 multipart body.
pub fn encode_multipart(
    parts: &[(ContentType, Vec<u8>)],
    opts: &EncodeOptions,
) -> Result<Symbol, LcError> {
    let body = lc_core::frame::multipart_encode(parts)?;
    encode(&body, ContentType::Multipart, opts)
}

/// Encode an already-built frame, running the full §8 pipeline.
pub fn encode_frame(frame: &Frame, opts: &EncodeOptions) -> Result<Symbol, LcError> {
    let bits = frame.to_bits()?;
    // The ring decides the layout (D17): with it, the finders and their company are gone and
    // that area is payload, so sizing has to use the layout the symbol will actually have.
    let version = match opts.version {
        Some(v) => v,
        None => capacity::smallest_version_for_symbol(
            frame.body.len(),
            opts.profile,
            opts.ecc,
            opts.reference_border,
        )?,
    };
    let layout = Layout::for_symbol(version, opts.profile, opts.reference_border);
    encode_bits(&bits, &layout, opts)
}

/// The §8.1–§8.6 core: framed bits in, symbol out.
fn encode_bits(bits: &BitVec, layout: &Layout, opts: &EncodeOptions) -> Result<Symbol, LcError> {
    let cap = layout.capacity();
    let luma_code = Code::new(LUMA_RATE_INDEX[opts.ecc.luma as usize & 3], cap.luma_bits)?;
    let chroma_code = if cap.chroma_bits == 0 {
        None
    } else {
        Some(Code::new(
            CHROMA_RATE_INDEX[opts.ecc.chroma as usize & 3],
            cap.chroma_bits,
        )?)
    };

    let k_luma = luma_code.info_bits();
    let k_chroma = chroma_code.as_ref().map_or(0, Code::info_bits);
    let available = k_luma + k_chroma;
    if bits.len() > available {
        return Err(LcError::Capacity {
            needed: bits.len(),
            available,
        });
    }

    // §8.1: luma-first. The luma layer holds a complete prefix of the packet, so a
    // decoder that recovers no chroma at all still recovers a prefix (§12.5).
    let take =
        |from: usize, n: usize| -> Vec<bool> { (0..n).map(|i| bits.get(from + i)).collect() };
    let luma_info = take(0, k_luma);
    let chroma_info = take(k_luma, k_chroma);

    let luma_cw = Interleaver::new(cap.luma_bits).interleave(&luma_code.encode(&luma_info)?);
    let chroma_cw = match &chroma_code {
        Some(c) => Interleaver::new(cap.chroma_bits).interleave(&c.encode(&chroma_info)?),
        None => Vec::new(),
    };

    // §8.5: pick the mask by penalty over the fully rendered luma layer — format region
    // included, because the detector sees that too.
    let make_format = |m: u8| FormatInfo {
        version: layout.version,
        profile: layout.profile,
        ecc: opts.ecc,
        mask: m,
        reference_border: opts.reference_border,
    };
    let render =
        |m: u8| -> Symbol { modulate::modulate(layout, make_format(m), &luma_cw, &chroma_cw) };

    let chosen = match opts.mask {
        Some(m) => m & 7,
        None => mask::choose_mask(|m| render(m).luma_grid()).0,
    };
    Ok(render(chosen))
}

/// Capacity of a version/profile/ECC combination, re-exported for sizing decisions.
pub fn capacity(version: Version, profile: Profile, ecc: EccLevel) -> SymbolCapacity {
    capacity::capacity(version, profile, ecc)
}

/// Capacity of the layout a symbol with these options actually uses (D17).
///
/// [`capacity`] is the classic, finder-bearing layout. With the §6.4 ring on, the symbol is
/// finder-less and holds more — size against this one.
pub fn capacity_for_symbol(
    version: Version,
    profile: Profile,
    ecc: EccLevel,
    reference_border: bool,
) -> SymbolCapacity {
    capacity::capacity_for_symbol(version, profile, ecc, reference_border)
}

pub mod sign;

#[cfg(test)]
mod tests {
    use super::*;
    use lc_core::modulate::demodulate;

    fn recover(sym: &Symbol) -> Result<Frame, LcError> {
        // Hard-decision inverse of the whole pipeline, for round-trip testing.
        let layout = sym.layout();
        let cap = layout.capacity();
        let (luma_cw, chroma_cw) = demodulate(&layout, sym);

        let luma_code = Code::new(LUMA_RATE_INDEX[sym.format.ecc.luma as usize], cap.luma_bits)?;
        let luma_llr: Vec<f32> = Interleaver::new(cap.luma_bits)
            .deinterleave(&luma_cw)
            .iter()
            .map(|&b| if b { -8.0 } else { 8.0 })
            .collect();
        let luma_bits = luma_code.decode(&luma_llr, 50)?.bits;

        let mut all = BitVec::new();
        for b in luma_bits {
            all.push(b);
        }
        if cap.chroma_bits > 0 {
            let cc = Code::new(
                CHROMA_RATE_INDEX[sym.format.ecc.chroma as usize],
                cap.chroma_bits,
            )?;
            let llr: Vec<f32> = Interleaver::new(cap.chroma_bits)
                .deinterleave(&chroma_cw)
                .iter()
                .map(|&b| if b { -8.0 } else { 8.0 })
                .collect();
            for b in cc.decode(&llr, 50)?.bits {
                all.push(b);
            }
        }
        Frame::from_bits(&all).map(|(f, _)| f)
    }

    #[test]
    fn roundtrip_across_profiles_and_versions() {
        for profile in Profile::ALL {
            for v in [1u8, 2, 5, 14, 32] {
                let version = Version::new(v).unwrap();
                let cap = capacity(version, profile, EccLevel::DEFAULT);
                let n = cap.body_bytes.min(400);
                let data: Vec<u8> = (0..n).map(|i| (i * 31 + 7) as u8).collect();
                let opts = EncodeOptions {
                    profile,
                    version: Some(version),
                    compression: Some(Compression::None),
                    ..Default::default()
                };
                let sym = encode_bytes(&data, &opts).unwrap();
                let frame = recover(&sym).unwrap_or_else(|e| panic!("v{v} {profile:?}: {e}"));
                assert_eq!(frame.body, data, "v{v} {profile:?}");
                assert_eq!(sym.format.version, version);
                assert_eq!(sym.format.profile, profile);
            }
        }
    }

    #[test]
    fn auto_version_selects_the_smallest_that_fits() {
        for n in [0usize, 1, 20, 100, 500, 2000] {
            let data = alloc::vec![0xA5u8; n];
            let opts = EncodeOptions {
                compression: Some(Compression::None),
                ..Default::default()
            };
            let sym = encode_bytes(&data, &opts).unwrap();
            let v = sym.format.version;
            // Default options carry the ring, so the symbol is finder-less (D17) and must
            // be sized against that layout, not the classic one.
            let cap =
                |v| capacity_for_symbol(v, Profile::C1, EccLevel::DEFAULT, opts.reference_border);
            assert!(cap(v).body_bytes >= n);
            if v.get() > 1 {
                let smaller = Version::new(v.get() - 1).unwrap();
                assert!(cap(smaller).body_bytes < n);
            }
        }
    }

    #[test]
    fn json_is_transmitted_as_cbor() {
        let sym = encode_json(r#"{"a":1,"b":[2,3]}"#, &EncodeOptions::default()).unwrap();
        let frame = recover(&sym).unwrap();
        assert_eq!(frame.ctype, ContentType::Cbor);
        let v = lc_core::cbor::Value::from_slice(&frame.payload().unwrap()).unwrap();
        assert_eq!(lc_core::json::to_json(&v), r#"{"a":1,"b":[2,3]}"#);
    }

    #[test]
    fn a_small_json_object_fits_a_tiny_symbol() {
        // §6.1's worked example.
        let sym = encode_json(
            r#"{"name":"Fred","loc":"45.00,55.0"}"#,
            &EncodeOptions::default(),
        )
        .unwrap();
        assert!(
            sym.format.version.get() <= 2,
            "needed v{}",
            sym.format.version.get()
        );
    }

    #[test]
    fn the_luma_prefix_is_recoverable_without_chroma() {
        // §8.1 / §12.5: chroma total loss must still leave a packet prefix.
        let data: Vec<u8> = (0..200u8).collect();
        let opts = EncodeOptions {
            compression: Some(Compression::None),
            ..Default::default()
        };
        let sym = encode_bytes(&data, &opts).unwrap();
        let layout = sym.layout();
        let cap = layout.capacity();
        let (luma_cw, _) = demodulate(&layout, &sym);
        let code = Code::new(LUMA_RATE_INDEX[1], cap.luma_bits).unwrap();
        let llr: Vec<f32> = Interleaver::new(cap.luma_bits)
            .deinterleave(&luma_cw)
            .iter()
            .map(|&b| if b { -8.0 } else { 8.0 })
            .collect();
        let bits = code.decode(&llr, 50).unwrap().bits;

        // The recovered luma bits must be a prefix of the framed packet.
        let full = Frame::new(ContentType::Opaque, data.clone())
            .unwrap()
            .to_bits()
            .unwrap();
        for (i, b) in bits.iter().take(full.len()).enumerate() {
            assert_eq!(*b, full.get(i), "luma prefix diverges at bit {i}");
        }
        assert!(
            bits.len() * 8 > full.len(),
            "luma layer should hold a meaningful prefix"
        );
    }

    #[test]
    fn masking_actually_changes_the_symbol() {
        let data = alloc::vec![0u8; 120];
        let mut seen = alloc::collections::BTreeSet::new();
        for m in 0..8u8 {
            let opts = EncodeOptions {
                mask: Some(m),
                compression: Some(Compression::None),
                ..Default::default()
            };
            let sym = encode_bytes(&data, &opts).unwrap();
            seen.insert(sym.modules.cells().to_vec());
            assert_eq!(sym.format.mask, m);
        }
        assert_eq!(seen.len(), 8, "masks produced duplicate symbols");
    }

    #[test]
    fn the_chosen_mask_beats_a_forced_bad_one_on_flat_data() {
        // All-zero payload is the pathological case masking exists for.
        let data = alloc::vec![0u8; 300];
        let auto = encode_bytes(
            &data,
            &EncodeOptions {
                compression: Some(Compression::None),
                ..Default::default()
            },
        )
        .unwrap();
        let auto_score = mask::penalty(&auto.luma_grid());
        for m in 0..8u8 {
            let forced = encode_bytes(
                &data,
                &EncodeOptions {
                    mask: Some(m),
                    compression: Some(Compression::None),
                    ..Default::default()
                },
            )
            .unwrap();
            assert!(
                auto_score <= mask::penalty(&forced.luma_grid()),
                "mask {m} beat the search"
            );
        }
    }

    #[test]
    fn oversized_payloads_are_rejected() {
        let data = alloc::vec![0u8; 50_000];
        assert!(matches!(
            encode_bytes(&data, &EncodeOptions::default()),
            Err(LcError::TooLarge(_))
        ));

        // Forcing too small a version is a capacity error, not a silent truncation.
        let opts = EncodeOptions {
            version: Some(Version::new(1).unwrap()),
            compression: Some(Compression::None),
            ..Default::default()
        };
        assert!(matches!(
            encode_bytes(&alloc::vec![0u8; 100], &opts),
            Err(LcError::Capacity { .. })
        ));
    }

    #[test]
    fn stronger_ecc_needs_a_bigger_symbol() {
        let data = alloc::vec![7u8; 150];
        let mut prev = 0u8;
        for level in 0..4u8 {
            let opts = EncodeOptions {
                ecc: EccLevel {
                    luma: level,
                    chroma: level,
                },
                compression: Some(Compression::None),
                ..Default::default()
            };
            let v = encode_bytes(&data, &opts).unwrap().format.version.get();
            assert!(v >= prev, "level {level} gave v{v} after v{prev}");
            prev = v;
        }
        assert!(
            prev > encode_bytes(
                &data,
                &EncodeOptions {
                    ecc: EccLevel { luma: 0, chroma: 0 },
                    compression: Some(Compression::None),
                    ..Default::default()
                }
            )
            .unwrap()
            .format
            .version
            .get()
        );
    }

    #[test]
    fn compression_is_recorded_in_the_frame() {
        let data = alloc::vec![b'x'; 1500];
        let sym = encode_bytes(&data, &EncodeOptions::default()).unwrap();
        let frame = recover(&sym).unwrap();
        assert_ne!(frame.cflag, Compression::None);
        assert_eq!(frame.payload().unwrap(), data);
        // And it bought a much smaller symbol.
        let raw = encode_bytes(
            &data,
            &EncodeOptions {
                compression: Some(Compression::None),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(sym.format.version < raw.format.version);
    }

    #[test]
    fn empty_payload_encodes() {
        let sym = encode_bytes(&[], &EncodeOptions::default()).unwrap();
        assert_eq!(recover(&sym).unwrap().body, Vec::<u8>::new());
    }
}
