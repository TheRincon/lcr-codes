//! Codeword interleaving (§8.3, spec open decision #7).
//!
//! ## Pinned: golden-ratio affine permutation
//!
//! §8.3 asks for a "version-dependent affine permutation over module index" and leaves
//! the constants open. Pinned here as
//!
//! ```text
//! module_position(j) = (a · j) mod n,   a = coprime(round(n / φ), n)
//! ```
//!
//! where `n` is the layer's transmitted length and `φ` is the golden ratio. `a` starts at
//! `round(n/φ)` and increments until it is coprime with `n`, which makes the map a
//! permutation and makes the choice deterministic from `n` alone — the "version
//! dependence" §8.3 wants falls out, since `n` differs for every version, profile and
//! ECC level.
//!
//! ## Why golden-ratio spacing rather than a block interleaver
//!
//! What matters is the *inverse* map: a physical burst covers consecutive module
//! positions, and those land on codeword indices in an arithmetic progression of stride
//! `a⁻¹ mod n`. A block interleaver spreads a burst evenly but periodically, so a burst
//! whose length is near the block width can still concentrate on a few rows. The
//! golden-ratio multiplier has the three-distance property: any `L` consecutive terms of
//! the progression are spread over the codeword with at most three distinct gap sizes,
//! and no burst length is pathological. Which is the right property here, because §15's
//! screen impairments — a specular blob, a finger, a band of PWM — produce bursts of
//! essentially arbitrary length.

use alloc::vec::Vec;

const INV_PHI: f64 = 0.618_033_988_749_894_8;

const fn gcd(mut a: usize, mut b: usize) -> usize {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

/// The multiplier `a` for a layer of `n` bits.
pub fn multiplier(n: usize) -> usize {
    if n <= 2 {
        return 1;
    }
    let mut a = ((n as f64 * INV_PHI).round() as usize).max(1);
    while gcd(a, n) != 1 {
        a += 1;
        if a >= n {
            a = 1;
        }
    }
    a
}

/// Forward permutation: codeword index → transmitted position.
#[derive(Clone, Debug)]
pub struct Interleaver {
    n: usize,
    a: usize,
}

impl Interleaver {
    /// Build the interleaver for a layer of `n` bits.
    pub fn new(n: usize) -> Interleaver {
        Interleaver {
            n,
            a: multiplier(n),
        }
    }

    /// Layer length.
    #[inline]
    pub const fn len(&self) -> usize {
        self.n
    }

    /// True when the layer carries no bits.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// Transmitted position of codeword bit `j`.
    #[inline]
    pub const fn position(&self, j: usize) -> usize {
        if self.n == 0 {
            0
        } else {
            (self.a * j) % self.n
        }
    }

    /// Scatter a codeword into transmission order.
    pub fn interleave<T: Copy + Default>(&self, codeword: &[T]) -> Vec<T> {
        debug_assert_eq!(codeword.len(), self.n);
        let mut out = alloc::vec![T::default(); self.n];
        for (j, &v) in codeword.iter().enumerate() {
            out[self.position(j)] = v;
        }
        out
    }

    /// Gather transmission order back into codeword order.
    pub fn deinterleave<T: Copy + Default>(&self, received: &[T]) -> Vec<T> {
        debug_assert_eq!(received.len(), self.n);
        let mut out = alloc::vec![T::default(); self.n];
        for j in 0..self.n {
            out[j] = received[self.position(j)];
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permutation_is_a_bijection() {
        for n in [1usize, 2, 3, 7, 64, 162, 324, 1000, 4028, 38812] {
            let il = Interleaver::new(n);
            let mut seen = alloc::vec![false; n];
            for j in 0..n {
                let p = il.position(j);
                assert!(!seen[p], "n={n} collision at {p}");
                seen[p] = true;
            }
        }
    }

    #[test]
    fn interleave_roundtrips() {
        let n = 324;
        let il = Interleaver::new(n);
        let data: Vec<u16> = (0..n as u16).collect();
        assert_eq!(il.deinterleave(&il.interleave(&data)), data);
    }

    /// The property the whole module exists for: a contiguous physical burst must not
    /// land on contiguous codeword bits.
    #[test]
    fn a_burst_spreads_across_the_codeword() {
        let n = 4028;
        let il = Interleaver::new(n);
        // Which codeword indices does a 200-bit physical burst damage?
        let burst: alloc::collections::BTreeSet<usize> = (0..n)
            .filter(|&j| (1000..1200).contains(&il.position(j)))
            .collect();
        assert_eq!(burst.len(), 200);
        let idx: Vec<usize> = burst.into_iter().collect();
        // No two damaged codeword bits adjacent, and they span most of the codeword.
        let min_gap = idx.windows(2).map(|w| w[1] - w[0]).min().unwrap();
        assert!(
            min_gap >= 3,
            "burst left adjacent codeword bits, min gap {min_gap}"
        );
        assert!(
            idx.last().unwrap() - idx[0] > n * 9 / 10,
            "burst did not spread"
        );
    }

    /// Three-distance property: consecutive damaged indices take few distinct gaps, and
    /// none of them is large enough to leave a codeword region untouched.
    #[test]
    fn gap_sizes_are_bounded() {
        let n = 2014;
        let il = Interleaver::new(n);
        let mut idx: Vec<usize> = (0..n).filter(|&j| il.position(j) < 100).collect();
        idx.sort_unstable();
        let gaps: alloc::collections::BTreeSet<usize> =
            idx.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(
            gaps.len() <= 3,
            "expected at most three distinct gaps, got {gaps:?}"
        );
    }

    #[test]
    fn degenerate_lengths_do_not_panic() {
        for n in [0usize, 1, 2] {
            let il = Interleaver::new(n);
            for j in 0..n {
                assert!(il.position(j) < n);
            }
        }
    }
}
