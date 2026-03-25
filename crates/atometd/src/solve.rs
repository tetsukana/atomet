use arc_swap::ArcSwap;
use imgproc::*;
use solver::extractor::{self, Star};
use solver::{Database, Equatorial};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{broadcast, watch};

use crate::config::SharedAppState;
use crate::luma;
use crate::watchdog::WatchdogHandle;

const WIDTH: usize = 640;
const HEIGHT: usize = 360;
const ROW_STRIDE: usize = WIDTH + 2; // 1-byte padding each side

// Star extraction tuning
const EXTRACT_FRAMES: usize = 100;
const CALIB_FRAMES: usize = 5;

// Plate solver tuning
const DB_PATH: &str = "/media/mmc/database.bin";
/// ATOM Cam2 gc2053: ~3.1mm focal length, 3.0µm pixel pitch
/// At 640×360 (3:1 downsample from 1920×1080): effective pixel = 9.0µm
/// focal_length_px = 3.1mm / 0.009mm ≈ 344
const FOCAL_LENGTH_PX: f32 = 344.0;
/// Angular matching tolerance for vote accumulation (0.5° → cos)
const DOT_THRESHOLD: f32 = 0.9999619; // cos(0.5°)
const MAX_HYPOTHESES: usize = 200;
const EARLY_STOP_VOTES: usize = 6;

// Cell grid constants (must match detection.rs)
const CELL_SIZE: usize = 8;
const CELLS_X: usize = WIDTH / CELL_SIZE; // 80
const CELLS_Y: usize = HEIGHT / CELL_SIZE; // 45

pub fn solve_task(
    mut luma_rx: watch::Receiver<Option<luma::LumaFrame>>,
    _app_state: SharedAppState,
    shutdown: Arc<AtomicBool>,
    wd: WatchdogHandle,
    debug_tx: broadcast::Sender<String>,
    stack_capture: Arc<AtomicBool>,
    mask: Arc<ArcSwap<Vec<u8>>>,
) {
    let handle = tokio::runtime::Handle::current();

    // Pre-allocated buffers (reused across extractions)
    let mut vote_map = vec![0u8; WIDTH * HEIGHT];
    let mut rows = [[0u8; ROW_STRIDE]; 3];

    // Load plate solver database
    let db = match std::fs::read(DB_PATH) {
        Ok(data) => {
            log::info!(
                "Loaded solver database from {} ({} bytes)",
                DB_PATH,
                data.len()
            );
            Some(Database::load_from_bytes(&data))
        }
        Err(e) => {
            log::warn!(
                "Solver database not found ({}): {} — plate solving disabled",
                DB_PATH,
                e
            );
            None
        }
    };

    log::info!(
        "Solve task started ({}×{}, focal_length={}px)",
        WIDTH,
        HEIGHT,
        FOCAL_LENGTH_PX
    );

    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        wd.tick();

        // Wait for luma frame as timing signal
        match handle.block_on(tokio::time::timeout(
            std::time::Duration::from_secs(3),
            luma_rx.changed(),
        )) {
            Ok(Ok(())) => {}
            Ok(Err(_)) => break,
            Err(_) => continue,
        }
        let _ = luma_rx.borrow_and_update();

        if stack_capture.load(Ordering::Relaxed) {
            let t0 = std::time::Instant::now();

            // Phase 1-3: Star extraction (SIMD voting + candidate extraction)
            let stars = extract_stars(
                &mut vote_map,
                &mut rows,
                &mut luma_rx,
                &shutdown,
                &wd,
                &handle,
            );

            // Filter out stars in masked cells
            let mask_snap = mask.load();
            let stars: Vec<Star> = stars
                .into_iter()
                .filter(|s| {
                    let cx = s.x as usize / CELL_SIZE;
                    let cy = s.y as usize / CELL_SIZE;
                    if cx < CELLS_X && cy < CELLS_Y {
                        mask_snap[cy * CELLS_X + cx] == 0
                    } else {
                        true
                    }
                })
                .collect();

            let extract_ms = t0.elapsed().as_secs_f64() * 1000.0;
            log::info!(
                "Star extraction: {} stars in {:.1}ms ({} frames)",
                stars.len(),
                extract_ms,
                EXTRACT_FRAMES,
            );

            // Send star list via debug channel
            if !stars.is_empty() && debug_tx.receiver_count() > 0 {
                let stars_json: Vec<String> = stars
                    .iter()
                    .map(|s| format!("[{},{},{}]", s.x, s.y, s.votes))
                    .collect();
                let msg = format!(
                    r#"{{"type":"stars","count":{},"stars":[{}]}}"#,
                    stars.len(),
                    stars_json.join(","),
                );
                let _ = debug_tx.send(msg);
            }

            // Phase 4: Plate solve
            if let Some(ref db) = db {
                if stars.len() >= 4 {
                    let centroids = extractor::stars_to_centroids(&stars, WIDTH, HEIGHT);
                    let t1 = std::time::Instant::now();

                    let result = db.solve(
                        &centroids,
                        (0.0, 0.0),
                        FOCAL_LENGTH_PX,
                        DOT_THRESHOLD,
                        MAX_HYPOTHESES,
                        Some(EARLY_STOP_VOTES),
                    );

                    let solve_ms = t1.elapsed().as_secs_f64() * 1000.0;

                    match result {
                        Some(sol) => {
                            let eq = Equatorial::from_cartesian(sol.pointing);
                            let ra_deg = eq.ra.to_degrees();
                            let dec_deg = eq.dec.to_degrees();
                            let roll_deg = sol.roll.to_degrees();

                            log::info!(
                                "Plate solve: RA={:.3}° Dec={:.3}° Roll={:.1}° votes={} in {:.1}ms",
                                ra_deg,
                                dec_deg,
                                roll_deg,
                                sol.votes,
                                solve_ms,
                            );

                            if debug_tx.receiver_count() > 0 {
                                let msg = format!(
                                    r#"{{"type":"solve","ra":{:.4},"dec":{:.4},"roll":{:.2},"votes":{},"stars":{},"extract_ms":{:.1},"solve_ms":{:.1}}}"#,
                                    ra_deg,
                                    dec_deg,
                                    roll_deg,
                                    sol.votes,
                                    stars.len(),
                                    extract_ms,
                                    solve_ms,
                                );
                                let _ = debug_tx.send(msg);
                            }
                        }
                        None => {
                            log::info!(
                                "Plate solve: no solution ({} stars, {:.1}ms)",
                                stars.len(),
                                solve_ms
                            );
                            if debug_tx.receiver_count() > 0 {
                                let msg = format!(
                                    r#"{{"type":"solve","ra":null,"dec":null,"roll":null,"votes":0,"stars":{},"extract_ms":{:.1},"solve_ms":{:.1}}}"#,
                                    stars.len(),
                                    extract_ms,
                                    solve_ms,
                                );
                                let _ = debug_tx.send(msg);
                            }
                        }
                    }
                } else {
                    log::info!("Plate solve: too few stars ({}), need ≥ 4", stars.len());
                }
            }

            stack_capture.store(false, Ordering::Relaxed);
        }
    }
}

// ============================================================
// SIMD-dependent frame processing (requires imgproc crate)
// ============================================================

/// Run star extraction over EXTRACT_FRAMES luma frames.
fn extract_stars(
    vote_map: &mut [u8],
    rows: &mut [[u8; ROW_STRIDE]; 3],
    luma_rx: &mut watch::Receiver<Option<luma::LumaFrame>>,
    shutdown: &AtomicBool,
    wd: &WatchdogHandle,
    handle: &tokio::runtime::Handle,
) -> Vec<Star> {
    vote_map.fill(0);

    // --- Phase 1: Calibrate threshold ---
    let mut hist = [0u32; 256];
    let mut calib_frames_data: Vec<Arc<Vec<u8>>> = Vec::with_capacity(CALIB_FRAMES);

    for _ in 0..CALIB_FRAMES {
        if shutdown.load(Ordering::Relaxed) {
            return Vec::new();
        }
        wd.tick();

        let frame = match wait_frame(luma_rx, handle) {
            Some(f) => f,
            None => return Vec::new(),
        };

        calibrate_frame(&frame.data, rows, &mut hist);
        calib_frames_data.push(Arc::clone(&frame.data));
    }

    let threshold = extractor::compute_threshold(&hist);
    log::info!("Star extract: threshold={}", threshold);

    // --- Phase 2: Vote ---
    for data in &calib_frames_data {
        vote_frame(data, rows, vote_map, threshold);
    }
    drop(calib_frames_data);

    for i in CALIB_FRAMES..EXTRACT_FRAMES {
        if shutdown.load(Ordering::Relaxed) {
            return Vec::new();
        }
        wd.tick();

        let frame = match wait_frame(luma_rx, handle) {
            Some(f) => f,
            None => return Vec::new(),
        };

        vote_frame(&frame.data, rows, vote_map, threshold);

        if (i + 1) % 25 == 0 {
            log::info!("Star extract: {}/{} frames", i + 1, EXTRACT_FRAMES);
        }
    }

    // --- Phase 3: Extract candidates ---
    let min_votes = (EXTRACT_FRAMES / 2) as u8;
    extractor::extract_candidates(vote_map, WIDTH, HEIGHT, min_votes)
}

/// Wait for next luma frame with 3s timeout.
fn wait_frame(
    luma_rx: &mut watch::Receiver<Option<luma::LumaFrame>>,
    handle: &tokio::runtime::Handle,
) -> Option<luma::LumaFrame> {
    match handle.block_on(tokio::time::timeout(
        std::time::Duration::from_secs(3),
        luma_rx.changed(),
    )) {
        Ok(Ok(())) => {}
        _ => return None,
    }
    luma_rx.borrow_and_update().clone()
}

/// Process one frame for threshold calibration (contrast histogram).
fn calibrate_frame(data: &[u8], rows: &mut [[u8; ROW_STRIDE]; 3], hist: &mut [u32; 256]) {
    load_row(data, 0, &mut rows[0]);
    load_row(data, 1, &mut rows[1]);

    for y in 1..(HEIGHT - 1) {
        let prev_idx = (y - 1) % 3;
        let curr_idx = y % 3;
        let next_idx = (y + 1) % 3;

        load_row(data, y + 1, &mut rows[next_idx]);

        unsafe {
            stars_calibrate_row(
                rows[prev_idx].as_ptr().add(1),
                rows[curr_idx].as_ptr().add(1),
                rows[next_idx].as_ptr().add(1),
                hist.as_mut_ptr(),
            );
        }
    }
}

/// Process one frame for voting.
fn vote_frame(data: &[u8], rows: &mut [[u8; ROW_STRIDE]; 3], vote_map: &mut [u8], threshold: u8) {
    load_row(data, 0, &mut rows[0]);
    load_row(data, 1, &mut rows[1]);

    for y in 1..(HEIGHT - 1) {
        let prev_idx = (y - 1) % 3;
        let curr_idx = y % 3;
        let next_idx = (y + 1) % 3;

        load_row(data, y + 1, &mut rows[next_idx]);

        unsafe {
            stars_process_row(
                rows[prev_idx].as_ptr().add(1),
                rows[curr_idx].as_ptr().add(1),
                rows[next_idx].as_ptr().add(1),
                vote_map.as_mut_ptr().add(y * WIDTH),
                threshold,
            );
        }
    }
}

/// Copy row y from frame data into a padded row buffer.
#[inline]
fn load_row(data: &[u8], y: usize, row: &mut [u8; ROW_STRIDE]) {
    row[0] = 0;
    row[WIDTH + 1] = 0;
    let src_offset = y * WIDTH;
    row[1..WIDTH + 1].copy_from_slice(&data[src_offset..src_offset + WIDTH]);
}
