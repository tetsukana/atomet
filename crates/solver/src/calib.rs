//! Lens-distortion calibration and plate-solve validation.
//!
//! The pattern matcher assumes a pinhole camera and only looks at the central
//! part of the frame, so a solution found there says little about the rest of
//! the image. This module takes that attitude as a starting point and
//!
//!  1. projects the catalog onto the image,
//!  2. matches detected stars to it ring by ring (30° → 75°, `RING_STEP_DEG`
//!     at a time), re-fitting after each ring: radial lens profile
//!     `r = f·(θ + c3·θ³ + c5·θ⁵)`, optical-axis offset (folded back into the
//!     pointing) and roll,
//!  3. validates: the number of stars matched *outside* the central 30° must
//!     be well above what chance alignment with the catalog would give.
//!
//! Geometry matches `compute_roll`: a sky point at position angle PA (north
//! toward east) and angular distance θ from the pointing lands at
//! `(r·sin(PA+roll), r·cos(PA+roll))` relative to the image center.

use crate::{AttitudeSolution, Vec3};

type V3 = [f64; 3];

pub const FRAME_W: f64 = 640.0;
pub const FRAME_H: f64 = 360.0;
/// Radius of the central region the pattern matcher is trusted in. Matches
/// beyond it are the independent evidence used for validation.
pub const INNER_DEG: f64 = 30.0;
const START_DEG: f64 = 15.0;
const RING_STEP_DEG: f64 = 5.0;
const END_DEG: f64 = 75.0;
/// Region used to pick the starting focal-length scale.
const SCALE_SEARCH_DEG: f64 = 25.0;

/// Match tolerance while growing: `TOL_BASE + TOL_FRAC · r` pixels.
const TOL_BASE: f64 = 3.0;
const TOL_FRAC: f64 = 0.03;
/// Tolerance for the final count (after the model has converged).
const FINAL_TOL_BASE: f64 = 2.5;
const FINAL_TOL_FRAC: f64 = 0.01;

/// Minimum matches to adjust scale/center, and to fit the full profile.
const MIN_SCALE_FIT: usize = 6;
const MIN_FULL_FIT: usize = 12;
/// The full profile also needs the matches to reach this far out.
const FULL_FIT_MIN_THETA_DEG: f64 = 22.0;

// Validation
const MIN_MATCHED: usize = 12;
const MIN_OUTER: usize = 5;
const MAX_RMS_PX: f64 = 3.0;
/// Outer matches must exceed chance level μ by this many σ (√μ).
const SIGMA_MARGIN: f64 = 5.0;

// ───────────────────────── small vector helpers ─────────────────────────

fn dot(a: &V3, b: &V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: &V3, b: &V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn add_scaled(a: &V3, b: &V3, s: f64) -> V3 {
    [a[0] + b[0] * s, a[1] + b[1] * s, a[2] + b[2] * s]
}
fn normalize(a: V3) -> V3 {
    let n = dot(&a, &a).sqrt();
    [a[0] / n, a[1] / n, a[2] / n]
}
fn from_vec3(v: &Vec3) -> V3 {
    [v.0 as f64, v.1 as f64, v.2 as f64]
}
fn wrap(a: f64) -> f64 {
    (a + std::f64::consts::PI).rem_euclid(2.0 * std::f64::consts::PI) - std::f64::consts::PI
}

/// (north axis, east axis) tangent to the sphere at `p` — same construction
/// as `compute_roll`.
fn sky_frame(p: &V3) -> (V3, V3) {
    let north = [0.0, 0.0, 1.0];
    let raw = add_scaled(&north, p, -dot(&north, p));
    let n = if dot(&raw, &raw).sqrt() > 1e-9 {
        normalize(raw)
    } else {
        let x = [1.0, 0.0, 0.0];
        normalize(add_scaled(&x, p, -dot(&x, p)))
    };
    let e = normalize(cross(&n, p));
    (n, e)
}

/// Unit vector (ICRS) of the sky point seen at pixel offset `(dx, dy)` from the
/// image center (x right, y down; array index − W/2), for a camera with
/// attitude `pose` and `lens`. Inverse of the projection used by the solver;
/// `None` outside the lens model's range.
pub fn pixel_to_sky(pose: &Pose, lens: &LensModel, dx: f64, dy: f64) -> Option<[f64; 3]> {
    let th = lens.theta(dx.hypot(dy))?;
    let pa = dx.atan2(dy) - pose.roll;
    let (n, e) = sky_frame(&pose.pointing);
    let (s, c) = th.sin_cos();
    let (sp, cp) = pa.sin_cos();
    Some(normalize([
        c * pose.pointing[0] + s * (cp * n[0] + sp * e[0]),
        c * pose.pointing[1] + s * (cp * n[1] + sp * e[1]),
        c * pose.pointing[2] + s * (cp * n[2] + sp * e[2]),
    ]))
}

// ───────────────────────────── lens model ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    /// `r = f·tan θ`
    Pinhole,
    /// `r = f·(θ + c3·θ³ + c5·θ⁵)`
    Poly { c3: f64, c5: f64 },
}

/// Radial lens profile; `f` is the on-axis scale in pixels per radian.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LensModel {
    pub f: f64,
    pub shape: Shape,
}

impl LensModel {
    pub fn pinhole(f: f64) -> Self {
        Self { f, shape: Shape::Pinhole }
    }

    fn g(&self, th: f64) -> f64 {
        match self.shape {
            Shape::Pinhole => th.tan(),
            Shape::Poly { c3, c5 } => th * (1.0 + th * th * (c3 + th * th * c5)),
        }
    }

    /// Radius in pixels of a source θ radians off-axis.
    pub fn radius(&self, th: f64) -> f64 {
        self.f * self.g(th)
    }

    /// Largest θ the model is trusted for (radians).
    pub fn theta_limit(&self) -> f64 {
        match self.shape {
            Shape::Pinhole => 1.45,
            Shape::Poly { .. } => 1.2,
        }
    }

    /// Inverse of `radius`; `None` outside the model's range.
    pub fn theta(&self, r: f64) -> Option<f64> {
        if r <= 0.0 {
            return Some(0.0);
        }
        let (mut lo, mut hi) = (0.0, self.theta_limit());
        if self.radius(hi) < r {
            return None;
        }
        for _ in 0..40 {
            let mid = 0.5 * (lo + hi);
            if self.radius(mid) < r {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        Some(0.5 * (lo + hi))
    }

    /// Re-express a pixel offset from the image center as the offset an ideal
    /// pinhole camera of focal length `f_pin` would have seen. `None` if the
    /// point is outside the model's range.
    pub fn to_pinhole(&self, x: f64, y: f64, f_pin: f64) -> Option<(f64, f64)> {
        let r = x.hypot(y);
        if r < 1e-9 {
            return Some((x, y));
        }
        let th = self.theta(r)?;
        if th > 1.4 {
            return None;
        }
        let s = f_pin * th.tan() / r;
        Some((x * s, y * s))
    }

    /// A polynomial profile must stay increasing over its working range.
    fn is_sane(&self) -> bool {
        match self.shape {
            Shape::Pinhole => self.f > 0.0,
            Shape::Poly { c3, c5 } => {
                self.f > 0.0
                    && (0..=24).all(|i| {
                        let t = i as f64 * 0.05;
                        1.0 + 3.0 * c3 * t * t + 5.0 * c5 * t.powi(4) > 0.1
                    })
            }
        }
    }
}

// ─────────────────────────────── pose ───────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct Pose {
    /// Unit vector of the image center on the celestial sphere.
    pub pointing: [f64; 3],
    /// Radians, same convention as `AttitudeSolution::roll`.
    pub roll: f64,
}

impl Pose {
    pub fn from_attitude(sol: &AttitudeSolution) -> Self {
        Self { pointing: from_vec3(&sol.pointing), roll: sol.roll as f64 }
    }

    pub fn pointing_vec3(&self) -> Vec3 {
        Vec3(self.pointing[0] as f32, self.pointing[1] as f32, self.pointing[2] as f32)
    }
}

// ───────────────────────── projection & matching ─────────────────────────

struct Proj {
    px: f64,
    py: f64,
    theta: f64,
    pa: f64,
}

fn project_catalog(cat: &[(Vec3, f32)], pose: &Pose, lens: &LensModel, theta_lim: f64) -> Vec<Proj> {
    let (n, e) = sky_frame(&pose.pointing);
    let cos_lim = theta_lim.cos();
    let mut out = Vec::new();
    for (v, _) in cat {
        let v = from_vec3(v);
        let c = dot(&v, &pose.pointing);
        if c < cos_lim {
            continue;
        }
        let theta = c.clamp(-1.0, 1.0).acos();
        let pa = dot(&v, &e).atan2(dot(&v, &n));
        let r = lens.radius(theta);
        let phi = pa + pose.roll;
        out.push(Proj { px: r * phi.sin(), py: r * phi.cos(), theta, pa });
    }
    out
}

/// A detected star paired with a catalog star.
#[derive(Clone, Copy)]
struct Match {
    /// Observed position (px from image center).
    x: f64,
    y: f64,
    /// Catalog star's angular distance / position angle from the pointing.
    theta: f64,
    pa: f64,
}

/// Greedy one-to-one nearest-neighbour matching in pixel space. Only
/// detections whose model θ is within `gate` take part.
fn match_stars(
    stars: &[(f64, f64)],
    proj: &[Proj],
    lens: &LensModel,
    gate: f64,
    tol_base: f64,
    tol_frac: f64,
) -> Vec<Match> {
    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for (si, &(x, y)) in stars.iter().enumerate() {
        let r = x.hypot(y);
        match lens.theta(r) {
            Some(t) if t <= gate => {}
            _ => continue,
        }
        let tol = tol_base + tol_frac * r;
        for (pi, p) in proj.iter().enumerate() {
            let d2 = (p.px - x).powi(2) + (p.py - y).powi(2);
            if d2 <= tol * tol {
                pairs.push((d2, si, pi));
            }
        }
    }
    pairs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let mut star_used = vec![false; stars.len()];
    let mut cat_used = vec![false; proj.len()];
    let mut out = Vec::new();
    for (_, si, pi) in pairs {
        if star_used[si] || cat_used[pi] {
            continue;
        }
        star_used[si] = true;
        cat_used[pi] = true;
        out.push(Match { x: stars[si].0, y: stars[si].1, theta: proj[pi].theta, pa: proj[pi].pa });
    }
    out
}

// ───────────────────────────── fitting ─────────────────────────────

/// Solve `A·x = b` (Gaussian elimination, partial pivoting).
fn solve_linear(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for col in 0..n {
        let piv = (col..n).max_by(|&i, &j| a[i][col].abs().partial_cmp(&a[j][col].abs()).unwrap())?;
        if a[piv][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        let pivot_row = a[col].clone();
        for row in col + 1..n {
            let k = a[row][col] / pivot_row[col];
            for (x, p) in a[row][col..n].iter_mut().zip(&pivot_row[col..n]) {
                *x -= k * p;
            }
            b[row] -= k * b[col];
        }
    }
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let s: f64 = (i + 1..n).map(|j| a[i][j] * x[j]).sum();
        x[i] = (b[i] - s) / a[i][i];
    }
    Some(x)
}

/// Least squares for `px = dx + r·sinφ`, `py = dy + r·cosφ` where
/// `r = Σ aₖ·basisₖ(θ)`. Returns `[a…, dx, dy]`.
fn lsq(m: &[Match], roll: f64, basis: &dyn Fn(f64) -> Vec<f64>) -> Option<Vec<f64>> {
    let nb = basis(0.5).len();
    let nu = nb + 2;
    let mut ata = vec![vec![0.0; nu]; nu];
    let mut atb = vec![0.0; nu];
    for x in m {
        let phi = x.pa + roll;
        let (s, c) = phi.sin_cos();
        let bs = basis(x.theta);
        for (obs, trig, off) in [(x.x, s, nb), (x.y, c, nb + 1)] {
            let mut row = vec![0.0; nu];
            for k in 0..nb {
                row[k] = bs[k] * trig;
            }
            row[off] = 1.0;
            for i in 0..nu {
                for j in 0..nu {
                    ata[i][j] += row[i] * row[j];
                }
                atb[i] += row[i] * obs;
            }
        }
    }
    solve_linear(ata, atb)
}

fn residuals(m: &[Match], roll: f64, lens: &LensModel, dx: f64, dy: f64) -> Vec<f64> {
    m.iter()
        .map(|x| {
            let phi = x.pa + roll;
            let r = lens.radius(x.theta);
            (x.x - dx - r * phi.sin()).hypot(x.y - dy - r * phi.cos())
        })
        .collect()
}

/// One fit of lens + axis offset + roll to the matches. Returns false if
/// there was not enough to go on (model left unchanged).
fn refine(pose: &mut Pose, lens: &mut LensModel, m: &[Match], f_ref: f64) -> bool {
    if m.len() < MIN_SCALE_FIT {
        return false;
    }
    let max_theta = m.iter().map(|x| x.theta).fold(0.0, f64::max);
    let full = m.len() >= MIN_FULL_FIT && max_theta >= FULL_FIT_MIN_THETA_DEG.to_radians();

    // Fit, then drop gross outliers and fit once more.
    let fit = |set: &[Match], lens: &LensModel| -> Option<(LensModel, f64, f64)> {
        if full {
            let u = lsq(set, pose.roll, &|t| vec![t, t.powi(3), t.powi(5)])?;
            let a1 = u[0];
            let cand = LensModel { f: a1, shape: Shape::Poly { c3: u[1] / a1, c5: u[2] / a1 } };
            if cand.is_sane() && a1 > 0.5 * f_ref && a1 < 2.0 * f_ref {
                return Some((cand, u[3], u[4]));
            }
            None
        } else {
            let l = *lens;
            let u = lsq(set, pose.roll, &|t| vec![l.g(t)])?;
            let cand = LensModel { f: u[0], shape: l.shape };
            if u[0] > 0.5 * f_ref && u[0] < 2.0 * f_ref {
                return Some((cand, u[1], u[2]));
            }
            None
        }
    };
    let Some(first) = fit(m, lens) else { return false };
    // Iterative clipping: refit on the matches within 2.5× the median residual
    // (at least 1.5 px), so a few false matches cannot drag the fit.
    let (mut new_lens, mut dx, mut dy) = first;
    let mut kept: Vec<Match> = m.to_vec();
    for _ in 0..3 {
        let res = residuals(&kept, pose.roll, &new_lens, dx, dy);
        let mut sorted = res.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let clip = (2.5 * sorted[sorted.len() / 2]).max(1.5);
        let next: Vec<Match> =
            kept.iter().zip(&res).filter(|(_, r)| **r <= clip).map(|(x, _)| *x).collect();
        if next.len() < MIN_SCALE_FIT || next.len() == kept.len() {
            break;
        }
        match fit(&next, lens) {
            Some(f) => (new_lens, dx, dy) = f,
            None => break,
        }
        kept = next;
    }

    // Roll: mean angular residual about the fitted axis, weighted by r²
    // (inliers only).
    let (mut sw, mut s) = (0.0, 0.0);
    for x in &kept {
        let (ox, oy) = (x.x - dx, x.y - dy);
        let r = ox.hypot(oy);
        if r < 20.0 {
            continue;
        }
        let d = wrap(ox.atan2(oy) - (x.pa + pose.roll));
        if d.abs() > 0.15 {
            continue;
        }
        sw += r * r;
        s += r * r * d;
    }

    // Fold the axis offset into the pointing: the image center now looks at
    // the sky point that the model puts at (-dx, -dy).
    let (n, e) = sky_frame(&pose.pointing);
    let dir = |pa: f64| add_scaled(&[n[0] * pa.cos(), n[1] * pa.cos(), n[2] * pa.cos()], &e, pa.sin());
    let ux = dir(std::f64::consts::FRAC_PI_2 - pose.roll); // image +x
    let uy = dir(-pose.roll); // image +y
    let mut p = add_scaled(&pose.pointing, &ux, -dx / new_lens.f);
    p = add_scaled(&p, &uy, -dy / new_lens.f);
    pose.pointing = normalize(p);
    if sw > 0.0 {
        pose.roll = wrap(pose.roll + s / sw);
    }
    *lens = new_lens;
    true
}

// ───────────────────────────── driver ─────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct Calibration {
    pub pose: Pose,
    pub lens: LensModel,
    /// Stars matched to the catalog over the whole frame.
    pub matched: usize,
    /// …of which lie beyond `INNER_DEG` (independent of the pattern match).
    pub outer: usize,
    /// Outer matches expected by chance at this tolerance.
    pub outer_chance: f64,
    pub rms_px: f64,
    /// Passed validation.
    pub valid: bool,
}

/// Refine `init` (a pinhole-based attitude from the pattern matcher), fit the
/// lens and validate. `stars` are pixel offsets from the image center;
/// `lens0` is the starting profile (pinhole, or a previous calibration).
/// `None` when too few catalog stars line up to fit anything.
pub fn calibrate(
    catalog: &[(Vec3, f32)],
    stars: &[(f32, f32)],
    init: Pose,
    lens0: LensModel,
) -> Option<Calibration> {
    let stars: Vec<(f64, f64)> = stars.iter().map(|&(x, y)| (x as f64, y as f64)).collect();
    let f_ref = lens0.f;
    let mut pose = init;
    let mut lens = lens0;

    // The focal length is only a guess, and being a few % off already pushes
    // stars past the match tolerance. Scan the scale and start from the one
    // that lines up the most stars in the central region.
    {
        let gate = SCALE_SEARCH_DEG.to_radians();
        let mut best = (0usize, 1.0f64);
        for i in -20..=20 {
            let s = 1.0 + i as f64 * 0.01;
            let cand = LensModel { f: lens.f * s, ..lens };
            let proj = project_catalog(catalog, &pose, &cand, gate + 3f64.to_radians());
            let n = match_stars(&stars, &proj, &cand, gate, 2.5, 0.015).len();
            // Prefer the scale closest to 1 among equals.
            if n > best.0 || (n == best.0 && (s - 1.0).abs() < (best.1 - 1.0).abs()) {
                best = (n, s);
            }
        }
        lens.f *= best.1;
    }

    let mut tmax = START_DEG.to_radians();
    let mut first = true;
    while tmax <= END_DEG.to_radians() && tmax <= lens.theta_limit() {
        // Wide tolerance first to catch the new ring, then tighter ones so that
        // junk detections that happened to fall near a catalog star drop out.
        for k in [1.0, 0.6, 0.4] {
            let proj = project_catalog(catalog, &pose, &lens, (tmax + 8f64.to_radians()).min(lens.theta_limit()));
            let m = match_stars(&stars, &proj, &lens, tmax, TOL_BASE * k, TOL_FRAC * k);
            // Too few matches yet (sparse centre): widen the ring and retry.
            if !refine(&mut pose, &mut lens, &m, f_ref) {
                break;
            }
            first = false;
        }
        tmax += RING_STEP_DEG.to_radians();
    }
    if first {
        return None;
    }

    // Final pass with a tight tolerance over everything the model covers.
    let lim = lens.theta_limit();
    for _ in 0..2 {
        let proj = project_catalog(catalog, &pose, &lens, lim);
        let m = match_stars(&stars, &proj, &lens, lim, FINAL_TOL_BASE, FINAL_TOL_FRAC);
        if !refine(&mut pose, &mut lens, &m, f_ref) {
            break;
        }
    }
    let proj = project_catalog(catalog, &pose, &lens, lim);
    let m = match_stars(&stars, &proj, &lens, lim, FINAL_TOL_BASE, FINAL_TOL_FRAC);

    let inner = INNER_DEG.to_radians();
    let outer = m.iter().filter(|x| x.theta > inner).count();
    let res = residuals(&m, pose.roll, &lens, 0.0, 0.0);
    let rms_px = if res.is_empty() {
        f64::INFINITY
    } else {
        (res.iter().map(|r| r * r).sum::<f64>() / res.len() as f64).sqrt()
    };

    // Chance level for the outer zone: P(a random point lies within tol of
    // some in-frame catalog star there) × number of outer detections.
    let r_in = lens.radius(inner);
    let mut outer_area = 0.0;
    let mut y = -FRAME_H / 2.0 + 2.0;
    while y < FRAME_H / 2.0 {
        let mut x = -FRAME_W / 2.0 + 2.0;
        while x < FRAME_W / 2.0 {
            if x.hypot(y) > r_in {
                outer_area += 16.0;
            }
            x += 4.0;
        }
        y += 4.0;
    }
    let cat_outer = proj
        .iter()
        .filter(|p| p.theta > inner && p.px.abs() < FRAME_W / 2.0 && p.py.abs() < FRAME_H / 2.0)
        .count() as f64;
    let n_outer_det = stars
        .iter()
        .filter(|&&(x, y)| x.hypot(y) > r_in && x.abs() < FRAME_W / 2.0 && y.abs() < FRAME_H / 2.0)
        .count() as f64;
    let tol = FINAL_TOL_BASE + FINAL_TOL_FRAC * 250.0;
    let p_chance = if outer_area > 0.0 {
        (cat_outer * std::f64::consts::PI * tol * tol / outer_area).min(1.0)
    } else {
        1.0
    };
    let outer_chance = n_outer_det * p_chance;
    let needed = (outer_chance + SIGMA_MARGIN * outer_chance.sqrt()).ceil().max(MIN_OUTER as f64);

    let valid = m.len() >= MIN_MATCHED && outer as f64 >= needed && rms_px <= MAX_RMS_PX;
    Some(Calibration { pose, lens, matched: m.len(), outer, outer_chance, rms_px, valid })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tiny deterministic generator (no rand dependency).
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((self.0 >> 11) as f64) / ((1u64 << 53) as f64)
        }
        fn gauss(&mut self) -> f64 {
            let (u, v) = (self.next().max(1e-12), self.next());
            (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()
        }
    }

    fn sky_catalog(n: usize, rng: &mut Lcg) -> Vec<(Vec3, f32)> {
        (0..n)
            .map(|_| {
                let z = 2.0 * rng.next() - 1.0;
                let a = 2.0 * std::f64::consts::PI * rng.next();
                let r = (1.0 - z * z).sqrt();
                (Vec3((r * a.cos()) as f32, (r * a.sin()) as f32, z as f32), 4.0)
            })
            .collect()
    }

    fn pose_radec(ra: f64, dec: f64, roll: f64) -> Pose {
        let (ra, dec) = (ra.to_radians(), dec.to_radians());
        Pose {
            pointing: [dec.cos() * ra.cos(), dec.cos() * ra.sin(), dec.sin()],
            roll: roll.to_radians(),
        }
    }

    fn angle_deg(a: &V3, b: &V3) -> f64 {
        dot(a, b).clamp(-1.0, 1.0).acos().to_degrees()
    }

    /// Detections a camera with `lens` at `pose` would produce.
    fn simulate(cat: &[(Vec3, f32)], pose: &Pose, lens: &LensModel, rng: &mut Lcg, n_false: usize) -> Vec<(f32, f32)> {
        let proj = project_catalog(cat, pose, lens, lens.theta_limit());
        let mut out = Vec::new();
        for p in proj {
            if p.px.abs() < FRAME_W / 2.0 && p.py.abs() < FRAME_H / 2.0 && rng.next() < 0.7 {
                out.push(((p.px + 0.4 * rng.gauss()) as f32, (p.py + 0.4 * rng.gauss()) as f32));
            }
        }
        for _ in 0..n_false {
            out.push((
                ((rng.next() - 0.5) * FRAME_W) as f32,
                ((rng.next() - 0.5) * FRAME_H) as f32,
            ));
        }
        out
    }

    #[test]
    fn pixel_to_sky_inverts_projection() {
        let pose = pose_radec(335.9, 65.0, -86.5);
        let lens = LensModel { f: 330.0, shape: Shape::Poly { c3: -0.07, c5: -0.014 } };
        let mut rng = Lcg(7);
        let cat = sky_catalog(4000, &mut rng);
        let mut n = 0;
        for p in project_catalog(&cat, &pose, &lens, 1.0) {
            let sky = pixel_to_sky(&pose, &lens, p.px, p.py).unwrap();
            // the catalog star this pixel came from must be recovered
            let best = cat
                .iter()
                .map(|(v, _)| angle_deg(&from_vec3(v), &sky))
                .fold(f64::MAX, f64::min);
            // loose: the f32 catalog vectors are not exactly unit, which
            // costs ~1e-2° in acos(θ) for stars right at the axis
            assert!(best < 2e-2, "recovered {best}° off");
            n += 1;
        }
        assert!(n > 100);
    }

    #[test]
    fn lens_theta_inverts_radius() {
        for lens in [
            LensModel::pinhole(344.0),
            LensModel { f: 320.0, shape: Shape::Poly { c3: 0.1, c5: 0.0 } },
        ] {
            for deg in [1.0, 10.0, 30.0, 55.0] {
                let th = f64::to_radians(deg);
                let back = lens.theta(lens.radius(th)).unwrap();
                assert!((back - th).abs() < 1e-6, "{deg}° → {}", back.to_degrees());
            }
        }
    }

    #[test]
    fn recovers_distorted_lens_and_validates() {
        for c3 in [0.0, 0.10, -0.05] {
            let mut rng = Lcg(42);
            let cat = sky_catalog(1600, &mut rng);
            let truth = pose_radec(356.4, 56.35, 99.1);
            let true_lens = LensModel { f: 320.0, shape: Shape::Poly { c3, c5: 0.0 } };
            let stars = simulate(&cat, &truth, &true_lens, &mut rng, 15);

            // What the pattern matcher would hand over: slightly off.
            let mut init = pose_radec(356.4, 56.35, 99.1);
            init.pointing = normalize(add_scaled(&init.pointing, &[0.0, 0.0, 1.0], 0.006));
            init.roll += 1.0f64.to_radians();

            let cal = calibrate(&cat, &stars, init, LensModel::pinhole(344.0)).expect("calibration");
            assert!(cal.valid, "c3={c3}: {cal:?}");
            assert!(cal.rms_px < 1.5, "c3={c3}: rms {}", cal.rms_px);
            assert!(angle_deg(&cal.pose.pointing, &truth.pointing) < 0.1, "c3={c3}: pointing");
            assert!(wrap(cal.pose.roll - truth.roll).to_degrees().abs() < 0.3, "c3={c3}: roll");
            for deg in [10.0, 25.0, 40.0] {
                let th = f64::to_radians(deg);
                let err = cal.lens.radius(th) / true_lens.radius(th) - 1.0;
                assert!(err.abs() < 0.015, "c3={c3}: radius error {err} at {deg}°");
            }
        }
    }

    #[test]
    fn rejects_wrong_attitude() {
        let mut rng = Lcg(7);
        let cat = sky_catalog(1600, &mut rng);
        let truth = pose_radec(356.4, 56.35, 99.1);
        let lens = LensModel { f: 320.0, shape: Shape::Poly { c3: 0.1, c5: 0.0 } };
        let stars = simulate(&cat, &truth, &lens, &mut rng, 15);
        for wrong in [pose_radec(20.0, 30.0, 10.0), pose_radec(356.4, 56.35, 190.0), pose_radec(10.0, 50.0, 99.1)] {
            match calibrate(&cat, &stars, wrong, LensModel::pinhole(344.0)) {
                None => {}
                Some(c) => assert!(!c.valid, "accepted a wrong attitude: {c:?}"),
            }
        }
    }

    /// Real stars survive, but a lot of junk is detected too (clouds, noise).
    #[test]
    fn accepts_true_solution_buried_in_junk() {
        let lens = LensModel { f: 320.0, shape: Shape::Poly { c3: 0.1, c5: 0.0 } };
        for (n_junk, keep) in [(0usize, 0.7), (100, 0.7), (200, 0.7), (200, 0.4), (300, 0.3)] {
            let mut ok = 0;
            let trials = 10;
            for seed in 0..trials {
                let mut rng = Lcg(1000 + seed);
                let cat = sky_catalog(1600, &mut rng);
                let truth = pose_radec(356.4, 56.35, 99.1);
                let proj = project_catalog(&cat, &truth, &lens, lens.theta_limit());
                let mut stars = Vec::new();
                for p in proj {
                    if p.px.abs() < FRAME_W / 2.0 && p.py.abs() < FRAME_H / 2.0 && rng.next() < keep {
                        stars.push(((p.px + 0.4 * rng.gauss()) as f32, (p.py + 0.4 * rng.gauss()) as f32));
                    }
                }
                for _ in 0..n_junk {
                    stars.push((((rng.next() - 0.5) * FRAME_W) as f32, ((rng.next() - 0.5) * FRAME_H) as f32));
                }
                let mut init = pose_radec(356.4, 56.35, 99.1);
                init.pointing = normalize(add_scaled(&init.pointing, &[0.0, 0.0, 1.0], 0.006));
                if calibrate(&cat, &stars, init, LensModel::pinhole(344.0)).is_some_and(|c| c.valid) {
                    ok += 1;
                }
            }
            eprintln!("junk={n_junk} keep={keep}: {ok}/{trials} accepted");
            // Clear-sky and moderately junky frames must always be accepted;
            // the extreme one (30% of stars visible, 300 junk) most of the time.
            let need = if n_junk <= 200 { trials } else { 7 };
            assert!(ok >= need, "junk={n_junk} keep={keep}: only {ok}/{trials} accepted");
        }
    }

    #[test]
    fn rejects_unrelated_stars() {
        let mut rng = Lcg(99);
        let cat = sky_catalog(1600, &mut rng);
        let stars: Vec<(f32, f32)> = (0..120)
            .map(|_| (((rng.next() - 0.5) * FRAME_W) as f32, ((rng.next() - 0.5) * FRAME_H) as f32))
            .collect();
        let res = calibrate(&cat, &stars, pose_radec(100.0, 20.0, 0.0), LensModel::pinhole(344.0));
        assert!(res.is_none_or(|c| !c.valid));
    }
}
