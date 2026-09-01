//! # lc-decode
//!
//! The §12 decoding pipeline. Only the output is normative, but the *ordering* is
//! load-bearing and this crate follows it exactly:
//!
//! ```text
//! §12.1  luma extraction, finder detection, homography   ← no colour touched yet
//! §12.2  format region: soft BCH decode, both copies
//! §12.3  illumination model fit
//! §12.4  soft demodulation to LLRs
//! §12.5  layered LDPC decode, luma first
//! §9.1   frame parse and CRC — the authoritative accept
//! §10    signature verification, only if a verifier was configured
//! ```
//!
//! ## The default decode path involves no cryptography at all
//!
//! [`Decoder::new`] pulls in no key material, no clock, and no signature code. A
//! `Decoder<NoKeys>` that meets a signed packet returns
//! `Packet::Unverified { reason: NoKeysConfigured, .. }`: it decoded fine, but this
//! decoder was not built to make trust decisions and will not pretend otherwise.
//!
//! ## Deviation from §14.4: `with_verifier`, not `with_keys`
//!
//! §14.4 sketches `Decoder::with_keys(keys, now)` and a `KeyResolver` trait at the decode
//! boundary. That shape drags key material, a clock, a `PublicKey` type and an algorithm
//! enum into `lc-decode`, which §14.2 wants free of crypto and which §14.1 wants
//! shippable without `lc-verify` at all.
//!
//! Inverted here: `lc-decode` knows only [`PacketVerifier`], which takes COSE bytes and
//! returns a [`Packet`]. `lc-verify` owns the `KeyResolver`, the clock and the algorithms,
//! and implements this one trait. The observable behaviour of §14.4 is unchanged and the
//! scanner bundle keeps a hard, checkable boundary: if crypto appears in the wasm
//! dependency graph, something is wired wrong.

#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

pub mod demod;
pub mod detect;
pub mod error;
pub mod illum;
pub mod image;
pub mod math;
pub mod sample;
pub mod transform;

use alloc::vec::Vec;
use demod::{ChannelStats, Demodulation};
use detect::Detection;
use illum::IlluminationModel;
use image::{ImageView, PixelFormat};
use lc_core::bch;
use lc_core::bits::BitVec;
use lc_core::ecc::ldpc::{CHROMA_RATE_INDEX, Code, LUMA_RATE_INDEX, MAX_ITERATIONS};
use lc_core::interleave::Interleaver;
use lc_core::layout::Layout;
use lc_core::palette::Palette;
use lc_core::{ContentType, FormatInfo, Frame, LcError, Packet, Profile, VerifyFailure, Version};
use transform::Transform;

pub use error::DecodeError;

/// Hands a COSE_Sign1 body to a verifier (§10).
///
/// Implemented by `lc-verify`. `lc-decode` deliberately knows nothing about keys,
/// algorithms or clocks — see the deviation note in the crate docs.
pub trait PacketVerifier {
    /// Verify a COSE_Sign1 body and turn it into a packet.
    ///
    /// Must return [`Packet::Unverified`] rather than an error on any failure, so a
    /// scanner can still report *why* to the user. §10.8 requires failing closed:
    /// unknown `kid`, unknown `alg`, absent `exp`, expired or malformed COSE must all
    /// come back as `Unverified`, never as content.
    fn verify(&self, cose_sign1: &[u8]) -> Packet;
}

/// The default verifier: refuses to make trust decisions.
#[derive(Copy, Clone, Debug, Default)]
pub struct NoKeys;

impl PacketVerifier for NoKeys {
    fn verify(&self, _cose_sign1: &[u8]) -> Packet {
        Packet::Unverified {
            reason: VerifyFailure::NoKeysConfigured,
            kid: [0; 4],
        }
    }
}

/// Why a decode failed, in terms a scanner can turn into user guidance (§14.3).
#[derive(Clone, Debug)]
pub struct Diagnostics {
    /// LDPC iterations spent on the luma layer.
    pub luma_iterations: u16,
    /// LDPC iterations spent on the chroma layer. Zero when chroma was never attempted.
    pub chroma_iterations: u16,
    /// Whether the chroma layer decoded.
    pub chroma_recovered: bool,
    /// RMS residual of the §12.3 fit, in linear light.
    pub illum_residual_rms: f32,
    /// References discarded by the §12.3 robust refit.
    pub references_rejected: u8,
    /// Per-module luma SNR, measured from the symbol's own known-colour modules.
    pub estimated_snr_db: f32,
    /// Frames combined before decoding (§12.4).
    pub frames_combined: u8,
    /// The format region as read.
    pub format: FormatInfo,
    /// Confidence margin of the winning format copy, from the soft BCH decode.
    pub format_margin: f32,
    /// Whether an alignment marker was found, and therefore whether the transform is a
    /// full homography or an affine fallback.
    pub alignment_found: bool,
}

/// A successful decode.
#[derive(Clone, Debug)]
pub struct DecodeOutput {
    /// The packet.
    pub packet: Packet,
    /// How the decode went.
    pub diagnostics: Diagnostics,
}

/// Decoder tuning.
#[derive(Clone, Debug)]
pub struct DecodeOptions {
    /// LDPC iteration cap per layer.
    pub max_iterations: u16,
    /// Return a luma-only prefix as [`Packet::Partial`] when the chroma layer fails
    /// (§12.5).
    ///
    /// Off by default. §9.1 makes the CRC the authoritative accept for decoding, and a
    /// prefix has no CRC to check; §8.1 forbids acting on unverified partial data at all
    /// for signed packets, which this honours by never producing `Partial` for one.
    pub allow_partial: bool,
    /// Force a profile instead of believing the format region. Diagnostics only.
    pub force_profile: Option<Profile>,
}

impl Default for DecodeOptions {
    fn default() -> Self {
        DecodeOptions {
            max_iterations: MAX_ITERATIONS,
            allow_partial: false,
            force_profile: None,
        }
    }
}

/// The decoder.
#[derive(Clone, Debug, Default)]
pub struct Decoder<V = NoKeys> {
    verifier: V,
    options: DecodeOptions,
}

impl Decoder<NoKeys> {
    /// A decoder that decodes but will not verify signatures.
    pub fn new() -> Decoder<NoKeys> {
        Decoder {
            verifier: NoKeys,
            options: DecodeOptions::default(),
        }
    }
}

impl<V: PacketVerifier> Decoder<V> {
    /// Opt in to signature verification by supplying something that can do it (§10).
    pub fn with_verifier<W: PacketVerifier>(self, verifier: W) -> Decoder<W> {
        Decoder {
            verifier,
            options: self.options,
        }
    }

    /// Replace the tuning options.
    pub fn with_options(mut self, options: DecodeOptions) -> Self {
        self.options = options;
        self
    }

    /// Decode from a packed RGB8 buffer.
    pub fn decode_rgb(
        &self,
        px: &[u8],
        w: u32,
        h: u32,
        stride: u32,
    ) -> Result<DecodeOutput, DecodeError> {
        self.decode(&ImageView::new(px, w, h, stride, PixelFormat::Rgb8)?)
    }

    /// Decode from an RGBA8 buffer, as `canvas.getImageData` produces (§14.2).
    pub fn decode_rgba(
        &self,
        px: &[u8],
        w: u32,
        h: u32,
        stride: u32,
    ) -> Result<DecodeOutput, DecodeError> {
        self.decode(&ImageView::new(px, w, h, stride, PixelFormat::Rgba8)?)
    }

    /// Decode from a greyscale buffer.
    ///
    /// Chroma is unavailable, so only the luma layer decodes — which for a colour profile
    /// yields a packet prefix rather than a packet, unless
    /// [`DecodeOptions::allow_partial`] is set. §1.1 goal 3 requires this to degrade
    /// rather than fail outright, and a `M`-profile symbol decodes completely.
    pub fn decode_gray(
        &self,
        px: &[u8],
        w: u32,
        h: u32,
        stride: u32,
    ) -> Result<DecodeOutput, DecodeError> {
        self.decode(&ImageView::new(px, w, h, stride, PixelFormat::Gray8)?)
    }

    /// Decode from any [`ImageView`].
    pub fn decode(&self, img: &ImageView) -> Result<DecodeOutput, DecodeError> {
        let prepared = prepare(img)?;
        let demodulation = prepared.demodulate(img);
        self.finish(&prepared, &demodulation, 1)
    }

    /// Turn demodulated LLRs into a packet: LDPC, frame, CRC, then optionally verify.
    fn finish(
        &self,
        p: &Prepared,
        d: &Demodulation,
        frames: u8,
    ) -> Result<DecodeOutput, DecodeError> {
        let cap = p.layout.capacity();
        let ecc = p.format.ecc;

        // §12.5 step 1: the luma layer, independent of all colour processing.
        let mut luma_llrs = d.luma.clone();
        demod::unmask_luma(&mut luma_llrs, &p.layout, p.format.mask);
        let luma_code = Code::new(LUMA_RATE_INDEX[ecc.luma as usize & 3], cap.luma_bits)?;
        let luma_out = luma_code.decode(
            &Interleaver::new(cap.luma_bits).deinterleave(&luma_llrs),
            self.options.max_iterations,
        )?;

        // §12.5 step 2: chroma, if it is there at all.
        let mut chroma_out = None;
        if !d.chroma.is_empty() && cap.chroma_bits > 0 {
            let code = Code::new(CHROMA_RATE_INDEX[ecc.chroma as usize & 3], cap.chroma_bits)?;
            let out = code.decode(
                &Interleaver::new(cap.chroma_bits).deinterleave(&d.chroma),
                self.options.max_iterations,
            )?;
            chroma_out = Some(out);
        }

        let mut diagnostics = Diagnostics {
            luma_iterations: luma_out.iterations,
            chroma_iterations: chroma_out.as_ref().map_or(0, |c| c.iterations),
            chroma_recovered: chroma_out.as_ref().is_some_and(|c| c.converged),
            illum_residual_rms: p.illum.residual_rms,
            references_rejected: p.illum.rejected.min(255) as u8,
            estimated_snr_db: d.stats.snr_db(),
            frames_combined: frames,
            format: p.format,
            format_margin: p.format_margin,
            alignment_found: p.alignment_found,
        };

        // §12.5 step 3: concatenate luma-first, then the CRC decides.
        let mut bits = BitVec::with_capacity(cap.total());
        for b in &luma_out.bits {
            bits.push(*b);
        }
        if let Some(c) = &chroma_out {
            for b in &c.bits {
                bits.push(*b);
            }
        }

        match Frame::from_bits(&bits) {
            Ok((frame, _)) => {
                let body = frame.payload()?;
                let packet = if frame.ctype == ContentType::CoseSign1 {
                    self.verifier.verify(&body)
                } else {
                    Packet::Unsigned {
                        body,
                        ctype: frame.ctype,
                    }
                };
                Ok(DecodeOutput {
                    packet,
                    diagnostics,
                })
            }
            Err(e) => {
                // §12.5: on chroma failure the luma layer still yields a prefix. For an
                // unsigned packet that MAY be surfaced; for a signed one it MUST NOT be,
                // and a prefix cannot tell us which it is — so `Partial` is opt-in and
                // carries no interpretation.
                if self.options.allow_partial && !diagnostics.chroma_recovered && luma_out.converged
                {
                    diagnostics.chroma_recovered = false;
                    let prefix: Vec<u8> = {
                        let mut b = BitVec::with_capacity(luma_out.bits.len());
                        for bit in &luma_out.bits {
                            b.push(*bit);
                        }
                        b.into_bytes()
                    };
                    return Ok(DecodeOutput {
                        packet: Packet::Partial { prefix },
                        diagnostics,
                    });
                }
                Err(e)
            }
        }
    }
}

/// Everything established about a symbol before demodulation: geometry, format, palette
/// and the illumination model.
///
/// Split out because [`FrameAccumulator`] establishes it once from the first frame and
/// reuses it — the symbol is not moving between frames of a preview, and re-detecting it
/// every frame would dominate the cost of multi-frame combining.
struct Prepared {
    layout: Layout,
    palette: Palette,
    format: FormatInfo,
    format_margin: f32,
    transform: Transform,
    illum: IlluminationModel,
    stats: ChannelStats,
    with_chroma: bool,
    alignment_found: bool,
}

impl Prepared {
    fn demodulate(&self, img: &ImageView) -> Demodulation {
        demod::demodulate(
            img,
            &self.transform,
            &self.layout,
            &self.palette,
            &self.illum,
            &self.stats,
            self.with_chroma,
        )
    }
}

/// §12.1–§12.3: detect, read the format region, fit the illumination model.
fn prepare(img: &ImageView) -> Result<Prepared, DecodeError> {
    // §12.1. Do all of this before touching colour.
    let gray = img.gray_buffer();
    let binary = image::binarize(&gray, img.width, img.height).denoised();
    let detection = detect::detect(&binary).ok_or(LcError::NotFound)?;

    // §12.2.
    let (format, format_margin, transform, alignment_found) =
        read_format(img, &binary, &detection)?;

    let layout = Layout::new(format.version, format.profile);
    let palette = Palette::for_profile(format.profile);
    let with_chroma = img.format.has_chroma() && format.profile.chroma_bits_per_cell() > 0;

    // §12.3.
    let refs =
        demod::collect_references(img, &transform, &layout, &palette, format.reference_border);
    let illum = if with_chroma {
        IlluminationModel::fit(&refs, layout.side, format.version.get())
    } else {
        // A greyscale capture has no colour to correct, and fitting one from neutral
        // samples would just fit noise.
        IlluminationModel::identity(layout.side)
    };
    let stats = demod::measure(&refs, &illum, &palette);

    Ok(Prepared {
        layout,
        palette,
        format,
        format_margin,
        transform,
        illum,
        stats,
        with_chroma,
        alignment_found,
    })
}

/// Read the format region, resolving the version chicken-and-egg (§12.2).
///
/// The format region says what version the symbol is, but *where* the region sits depends
/// on the version. The geometric estimate from §12.1 is good to about one version step,
/// so the candidates are that estimate and its neighbours out to ±3, and a candidate is
/// accepted only when the version it decodes to **is** the candidate. That self-consistency check
/// is much stronger than either signal alone: a wrong version puts copy B in the wrong
/// place, so it decodes to noise, and the odds of noise landing on a valid BCH codeword
/// that names the version you guessed are negligible.
///
/// Both copies are decoded softly and ranked by *margin* — the correlation gap to the
/// runner-up — rather than by Hamming distance. Two copies can both pass at distance ≤ 3
/// and disagree; the one whose runner-up is further away is the one to believe.
fn read_format(
    img: &ImageView,
    binary: &image::Binary,
    detection: &Detection,
) -> Result<(FormatInfo, f32, Transform, bool), DecodeError> {
    let estimate = detection.version.get() as i32;
    let mut best: Option<(FormatInfo, f32, Transform, bool)> = None;

    for delta in [0i32, -1, 1, -2, 2, -3, 3] {
        let Ok(version) = Version::new((estimate + delta).clamp(1, 32) as u8) else {
            continue;
        };
        if version.get() as i32 != estimate + delta {
            continue;
        }

        // Re-find the alignment marker for this candidate: it lives at (side−7, side−7),
        // which moves with the version.
        let module = detection.finders.iter().map(|f| f.module).sum::<f32>() / 3.0;
        let rough = detect::build_transform(&detection.finders, None, version)
            .ok_or(LcError::BadGeometry)?;
        let alignment = detect::locate_alignment(binary, &rough, version, module);
        let transform = detect::build_transform(&detection.finders, alignment, version)
            .ok_or(LcError::BadGeometry)?;
        // One refinement pass: re-find the marker from the homography's own prediction,
        // which is much better than the affine one that seeded the first search.
        let (transform, alignment) =
            detect::refine_alignment(binary, &detection.finders, version, transform, module);

        // The format region is luma-only, so this runs with no colour correction at all.
        // Profile is unknown until the region decodes, so measure against the monochrome
        // palette; the two levels are what matter and every profile has the same two.
        let probe = Layout::new(version, Profile::M);
        let mono = Palette::for_profile(Profile::M);
        let identity = IlluminationModel::identity(version.side());
        let refs = demod::collect_references(img, &transform, &probe, &mono, false);
        let stats = demod::measure(&refs, &identity, &mono);

        let copies = demod::format_llrs(img, &transform, &probe, &identity, &stats);
        for llrs in &copies {
            let (bits, margin) = bch::decode_soft(llrs);
            let Ok(info) = FormatInfo::from_bits(bits) else {
                continue;
            };
            if info.version != version {
                continue;
            }
            if best.as_ref().is_none_or(|(_, m, _, _)| margin > *m) {
                best = Some((info, margin, transform, alignment.is_some()));
            }
        }
    }
    best.ok_or(LcError::FormatUndecodable)
}

/// Multi-frame LLR accumulation (§12.4).
///
/// > Banding and glare move between frames, so this is very effective and much cheaper
/// > than tightening the code rate. This is the single best use of a live camera preview.
/// > — §12.4
///
/// Geometry and format are established once from the first frame that detects, and later
/// frames are demodulated against it. That is both faster and *better*: re-detecting per
/// frame would let detection jitter smear the accumulated LLRs, which is exactly the
/// coherence the accumulation depends on. A frame that fails to demodulate is skipped
/// rather than fatal — in a preview stream most frames are blurred or mid-refresh.
pub struct FrameAccumulator {
    prepared: Option<Prepared>,
    luma: Vec<f32>,
    chroma: Vec<f32>,
    stats: Option<ChannelStats>,
    frames: u8,
}

impl Default for FrameAccumulator {
    fn default() -> Self {
        FrameAccumulator::new()
    }
}

impl FrameAccumulator {
    /// An empty accumulator.
    pub fn new() -> FrameAccumulator {
        FrameAccumulator {
            prepared: None,
            luma: Vec::new(),
            chroma: Vec::new(),
            stats: None,
            frames: 0,
        }
    }

    /// Frames accumulated so far.
    #[inline]
    pub fn frames(&self) -> u8 {
        self.frames
    }

    /// Whether a symbol has been located yet.
    #[inline]
    pub fn has_symbol(&self) -> bool {
        self.prepared.is_some()
    }

    /// Discard everything, for when the symbol has moved or changed.
    pub fn reset(&mut self) {
        *self = FrameAccumulator::new();
    }

    /// Add a frame.
    ///
    /// The first frame that detects fixes the geometry. Returns the error from that first
    /// detection attempt if it fails, so a caller can drive user guidance; once a symbol
    /// is locked, later frames that fail are simply skipped.
    pub fn push(&mut self, img: &ImageView) -> Result<(), DecodeError> {
        if self.prepared.is_none() {
            let prepared = prepare(img)?;
            let cap = prepared.layout.capacity();
            self.luma = alloc::vec![0.0; cap.luma_bits];
            self.chroma = alloc::vec![0.0; cap.chroma_bits];
            self.prepared = Some(prepared);
        }
        let prepared = self.prepared.as_ref().expect("just set");
        let d = prepared.demodulate(img);

        // Independent observations of the same bit: LLRs add.
        for (acc, v) in self.luma.iter_mut().zip(&d.luma) {
            *acc += v;
        }
        for (acc, v) in self.chroma.iter_mut().zip(&d.chroma) {
            *acc += v;
        }
        self.stats = Some(d.stats);
        self.frames = self.frames.saturating_add(1);
        Ok(())
    }

    /// Add a frame from a packed RGB8 buffer.
    pub fn push_rgb(&mut self, px: &[u8], w: u32, h: u32, stride: u32) -> Result<(), DecodeError> {
        self.push(&ImageView::new(px, w, h, stride, PixelFormat::Rgb8)?)
    }

    /// Try to decode everything accumulated so far.
    pub fn try_decode<V: PacketVerifier>(
        &self,
        decoder: &Decoder<V>,
    ) -> Result<DecodeOutput, DecodeError> {
        let prepared = self.prepared.as_ref().ok_or(LcError::NotFound)?;
        let stats = self.stats.ok_or(LcError::NotFound)?;
        // Average rather than sum. Min-sum is scale invariant — multiplying every LLR by
        // a positive constant cannot change a single hard decision — so the gain from
        // combining frames is entirely in the *relative* magnitudes, and averaging N
        // independent observations improves those by √N. Clamping the raw sum instead
        // would saturate exactly the bits that agreed across frames while leaving the
        // disputed ones small, which is the opposite of what accumulation is for; it is
        // why this originally decoded worse with twelve frames than with one.
        let n = (self.frames.max(1)) as f32;
        let avg = |v: &f32| (v / n).clamp(-demod::LLR_CLAMP, demod::LLR_CLAMP);
        let d = Demodulation {
            luma: self.luma.iter().map(avg).collect(),
            chroma: self.chroma.iter().map(avg).collect(),
            stats,
        };
        decoder.finish(prepared, &d, self.frames)
    }
}
