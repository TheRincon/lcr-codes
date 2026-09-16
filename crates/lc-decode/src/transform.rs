//! Mapping module coordinates to image pixels (§12.1).
//!
//! §6.2 adds the alignment marker specifically so a decoder can fit a **homography**
//! rather than an affine transform. The difference is not cosmetic: an affine fit cannot
//! represent perspective, and §13.2 actively recommends capturing at a 10–20° off-axis
//! angle to move the camera's own reflection off a glossy screen. A decoder that can only
//! fit an affine transform is a decoder that fails at exactly the angle the capture
//! guidance tells users to hold.
//!
//! Module `(x, y)` is addressed at its **centre**, `(x + 0.5, y + 0.5)`.

use crate::math::solve;
use alloc::vec;

/// A projective transform from module space to pixel space.
#[derive(Copy, Clone, Debug)]
pub struct Transform {
    m: [f64; 8],
}

impl Transform {
    /// Identity.
    pub const IDENTITY: Transform = Transform {
        m: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
    };

    /// Fit from four module→pixel correspondences.
    ///
    /// Returns `None` when the correspondences are degenerate — three collinear points,
    /// or two that coincide.
    pub fn from_quad(src: [(f32, f32); 4], dst: [(f32, f32); 4]) -> Option<Transform> {
        let mut a = vec![0f64; 64];
        let mut b = vec![0f64; 8];
        for i in 0..4 {
            let (u, v) = (src[i].0 as f64, src[i].1 as f64);
            let (x, y) = (dst[i].0 as f64, dst[i].1 as f64);
            let r0 = i * 2;
            let r1 = r0 + 1;
            a[r0 * 8..r0 * 8 + 8].copy_from_slice(&[u, v, 1.0, 0.0, 0.0, 0.0, -x * u, -x * v]);
            a[r1 * 8..r1 * 8 + 8].copy_from_slice(&[0.0, 0.0, 0.0, u, v, 1.0, -y * u, -y * v]);
            b[r0] = x;
            b[r1] = y;
        }
        let h = solve(&mut a, &mut b, 8)?;
        Some(Transform {
            m: [h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]],
        })
    }

    /// Fit an affine transform from three correspondences.
    ///
    /// The fallback when the alignment marker cannot be found — a v1 symbol has none at
    /// all (§6.2), and on larger versions a specular blob can take it out.
    pub fn from_triangle(src: [(f32, f32); 3], dst: [(f32, f32); 3]) -> Option<Transform> {
        let mut a = vec![0f64; 36];
        let mut b = vec![0f64; 6];
        for i in 0..3 {
            let (u, v) = (src[i].0 as f64, src[i].1 as f64);
            let r0 = i * 2;
            let r1 = r0 + 1;
            a[r0 * 6..r0 * 6 + 6].copy_from_slice(&[u, v, 1.0, 0.0, 0.0, 0.0]);
            a[r1 * 6..r1 * 6 + 6].copy_from_slice(&[0.0, 0.0, 0.0, u, v, 1.0]);
            b[r0] = dst[i].0 as f64;
            b[r1] = dst[i].1 as f64;
        }
        let h = solve(&mut a, &mut b, 6)?;
        Some(Transform {
            m: [h[0], h[1], h[2], h[3], h[4], h[5], 0.0, 0.0],
        })
    }

    /// Map a module-space point to pixel space.
    #[inline]
    pub fn apply(&self, u: f32, v: f32) -> (f32, f32) {
        let (u, v) = (u as f64, v as f64);
        let w = self.m[6] * u + self.m[7] * v + 1.0;
        let w = if w.abs() < 1e-9 { 1e-9 } else { w };
        (
            ((self.m[0] * u + self.m[1] * v + self.m[2]) / w) as f32,
            ((self.m[3] * u + self.m[4] * v + self.m[5]) / w) as f32,
        )
    }

    /// Pixel position of the centre of module `(x, y)`.
    #[inline]
    pub fn module_center(&self, x: u16, y: u16) -> (f32, f32) {
        self.apply(x as f32 + 0.5, y as f32 + 0.5)
    }

    /// Approximate module pitch in pixels near module `(x, y)`.
    ///
    /// Measured locally rather than globally because under perspective the near edge of a
    /// symbol can be half again the pitch of the far edge, and the sampling kernel of
    /// §12.6 is defined as a *fraction of the module footprint*.
    pub fn local_pitch(&self, x: u16, y: u16) -> f32 {
        let c = self.module_center(x, y);
        let r = self.apply(x as f32 + 1.5, y as f32 + 0.5);
        let d = self.apply(x as f32 + 0.5, y as f32 + 1.5);
        let dx = ((r.0 - c.0).powi(2) + (r.1 - c.1).powi(2)).sqrt();
        let dy = ((d.0 - c.0).powi(2) + (d.1 - c.1).powi(2)).sqrt();
        0.5 * (dx + dy)
    }

    /// Module pitch at the extremes of a `side`×`side` symbol, as `(min, max)`.
    ///
    /// Two facts about a capture fall out of this pair, and neither needs the camera
    /// intrinsics — which a browser does not expose and a phone does not report:
    ///
    /// * **Range** is the pitch itself. A symbol drawn at a fixed size on a screen
    ///   subtends fewer sensor pixels the further away it is, so the pitch *is* the
    ///   distance, measured in the unit §13.1 actually constrains.
    /// * **Obliquity** is `max / min`. A projective map compresses the far edge more than
    ///   the near one, so a fronto-parallel view gives a ratio of 1 and the ratio climbs
    ///   with viewing angle. Converting it to degrees would need the focal length; the
    ///   ratio needs nothing and is the honest measurement.
    ///
    /// Corners are sampled inset by two modules so [`Transform::local_pitch`]'s forward
    /// difference stays inside the grid rather than extrapolating past its edge.
    pub fn pitch_range(&self, side: u16) -> (f32, f32) {
        let far = side.saturating_sub(2);
        let mut lo = f32::INFINITY;
        let mut hi = 0.0f32;
        // Under a projective map the pitch is monotone along the gradient direction, so
        // the extremes over the grid are attained at its corners.
        for (x, y) in [(0, 0), (far, 0), (0, far), (far, far)] {
            let p = self.local_pitch(x, y);
            lo = lo.min(p);
            hi = hi.max(p);
        }
        (lo, hi)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: (f32, f32), b: (f32, f32), tol: f32) -> bool {
        (a.0 - b.0).abs() < tol && (a.1 - b.1).abs() < tol
    }

    #[test]
    fn recovers_a_pure_scale_and_offset() {
        let src = [(0.0, 0.0), (10.0, 0.0), (0.0, 10.0), (10.0, 10.0)];
        let dst = [(100.0, 50.0), (200.0, 50.0), (100.0, 150.0), (200.0, 150.0)];
        let t = Transform::from_quad(src, dst).unwrap();
        for (s, d) in src.iter().zip(dst.iter()) {
            assert!(close(t.apply(s.0, s.1), *d, 1e-3));
        }
        assert!(close(t.apply(5.0, 5.0), (150.0, 100.0), 1e-3));
    }

    /// The property the alignment marker exists to buy: a genuinely projective
    /// correspondence that an affine fit cannot represent.
    #[test]
    fn represents_perspective_that_affine_cannot() {
        let src = [(0.0, 0.0), (10.0, 0.0), (0.0, 10.0), (10.0, 10.0)];
        // Far edge foreshortened: the bottom of the symbol is narrower than the top.
        let dst = [(0.0, 0.0), (100.0, 0.0), (20.0, 100.0), (80.0, 100.0)];
        let t = Transform::from_quad(src, dst).unwrap();
        for (s, d) in src.iter().zip(dst.iter()) {
            assert!(
                close(t.apply(s.0, s.1), *d, 1e-3),
                "{s:?} -> {:?} want {d:?}",
                t.apply(s.0, s.1)
            );
        }
        let affine =
            Transform::from_triangle([src[0], src[1], src[2]], [dst[0], dst[1], dst[2]]).unwrap();
        // The affine fit must miss the fourth point badly.
        assert!(!close(affine.apply(src[3].0, src[3].1), dst[3], 5.0));
    }

    #[test]
    fn affine_fit_is_exact_on_its_three_points() {
        let src = [(0.0, 0.0), (20.0, 0.0), (0.0, 20.0)];
        let dst = [(5.0, 7.0), (45.0, 11.0), (1.0, 47.0)];
        let t = Transform::from_triangle(src, dst).unwrap();
        for (s, d) in src.iter().zip(dst.iter()) {
            assert!(close(t.apply(s.0, s.1), *d, 1e-3));
        }
    }

    #[test]
    fn pitch_range_is_flat_when_the_view_is_fronto_parallel() {
        let src = [(0.0, 0.0), (21.0, 0.0), (0.0, 21.0), (21.0, 21.0)];
        let dst = [(0.0, 0.0), (210.0, 0.0), (0.0, 210.0), (210.0, 210.0)];
        let t = Transform::from_quad(src, dst).unwrap();
        let (lo, hi) = t.pitch_range(21);
        assert!((lo - 10.0).abs() < 1e-3, "min pitch {lo}");
        assert!((hi / lo - 1.0).abs() < 1e-3, "obliquity {}", hi / lo);
    }

    /// The measurement the capture sweep rests on: obliquity has to *increase* with the
    /// viewing angle, or the logged ratio says nothing about how the phone was held.
    #[test]
    fn pitch_range_obliquity_grows_with_foreshortening() {
        let src = [(0.0, 0.0), (20.0, 0.0), (0.0, 20.0), (20.0, 20.0)];
        let ratio = |far_width: f32| {
            let inset = (200.0 - far_width) / 2.0;
            let dst = [
                (0.0, 0.0),
                (200.0, 0.0),
                (inset, 200.0),
                (200.0 - inset, 200.0),
            ];
            let t = Transform::from_quad(src, dst).unwrap();
            let (lo, hi) = t.pitch_range(20);
            hi / lo
        };
        let (mild, steep) = (ratio(160.0), ratio(90.0));
        assert!(mild > 1.0, "a foreshortened view is not flat: {mild}");
        assert!(steep > mild, "steeper must read higher: {steep} vs {mild}");
    }

    #[test]
    fn degenerate_correspondences_are_rejected() {
        let collinear = [(0.0, 0.0), (1.0, 1.0), (2.0, 2.0), (3.0, 3.0)];
        let dst = [(0.0, 0.0), (1.0, 1.0), (2.0, 2.0), (9.0, 3.0)];
        assert!(Transform::from_quad(collinear, dst).is_none());
        assert!(Transform::from_triangle([(0.0, 0.0); 3], [(1.0, 2.0); 3]).is_none());
    }

    #[test]
    fn module_centres_land_at_half_module_offsets() {
        let src = [(0.0, 0.0), (21.0, 0.0), (0.0, 21.0), (21.0, 21.0)];
        let dst = [(0.0, 0.0), (210.0, 0.0), (0.0, 210.0), (210.0, 210.0)];
        let t = Transform::from_quad(src, dst).unwrap();
        assert!(close(t.module_center(0, 0), (5.0, 5.0), 1e-3));
        assert!(close(t.module_center(20, 20), (205.0, 205.0), 1e-3));
        assert!((t.local_pitch(10, 10) - 10.0).abs() < 1e-3);
    }

    #[test]
    fn local_pitch_tracks_perspective() {
        let src = [(0.0, 0.0), (20.0, 0.0), (0.0, 20.0), (20.0, 20.0)];
        let dst = [(0.0, 0.0), (200.0, 0.0), (40.0, 200.0), (160.0, 200.0)];
        let t = Transform::from_quad(src, dst).unwrap();
        assert!(
            t.local_pitch(10, 1) > t.local_pitch(10, 18),
            "near edge should be coarser"
        );
    }
}
