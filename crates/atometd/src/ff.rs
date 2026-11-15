use crate::fits;
use crate::luma;
use crate::paths;
use crate::solve;
use crate::watchdog::WatchdogHandle;
use arc_swap::ArcSwap;
use chrono::{DateTime, Datelike, Local, Timelike, Utc};
use imgproc::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;
use tokio::sync::{broadcast, mpsc, watch};

pub const FRAMES_PER_BLOCK: u32 = 256;
const N: usize = luma::WIDTH * luma::HEIGHT;

/// A finalized FF (Four-Field) block: 4 derived images over a 256-frame window.
///
/// `maxpixel`/`maxframe` together let you reconstruct the brightest event at
/// every pixel and *when* it occurred (frame index 0..=255).
/// `avepixel`/`stdpixel` are the per-pixel temporal mean and stddev — used
/// downstream for σ-clip detection: `maxpixel > avepixel + K·stdpixel`.
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
}

/// Measured fps of the latest block, in thousandths of a frame per second
/// (0 = none yet). Feeds `fps_actual` in the UI.
pub static FPS_MEASURED_MILLI: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Latest measured fps, rounded to whole frames per second (0 = unknown).
pub fn measured_fps_rounded() -> u32 {
    (FPS_MEASURED_MILLI.load(Ordering::Relaxed) as f64 / 1000.0).round() as u32
}

/// Online accumulator for a single FF block. No frame buffering — each frame
/// is folded into running statistics and discarded.
pub struct FfStacker {
    maxpixel: Vec<u8>,
    maxframe: Vec<u8>,
    sum: Vec<u16>,
    sum_sq: Vec<u32>,
    frm_num: u8,
    n: u32,
    start_ts: Option<SystemTime>,
    // Timing of the block: driver timestamps, driver frame counter, wall clock.
    first_hw_ts: i64,
    last_hw_ts: i64,
    first_hw_idx: i32,
    last_hw_idx: i32,
    first_inst: Option<std::time::Instant>,
    last_inst: Option<std::time::Instant>,
    /// Configured fps of the previous frame (0 = none yet); survives blocks.
    last_cfg_fps: u32,
    mixed_fps: bool,
}

impl FfStacker {
    pub fn new() -> Self {
        Self {
            maxpixel: vec![0u8; N],
            maxframe: vec![0u8; N],
            sum: vec![0u16; N],
            sum_sq: vec![0u32; N],
            frm_num: 0,
            n: 0,
            start_ts: None,
            first_hw_ts: 0,
            last_hw_ts: 0,
            first_hw_idx: 0,
            last_hw_idx: 0,
            first_inst: None,
            last_inst: None,
            last_cfg_fps: 0,
            mixed_fps: false,
        }
    }

    /// `hw_ts` / `hw_idx` are the driver's timestamp and frame counter of
    /// `src`; `cfg_fps` is the configured sensor fps at this moment.
    pub fn add_frame(&mut self, src: &[u8], hw_ts: i64, hw_idx: i32, cfg_fps: u32) {
        debug_assert_eq!(src.len(), N);
        let now = std::time::Instant::now();
        if self.start_ts.is_none() {
            self.start_ts = Some(SystemTime::now());
            self.first_hw_ts = hw_ts;
            self.first_hw_idx = hw_idx;
            self.first_inst = Some(now);
        }
        self.last_hw_ts = hw_ts;
        self.last_hw_idx = hw_idx;
        self.last_inst = Some(now);
        // A change of the configured fps (also right after it, while the
        // sensor settles) makes the block's time axis non-uniform.
        if self.last_cfg_fps != 0 && cfg_fps != self.last_cfg_fps {
            self.mixed_fps = true;
        }
        self.last_cfg_fps = cfg_fps;
        unsafe {
            // maxframe must update BEFORE maxpixel — it compares src against
            // the *previous* max to decide whether to record frm_num.
            update_peak_frame(
                src.as_ptr(),
                self.maxframe.as_mut_ptr(),
                self.maxpixel.as_ptr(),
                self.frm_num,
                N,
            );
            brightest(
                src.as_ptr(),
                self.maxpixel.as_ptr(),
                self.maxpixel.as_mut_ptr(),
                N,
            );
            stack_add(src.as_ptr(), self.sum.as_mut_ptr(), N);
            stack_add_sq(src.as_ptr(), self.sum_sq.as_mut_ptr(), N);
        }
        self.frm_num = self.frm_num.wrapping_add(1);
        self.n += 1;
    }

    pub fn is_full(&self) -> bool {
        self.n >= FRAMES_PER_BLOCK
    }

    pub fn finalize(&mut self) -> FfBlock {
        let mut avepixel = vec![0u8; N];
        let mut stdpixel = vec![0u8; N];

        // Per-pixel mean / stddev derivation, kept in Rust:
        //   ave[i] = round(sum[i] / 256)
        //   var[i] = ((sum_sq << 8) - sum²) >> 16  (exact integer math; CS guarantees ≥ 0)
        //   std[i] = sqrt(var[i])  clamped to u8
        // Runs once per ~10s block, so scalar f32 sqrt is fine.
        for i in 0..N {
            let s = self.sum[i] as u32;
            let ss = self.sum_sq[i] as u64;
            avepixel[i] = ((s + 128) >> 8) as u8;
            let num = (ss << 8).saturating_sub((s as u64) * (s as u64));
            let var = (num >> 16) as u32;
            let sd = (var as f32).sqrt() as u32;
            stdpixel[i] = if sd > 255 { 255 } else { sd as u8 };
        }

        let (duration_s, timing_src) = self.measure_duration();
        let fps_eff = if duration_s > 0.0 && self.n > 1 {
            (self.n - 1) as f64 / duration_s
        } else {
            0.0
        };
        // Driver counter span vs frames stacked = frames lost on the way.
        // Only trusted when the counter looks like +1 per frame.
        let span = self.last_hw_idx as i64 - self.first_hw_idx as i64 + 1;
        let dropped = if span >= self.n as i64 && span < 4 * self.n as i64 {
            (span - self.n as i64) as u32
        } else {
            0
        };
        if fps_eff > 0.0 {
            FPS_MEASURED_MILLI.store((fps_eff * 1000.0) as u32, Ordering::Relaxed);
        }

        let block = FfBlock {
            maxpixel: std::mem::replace(&mut self.maxpixel, vec![0u8; N]),
            maxframe: std::mem::replace(&mut self.maxframe, vec![0u8; N]),
            avepixel,
            stdpixel,
            width: luma::WIDTH as u32,
            height: luma::HEIGHT as u32,
            start_ts: self.start_ts.unwrap_or_else(SystemTime::now),
            n_frames: self.n,
            duration_s,
            fps_eff,
            dropped,
            mixed_fps: self.mixed_fps,
            cfg_fps: self.last_cfg_fps,
            timing_src,
        };
        self.sum.fill(0);
        self.sum_sq.fill(0);
        self.frm_num = 0;
        self.n = 0;
        self.start_ts = None;
        self.first_inst = None;
        self.last_inst = None;
        self.mixed_fps = false;
        block
    }

    /// First-to-last frame time of the block in seconds, and its source.
    /// The driver timestamps are used when they agree with the wall clock
    /// (the unit is not documented; µs is expected), otherwise the wall
    /// clock, which also includes any consumer-side jitter.
    fn measure_duration(&self) -> (f64, &'static str) {
        let wall = match (self.first_inst, self.last_inst) {
            (Some(a), Some(b)) => b.duration_since(a).as_secs_f64(),
            _ => 0.0,
        };
        let hw = (self.last_hw_ts - self.first_hw_ts) as f64 / 1e6;
        if hw > 0.0 && wall > 0.0 && (hw / wall - 1.0).abs() < 0.25 {
            (hw, "hw")
        } else if wall > 0.0 {
            (wall, "wall")
        } else {
            (0.0, "none")
        }
    }
}

/// Subscribe to the luma watch channel and emit one `FfBlock` per 256 frames.
///
/// Must run inside `tokio::task::spawn_blocking` (uses `Handle::block_on`).
pub fn ff_stack_worker(
    mut luma_rx: watch::Receiver<Option<luma::LumaFrame>>,
    block_tx: mpsc::Sender<FfBlock>,
    shutdown: Arc<AtomicBool>,
    wd: WatchdogHandle,
    app_state: crate::config::SharedAppState,
) {
    let handle = tokio::runtime::Handle::current();
    let mut stacker = FfStacker::new();

    log::info!("FF stack worker started ({} frames/block)", FRAMES_PER_BLOCK);

    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        wd.tick();

        if handle.block_on(luma_rx.changed()).is_err() {
            break;
        }
        let frame = match luma_rx.borrow_and_update().clone() {
            Some(f) => f,
            None => continue,
        };

        stacker.add_frame(&frame.data, frame.ts, frame.hw_idx, app_state.load().fps);

        if stacker.is_full() {
            let block = stacker.finalize();
            // Non-blocking: if downstream can't keep up, drop the block rather
            // than stalling the stacker (which would back up the luma channel).
            if let Err(e) = block_tx.try_send(block) {
                log::warn!("ff_stack: dropping block, downstream full: {e}");
            }
        }
    }

    log::info!("FF stack worker stopped");
}

// σ-clip threshold multiplier: pixel flagged when maxpixel > avepixel + K * stdpixel
const SIGMA_K: f32 = 5.0;
// Minimum flagged pixels to consider a block "interesting"
const MIN_INTERESTING_PIXELS: usize = 10;

/// Returns true if the block contains a statistically anomalous bright transient.
///
/// Uses per-pixel σ-clip: flag pixel i when `maxpixel[i] > avepixel[i] + K * stdpixel[i]`.
/// stdpixel = 0 means σ < 1 ADU (very stable pixel); treat as σ = 1 so bright
/// meteors still get flagged against flat-dark skies.
pub fn ff_is_interesting(block: &FfBlock) -> bool {
    let n = (block.width * block.height) as usize;
    let mut count = 0usize;
    for i in 0..n {
        let max = block.maxpixel[i] as f32;
        let ave = block.avepixel[i] as f32;
        let std = (block.stdpixel[i] as f32).max(1.0);
        if max > ave + SIGMA_K * std {
            count += 1;
            if count >= MIN_INTERESTING_PIXELS {
                return true;
            }
        }
    }
    false
}

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

/// Minimum flagged pixels in one connected cluster.
const FF_DET_MIN_PIXELS: usize = 12;
/// Absolute floor on `maxpixel - avepixel` (ADU). `stdpixel` is uint8-quantised,
/// so `5σ` alone is ≈ noise level on a dark/cloudy sky and flags thousands of
/// pixels; a real meteor streak is far brighter than this.
const FF_DET_MIN_EXCESS: f32 = 15.0;
/// More flagged pixels *in total* than this = lightning / illumination step
/// (a flood no streak analysis can untangle). With the excess floor, clouds
/// and noise flag only a few hundred pixels.
const FF_DET_MAX_PIXELS: usize = 20_000;
/// Keep at most this many clusters (largest first).
const FF_DET_MAX_METEORS: usize = 8;
const FF_DET_MIN_LINEARITY: f32 = 5.0;
const FF_DET_MIN_LEN_PX: f32 = 12.0;
/// |Pearson r| between position along the line and maxframe.
const FF_DET_MIN_TIME_CORR: f32 = 0.8;
/// Apparent speed floor (px/frame) at FF_REF_FPS. Satellites take the whole
/// block (~10 s) to cross and fall well below this. Scaled to the configured
/// fps in `ff_detect_meteor` so the floor stays the same in px/s.
const FF_DET_MIN_SPEED_PX: f32 = 0.8;
/// Frame rate FF_DET_MIN_SPEED_PX was set at.
const FF_REF_FPS: f32 = 25.0;

// Detection mask cell grid (must match detection.rs / the mask editor)
const MASK_CELL_SIZE: usize = 8;
const MASK_CELLS_X: usize = luma::WIDTH / MASK_CELL_SIZE; // 80

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
}

/// Detect meteors in a finalized FF block (see module notes above).
/// `cell_mask` is the 80×45 detection mask (non-zero = masked).
///
/// Flagged pixels are split into 8-connected clusters (after a 1 px dilation
/// that bridges gaps in a faint streak) and each cluster is judged on its own
/// (line shape, maxframe gradient, speed). Judging the whole frame at once
/// fails as soon as clouds or noise flag scattered pixels around the streak.
/// Returns the accepted clusters, largest first.
pub fn ff_detect_meteor(block: &FfBlock, cell_mask: &[u8], fps: f32) -> Vec<FfMeteor> {
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
            if excess > SIGMA_K * std && excess > FF_DET_MIN_EXCESS {
                flag[i] = true;
                total += 1;
                if total > FF_DET_MAX_PIXELS {
                    return Vec::new();
                }
            }
        }
    }
    if total < FF_DET_MIN_PIXELS {
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
        if pts.len() >= FF_DET_MIN_PIXELS
            && let Some(m) = analyze_cluster(&pts, fps)
        {
            out.push(m);
        }
    }
    out.sort_by_key(|m| std::cmp::Reverse(m.pixels));
    out.truncate(FF_DET_MAX_METEORS);
    out
}

/// Judge one cluster of flagged pixels `(x, y, maxframe)`: line shape,
/// maxframe gradient along the line, and apparent speed.
fn analyze_cluster(pts: &[(f32, f32, f32)], fps: f32) -> Option<FfMeteor> {
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
    if l1 / l2 < FF_DET_MIN_LINEARITY || length < FF_DET_MIN_LEN_PX {
        return None;
    }
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
    if r.abs() < FF_DET_MIN_TIME_CORR {
        return None;
    }

    // Apparent speed along the line (satellite/aircraft guard)
    let span = (f_max - f_min).max(1.0);
    let speed = length / span;
    // Same px/s floor at any frame rate (unchanged at FF_REF_FPS).
    let min_speed = FF_DET_MIN_SPEED_PX * FF_REF_FPS / fps.clamp(1.0, 60.0);
    if speed < min_speed {
        return None;
    }

    // Endpoints from projection extremes, oriented earliest-end first
    let (mut x0, mut y0) = (mx + ux * pr_min, my + uy * pr_min);
    let (mut x1, mut y1) = (mx + ux * pr_max, my + uy * pr_max);
    if r < 0.0 {
        std::mem::swap(&mut x0, &mut x1);
        std::mem::swap(&mut y0, &mut y1);
    }

    Some(FfMeteor {
        x0,
        y0,
        x1,
        y1,
        length,
        frames: span as u32,
        speed,
        pixels: pts.len(),
        time_corr: r,
        f0: f_min,
        f1: f_max,
    })
}

/// WCS stamped into a block: FITS cards plus the attitude/lens they came from
/// (so the saver can turn pixel positions into sky coordinates).
#[derive(Default)]
struct BlockWcs {
    cards: Vec<Vec<u8>>,
    /// Attitude at the block start and the lens.
    geom: Option<(solver::calib::Pose, solver::calib::LensModel)>,
}

/// Sky coordinates of a meteor's endpoints and the great circle it lies on.
///
/// `pose` is the camera attitude at the block start; each end is advanced to
/// its own time (`maxframe / fps` after the block start) so Earth's rotation
/// is accounted for. Everything is ICRS. The pole `n` is oriented so that the
/// meteor moves counter-clockwise about it (`n ∝ v0 × v1`); the radiant lies
/// on the same great circle, behind the start point. Returns `None` if an end
/// is outside the lens model's range or the two ends coincide.
fn meteor_sky(
    m: &FfMeteor,
    pose: &solver::calib::Pose,
    lens: &solver::calib::LensModel,
    fps: f32,
    showers: Option<&crate::showers::Catalog>,
    block_unix: f64,
) -> Option<serde_json::Value> {
    let fps = if fps > 1.0 { fps as f64 } else { FF_REF_FPS as f64 };
    let (cx, cy) = (luma::WIDTH as f64 / 2.0, luma::HEIGHT as f64 / 2.0);
    let end = |x: f32, y: f32, frame: f32| -> Option<[f64; 3]> {
        let p = crate::wcs::advance_pose(pose, frame as f64 / fps);
        solver::calib::pixel_to_sky(&p, lens, x as f64 - cx, y as f64 - cy)
    };
    let v0 = end(m.x0, m.y0, m.f0)?;
    let v1 = end(m.x1, m.y1, m.f1)?;
    let cross = [
        v0[1] * v1[2] - v0[2] * v1[1],
        v0[2] * v1[0] - v0[0] * v1[2],
        v0[0] * v1[1] - v0[1] * v1[0],
    ];
    let cn = (cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2]).sqrt();
    if cn < 1e-9 {
        return None;
    }
    let dot = (v0[0] * v1[0] + v0[1] * v1[1] + v0[2] * v1[2]).clamp(-1.0, 1.0);
    let length_deg = dot.acos().to_degrees();
    let dt = ((m.f1 - m.f0) as f64 / fps).max(1.0 / fps);
    let radec = |v: [f64; 3]| {
        (
            v[1].atan2(v[0]).to_degrees().rem_euclid(360.0),
            v[2].clamp(-1.0, 1.0).asin().to_degrees(),
        )
    };
    let (ra0, dec0) = radec(v0);
    let (ra1, dec1) = radec(v1);
    let mut sky = serde_json::json!({
        "ra0": ra0, "dec0": dec0, "ra1": ra1, "dec1": dec1,
        "pole": [cross[0] / cn, cross[1] / cn, cross[2] / cn],
        "length_deg": length_deg,
        "speed_deg_s": length_deg / dt,
        "t0_s": m.f0 as f64 / fps,
        "t1_s": m.f1 as f64 / fps,
    });
    if let Some(cat) = showers {
        let when = block_unix + m.f0 as f64 / fps;
        let (lam, tol, matches) = cat.associate(&v0, &v1, m.length as f64, when);
        let best = matches.iter().find(|x| x.hit);
        sky["sun_lon"] = lam.into();
        sky["shower_tol_deg"] = tol.into();
        sky["shower"] = best.map(|x| x.code.clone().into()).unwrap_or(serde_json::Value::Null);
        sky["showers"] = matches.iter().map(|x| x.to_json()).collect::<Vec<_>>().into();
    }
    Some(sky)
}

/// Serialize an `FfBlock` to a 4-HDU FITS byte stream (RMS-compatible layout).
///
///   HDU 0 (Primary): MAXPIXEL  — per-pixel maximum value over the block
///   HDU 1 (Image):   MAXFRAME  — frame index (0..255) where the max occurred
///   HDU 2 (Image):   AVEPIXEL  — temporal mean
///   HDU 3 (Image):   STDPIXEL  — temporal stddev
///
/// Also stamps each HDU with DATE-OBS / NFRAMES / EXTNAME etc.
pub fn ff_block_to_fits(block: &FfBlock, wcs: &[Vec<u8>]) -> Vec<u8> {
    let dt: DateTime<Local> = DateTime::<Utc>::from(block.start_ts).with_timezone(&Local);
    let date_obs = dt.format("%Y-%m-%dT%H:%M:%S%.3f").to_string();

    let mut common_cards = vec![
        fits::card_str("DATE-OBS", &date_obs),
        fits::card_int("NFRAMES", block.n_frames as i64),
        fits::card_str("INSTRUME", "Atomet"),
    ];
    // Measured timing of the block (see FfBlock): lets offline code turn
    // maxframe / frame counts into seconds.
    if block.duration_s > 0.0 {
        common_cards.push(fits::card_float("BLKDUR", block.duration_s));
        common_cards.push(fits::card_float("FPS", block.fps_eff));
        common_cards.push(fits::card_str("TIMESRC", block.timing_src));
    }
    common_cards.push(fits::card_int("CFGFPS", block.cfg_fps as i64));
    common_cards.push(fits::card_int("DROPPED", block.dropped as i64));
    common_cards.push(fits::card_bool("MIXEDFPS", block.mixed_fps));
    // Plate-solution WCS (ZPN) — same pixel grid in every HDU.
    common_cards.extend(wcs.iter().cloned());

    let primary_extras = {
        let mut v = common_cards.clone();
        v.push(fits::card_str("EXTNAME", "MAXPIXEL"));
        v
    };

    let mut out = Vec::with_capacity((block.maxpixel.len() + 2880) * 4);

    // HDU 0: primary, with EXTEND=T because we have extensions
    out.extend(fits::primary_image_header(
        block.width,
        block.height,
        true,
        &primary_extras,
    ));
    out.extend(fits::pad_data(block.maxpixel.clone()));

    for (name, data) in [
        ("MAXFRAME", &block.maxframe),
        ("AVEPIXEL", &block.avepixel),
        ("STDPIXEL", &block.stdpixel),
    ] {
        out.extend(fits::image_extension_header(
            block.width,
            block.height,
            name,
            &common_cards,
        ));
        out.extend(fits::pad_data(data.clone()));
    }

    out
}

/// Save an `FfBlock`. If `detected` is true, writes under `ff/detections/`,
/// otherwise under `ff/`.
pub fn save_ff_block(
    block: &FfBlock,
    detected: bool,
    wcs: &[Vec<u8>],
) -> std::io::Result<std::path::PathBuf> {
    let dt: DateTime<Local> = DateTime::<Utc>::from(block.start_ts).with_timezone(&Local);
    let dir = if detected {
        paths::ff_detections_dir(dt)
    } else {
        paths::ff_dir(dt)
    };
    let path = dir.join(paths::ff_filename(dt));
    let bytes = ff_block_to_fits(block, wcs);
    fits::atomic_write(&path, &bytes)?;
    Ok(path)
}

/// Max realtime detection events kept per block for the sidecar.
const SIDECAR_MAX_EVENTS: usize = 64;

/// True for the realtime events worth persisting (`meteor`, `meteor_end`);
/// skips the per-frame `det` debug stream without parsing it.
fn is_sidecar_event(msg: &str) -> bool {
    msg.starts_with(r#"{"type":"meteor""#) || msg.starts_with(r#"{"type":"meteor_end""#)
}

/// Write `<FITS stem>.json` next to a saved FF block: what the realtime and
/// FF-pass detectors reported for that block, for offline re-analysis.
/// `meta` must be a JSON object; `fits` (file name) is added here.
pub fn save_ff_sidecar(fits_path: &std::path::Path, mut meta: serde_json::Value) -> std::io::Result<()> {
    if let (Some(obj), Some(name)) = (meta.as_object_mut(), fits_path.file_name()) {
        obj.insert("fits".into(), name.to_string_lossy().into_owned().into());
    }
    let bytes = serde_json::to_vec_pretty(&meta)
        .map_err(std::io::Error::other)?;
    fits::atomic_write(&fits_path.with_extension("json"), &bytes)
}

/// Smart FF save task.
///
/// Saves a block when ANY of the following is true:
///   - `detection_rx` received at least one event during this block (detected transient)
///   - σ-clip analysis flags the block as interesting (bright anomaly)
///   - One block per hour has not yet been saved (calibration baseline)
///
/// Detected blocks go under `ff/detections/`; others under `ff/`.
/// FITS I/O is handed to `spawn_blocking` to avoid stalling the async runtime.
#[allow(clippy::too_many_arguments)]
pub async fn ff_save_smart_task(
    mut block_rx: mpsc::Receiver<FfBlock>,
    mut detection_rx: broadcast::Receiver<String>,
    ff_tx: tokio::sync::broadcast::Sender<String>,
    shutdown: Arc<AtomicBool>,
    mask: Arc<ArcSwap<Vec<u8>>>,
    db: Arc<Option<solver::Database>>,
    fps: u32,
    app_state: crate::config::SharedAppState,
) {
    log::info!("FF save task started");
    let mut had_detection = false;
    let mut last_baseline: Option<(i32, u32, u32)> = None; // (year, month*100+day, hour)

    // Follows star detections across blocks: a source must be re-sighted in
    // several blocks before it reaches the plate solver, and sources that
    // never move (hot pixels / persistent artifacts) are rejected.
    let star_tracker = Arc::new(std::sync::Mutex::new(
        solver::extractor::StarTracker::new(std::time::Duration::from_secs_f64(
            FRAMES_PER_BLOCK as f64 / fps.max(1) as f64,
        )),
    ));
    // Lens profile from the last validated solve. The lens does not change
    // between blocks, so it seeds the next solve (undistorting the whole
    // frame for the pattern matcher) and is refined by each validated one.
    let lens_prior = Arc::new(std::sync::Mutex::new(None::<solver::calib::LensModel>));
    // Reference attitude + lens for the (fixed) camera, stamped into every
    // FITS as WCS. Loaded from the SD card so a reboot keeps it (the clock is
    // NTP-synced before atometd starts; a stored time in the future is ignored).
    // The solver only runs to build / re-verify it.
    let now_unix = Utc::now().timestamp_millis() as f64 / 1000.0;
    let reference = Arc::new(std::sync::Mutex::new(crate::wcs::Reference::load(now_unix)));
    let mut ff_meteor_seq: u32 = 0;
    // Realtime meteor events seen while the current block accumulated.
    let mut rt_events: Vec<serde_json::Value> = Vec::new();
    let mut shower_cache = crate::showers::CatalogCache::default();

    while !shutdown.load(Ordering::Relaxed) {
        tokio::select! {
            block = block_rx.recv() => {
                let block = match block { Some(b) => b, None => break };
                let dt: DateTime<Local> =
                    DateTime::<Utc>::from(block.start_ts).with_timezone(&Local);

                // Recalibration requested from the UI: forget the reference.
                if crate::wcs::RECALIBRATE.swap(false, Ordering::Relaxed) {
                    reference.lock().unwrap().reset();
                    crate::wcs::forget();
                    log::info!("Plate solver: reference cleared, recalibrating");
                }

                // Star extraction + plate solve on the block's mean frame, only
                // while the reference is missing / due for re-verification, at
                // night, with the solver switched on. The handle resolves to the
                // WCS cards stamped into the FITS (from the reference).
                let solve_handle = {
                    let avepixel = block.avepixel.clone();
                    let mask_snap = mask.load_full();
                    let stars_tx = ff_tx.clone();
                    let tracker = Arc::clone(&star_tracker);
                    let lens_prior = Arc::clone(&lens_prior);
                    let reference = Arc::clone(&reference);
                    let db = Arc::clone(&db);
                    let ts = dt.format("%Y%m%d_%H%M%S").to_string();
                    let t_unix = dt.timestamp_millis() as f64 / 1000.0;
                    let (bw, bh) = (block.width, block.height);
                    // Solver switch + schedule (same kinds as detection's).
                    let (solver_enabled, in_schedule) = {
                        let s = app_state.load();
                        (
                            s.solver_enabled,
                            crate::config::schedule_active(
                                dt,
                                s.solver_use_twilight,
                                s.solver_twilight_astro,
                                s.solver_start_hour,
                                s.solver_end_hour,
                                s.latitude,
                                s.longitude,
                            ),
                        )
                    };
                    tokio::task::spawn_blocking(move || {
                        // WCS cards for this block's FITS, from the reference.
                        let stamp = |r: &crate::wcs::Reference| -> BlockWcs {
                            match r.at(t_unix) {
                                Some((pose, lens, age)) => BlockWcs {
                                    cards: crate::wcs::wcs_cards(&pose, &lens, bw, bh, age),
                                    geom: Some((pose, lens)),
                                },
                                None => BlockWcs::default(),
                            }
                        };
                        // Calibration status for the UI, once per block.
                        let status = |r: &crate::wcs::Reference| {
                            if stars_tx.receiver_count() > 0 {
                                let _ = stars_tx.send(r.status_json(t_unix, solver_enabled, in_schedule));
                            }
                        };
                        if !solver_enabled {
                            status(&reference.lock().unwrap());
                            return BlockWcs::default();
                        }
                        // `should_try` paces the attempts (bursts with backoff);
                        // it is only consulted inside the schedule.
                        if !in_schedule || !reference.lock().unwrap().should_try(t_unix) {
                            let r = reference.lock().unwrap();
                            status(&r);
                            return stamp(&r);
                        }
                        let t0 = std::time::Instant::now();
                        let (stars, threshold) =
                            solve::extract_stars_from_frame(&avepixel, &mask_snap);
                        let track = tracker
                            .lock()
                            .unwrap()
                            .update(&stars, std::time::Instant::now());
                        let n_static = track.iter().filter(|t| t.is_static).count();
                        let n_confirmed = track.iter().filter(|t| t.confirmed).count();
                        let n_tentative = stars.len() - n_static - n_confirmed;

                        // Only confirmed stars (re-sighted across blocks, not
                        // static) feed the plate solver — the same list the UI
                        // marks as accepted.
                        let kept: Vec<(f32, f32)> = stars
                            .iter()
                            .zip(&track)
                            .filter(|&(_, t)| t.confirmed)
                            .map(|(s, _)| (s.x, s.y))
                            .collect();
                        let solve_json = match db.as_ref() {
                            Some(db) if kept.len() >= 4 => {
                                let t1 = std::time::Instant::now();
                                let prior = *lens_prior.lock().unwrap();
                                let r = solve::solve_stars(db, &kept, prior.as_ref());
                                let solve_ms = t1.elapsed().as_secs_f64() * 1000.0;
                                match r {
                                    Some(o) => {
                                        // Collect agreeing validated solves; once
                                        // enough, they become the new reference.
                                        let update = reference
                                            .lock()
                                            .unwrap()
                                            .add_sample(o.pose, o.lens, t_unix);
                                        if let Some(u) = update {
                                            match u.moved_deg {
                                                Some(d) if d > 0.3 => log::warn!(
                                                    "Plate solver: new reference differs from the old by {:.2}° — camera moved?", d
                                                ),
                                                Some(d) => log::info!("Plate solver: reference verified (drift {:.3}°)", d),
                                                None => log::info!("Plate solver: reference established"),
                                            }
                                            match crate::wcs::save(&u.saved) {
                                                Ok(()) => log::info!("Saved reference to {}", crate::paths::SOLVE_LOCK),
                                                Err(e) => log::warn!("Failed to save reference: {}", e),
                                            }
                                        }
                                        log::info!(
                                            "FF plate solve: RA={:.3}° Dec={:.3}° Roll={:.1}° votes={} matched={} outer={} rms={:.2}px in {:.1}ms",
                                            o.ra, o.dec, o.roll, o.votes, o.matched, o.outer, o.rms_px, solve_ms
                                        );
                                        // Remember the lens; blend with the previous
                                        // fit so one noisy solve cannot swing it.
                                        {
                                            use solver::calib::{LensModel, Shape};
                                            let mut slot = lens_prior.lock().unwrap();
                                            *slot = Some(match (*slot, o.lens.shape) {
                                                (
                                                    Some(LensModel { f: f0, shape: Shape::Poly { c3: a0, c5: b0 } }),
                                                    Shape::Poly { c3, c5 },
                                                ) => LensModel {
                                                    f: 0.5 * (f0 + o.lens.f),
                                                    shape: Shape::Poly { c3: 0.5 * (a0 + c3), c5: 0.5 * (b0 + c5) },
                                                },
                                                _ => o.lens,
                                            });
                                        }
                                        // Lens profile for the UI overlay:
                                        // r = f·(θ + c3·θ³ + c5·θ⁵), or f·tanθ without c3/c5.
                                        let lens_json = match o.lens.shape {
                                            solver::calib::Shape::Poly { c3, c5 } => format!(
                                                r#"{{"f":{:.2},"c3":{:.5},"c5":{:.5}}}"#,
                                                o.lens.f, c3, c5
                                            ),
                                            solver::calib::Shape::Pinhole => {
                                                format!(r#"{{"f":{:.2}}}"#, o.lens.f)
                                            }
                                        };
                                        format!(
                                            r#"{{"ra":{:.4},"dec":{:.4},"roll":{:.2},"votes":{},"matched":{},"outer":{},"rms":{:.2},"lens":{},"ms":{:.1}}}"#,
                                            o.ra, o.dec, o.roll, o.votes, o.matched, o.outer, o.rms_px, lens_json, solve_ms
                                        )
                                    }
                                    None => {
                                        log::info!(
                                            "FF plate solve: no solution ({} stars, {:.1}ms)",
                                            kept.len(), solve_ms
                                        );
                                        // Clouds etc.: keep the existing reference.
                                        "null".to_string()
                                    }
                                }
                            }
                            _ => "null".to_string(),
                        };
                        // After a possible update: report and stamp from the reference.
                        let wcs_cards = {
                            let r = reference.lock().unwrap();
                            status(&r);
                            stamp(&r)
                        };

                        log::info!(
                            "FF star extract: {} confirmed, {} tentative, {} static rejected (threshold={}) in {:.1}ms",
                            n_confirmed,
                            n_tentative,
                            n_static,
                            threshold,
                            t0.elapsed().as_secs_f64() * 1000.0,
                        );
                        if stars_tx.receiver_count() > 0 {
                            let stars_json: Vec<String> = stars
                                .iter()
                                .zip(&track)
                                .map(|(s, t)| {
                                    format!(
                                        "[{:.1},{:.1},{},{},{}]",
                                        s.x,
                                        s.y,
                                        s.flux,
                                        // 0 = confirmed, 1 = static, 2 = tentative
                                        if t.is_static { 1 } else if t.confirmed { 0 } else { 2 },
                                        t.hits
                                    )
                                })
                                .collect();
                            let msg = format!(
                                r#"{{"type":"stars","ts":"{}","count":{},"tentative":{},"rejected":{},"threshold":{},"solve":{},"stars":[{}]}}"#,
                                ts,
                                n_confirmed,
                                n_tentative,
                                n_static,
                                threshold,
                                solve_json,
                                stars_json.join(","),
                            );
                            let _ = stars_tx.send(msg);
                        }
                        wcs_cards
                    })
                };

                // FF second-pass meteor detection (σ-clip line + maxframe
                // gradient). Catches faint meteors the per-frame realtime
                // path misses; the block round-trips through spawn_blocking.
                let mask_det = mask.load_full();
                let (block, ff_meteors) = match tokio::task::spawn_blocking(move || {
                    // Use the measured rate of this block (falls back to the
                    // configured one); a block that straddles an fps change
                    // has no uniform time axis, so it is not analysed.
                    let fps_det = if block.fps_eff > 1.0 { block.fps_eff as f32 } else { fps as f32 };
                    let m = if block.mixed_fps {
                        Vec::new()
                    } else {
                        ff_detect_meteor(&block, &mask_det, fps_det)
                    };
                    (block, m)
                })
                .await
                {
                    Ok(v) => v,
                    Err(e) => {
                        log::error!("FF detect task panicked: {}", e);
                        had_detection = false;
                        continue;
                    }
                };

                for m in &ff_meteors {
                    ff_meteor_seq += 1;
                    log::info!(
                        "FF meteor: len={:.0}px frames={} speed={:.1}px/f corr={:.2} pixels={}",
                        m.length, m.frames, m.speed, m.time_corr, m.pixels
                    );
                    let evt = serde_json::json!({
                        "type": "meteor",
                        "id": 10_000 + ff_meteor_seq,
                        "speed": m.speed,
                        "frames": m.frames,
                        "first_frame": 0,
                        "last_frame": m.frames,
                        "trajectory": [[m.x0, m.y0], [m.x1, m.y1]],
                        "ts": dt.format("%Y%m%d_%H%M%S").to_string(),
                        "ff": true,
                    });
                    let _ = ff_tx.send(evt.to_string());
                }

                let detected  = had_detection || !ff_meteors.is_empty();
                let interesting = ff_is_interesting(&block);
                let this_hour = (dt.year(), dt.month() * 100 + dt.day(), dt.hour());
                let is_baseline = last_baseline != Some(this_hour);

                let should_save = detected || interesting || is_baseline;

                if should_save {
                    if is_baseline {
                        last_baseline = Some(this_hour);
                    }
                    let reason = if detected { "detected" }
                                 else if interesting { "interesting" }
                                 else { "baseline" };
                    log::info!(
                        "FF saving block ({}) start={}",
                        reason,
                        dt.format("%Y-%m-%dT%H:%M:%S")
                    );
                    let block_wcs = match solve_handle.await {
                        Ok(c) => c,
                        Err(e) => {
                            log::error!("FF solve task panicked: {}", e);
                            BlockWcs::default()
                        }
                    };
                    let wcs_cards = block_wcs.cards.clone();
                    // Sidecar metadata (detected blocks only).
                    let block_unix = block
                        .start_ts
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs_f64())
                        .unwrap_or(0.0);
                    let shower_cat = if detected && !ff_meteors.is_empty() { shower_cache.get() } else { None };
                    let sidecar = detected.then(|| {
                        serde_json::json!({
                            "version": 1,
                            "block_start": dt.format("%Y-%m-%dT%H:%M:%S%.3f").to_string(),
                            "n_frames": block.n_frames,
                            "fps_config": block.cfg_fps,
                            "fps": block.fps_eff,
                            "duration_s": block.duration_s,
                            "dropped": block.dropped,
                            "mixed_fps": block.mixed_fps,
                            "timing_src": block.timing_src,
                            "clock": crate::clockstat::read(),
                            "shower_catalog": shower_cat.map(|c| c.info.clone()),
                            "reason": reason,
                            "wcs": !wcs_cards.is_empty(),
                            "ff_meteors": ff_meteors.iter().map(|m| {
                                let mut v = serde_json::json!({
                                    "x0": m.x0, "y0": m.y0, "x1": m.x1, "y1": m.y1,
                                    "length": m.length,
                                    "frames": m.frames,
                                    "speed": m.speed,
                                    "pixels": m.pixels,
                                    "time_corr": m.time_corr,
                                    "f0": m.f0, "f1": m.f1,
                                });
                                if let Some((pose, lens)) = &block_wcs.geom
                                    && let Some(sky) = meteor_sky(m, pose, lens, block.fps_eff as f32, shower_cat, block_unix)
                                {
                                    v["sky"] = sky;
                                }
                                v
                            }).collect::<Vec<_>>(),
                            "realtime_events": std::mem::take(&mut rt_events),
                        })
                    });
                    let result = tokio::task::spawn_blocking(move || -> std::io::Result<std::path::PathBuf> {
                        let path = save_ff_block(&block, detected, &wcs_cards)?;
                        if let Some(meta) = sidecar
                            && let Err(e) = save_ff_sidecar(&path, meta)
                        {
                            log::warn!("Failed to save FF sidecar: {}", e);
                        }
                        Ok(path)
                    }).await;
                    match result {
                        Ok(Ok(ref path)) => {
                            log::info!("Saved FF: {}", path.display());
                            // Notify WebSocket clients
                            let url_path = path
                                .components()
                                .skip_while(|c| c.as_os_str() != "ff")
                                .collect::<std::path::PathBuf>();
                            let evt = serde_json::json!({
                                "type": "ff_saved",
                                "reason": reason,
                                "ts": dt.format("%Y%m%d_%H%M%S_%3f").to_string(),
                                "path": format!("/files/{}", url_path.display()),
                            });
                            let _ = ff_tx.send(evt.to_string());
                        }
                        Ok(Err(e)) => log::error!("Failed to save FF: {}", e),
                        Err(e) => log::error!("FF save task panicked: {}", e),
                    }
                } else {
                    log::debug!("FF block skipped (not interesting)");
                }

                had_detection = false;
                rt_events.clear();
            }

            // Drain detection events that fired during block accumulation.
            Ok(msg) = detection_rx.recv() => {
                had_detection = true;
                if rt_events.len() < SIDECAR_MAX_EVENTS
                    && is_sidecar_event(&msg)
                    && let Ok(v) = serde_json::from_str::<serde_json::Value>(&msg)
                {
                    rt_events.push(v);
                }
            }
        }
    }
    log::info!("FF save task stopped");
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
        }
    }

    /// Draw a streak (x0,y0)→(x1,y1) peaking at frame `f0..f1`.
    fn add_streak(b: &mut FfBlock, (x0, y0): (f32, f32), (x1, y1): (f32, f32), f0: f32, f1: f32) {
        let n = 80;
        for k in 0..=n {
            let t = k as f32 / n as f32;
            let x = (x0 + (x1 - x0) * t).round() as usize;
            let y = (y0 + (y1 - y0) * t).round() as usize;
            let i = y * W + x;
            b.maxpixel[i] = 240;
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
