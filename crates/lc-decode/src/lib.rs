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
pub mod equalize;
pub mod error;
pub mod illum;
pub mod image;
pub mod math;
pub mod ring;
pub mod sample;
pub mod transform;

use alloc::vec::Vec;
use demod::{ChannelStats, Demodulation};
use detect::Detection;
use illum::IlluminationModel;
use image::{ImageView, PixelFormat};
use lc_core::bch;
use lc_core::bits::BitVec;
use lc_core::border;
use lc_core::ecc::ldpc::{CHROMA_RATE_INDEX, Code, DecodeResult, LUMA_RATE_INDEX, MAX_ITERATIONS};
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
    /// Per-cell chroma SNR, on the same scale as [`Diagnostics::estimated_snr_db`].
    ///
    /// Not in §14.3's `Diagnostics`, which lists only the one figure. Added because
    /// §8.2's unequal protection is a quantitative claim about the *gap* between the two
    /// layers' channels, and a single combined SNR cannot express it — see D10.
    /// `f32::NEG_INFINITY` on profile `M`, which has no chroma layer.
    pub chroma_snr_db: f32,
    /// Frames combined before decoding (§12.4).
    pub frames_combined: u8,
    /// The format region as read.
    pub format: FormatInfo,
    /// Confidence margin of the winning format copy, from the soft BCH decode.
    pub format_margin: f32,
    /// Whether an alignment marker was found, and therefore whether the transform is a
    /// full homography or an affine fallback.
    pub alignment_found: bool,
    /// Module pitch as **captured**, in sensor pixels, at the symbol's finest corner.
    ///
    /// Not §13.1's device-pixels-per-module, which is a property of how the *display*
    /// drew the symbol. This is what arrived at the sensor after the optical path, and it
    /// is the quantity a working-range measurement is actually a measurement of: a symbol
    /// of fixed physical size subtends fewer sensor pixels the further away the camera is.
    ///
    /// Reported at the finest corner because that is where sampling is hardest, so it is
    /// the figure a floor should be stated against.
    pub capture_pitch_min: f32,
    /// Module pitch as captured at the symbol's coarsest corner.
    ///
    /// `capture_pitch_max / capture_pitch_min` measures how obliquely the symbol was
    /// viewed — 1.0 fronto-parallel, climbing with angle — without needing the camera
    /// intrinsics. See [`Transform::pitch_range`].
    pub capture_pitch_max: f32,
}

/// What [`Decoder::probe`] establishes about a capture before any payload decoding.
///
/// Every field is also present in [`Diagnostics`]; the point is that these survive a
/// decode that fails, and the fields that need the payload do not appear.
#[derive(Copy, Clone, Debug)]
pub struct Probe {
    /// The format region as read.
    pub format: FormatInfo,
    /// Confidence margin of the winning format copy.
    pub format_margin: f32,
    /// Whether a full homography was fitted rather than an affine fallback.
    pub alignment_found: bool,
    /// RMS residual of the §12.3 fit, in linear light.
    pub illum_residual_rms: f32,
    /// References discarded by the §12.3 robust refit. High means glare.
    pub references_rejected: u8,
    /// Captured module pitch at the finest corner. See [`Diagnostics::capture_pitch_min`].
    pub capture_pitch_min: f32,
    /// Captured module pitch at the coarsest corner.
    pub capture_pitch_max: f32,
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
    /// Largest version the §6.4 ring detector will consider.
    ///
    /// The detector correlates a candidate hull against **every** version's geometry and
    /// sequence, so this is a direct latency knob: measured on a desktop release build,
    /// lowering it from 64 to 32 cuts a v20 decode from 48.8 ms to 39.9 ms (−18%) and a
    /// v10 from 27.0 ms to 20.9 ms (−23%), because a small symbol otherwise pays for a
    /// range it cannot be in.
    ///
    /// Defaults to [`Version::MAX`], so a decoder finds everything unless told otherwise
    /// — silently failing to see a large symbol is a worse default than being slower.
    /// A latency-sensitive scanner that only ever meets classic-range symbols should set
    /// [`Version::MAX_CLASSIC`].
    pub max_version: Version,
}

impl Default for DecodeOptions {
    fn default() -> Self {
        DecodeOptions {
            max_iterations: MAX_ITERATIONS,
            allow_partial: false,
            force_profile: None,
            max_version: Version::MAX,
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
    ///
    /// Where the frame holds more than one code, several candidates are tried and **the
    /// CRC decides** — §9.1 makes it "the authoritative accept/reject for decoding", and
    /// nothing earlier in the pipeline is strong enough to stand in for it. The format
    /// region is not: BCH(30,15) corrects three errors, so a wrong triple of finders
    /// yields a format word that decodes far more often than is comfortable, and
    /// committing to the first candidate that gets that far would trade one failure mode
    /// for a worse one.
    ///
    /// This costs nothing on the ordinary single-symbol frame, where the first candidate
    /// decodes and the loop stops. It costs a little on frames that fail — which is the
    /// right place to spend it, since §6.2 shares QR's finder by design and a §17.2
    /// dual-render, or simply two symbols on one poster, is otherwise undecodable.
    pub fn decode(&self, img: &ImageView) -> Result<DecodeOutput, DecodeError> {
        // Greyscale only, for now: the ring path (D17) reads luma and never touches the
        // binary, so a screen symbol — which is what a finder-less symbol is — should not
        // pay for binarisation and denoising at all. That cost moves below, behind the
        // finder fallback that actually needs it.
        let gray = img.gray_buffer();
        let mut first_err = None;

        // Finder-less first (D17): it is what a screen symbol now is, and a frame with no
        // ring fails the sequence correlation cheaply without ever reaching a format read.
        match prepare_ring(img, &gray, self.options.max_version) {
            Ok(prepared) => {
                let demodulation = prepared.demodulate(img);
                match self.finish(&prepared, &demodulation, 1, None, Some(img)) {
                    Ok(out) => return Ok(out),
                    Err(e) => {
                        first_err = Some(e);
                    }
                }
            }
            // A ring that was found and then failed is a better diagnosis than whatever the
            // finder path says about a symbol that has no finders.
            Err(LcError::NotFound) => {}
            Err(e) => first_err = Some(e),
        }

        let binary = binarise(&gray, img);
        let candidates = match scan(img, gray, &binary) {
            Ok(c) => c,
            Err(e) => return Err(first_err.unwrap_or(e)),
        };
        for detection in &candidates {
            let prepared = match prepare_from(img, &binary, detection) {
                Ok(p) => p,
                Err(e) => {
                    first_err.get_or_insert(e);
                    continue;
                }
            };
            let demodulation = prepared.demodulate(img);
            match self.finish(&prepared, &demodulation, 1, None, Some(img)) {
                Ok(out) => return Ok(out),
                Err(e) => {
                    first_err.get_or_insert(e);
                }
            }
        }
        Err(first_err.unwrap_or(LcError::NotFound))
    }

    /// Establish where a symbol is and how it was captured, without decoding it.
    ///
    /// Runs §12.1–§12.3 — detection, the format region, the illumination fit — and stops
    /// before demodulation. Two uses:
    ///
    /// * **User guidance.** §14.3 asks for diagnostics because "colour codes fail for
    ///   diagnosable reasons", but a failed [`Decoder::decode`] returns only an error:
    ///   the caller cannot tell "nothing in frame" from "found it, payload was marginal",
    ///   which are opposite instructions to give a user. A probe separates them.
    /// * **Measurement.** A capture sweep needs to bin *failures* by the conditions they
    ///   failed under, and a failure that carries no geometry cannot be binned at all.
    ///
    /// Succeeding here says the symbol was located and its format region read. It implies
    /// nothing about whether the payload is recoverable.
    pub fn probe(&self, img: &ImageView) -> Result<Probe, DecodeError> {
        let p = prepare(img, self.options.max_version)?;
        let (capture_pitch_min, capture_pitch_max) = p.transform.pitch_range(p.layout.side);
        Ok(Probe {
            format: p.format,
            format_margin: p.format_margin,
            alignment_found: p.alignment_found,
            illum_residual_rms: p.illum.residual_rms,
            references_rejected: p.illum.rejected.min(255) as u8,
            capture_pitch_min,
            capture_pitch_max,
        })
    }

    /// One rung of D24's equalising retry against `obs`: turbo-equalised luma until it
    /// converges, then turbo-equalised chroma if luma converged and chroma did not. Returns
    /// whether either layer changed.
    fn equalising_retry(
        &self,
        p: &Prepared,
        d: &Demodulation,
        obs: &equalize::Observation,
        luma_code: &Code,
        luma_out: &mut DecodeResult,
        chroma_out: &mut Option<DecodeResult>,
    ) -> Result<bool, DecodeError> {
        let cap = p.layout.capacity();
        let ecc = p.format.ecc;
        let mut changed = false;
        // Turbo equalisation (D25): each round equalises against the levels the last
        // decode settled on — the channel's own decisions in the first round — so the
        // decoder's corrections feed back into the neighbour estimates.
        let mut prior: Option<Vec<bool>> = None;
        for _ in 0..TURBO_ROUNDS {
            if luma_out.converged {
                break;
            }
            let Some(mut llrs) = equalize::luma(
                obs,
                &p.layout,
                &p.palette,
                &d.stats,
                p.ring,
                &d.luma,
                prior.as_deref(),
            ) else {
                break;
            };
            demod::unmask_luma(&mut llrs, &p.layout, p.format.mask);
            let out = luma_code.decode(
                &Interleaver::new(cap.luma_bits).deinterleave(&llrs),
                self.options.max_iterations,
            )?;
            if out.converged {
                *luma_out = out;
                changed = true;
                break;
            }
            let next = equalize::luma_levels(&out.codeword, &p.layout, p.format.mask);
            // A fixed point: the next round would equalise against the same levels.
            if prior.as_ref() == Some(&next) {
                break;
            }
            prior = Some(next);
        }
        if luma_out.converged && chroma_out.as_ref().is_some_and(|c| !c.converged) {
            let light = equalize::luma_levels(&luma_out.codeword, &p.layout, p.format.mask);
            let code = Code::new(CHROMA_RATE_INDEX[ecc.chroma as usize & 3], cap.chroma_bits)?;
            // Turbo again: later rounds take neighbour hues from the last decode.
            let mut prior: Option<Vec<usize>> = None;
            for _ in 0..TURBO_ROUNDS {
                let Some(llrs) = equalize::chroma(
                    obs,
                    &p.layout,
                    &p.palette,
                    &d.stats,
                    p.ring,
                    &light,
                    prior.as_deref(),
                ) else {
                    break;
                };
                let out = code.decode(
                    &Interleaver::new(cap.chroma_bits).deinterleave(&llrs),
                    self.options.max_iterations,
                )?;
                if out.converged {
                    *chroma_out = Some(out);
                    changed = true;
                    break;
                }
                let next = equalize::chroma_hues(&out.codeword, &p.layout, p.palette.hues);
                if prior.as_ref() == Some(&next) {
                    break;
                }
                prior = Some(next);
            }
        }
        Ok(changed)
    }

    /// Turn demodulated LLRs into a packet: LDPC, frame, CRC, then optionally verify.
    ///
    /// `mean` is the accumulated observation of many frames, for D24's equalising retry when
    /// the CRC rejects the first pass (D25); `None` reads `d`'s own colours instead. `img` is
    /// the single frame `d` came from, when there is one, for the retry's second rung.
    fn finish(
        &self,
        p: &Prepared,
        d: &Demodulation,
        frames: u8,
        mean: Option<&equalize::Observation>,
        img: Option<&ImageView>,
    ) -> Result<DecodeOutput, DecodeError> {
        let cap = p.layout.capacity();
        let ecc = p.format.ecc;

        // §12.5 step 1: the luma layer, independent of all colour processing.
        let mut luma_llrs = d.luma.clone();
        demod::unmask_luma(&mut luma_llrs, &p.layout, p.format.mask);
        let luma_code = Code::new(LUMA_RATE_INDEX[ecc.luma as usize & 3], cap.luma_bits)?;
        let mut luma_out = luma_code.decode(
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

        // §12.5 step 3: concatenate luma-first, then the CRC decides.
        let mut parsed = Frame::from_bits(&layer_bits(&luma_out, chroma_out.as_ref(), cap.total()));

        // D24: the CRC rejected the first pass. Re-read with neighbour bleed removed — luma
        // first if it did not converge, its own confident decisions standing in for the
        // levels; then chroma, if luma converged and chroma did not, with every level now
        // known exactly. Only after the CRC, because a non-converged layer can still carry the
        // right bits and the CRC is what says so: gating on convergence instead re-read
        // frames that were already decoding.
        //
        // Two rungs (D25). First from the payload colours demodulation already sampled, or the
        // accumulated mean of many frames: free, and its kernel fit is not biased by the
        // white gap and quiet zone, which cost `C1` half its decodes at blur radius 9. Then,
        // for a single frame only, with every surrounding cell sampled too, which `C3`
        // needs at the same blur — the known cells steady a fit that few of its payload
        // decisions are confident enough to join.
        if parsed.is_err() {
            let own = match mean {
                Some(_) => None,
                None => equalize::Observation::new(&p.layout, p.ring.is_some(), &d.colours),
            };
            if let Some(obs) = mean.or(own.as_ref())
                && self.equalising_retry(p, d, obs, &luma_code, &mut luma_out, &mut chroma_out)?
            {
                parsed = Frame::from_bits(&layer_bits(&luma_out, chroma_out.as_ref(), cap.total()));
            }
            if parsed.is_err()
                && let (Some(img), Some(mut full)) = (img, own)
            {
                full.surround(img, &p.transform, &p.layout, &p.illum);
                if self.equalising_retry(p, d, &full, &luma_code, &mut luma_out, &mut chroma_out)? {
                    parsed =
                        Frame::from_bits(&layer_bits(&luma_out, chroma_out.as_ref(), cap.total()));
                }
            }
        }

        let pitch = p.transform.pitch_range(p.layout.side);
        let mut diagnostics = Diagnostics {
            luma_iterations: luma_out.iterations,
            chroma_iterations: chroma_out.as_ref().map_or(0, |c| c.iterations),
            chroma_recovered: chroma_out.as_ref().is_some_and(|c| c.converged),
            illum_residual_rms: p.illum.residual_rms,
            references_rejected: p.illum.rejected.min(255) as u8,
            estimated_snr_db: d.stats.snr_db(),
            chroma_snr_db: d.stats.chroma_snr_db(&p.palette),
            frames_combined: frames,
            format: p.format,
            format_margin: p.format_margin,
            alignment_found: p.alignment_found,
            capture_pitch_min: pitch.0,
            capture_pitch_max: pitch.1,
        };

        match parsed {
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

/// Equalise-then-decode rounds each retry makes before giving up (D25). Three left `C3` at
/// 31% under blur radius 9, five reached 88%, and eight gained nothing over five. A round
/// whose decisions match the last one's ends the loop early.
const TURBO_ROUNDS: usize = 5;

/// Both layers' information bits, luma first, as §12.5 step 3 concatenates them.
fn layer_bits(luma: &DecodeResult, chroma: Option<&DecodeResult>, total: usize) -> BitVec {
    let mut bits = BitVec::with_capacity(total);
    for &b in luma
        .bits
        .iter()
        .chain(chroma.map_or(&[][..], |c| &c.bits[..]))
    {
        bits.push(b);
    }
    bits
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
    /// The symbol's ring, as its colours are known after the format read; `None` without
    /// one. Kept for D24's equaliser, which needs every cell's nominal colour.
    ring: Option<border::RingMode>,
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

/// Halvings tried when the full-resolution finder scan comes up empty (D16).
///
/// §12.1's run-ratio scan has an **upper** limit on module pitch as well as the lower one
/// §13.1 states. Measured on clean renders, detection works from 8 to about 22 captured
/// pixels per module and then stops — and fails at exactly 16 and 24, `2·BLOCK` and
/// `3·BLOCK`, where the binariser's fixed 8-pixel grid aligns with the module grid. That
/// second fault is D6's mechanism recurring at multiples D6 never tested; one pixel of
/// image offset fixes those two and nothing rescues the wall above 24, which is how the two
/// were told apart.
///
/// The ceiling caps working range, because range is bought with sensor pixels per module
/// and raising them walks into it: a 4K capture tripled the pitch and decoded nothing at
/// any distance.
///
/// Halving the image halves the pitch, so a symbol at pitch 40 lands at 10 two levels down.
///
/// **A mipmap, not a list of factors.** The first version tried reduction factors 1, 2, 3
/// and 4, each downscaled from the full-resolution image — three full-resolution passes on
/// every frame where detection fails. Two changes, coverage-neutral on clean renders (80 of
/// 88 pitch × grid-phase cells decode either way):
///
/// * **Factor 3 was redundant.** Detection works over pitch 8–22, so a factor of 2 covers
///   captured pitch 16–44 and a factor of 4 covers 32–88. Those already overlap.
/// * **Each level is built from the one before**, so only the first halving touches
///   full resolution and the second costs a quarter of it.
///
/// **What the pyramid costs, timed interleaved at 1280×720.** On a clean frame with no
/// symbol it adds 8.5 ms to a 22.3 ms scan (+38%), because every level has to fail first.
/// On a *noisy* frame it adds nothing: noise yields spurious finder triples at full
/// resolution, `scan` returns early, and the pyramid never runs. The larger cost on those
/// frames — 41 ms beyond the scan — is `Decoder::decode` walking D13's candidates through
/// format reads, which predates this. An earlier measurement blamed a +131% on the pyramid;
/// it had been comparing a bare scan against a full decode.
const PYRAMID_LEVELS: u32 = 2;

/// List the plausible finder-bearing symbols in the frame, best-scoring first.
///
/// Only the **finder search** runs on a reduced image. The alignment marker, the transform
/// and every sampling stage afterwards work against the full-resolution binary, so nothing
/// that needs precision is computed from a shrunken image — the reduction exists solely to
/// bring the module pitch into the range the run-ratio scan can see.
fn scan(
    img: &ImageView,
    gray: Vec<u8>,
    binary: &image::Binary,
) -> Result<Vec<Detection>, DecodeError> {
    // The ordinary case: found at full resolution, and the pyramid is never built.
    let candidates = detect::candidates_from(binary, detect::find_finders(binary));
    if !candidates.is_empty() {
        return Ok(candidates);
    }

    let (mut level, mut w, mut h) = (gray, img.width, img.height);
    let mut factor = 1u32;
    for _ in 0..PYRAMID_LEVELS {
        if w / 2 < 64 || h / 2 < 64 {
            break; // nothing left to find
        }
        // From the previous level, not from full resolution — see `PYRAMID_LEVELS`.
        (level, w, h) = image::downscale(&level, w, h, 2);
        factor *= 2;
        let reduced = image::binarize(&level, w, h).denoised();

        // Lift the finders back into full-resolution coordinates. A reduced pixel averages
        // a `factor`-wide block, so the half-pixel terms recentre it on that block. Nested
        // halvings average the same block a single `factor` reduction would, so the
        // cumulative factor is the right one to scale by.
        let k = factor as f32;
        let finders = detect::find_finders(&reduced)
            .into_iter()
            .map(|mut f| {
                f.x = (f.x + 0.5) * k - 0.5;
                f.y = (f.y + 0.5) * k - 0.5;
                f.module *= k;
                f
            })
            .collect();

        let candidates = detect::candidates_from(binary, finders);
        if !candidates.is_empty() {
            return Ok(candidates);
        }
    }
    Err(LcError::NotFound)
}

/// §12.1–§12.3 against the frame's most likely symbol.
///
/// Uses the best-scoring candidate **only**, and that restraint is deliberate.
///
/// Callers here want *a* symbol's geometry rather than its content — [`Decoder::probe`],
/// and [`FrameAccumulator`], which has to lock onto one symbol and keep it — so they have
/// no CRC to check a guess against. Walking further down the candidate list without one
/// makes the decoder *worse*: BCH(30,15) corrects three errors, so a random 30-bit word
/// lands on a valid codeword often enough that trying six candidates turns "found
/// nothing" into "found something" on frames containing no symbol at all. Measured, not
/// reasoned: `probe_finds_nothing_in_a_frame_with_no_symbol` fails the moment this loops.
///
/// [`Decoder::decode`] can afford to try several precisely because it has the CRC.
///
/// **Known limitation.** In a frame holding two codes this may lock onto a triple
/// straddling them, and [`FrameAccumulator`] will then keep that geometry until it is
/// reset. Single-frame [`Decoder::decode`] handles that case; multi-frame accumulation
/// does not yet.
fn prepare(img: &ImageView, max_version: Version) -> Result<Prepared, DecodeError> {
    let gray = img.gray_buffer();
    // Safe to try before the finder path even here, where there is no CRC: a ring is only
    // accepted after the sequence correlates *and* a BCH-protected format decodes to the
    // version the correlation named. Noise does not clear both.
    let ring_err = match prepare_ring(img, &gray, max_version) {
        Ok(p) => return Ok(p),
        Err(e) => e,
    };
    let binary = binarise(&gray, img);
    match scan(img, gray, &binary) {
        Ok(candidates) => prepare_from(img, &binary, &candidates[0]),
        Err(e) => Err(if matches!(ring_err, LcError::NotFound) {
            e
        } else {
            ring_err
        }),
    }
}

/// §12.1's binarisation, for the finder path only.
///
/// Deliberately *not* done alongside the greyscale conversion. The ring detector reads
/// greyscale directly, so on the common path — a screen symbol, found by its ring — this
/// never runs, and a live preview stops paying for a full-frame binarise and denoise per
/// frame that nothing reads.
fn binarise(gray: &[u8], img: &ImageView) -> image::Binary {
    image::binarize(gray, img.width, img.height).denoised()
}

/// §12.1–§12.3 for a finder-less symbol, located and described entirely by its ring (D17).
///
/// Geometry, version, orientation and mirror come from [`ring::detect_ring`]; the format
/// from the ring's colour choices. Needs a colour capture: the format rides on hue, so a
/// greyscale frame of a finder-less symbol cannot be decoded — a real trade against §1.1
/// goal 3, recorded in D17.
fn prepare_ring(
    img: &ImageView,
    gray: &[u8],
    max_version: Version,
) -> Result<Prepared, DecodeError> {
    let fix =
        ring::detect_ring(gray, img.width, img.height, max_version).ok_or(LcError::NotFound)?;
    let (format, format_margin) = ring::read_ring_format(img, &fix.transform, fix.version)?;

    let layout = Layout::for_symbol(format.version, format.profile, format.reference_border);
    let palette = Palette::for_profile(format.profile);
    let with_chroma = img.format.has_chroma() && format.profile.chroma_bits_per_cell() > 0;

    // With the format known, every ring colour is known too, so the whole ring joins the
    // §12.3 fit rather than only its reference positions.
    let ring_mode = Some(border::RingMode::Data(format.to_ring_codeword()));
    let refs = demod::collect_references(img, &fix.transform, &layout, &palette, ring_mode);
    let illum = if with_chroma {
        IlluminationModel::fit(&refs, layout.side, format.version.get())
    } else {
        IlluminationModel::identity(layout.side)
    };
    let stats = demod::measure(&refs, &illum, &palette);

    Ok(Prepared {
        layout,
        palette,
        format,
        format_margin,
        transform: fix.transform,
        illum,
        stats,
        with_chroma,
        // A full homography, fitted from the ring's four corners.
        alignment_found: true,
        ring: ring_mode,
    })
}

/// §12.2–§12.3 for one candidate: read its format region, then fit the illumination model.
fn prepare_from(
    img: &ImageView,
    binary: &image::Binary,
    detection: &Detection,
) -> Result<Prepared, DecodeError> {
    // §12.2.
    let (format, format_margin, transform, alignment_found) = read_format(img, binary, detection)?;

    let layout = Layout::for_symbol(format.version, format.profile, format.reference_border);
    let palette = Palette::for_profile(format.profile);
    let with_chroma = img.format.has_chroma() && format.profile.chroma_bits_per_cell() > 0;

    // §12.3.
    let ring_mode = format.reference_border.then(|| {
        if Layout::is_finderless(format.profile, true) {
            border::RingMode::Data(format.to_ring_codeword())
        } else {
            border::RingMode::Plain
        }
    });
    let refs = demod::collect_references(img, &transform, &layout, &palette, ring_mode);
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
        ring: ring_mode,
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
        let refs = demod::collect_references(img, &transform, &probe, &mono, None);
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
    /// Ceiling for ring detection on the first frame — see `DecodeOptions::max_version`.
    max_version: Version,
    prepared: Option<Prepared>,
    luma: Vec<f32>,
    chroma: Vec<f32>,
    stats: Option<ChannelStats>,
    frames: u8,
    /// Every cell's colour summed over the frames taken, for D24's equalising retry (D25).
    /// Blur is the same in every frame of a preview and noise is not, so the mean is what
    /// to equalise.
    observed: Option<equalize::Observation>,
}

impl Default for FrameAccumulator {
    fn default() -> Self {
        FrameAccumulator::new()
    }
}

impl FrameAccumulator {
    /// Bound ring detection to `max_version` on the frame that fixes the geometry.
    ///
    /// The same latency knob as `DecodeOptions::max_version`, and it matters more here:
    /// a preview loop runs detection on every frame until one locks.
    #[must_use]
    pub fn with_max_version(mut self, max_version: Version) -> FrameAccumulator {
        self.max_version = max_version;
        self
    }

    /// An empty accumulator.
    pub fn new() -> FrameAccumulator {
        FrameAccumulator {
            max_version: Version::MAX,
            prepared: None,
            luma: Vec::new(),
            chroma: Vec::new(),
            stats: None,
            frames: 0,
            observed: None,
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
            let prepared = prepare(img, self.max_version)?;
            let cap = prepared.layout.capacity();
            self.luma = alloc::vec![0.0; cap.luma_bits];
            self.chroma = alloc::vec![0.0; cap.chroma_bits];
            self.prepared = Some(prepared);
        }
        let prepared = self.prepared.as_ref().expect("just set");
        let d = prepared.demodulate(img);
        let o = equalize::Observation::new(&prepared.layout, prepared.ring.is_some(), &d.colours);
        match (&mut self.observed, o) {
            (Some(sum), Some(o)) => sum.add(&o),
            (None, o) => self.observed = o,
            _ => {}
        }

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
            colours: Vec::new(),
        };
        decoder.finish(prepared, &d, self.frames, self.observed.as_ref(), None)
    }
}
