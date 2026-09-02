//! Quasi-cyclic LDPC encode and soft-decision decode (§8.2, §12.4).
//!
//! ## Rate selection
//!
//! §8.2's table, with the layer asymmetry made explicit:
//!
//! | Level | Luma rate | Chroma rate |
//! |---|---|---|
//! | 0 | 0.85 | 0.65 |
//! | 1 | 0.75 | 0.55 |
//! | 2 | 0.65 | 0.45 |
//! | 3 | 0.55 | 0.35 |
//!
//! ## Fitting a fixed base graph to an arbitrary layer length
//!
//! Layer lengths are set by geometry, not by the code: a v1 `C2` chroma layer is 26 bits
//! and a v32 `C1` luma layer is 19406. A base graph with `n_b` columns lifted by `Z`
//! produces `Z·n_b` coded bits, which will not equal the layer length, so the code is
//! **shortened**: `Z` is the smallest lift that covers the layer, and the surplus
//! `s = Z·n_b − n_tx` information bits are fixed to zero and not transmitted. The decoder
//! knows they are zero and enters them at infinite LLR, so shortening costs nothing but
//! rate.
//!
//! ```text
//! Z = ⌈n_tx / n_b⌉        s = Z·n_b − n_tx        K = Z·k_b − s
//! ```
//!
//! Which requires `Z·m_b < n_tx` — the parity part alone must fit. Short layers fail that
//! test against the 60-column family, which is why [`tables`] carries 20- and 10-column
//! families as well; [`Code::new`] takes the largest graph that fits, since a wider base
//! graph means a lower check-node degree and a better-conditioned min-sum decode.
//!
//! Because shortening removes information bits but not parity bits, the *effective* rate
//! of a heavily shortened code is below nominal. That is the safe direction — short
//! layers are the ones with the least room to spare — and [`Code::effective_rate`]
//! reports it rather than the table value.
//!
//! ## Decoding
//!
//! Normalised min-sum, which is what makes §12.4's "soft information is worth 2 dB"
//! claim collectible. Sum-product would be marginally better and much more expensive in
//! a WASM bundle; the normalisation factor recovers most of the gap.

use super::tables::{self, BaseGraph};
use crate::error::LcError;
use alloc::vec;
use alloc::vec::Vec;

/// The six rates, in [`tables::BY_RATE_LARGE`] index order.
pub const RATES: [f32; 6] = [0.85, 0.75, 0.65, 0.55, 0.45, 0.35];

/// Rate index for the luma layer at ECC level `0..=3` (§8.2).
pub const LUMA_RATE_INDEX: [usize; 4] = [0, 1, 2, 3];
/// Rate index for the chroma layer at ECC level `0..=3` (§8.2).
pub const CHROMA_RATE_INDEX: [usize; 4] = [2, 3, 4, 5];

/// Default min-sum iteration cap.
pub const MAX_ITERATIONS: u16 = 50;

/// Min-sum normalisation factor. 0.75 is the standard choice for `dv = 3` graphs.
const ALPHA: f32 = 0.75;

/// LLR magnitude used for a bit the decoder knows with certainty (a shortened zero).
const CERTAIN: f32 = 1e6;

/// A concrete code: a base graph, a lift, and a shortening.
#[derive(Clone, Debug)]
pub struct Code {
    base: &'static BaseGraph,
    z: usize,
    /// Information bits actually carried.
    k: usize,
    /// Information bits fixed to zero and not transmitted.
    shortened: usize,
    /// Transmitted bits — equals the layer length.
    n_tx: usize,
    /// Flattened check-node adjacency.
    check_vars: Vec<u32>,
    check_offsets: Vec<u32>,
    /// Edge indices incident on each variable node.
    var_edges: Vec<u32>,
    var_offsets: Vec<u32>,
}

impl Code {
    /// Build the code for `n_tx` transmitted bits at rate index `rate`.
    ///
    /// Returns [`LcError::Capacity`] when no base graph can be lifted to fit — which in
    /// practice means a layer of a handful of bits at a high-redundancy rate.
    pub fn new(rate: usize, n_tx: usize) -> Result<Code, LcError> {
        let rate = rate.min(RATES.len() - 1);
        for family in [
            tables::BY_RATE_LARGE[rate],
            tables::BY_RATE_SMALL[rate],
            tables::BY_RATE_TINY[rate],
        ] {
            let z = n_tx.div_ceil(family.n_b).max(1);
            // The parity part alone must leave room for at least one information bit.
            if z * family.m_b >= n_tx {
                continue;
            }
            let shortened = z * family.n_b - n_tx;
            if shortened >= z * family.k_b {
                continue;
            }
            let k = z * family.k_b - shortened;
            let mut code = Code {
                base: family,
                z,
                k,
                shortened,
                n_tx,
                check_vars: Vec::new(),
                check_offsets: Vec::new(),
                var_edges: Vec::new(),
                var_offsets: Vec::new(),
            };
            code.build_graph();
            return Ok(code);
        }
        Err(LcError::Capacity {
            needed: 1,
            available: n_tx,
        })
    }

    /// Information bits carried.
    #[inline]
    pub const fn info_bits(&self) -> usize {
        self.k
    }

    /// Transmitted bits.
    #[inline]
    pub const fn coded_bits(&self) -> usize {
        self.n_tx
    }

    /// Lift factor.
    #[inline]
    pub const fn lift(&self) -> usize {
        self.z
    }

    /// Information bits fixed to zero by shortening.
    #[inline]
    pub const fn shortened_bits(&self) -> usize {
        self.shortened
    }

    /// Realised rate, which shortening can push below the nominal table value.
    #[inline]
    pub fn effective_rate(&self) -> f32 {
        self.k as f32 / self.n_tx as f32
    }

    /// Every base-graph edge as `(base row, base column, shift)`.
    fn edges(&self) -> Vec<(usize, usize, u16)> {
        let mut e: Vec<(usize, usize, u16)> = Vec::new();
        for (j, col) in self.base.info.iter().enumerate() {
            for &(row, shift) in col {
                e.push((row as usize, j, shift));
            }
        }
        // Parity structure — see `xtask::parity_edges`, which must stay in step.
        let m = self.base.m_b;
        let kb = self.base.k_b;
        e.push((0, kb, 0));
        e.push((m - 1, kb, 0));
        e.push((m / 2, kb, 1));
        for j in 1..m {
            e.push((j, kb + j, 0));
            e.push((j - 1, kb + j, 0));
        }
        e
    }

    fn build_graph(&mut self) {
        let z = self.z;
        let m_lift = self.base.m_b * z;
        let n_lift = self.base.n_b * z;
        let base_edges = self.edges();

        let mut per_check: Vec<Vec<u32>> = vec![Vec::new(); m_lift];
        for &(r, c, sh) in &base_edges {
            for t in 0..z {
                let check = r * z + t;
                let var = c * z + (t + sh as usize) % z;
                per_check[check].push(var as u32);
            }
        }

        self.check_offsets = Vec::with_capacity(m_lift + 1);
        self.check_offsets.push(0);
        for vars in &per_check {
            self.check_vars.extend_from_slice(vars);
            self.check_offsets.push(self.check_vars.len() as u32);
        }

        let mut per_var: Vec<Vec<u32>> = vec![Vec::new(); n_lift];
        for (e, &v) in self.check_vars.iter().enumerate() {
            per_var[v as usize].push(e as u32);
        }
        self.var_offsets = Vec::with_capacity(n_lift + 1);
        self.var_offsets.push(0);
        for edges in &per_var {
            self.var_edges.extend_from_slice(edges);
            self.var_offsets.push(self.var_edges.len() as u32);
        }
    }

    /// Systematically encode `info` (`k` bits) into `n_tx` transmitted bits.
    ///
    /// Transmission order is information bits followed by parity bits; the §8.3
    /// interleaver scatters them afterwards.
    pub fn encode(&self, info: &[bool]) -> Result<Vec<bool>, LcError> {
        if info.len() != self.k {
            return Err(LcError::Capacity {
                needed: info.len(),
                available: self.k,
            });
        }
        let z = self.z;
        let (kb, mb) = (self.base.k_b, self.base.m_b);

        // Information vector padded with the shortened zeros.
        let mut full = vec![false; kb * z];
        full[..self.k].copy_from_slice(info);

        // Per-row information syndrome.
        let mut syn = vec![vec![false; z]; mb];
        for (j, col) in self.base.info.iter().enumerate() {
            for &(row, shift) in col {
                let chunk = &full[j * z..(j + 1) * z];
                let dst = &mut syn[row as usize];
                for (t, d) in dst.iter_mut().enumerate() {
                    *d ^= chunk[(t + shift as usize) % z];
                }
            }
        }

        // Summing every row cancels the staircase and leaves P¹·p₀.
        let mut total = vec![false; z];
        for row in &syn {
            for (t, d) in total.iter_mut().enumerate() {
                *d ^= row[t];
            }
        }
        let mut p = vec![vec![false; z]; mb];
        // p₀ = P⁻¹ · total.
        for t in 0..z {
            p[0][t] = total[(t + z - 1) % z];
        }
        // Row 0: p₀ + p₁ = syn₀.
        for t in 0..z {
            p[1][t] = syn[0][t] ^ p[0][t];
        }
        // Rows 1..m−2, forward substitution.
        for i in 1..mb - 1 {
            for t in 0..z {
                let extra = if i == mb / 2 {
                    p[0][(t + 1) % z]
                } else {
                    false
                };
                p[i + 1][t] = syn[i][t] ^ p[i][t] ^ extra;
            }
        }
        debug_assert!(
            (0..z).all(|t| p[0][t] ^ p[mb - 1][t] == syn[mb - 1][t]),
            "final parity row inconsistent: encoder and parity structure disagree"
        );

        let mut out = Vec::with_capacity(self.n_tx);
        out.extend_from_slice(info);
        for row in &p {
            out.extend_from_slice(row);
        }
        debug_assert_eq!(out.len(), self.n_tx);
        Ok(out)
    }

    /// Expand received LLRs over the transmitted bits into the full lifted codeword,
    /// entering shortened information bits at certainty.
    fn channel_llrs(&self, rx: &[f32]) -> Vec<f32> {
        let z = self.z;
        let kb = self.base.k_b;
        let mut ch = vec![0f32; self.base.n_b * z];
        for (i, slot) in ch.iter_mut().take(kb * z).enumerate() {
            *slot = if i < self.k { rx[i] } else { CERTAIN };
        }
        ch[kb * z..].copy_from_slice(&rx[self.k..]);
        ch
    }

    /// Soft-decision decode. `rx[i]` is the LLR of transmitted bit `i`, positive meaning
    /// bit 0 is more likely.
    ///
    /// Returns the `k` information bits, the iterations spent, and whether every parity
    /// check was satisfied. **A non-converged result is still returned** — §12.5 wants
    /// the partial data as a retry and diagnostic aid, and the frame CRC (§9.1), not this
    /// flag, is the authoritative accept.
    pub fn decode(&self, rx: &[f32], max_iterations: u16) -> Result<DecodeResult, LcError> {
        if rx.len() != self.n_tx {
            return Err(LcError::Capacity {
                needed: rx.len(),
                available: self.n_tx,
            });
        }
        let ch = self.channel_llrs(rx);
        let n_edges = self.check_vars.len();
        let mut v2c = vec![0f32; n_edges];
        let mut c2v = vec![0f32; n_edges];
        let mut total = ch.clone();

        // Initialise variable-to-check with the channel value.
        for (v, &llr) in ch.iter().enumerate() {
            for &e in
                &self.var_edges[self.var_offsets[v] as usize..self.var_offsets[v + 1] as usize]
            {
                v2c[e as usize] = llr;
            }
        }

        let mut iterations = 0u16;
        let mut converged = false;
        while iterations < max_iterations {
            iterations += 1;

            // Check nodes: normalised min-sum.
            for c in 0..self.check_offsets.len() - 1 {
                let (lo, hi) = (
                    self.check_offsets[c] as usize,
                    self.check_offsets[c + 1] as usize,
                );
                let mut min1 = f32::INFINITY;
                let mut min2 = f32::INFINITY;
                let mut argmin = lo;
                let mut sign = 1f32;
                for (i, &m) in v2c[lo..hi].iter().enumerate() {
                    if m < 0.0 {
                        sign = -sign;
                    }
                    let a = m.abs();
                    if a < min1 {
                        min2 = min1;
                        min1 = a;
                        argmin = lo + i;
                    } else if a < min2 {
                        min2 = a;
                    }
                }
                for e in lo..hi {
                    let excl_min = if e == argmin { min2 } else { min1 };
                    let s = if v2c[e] < 0.0 { -sign } else { sign };
                    c2v[e] = ALPHA * s * excl_min;
                }
            }

            // Variable nodes.
            for v in 0..ch.len() {
                let (lo, hi) = (
                    self.var_offsets[v] as usize,
                    self.var_offsets[v + 1] as usize,
                );
                let mut sum = ch[v];
                for &e in &self.var_edges[lo..hi] {
                    sum += c2v[e as usize];
                }
                total[v] = sum;
                for &e in &self.var_edges[lo..hi] {
                    v2c[e as usize] = sum - c2v[e as usize];
                }
            }

            if self.syndrome_ok(&total) {
                converged = true;
                break;
            }
        }

        let bits: Vec<bool> = total[..self.k].iter().map(|&l| l < 0.0).collect();
        Ok(DecodeResult {
            bits,
            iterations,
            converged,
        })
    }

    fn syndrome_ok(&self, total: &[f32]) -> bool {
        for c in 0..self.check_offsets.len() - 1 {
            let (lo, hi) = (
                self.check_offsets[c] as usize,
                self.check_offsets[c + 1] as usize,
            );
            let mut parity = false;
            for &v in &self.check_vars[lo..hi] {
                parity ^= total[v as usize] < 0.0;
            }
            if parity {
                return false;
            }
        }
        true
    }
}

/// What [`Code::decode`] produces.
#[derive(Clone, Debug)]
pub struct DecodeResult {
    /// Recovered information bits.
    pub bits: Vec<bool>,
    /// Iterations spent.
    pub iterations: u16,
    /// Whether every parity check was satisfied. Not a decode verdict on its own —
    /// see §9.1.
    pub converged: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic xorshift, so failures are reproducible.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn bit(&mut self) -> bool {
            self.next() & 1 == 1
        }
        fn unit(&mut self) -> f32 {
            (self.next() >> 11) as f32 / (1u64 << 53) as f32
        }
        /// Box–Muller.
        fn normal(&mut self) -> f32 {
            let u1 = self.unit().max(1e-9);
            let u2 = self.unit();
            (-2.0 * u1.ln()).sqrt() * (core::f32::consts::TAU * u2).cos()
        }
    }

    fn layer_lengths() -> [usize; 8] {
        // Real layer lengths from the layout, smallest to largest.
        [26, 50, 150, 162, 437, 2014, 4412, 19406]
    }

    #[test]
    fn every_real_layer_length_and_rate_builds() {
        for n in layer_lengths() {
            for (rate, &nominal) in RATES.iter().enumerate() {
                match Code::new(rate, n) {
                    Ok(c) => {
                        assert!(c.info_bits() > 0, "n={n} rate={rate}");
                        assert_eq!(c.coded_bits(), n);
                        assert!(c.effective_rate() <= nominal + 1e-3, "n={n} rate={rate}");
                    }
                    Err(e) => panic!("n={n} rate={rate}: {e}"),
                }
            }
        }
    }

    #[test]
    fn shortening_only_lowers_the_rate() {
        // A long layer should land essentially on the nominal rate.
        let c = Code::new(1, 19406).unwrap();
        assert!(
            (c.effective_rate() - 0.75).abs() < 0.01,
            "{}",
            c.effective_rate()
        );
        // A tiny layer pays for the parity part it cannot shrink.
        let c = Code::new(3, 26).unwrap();
        assert!(c.effective_rate() < 0.55);
        assert!(c.effective_rate() > 0.1, "{}", c.effective_rate());
    }

    #[test]
    fn encode_produces_a_valid_codeword() {
        let mut rng = Rng(0xC0FFEE);
        for n in layer_lengths() {
            for rate in [0usize, 3, 5] {
                let c = Code::new(rate, n).unwrap();
                let info: Vec<bool> = (0..c.info_bits()).map(|_| rng.bit()).collect();
                let code = c.encode(&info).unwrap();
                assert_eq!(code.len(), n);
                // Every parity check must be satisfied by the encoded word.
                let hard: Vec<f32> = c
                    .channel_llrs(
                        &code
                            .iter()
                            .map(|&b| if b { -1.0 } else { 1.0 })
                            .collect::<Vec<_>>(),
                    )
                    .to_vec();
                assert!(
                    c.syndrome_ok(&hard),
                    "n={n} rate={rate}: encoder emitted a non-codeword"
                );
            }
        }
    }

    #[test]
    fn noiseless_decode_is_exact() {
        let mut rng = Rng(0x1234_5678);
        for n in layer_lengths() {
            for rate in 0..RATES.len() {
                let c = Code::new(rate, n).unwrap();
                let info: Vec<bool> = (0..c.info_bits()).map(|_| rng.bit()).collect();
                let code = c.encode(&info).unwrap();
                let llrs: Vec<f32> = code.iter().map(|&b| if b { -8.0 } else { 8.0 }).collect();
                let r = c.decode(&llrs, MAX_ITERATIONS).unwrap();
                assert!(r.converged, "n={n} rate={rate}");
                assert_eq!(r.bits, info, "n={n} rate={rate}");
                assert!(
                    r.iterations <= 2,
                    "clean decode took {} iterations",
                    r.iterations
                );
            }
        }
    }

    /// Simulate BPSK over AWGN and report the frame error rate, per §14.5 build step 5:
    /// the code is validated standalone before it is wired into the symbol path.
    fn frame_error_rate(rate: usize, n: usize, ebn0_db: f32, trials: usize, seed: u64) -> f32 {
        let c = Code::new(rate, n).unwrap();
        let r = c.effective_rate();
        // Es/N0 = Eb/N0 · rate; with unit-energy BPSK, sigma² = 1/(2·R·Eb/N0).
        let ebn0 = 10f32.powf(ebn0_db / 10.0);
        let sigma = (1.0 / (2.0 * r * ebn0)).sqrt();
        let mut rng = Rng(seed);
        let mut errors = 0;
        for _ in 0..trials {
            let info: Vec<bool> = (0..c.info_bits()).map(|_| rng.bit()).collect();
            let code = c.encode(&info).unwrap();
            let llrs: Vec<f32> = code
                .iter()
                .map(|&b| {
                    let tx = if b { -1.0 } else { 1.0 };
                    let y = tx + sigma * rng.normal();
                    2.0 * y / (sigma * sigma)
                })
                .collect();
            let out = c.decode(&llrs, MAX_ITERATIONS).unwrap();
            if out.bits != info {
                errors += 1;
            }
        }
        errors as f32 / trials as f32
    }

    #[test]
    fn awgn_waterfall_is_monotone_and_lands_in_range() {
        // Rate 0.75 on a mid-sized layer: no errors well above threshold, and a clear
        // waterfall as SNR drops. Absolute thresholds are recorded in docs/ldpc.md.
        let n = 2014;
        let hi = frame_error_rate(1, n, 6.0, 40, 0xAAAA);
        let mid = frame_error_rate(1, n, 3.5, 40, 0xBBBB);
        let lo = frame_error_rate(1, n, 1.5, 40, 0xCCCC);
        assert_eq!(hi, 0.0, "clean channel should be error free, got {hi}");
        assert!(
            mid <= lo,
            "waterfall not monotone: {mid} at 3.5 dB vs {lo} at 1.5 dB"
        );
        assert!(lo > 0.5, "code should fail well below threshold, got {lo}");
    }

    #[test]
    fn lower_rates_tolerate_more_noise() {
        let n = 2014;
        let strong = frame_error_rate(5, n, 2.0, 30, 0xD00D); // rate 0.35
        let weak = frame_error_rate(0, n, 2.0, 30, 0xD00D); // rate 0.85
        assert!(
            strong < weak,
            "rate 0.35 FER {strong} not better than rate 0.85 FER {weak}"
        );
    }

    #[test]
    fn soft_information_beats_hard_decisions() {
        // §12.4 claims ~2 dB for soft input. Compare the same channel realisation fed as
        // LLRs versus clipped to ±1 hard decisions.
        let n = 2014;
        let c = Code::new(1, n).unwrap();
        let ebn0 = 10f32.powf(3.0 / 10.0);
        let sigma = (1.0 / (2.0 * c.effective_rate() * ebn0)).sqrt();
        let mut rng = Rng(0xFEED);
        let (mut soft_err, mut hard_err) = (0, 0);
        for _ in 0..25 {
            let info: Vec<bool> = (0..c.info_bits()).map(|_| rng.bit()).collect();
            let code = c.encode(&info).unwrap();
            let y: Vec<f32> = code
                .iter()
                .map(|&b| (if b { -1.0 } else { 1.0 }) + sigma * rng.normal())
                .collect();
            let soft: Vec<f32> = y.iter().map(|v| 2.0 * v / (sigma * sigma)).collect();
            let hard: Vec<f32> = y
                .iter()
                .map(|v| if *v > 0.0 { 1.0 } else { -1.0 })
                .collect();
            if c.decode(&soft, MAX_ITERATIONS).unwrap().bits != info {
                soft_err += 1;
            }
            if c.decode(&hard, MAX_ITERATIONS).unwrap().bits != info {
                hard_err += 1;
            }
        }
        assert!(
            soft_err < hard_err,
            "soft {soft_err} not better than hard {hard_err}"
        );
    }

    #[test]
    fn a_burst_of_erasures_is_survivable() {
        // What interleaving turns a physical burst into: a scattering of zero-LLR bits.
        let c = Code::new(3, 4412).unwrap();
        let mut rng = Rng(0x5A5A);
        let info: Vec<bool> = (0..c.info_bits()).map(|_| rng.bit()).collect();
        let code = c.encode(&info).unwrap();
        let mut llrs: Vec<f32> = code.iter().map(|&b| if b { -6.0 } else { 6.0 }).collect();
        for (i, l) in llrs.iter_mut().enumerate() {
            if i % 4 == 0 {
                *l = 0.0; // 25% erased
            }
        }
        let r = c.decode(&llrs, MAX_ITERATIONS).unwrap();
        assert!(r.converged);
        assert_eq!(r.bits, info);
    }

    #[test]
    fn wrong_length_inputs_are_rejected() {
        let c = Code::new(1, 500).unwrap();
        assert!(c.encode(&vec![false; c.info_bits() + 1]).is_err());
        assert!(c.decode(&vec![0.0; 499], 10).is_err());
    }

    /// §12.5 wants the partial result even when the layer does not decode, as a retry
    /// and user-guidance signal. It must never be mistaken for success — that is the
    /// CRC's job — but it must come back.
    #[test]
    fn non_convergence_still_returns_bits() {
        let c = Code::new(0, 1000).unwrap();
        let mut rng = Rng(0x600D);
        // Pure noise: no codeword is consistent with it.
        let llrs: Vec<f32> = (0..1000).map(|_| rng.normal() * 2.0).collect();
        let r = c.decode(&llrs, 5).unwrap();
        assert!(!r.converged);
        assert_eq!(r.bits.len(), c.info_bits());
        assert_eq!(r.iterations, 5);
    }

    /// An all-zero LLR vector is not "undecodable": the all-zero codeword satisfies
    /// every check, so the decoder is right to converge on it instantly. Pinned because
    /// it looks like a bug the first time you see it.
    #[test]
    fn zero_llrs_converge_to_the_all_zero_codeword() {
        let c = Code::new(0, 1000).unwrap();
        let r = c.decode(&vec![0.0; 1000], 5).unwrap();
        assert!(r.converged);
        assert!(r.bits.iter().all(|b| !b));
    }
}
