//! Small dense linear algebra: enough to fit a homography and a few least-squares
//! surfaces, and nothing more.
//!
//! Everything here is `O(n³)` on matrices of size 4 to 12, called a handful of times per
//! decode. A BLAS dependency would be larger than the whole wasm budget of §14.2 and
//! would buy nothing.

use alloc::vec;
use alloc::vec::Vec;

/// Solve `A · x = b` by Gaussian elimination with partial pivoting.
///
/// `a` is row-major `n × n`. Returns `None` when the system is singular to within
/// `1e-12`, which for a homography means three collinear finders and for a surface fit
/// means too few distinct reference positions — both of which the caller must handle
/// rather than paper over.
pub fn solve(a: &mut [f64], b: &mut [f64], n: usize) -> Option<Vec<f64>> {
    for col in 0..n {
        // Partial pivot.
        let mut pivot = col;
        for r in (col + 1)..n {
            if a[r * n + col].abs() > a[pivot * n + col].abs() {
                pivot = r;
            }
        }
        if a[pivot * n + col].abs() < 1e-12 {
            return None;
        }
        if pivot != col {
            for k in 0..n {
                a.swap(col * n + k, pivot * n + k);
            }
            b.swap(col, pivot);
        }
        let d = a[col * n + col];
        for r in (col + 1)..n {
            let f = a[r * n + col] / d;
            if f == 0.0 {
                continue;
            }
            for k in col..n {
                a[r * n + k] -= f * a[col * n + k];
            }
            b[r] -= f * b[col];
        }
    }
    // Back substitution.
    let mut x = vec![0f64; n];
    for r in (0..n).rev() {
        let mut s = b[r];
        for k in (r + 1)..n {
            s -= a[r * n + k] * x[k];
        }
        x[r] = s / a[r * n + r];
    }
    Some(x)
}

/// Least-squares fit of `basis(sample) · coeffs ≈ target`, by normal equations.
///
/// `rows` supplies `(basis_values, target)` pairs. Normal equations rather than QR
/// because the bases here are tiny (4 or 9 terms) and well-scaled — coordinates are
/// normalised to `[-1, 1]` before they get here — so the squared condition number is not
/// a problem in `f64`.
pub fn least_squares(rows: &[(Vec<f64>, f64)], terms: usize) -> Option<Vec<f64>> {
    if rows.len() < terms {
        return None;
    }
    let mut ata = vec![0f64; terms * terms];
    let mut atb = vec![0f64; terms];
    for (basis, target) in rows {
        for i in 0..terms {
            atb[i] += basis[i] * target;
            for j in 0..terms {
                ata[i * terms + j] += basis[i] * basis[j];
            }
        }
    }
    // Tikhonov nudge: keeps a degenerate reference layout from producing a wild surface
    // instead of no surface. Small enough not to bias a well-posed fit.
    for i in 0..terms {
        ata[i * terms + i] += 1e-9;
    }
    solve(&mut ata, &mut atb, terms)
}

/// Median of a slice, by partial sort. Consumes a scratch copy.
pub fn median(values: &mut [f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
    let n = values.len();
    if n % 2 == 1 {
        values[n / 2]
    } else {
        0.5 * (values[n / 2 - 1] + values[n / 2])
    }
}

/// Median absolute deviation, scaled to estimate a Gaussian standard deviation.
///
/// The 1.4826 factor makes MAD comparable to σ for normal data. §12.3 step 3 needs a
/// robust scale precisely because the contaminating samples — a reference under a
/// specular highlight — are the ones a standard deviation would be dominated by.
pub fn mad_sigma(values: &[f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let mut v = values.to_vec();
    let med = median(&mut v);
    let mut dev: Vec<f32> = values.iter().map(|x| (x - med).abs()).collect();
    1.4826 * median(&mut dev)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solves_a_known_system() {
        // 2x + y = 5 ; x - y = 1  =>  x = 2, y = 1
        let mut a = vec![2.0, 1.0, 1.0, -1.0];
        let mut b = vec![5.0, 1.0];
        let x = solve(&mut a, &mut b, 2).unwrap();
        assert!((x[0] - 2.0).abs() < 1e-12);
        assert!((x[1] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn needs_pivoting_and_still_works() {
        // Leading zero forces a row swap.
        let mut a = vec![0.0, 2.0, 1.0, 1.0];
        let mut b = vec![4.0, 3.0];
        let x = solve(&mut a, &mut b, 2).unwrap();
        assert!((x[0] - 1.0).abs() < 1e-12, "{x:?}");
        assert!((x[1] - 2.0).abs() < 1e-12, "{x:?}");
    }

    #[test]
    fn singular_systems_return_none() {
        let mut a = vec![1.0, 2.0, 2.0, 4.0];
        let mut b = vec![1.0, 2.0];
        assert!(solve(&mut a, &mut b, 2).is_none());
    }

    #[test]
    fn least_squares_recovers_an_exact_plane() {
        // target = 3 + 2x - y over a grid, basis [1, x, y].
        let mut rows = Vec::new();
        for x in -2..=2 {
            for y in -2..=2 {
                let (x, y) = (x as f64, y as f64);
                rows.push((vec![1.0, x, y], 3.0 + 2.0 * x - y));
            }
        }
        let c = least_squares(&rows, 3).unwrap();
        assert!((c[0] - 3.0).abs() < 1e-6, "{c:?}");
        assert!((c[1] - 2.0).abs() < 1e-6, "{c:?}");
        assert!((c[2] + 1.0).abs() < 1e-6, "{c:?}");
    }

    #[test]
    fn least_squares_needs_enough_rows() {
        assert!(least_squares(&[(vec![1.0, 0.0], 1.0)], 2).is_none());
    }

    #[test]
    fn median_handles_both_parities() {
        assert_eq!(median(&mut [3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(&mut [4.0, 1.0, 3.0, 2.0]), 2.5);
        assert_eq!(median(&mut []), 0.0);
    }

    /// The reason §12.3 step 3 uses MAD: one reference under a specular highlight must
    /// not be allowed to set the scale. A standard deviation would be dragged two orders
    /// of magnitude; MAD barely moves.
    #[test]
    fn mad_ignores_a_wild_outlier() {
        let clean = [1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0];
        let mut dirty = clean.to_vec();
        dirty[5] = 1000.0;

        let stddev = |v: &[f32]| {
            let m = v.iter().sum::<f32>() / v.len() as f32;
            (v.iter().map(|x| (x - m).powi(2)).sum::<f32>() / v.len() as f32).sqrt()
        };
        let (mad_c, mad_d) = (mad_sigma(&clean), mad_sigma(&dirty));
        let (sd_c, sd_d) = (stddev(&clean), stddev(&dirty));

        assert!(mad_d / mad_c < 1.5, "MAD moved {mad_c} -> {mad_d}");
        assert!(
            sd_d / sd_c > 100.0,
            "stddev should blow up: {sd_c} -> {sd_d}"
        );
    }
}
