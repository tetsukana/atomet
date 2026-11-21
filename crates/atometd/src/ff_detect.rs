//! FF second-pass meteor detection on a finalized 256-frame block.
//!
//! Pure computation (no device, async or crate dependencies) so the very same code runs in the
//! daemon (`ff.rs`) and in the host-side re-detection tool (`crates/solver/examples/ff_redetect.rs`,
//! which includes this file with `#[path]`). Keep it free of `crate::` imports.

use std::time::SystemTime;

/// Frame size of an FF block (equals `luma::WIDTH`/`HEIGHT`).
pub const WIDTH: usize = 640;
pub const HEIGHT: usize = 360;

/// A finalized FF (Four-Field) block: 4 derived images over a 256-frame window.
///
/// `maxpixel`/`maxframe` together let you reconstruct the brightest event at
/// every pixel and *when* it occurred (frame index 0..=255).
/// `avepixel`/`stdpixel` are the per-pixel temporal mean and stddev — used
/// downstream for σ-clip detection: `maxpixel > avepixel + K·stdpixel`.
/// Camera auto-exposure state when the block ended (gain in 0.1 dB, exposure in µs). Noise follows gain, so the
/// sidecar/FITS record it; `age_s` is how old the sample was (the ISP task samples at 4 Hz).
#[derive(Clone, Copy, Debug)]
pub struct AeInfo {
    pub ae_mode: u32,
    pub it_lines: u32,
    pub it_us: u32,
    pub ag_db10: u32,
    pub sdg_db10: u32,
    pub idg_db10: u32,
    pub total_db10: u32,
    pub age_s: f32,
}

pub struct FfBlock {
    pub maxpixel: Vec<u8>,
    pub maxframe: Vec<u8>,
    pub avepixel: Vec<u8>,
    pub stdpixel: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub start_ts: SystemTime,
    pub n_frames: u32,
    /// Measured time from the first to the last frame of the block, seconds
    /// (0 = unknown). Covers `n_frames - 1` frame intervals.
    pub duration_s: f64,
    /// Measured frames per second of the frames that were stacked,
    /// `(n_frames - 1) / duration_s` (0 = unknown). This is the rate that
    /// matters for `maxframe` (a stacked frame is one step in time).
    pub fps_eff: f64,
    /// Frames the driver produced but the stacker never saw (from the
    /// driver's frame counter; 0 if the counter was not usable).
    pub dropped: u32,
    /// The configured fps changed while this block was being stacked, so its
    /// time axis is not uniform: skip time-based detection on it.
    pub mixed_fps: bool,
    /// Configured fps at the end of the block.
    pub cfg_fps: u32,
    /// Where `duration_s` came from: "hw" (driver timestamps) or "wall".
    pub timing_src: &'static str,
    /// Auto-exposure state at the end of the block (`None` = unknown / host tool).
    pub ae: Option<AeInfo>,
}

// σ-clip threshold multiplier: pixel flagged when maxpixel > avepixel + K * stdpixel
const SIGMA_K: f32 = 5.0;
// ============================================================
// FF second-pass meteor detection
// ============================================================
//
// σ-clip flags pixels where maxpixel ≫ avepixel over the 256-frame block;
// a meteor leaves a *line* of flagged pixels whose maxframe (the frame
// index where each pixel peaked) advances monotonically along the line.
// Integrating over the whole block makes this far more sensitive than the
// per-frame realtime path. Discriminators:
//   - stars / noise: no linear spatial structure
//   - lightning / illumination steps: everything peaks at the same frame
//     (no time gradient) or floods the pixel cap
//   - satellites / aircraft: linear with a time gradient, but slow
//     (low px/frame along the line)

/// Smallest cluster (flagged pixels) any tier looks at.
const FF_DET_MIN_CLUSTER_PIXELS: usize = 8;
/// Excess floors (ADU) the σ-clip is run at. `stdpixel` is uint8-quantised, so
/// `5σ` alone is ≈ noise level on a dark/cloudy sky and flags thousands of
/// pixels; a real meteor streak is far brighter than this. The strict levels
/// give clean clusters; the lower ones find fainter streaks but their extra
/// flagged pixels blur a bright streak's shape, so all are run and the best
/// result per place is kept.
///
/// The top level (40) matters on a dark, high-gain night: with 6-12% of the pixels above 12-15 the
/// noise fuses with a streak into one blob, and on 2026-11-19/20 the 15/12 pair found only 3 of 17
/// clear streaks in tier A (all 17 once 40 and 20 were added; 19/19 on the quiet night of 11-14).
const FF_EXCESS_LEVELS: [f32; 3] = [40.0, 20.0, 12.0];
/// More flagged pixels *in total* than this = lightning / illumination step
/// (a flood no streak analysis can untangle). With the excess floor, clouds
/// and noise flag only a few hundred pixels.
const FF_DET_MAX_PIXELS: usize = 20_000;
/// Keep at most this many clusters (best tier, then largest).
const FF_DET_MAX_METEORS: usize = 8;
/// Frame rate the speed floors below were set at; scaled to the configured fps
/// in `tier_of` so they stay the same in px/s.
pub const FF_REF_FPS: f32 = 25.0;
/// Detections whose centres are within this many px (both axes) are the same
/// event seen at different excess levels / tiers.
const FF_DET_MERGE_PX: f32 = 15.0;

/// Confidence of an FF detection. `A` is the original detector (what a
/// sidecar without a tier means); `B` and `C` accept progressively fainter /
/// shorter / less time-ordered streaks and carry a lower confidence. Order:
/// `A < B < C` (best first).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FfTier {
    A,
    B,
    C,
}

impl FfTier {
    pub fn as_str(self) -> &'static str {
        match self {
            FfTier::A => "A",
            FfTier::B => "B",
            FfTier::C => "C",
        }
    }
}

/// Gates of one tier. A cluster must pass every gate; the first (best) tier it
/// passes is its tier.
#[derive(Clone, Copy, Debug)]
pub struct TierGate {
    pub tier: FfTier,
    /// Lowest σ-clip excess level whose clusters this tier accepts.
    pub min_excess: f32,
    /// Minimum flagged pixels in the cluster *and* in the whole block.
    pub min_pixels: usize,
    /// Minimum λ1/λ2 of the pixel positions.
    pub min_lin: f32,
    pub min_len: f32,
    /// |Pearson r| between position along the line and maxframe.
    pub min_corr: f32,
    /// Apparent speed floor (px/frame) at FF_REF_FPS. Satellites take the whole
    /// block (~10 s) to cross and fall well below this.
    pub min_speed: f32,
    /// Reject lines whose ends come within this many px of the image border
    /// (sensor-edge noise); 0 = no border rule.
    pub edge: f32,
    /// Reject when the mean stdpixel within FF_BAND_PX of the line exceeds this
    /// (cloud edge). f32::INFINITY = no cloud rule.
    pub max_band_std: f32,
    /// Longest maxframe span (frames) this tier accepts; f32::INFINITY = only the global cap.
    pub max_frames: f32,
    /// A cluster spanning at most `short_frames` frames (a 1-4 frame streak) must be at least `short_min_len`
    /// px long to pass this tier. 0 = no such rule. Short, small blobs are where noise and meteors overlap.
    pub short_frames: f32,
    pub short_min_len: f32,
}

/// Longest maxframe span (frames) accepted at any tier (60 = 2.4 s at 25 fps).
const FF_MAX_SPAN_FRAMES: f32 = 60.0;
/// Half-width (px) of the band around a line used for the cloud-edge test.
const FF_BAND_PX: f32 = 20.0;
/// Cloud-edge limit for tiers B/C. One night (2026-11-18): tier-A meteors all <= 2.6,
/// tier C 61% > 3.5. Provisional; re-check on other nights.
const FF_CLOUD_BAND_STD: f32 = 3.5;

// Tier A speed floor 1.6 px/frame (was 0.8): on the dark night of 2026-11-19/20 the 32 streaks judged to be meteors
// ran 2.0-10.7 px/frame and 10 of 17 rejected blobs were slower than 1.9 (long, slow noise chains: 11-16 frames at
// ~1 px/frame); the 19 reviewed meteors of the quiet night 11-14 all stay detected (18 in tier A, 1 in B).
// Tier A min_excess 15 -> 12 (2026-11-21, artificial-meteor test docs/ff-artificial-meteor-plan.md): on the BCSH-on night 11-21
// detection of an A=14 ADU streak rose 78% -> 91% with no new blocks flagged on 11-14 (2,110 blocks) / 11-21 (3,733); no effect on the
// noisy 11-20 night (limit there is ~A=45, noise fuses with the streak).
// Tiers, best first. Tuned on 2026-11-14 00-05h (26 known meteors, 2,110 blocks):
// A = 19 detections, A+B = 22, A+B+C = 26, all with 0 false positives there.
// One night: re-check on more data before trusting the lower tiers.
pub const FF_TIERS: [TierGate; 3] = [
    TierGate { tier: FfTier::A, min_excess: 12.0, min_pixels: 12, min_lin: 5.0, min_len: 12.0, min_corr: 0.8, min_speed: 1.6, edge: 0.0, max_band_std: f32::INFINITY, max_frames: f32::INFINITY, short_frames: 0.0, short_min_len: 0.0 },
    TierGate { tier: FfTier::B, min_excess: 12.0, min_pixels: 20, min_lin: 5.0, min_len: 12.0, min_corr: 0.7, min_speed: 0.8, edge: 8.0, max_band_std: FF_CLOUD_BAND_STD, max_frames: f32::INFINITY, short_frames: 0.0, short_min_len: 0.0 },
    TierGate { tier: FfTier::C, min_excess: 12.0, min_pixels: 16, min_lin: 3.0, min_len: 8.0, min_corr: 0.5, min_speed: 1.5, edge: 8.0, max_band_std: FF_CLOUD_BAND_STD, max_frames: f32::INFINITY, short_frames: 0.0, short_min_len: 0.0 },
];

/// Every knob of the detector, so the host tool can sweep them. `Default` is exactly what the daemon runs.
#[derive(Clone, Debug)]
pub struct Tuning {
    /// σ-clip multiplier: flagged when `max - ave > sigma_k * stdpixel`.
    pub sigma_k: f32,
    /// Absolute excess floors (ADU) the σ-clip runs at.
    pub levels: Vec<f32>,
    pub tiers: Vec<TierGate>,
    /// Longest maxframe span accepted at any tier (frames at FF_REF_FPS).
    pub max_span: f32,
}

impl Default for Tuning {
    fn default() -> Self {
        Tuning { sigma_k: SIGMA_K, levels: FF_EXCESS_LEVELS.to_vec(), tiers: FF_TIERS.to_vec(), max_span: FF_MAX_SPAN_FRAMES }
    }
}

// Detection mask cell grid (must match detection.rs / the mask editor)
const MASK_CELL_SIZE: usize = 8;
const MASK_CELLS_X: usize = WIDTH / MASK_CELL_SIZE; // 80

/// Result of FF second-pass meteor detection.
pub struct FfMeteor {
    /// Line endpoints in 640×360 px, earliest end first.
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub length: f32,
    /// maxframe span covered by the event (frames).
    pub frames: u32,
    /// Apparent speed along the line, px/frame.
    pub speed: f32,
    /// Number of σ-clipped pixels in the cluster.
    pub pixels: usize,
    /// Pearson correlation between line position and maxframe.
    pub time_corr: f32,
    /// maxframe at the earliest / latest end of the cluster.
    pub f0: f32,
    pub f1: f32,
    /// Confidence tier (A = most confident).
    pub tier: FfTier,
    /// σ-clip excess floor (ADU) the cluster was found at.
    pub excess: f32,
    /// λ1/λ2 of the cluster's pixel positions (linearity).
    pub lin: f32,
    /// Mean stdpixel in the band around the line (cloud-edge measure; 0 if not computed).
    pub band_std: f32,
}

/// Detect meteors in a finalized FF block (see module notes above).
/// `cell_mask` is the 80×45 detection mask (non-zero = masked).
///
/// Flagged pixels are split into 8-connected clusters (after a 1 px dilation
/// that bridges gaps in a faint streak) and each cluster is judged on its own
/// (line shape, maxframe gradient, speed). Judging the whole frame at once
/// fails as soon as clouds or noise flag scattered pixels around the streak.
/// The σ-clip runs at each of `FF_EXCESS_LEVELS`; an event found more than
/// once keeps its best tier. Returns the accepted clusters, best tier first,
/// then largest.
pub fn ff_detect_meteor(block: &FfBlock, cell_mask: &[u8], fps: f32) -> Vec<FfMeteor> {
    ff_detect_meteor_with(block, cell_mask, fps, &Tuning::default())
}

/// `ff_detect_meteor` with explicit tuning (host-side experiments).
pub fn ff_detect_meteor_with(block: &FfBlock, cell_mask: &[u8], fps: f32, t: &Tuning) -> Vec<FfMeteor> {
    // Collected in level order, then stable-sorted by tier: for equal tiers the
    // strict level wins.
    let mut all: Vec<FfMeteor> = Vec::new();
    for &level in &t.levels {
        all.extend(detect_at_level(block, cell_mask, fps, level, t));
    }
    all.sort_by_key(|m| m.tier);
    let mut out: Vec<FfMeteor> = Vec::new();
    for m in all {
        let (cx, cy) = ((m.x0 + m.x1) / 2.0, (m.y0 + m.y1) / 2.0);
        let dup = out.iter().any(|o| {
            ((o.x0 + o.x1) / 2.0 - cx).abs() <= FF_DET_MERGE_PX
                && ((o.y0 + o.y1) / 2.0 - cy).abs() <= FF_DET_MERGE_PX
        });
        if !dup {
            out.push(m);
        }
    }
    out.sort_by_key(|m| (m.tier, std::cmp::Reverse(m.pixels)));
    out.truncate(FF_DET_MAX_METEORS);
    out
}

/// One σ-clip level: flag, cluster, measure, assign tiers.
fn detect_at_level(block: &FfBlock, cell_mask: &[u8], fps: f32, level: f32, t: &Tuning) -> Vec<FfMeteor> {
    let w = block.width as usize;
    let h = block.height as usize;

    // σ-clip with an absolute excess floor (outside masked cells).
    let mut flag = vec![false; w * h];
    let mut total = 0usize;
    for y in 0..h {
        let mask_row = y / MASK_CELL_SIZE * MASK_CELLS_X;
        for x in 0..w {
            if cell_mask[mask_row + x / MASK_CELL_SIZE] != 0 {
                continue;
            }
            let i = y * w + x;
            let max = block.maxpixel[i] as f32;
            let ave = block.avepixel[i] as f32;
            let std = (block.stdpixel[i] as f32).max(1.0);
            let excess = max - ave;
            if excess > t.sigma_k * std && excess > level {
                flag[i] = true;
                total += 1;
                if total > FF_DET_MAX_PIXELS {
                    return Vec::new();
                }
            }
        }
    }
    if total < FF_DET_MIN_CLUSTER_PIXELS {
        return Vec::new();
    }

    // 3×3 dilation: connectivity only; cluster members stay the flagged pixels.
    let mut grow = flag.clone();
    for y in 0..h {
        for x in 0..w {
            if !flag[y * w + x] {
                continue;
            }
            for ny in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                    grow[ny * w + nx] = true;
                }
            }
        }
    }

    // Flood-fill labelling of the dilated mask.
    let mut seen = vec![false; w * h];
    let mut stack: Vec<usize> = Vec::new();
    let mut out: Vec<FfMeteor> = Vec::new();
    for start in 0..w * h {
        if !grow[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let mut pts: Vec<(f32, f32, f32)> = Vec::new(); // (x, y, maxframe)
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            if flag[i] {
                pts.push((x as f32, y as f32, block.maxframe[i] as f32));
            }
            for ny in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                    let j = ny * w + nx;
                    if grow[j] && !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        if pts.len() >= FF_DET_MIN_CLUSTER_PIXELS
            && let Some(mut m) = cluster_metrics(&pts)
            && {
                // The band is only needed by gates that look at it, and only for
                // clusters that already pass the cheap shape/span tests.
                if m.span <= t.max_span {
                    m.band_std = band_mean_std(block, &m);
                }
                true
            }
            && let Some(tier) = tier_of(&m, fps, level, total, t)
        {
            out.push(FfMeteor {
                x0: m.x0,
                y0: m.y0,
                x1: m.x1,
                y1: m.y1,
                length: m.length,
                frames: m.span as u32,
                speed: m.speed,
                pixels: m.pixels,
                time_corr: m.corr,
                f0: m.f0,
                f1: m.f1,
                tier,
                excess: level,
                lin: m.lin,
                band_std: m.band_std,
            });
        }
    }
    out
}

/// Shape / time measurements of one cluster, before any gate is applied.
struct ClusterMetrics {
    pixels: usize,
    /// λ1/λ2 of the pixel positions.
    lin: f32,
    length: f32,
    /// Pearson r between position along the line and maxframe.
    corr: f32,
    /// maxframe span (frames), at least 1.
    span: f32,
    /// Apparent speed along the line, px/frame.
    speed: f32,
    /// Endpoints from the projection extremes, earliest end first.
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    f0: f32,
    f1: f32,
    /// Mean stdpixel around the line (filled by `detect_at_level`).
    band_std: f32,
}

/// Measure one cluster of flagged pixels `(x, y, maxframe)`: line shape,
/// maxframe gradient along the line, and apparent speed. `None` if all pixels
/// peak at the same frame (no time gradient — a flash, not a moving source).
fn cluster_metrics(pts: &[(f32, f32, f32)]) -> Option<ClusterMetrics> {
    // PCA line fit on flagged pixel positions
    let n = pts.len() as f32;
    let mx = pts.iter().map(|p| p.0).sum::<f32>() / n;
    let my = pts.iter().map(|p| p.1).sum::<f32>() / n;
    let (mut sxx, mut sxy, mut syy) = (0f32, 0f32, 0f32);
    for &(x, y, _) in pts {
        let dx = x - mx;
        let dy = y - my;
        sxx += dx * dx;
        sxy += dx * dy;
        syy += dy * dy;
    }
    sxx /= n;
    sxy /= n;
    syy /= n;

    let trace = sxx + syy;
    let det = sxx * syy - sxy * sxy;
    let disc = ((trace * trace / 4.0) - det).max(0.0).sqrt();
    let l1 = trace / 2.0 + disc;
    let l2 = (trace / 2.0 - disc).max(0.0625); // (0.5 px)² floor
    let length = (12.0 * l1).sqrt();
    let theta = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let (ux, uy) = (theta.cos(), theta.sin());

    // Temporal propagation: maxframe must correlate with position along
    // the line. A lightning flash flags many pixels at one frame (sff ≈ 0).
    let mf = pts.iter().map(|p| p.2).sum::<f32>() / n;
    let (mut spp, mut spf, mut sff) = (0f32, 0f32, 0f32);
    let mut pr_min = f32::MAX;
    let mut pr_max = f32::MIN;
    let mut f_min = f32::MAX;
    let mut f_max = f32::MIN;
    for &(x, y, f) in pts {
        let pr = (x - mx) * ux + (y - my) * uy;
        let df = f - mf;
        spp += pr * pr;
        spf += pr * df;
        sff += df * df;
        pr_min = pr_min.min(pr);
        pr_max = pr_max.max(pr);
        f_min = f_min.min(f);
        f_max = f_max.max(f);
    }
    if spp <= 0.0 || sff <= 0.0 {
        return None;
    }
    let r = spf / (spp * sff).sqrt();
    let span = (f_max - f_min).max(1.0);

    // Endpoints from projection extremes, oriented earliest-end first
    let (mut x0, mut y0) = (mx + ux * pr_min, my + uy * pr_min);
    let (mut x1, mut y1) = (mx + ux * pr_max, my + uy * pr_max);
    if r < 0.0 {
        std::mem::swap(&mut x0, &mut x1);
        std::mem::swap(&mut y0, &mut y1);
    }

    Some(ClusterMetrics {
        pixels: pts.len(),
        lin: l1 / l2,
        length,
        corr: r,
        span,
        speed: length / span,
        x0,
        y0,
        x1,
        y1,
        f0: f_min,
        f1: f_max,
        band_std: 0.0,
    })
}

/// Mean stdpixel of the pixels within `FF_BAND_PX` of the segment (x0,y0)-(x1,y1).
/// Cloud edges drift through the block, so the per-pixel std around them is high;
/// a meteor over a clear sky leaves it at the sensor-noise level.
fn band_mean_std(block: &FfBlock, m: &ClusterMetrics) -> f32 {
    let (w, h) = (block.width as usize, block.height as usize);
    let r = FF_BAND_PX;
    let xa = (m.x0.min(m.x1) - r).floor().max(0.0) as usize;
    let xb = ((m.x0.max(m.x1) + r).ceil() as usize).min(w - 1);
    let ya = (m.y0.min(m.y1) - r).floor().max(0.0) as usize;
    let yb = ((m.y0.max(m.y1) + r).ceil() as usize).min(h - 1);
    let (dx, dy) = (m.x1 - m.x0, m.y1 - m.y0);
    let len2 = (dx * dx + dy * dy).max(1.0);
    let (mut sum, mut n) = (0f64, 0u32);
    for y in ya..=yb {
        for x in xa..=xb {
            let t = (((x as f32 - m.x0) * dx + (y as f32 - m.y0) * dy) / len2).clamp(0.0, 1.0);
            let (px, py) = (m.x0 + t * dx, m.y0 + t * dy);
            if (x as f32 - px).hypot(y as f32 - py) <= r {
                sum += block.stdpixel[y * w + x] as f64;
                n += 1;
            }
        }
    }
    if n == 0 { 0.0 } else { (sum / n as f64) as f32 }
}

/// Best tier whose gates the cluster passes, or `None`. `level` is the σ-clip
/// excess floor the cluster was found at and `total` the flagged pixels of the
/// whole block at that level.
fn tier_of(m: &ClusterMetrics, fps: f32, level: f32, total: usize, t: &Tuning) -> Option<FfTier> {
    // Same px/s floor at any frame rate (unchanged at FF_REF_FPS).
    let fps_scale = FF_REF_FPS / fps.clamp(1.0, 60.0);
    let (w, h) = (WIDTH as f32, HEIGHT as f32);
    // Aircraft / satellites / drifting cloud take seconds to cross; a meteor
    // is over in well under 2 s. Applies to every tier.
    if m.span > t.max_span {
        return None;
    }
    t.tiers.iter().find_map(|g| {
        if m.band_std > g.max_band_std || m.span > g.max_frames {
            return None;
        }
        if m.span <= g.short_frames && m.length < g.short_min_len {
            return None;
        }
        if level < g.min_excess || total < g.min_pixels || m.pixels < g.min_pixels {
            return None;
        }
        if m.lin < g.min_lin || m.length < g.min_len || m.corr.abs() < g.min_corr {
            return None;
        }
        if m.speed < g.min_speed * fps_scale {
            return None;
        }
        let e = g.edge;
        if e > 0.0
            && (m.x0.min(m.x1) < e
                || m.x0.max(m.x1) > w - 1.0 - e
                || m.y0.min(m.y1) < e
                || m.y0.max(m.y1) > h - 1.0 - e)
        {
            return None;
        }
        Some(g.tier)
    })
}

#[cfg(test)]
mod ff_detect_tests {
    use super::*;

    const W: usize = 640;
    const H: usize = 360;

    fn blank_block() -> FfBlock {
        FfBlock {
            maxpixel: vec![102; W * H],
            maxframe: vec![0; W * H],
            avepixel: vec![100; W * H],
            stdpixel: vec![2; W * H],
            width: W as u32,
            height: H as u32,
            start_ts: SystemTime::UNIX_EPOCH,
            n_frames: 256,
            duration_s: 10.2,
            fps_eff: 25.0,
            dropped: 0,
            mixed_fps: false,
            cfg_fps: 25,
            timing_src: "hw",
            ae: None,
        }
    }

    /// Draw a streak (x0,y0)→(x1,y1) peaking at frame `f0..f1`.
    fn add_streak(b: &mut FfBlock, p0: (f32, f32), p1: (f32, f32), f0: f32, f1: f32) {
        add_streak_peak(b, p0, p1, f0, f1, 240);
    }

    /// Same, with an explicit peak value (the excess over the ave of 100 is
    /// `peak - 100`).
    fn add_streak_peak(
        b: &mut FfBlock,
        (x0, y0): (f32, f32),
        (x1, y1): (f32, f32),
        f0: f32,
        f1: f32,
        peak: u8,
    ) {
        let n = 80;
        for k in 0..=n {
            let t = k as f32 / n as f32;
            let x = (x0 + (x1 - x0) * t).round() as usize;
            let y = (y0 + (y1 - y0) * t).round() as usize;
            let i = y * W + x;
            b.maxpixel[i] = peak;
            b.maxframe[i] = (f0 + (f1 - f0) * t).round() as u8;
        }
    }

    /// Deterministic scattered "cloud/noise" pixels above 5σ (but weak).
    fn add_scatter(b: &mut FfBlock, count: usize) {
        let mut s: u32 = 12345;
        for _ in 0..count {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            let x = (s >> 8) as usize % W;
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            let y = (s >> 8) as usize % H;
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            let i = y * W + x;
            if b.maxpixel[i] < 200 {
                b.maxpixel[i] = 100 + 11; // 5.5σ, excess 11 < floor 15
            }
            b.maxframe[i] = (s >> 24) as u8;
        }
    }

    /// Like `add_scatter` but with a chosen excess (ADU above `avepixel`).
    fn add_noise_blobs(b: &mut FfBlock, count: usize, excess: u8) {
        let mut s: u32 = 987654321;
        for _ in 0..count {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            let x = (s >> 8) as usize % W;
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            let y = (s >> 8) as usize % H;
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            let i = y * W + x;
            if b.maxpixel[i] < 200 {
                b.maxpixel[i] = 100 + excess;
            }
            b.maxframe[i] = (s >> 24) as u8;
        }
    }

    fn open_mask() -> Vec<u8> {
        vec![0u8; MASK_CELLS_X * (H / MASK_CELL_SIZE)]
    }

    #[test]
    fn finds_streak_amid_scatter() {
        let mut b = blank_block();
        add_scatter(&mut b, 15_000); // far above the old 4000-pixel cap
        add_streak(&mut b, (206.0, 193.0), (152.0, 205.0), 184.0, 199.0);
        let found = ff_detect_meteor(&b, &open_mask(), 25.0);
        assert_eq!(found.len(), 1, "expected exactly the streak");
        let m = &found[0];
        assert!((m.length - 55.0).abs() < 8.0, "length {}", m.length);
        // earliest end first → x0 is the right end (frame 184 at x=206)
        assert!(m.x0 > m.x1, "direction: x0={} x1={}", m.x0, m.x1);
        assert!(m.speed > 2.0 && m.speed < 5.0, "speed {}", m.speed);
        // seen at both excess levels, reported once, at the best tier
        assert_eq!(m.tier, FfTier::A);
    }

    /// A dark, high-gain block: ~6% of the pixels exceed the low levels, so at 15/12 the noise fuses with the
    /// streak into one blob (the 2026-11-20 02:16:55 meteor was missed this way). The 40 level isolates it.
    #[test]
    fn bright_streak_survives_a_noisy_block() {
        let mut b = blank_block();
        add_noise_blobs(&mut b, 15_000, 17); // excess 17: above 15 and 12, far below a real streak
        add_streak(&mut b, (257.0, 93.0), (200.0, 138.0), 27.0, 35.0); // peak 240 -> excess 140
        let found = ff_detect_meteor(&b, &open_mask(), 25.0);
        assert!(
            found.iter().any(|m| m.tier == FfTier::A && (m.length - 73.0).abs() < 10.0),
            "bright streak not found in tier A: {:?}",
            found.iter().map(|m| (m.tier, m.length)).collect::<Vec<_>>()
        );
        // the previous 15/12 levels lose it in the same block
        let old = Tuning { levels: vec![15.0, 12.0], ..Tuning::default() };
        let missed = ff_detect_meteor_with(&b, &open_mask(), 25.0, &old);
        assert!(
            !missed.iter().any(|m| m.tier == FfTier::A && (m.length - 73.0).abs() < 10.0),
            "expected the old levels to fuse the streak with the noise"
        );
    }

    /// Tier A needs 1.6 px/frame: a short blob that crawls over 11 frames (~1.2 px/frame, the noise chains of the
    /// 2026-11-19/20 night) is not tier A, while the same length crossed in 6 frames (3.3 px/frame) is.
    #[test]
    fn slow_short_blob_is_not_tier_a() {
        let mut slow = blank_block();
        add_streak(&mut slow, (100.0, 100.0), (113.0, 100.0), 100.0, 111.0);
        let found = ff_detect_meteor(&slow, &open_mask(), 25.0);
        assert!(!found.iter().any(|m| m.tier == FfTier::A), "slow blob reached tier A: {:?}", found.iter().map(|m| (m.tier, m.speed)).collect::<Vec<_>>());
        let mut fast = blank_block();
        add_streak(&mut fast, (100.0, 100.0), (120.0, 100.0), 100.0, 106.0);
        let found = ff_detect_meteor(&fast, &open_mask(), 25.0);
        assert!(found.iter().any(|m| m.tier == FfTier::A), "fast streak not tier A: {:?}", found.iter().map(|m| (m.tier, m.speed, m.pixels)).collect::<Vec<_>>());
    }

    /// Excess 13: below the strict level (15), above the lower one (12).
    #[test]
    fn faint_streak_is_a_lower_tier() {
        let mut b = blank_block();
        add_streak_peak(&mut b, (206.0, 193.0), (152.0, 205.0), 184.0, 199.0, 113);
        let found = ff_detect_meteor(&b, &open_mask(), 25.0);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].tier, FfTier::B);
        assert_eq!(found[0].excess, 12.0);
    }

    /// Streak along the top border: only the original tier accepts it (no
    /// border rule there); a faint one is not seen at all.
    #[test]
    fn border_streak_is_tier_a_only() {
        let mut strong = blank_block();
        add_streak(&mut strong, (100.0, 3.0), (150.0, 4.0), 30.0, 45.0);
        let found = ff_detect_meteor(&strong, &open_mask(), 25.0);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].tier, FfTier::A);

        let mut faint = blank_block();
        add_streak_peak(&mut faint, (100.0, 3.0), (150.0, 4.0), 30.0, 45.0, 113);
        assert!(ff_detect_meteor(&faint, &open_mask(), 25.0).is_empty());
    }

    /// A streak that takes the whole block to cross (aircraft/satellite) is
    /// rejected at every tier.
    #[test]
    fn slow_long_streak_is_rejected() {
        let mut b = blank_block();
        add_streak(&mut b, (100.0, 50.0), (400.0, 150.0), 10.0, 250.0);
        assert!(ff_detect_meteor(&b, &open_mask(), 25.0).is_empty());
    }

    /// Faint streak (tier B/C level) in a high-stdpixel neighbourhood = cloud edge:
    /// rejected; the same streak over a quiet sky is kept.
    #[test]
    fn faint_streak_in_noisy_band_is_a_cloud_edge() {
        let line = ((206.0, 193.0), (152.0, 205.0));
        let mut quiet = blank_block();
        add_streak_peak(&mut quiet, line.0, line.1, 184.0, 199.0, 113);
        assert_eq!(ff_detect_meteor(&quiet, &open_mask(), 25.0).len(), 1);

        let mut cloudy = blank_block();
        cloudy.stdpixel.iter_mut().for_each(|s| *s = 6);
        add_streak_peak(&mut cloudy, line.0, line.1, 184.0, 199.0, 113);
        for k in 0..=80 {
            let t = k as f32 / 80.0;
            let x = (line.0.0 + (line.1.0 - line.0.0) * t).round() as usize;
            let y = (line.0.1 + (line.1.1 - line.0.1) * t).round() as usize;
            cloudy.stdpixel[y * W + x] = 2; // keep the streak itself flagged
        }
        assert!(ff_detect_meteor(&cloudy, &open_mask(), 25.0).is_empty());
    }

    #[test]
    fn scatter_alone_is_not_a_meteor() {
        let mut b = blank_block();
        add_scatter(&mut b, 15_000);
        assert!(ff_detect_meteor(&b, &open_mask(), 25.0).is_empty());
    }

    #[test]
    fn two_streaks_both_reported() {
        let mut b = blank_block();
        add_streak(&mut b, (100.0, 50.0), (150.0, 60.0), 30.0, 45.0);
        add_streak(&mut b, (400.0, 300.0), (360.0, 250.0), 120.0, 135.0);
        let found = ff_detect_meteor(&b, &open_mask(), 25.0);
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn simultaneous_flash_is_rejected() {
        // Everything peaks at the same frame: no time gradient.
        let mut b = blank_block();
        add_streak(&mut b, (100.0, 50.0), (160.0, 50.0), 77.0, 77.0);
        assert!(ff_detect_meteor(&b, &open_mask(), 25.0).is_empty());
    }

    #[test]
    fn masked_cells_are_ignored() {
        let mut b = blank_block();
        add_streak(&mut b, (200.0, 192.0), (152.0, 200.0), 184.0, 199.0);
        let mut mask = open_mask();
        for cx in 15..28 {
            for cy in 22..27 {
                mask[cy * MASK_CELLS_X + cx] = 1;
            }
        }
        assert!(ff_detect_meteor(&b, &mask, 25.0).is_empty());
    }
}
