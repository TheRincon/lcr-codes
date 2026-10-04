//! Composable capture degradations, and the decoder's behaviour under them (§15).
//!
//! This is the seed of the §15 harness — the screen impairments that actually break
//! colour codes on glass, each isolated so a regression names itself. It is not yet the
//! full suite (no PWM banding, no subpixel geometry, no moiré sweep), but it covers the
//! impairments the design makes specific claims about, so those claims are now testable
//! rather than asserted:
//!
//! | Claim | Test |
//! |---|---|
//! | §6.2 alignment marker buys perspective | [`survives_perspective`] |
//! | §12.3 fit cancels shading | [`survives_a_shading_gradient`] |
//! | §12.3 affine cancels display colour modes | [`survives_night_shift`] |
//! | §12.3 robust rejection survives specular glare | [`survives_a_specular_blob`] |
//! | §12.4 soft LLRs beat hard decisions | [`survives_sensor_noise`] |
//! | §12.4 multi-frame combining is very effective | [`multi_frame_beats_single_frame`] |
//! | §1.4 chroma survives 4:2:0 subsampling | [`survives_chroma_subsampling`] |
//! | §8.3 interleaving spreads a burst | [`survives_an_occluding_finger`] |
//!
//! ## The measured operating envelope
//!
//! Established by `cargo run -p lc-decode --release --example envelope`, at v4 `C1`,
//! ECC level 1, 12 device pixels per module. The tests below sit inside it, so a
//! regression shows up as a test failure rather than as a quietly narrower envelope.
//!
//! | Impairment | Decodes to | First failure |
//! |---|---|---|
//! | Perspective tilt | 0.5 | 0.6 |
//! | In-plane rotation | 90° | — (no failure) |
//! | Tilt at 45° rotation | 0.5 | — (no failure) |
//! | Shading falloff | 0.8 | — (no failure) |
//! | Specular strength (r = 0.30, over a finder) | 0.5 | 0.6 |
//! | Specular radius (s = 0.6, centred) | 0.25 | 0.30 |
//! | Sensor noise σ (linear) | 0.06 | 0.08 |
//! | Defocus radius | 4 px of 12 | 5 px |
//! | Occlusion | 30% | 35% |
//!
//! Three of these are worth reading twice.
//!
//! **Shading never fails**, up to losing 80% of the light across the frame. That is the
//! D3 illumination model earning its keep — the literal reading of §12.3 leaves a
//! residual larger than the gap between palette entries at 50%.
//!
//! **Rotation never fails**, including 45°, where neither axis of the module grid is
//! aligned to the sensor and every run-length scan crosses modules diagonally.
//!
//! **Perspective tilt 0.5** puts the near edge 1.67× the far edge, around 45° off-axis
//! and well past the 10–20° §13.2 recommends — and it holds *combined* with 45° of
//! in-plane rotation. This number was 0.2 until the harness stopped clipping the symbol
//! out of frame, which is worth remembering: a degradation harness reports the weaker of
//! the decoder and itself.
//!
//! The combined tilt-and-rotation row has one or two isolated failures inside its range
//! (tilt 0.4 at 15°, tilt 0.1 at 30°) rather than a clean cliff. Those are marginal
//! format-region decodes, not a structural limit, and the cases below avoid them rather
//! than pretending they are not there.

// The shared harness in `support/ops.rs` carries every impairment; each consumer uses a
// subset.
#![allow(dead_code)]

use lc_core::{Compression, Packet, Profile};
use lc_decode::image::{ImageView, PixelFormat};
use lc_decode::{Decoder, FrameAccumulator};
use lc_encode::EncodeOptions;
use lc_render::{Raster, RenderOptions, render_rgb};

// ---------------------------------------------------------------- harness

include!("support/ops.rs");

// ---------------------------------------------------------------- fixtures

fn payload() -> Vec<u8> {
    (0..96u8)
        .map(|i| i.wrapping_mul(101).wrapping_add(29))
        .collect()
}

fn encode(profile: Profile) -> lc_core::Symbol {
    let opts = EncodeOptions {
        profile,
        reference_border: true,
        compression: Some(Compression::None),
        ..Default::default()
    };
    lc_encode::encode_bytes(&payload(), &opts).unwrap()
}

/// Render at a generous pitch: §13.1 requires ≥ 8 device pixels per module and prefers
/// 10–12, and the degradations below are meant to test the decoder, not to test what
/// happens when a conforming display app is ignored.
fn render(sym: &lc_core::Symbol) -> Raster {
    render_at(sym, 12)
}

fn render_at(sym: &lc_core::Symbol, module_px: u32) -> Raster {
    render_rgb(
        sym,
        &RenderOptions {
            module_px,
            ..RenderOptions::screen()
        },
    )
}

fn try_decode(r: &Raster) -> Result<Packet, lc_decode::DecodeError> {
    Decoder::new()
        .decode_rgb(&r.data, r.width, r.height, r.width * 3)
        .map(|o| o.packet)
}

/// Decode and assert the payload came back intact.
#[track_caller]
fn assert_decodes(r: &Raster, what: &str) {
    match try_decode(r) {
        Ok(Packet::Unsigned { body, .. }) => assert_eq!(body, payload(), "{what}: wrong payload"),
        Ok(other) => panic!("{what}: {other:?}"),
        Err(e) => panic!("{what}: {e}"),
    }
}

// ---------------------------------------------------------------- tests

#[test]
fn baseline_render_decodes() {
    for profile in Profile::ALL {
        assert_decodes(&render(&encode(profile)), &format!("baseline {profile:?}"));
    }
}

/// §6.2 adds the alignment marker so a decoder can fit a homography rather than an affine
/// transform, and §13.2 tells users to hold the camera 10–20° off-axis. Those two only
/// make sense together.
///
/// Two assertions, because the range has two regimes.
///
/// **Structural**, and asserted cell by cell: a head-on capture at any rotation, and the
/// on-axis tilt series out to 0.3. Rotation alone is a pure homography with no
/// foreshortening, and 0.3 covers the 10–20° §13.2 asks users for. A failure in either is
/// a real defect.
///
/// **Marginal**, and asserted only in aggregate: everything else. The envelope here is
/// speckled rather than a clean cliff — these are format-region decodes near the BCH
/// margin, and individual cells flip in *both* directions on small changes to sampling or
/// binarisation. The header above already records `tilt 0.1 rot 30` as one such cell.
/// Pinning named cells here measures luck, and §15 is explicit that "a pass/fail corpus
/// will not tell you whether a change helped", so this holds the count instead.
#[test]
fn survives_perspective() {
    let base = render(&encode(Profile::C1));
    let rotations = [0.0f32, 15.0, 30.0, 45.0, 90.0];
    let tilts = [0.0f32, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7];

    for rot in rotations {
        assert_decodes(
            &perspective(&base, 0.0, rot),
            &format!("head-on, rot {rot}"),
        );
    }
    for tilt in [0.1f32, 0.2, 0.3] {
        assert_decodes(
            &perspective(&base, tilt, 0.0),
            &format!("on-axis, tilt {tilt}"),
        );
    }

    // Whole grid. Measured 28/40 when this was written, against 27/40 before the D6
    // binarisation fix; the floor sits below both so ordinary reshuffling between
    // marginal cells does not trip it, but a real collapse of the envelope does.
    let ok = tilts
        .iter()
        .flat_map(|&t| rotations.map(move |r| (t, r)))
        .filter(|&(tilt, rot)| {
            let r = perspective(&base, tilt, rot);
            Decoder::new()
                .decode_rgb(&r.data, r.width, r.height, r.width * 3)
                .is_ok()
        })
        .count();
    let total = tilts.len() * rotations.len();
    assert!(ok >= 24, "perspective envelope collapsed to {ok}/{total}");
}

/// The impairment D3 in `docs/decisions.md` is about. An additive residual surface could
/// not cancel this.
#[test]
fn survives_a_shading_gradient() {
    let base = render(&encode(Profile::C1));
    for strength in [0.25f32, 0.45, 0.60] {
        assert_decodes(&shading(&base, strength), &format!("shading {strength}"));
    }
}

/// §3 lists display colour mode as a **severe** screen impairment, and §13.1 tells a
/// conforming app to turn it off — but a scanner cannot rely on that.
#[test]
fn survives_night_shift() {
    let base = render(&encode(Profile::C1));
    for (name, gains) in [
        ("night shift", [1.0f32, 0.85, 0.55]),
        ("true tone", [1.0, 0.95, 0.82]),
        ("vivid", [1.10, 1.0, 1.05]),
        ("cool", [0.82, 0.93, 1.0]),
    ] {
        assert_decodes(&color_cast(&base, gains), name);
    }
}

/// §12.3 step 3: "One reference sitting under a specular highlight can drag a global fit
/// badly, and on a glossy screen that is a routine occurrence rather than an edge case."
#[test]
fn survives_a_specular_blob() {
    let base = render(&encode(Profile::C1));
    for (cx, cy, radius, strength) in [
        (0.5f32, 0.5f32, 0.25f32, 0.5f32),
        (0.75, 0.3, 0.30, 0.5),
        (0.3, 0.7, 0.20, 0.6),
    ] {
        let damaged = specular(&base, cx, cy, radius, strength);
        assert_decodes(
            &damaged,
            &format!("specular at ({cx},{cy}) r={radius} s={strength}"),
        );
    }
}

#[test]
fn survives_sensor_noise() {
    let base = render(&encode(Profile::C1));
    for sigma in [0.02f32, 0.04, 0.06] {
        assert_decodes(&noise(&base, sigma, 0xC0FFEE), &format!("noise σ={sigma}"));
    }
}

#[test]
fn survives_defocus() {
    let base = render(&encode(Profile::C1));
    for radius in [1u32, 2, 3, 4] {
        assert_decodes(&blur(&base, radius), &format!("blur r={radius}"));
    }

    // Radius 4 is the last *reliable* step and is what this asserts. `just envelope` puts
    // the knee immediately after it: over 32 trials, 100% through 4 px, 28% at 5, 0% at 6.
    //
    // An earlier version of this test asserted radius 5 as well, on the strength of one
    // sample that happened to decode. A 28% point is not a gate — it passed until a change
    // that has nothing to do with defocus (D21's ring word) shifted the ring's colours and
    // the sample stopped landing the lucky side of the knee. The sweep is where a rate like
    // that belongs.
}

/// §1.4's whole premise: chroma is the low-bandwidth channel, and 4:2:0 is why. Splitting
/// the payload into a full-pitch luma layer and a separately-coded chroma layer is the
/// response, and this is the test that says it worked.
#[test]
fn survives_chroma_subsampling() {
    for profile in [Profile::C1, Profile::C2, Profile::C3] {
        let base = render(&encode(profile));
        assert_decodes(&chroma_subsample(&base), &format!("4:2:0 {profile:?}"));
    }
}

/// §8.3 interleaves each layer so a physical burst spreads across many codeword
/// positions. A finger over a corner of the symbol is the burst it was written for.
#[test]
fn survives_an_occluding_finger() {
    let base = render(&encode(Profile::C1));
    // A patch over the payload area, away from the finders.
    assert_decodes(&occlude(&base, 0.55, 0.55, 0.18, 0.18), "occlusion 18%");
}

/// §12.4: "Banding and glare move between frames, so this is very effective and much
/// cheaper than tightening the code rate."
///
/// Moving glare rather than noise, which is what this used to test. The ring detector (D17)
/// put the single-frame noise limit at σ ≈ 0.36, up from ≈ 0.06, and between there and
/// where detection itself gives out there is under 0.02 of σ in which accumulation wins — a
/// test pinned inside that window would measure seed luck. A saturating highlight wandering
/// across the symbol is the impairment §12.4 names: it defeats every frame here on its own,
/// and each frame loses a different part of the symbol.
#[test]
fn multi_frame_beats_single_frame() {
    let base = render(&encode(Profile::C1));
    let glared = |k: u64| {
        // Golden-angle steps put each frame's highlight somewhere new.
        let a = k as f32 * 2.399;
        // Radius 0.45 at strength 1.6. Both have had to grow three times: D18's canvas and
        // D19's smaller version left a fixed-fraction blob covering less of the symbol, and
        // D22's local luma read decodes 9 of 12 frames single-handed at the previous 0.35 /
        // 1.2. Here no frame decodes alone (0/12 from radius 0.40 / strength 1.4) and
        // accumulation still does up to 0.50 / 2.0 — this sits mid-window.
        specular(&base, 0.5 + 0.25 * a.cos(), 0.5 + 0.25 * a.sin(), 0.45, 1.6)
    };
    for k in 1..=12u64 {
        assert!(
            try_decode(&glared(k)).is_err(),
            "frame {k} should fail on its own"
        );
    }

    let mut acc = FrameAccumulator::new();
    let mut pushed = 0;
    for k in 1..=12u64 {
        let frame = glared(k);
        let view = ImageView::new(
            &frame.data,
            frame.width,
            frame.height,
            frame.width * 3,
            PixelFormat::Rgb8,
        )
        .unwrap();
        if acc.push(&view).is_ok() {
            pushed += 1;
        }
    }
    assert!(pushed >= 8, "only {pushed} of 12 frames located the symbol");

    let out = acc
        .try_decode(&Decoder::new())
        .expect("accumulated frames should decode");
    match out.packet {
        Packet::Unsigned { body, .. } => assert_eq!(body, payload()),
        other => panic!("{other:?}"),
    }
    assert!(out.diagnostics.frames_combined >= 8);
}

/// Impairments do not arrive one at a time — and they do not compose for free either.
/// Each of these is well inside its own limit above; stacked, they consume each other's
/// margin, so this case sits at roughly half of each individual envelope. The capture it
/// describes is an ordinary one: a phone held at the 10–20° §13.2 asks for, Night Shift
/// left on, a window reflected in the corner of the screen, and a JPEG in the middle.
#[test]
fn survives_a_realistic_combination() {
    let base = render(&encode(Profile::C1));
    let r = perspective(&base, 0.15, 12.0);
    let r = shading(&r, 0.25);
    let r = color_cast(&r, [1.0, 0.88, 0.62]);
    let r = specular(&r, 0.7, 0.35, 0.15, 0.35);
    let r = chroma_subsample(&r);
    let r = noise(&r, 0.03, 0xBEEF);
    assert_decodes(&r, "tilt + shading + night shift + glare + 4:2:0 + noise");
}

/// Degradation must produce a clean failure, never a wrong payload presented as success.
/// §9.1 makes the CRC the authoritative accept precisely so this holds.
#[test]
fn extreme_degradation_fails_closed() {
    let base = render(&encode(Profile::C1));
    for r in [
        noise(&base, 0.45, 9),
        blur(&base, 12),
        occlude(&base, 0.1, 0.1, 0.8, 0.8),
        specular(&base, 0.5, 0.5, 1.0, 1.0),
    ] {
        match try_decode(&r) {
            Err(_) => {}
            Ok(Packet::Unsigned { body, .. }) => {
                assert_eq!(
                    body,
                    payload(),
                    "decoder returned a WRONG payload as success"
                )
            }
            Ok(other) => panic!("unexpected {other:?}"),
        }
    }
}
