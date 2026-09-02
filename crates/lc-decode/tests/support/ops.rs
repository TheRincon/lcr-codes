// Composable capture degradations (§15). Shared by the `degradation` test and the
// `envelope` example via `include!`, so the sweep that establishes the operating envelope
// and the regression suite that guards it can never disagree about what a given
// impairment means.

/// Deterministic xorshift, so a failure is reproducible from its seed alone.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn unit(&mut self) -> f32 {
        (self.next() >> 11) as f32 / (1u64 << 53) as f32
    }
    /// Uniform in `lo..hi`.
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + self.unit() * (hi - lo)
    }
    fn normal(&mut self) -> f32 {
        let u1 = self.unit().max(1e-9);
        (-2.0 * u1.ln()).sqrt() * (std::f32::consts::TAU * self.unit()).cos()
    }
}

fn srgb_to_linear(v: u8) -> f32 {
    lc_core::color::srgb_to_linear_f(v as f32 / 255.0)
}

fn linear_to_srgb(v: f32) -> u8 {
    (lc_core::color::linear_to_srgb_f(v.clamp(0.0, 1.0)) * 255.0).round() as u8
}

/// Apply a per-pixel function in **linear light**, which is where every optical
/// impairment is actually linear.
fn map_linear(r: &Raster, mut f: impl FnMut(f32, f32, [f32; 3]) -> [f32; 3]) -> Raster {
    let mut out = r.clone();
    for y in 0..r.height {
        for x in 0..r.width {
            let i = ((y * r.width + x) * 3) as usize;
            let lin = [
                srgb_to_linear(r.data[i]),
                srgb_to_linear(r.data[i + 1]),
                srgb_to_linear(r.data[i + 2]),
            ];
            let v = f(x as f32 / r.width as f32, y as f32 / r.height as f32, lin);
            for (c, &val) in v.iter().enumerate() {
                out.data[i + c] = linear_to_srgb(val);
            }
        }
    }
    out
}

fn bilinear(r: &Raster, x: f32, y: f32) -> [f32; 3] {
    let x = x.clamp(0.0, r.width as f32 - 1.0);
    let y = y.clamp(0.0, r.height as f32 - 1.0);
    let (x0, y0) = (x.floor() as u32, y.floor() as u32);
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let (x1, y1) = ((x0 + 1).min(r.width - 1), (y0 + 1).min(r.height - 1));
    let at = |px: u32, py: u32, c: usize| {
        srgb_to_linear(r.data[(((py * r.width + px) * 3) as usize) + c])
    };
    std::array::from_fn(|c| {
        let top = at(x0, y0, c) * (1.0 - fx) + at(x1, y0, c) * fx;
        let bot = at(x0, y1, c) * (1.0 - fx) + at(x1, y1, c) * fx;
        top * (1.0 - fy) + bot * fy
    })
}

/// Warp as if photographed off-axis. `tilt` foreshortens the far edge; §13.2 recommends
/// 10–20° off-axis to move the camera's own reflection off a glossy screen.
///
/// Two details that took a round of wrong answers to get right:
///
/// * The map is a **genuine projective one** — both axes divided by the same `1 + k·v` —
///   not a shear. An affine-only decoder must fail here, and a decoder that passes a
///   non-projective "perspective" has proved nothing.
/// * The output canvas **grows** to fit the warped symbol rather than the content being
///   scaled down to fit a fixed canvas. Scaling down changes the device pixels per module,
///   which is a different impairment (§13.1) that would be measured here by accident: an
///   early version of this function reported that the decoder failed above 15° of
///   rotation, when what it had actually measured was 8.5 px/module.
fn perspective(r: &Raster, tilt: f32, rotate_deg: f32) -> Raster {
    let (sin, cos) = rotate_deg.to_radians().sin_cos();
    let (w, h) = (r.width as f32, r.height as f32);
    // The rotated, projected symbol plus its quiet zone must fit. The `+ 0.3` is not
    // cosmetic: without it `grow` rises with tilt, so a *mild* tilt gets a tighter canvas
    // than a strong one and clips the quiet zone off the corners — which made the tilt
    // sweep non-monotonic (0.1 failing where 0.3 passed) and was measuring the harness
    // rather than the decoder.
    let grow = (cos.abs() + sin.abs()) / (1.0 - 0.6 * tilt) + 0.3;
    let size = ((w * grow).ceil() as u32).max(r.width);
    let mut out = Raster { width: size, height: size, data: vec![255; (size * size * 3) as usize] };
    let centre = size as f32 / 2.0;

    for py in 0..size {
        for px in 0..size {
            // Normalise about the output centre in *source* units, so module pitch is
            // preserved; then undo the rotation, then undo the projection.
            let (u, v) = ((px as f32 - centre) / w, (py as f32 - centre) / h);
            let (u, v) = (u * cos + v * sin, -u * sin + v * cos);
            let denom = 1.0 - tilt * v;
            if denom.abs() < 1e-3 {
                continue;
            }
            let (su, sv) = (u / denom, v / denom);
            let (sx, sy) = ((su + 0.5) * w, (sv + 0.5) * h);
            let lin = if (0.0..w).contains(&sx) && (0.0..h).contains(&sy) {
                bilinear(r, sx, sy)
            } else {
                [1.0, 1.0, 1.0]
            };
            let i = ((py * size + px) * 3) as usize;
            for (c, &val) in lin.iter().enumerate() {
                out.data[i + c] = linear_to_srgb(val);
            }
        }
    }
    out
}

/// Multiplicative brightness falloff across the frame — vignetting, or the shoulder of a
/// glare gradient. `strength` is the fraction of light lost at the far corner.
fn shading(r: &Raster, strength: f32) -> Raster {
    // atan2(0.4, 0.6) — the direction the original fixed 0.6u + 0.4v gradient ran in.
    shading_dir(r, strength, 33.690_07)
}

/// [`shading`] with the gradient running in an arbitrary direction.
///
/// Which way a shadow falls is a property of the scene, not of how bad the shadow is, so
/// a success *rate* at a given strength has to average over it (§15).
fn shading_dir(r: &Raster, strength: f32, dir_deg: f32) -> Raster {
    let (sin, cos) = dir_deg.to_radians().sin_cos();
    // Normalise so the projection spans exactly 1.0 across the frame at any angle, and
    // offset so the brightest corner is untouched — `strength` then means the same
    // fraction of light lost at the far corner regardless of direction.
    let norm = cos.abs() + sin.abs();
    let (a, b) = (cos / norm, sin / norm);
    let t0 = a.min(0.0) + b.min(0.0);
    map_linear(r, |u, v, c| {
        let s = 1.0 - strength * (a * u + b * v - t0);
        std::array::from_fn(|i| c[i] * s)
    })
}

/// A display colour mode: per-channel gain. Night Shift is roughly `(1.0, 0.85, 0.55)`.
fn color_cast(r: &Raster, gains: [f32; 3]) -> Raster {
    map_linear(r, |_, _, c| std::array::from_fn(|i| c[i] * gains[i]))
}

/// A saturating specular highlight — the dominant screen impairment per §3.
fn specular(r: &Raster, cx: f32, cy: f32, radius: f32, strength: f32) -> Raster {
    map_linear(r, |u, v, c| {
        let d2 = ((u - cx).powi(2) + (v - cy).powi(2)) / (radius * radius);
        let glare = strength * (-d2 * 4.0).exp();
        std::array::from_fn(|i| (c[i] + glare).min(1.0))
    })
}

/// Additive per-channel sensor noise, σ in linear light.
fn noise(r: &Raster, sigma: f32, seed: u64) -> Raster {
    let mut rng = Rng(seed | 1);
    map_linear(r, |_, _, c| std::array::from_fn(|i| (c[i] + sigma * rng.normal()).clamp(0.0, 1.0)))
}

/// Box blur, approximating defocus. `radius` is in pixels.
fn blur(r: &Raster, radius: u32) -> Raster {
    let mut out = r.clone();
    let n = (2 * radius + 1) as f32;
    for y in 0..r.height {
        for x in 0..r.width {
            let mut acc = [0f32; 3];
            for dy in -(radius as i32)..=(radius as i32) {
                for dx in -(radius as i32)..=(radius as i32) {
                    let sx = (x as i32 + dx).clamp(0, r.width as i32 - 1) as u32;
                    let sy = (y as i32 + dy).clamp(0, r.height as i32 - 1) as u32;
                    let i = ((sy * r.width + sx) * 3) as usize;
                    for (c, a) in acc.iter_mut().enumerate() {
                        *a += srgb_to_linear(r.data[i + c]);
                    }
                }
            }
            let i = ((y * r.width + x) * 3) as usize;
            for (c, &a) in acc.iter().enumerate() {
                out.data[i + c] = linear_to_srgb(a / (n * n));
            }
        }
    }
    out
}

/// 4:2:0 chroma subsampling, as a JPEG or a camera pipeline applies it.
///
/// §1.4 identifies this as the reason chroma is the low-bandwidth channel: a Bayer sensor
/// interpolates most of it, and a 4:2:0 stage halves it again in both axes.
fn chroma_subsample(r: &Raster) -> Raster {
    let ycbcr = |p: [f32; 3]| {
        let y = 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2];
        [y, (p[2] - y) * 0.564, (p[0] - y) * 0.713]
    };
    let rgb = |c: [f32; 3]| [c[0] + 1.402 * c[2], c[0] - 0.344 * c[1] - 0.714 * c[2], c[0] + 1.772 * c[1]];

    // Work on gamma-encoded values, which is where the real pipeline does it.
    let px = |x: u32, y: u32| -> [f32; 3] {
        let i = ((y.min(r.height - 1) * r.width + x.min(r.width - 1)) * 3) as usize;
        std::array::from_fn(|c| r.data[i + c] as f32 / 255.0)
    };
    let mut out = r.clone();
    for by in (0..r.height).step_by(2) {
        for bx in (0..r.width).step_by(2) {
            let mut cb = 0.0;
            let mut cr = 0.0;
            for dy in 0..2 {
                for dx in 0..2 {
                    let c = ycbcr(px(bx + dx, by + dy));
                    cb += c[1] / 4.0;
                    cr += c[2] / 4.0;
                }
            }
            for dy in 0..2 {
                for dx in 0..2 {
                    let (x, y) = (bx + dx, by + dy);
                    if x >= r.width || y >= r.height {
                        continue;
                    }
                    let luma = ycbcr(px(x, y))[0];
                    let v = rgb([luma, cb, cr]);
                    let i = ((y * r.width + x) * 3) as usize;
                    for (c, &val) in v.iter().enumerate() {
                        out.data[i + c] = (val.clamp(0.0, 1.0) * 255.0).round() as u8;
                    }
                }
            }
        }
    }
    out
}

/// Paint an opaque rectangle over part of the symbol — a finger, a sticker, a crease.
fn occlude(r: &Raster, x0: f32, y0: f32, w: f32, h: f32) -> Raster {
    map_linear(r, |u, v, c| {
        if u >= x0 && u < x0 + w && v >= y0 && v < y0 + h { [0.35, 0.30, 0.28] } else { c }
    })
}


// ---------------------------------------------------------------------------------
// Seeded trials (§15)
// ---------------------------------------------------------------------------------

/// The impairments swept for an operating envelope, as one axis a harness can iterate.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Impair {
    Tilt,
    Rotation,
    Shading,
    SpecularStrength,
    SpecularRadius,
    Noise,
    Blur,
    Occlusion,
}

impl Impair {
    const ALL: [Impair; 8] = [
        Impair::Tilt,
        Impair::Rotation,
        Impair::Shading,
        Impair::SpecularStrength,
        Impair::SpecularRadius,
        Impair::Noise,
        Impair::Blur,
        Impair::Occlusion,
    ];

    fn name(self) -> &'static str {
        match self {
            Impair::Tilt => "perspective tilt",
            Impair::Rotation => "in-plane rotation",
            Impair::Shading => "shading",
            Impair::SpecularStrength => "specular strength",
            Impair::SpecularRadius => "specular radius",
            Impair::Noise => "noise sigma",
            Impair::Blur => "blur radius",
            Impair::Occlusion => "occlusion frac",
        }
    }

    /// Magnitudes to sweep. Chosen to straddle the failure knee rather than to look tidy.
    fn magnitudes(self) -> &'static [f32] {
        match self {
            Impair::Tilt => &[0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7],
            Impair::Rotation => &[5.0, 15.0, 30.0, 45.0, 60.0, 90.0],
            Impair::Shading => &[0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8],
            Impair::SpecularStrength => &[0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9],
            Impair::SpecularRadius => &[0.15, 0.20, 0.25, 0.30, 0.35, 0.40],
            Impair::Noise => &[0.02, 0.04, 0.06, 0.08, 0.10, 0.12, 0.14],
            Impair::Blur => &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
            Impair::Occlusion => &[0.10, 0.15, 0.20, 0.25, 0.30, 0.35],
        }
    }
}

/// Apply `imp` at `magnitude`, drawing from `seed` everything a real capture varies but
/// the magnitude does not name.
///
/// This is what turns a pass/fail grid into the success *rate* §15 asks for. Magnitude is
/// the quantity under study and is held exactly; where the glare sits, which way the
/// shading runs, which axis the symbol tilts about and where a finger lands are nuisance
/// parameters, and a single fixed choice of them measures one arbitrary scenario rather
/// than the impairment. Note `Rotation` and `Blur` draw nothing — neither has a nuisance
/// parameter — so for those the payload is the only axis of variation, which is why the
/// caller varies that too.
fn apply(imp: Impair, r: &Raster, magnitude: f32, seed: u64) -> Raster {
    let mut rng = Rng(seed | 1);
    match imp {
        // Tilt about a random axis: the magnitude is how far it leans, not which way.
        //
        // KNOWN ARTEFACT: the tilt row is not monotone — 0.1 scores below 0.3. A mild
        // tilt scales the image by a factor very near 1, and `bilinear` resampling at a
        // near-unity ratio beats against the 12 px module grid and aliases; a stronger
        // tilt moves the ratio away from 1 and the beat period shortens until it averages
        // out within a module. It is real moiré (§15 asks for a sweep of exactly this),
        // but here it is confounded with the quantity being swept, so read the tilt row
        // as indicative only. Fixing it means supersampling `perspective` rather than
        // point-sampling one bilinear tap per output pixel.
        Impair::Tilt => perspective(r, magnitude, rng.range(0.0, 360.0)),
        Impair::Rotation => perspective(r, 0.0, magnitude),
        Impair::Shading => shading_dir(r, magnitude, rng.range(0.0, 360.0)),
        Impair::SpecularStrength => {
            let (cx, cy) = (rng.range(0.2, 0.8), rng.range(0.2, 0.8));
            specular(r, cx, cy, 0.30, magnitude)
        }
        Impair::SpecularRadius => {
            let (cx, cy) = (rng.range(0.2, 0.8), rng.range(0.2, 0.8));
            specular(r, cx, cy, magnitude, 0.6)
        }
        Impair::Noise => noise(r, magnitude, rng.next()),
        Impair::Blur => blur(r, magnitude as u32),
        Impair::Occlusion => {
            let x = rng.range(0.0, 1.0 - magnitude);
            let y = rng.range(0.0, 1.0 - magnitude);
            occlude(r, x, y, magnitude, magnitude)
        }
    }
}

/// A pseudo-random payload of `len` bytes.
///
/// Varying the payload across trials is what gives `Rotation` and `Blur` independent
/// realisations at all, and for every impairment it averages over codewords — which is
/// the point when the quantity being measured is an ECC rate.
fn trial_payload(len: usize, seed: u64) -> Vec<u8> {
    let mut rng = Rng(seed | 1);
    (0..len).map(|_| (rng.next() >> 33) as u8).collect()
}
