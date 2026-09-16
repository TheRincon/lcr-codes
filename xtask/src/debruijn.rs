//! Position sequences for the §6.4 reference border.
//!
//! ## What this is for
//!
//! §6.4's border currently cycles the palette in index order, which makes it a good set of
//! illumination references (§12.3) and nothing else: a periodic sequence tells a decoder
//! nothing about *where on the ring* it is looking. Replacing the period with a sequence
//! whose every `k`-symbol window is unique turns the same cells into an absolute position
//! reference — orientation, which edge, and how far along it — for **no extra modules**.
//!
//! The alternative, a localised asymmetry (a gap, or a marked corner), is a single point
//! of failure on a channel whose dominant impairment is specular glare (§3). §12.3 already
//! expects to *discard* border references a highlight has ruined. A distributed sequence
//! needs one readable fragment anywhere on the ring; a marked corner needs that corner.
//!
//! Nothing consumes these tables yet — the detector work they enable is separate, and this
//! module exists so the sequences can be generated and their properties proved first.
//!
//! ## What is generated, and what is checked
//!
//! A **cyclic** binary sequence per version, over the ring's cell count, satisfying:
//!
//! * every window of `k` consecutive cells is distinct — the property the whole idea rests
//!   on, verified by brute force over the cyclic windows rather than trusted to the
//!   construction;
//! * no run of identical luma longer than `MAX_RUN`, so the ring always carries module
//!   pitch the way a timing pattern does, and never presents a long featureless edge;
//! * roughly even dark/light balance, so binarisation has no DC bias to fight.
//!
//! The binary sequence is the **luma** layer, which is what §12.1 requires detection to
//! work from. Colour rides on top: each cell takes a palette entry whose luma *level*
//! matches its bit, round-robin within the eligible set, so all §6.4 border colours still
//! appear and stay spatially spread for the §12.3 fit.
//!
//! ## Construction
//!
//! A cyclic sequence of length `p` whose `k`-windows are distinct is a closed trail of `p`
//! distinct edges in the binary de Bruijn graph of order `k`: vertices are `(k−1)`-bit
//! words, and the edge leaving a vertex on symbol `b` is the `k`-bit window formed by
//! appending it. Randomised depth-first search with backtracking finds one easily while
//! `p` stays well below the `2^k` edges available.
//!
//! **Window length is a trade and the report prints both sides of it.** A smaller `k`
//! localises from fewer cells, which matters when glare has left only a short fragment
//! readable. A larger `k` leaves more slack in the graph, which buys Hamming separation
//! between windows — and separation is what decides whether a single misread cell moves
//! you to a *wrong* position or to no position at all. The minimum distance achieved is
//! measured here rather than designed for, because the right target depends on a detector
//! that does not exist yet.

use std::fmt::Write as _;

/// Longest permitted run of identical luma.
///
/// Four is the shortest cap that leaves the search room at the densest version. A long run
/// is bad twice over: it gives no module pitch, and it makes a stretch of the ring look
/// like a plain edge to a detector hunting for one.
const MAX_RUN: usize = 4;

/// Search effort before giving up on a seed and restarting.
const BUDGET: u64 = 40_000_000;

/// Number of distinct sequence positions around the ring, for a given symbol side.
///
/// **Straight from `lc_core::border`, not recomputed here.** This used to derive the ring
/// from its own copy of the canvas geometry, and when D18 narrowed the gap between grid and
/// ring the generator kept emitting sequences for the old, longer ring: every version's table
/// was quietly wrong, and `every_sequence_window_is_unique` in lc-core was what caught it.
fn ring_positions(side: u32) -> usize {
    lc_core::border::positions(side as u16)
}

/// Every version's side length, per §6.1.
fn sides() -> impl Iterator<Item = (u8, u32)> {
    (1u8..=32).map(|v| (v, 21 + 4 * (v as u32 - 1)))
}

// ---------------------------------------------------------------- generation

struct Search {
    p: usize,
    edge_mask: u32,
    vertex_mask: u32,
    used: Vec<bool>,
    seq: Vec<u8>,
    budget: u64,
    rng: u64,
    /// Ring corners, forced dark — see [`corners`].
    forced_dark: [usize; 4],
}

impl Search {
    fn next_bit(&mut self) -> u64 {
        // xorshift64*, so the search is reproducible from its seed.
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Depth-first walk. `state` holds the previous `k−1` symbols.
    fn walk(&mut self, state: u32, start: u32, run: usize, last: u8) -> bool {
        if self.seq.len() == self.p {
            // Closing the cycle means returning to the vertex we began at, so that the
            // wrap-around windows are edges we actually traversed.
            return state == start && self.wrap_run_ok();
        }
        if self.budget == 0 {
            return false;
        }
        self.budget -= 1;

        let first = (self.next_bit() & 1) as u8;
        let forced = self.forced_dark.contains(&self.seq.len());
        for i in 0..2u8 {
            let bit = first ^ i;
            if forced && bit != 0 {
                continue;
            }
            let run = if bit == last { run + 1 } else { 1 };
            if run > MAX_RUN {
                continue;
            }
            let edge = (((state << 1) | bit as u32) & self.edge_mask) as usize;
            if self.used[edge] {
                continue;
            }
            self.used[edge] = true;
            self.seq.push(bit);
            let next = (edge as u32) & self.vertex_mask;
            if self.walk(next, start, run, bit) {
                return true;
            }
            self.seq.pop();
            self.used[edge] = false;
        }
        false
    }

    /// The run constraint has to hold across the seam too, or the cycle has a flat spot
    /// exactly where two ends meet.
    fn wrap_run_ok(&self) -> bool {
        let n = self.seq.len();
        let mut run = 0;
        for i in 0..n {
            if self.seq[(n - 1 + n - i) % n] == self.seq[n - 1] {
                run += 1;
            } else {
                break;
            }
        }
        let mut lead = 0;
        for i in 0..n {
            if self.seq[i] == self.seq[0] {
                lead += 1;
            } else {
                break;
            }
        }
        if self.seq[0] != self.seq[n - 1] {
            return true;
        }
        run + lead <= MAX_RUN
    }
}

/// The four ring positions that sit on the corners of the outer square.
///
/// **Forced dark (D17).** The finder-less detector finds a symbol by the convex hull of its
/// dark pixels, and the hull's corners are only guaranteed to be the ring's corners if those
/// four cells are dark. A light corner would let the hull cut across it and bend the
/// homography at exactly the points it is fitted from.
fn corners(p: usize) -> [usize; 4] {
    let s = (p + 4) / 4;
    [0, s - 1, 2 * (s - 1), 3 * (s - 1)]
}

/// A cyclic binary sequence of length `p` whose every `k`-window is distinct.
fn generate(p: usize, k: usize) -> Option<Vec<u8>> {
    for seed in 1u64..=64 {
        let vertex_bits = k - 1;
        let mut s = Search {
            p,
            edge_mask: (1u32 << k) - 1,
            vertex_mask: (1u32 << vertex_bits) - 1,
            used: vec![false; 1 << k],
            seq: Vec::with_capacity(p),
            budget: BUDGET,
            rng: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1,
            forced_dark: corners(p),
        };
        let start = (s.rng as u32) & s.vertex_mask;
        if s.walk(start, start, 0, 2) {
            return Some(s.seq);
        }
    }
    None
}

// ---------------------------------------------------------------- verification

/// Extract the `p` cyclic windows, as integers.
fn windows(seq: &[u8], k: usize) -> Vec<u32> {
    let p = seq.len();
    (0..p)
        .map(|i| {
            let mut w = 0u32;
            for j in 0..k {
                w = (w << 1) | seq[(i + j) % p] as u32;
            }
            w
        })
        .collect()
}

/// Independent check that every cyclic window is distinct.
///
/// Deliberately not trusting the construction: the graph walk *should* guarantee this, but
/// an off-by-one in how a window maps to an edge would produce a sequence that looks right
/// and localises to the wrong cell. Brute force is cheap at these lengths.
fn windows_unique(seq: &[u8], k: usize) -> bool {
    let mut w = windows(seq, k);
    w.sort_unstable();
    w.windows(2).all(|pair| pair[0] != pair[1])
}

/// Smallest Hamming distance between any two distinct windows.
///
/// The number that decides what a single misread cell costs: at distance 1 it silently
/// relocates you, at 3 it is correctable. Measured, not targeted — see the module docs.
fn min_distance(seq: &[u8], k: usize) -> u32 {
    let w = windows(seq, k);
    let mut best = k as u32;
    for i in 0..w.len() {
        for j in (i + 1)..w.len() {
            let d = (w[i] ^ w[j]).count_ones();
            if d < best {
                best = d;
            }
        }
    }
    best
}

/// Shortest readable arc that still localises uniquely, under `errors` misread cells.
///
/// **This is the operative number, not the minimum window distance.** A decoder does not
/// read exactly `k` cells and look them up; it reads whatever arc of the ring is not
/// obscured and correlates it against all `p` rotations, taking the best. Minimum window
/// distance describes the worst case where exactly `k` cells are visible — which is not the
/// case a glare blob produces. What matters is how much *contiguous* ring has to survive.
///
/// Measured by trial rather than derived: pick a random start, take an arc, corrupt
/// `errors` cells of it, correlate against every rotation, and require the true start to
/// win outright. Returns the smallest arc length that never failed.
fn arc_to_localise(seq: &[u8], errors: usize, trials: usize, rng: &mut u64) -> Option<usize> {
    let p = seq.len();
    let mut next = || {
        *rng ^= *rng >> 12;
        *rng ^= *rng << 25;
        *rng ^= *rng >> 27;
        rng.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };

    for len in 4..=48usize {
        let mut ok = true;
        for _ in 0..trials {
            let start = (next() % p as u64) as usize;
            let mut arc: Vec<u8> = (0..len).map(|i| seq[(start + i) % p]).collect();
            for _ in 0..errors {
                let at = (next() % len as u64) as usize;
                arc[at] ^= 1;
            }
            // Correlate against every rotation; the true start must win outright.
            let (mut best, mut best_at, mut ties) = (usize::MAX, 0usize, 0usize);
            for r in 0..p {
                let mut d = 0;
                for (i, &b) in arc.iter().enumerate() {
                    if seq[(r + i) % p] != b {
                        d += 1;
                    }
                }
                if d < best {
                    best = d;
                    best_at = r;
                    ties = 1;
                } else if d == best {
                    ties += 1;
                }
            }
            if best_at != start || ties != 1 {
                ok = false;
                break;
            }
        }
        if ok {
            return Some(len);
        }
    }
    None
}

fn longest_run(seq: &[u8]) -> usize {
    let n = seq.len();
    let mut best = 1;
    let mut run = 1;
    for i in 1..n * 2 {
        if seq[i % n] == seq[(i - 1) % n] {
            run += 1;
            best = best.max(run);
        } else {
            run = 1;
        }
    }
    best.min(n)
}

// ---------------------------------------------------------------- colour

/// Assign a border colour index to every cell.
///
/// The luma bit fixes which entries are eligible — §6.4's cycle is the palette followed by
/// white and black, so a dark cell may be any `L0` palette entry or black, and a light cell
/// any `L1` entry or white. Round-robin within the eligible set keeps every colour present
/// and spatially spread, which is what the §12.3 fit wants from the border.
fn colourise(seq: &[u8], hues: usize) -> Vec<u8> {
    // Border cycle order: palette indices 0..2·hues, then white, then black.
    let dark: Vec<u8> = (0..hues as u8)
        .chain(std::iter::once(2 * hues as u8 + 1))
        .collect();
    let light: Vec<u8> = (hues as u8..2 * hues as u8)
        .chain(std::iter::once(2 * hues as u8))
        .collect();
    let (mut di, mut li) = (0usize, 0usize);
    seq.iter()
        .map(|&b| {
            if b == 0 {
                let c = dark[di % dark.len()];
                di += 1;
                c
            } else {
                let c = light[li % light.len()];
                li += 1;
                c
            }
        })
        .collect()
}

// ---------------------------------------------------------------- report

/// Smallest window length giving the search real slack.
///
/// `2^k ≥ 2·p` — half the de Bruijn graph's edges left unused. At the theoretical minimum
/// (`2^k ≥ p`) the walk has to consume nearly every edge, which both makes the search hard
/// and drives the minimum Hamming distance to 1.
fn window_for(p: usize) -> usize {
    (1..=16).find(|k| (1usize << k) >= 2 * p).unwrap()
}

/// Emit the sequences as a committed Rust table.
///
/// Generated offline and committed, in the same spirit as §8.2's "publish as static tables
/// in the crate — do not generate at runtime": the search is a backtracking walk with a
/// budget, which has no business running inside a decoder.
///
/// Packed one bit per position, LSB first, so the whole table is ~2.4 KB — it has to fit
/// the scanner, which §14.2 keeps under 200 KB and which is already at 96% of that.
pub fn table() -> String {
    let mut s = String::new();
    s.push_str(
        "//! §6.4 border position sequences — GENERATED, DO NOT EDIT.\n\
         //!\n\
         //! Regenerate with `just debruijn-table`. Properties are proved by\n\
         //! `just debruijn`, which writes `docs/border-sequence.md`, and re-checked here by\n\
         //! `every_sequence_window_is_unique`.\n\
         //!\n\
         //! Each entry is one version's cyclic sequence over the border ring, packed one\n\
         //! bit per position, LSB first. A set bit is a **light** cell, a clear bit dark —\n\
         //! the luma layer §12.1 detects from. Colour rides on top; see `border::cell_color`.\n\n",
    );
    let _ = writeln!(
        s,
        "/// Window length per version: the number of consecutive positions that identify\n\
         /// a location on the ring uniquely. Indexed by `version - 1`.\n\
         pub static WINDOW: [u8; 32] = ["
    );
    let mut windows = Vec::new();
    let mut seqs = Vec::new();
    for (_, side) in sides() {
        let p = ring_positions(side);
        let k = window_for(p);
        windows.push(k);
        seqs.push(generate(p, k).expect("sequence generation must succeed"));
    }
    for row in windows.chunks(16) {
        let _ = writeln!(
            s,
            "    {}",
            row.iter()
                .map(|k| format!("{k},"))
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
    s.push_str("];\n\n");

    let _ = writeln!(
        s,
        "/// Packed sequences, indexed by `version - 1`. Bit `i` of the slice is position\n\
         /// `i` around the ring; set means light.\n\
         pub static SEQUENCES: [&[u8]; 32] = ["
    );
    for seq in &seqs {
        let mut bytes = vec![0u8; seq.len().div_ceil(8)];
        for (i, &b) in seq.iter().enumerate() {
            if b == 1 {
                bytes[i / 8] |= 1 << (i % 8);
            }
        }
        let _ = writeln!(
            s,
            "    &[{}],",
            bytes
                .iter()
                .map(|b| format!("0x{b:02x}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    s.push_str("];\n");
    s
}

pub fn report() -> String {
    let mut s = String::new();
    let _ = writeln!(s, "# §6.4 border position sequences\n");
    let _ = writeln!(
        s,
        "Cyclic binary sequences over the reference-border ring, one per version, in which\n\
         every window of `k` consecutive cells is unique. The luma layer detection reads\n\
         (§12.1); colour rides on top for the §12.3 references.\n\n\
         `min dist` is the smallest Hamming distance between any two windows — what a\n\
         single misread cell costs. `max run` is the longest stretch of one luma, capped at\n\
         {MAX_RUN} so the ring always carries module pitch.\n"
    );
    let _ = writeln!(
        s,
        "| v | side | ring positions | k | unique | min dist | max run | balance |"
    );
    let _ = writeln!(s, "|---|---:|---:|---:|:--:|---:|---:|---:|");

    let mut failures = 0;
    for (v, side) in sides() {
        let p = ring_positions(side);
        let k = window_for(p);
        let Some(seq) = generate(p, k) else {
            let _ = writeln!(
                s,
                "| {v} | {side} | {p} | {k} | **NOT FOUND** | — | — | — |"
            );
            failures += 1;
            continue;
        };
        let unique = windows_unique(&seq, k);
        let dist = min_distance(&seq, k);
        let run = longest_run(&seq);
        let ones = seq.iter().filter(|&&b| b == 1).count();
        let balance = 100.0 * ones as f64 / p as f64;
        if !unique || run > MAX_RUN {
            failures += 1;
        }
        let _ = writeln!(
            s,
            "| {v} | {side} | {p} | {k} | {} | {dist} | {run} | {balance:.0}% |",
            if unique { "yes" } else { "**NO**" },
        );
    }

    let _ = writeln!(s);
    if failures == 0 {
        let _ = writeln!(
            s,
            "All 32 versions generated and verified: every cyclic window is distinct, no run\n\
             exceeds {MAX_RUN}, and the checks are brute force over the extracted windows\n\
             rather than an appeal to the construction."
        );
    } else {
        let _ = writeln!(s, "**{failures} version(s) failed.** See the table.");
    }

    let _ = writeln!(
        s,
        "\n## Minimum window distance is 1, and that is not the number to judge this on\n\n\
         Every version above reports `min dist` 1: some pair of windows differs in a single\n\
         cell. That is not a flaw in the search, it is sphere-packing. Separating `p`\n\
         windows by distance 3 needs `p·(1+k) ≤ 2^k`, which at v1 means a window of **12**\n\
         cells rather than 9, and at v32 **15** rather than 12. The constraint is real and\n\
         the cost is a longer readable fragment.\n\n\
         But a decoder does not read exactly `k` cells and look them up. It reads whatever\n\
         arc of the ring is not obscured and correlates it against all `p` rotations. A\n\
         single misread cell then costs nothing as long as the arc is long enough for the\n\
         true rotation to win outright — which is what the table below measures, by\n\
         corrupting arcs and checking the correct position still wins.\n"
    );
    let _ = writeln!(
        s,
        "| v | ring positions | k | clean | 1 misread | 2 misreads | worst case, as % of ring |"
    );
    let _ = writeln!(s, "|---|---:|---:|---:|---:|---:|---:|");
    let mut rng = 0x243F_6A88_85A3_08D3u64;
    for v in [1u8, 5, 12, 32] {
        let side = 21 + 4 * (v as u32 - 1);
        let p = ring_positions(side);
        let k = window_for(p);
        let Some(seq) = generate(p, k) else { continue };
        let a0 = arc_to_localise(&seq, 0, 60, &mut rng);
        let a1 = arc_to_localise(&seq, 1, 60, &mut rng);
        let a2 = arc_to_localise(&seq, 2, 60, &mut rng);
        let show = |a: Option<usize>| a.map_or("—".to_string(), |n| n.to_string());
        let pct = a2.map_or(String::from("—"), |n| {
            format!("{:.1}%", 100.0 * n as f64 / p as f64)
        });
        let _ = writeln!(
            s,
            "| {v} | {p} | {k} | {} | {} | {} | {pct} |",
            show(a0),
            show(a1),
            show(a2),
        );
    }
    let _ = writeln!(
        s,
        "\nMeasured over 60 randomised trials per arc length, the true position required to\n\
         win outright — ties count as failures. Read the last column as the answer to \
         \"how much of the ring does glare have to leave me?\""
    );

    // One worked example, so the shape is visible without running anything.
    let p = ring_positions(21);
    let k = window_for(p);
    if let Some(seq) = generate(p, k) {
        let _ = writeln!(s, "\n## v1 worked example — {p} cells, window {k}\n");
        let luma: String = seq
            .iter()
            .map(|&b| if b == 1 { 'L' } else { 'D' })
            .collect();
        let _ = writeln!(s, "Luma sequence, the layer §12.1 detects from:\n");
        let _ = writeln!(s, "```");
        for chunk in luma.as_bytes().chunks(72) {
            let _ = writeln!(s, "{}", std::str::from_utf8(chunk).unwrap());
        }
        let _ = writeln!(s, "```\n");
        let colours = colourise(&seq, 2);
        let counts = (0..6)
            .map(|c| colours.iter().filter(|&&x| x == c).count())
            .collect::<Vec<_>>();
        let _ = writeln!(
            s,
            "Colour assignment for `C1`, counted by border-cycle index \
             (0–1 dark hues, 2–3 light hues, 4 white, 5 black):\n"
        );
        let _ = writeln!(s, "```");
        let _ = writeln!(s, "{counts:?}");
        let _ = writeln!(s, "```\n");
        let _ = writeln!(
            s,
            "Every border colour still appears, which is what §12.3 needs from the ring — \
             the sequence changes the *order*, not the coverage."
        );
    }
    s
}
