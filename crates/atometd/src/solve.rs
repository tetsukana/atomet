use imgproc::*;
use solver::extractor::{self, RefinedStar};
use solver::calib::{self, LensModel, Pose};
use solver::{Database, Equatorial};

use crate::paths;

pub const WIDTH: usize = 640;
pub const HEIGHT: usize = 360;
const ROW_STRIDE: usize = WIDTH + 2; // 1-byte padding each side

// Plate solver tuning
/// ATOM Cam2 gc2053: ~3.1mm focal length, 3.0µm pixel pitch
/// At 640×360 (3:1 downsample from 1920×1080): effective pixel = 9.0µm
/// focal_length_px = 3.1mm / 0.009mm ≈ 344
pub const FOCAL_LENGTH_PX: f32 = 344.0;
/// Angular matching tolerance for vote accumulation (0.5° → cos)
pub const DOT_THRESHOLD: f32 = 0.9999619; // cos(0.5°)
pub const MAX_HYPOTHESES: usize = 200;
pub const EARLY_STOP_VOTES: usize = 6;

// Cell grid constants (must match detection.rs)
const CELL_SIZE: usize = 8;
const CELLS_X: usize = WIDTH / CELL_SIZE; // 80
const CELLS_Y: usize = HEIGHT / CELL_SIZE; // 45

/// Load the plate solver database from disk, logging the outcome.
///
/// The SD-card copy (`paths::SOLVER_DB`) overrides the one built into the
/// rootfs (`paths::SOLVER_DB_BUILTIN`), so a custom database can be tried
/// without reflashing. Returns `None` (plate solving disabled) if neither
/// exists.
pub fn load_database() -> Option<Database> {
    for path in [paths::SOLVER_DB, paths::SOLVER_DB_BUILTIN] {
        match std::fs::read(path) {
            Ok(data) => {
                log::info!(
                    "Loaded solver database from {} ({} bytes)",
                    path,
                    data.len()
                );
                return Some(Database::load_from_bytes(&data));
            }
            Err(e) => log::info!("Solver database not at {}: {}", path, e),
        }
    }
    log::warn!("No solver database found — plate solving disabled");
    None
}

/// Angular radius (from the image center, assumed to be the optical axis)
/// of the first solve attempt. Lens distortion grows with the off-axis
/// angle and the solver models none, so the central stars are tried first.
const CENTER_STAGE_DEG: f32 = 30.0;

/// A validated plate solution.
#[derive(Debug, Clone, Copy)]
pub struct SolveOutcome {
    pub ra: f64,
    pub dec: f64,
    pub roll: f64,
    /// Pattern-matcher votes for the initial attitude.
    pub votes: usize,
    /// Stars matched to the catalog after calibration (whole frame).
    pub matched: usize,
    /// …of which beyond `calib::INNER_DEG` — independent of the pattern match.
    pub outer: usize,
    /// Residual RMS in pixels.
    pub rms_px: f64,
    /// Fitted lens profile (`r = f·(θ + c3·θ³ + c5·θ⁵)`), for reuse as a prior.
    pub lens: LensModel,
    /// Full attitude (pointing + roll) behind `ra`/`dec`/`roll`.
    pub pose: Pose,
}

fn outcome(cal: &calib::Calibration, votes: usize) -> SolveOutcome {
    let eq = Equatorial::from_cartesian(cal.pose.pointing_vec3());
    SolveOutcome {
        ra: eq.ra.to_degrees() as f64,
        dec: eq.dec.to_degrees() as f64,
        roll: cal.pose.roll.to_degrees(),
        votes,
        matched: cal.matched,
        outer: cal.outer,
        rms_px: cal.rms_px,
        lens: cal.lens,
        pose: cal.pose,
    }
}

/// Plate-solve a list of star positions in WIDTH×HEIGHT pixel coordinates.
///
/// 1. Pattern matching gives an initial attitude. Stars within
///    `CENTER_STAGE_DEG` of the image center (taken as the optical axis) are
///    tried first, then all of them. With a `prior` lens profile the stars are
///    first mapped to pinhole coordinates, so the whole frame is usable.
/// 2. `calib::calibrate` projects the catalog, fits the lens profile / axis /
///    roll ring by ring, and validates against stars beyond 30° that the
///    pattern matcher never relied on.
///
/// Returns `None` if no attitude was found or it failed validation (a
/// pattern-only solution is not reported). Without a usable prior the attempt
/// is repeated assuming a pinhole.
pub fn solve_stars(
    db: &Database,
    stars_px: &[(f32, f32)],
    prior: Option<&LensModel>,
) -> Option<SolveOutcome> {
    if stars_px.len() < 4 {
        return None;
    }
    let cx = WIDTH as f32 / 2.0;
    let cy = HEIGHT as f32 / 2.0;
    let centered: Vec<(f32, f32)> = stars_px.iter().map(|&(x, y)| (x - cx, y - cy)).collect();

    let pinhole = LensModel::pinhole(FOCAL_LENGTH_PX as f64);
    let attempts: Vec<(LensModel, &str)> = match prior {
        Some(p) => vec![(*p, "prior lens"), (pinhole, "pinhole")],
        None => vec![(pinhole, "pinhole")],
    };
    for (lens0, label) in attempts {
        // Pinhole coordinates for the pattern matcher, scale-matched to `lens0`.
        let f_pin = lens0.f as f32;
        let working: Vec<(f32, f32)> = match lens0.shape {
            calib::Shape::Pinhole => centered.clone(),
            calib::Shape::Poly { .. } => centered
                .iter()
                .filter_map(|&(x, y)| {
                    lens0
                        .to_pinhole(x as f64, y as f64, f_pin as f64)
                        .map(|(a, b)| (a as f32, b as f32))
                })
                .collect(),
        };
        let Some((sol, stage)) = initial_attitude(db, &working, f_pin) else {
            log::info!("solve_stars: no pattern match ({label}, {} stars)", working.len());
            continue;
        };
        let Some(cal) = calib::calibrate(&db.stars, &centered, Pose::from_attitude(&sol), lens0)
        else {
            log::info!("solve_stars: calibration found nothing to fit ({label}, {stage}, votes={})", sol.votes);
            continue;
        };
        log::info!(
            "solve_stars: {label}/{stage}: votes={} matched={} outer={} (chance {:.1}) rms={:.2}px f={:.1} → {}",
            sol.votes,
            cal.matched,
            cal.outer,
            cal.outer_chance,
            cal.rms_px,
            cal.lens.f,
            if cal.valid { "valid" } else { "REJECTED" },
        );
        if !cal.valid {
            continue;
        }
        return Some(outcome(&cal, sol.votes));
    }
    None
}

/// Pattern-match an attitude from centered pinhole-like coordinates
/// (focal length `f_pin` px): central stars first, then all.
fn initial_attitude(
    db: &Database,
    all: &[(f32, f32)],
    f_pin: f32,
) -> Option<(solver::AttitudeSolution, &'static str)> {
    // Pinhole model: angle from the axis θ = atan(r / f) ⇒ r_max = f·tan(θ_max).
    let r_max = f_pin * CENTER_STAGE_DEG.to_radians().tan();
    let central: Vec<(f32, f32)> = all
        .iter()
        .copied()
        .filter(|&(x, y)| x.hypot(y) <= r_max)
        .collect();

    if central.len() >= 4 && central.len() < all.len() {
        match solve_centroids(db, &central, f_pin) {
            Some(sol) => Some((sol, "central")),
            None => Some((solve_centroids(db, all, f_pin)?, "all (central failed)")),
        }
    } else {
        Some((solve_centroids(db, all, f_pin)?, "all"))
    }
}

fn solve_centroids(
    db: &Database,
    centroids: &[(f32, f32)],
    f_pin: f32,
) -> Option<solver::AttitudeSolution> {
    db.solve(
        centroids,
        (0.0, 0.0),
        f_pin,
        DOT_THRESHOLD,
        MAX_HYPOTHESES,
        Some(EARLY_STOP_VOTES),
    )
}

// ============================================================
// SIMD-dependent frame processing (requires imgproc crate)
// ============================================================

/// Extract stars from a single frame (e.g. an FF block's avepixel).
///
/// Calibrates the contrast threshold on the frame itself (masked pixels
/// excluded from the histogram), runs one voting pass with masked pixels
/// excluded from voting, then refines the 3×3 local maxima to sub-pixel
/// centroids with background-subtracted flux, sorted by flux descending.
///
/// `mask` is the 80×45 cell mask (non-zero = masked).
/// Returns the refined stars and the threshold used.
pub fn extract_stars_from_frame(frame: &[u8], mask: &[u8]) -> (Vec<RefinedStar>, u8) {
    debug_assert_eq!(frame.len(), WIDTH * HEIGHT);
    let mask_px = expand_cell_mask(mask);
    let mut rows = [[0u8; ROW_STRIDE]; 3];

    let mut hist = [0u32; 256];
    calibrate_frame(frame, &mask_px, &mut rows, &mut hist);
    let threshold = extractor::compute_threshold(&hist);

    let mut vote_map = vec![0u8; WIDTH * HEIGHT];
    vote_frame(frame, &mask_px, &mut rows, &mut vote_map, threshold);

    let stars = extractor::extract_candidates(&vote_map, WIDTH, HEIGHT, 1);
    let refined = extractor::refine_stars(frame, WIDTH, HEIGHT, &stars);
    (refined, threshold)
}

/// Expand the 80×45 cell mask (non-zero = masked) to a full-resolution
/// per-pixel mask for the SIMD kernels.
fn expand_cell_mask(cell_mask: &[u8]) -> Vec<u8> {
    let mut px = vec![0u8; WIDTH * HEIGHT];
    for cy in 0..CELLS_Y {
        for cx in 0..CELLS_X {
            if cell_mask[cy * CELLS_X + cx] != 0 {
                for y in (cy * CELL_SIZE)..((cy + 1) * CELL_SIZE) {
                    let off = y * WIDTH + cx * CELL_SIZE;
                    px[off..off + CELL_SIZE].fill(0xff);
                }
            }
        }
    }
    px
}

/// Process one frame for threshold calibration (contrast histogram).
/// `mask_px` is the per-pixel mask (WIDTH×HEIGHT, non-zero = excluded).
fn calibrate_frame(
    data: &[u8],
    mask_px: &[u8],
    rows: &mut [[u8; ROW_STRIDE]; 3],
    hist: &mut [u32; 256],
) {
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
                mask_px.as_ptr().add(y * WIDTH),
                hist.as_mut_ptr(),
            );
        }
    }
}

/// Process one frame for voting.
/// `mask_px` is the per-pixel mask (WIDTH×HEIGHT, non-zero = excluded).
fn vote_frame(
    data: &[u8],
    mask_px: &[u8],
    rows: &mut [[u8; ROW_STRIDE]; 3],
    vote_map: &mut [u8],
    threshold: u8,
) {
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
                mask_px.as_ptr().add(y * WIDTH),
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
