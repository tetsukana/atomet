//! Reference plate solution and FITS WCS headers.
//!
//! The camera is assumed fixed, so one calibration serves every block: the
//! reference attitude advanced by the Earth's rotation (`Rz(ω·Δt)`) gives the
//! pointing at any time. The solver only runs to (re)build that reference —
//! at first, and again once a day — and never replaces it with anything that
//! was not validated. Clouds / failed solves just leave the old reference in
//! place.
//!
//! A new reference is the mean of several validated solves that agree with
//! each other, so one bad solve cannot move it.

use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};
use solver::calib::{LensModel, Pose, Shape};

use crate::{fits, paths};

/// Sidereal day in seconds (Earth's rotation period relative to the stars).
const SIDEREAL_DAY_S: f64 = 86164.0905;

/// Re-verify the reference this often (seconds since the last accepted one).
const VERIFY_INTERVAL_S: f64 = 24.0 * 3600.0;
/// Agreeing validated solves averaged into a new reference.
pub const N_SAMPLES: usize = 5;
/// Samples must agree to this (pointing and roll, degrees) to be averaged;
/// a disagreeing one restarts the collection.
const AGREE_DEG: f64 = 0.3;
/// A collection older than this is discarded.
const SAMPLE_WINDOW_S: f64 = 3600.0;
/// Blocks tried back to back per attempt. The star tracker needs a few
/// consecutive blocks to confirm stars, so attempts are bursts, not single blocks.
const BURST_BLOCKS: u32 = 8;
/// After a burst with no progress, wait this long; doubles per failed burst
/// up to the maximum.
const BACKOFF_BASE_S: f64 = 120.0;
const BACKOFF_MAX_S: f64 = 3600.0;

/// Set by the web UI to throw the reference away and calibrate again
/// (e.g. after moving the camera). Consumed by the FF task.
pub static RECALIBRATE: AtomicBool = AtomicBool::new(false);

/// The reference on the SD card (`paths::SOLVE_LOCK`).
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Saved {
    /// Unix time the attitude refers to.
    pub t: f64,
    /// Unix time the reference was last accepted.
    pub verified: f64,
    pub pointing: [f64; 3],
    pub roll: f64,
    pub f: f64,
    /// Poly lens coefficients; both absent for a pinhole lens.
    pub c3: Option<f64>,
    pub c5: Option<f64>,
}

impl Saved {
    fn new(pose: &Pose, lens: &LensModel, t: f64, verified: f64) -> Self {
        let (c3, c5) = match lens.shape {
            Shape::Poly { c3, c5 } => (Some(c3), Some(c5)),
            Shape::Pinhole => (None, None),
        };
        Saved { t, verified, pointing: pose.pointing, roll: pose.roll, f: lens.f, c3, c5 }
    }

    fn pose(&self) -> Pose {
        Pose { pointing: self.pointing, roll: self.roll }
    }

    fn lens(&self) -> LensModel {
        match (self.c3, self.c5) {
            (Some(c3), Some(c5)) => LensModel { f: self.f, shape: Shape::Poly { c3, c5 } },
            _ => LensModel::pinhole(self.f),
        }
    }

    fn sane(&self) -> bool {
        let n = self.pointing.iter().map(|v| v * v).sum::<f64>().sqrt();
        self.t.is_finite()
            && self.verified.is_finite()
            && self.roll.is_finite()
            && self.f.is_finite()
            && self.f > 0.0
            && (n - 1.0).abs() < 1e-3
            && self.c3.is_none_or(f64::is_finite)
            && self.c5.is_none_or(f64::is_finite)
    }
}

/// Write the reference atomically.
pub fn save(s: &Saved) -> std::io::Result<()> {
    let json = serde_json::to_vec_pretty(s).map_err(std::io::Error::other)?;
    fits::atomic_write(std::path::Path::new(paths::SOLVE_LOCK), &json)
}

/// Delete the stored reference (recalibration request).
pub fn forget() {
    let _ = std::fs::remove_file(paths::SOLVE_LOCK);
}

/// What changed when a sample completed a new reference.
pub struct Update {
    pub saved: Saved,
    /// Angle between the previous reference (advanced to now) and the new
    /// one, degrees. `None` if there was no previous reference.
    pub moved_deg: Option<f64>,
}

#[derive(Default)]
pub struct Reference {
    current: Option<Saved>,
    samples: Vec<(Pose, LensModel, f64)>,
    /// Blocks left in the running attempt burst.
    burst_left: u32,
    /// No new burst before this unix time.
    retry_after: f64,
    /// Bursts in a row that made no progress.
    fails: u32,
    /// How far the last completed reference moved from the one before, degrees.
    last_moved: Option<f64>,
}

impl Reference {
    /// Start from the reference on the SD card, if usable. `now` is the current
    /// unix time; a stored reference from the future means the clock is wrong
    /// (NTP not synced), so it is ignored.
    pub fn load(now: f64) -> Self {
        let mut st = Self::default();
        let Ok(bytes) = std::fs::read(paths::SOLVE_LOCK) else {
            log::info!("No saved solve at {}", paths::SOLVE_LOCK);
            return st;
        };
        match serde_json::from_slice::<Saved>(&bytes) {
            Ok(s) if s.sane() && s.verified <= now && s.t <= now => {
                log::info!(
                    "Loaded reference solve, verified {:.1} h ago (f={:.1})",
                    (now - s.verified) / 3600.0,
                    s.f
                );
                st.current = Some(s);
            }
            Ok(_) => log::warn!("Ignoring saved solve: invalid or clock behind"),
            Err(e) => log::warn!("Ignoring unreadable {}: {}", paths::SOLVE_LOCK, e),
        }
        st
    }

    /// Lens of the loaded reference (seeds the solver's lens prior after a restart).
    pub fn lens(&self) -> Option<LensModel> {
        self.current.as_ref().map(Saved::lens)
    }

    /// Drop the reference and any collected samples.
    pub fn reset(&mut self) {
        self.current = None;
        self.samples.clear();
        self.burst_left = 0;
        self.retry_after = 0.0;
        self.fails = 0;
        self.last_moved = None;
    }

    /// Status for the UI, as a `solver_status` WebSocket message.
    pub fn status_json(&self, now: f64, enabled: bool, in_schedule: bool) -> String {
        let age = self.current.as_ref().map(|s| (now - s.verified).max(0.0));
        let retry_in = (self.due(now) && self.burst_left == 0 && now < self.retry_after)
            .then_some(self.retry_after - now);
        // The locked reference advanced to `now`, so the UI can draw the sky
        // overlay without a running solver. Same units as the `stars` event's
        // `solve` (degrees); the lens omits c3/c5 for a pinhole.
        let pose = self.at(now).map(|(p, l, _)| {
            let [x, y, z] = p.pointing;
            let mut lens = serde_json::json!({ "f": l.f });
            if let Shape::Poly { c3, c5 } = l.shape {
                lens["c3"] = c3.into();
                lens["c5"] = c5.into();
            }
            serde_json::json!({
                "ra": y.atan2(x).to_degrees().rem_euclid(360.0),
                "dec": z.clamp(-1.0, 1.0).asin().to_degrees(),
                "roll": p.roll.to_degrees(),
                "lens": lens,
            })
        });
        serde_json::json!({
            "type": "solver_status",
            "pose": pose,
            "enabled": enabled,
            "in_schedule": in_schedule,
            "has_ref": self.current.is_some(),
            "age": age,
            "due": self.due(now),
            "samples": self.samples.len(),
            "need": N_SAMPLES,
            "trying": self.burst_left > 0,
            "retry_in": retry_in,
            "moved": self.last_moved,
        })
        .to_string()
    }

    /// No reference yet, or it is due for re-verification.
    fn due(&self, now: f64) -> bool {
        match &self.current {
            None => true,
            Some(s) => now - s.verified >= VERIFY_INTERVAL_S,
        }
    }

    /// Call once per block (while the solver is on and in its schedule):
    /// should this block be extracted and solved?
    ///
    /// Attempts come in bursts of `BURST_BLOCKS` consecutive blocks. A burst
    /// that ends without progress is followed by a wait that doubles each
    /// time, up to `BACKOFF_MAX_S`. A validated solve counts as progress and
    /// keeps the burst going; a finished reference ends the attempts until the
    /// next verification is due.
    pub fn should_try(&mut self, now: f64) -> bool {
        if !self.due(now) {
            self.burst_left = 0;
            return false;
        }
        if self.burst_left == 0 {
            if now < self.retry_after {
                return false;
            }
            self.burst_left = BURST_BLOCKS;
        }
        self.burst_left -= 1;
        if self.burst_left == 0 {
            let delay = (BACKOFF_BASE_S * 2f64.powi(self.fails.min(16) as i32)).min(BACKOFF_MAX_S);
            self.retry_after = now + delay;
            self.fails += 1;
        }
        true
    }

    /// Attitude, lens and seconds since verification at unix time `t`.
    pub fn at(&self, t: f64) -> Option<(Pose, LensModel, f64)> {
        let s = self.current.as_ref()?;
        if t < s.t {
            return None; // clock went backwards
        }
        Some((advance_pose(&s.pose(), t - s.t), s.lens(), (t - s.verified).max(0.0)))
    }

    /// Offer a validated solve made at unix time `t`. Returns the new
    /// reference once enough agreeing solves have been collected.
    pub fn add_sample(&mut self, pose: Pose, lens: LensModel, t: f64) -> Option<Update> {
        // Stale collection (long gap, e.g. clouds): start over.
        if self.samples.first().is_some_and(|s| t - s.2 > SAMPLE_WINDOW_S) {
            self.samples.clear();
        }
        // Disagreeing sample (outlier, or the camera moved): restart from it.
        if let Some(first) = self.samples.first() {
            let mean = mean_pose(&self.samples, first.2);
            let q = advance_pose(&pose, first.2 - t);
            if angle_deg(&mean.pointing, &q.pointing) > AGREE_DEG
                || wrap_deg((mean.roll - q.roll).to_degrees()).abs() > AGREE_DEG
            {
                self.samples.clear();
            }
        }
        self.samples.push((pose, lens, t));
        // Progress: keep trying without backing off.
        self.burst_left = BURST_BLOCKS;
        self.fails = 0;
        if self.samples.len() < N_SAMPLES {
            return None;
        }
        self.burst_left = 0;
        self.retry_after = 0.0;

        let t_ref = self.samples[0].2;
        let mean = mean_pose(&self.samples, t_ref);
        let lens = mean_lens(&self.samples);
        self.samples.clear();

        let moved_deg = self.current.as_ref().map(|old| {
            let prev = advance_pose(&old.pose(), t_ref - old.t);
            angle_deg(&prev.pointing, &mean.pointing)
        });
        let saved = Saved::new(&mean, &lens, t_ref, t);
        self.last_moved = moved_deg;
        self.current = Some(saved.clone());
        Some(Update { saved, moved_deg })
    }
}

/// Mean attitude of the samples, all advanced to `t_ref` (roll: circular mean).
fn mean_pose(samples: &[(Pose, LensModel, f64)], t_ref: f64) -> Pose {
    let (mut p, mut s, mut c) = ([0.0f64; 3], 0.0f64, 0.0f64);
    for (pose, _, t) in samples {
        let q = advance_pose(pose, t_ref - t);
        for (acc, v) in p.iter_mut().zip(q.pointing) {
            *acc += v;
        }
        s += pose.roll.sin();
        c += pose.roll.cos();
    }
    let n = p.iter().map(|v| v * v).sum::<f64>().sqrt();
    Pose { pointing: [p[0] / n, p[1] / n, p[2] / n], roll: s.atan2(c) }
}

/// Mean lens: coefficient-wise if every sample is a polynomial profile,
/// otherwise the latest one.
fn mean_lens(samples: &[(Pose, LensModel, f64)]) -> LensModel {
    let n = samples.len() as f64;
    let polys: Vec<(f64, f64, f64)> = samples
        .iter()
        .filter_map(|(_, l, _)| match l.shape {
            Shape::Poly { c3, c5 } => Some((l.f, c3, c5)),
            Shape::Pinhole => None,
        })
        .collect();
    if polys.len() == samples.len() {
        let f = polys.iter().map(|p| p.0).sum::<f64>() / n;
        let c3 = polys.iter().map(|p| p.1).sum::<f64>() / n;
        let c5 = polys.iter().map(|p| p.2).sum::<f64>() / n;
        LensModel { f, shape: Shape::Poly { c3, c5 } }
    } else {
        samples[samples.len() - 1].1
    }
}

fn angle_deg(a: &[f64; 3], b: &[f64; 3]) -> f64 {
    let d: f64 = (0..3).map(|i| a[i] * b[i]).sum();
    d.clamp(-1.0, 1.0).acos().to_degrees()
}

fn wrap_deg(a: f64) -> f64 {
    (a + 180.0).rem_euclid(360.0) - 180.0
}

/// Rotate an attitude about the celestial pole by `dt` seconds of Earth
/// rotation. The sky frame (north/east at the pointing) rotates rigidly with
/// the pointing, so the roll is unchanged.
pub fn advance_pose(p: &Pose, dt_s: f64) -> Pose {
    let a = std::f64::consts::TAU * dt_s / SIDEREAL_DAY_S;
    let (s, c) = a.sin_cos();
    let [x, y, z] = p.pointing;
    Pose { pointing: [c * x - s * y, s * x + c * y, z], roll: p.roll }
}

fn card_f(key: &str, value: f64) -> Vec<u8> {
    fits::make_card(&format!("{:<8}= {:>20.12E}", key, value))
}

/// WCS cards for a 2-D image whose array indices are the pixel coordinates
/// (FITS pixel = index + 1). The image center is the lens axis. `age_s` is
/// the time since the reference was last verified (`WCSAGE`).
///
/// The lens `r = f·(θ + c3θ³ + c5θ⁵)` maps directly onto ZPN
/// (`R = Σ PV2_m·ζᵐ`, ζ = angle from the axis) with `CDELT = (180/π)/f`.
/// A pinhole lens is written as TAN.
pub fn wcs_cards(pose: &Pose, lens: &LensModel, width: u32, height: u32, age_s: f64) -> Vec<Vec<u8>> {
    let [x, y, z] = pose.pointing;
    let ra = y.atan2(x).to_degrees().rem_euclid(360.0);
    let dec = z.clamp(-1.0, 1.0).asin().to_degrees();
    let scale = (180.0 / std::f64::consts::PI) / lens.f; // deg per pixel
    let (s, c) = pose.roll.sin_cos();

    // Model: px = cosφ·E + sinφ·N, py = -sinφ·E + cosφ·N for roll φ, with
    // (E, N) the east/north tangent offsets in pixels; invert for CD.
    let cd = [[scale * c, -scale * s], [scale * s, scale * c]];
    let proj = match lens.shape {
        Shape::Poly { .. } => "ZPN",
        Shape::Pinhole => "TAN",
    };

    let mut v = vec![
        fits::card_int("WCSAXES", 2),
        fits::card_str("CTYPE1", &format!("RA---{proj}")),
        fits::card_str("CTYPE2", &format!("DEC--{proj}")),
        fits::card_str("CUNIT1", "deg"),
        fits::card_str("CUNIT2", "deg"),
        card_f("CRVAL1", ra),
        card_f("CRVAL2", dec),
        card_f("CRPIX1", width as f64 / 2.0 + 1.0),
        card_f("CRPIX2", height as f64 / 2.0 + 1.0),
        card_f("CD1_1", cd[0][0]),
        card_f("CD1_2", cd[0][1]),
        card_f("CD2_1", cd[1][0]),
        card_f("CD2_2", cd[1][1]),
        card_f("LONPOLE", 180.0),
        fits::card_str("RADESYS", "ICRS"),
        card_f("EQUINOX", 2000.0),
    ];
    if let Shape::Poly { c3, c5 } = lens.shape {
        v.push(card_f("PV2_1", 1.0));
        v.push(card_f("PV2_3", c3));
        v.push(card_f("PV2_5", c5));
    }
    v.push(card_f("WCSAGE", age_s));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> Pose {
        Pose { pointing: [1.0, 0.0, 0.0], roll: 0.2 }
    }

    fn lens() -> LensModel {
        LensModel { f: 330.0, shape: Shape::Poly { c3: 0.05, c5: 0.0 } }
    }

    /// Sample of the same fixed camera, as the solver would see it at `t`.
    fn at(t: f64) -> (Pose, LensModel, f64) {
        (advance_pose(&p(), t), lens(), t)
    }

    #[test]
    fn advance_is_a_pole_rotation() {
        let q = advance_pose(&Pose { pointing: [1.0, 0.0, 0.0], roll: 0.3 }, SIDEREAL_DAY_S / 4.0);
        assert!((q.pointing[1] - 1.0).abs() < 1e-12); // RA 0° → 90°
        assert_eq!(q.roll, 0.3);
    }

    #[test]
    fn needs_solve_until_reference_then_daily() {
        let mut r = Reference::default();
        assert!(r.due(0.0));
        let mut done = None;
        for i in 0..N_SAMPLES {
            let (pose, l, t) = at(60.0 * i as f64);
            assert!(done.is_none());
            done = r.add_sample(pose, l, t);
        }
        let u = done.expect("reference after N samples");
        assert!(u.moved_deg.is_none());
        let t_ok = 60.0 * N_SAMPLES as f64;
        assert!(!r.due(t_ok + 3600.0));
        assert!(r.due(t_ok + VERIFY_INTERVAL_S));
        // the reference follows the sky
        let (pose, _, age) = r.at(1.0e5).unwrap();
        let want = advance_pose(&p(), 1.0e5);
        assert!(angle_deg(&pose.pointing, &want.pointing) < 1e-6);
        assert!(age > 0.0);
    }

    #[test]
    fn outlier_restarts_collection_and_move_is_reported() {
        let mut r = Reference::default();
        for i in 0..N_SAMPLES {
            let (pose, l, t) = at(60.0 * i as f64);
            r.add_sample(pose, l, t);
        }
        // 1 day later: 4 good + 1 wild sample in the middle → still needs 5 agreeing
        let t0 = VERIFY_INTERVAL_S + 1000.0;
        let mut got = None;
        for i in 0..3 {
            let (pose, l, t) = at(t0 + 60.0 * i as f64);
            got = r.add_sample(pose, l, t);
        }
        let mut wild = advance_pose(&p(), t0 + 180.0);
        wild.pointing = [0.8, 0.6, 0.0];
        let n = (wild.pointing.iter().map(|v| v * v).sum::<f64>()).sqrt();
        for v in wild.pointing.iter_mut() {
            *v /= n;
        }
        assert!(r.add_sample(wild, lens(), t0 + 180.0).is_none());
        assert!(got.is_none());
        // the wild one became the new seed; four more of it (a moved camera)
        for i in 1..N_SAMPLES {
            let t = t0 + 180.0 + 60.0 * i as f64;
            let mut w = advance_pose(&wild, 60.0 * i as f64);
            w.roll = wild.roll;
            got = r.add_sample(w, lens(), t);
        }
        let u = got.expect("new reference from moved camera");
        assert!(u.moved_deg.unwrap() > 1.0);
    }

    #[test]
    fn failed_bursts_back_off_up_to_an_hour() {
        let mut r = Reference::default();
        let (mut t, mut starts, mut last_start) = (0.0f64, Vec::new(), f64::NAN);
        // Ten-second blocks, nothing ever solves.
        while t < 6.0 * 3600.0 {
            let was_idle = r.burst_left == 0;
            if r.should_try(t) && was_idle {
                if !last_start.is_nan() {
                    starts.push(t - last_start);
                }
                last_start = t;
            }
            t += 10.0;
        }
        // gaps between burst starts grow, never beyond an hour (+ the burst itself)
        assert!(starts.windows(2).all(|w| w[1] >= w[0] - 1.0), "{starts:?}");
        let max = starts.iter().cloned().fold(0.0, f64::max);
        assert!(max <= BACKOFF_MAX_S + BURST_BLOCKS as f64 * 10.0 + 1.0, "{max}");
        assert!(max >= BACKOFF_MAX_S, "{starts:?}");
        // a burst is BURST_BLOCKS consecutive blocks
        let mut r = Reference::default();
        assert!((0..BURST_BLOCKS).all(|i| r.should_try(10.0 * i as f64)));
        assert!(!r.should_try(10.0 * BURST_BLOCKS as f64));
        // a recalibration request tries again at once
        r.reset();
        assert!(r.should_try(10.0 * BURST_BLOCKS as f64));
    }

    #[test]
    fn cards_ok() {
        let pose = Pose { pointing: [0.5f64.sqrt(), 0.5f64.sqrt(), 0.0], roll: 0.4 };
        for card in wcs_cards(&pose, &lens(), 640, 360, 1234.0) {
            assert_eq!(card.len(), 80);
        }
        let c = wcs_cards(&pose, &LensModel::pinhole(344.0), 640, 360, 0.0);
        assert!(String::from_utf8_lossy(&c[1]).contains("RA---TAN"));
    }
}
