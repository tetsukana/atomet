pub mod extractor;

use std::{
    fs::File,
    io::{BufReader, Read},
    ops::{Add, AddAssign, Div, Mul, Sub},
};

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Vec2(pub f32, pub f32);

impl Add for Vec2 {
    type Output = Vec2;
    fn add(self, other: Vec2) -> Vec2 {
        Vec2(self.0 + other.0, self.1 + other.1)
    }
}

impl Sub for Vec2 {
    type Output = Vec2;
    fn sub(self, other: Vec2) -> Vec2 {
        Vec2(self.0 - other.0, self.1 - other.1)
    }
}

impl Div<f32> for Vec2 {
    type Output = Vec2;
    fn div(self, scalar: f32) -> Vec2 {
        Vec2(self.0 / scalar, self.1 / scalar)
    }
}

impl Vec2 {
    fn norm(&self) -> f32 {
        (self.0 * self.0 + self.1 * self.1).sqrt()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Vec3(pub f32, pub f32, pub f32);

impl Add for Vec3 {
    type Output = Vec3;
    fn add(self, other: Vec3) -> Self::Output {
        Vec3(self.0 + other.0, self.1 + other.1, self.2 + other.2)
    }
}

impl AddAssign for Vec3 {
    fn add_assign(&mut self, other: Vec3) {
        self.0 += other.0;
        self.1 += other.1;
        self.2 += other.2;
    }
}

impl Sub for Vec3 {
    type Output = Vec3;
    fn sub(self, other: Vec3) -> Vec3 {
        Vec3(self.0 - other.0, self.1 - other.1, self.2 - other.2)
    }
}

impl Mul<f32> for Vec3 {
    type Output = Vec3;
    fn mul(self, scalar: f32) -> Vec3 {
        Vec3(self.0 * scalar, self.1 * scalar, self.2 * scalar)
    }
}

impl Div<f32> for Vec3 {
    type Output = Vec3;
    fn div(self, scalar: f32) -> Self::Output {
        self * (1. / scalar)
    }
}

impl Vec3 {
    pub fn dot(&self, other: &Vec3) -> f32 {
        self.0 * other.0 + self.1 * other.1 + self.2 * other.2
    }

    pub fn norm(&self) -> f32 {
        self.dot(self).sqrt()
    }

    pub fn normalize(&self) -> Vec3 {
        *self / self.norm()
    }

    pub fn cross(&self, other: &Vec3) -> Vec3 {
        Vec3(
            self.1 * other.2 - self.2 * other.1,
            self.2 * other.0 - self.0 * other.2,
            self.0 * other.1 - self.1 * other.0,
        )
    }

    pub fn angle_between(&self, other: &Vec3) -> f32 {
        let dot = self.dot(other);
        let norms = self.norm() * other.norm();
        (dot / norms).acos()
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Equatorial {
    pub ra: f32,
    pub dec: f32,
}

impl Equatorial {
    pub fn to_cartesian(self) -> Vec3 {
        let x = self.dec.cos() * self.ra.cos();
        let y = self.dec.cos() * self.ra.sin();
        let z = self.dec.sin();
        Vec3(x, y, z)
    }

    /// Convert a unit vector on the celestial sphere to equatorial coordinates.
    pub fn from_cartesian(v: Vec3) -> Self {
        let dec = v.2.clamp(-1.0, 1.0).asin();
        let ra = v.1.atan2(v.0).rem_euclid(2.0 * std::f32::consts::PI);
        Self { ra, dec }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct HipStar {
    pub id: u32,
    pub mag: f32,
    pub eq: Equatorial,
}

pub struct Pattern {
    /// Direction vector of the longest edge (b - a in image space, stored in sky frame).
    pub(crate) ref_dir: Vec3,
    /// Centroid of the 4-star quad in sky coordinates (unit vector on celestial sphere).
    pub(crate) sky_centroid: Vec3,
}

impl Pattern {
    pub fn from_compressed(compressed: u64) -> Pattern {
        let ref_dir_x = fixed10_to_float((compressed & 0x3FF) as u16);
        let ref_dir_y = fixed10_to_float(((compressed >> 10) & 0x3FF) as u16);
        let ref_dir_z = fixed10_to_float(((compressed >> 20) & 0x3FF) as u16);
        let centroid_x = fixed10_to_float(((compressed >> 30) & 0x3FF) as u16);
        let centroid_y = fixed10_to_float(((compressed >> 40) & 0x3FF) as u16);
        let centroid_z = fixed10_to_float(((compressed >> 50) & 0x3FF) as u16);

        Pattern {
            ref_dir: Vec3(ref_dir_x, ref_dir_y, ref_dir_z),
            sky_centroid: Vec3(centroid_x, centroid_y, centroid_z),
        }
    }

    /// Returns the direction perpendicular to both `sky_centroid` and `ref_dir`,
    /// scaled to match the magnitude of `ref_dir`.
    pub fn perp_dir(&self) -> Vec3 {
        let n = self.sky_centroid.cross(&self.ref_dir);
        n * (self.ref_dir.norm() / n.norm())
    }
}

/// Number of quantization bins used for pattern key generation.
/// Must match the value used during database generation.
pub const QUANTIZATION_BINS: f32 = 61.0;

pub fn float_to_fixed10(value: f32) -> u16 {
    let clamped = value.clamp(-1.0, 1.0);
    let scaled = (clamped * 511.0).round() as i16;
    (scaled as u16) & 0x3FF
}

#[inline(always)]
fn fixed10_to_float(fixed: u16) -> f32 {
    let masked = fixed & 0x3FF;
    let signed = if masked & 0x200 != 0 {
        (masked as i16) | !0x3FF
    } else {
        masked as i16
    };
    signed as f32 / 511.
}

/// Star pattern database loaded from a binary file.
///
/// Encapsulates the LUT, sorted keys, per-key pattern offsets, compressed
/// pattern vectors, and the star catalog used during solving.
pub struct Database {
    /// Fast bucket lookup: maps the top 12 bits of a key to an index range in `keys`.
    lut: Vec<u32>,
    /// Sorted 30-bit pattern keys.
    keys: Vec<u32>,
    /// Per-key offset into `patterns`: packed as `(start << 12) | length`.
    offsets: Vec<u32>,
    /// Compressed (ref_dir, sky_centroid) pairs — 6 × 10-bit fixed-point per entry.
    patterns: Vec<u64>,
    /// Star catalog: (unit vector on celestial sphere, visual magnitude).
    pub stars: Vec<(Vec3, f32)>,
}

impl Database {
    /// Load a database from a binary file produced by `generate_database`.
    pub fn load(path: &str) -> Self {
        let file = File::open(path).expect("Could not open database file");
        let mut reader = BufReader::new(file);

        macro_rules! read_u32 {
            () => {{
                let mut b = [0u8; 4];
                reader.read_exact(&mut b).unwrap();
                u32::from_be_bytes(b)
            }};
        }
        macro_rules! read_f32 {
            () => {{
                let mut b = [0u8; 4];
                reader.read_exact(&mut b).unwrap();
                f32::from_be_bytes(b)
            }};
        }
        macro_rules! read_u64 {
            () => {{
                let mut b = [0u8; 8];
                reader.read_exact(&mut b).unwrap();
                u64::from_be_bytes(b)
            }};
        }

        let n_keys = read_u32!() as usize;
        let n_patterns = read_u32!() as usize;
        let n_lut = read_u32!() as usize;
        let n_stars = read_u32!() as usize;

        let mut keys = Vec::with_capacity(n_keys);
        for _ in 0..n_keys {
            keys.push(read_u32!());
        }

        let mut offsets = Vec::with_capacity(n_keys);
        for _ in 0..n_keys {
            offsets.push(read_u32!());
        }

        let mut patterns = Vec::with_capacity(n_patterns);
        for _ in 0..n_patterns {
            patterns.push(read_u64!());
        }

        let mut lut = Vec::with_capacity(n_lut);
        for _ in 0..n_lut {
            lut.push(read_u32!());
        }

        let mut stars = Vec::with_capacity(n_stars);
        for _ in 0..n_stars {
            let x = read_f32!();
            let y = read_f32!();
            let z = read_f32!();
            let mag = read_f32!();
            stars.push((Vec3(x, y, z), mag));
        }

        Self {
            lut,
            keys,
            offsets,
            patterns,
            stars,
        }
    }

    /// Load a database from a byte slice (e.g. embedded or mmap'd).
    pub fn load_from_bytes(data: &[u8]) -> Self {
        let mut pos = 0usize;

        macro_rules! read_u32 {
            () => {{
                let b = &data[pos..pos + 4];
                pos += 4;
                u32::from_be_bytes([b[0], b[1], b[2], b[3]])
            }};
        }
        macro_rules! read_f32 {
            () => {{
                let b = &data[pos..pos + 4];
                pos += 4;
                f32::from_be_bytes([b[0], b[1], b[2], b[3]])
            }};
        }
        macro_rules! read_u64 {
            () => {{
                let b = &data[pos..pos + 8];
                pos += 8;
                u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
            }};
        }

        let n_keys = read_u32!() as usize;
        let n_patterns = read_u32!() as usize;
        let n_lut = read_u32!() as usize;
        let n_stars = read_u32!() as usize;

        let mut keys = Vec::with_capacity(n_keys);
        for _ in 0..n_keys {
            keys.push(read_u32!());
        }

        let mut offsets = Vec::with_capacity(n_keys);
        for _ in 0..n_keys {
            offsets.push(read_u32!());
        }

        let mut patterns = Vec::with_capacity(n_patterns);
        for _ in 0..n_patterns {
            patterns.push(read_u64!());
        }

        let mut lut = Vec::with_capacity(n_lut);
        for _ in 0..n_lut {
            lut.push(read_u32!());
        }

        let mut stars = Vec::with_capacity(n_stars);
        for _ in 0..n_stars {
            let x = read_f32!();
            let y = read_f32!();
            let z = read_f32!();
            let mag = read_f32!();
            stars.push((Vec3(x, y, z), mag));
        }

        Self {
            lut,
            keys,
            offsets,
            patterns,
            stars,
        }
    }

    /// Attempt to solve the pointing direction and roll angle from a list of
    /// star centroids.
    ///
    /// * `centroids`        – (x, y) positions sorted by brightness (brightest first).
    ///   For raw pixel coordinates, origin is the image center.
    ///   For tangent-plane coordinates, use `focal_length_px = 1.0`.
    /// * `center`           – coordinate of the image center (usually (0, 0)).
    /// * `focal_length_px`  – focal length in the same units as the centroid coordinates.
    ///   Pass `1.0` for tangent-plane inputs; pass the actual
    ///   focal length in pixels for raw pixel inputs.
    /// * `dot_threshold`    – cos(max angular error) for two hypotheses to be considered equal.
    /// * `max_count`        – stop after evaluating this many pattern hypotheses.
    /// * `early_stop_n`     – return immediately once a candidate reaches this vote count.
    pub fn solve(
        &self,
        centroids: &[(f32, f32)],
        center: (f32, f32),
        focal_length_px: f32,
        dot_threshold: f32,
        max_count: usize,
        early_stop_n: Option<usize>,
    ) -> Option<AttitudeSolution> {
        solve_inner(
            &self.lut,
            &self.keys,
            &self.offsets,
            &self.patterns,
            centroids,
            center,
            focal_length_px,
            dot_threshold,
            max_count,
            early_stop_n,
        )
    }
}

#[inline(always)]
fn fast_asin(x: f32) -> f32 {
    let x2 = x * x;
    x * (1.0 + x2 * 0.16666667)
}

/// Convert a pixel or tangent-plane point to a local unit direction vector.
///
/// For tangent-plane inputs (e.g. from `tan_projection`), pass `focal_length_px = 1.0`.
/// For raw pixel coordinates, pass the actual focal length in pixels so that the
/// geometry `Vec3(focal_length_px, px, py)` correctly represents the ray direction.
#[inline(always)]
fn tangent_to_unit(p: Vec2, focal_length_px: f32) -> Vec3 {
    Vec3(focal_length_px, p.0, p.1).normalize()
}

#[inline(always)]
fn edge_length2(points: [Vec2; 4], focal_length_px: f32) -> [(f32, usize, usize); 6] {
    // Project each point onto the unit sphere before measuring distances.
    // Without this, 2D Euclidean distance ≈ tan(θ) rather than the chord sin(θ),
    // causing ~2% error at 30° FOV that shifts the pattern key and causes mismatches.
    let v = [
        tangent_to_unit(points[0], focal_length_px),
        tangent_to_unit(points[1], focal_length_px),
        tangent_to_unit(points[2], focal_length_px),
        tangent_to_unit(points[3], focal_length_px),
    ];
    let d = |i: usize, j: usize| fast_asin((v[i] - v[j]).norm() / 2.) * 2.;
    [
        (d(0, 1), 0, 1),
        (d(0, 2), 0, 2),
        (d(0, 3), 0, 3),
        (d(1, 2), 1, 2),
        (d(1, 3), 1, 3),
        (d(2, 3), 2, 3),
    ]
}

#[inline(always)]
fn sort6(a: &mut [(f32, usize, usize); 6]) {
    macro_rules! cs {
        ($i:expr, $j:expr) => {
            if a[$i].0 > a[$j].0 {
                a.swap($i, $j);
            }
        };
    }

    cs!(1, 2);
    cs!(4, 5);
    cs!(0, 2);
    cs!(3, 5);
    cs!(0, 1);
    cs!(3, 4);
    cs!(2, 5);
    cs!(0, 3);
    cs!(1, 4);
    cs!(2, 4);
    cs!(1, 3);
    cs!(2, 3);
}

/// The result of a plate solve.
#[derive(Debug, Clone, Copy)]
pub struct AttitudeSolution {
    /// Pointing direction of the image center as a unit vector on the celestial sphere.
    pub pointing: Vec3,

    /// Roll angle in radians: rotation of the camera frame relative to the North-up
    /// orientation. `roll = 0` means image y-axis points toward celestial North.
    /// Positive roll = camera rotated clockwise when looking at the sky.
    pub roll: f32,

    /// Number of independent pattern hypotheses that agreed on this solution.
    /// Higher = more confident.
    pub votes: usize,
}

impl AttitudeSolution {
    /// Convert to WCS (World Coordinate System) parameters.
    ///
    /// Produces a gnomonic (TAN) projection compatible with the FITS WCS standard.
    ///
    /// `pixel_scale_deg`: angular size of one pixel in degrees (must be positive).
    pub fn to_wcs(&self, pixel_scale_deg: f32) -> WcsSolution {
        let eq = Equatorial::from_cartesian(self.pointing);
        let (sin_r, cos_r) = self.roll.sin_cos();
        let s = pixel_scale_deg;

        WcsSolution {
            ra_deg: eq.ra.to_degrees(),
            dec_deg: eq.dec.to_degrees(),
            cd: [[-s * cos_r, s * sin_r], [s * sin_r, s * cos_r]],
        }
    }
}

/// WCS (World Coordinate System) parameters for a gnomonic (TAN) projection.
///
/// Compatible with the FITS WCS standard (Calabretta & Greisen 2002).
#[derive(Debug, Clone, Copy)]
pub struct WcsSolution {
    /// RA at the reference pixel (CRVAL1) in degrees, range [0, 360).
    pub ra_deg: f32,
    /// Declination at the reference pixel (CRVAL2) in degrees, range [-90, 90].
    pub dec_deg: f32,
    /// CD transformation matrix [degrees/pixel].
    pub cd: [[f32; 2]; 2],
}

impl WcsSolution {
    /// Pixel scale in degrees per pixel.
    pub fn pixel_scale_deg(&self) -> f32 {
        (self.cd[1][0].powi(2) + self.cd[1][1].powi(2)).sqrt()
    }

    /// CROTA2 in degrees.
    pub fn crota2_deg(&self) -> f32 {
        self.cd[0][1].atan2(self.cd[1][1]).to_degrees()
    }
}

/// Compute the roll angle from a matched pattern hypothesis.
fn compute_roll(pointing: Vec3, c1: Vec2, ref_dir: Vec3) -> f32 {
    let north = Vec3(0.0, 0.0, 1.0);
    let y_raw = north - pointing * north.dot(&pointing);
    let y_sky = if y_raw.norm() > 1e-12 {
        y_raw.normalize()
    } else {
        let xaxis = Vec3(1.0, 0.0, 0.0);
        (xaxis - pointing * xaxis.dot(&pointing)).normalize()
    };
    let x_sky = y_sky.cross(&pointing).normalize();

    let phi_sky = ref_dir.dot(&x_sky).atan2(ref_dir.dot(&y_sky));
    let phi_img = c1.0.atan2(c1.1);

    phi_img - phi_sky
}

/// Accumulates pointing and roll votes for one candidate attitude.
struct Candidate {
    pointing: Vec3,
    roll_cos_sum: f32,
    roll_sin_sum: f32,
    votes: usize,
}

impl Candidate {
    fn new(pointing: Vec3, roll: f32) -> Self {
        Self {
            pointing,
            roll_cos_sum: roll.cos(),
            roll_sin_sum: roll.sin(),
            votes: 1,
        }
    }

    fn update(&mut self, pointing: Vec3, roll: f32) {
        self.votes += 1;
        self.pointing = (self.pointing * ((self.votes - 1) as f32 / self.votes as f32)
            + pointing * (1.0 / self.votes as f32))
            .normalize();
        self.roll_cos_sum += roll.cos();
        self.roll_sin_sum += roll.sin();
    }

    fn roll(&self) -> f32 {
        self.roll_sin_sum.atan2(self.roll_cos_sum)
    }
}

fn solve_inner(
    lut: &[u32],
    keys: &[u32],
    offsets: &[u32],
    patterns: &[u64],
    centroids: &[(f32, f32)],
    center: (f32, f32),
    focal_length_px: f32,
    dot_threshold: f32,
    max_count: usize,
    early_stop_n: Option<usize>,
) -> Option<AttitudeSolution> {
    let mut candidates: Vec<Candidate> = vec![];
    let mut count = 0usize;
    'outer: for i in 0..centroids.len() {
        for j in 0..i {
            for k in 0..j {
                for l in 0..k {
                    let points = [
                        Vec2(centroids[l].0, centroids[l].1),
                        Vec2(centroids[k].0, centroids[k].1),
                        Vec2(centroids[j].0, centroids[j].1),
                        Vec2(centroids[i].0, centroids[i].1),
                    ];

                    let mut l = edge_length2(points, focal_length_px);
                    sort6(&mut l);

                    let largest_edge = l.last().unwrap();

                    let mut key: u32 = 0;
                    key |= ((((l[0].0 / largest_edge.0) * QUANTIZATION_BINS) as u8 & 0x3F) as u32)
                        << 26;
                    key |= ((((l[1].0 / largest_edge.0) * QUANTIZATION_BINS) as u8 & 0x3F) as u32)
                        << 20;
                    key |= ((((l[2].0 / largest_edge.0) * QUANTIZATION_BINS) as u8 & 0x3F) as u32)
                        << 14;
                    key |= ((((l[3].0 / largest_edge.0) * QUANTIZATION_BINS) as u8 & 0x3F) as u32)
                        << 8;
                    key |= ((((l[4].0 / largest_edge.0) * QUANTIZATION_BINS) as u8 & 0x3F) as u32)
                        << 2;

                    let bucket = ((key >> 20) & 0xFFF) as usize;
                    let start = lut[bucket] as usize;
                    let end = lut[bucket + 1] as usize;
                    let index = if let Ok(ff) = keys[start..end].binary_search(&key) {
                        ff + start
                    } else {
                        continue;
                    };

                    let centroid = (points[0] + points[1] + points[2] + points[3]) / 4.0;

                    let distance_from_centroid = [
                        (points[0] - centroid).norm(),
                        (points[1] - centroid).norm(),
                        (points[2] - centroid).norm(),
                        (points[3] - centroid).norm(),
                    ];

                    let a = if distance_from_centroid[largest_edge.1]
                        > distance_from_centroid[largest_edge.2]
                    {
                        points[largest_edge.1]
                    } else {
                        points[largest_edge.2]
                    };

                    let b = if distance_from_centroid[largest_edge.1]
                        > distance_from_centroid[largest_edge.2]
                    {
                        points[largest_edge.2]
                    } else {
                        points[largest_edge.1]
                    };

                    let c1 = b - a;

                    let ab = (center.0 - centroid.0, center.1 - centroid.1);
                    let det = c1.0 * c1.0 + c1.1 * c1.1;

                    let alpha = (ab.0 * c1.0 + ab.1 * c1.1) / det;
                    let beta = (c1.0 * ab.1 - c1.1 * ab.0) / det;

                    let info = offsets[index];
                    let start = ((info >> 12) & 0xFFFFF) as usize;
                    let length = (info & 0xFFF) as usize;

                    for compressed in patterns.iter().skip(start).take(length) {
                        let pattern = Pattern::from_compressed(*compressed);
                        let perp = pattern.perp_dir();

                        let solved_vector =
                            (pattern.sky_centroid + pattern.ref_dir * alpha + perp * beta)
                                .normalize();

                        let roll = compute_roll(solved_vector, c1, pattern.ref_dir);

                        let mut matched_idx = None;
                        for (idx, cand) in candidates.iter_mut().enumerate() {
                            if solved_vector.dot(&cand.pointing) > dot_threshold {
                                cand.update(solved_vector, roll);
                                matched_idx = Some((cand.votes, idx));
                                break;
                            }
                        }

                        match matched_idx {
                            Some((votes, idx)) => {
                                candidates.swap(0, idx);
                                if let Some(threshold_n) = early_stop_n
                                    && votes >= threshold_n
                                {
                                    break 'outer;
                                }
                            }
                            None => candidates.push(Candidate::new(solved_vector, roll)),
                        }

                        count += 1;
                        if count > max_count {
                            break 'outer;
                        }
                    }
                }
            }
        }
    }

    candidates.sort_by_key(|b| std::cmp::Reverse(b.votes));

    let best = candidates.first()?;

    Some(AttitudeSolution {
        pointing: best.pointing,
        roll: best.roll(),
        votes: best.votes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vec3_dot_orthogonal() {
        let a = Vec3(1., 0., 0.);
        let b = Vec3(0., 1., 0.);
        assert_eq!(a.dot(&b), 0.0);
    }

    #[test]
    fn vec3_dot_self_is_norm_squared() {
        let v = Vec3(3., 4., 0.);
        assert!((v.dot(&v) - 25.0).abs() < 1e-6);
    }

    #[test]
    fn vec3_norm() {
        let v = Vec3(3., 4., 0.);
        assert!((v.norm() - 5.0).abs() < 1e-6);
    }

    #[test]
    fn vec3_normalize_has_unit_length() {
        let v = Vec3(3., 4., 12.);
        let n = v.normalize();
        assert!((n.norm() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn vec3_cross_xy_gives_z() {
        let x = Vec3(1., 0., 0.);
        let y = Vec3(0., 1., 0.);
        let z = x.cross(&y);
        assert!(z.0.abs() < 1e-6);
        assert!(z.1.abs() < 1e-6);
        assert!((z.2 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn vec3_cross_anticommutative() {
        let a = Vec3(1., 2., 3.);
        let b = Vec3(4., 5., 6.);
        let ab = a.cross(&b);
        let ba = b.cross(&a);
        assert!((ab.0 + ba.0).abs() < 1e-6);
        assert!((ab.1 + ba.1).abs() < 1e-6);
        assert!((ab.2 + ba.2).abs() < 1e-6);
    }

    #[test]
    fn vec3_angle_between_orthogonal() {
        let x = Vec3(1., 0., 0.);
        let y = Vec3(0., 1., 0.);
        let angle = x.angle_between(&y);
        assert!((angle - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
    }

    #[test]
    fn vec3_angle_between_parallel() {
        let v = Vec3(0., 0., 1.);
        assert!(v.angle_between(&v).abs() < 1e-5);
    }

    #[test]
    fn vec2_add_sub() {
        let a = Vec2(1., 2.);
        let b = Vec2(3., 4.);
        let s = a + b;
        let d = b - a;
        assert_eq!(s, Vec2(4., 6.));
        assert_eq!(d, Vec2(2., 2.));
    }

    #[test]
    fn vec2_norm() {
        let v = Vec2(3., 4.);
        assert!((v.norm() - 5.0).abs() < 1e-6);
    }

    #[test]
    fn equatorial_ra0_dec0_gives_x_axis() {
        let eq = Equatorial { ra: 0.0, dec: 0.0 };
        let v = eq.to_cartesian();
        assert!((v.0 - 1.0).abs() < 1e-6);
        assert!(v.1.abs() < 1e-6);
        assert!(v.2.abs() < 1e-6);
    }

    #[test]
    fn equatorial_north_pole_gives_z_axis() {
        let eq = Equatorial {
            ra: 0.0,
            dec: std::f32::consts::FRAC_PI_2,
        };
        let v = eq.to_cartesian();
        assert!(v.0.abs() < 1e-6);
        assert!(v.1.abs() < 1e-6);
        assert!((v.2 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn equatorial_to_cartesian_is_unit_length() {
        let eq = Equatorial {
            ra: 1.23,
            dec: 0.45,
        };
        let v = eq.to_cartesian();
        assert!((v.norm() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn fixed10_roundtrip() {
        for &val in &[0.0f32, 1.0, -1.0, 0.5, -0.5, 0.123, -0.789] {
            let fixed = float_to_fixed10(val);
            let back = fixed10_to_float(fixed);
            assert!((back - val).abs() < 0.003, "val={val}, back={back}");
        }
    }

    #[test]
    fn fixed10_clamps_to_range() {
        assert_eq!(float_to_fixed10(2.0), float_to_fixed10(1.0));
        assert_eq!(float_to_fixed10(-2.0), float_to_fixed10(-1.0));
    }

    #[test]
    fn pattern_compressed_roundtrip() {
        let ref_dir = Vec3(0.5, -0.3, 0.8);
        let sky_centroid = Vec3(-0.2, 0.7, -0.1);

        let compressed = (float_to_fixed10(ref_dir.0) as u64)
            | ((float_to_fixed10(ref_dir.1) as u64) << 10)
            | ((float_to_fixed10(ref_dir.2) as u64) << 20)
            | ((float_to_fixed10(sky_centroid.0) as u64) << 30)
            | ((float_to_fixed10(sky_centroid.1) as u64) << 40)
            | ((float_to_fixed10(sky_centroid.2) as u64) << 50);

        let p = Pattern::from_compressed(compressed);

        assert!((p.ref_dir.0 - ref_dir.0).abs() < 0.003);
        assert!((p.ref_dir.1 - ref_dir.1).abs() < 0.003);
        assert!((p.ref_dir.2 - ref_dir.2).abs() < 0.003);
        assert!((p.sky_centroid.0 - sky_centroid.0).abs() < 0.003);
        assert!((p.sky_centroid.1 - sky_centroid.1).abs() < 0.003);
        assert!((p.sky_centroid.2 - sky_centroid.2).abs() < 0.003);
    }

    #[test]
    fn pattern_perp_dir_perpendicular_to_ref_dir() {
        let ref_dir = Vec3(0.6, 0.0, 0.0);
        let sky_centroid = Vec3(0.0, 0.0, 1.0);
        let compressed = (float_to_fixed10(ref_dir.0) as u64)
            | ((float_to_fixed10(sky_centroid.2) as u64) << 50);
        let p = Pattern::from_compressed(compressed);
        let perp = p.perp_dir();
        let dot = perp.dot(&p.ref_dir);
        assert!(
            dot.abs() < 0.01,
            "perp_dir should be perpendicular to ref_dir, dot={dot}"
        );
    }

    #[test]
    fn sort6_produces_sorted_array() {
        let mut arr = [
            (0.9f32, 0, 1),
            (0.3, 0, 2),
            (0.7, 0, 3),
            (0.1, 1, 2),
            (0.5, 1, 3),
            (0.4, 2, 3),
        ];
        sort6(&mut arr);
        for w in arr.windows(2) {
            assert!(w[0].0 <= w[1].0, "not sorted: {arr:?}");
        }
    }

    #[test]
    fn sort6_already_sorted() {
        let mut arr = [
            (0.1f32, 0, 1),
            (0.2, 0, 2),
            (0.3, 0, 3),
            (0.4, 1, 2),
            (0.5, 1, 3),
            (0.6, 2, 3),
        ];
        sort6(&mut arr);
        for w in arr.windows(2) {
            assert!(w[0].0 <= w[1].0);
        }
    }

    #[test]
    fn sort6_reverse_sorted() {
        let mut arr = [
            (0.6f32, 0, 1),
            (0.5, 0, 2),
            (0.4, 0, 3),
            (0.3, 1, 2),
            (0.2, 1, 3),
            (0.1, 2, 3),
        ];
        sort6(&mut arr);
        for w in arr.windows(2) {
            assert!(w[0].0 <= w[1].0);
        }
    }

    #[test]
    fn equatorial_roundtrip() {
        let original = Equatorial {
            ra: 1.23,
            dec: 0.45,
        };
        let v = original.to_cartesian();
        let recovered = Equatorial::from_cartesian(v);
        assert!((recovered.ra - original.ra).abs() < 1e-5);
        assert!((recovered.dec - original.dec).abs() < 1e-5);
    }

    #[test]
    fn equatorial_from_cartesian_x_axis() {
        let eq = Equatorial::from_cartesian(Vec3(1.0, 0.0, 0.0));
        assert!(eq.ra.abs() < 1e-5);
        assert!(eq.dec.abs() < 1e-5);
    }

    #[test]
    fn equatorial_from_cartesian_north_pole() {
        let eq = Equatorial::from_cartesian(Vec3(0.0, 0.0, 1.0));
        assert!((eq.dec - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
    }

    #[test]
    fn to_wcs_north_up_cd_matrix() {
        let sol = AttitudeSolution {
            pointing: Vec3(1.0, 0.0, 0.0),
            roll: 0.0,
            votes: 1,
        };
        let wcs = sol.to_wcs(0.1);
        assert!((wcs.cd[0][0] + 0.1).abs() < 1e-6, "CD1_1={}", wcs.cd[0][0]);
        assert!(wcs.cd[0][1].abs() < 1e-6, "CD1_2={}", wcs.cd[0][1]);
        assert!(wcs.cd[1][0].abs() < 1e-6, "CD2_1={}", wcs.cd[1][0]);
        assert!((wcs.cd[1][1] - 0.1).abs() < 1e-6, "CD2_2={}", wcs.cd[1][1]);
    }

    #[test]
    fn to_wcs_roll_90_cd_matrix() {
        let sol = AttitudeSolution {
            pointing: Vec3(1.0, 0.0, 0.0),
            roll: std::f32::consts::FRAC_PI_2,
            votes: 1,
        };
        let wcs = sol.to_wcs(0.1);
        assert!(wcs.cd[0][0].abs() < 1e-6, "CD1_1={}", wcs.cd[0][0]);
        assert!((wcs.cd[0][1] - 0.1).abs() < 1e-6, "CD1_2={}", wcs.cd[0][1]);
        assert!((wcs.cd[1][0] - 0.1).abs() < 1e-6, "CD2_1={}", wcs.cd[1][0]);
        assert!(wcs.cd[1][1].abs() < 1e-6, "CD2_2={}", wcs.cd[1][1]);
    }

    #[test]
    fn to_wcs_pixel_scale_roundtrip() {
        let scale = 0.05_f32;
        let sol = AttitudeSolution {
            pointing: Vec3(1.0, 0.0, 0.0),
            roll: 0.7,
            votes: 1,
        };
        let wcs = sol.to_wcs(scale);
        assert!((wcs.pixel_scale_deg() - scale).abs() < 1e-6);
    }

    #[test]
    fn to_wcs_crota2_roundtrip() {
        let roll_deg = 37.0_f32;
        let sol = AttitudeSolution {
            pointing: Vec3(1.0, 0.0, 0.0),
            roll: roll_deg.to_radians(),
            votes: 1,
        };
        let wcs = sol.to_wcs(0.1);
        assert!(
            (wcs.crota2_deg() - roll_deg).abs() < 1e-4,
            "crota2={}",
            wcs.crota2_deg()
        );
    }

    #[test]
    fn to_wcs_radec_from_pointing() {
        let ra_rad = 45_f32.to_radians();
        let dec_rad = 30_f32.to_radians();
        let pointing = Vec3(
            dec_rad.cos() * ra_rad.cos(),
            dec_rad.cos() * ra_rad.sin(),
            dec_rad.sin(),
        );
        let sol = AttitudeSolution {
            pointing,
            roll: 0.0,
            votes: 1,
        };
        let wcs = sol.to_wcs(0.1);
        assert!((wcs.ra_deg - 45.0).abs() < 1e-3, "ra={}", wcs.ra_deg);
        assert!((wcs.dec_deg - 30.0).abs() < 1e-3, "dec={}", wcs.dec_deg);
    }

    #[test]
    fn compute_roll_north_up_is_zero() {
        let pointing = Vec3(1.0, 0.0, 0.0);
        let roll = compute_roll(pointing, Vec2(0.0, 1.0), Vec3(0.0, 0.0, 1.0));
        assert!(roll.abs() < 1e-5, "roll={roll}");
    }

    #[test]
    fn compute_roll_90_degrees_cw() {
        let pointing = Vec3(1.0, 0.0, 0.0);
        let roll = compute_roll(pointing, Vec2(1.0, 0.0), Vec3(0.0, 0.0, 1.0));
        assert!(
            (roll - std::f32::consts::FRAC_PI_2).abs() < 1e-5,
            "roll={roll}"
        );
    }

    #[test]
    fn compute_roll_90_degrees_ccw() {
        let pointing = Vec3(1.0, 0.0, 0.0);
        let roll = compute_roll(pointing, Vec2(-1.0, 0.0), Vec3(0.0, 0.0, 1.0));
        assert!(
            (roll + std::f32::consts::FRAC_PI_2).abs() < 1e-5,
            "roll={roll}"
        );
    }

    #[test]
    fn fast_asin_accuracy_small_angles() {
        for i in 0..=10 {
            let x = i as f32 * 0.03;
            let approx = fast_asin(x);
            let exact = x.asin();
            assert!(
                (approx - exact).abs() < 0.005,
                "x={x}, approx={approx}, exact={exact}"
            );
        }
    }
}
