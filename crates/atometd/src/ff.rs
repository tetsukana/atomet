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
        }
    }

    pub fn add_frame(&mut self, src: &[u8]) {
        debug_assert_eq!(src.len(), N);
        if self.start_ts.is_none() {
            self.start_ts = Some(SystemTime::now());
        }
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

        let block = FfBlock {
            maxpixel: std::mem::replace(&mut self.maxpixel, vec![0u8; N]),
            maxframe: std::mem::replace(&mut self.maxframe, vec![0u8; N]),
            avepixel,
            stdpixel,
            width: luma::WIDTH as u32,
            height: luma::HEIGHT as u32,
            start_ts: self.start_ts.unwrap_or_else(SystemTime::now),
            n_frames: self.n,
        };
        self.sum.fill(0);
        self.sum_sq.fill(0);
        self.frm_num = 0;
        self.n = 0;
        self.start_ts = None;
        block
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

        stacker.add_frame(&frame.data);

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

const FF_DET_MIN_PIXELS: usize = 12;
/// More flagged pixels than this = clouds / lightning / illumination step.
const FF_DET_MAX_PIXELS: usize = 4000;
const FF_DET_MIN_LINEARITY: f32 = 5.0;
const FF_DET_MIN_LEN_PX: f32 = 12.0;
/// |Pearson r| between position along the line and maxframe.
const FF_DET_MIN_TIME_CORR: f32 = 0.8;
/// Apparent speed floor (px/frame). Satellites take the whole block
/// (~10 s) to cross and fall well below this.
const FF_DET_MIN_SPEED_PX: f32 = 0.8;

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
}

/// Detect a meteor in a finalized FF block (see module notes above).
/// `cell_mask` is the 80×45 detection mask (non-zero = masked).
pub fn ff_detect_meteor(block: &FfBlock, cell_mask: &[u8]) -> Option<FfMeteor> {
    let w = block.width as usize;
    let h = block.height as usize;

    // σ-clip: collect anomalous pixels (outside masked cells) with the
    // frame index where each peaked.
    let mut pts: Vec<(f32, f32, f32)> = Vec::new(); // (x, y, maxframe)
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
            if max > ave + SIGMA_K * std {
                if pts.len() >= FF_DET_MAX_PIXELS {
                    return None;
                }
                pts.push((x as f32, y as f32, block.maxframe[i] as f32));
            }
        }
    }
    if pts.len() < FF_DET_MIN_PIXELS {
        return None;
    }

    // PCA line fit on flagged pixel positions
    let n = pts.len() as f32;
    let mx = pts.iter().map(|p| p.0).sum::<f32>() / n;
    let my = pts.iter().map(|p| p.1).sum::<f32>() / n;
    let (mut sxx, mut sxy, mut syy) = (0f32, 0f32, 0f32);
    for &(x, y, _) in &pts {
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
    for &(x, y, f) in &pts {
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
    if speed < FF_DET_MIN_SPEED_PX {
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
    })
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

/// Latest FF-block star extraction, shared with the web layer so the solve
/// page shows exactly the stars the automatic solve used.
pub struct LatestStars {
    /// The block's mean frame (640×360 luma) the stars were extracted from.
    pub avepixel: Vec<u8>,
    /// `[x, y, flux]` of non-static stars, sub-pixel, 640×360 px.
    pub stars: Vec<[f32; 3]>,
    pub threshold: u8,
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
pub async fn ff_save_smart_task(
    mut block_rx: mpsc::Receiver<FfBlock>,
    mut detection_rx: broadcast::Receiver<String>,
    ff_tx: tokio::sync::broadcast::Sender<String>,
    shutdown: Arc<AtomicBool>,
    mask: Arc<ArcSwap<Vec<u8>>>,
    db: Arc<Option<solver::Database>>,
    latest_stars: Arc<arc_swap::ArcSwapOption<LatestStars>>,
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
                    let latest_stars = Arc::clone(&latest_stars);
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
                        let stamp = |r: &crate::wcs::Reference| -> Vec<Vec<u8>> {
                            match r.at(t_unix) {
                                Some((pose, lens, age)) => {
                                    crate::wcs::wcs_cards(&pose, &lens, bw, bh, age)
                                }
                                None => Vec::new(),
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
                            return Vec::new();
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

                        latest_stars.store(Some(Arc::new(LatestStars {
                            avepixel,
                            stars: stars
                                .iter()
                                .zip(&track)
                                .filter(|&(_, t)| t.confirmed)
                                .map(|(s, _)| [s.x, s.y, s.flux as f32])
                                .collect(),
                            threshold,
                        })));

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
                let (block, ff_meteor) = match tokio::task::spawn_blocking(move || {
                    let m = ff_detect_meteor(&block, &mask_det);
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

                if let Some(m) = &ff_meteor {
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

                let detected  = had_detection || ff_meteor.is_some();
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
                    let wcs_cards = match solve_handle.await {
                        Ok(c) => c,
                        Err(e) => {
                            log::error!("FF solve task panicked: {}", e);
                            Vec::new()
                        }
                    };
                    let result = tokio::task::spawn_blocking(
                        move || save_ff_block(&block, detected, &wcs_cards)
                    ).await;
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
            }

            // Drain detection events that fired during block accumulation.
            Ok(_) = detection_rx.recv() => {
                had_detection = true;
            }
        }
    }
    log::info!("FF save task stopped");
}
