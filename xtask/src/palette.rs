//! Palette derivation search (§4.2, decisions D1/D2).
//!
//! `cargo run -p xtask -- palette` searches for the constants pinned in
//! `crates/lc-core/src/palette.rs`. D1 describes the palettes as "derived from the §4.2
//! constraints — iso-luminant hue states shared across levels, a single common `C*`, and
//! `C*` taken as high as the sRGB gamut allows at every corner", but until this task
//! existed nothing computed that last clause. The pinned values did not satisfy it:
//! `C4_CHROMA = 55` against a true limit of 47.9, and `C8_CHROMA = 40` against 39.4.
//!
//! ## What is being maximised
//!
//! For a candidate configuration — two `L*` levels and `k` hue angles shared between
//! them — the achievable common chroma is
//!
//! ```text
//! C(L0, L1, h) = min over the 2k corners of  maxC*(l, h)
//! ```
//!
//! where `maxC*` is the sRGB gamut boundary at that lightness and hue. The search
//! maximises that subject to §4.2: inter-level gap ≥ 40, hue separation ≥ 300°/k, and
//! `C* ≥ 35`. Intra-level spread is 0 by construction, since a hue state is one angle
//! used at both levels.
//!
//! ## Why the search is structured this way
//!
//! The objective is a *minimum* over corners, so for a fixed pair of levels the problem
//! becomes: given `m(h) = min(maxC*(L0,h), maxC*(L1,h))`, choose `k` hues from the circle
//! with adjacent gaps ≥ `300/k` maximising `min m(h)`. Binary-searching the answer turns
//! that into a feasibility question — "can `k` hues be placed using only angles where
//! `m(h) ≥ C`?" — which greedy placement settles in one pass. That is what keeps a search
//! over levels *and* hues tractable at 0.5° resolution.
//!
//! The report is deliberately more than the winning numbers. §4.2 is a floor, not an
//! objective, and two of the trade-offs it leaves open are load-bearing:
//!
//! * **Luma gap against chroma.** Both layers want margin and the gamut will not give
//!   both. The gap/chroma table prices that exchange rather than assuming 40 is right.
//! * **Hue orientation.** D1 chose green–magenta so the discriminating signal lands on
//!   G against R+B, where a Bayer sensor has twice the photosite density. That is a
//!   sensor argument the gamut knows nothing about, so the orientation table shows what
//!   each axis costs and leaves the choice to the caller.

use lc_core::color::{self, Lab, Srgb8};
use lc_core::palette::Palette;
use std::fmt::Write as _;

/// `L*` search bounds and resolution.
const L_MIN: f32 = 12.0;
const L_MAX: f32 = 96.0;
const L_STEP: f32 = 0.5;
/// Hue resolution in degrees. 720 samples around the circle.
const H_STEP: f32 = 0.5;
/// Chroma probe ceiling — far outside sRGB at every hue, so the bisection always brackets.
const C_PROBE: f32 = 200.0;
/// §4.2's `C* ≥ 35` floor.
const MIN_CHROMA: f32 = 35.0;
/// §4.2's inter-level luma gap floor.
const MIN_GAP: f32 = 40.0;
/// Headroom the search holds back on the luma gap and on hue separation.
///
/// The search works in continuous CIELAB; the palette that ships is 8-bit sRGB. Rounding
/// each corner to the nearest representable colour moves its `L*` and hue by up to a
/// couple of tenths, so a configuration sitting exactly on a §4.2 bound lands just under
/// it once quantised. Both margins are set well above the drift measured across the
/// candidates here, and cost about 0.75 of `C*` between them.
const GAP_MARGIN: f32 = 1.0;
const HUE_MARGIN: f32 = 0.5;

/// The sRGB gamut boundary, tabulated over `L*` × hue.
struct Gamut {
    nl: usize,
    nh: usize,
    maxc: Vec<f32>,
}

/// Largest in-gamut `C*` at a given lightness and hue angle.
fn max_chroma(l: f32, h_deg: f32) -> f32 {
    let rad = h_deg.to_radians();
    let (cos, sin) = (rad.cos(), rad.sin());
    let in_gamut = |c: f32| color::lab_to_linear(Lab::new(l, c * cos, c * sin)).in_gamut();
    if in_gamut(C_PROBE) {
        return C_PROBE;
    }
    let (mut lo, mut hi) = (0.0f32, C_PROBE);
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if in_gamut(mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

impl Gamut {
    fn build() -> Gamut {
        let nl = ((L_MAX - L_MIN) / L_STEP) as usize + 1;
        let nh = (360.0 / H_STEP) as usize;
        let mut maxc = vec![0.0; nl * nh];
        for li in 0..nl {
            let l = L_MIN + li as f32 * L_STEP;
            for hj in 0..nh {
                maxc[li * nh + hj] = max_chroma(l, hj as f32 * H_STEP);
            }
        }
        Gamut { nl, nh, maxc }
    }

    fn l_of(&self, li: usize) -> f32 {
        L_MIN + li as f32 * L_STEP
    }

    fn at(&self, li: usize, hj: usize) -> f32 {
        self.maxc[li * self.nh + hj]
    }
}

/// Greedy circular placement of `k` indices drawn from `allowed`, starting at `start`,
/// with every adjacent gap at least `step` indices.
///
/// Greedy-earliest is optimal here: placing a hue as early as the gap permits leaves the
/// most room for the rest, so if any placement from `start` exists this finds one.
fn place_from(nxt: &[usize], nh: usize, k: usize, step: usize, start: usize) -> Option<Vec<usize>> {
    let mut out = Vec::with_capacity(k);
    out.push(start);
    let mut cur = start;
    for _ in 1..k {
        let j = nxt[cur + step];
        if j == usize::MAX || j >= start + nh {
            return None;
        }
        out.push(j % nh);
        cur = j;
    }
    // The wrap-around gap closes the circle and is subject to the same bound.
    if start + nh - cur < step {
        return None;
    }
    Some(out)
}

/// Can `k` hues be placed using only angles where `m(h) ≥ c`?
fn feasible(m: &[f32], nh: usize, k: usize, step: usize, c: f32) -> Option<Vec<usize>> {
    // `nxt[i]` = the first allowed index at or after `i`, unwrapped over three laps so a
    // placement that crosses 0° never falls off the end.
    let mut nxt = vec![usize::MAX; 3 * nh];
    for i in (0..2 * nh).rev() {
        nxt[i] = if m[i % nh] >= c { i } else { nxt[i + 1] };
    }
    for (start, &avail) in m.iter().enumerate() {
        if avail >= c
            && let Some(hs) = place_from(&nxt, nh, k, step, start)
        {
            return Some(hs);
        }
    }
    None
}

/// A candidate palette configuration.
#[derive(Clone, Debug)]
struct Solution {
    chroma: f32,
    l0: f32,
    l1: f32,
    hues: Vec<f32>,
}

/// Best achievable common `C*` for one pair of levels.
fn best_for_levels(
    g: &Gamut,
    l0i: usize,
    l1i: usize,
    k: usize,
    step: usize,
    floor: f32,
) -> Option<(f32, Vec<usize>)> {
    let m: Vec<f32> = (0..g.nh).map(|h| g.at(l0i, h).min(g.at(l1i, h))).collect();
    let ub = m.iter().copied().fold(f32::MIN, f32::max);
    if ub < floor {
        return None;
    }
    let mut best = feasible(&m, g.nh, k, step, floor).map(|hs| (floor, hs))?;
    let (mut lo, mut hi) = (floor, ub);
    for _ in 0..30 {
        let mid = 0.5 * (lo + hi);
        match feasible(&m, g.nh, k, step, mid) {
            Some(hs) => {
                best = (mid, hs);
                lo = mid;
            }
            None => hi = mid,
        }
    }
    Some(best)
}

/// Best common `C*` for `k` hues shared across an arbitrary set of `L*` levels.
///
/// [`best_for_levels`] generalised from a pair to any set: `m(h)` is the minimum over
/// *every* level, because a hue state is one angle used at all of them. No floor — the
/// caller wants to see the number even when it misses §4.2, since the point is to price a
/// trade rather than to ship a palette.
fn best_for_level_set(
    g: &Gamut,
    levels: &[usize],
    k: usize,
    step: usize,
) -> Option<(f32, Vec<usize>)> {
    let m: Vec<f32> = (0..g.nh)
        .map(|h| {
            levels
                .iter()
                .map(|&li| g.at(li, h))
                .fold(f32::INFINITY, f32::min)
        })
        .collect();
    let ub = m.iter().copied().fold(f32::MIN, f32::max);
    if ub <= 0.0 {
        return None;
    }
    let mut best = feasible(&m, g.nh, k, step, 0.0).map(|hs| (0.0, hs))?;
    let (mut lo, mut hi) = (0.0f32, ub);
    for _ in 0..30 {
        let mid = 0.5 * (lo + hi);
        match feasible(&m, g.nh, k, step, mid) {
            Some(hs) => {
                best = (mid, hs);
                lo = mid;
            }
            None => hi = mid,
        }
    }
    Some(best)
}

/// Search every level pair meeting `required_gap` for the largest common `C*`.
///
/// `required_gap` is the gap the *shipped* palette must show, so [`GAP_MARGIN`] is added
/// before the level pair is tested.
fn search(g: &Gamut, k: usize, required_gap: f32) -> Option<Solution> {
    let step = ((300.0 / k as f32 + HUE_MARGIN) / H_STEP).ceil() as usize;
    let mut best: Option<Solution> = None;
    for l0i in 0..g.nl {
        for l1i in 0..g.nl {
            if g.l_of(l1i) - g.l_of(l0i) < required_gap + GAP_MARGIN {
                continue;
            }
            let floor = best.as_ref().map_or(MIN_CHROMA, |b| b.chroma);
            if let Some((c, hs)) = best_for_levels(g, l0i, l1i, k, step, floor)
                && best.as_ref().is_none_or(|b| c > b.chroma)
            {
                best = Some(Solution {
                    chroma: c,
                    l0: g.l_of(l0i),
                    l1: g.l_of(l1i),
                    hues: hs.iter().map(|&h| h as f32 * H_STEP).collect(),
                });
            }
        }
    }
    best
}

/// Best common `C*` when one hue is pinned to `h0` — the orientation landscape.
fn best_at_orientation(g: &Gamut, h0: f32, required_gap: f32) -> Option<Solution> {
    let step = ((150.0 + HUE_MARGIN) / H_STEP).ceil() as usize;
    let h0i = (h0 / H_STEP) as usize % g.nh;
    let mut best: Option<Solution> = None;
    for l0i in 0..g.nl {
        for l1i in 0..g.nl {
            if g.l_of(l1i) - g.l_of(l0i) < required_gap + GAP_MARGIN {
                continue;
            }
            // With k = 2 the partner is any hue at least 150° away in either direction.
            for d in step..=(g.nh - step) {
                let h1i = (h0i + d) % g.nh;
                let c = g
                    .at(l0i, h0i)
                    .min(g.at(l1i, h0i))
                    .min(g.at(l0i, h1i))
                    .min(g.at(l1i, h1i));
                if c >= MIN_CHROMA && best.as_ref().is_none_or(|b| c > b.chroma) {
                    best = Some(Solution {
                        chroma: c,
                        l0: g.l_of(l0i),
                        l1: g.l_of(l1i),
                        hues: vec![h0, h1i as f32 * H_STEP],
                    });
                }
            }
        }
    }
    best
}

/// Derive 8-bit colours the way `lc_core::palette::derive` does, so the realised metrics
/// below are the ones the crate will actually see.
fn derive(sol: &Solution) -> Vec<Srgb8> {
    let mut out = Vec::with_capacity(2 * sol.hues.len());
    for &l in &[sol.l0, sol.l1] {
        for &h in &sol.hues {
            out.push(color::lch_to_srgb8_gamut_clipped(l, sol.chroma, h).0);
        }
    }
    out
}

/// Largest chroma at or below `sol.chroma` whose derived palette still passes §4.2 after
/// 8-bit quantisation. Quantisation moves every corner slightly and the search works in
/// continuous Lab, so the two can disagree at the last decimal.
///
/// Only chroma is backed off, so a configuration that fails on the luma gap or on hue
/// separation cannot be rescued here — the search margins exist to keep that from
/// happening, and the error is returned rather than swallowed if it does.
fn back_off(sol: &Solution) -> Result<(Solution, Vec<Srgb8>), &'static str> {
    let mut c = sol.chroma;
    let mut last = "no candidate evaluated";
    while c >= MIN_CHROMA {
        let cand = Solution {
            chroma: c,
            ..sol.clone()
        };
        let colors = derive(&cand);
        match Palette::new(cand.hues.len(), &colors).validate() {
            Ok(_) => return Ok((cand, colors)),
            Err(e) => last = e,
        }
        c -= 0.05;
    }
    Err(last)
}

fn report_solution(s: &mut String, name: &str, sol: &Solution) {
    let (sol, colors) = match back_off(sol) {
        Ok(v) => v,
        Err(e) => {
            let _ = writeln!(s, "  {name}: derived palette violates {e}\n");
            return;
        }
    };
    let p = Palette::new(sol.hues.len(), &colors);
    let m = p
        .validate()
        .expect("back_off returns only validating palettes");

    let hues: Vec<String> = sol.hues.iter().map(|h| format!("{h:.1}°")).collect();
    let _ = writeln!(s, "  {name}");
    let _ = writeln!(
        s,
        "    L* {:.1} / {:.1}   C* {:.2}   hues {}",
        sol.l0,
        sol.l1,
        sol.chroma,
        hues.join(", ")
    );
    let _ = writeln!(
        s,
        "    realised: gap {:.2} (≥40)  spread {:.2} (≤12)  hue sep {:.1}° (≥{:.0}°)  C* {:.2} (≥35)",
        m.inter_level_gap,
        m.intra_level_spread,
        m.min_hue_sep,
        300.0 / p.hues as f32,
        m.min_chroma
    );
    let _ = write!(s, "    colours:");
    for c in &colors {
        let _ = write!(s, " #{:06X}", c.to_hex());
    }
    let _ = writeln!(s, "\n");
}

/// Emit the search report and paste-ready constants.
pub fn report() -> String {
    let g = Gamut::build();
    let mut s = String::new();

    let _ = writeln!(
        s,
        "§4.2 palette search — sRGB gamut tabulated at {L_STEP} L* × {H_STEP}° over \
         {} × {} samples\n",
        g.nl, g.nh
    );

    let _ = writeln!(
        s,
        "Best configuration at the §4.2 minimum gap of {MIN_GAP:.0}:\n"
    );
    let c4 = search(&g, 2, MIN_GAP);
    let c8 = search(&g, 4, MIN_GAP);
    // k = 8 is the proposed `C4`. Reported alongside the shipped configurations so the
    // cost of the extra hue bit is a measurement rather than an estimate: the binding
    // constraint is a *single common* C* reachable at all 2k corners, and adding hues
    // forces directions where the sRGB gamut is tight.
    let c16 = search(&g, 8, MIN_GAP);
    if let Some(sol) = &c4 {
        report_solution(&mut s, "k = 2  (C1 / C2)", sol);
    }
    if let Some(sol) = &c8 {
        report_solution(&mut s, "k = 4  (C3)", sol);
    }
    match &c16 {
        Some(sol) => report_solution(&mut s, "k = 8  (C4, proposed)", sol),
        None => {
            let _ = writeln!(
                s,
                "k = 8  (C4, proposed): **no solution** at the §4.2 floors                  (gap >= {MIN_GAP:.0}, separation >= {:.1}deg, C* >= 35).\n",
                300.0 / 8.0
            );
        }
    }

    // ---- evenly-spaced hues: the ceiling for a k-hue profile -------------------
    //
    // The searches above place hues where the gamut is roomiest, which is why `C3`'s are
    // uneven. This asks the blunter question the `C4` proposal turns on: with k hues
    // spaced evenly around the circle, what single C* does sRGB reach at **all** 2k
    // corners? That is the ceiling any k-hue palette must live under, and it is what
    // collapses as k grows — the constraint is not the angular separation but the fact
    // that one common C* must clear the gamut's tightest direction.
    let _ = writeln!(
        s,
        "\nEvenly-spaced hues — best common C* over all 2k corners, searched over both\n\
         L* levels and the starting hue offset. §4.2 requires C* >= 35:\n\n\
           | k | colours | bits/module | best C* | L* | offset | meets C* >= 35 |\n\
           |---:|---:|---:|---:|:---|---:|:---|"
    );
    for k in [2usize, 4, 8, 16] {
        let step = g.nh / k;
        let mut best: Option<(f32, f32, f32, f32)> = None;
        for off in 0..step {
            for li0 in 0..g.nl {
                let l0 = g.l_of(li0);
                for li1 in (li0 + 1)..g.nl {
                    let l1 = g.l_of(li1);
                    if l1 - l0 < MIN_GAP {
                        continue;
                    }
                    let mut c = f32::INFINITY;
                    for j in 0..k {
                        let hj = (off + j * step) % g.nh;
                        c = c.min(g.at(li0, hj)).min(g.at(li1, hj));
                    }
                    if best.is_none_or(|b| c > b.0) {
                        best = Some((c, l0, l1, off as f32 * H_STEP));
                    }
                }
            }
        }
        if let Some((c, l0, l1, off)) = best {
            let _ = writeln!(
                s,
                "  | {k} | {} | {} | {c:.1} | {l0:.1}/{l1:.1} | {off:.1} | {} |",
                2 * k,
                1 + (k as f32).log2() as usize,
                if c >= MIN_CHROMA { "yes" } else { "**no**" }
            );
        }
    }

    // ---- C4b: four luma levels x four hues --------------------------------------
    //
    // The alternative to eight hues: spend the extra bit on luma. The luma axis becomes
    // hierarchical — an MSB split with §4.2's >= 40 gap, so a monochrome reader still
    // recovers the top bit, and an LSB step `g` inside each half:
    //
    //     L0 --g-- L1 ======= MSB gap >= 40 ======= L2 --g-- L3
    //
    // Chroma does *not* come free. Hue states are shared across all four levels, and the
    // outer two reach toward black and white where sRGB has almost no chroma to give, so
    // the common C* is set by the extremes. This is the trade that decides whether C4b is
    // worth building, so it is swept rather than asserted.
    let _ = writeln!(
        s,
        "\nC4b — four luma levels x four hues (MSB gap >= 40, symmetric LSB step g).\n\
         L* table spans {L_MIN}..{L_MAX}, so 2g + 40 <= {:.0} caps g at {:.0}.\n\n\
           | LSB g | MSB gap | best C* | L* levels | hues | chroma d | luma LSB vs C3 | chroma vs C3 |\n\
           |---:|---:|---:|:---|:---|---:|---:|---:|",
        L_MAX - L_MIN,
        ((L_MAX - L_MIN) - MIN_GAP) / 2.0
    );
    let step4 = ((300.0 / 4.0 + HUE_MARGIN) / H_STEP).ceil() as usize;
    // Shipped C3 references: inter-level gap 40.77, chroma distance at C* 42.7 / 75deg.
    let c3_luma = 40.77f32;
    let c3_d = 2.0 * 42.7f32 * (75.0f32.to_radians() / 2.0).sin();
    for g_lsb in [8.0f32, 10.0, 12.0, 14.0, 16.0, 18.0, 20.0, 22.0] {
        let gi = (g_lsb / L_STEP).round() as usize;
        let mut best: Option<(f32, f32, [f32; 4], Vec<usize>)> = None;
        let min_msb = (MIN_GAP / L_STEP).round() as usize;
        for msb in min_msb..g.nl {
            for l0 in 0..g.nl {
                let lv = [l0, l0 + gi, l0 + gi + msb, l0 + 2 * gi + msb];
                if lv[3] >= g.nl {
                    break;
                }
                if let Some((c, hs)) = best_for_level_set(&g, &lv, 4, step4)
                    && best.as_ref().is_none_or(|b| c > b.0)
                {
                    best = Some((
                        c,
                        msb as f32 * L_STEP,
                        [g.l_of(lv[0]), g.l_of(lv[1]), g.l_of(lv[2]), g.l_of(lv[3])],
                        hs,
                    ));
                }
            }
        }
        match best {
            Some((c, msb, lv, hs)) => {
                let hues: Vec<String> = hs
                    .iter()
                    .map(|&h| format!("{:.0}", h as f32 * H_STEP))
                    .collect();
                let d = 2.0 * c * (75.0f32.to_radians() / 2.0).sin();
                let _ = writeln!(
                    s,
                    "  | {g_lsb:.0} | {msb:.1} | {c:.1} | {:.0}/{:.0}/{:.0}/{:.0} | {} | {d:.1} | {:+.1} dB | {:+.1} dB |",
                    lv[0],
                    lv[1],
                    lv[2],
                    lv[3],
                    hues.join("/"),
                    20.0 * (g_lsb / c3_luma).log10(),
                    20.0 * (d / c3_d).log10(),
                );
            }
            None => {
                let _ = writeln!(s, "  | {g_lsb:.0} | — | — | — | — | — | — | — |");
            }
        }
    }

    // ---- what luma margin costs in chroma -------------------------------------
    let _ = writeln!(
        s,
        "Gap/chroma trade — §4.2 sets a floor of 40, not a target. Buying luma margin\n\
         above it is paid for in chroma, and this is the exchange rate. The gap column is\n\
         what the shipped 8-bit palette realises, not the nominal level difference:\n"
    );
    for k in [2usize, 4, 8] {
        let _ = writeln!(
            s,
            "  k = {k}\n  | gap | C* | L* | hues |\n  |---:|---:|---:|:---|"
        );
        let mut gap = MIN_GAP;
        while gap <= 54.0 {
            match search(&g, k, gap) {
                Some(sol) => {
                    let hues: Vec<String> = sol.hues.iter().map(|h| format!("{h:.1}°")).collect();
                    let _ = writeln!(
                        s,
                        "  | {:.0} | {:.2} | {:.1}/{:.1} | {} |",
                        gap,
                        sol.chroma,
                        sol.l0,
                        sol.l1,
                        hues.join(", ")
                    );
                }
                None => {
                    let _ = writeln!(s, "  | {gap:.0} | — | — | — |");
                }
            }
            gap += 2.0;
        }
        let _ = writeln!(s);
    }

    // ---- where the hue axis can point ------------------------------------------
    let _ = writeln!(
        s,
        "\nHue orientation, k = 2 — the best common C* with one hue pinned to each angle.\n\
         The gamut has no opinion about sensors; D1 picked green–magenta so the\n\
         discriminating signal is G against R+B, where a Bayer sensor has twice the\n\
         photosite density. This prices that choice against the alternatives:\n"
    );
    let _ = writeln!(s, "  |  h0 | partner |    L* | C* |");
    let _ = writeln!(s, "  |---:|---:|---:|---:|");
    let mut h = 0.0f32;
    while h < 360.0 {
        if let Some(sol) = best_at_orientation(&g, h, MIN_GAP) {
            let _ = writeln!(
                s,
                "  | {:.0}° | {:.0}° | {:.0}/{:.0} | {:.2} |",
                h, sol.hues[1], sol.l0, sol.l1, sol.chroma
            );
        }
        h += 15.0;
    }

    // ---- paste-ready ------------------------------------------------------------
    let _ = writeln!(s, "\nConstants for crates/lc-core/src/palette.rs:\n");
    if let Some(Ok(sol)) = c4.as_ref().map(back_off) {
        emit(&mut s, "C4", &sol.0, &sol.1);
    }
    if let Some(Ok(sol)) = c8.as_ref().map(back_off) {
        emit(&mut s, "C8", &sol.0, &sol.1);
    }
    s
}

fn emit(s: &mut String, prefix: &str, sol: &Solution, colors: &[Srgb8]) {
    let hues: Vec<String> = sol.hues.iter().map(|h| format!("{h:.1}")).collect();
    let _ = writeln!(
        s,
        "pub const {prefix}_HUES: [f32; {}] = [{}];",
        sol.hues.len(),
        hues.join(", ")
    );
    let _ = writeln!(s, "pub const {prefix}_L0: f32 = {:.1};", sol.l0);
    let _ = writeln!(s, "pub const {prefix}_L1: f32 = {:.1};", sol.l1);
    let _ = writeln!(s, "pub const {prefix}_CHROMA: f32 = {:.2};", sol.chroma);
    let _ = writeln!(
        s,
        "pub const {prefix}_COLORS: [Srgb8; {}] = [",
        colors.len()
    );
    for c in colors {
        let _ = writeln!(s, "    Srgb8::from_hex(0x{:06X}),", c.to_hex());
    }
    let _ = writeln!(s, "];\n");
}
