//! Star extractor — vote map processing and star candidate extraction.
//!
//! Converts the output of the SIMD-based morphological peak detection
//! (vote map) into a list of star candidates suitable for plate solving.

use std::time::{Duration, Instant};

/// Tuning constants for star extraction.
pub const MAX_STARS: usize = 200;
pub const THRESHOLD_K: u32 = 4;
pub const THRESHOLD_MIN: u8 = 5;
pub const THRESHOLD_MAX: u8 = 200;

/// A detected star candidate from the vote map.
#[derive(Clone, Debug)]
pub struct Star {
    pub x: u16,
    pub y: u16,
    pub votes: u8,
}

/// Compute threshold from a contrast histogram: `k * median_contrast`.
///
/// `hist` is a 256-bin histogram of contrast values accumulated
/// across calibration frames by the SIMD kernel.
pub fn compute_threshold(hist: &[u32; 256]) -> u8 {
    let total: u64 = hist.iter().map(|&c| c as u64).sum();
    let half = total / 2;

    let mut cumsum: u64 = 0;
    let mut median_contrast: u8 = 0;
    for (i, v) in hist.iter().enumerate().take(256) {
        cumsum += *v as u64;
        if cumsum >= half {
            median_contrast = i as u8;
            break;
        }
    }

    let raw = (median_contrast as u32).saturating_mul(THRESHOLD_K);
    raw.clamp(THRESHOLD_MIN as u32, THRESHOLD_MAX as u32) as u8
}

/// Extract star candidates from a vote map.
///
/// Finds 3×3 local maxima with `votes >= min_votes`.
/// Returns up to `MAX_STARS` candidates sorted by votes descending.
///
/// `width` and `height` are the dimensions of the vote map.
pub fn extract_candidates(
    vote_map: &[u8],
    width: usize,
    height: usize,
    min_votes: u8,
) -> Vec<Star> {
    let mut stars: Vec<Star> = Vec::with_capacity(MAX_STARS);

    for y in 1..(height - 1) {
        for x in 1..(width - 1) {
            let v = vote_map[y * width + x];
            if v < min_votes {
                continue;
            }

            // 3×3 local maximum check
            let mut is_max = true;
            'outer: for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let ny = (y as i32 + dy) as usize;
                    let nx = (x as i32 + dx) as usize;
                    let nv = vote_map[ny * width + nx];
                    if nv > v {
                        is_max = false;
                        break 'outer;
                    }
                    // Tie-breaking: top-left priority
                    if nv == v && (dy < 0 || (dy == 0 && dx < 0)) {
                        is_max = false;
                        break 'outer;
                    }
                }
            }

            if is_max {
                stars.push(Star {
                    x: x as u16,
                    y: y as u16,
                    votes: v,
                });
                if stars.len() >= MAX_STARS {
                    break;
                }
            }
        }
        if stars.len() >= MAX_STARS {
            break;
        }
    }

    // Sort by votes descending (brightest first)
    stars.sort_unstable_by_key(|b| std::cmp::Reverse(b.votes));
    stars
}

/// A star candidate refined to sub-pixel accuracy.
#[derive(Clone, Debug)]
pub struct RefinedStar {
    pub x: f32,
    pub y: f32,
    /// Background-subtracted flux summed over the 5×5 window.
    pub flux: u32,
    /// Frame value at the peak pixel.
    pub peak: u8,
}

/// Refine candidate positions to sub-pixel accuracy and measure flux.
///
/// For each candidate, the local background is estimated as the median of
/// the 16 border pixels of the 5×5 window around the peak, then a
/// background-subtracted weighted centroid and flux are computed over the
/// window. Returns the refined stars sorted by flux descending.
pub fn refine_stars(
    frame: &[u8],
    width: usize,
    height: usize,
    stars: &[Star],
) -> Vec<RefinedStar> {
    let mut refined: Vec<RefinedStar> = stars
        .iter()
        .map(|s| refine_star(frame, width, height, s))
        .collect();
    refined.sort_unstable_by_key(|s| std::cmp::Reverse(s.flux));
    refined
}

fn refine_star(frame: &[u8], width: usize, height: usize, star: &Star) -> RefinedStar {
    // Clamp so the 5×5 window stays inside the frame (shifts the window by
    // ≤1 px for candidates on the 1-px border extract_candidates allows).
    let cx = (star.x as usize).clamp(2, width - 3);
    let cy = (star.y as usize).clamp(2, height - 3);
    let px = |dx: i32, dy: i32| -> u8 {
        frame[(cy as i32 + dy) as usize * width + (cx as i32 + dx) as usize]
    };

    // Background: median of the 16 border pixels of the 5×5 window.
    let mut border = [0u8; 16];
    let mut k = 0;
    for d in -2i32..=2 {
        border[k] = px(d, -2);
        border[k + 1] = px(d, 2);
        k += 2;
    }
    for d in -1i32..=1 {
        border[k] = px(-2, d);
        border[k + 1] = px(2, d);
        k += 2;
    }
    border.sort_unstable();
    let bg = ((border[7] as u16 + border[8] as u16) / 2) as u8;

    // Background-subtracted weighted centroid over the 5×5 window.
    let mut sum_w = 0u32;
    let mut sum_wx = 0u32;
    let mut sum_wy = 0u32;
    for dy in -2i32..=2 {
        for dx in -2i32..=2 {
            let w = px(dx, dy).saturating_sub(bg) as u32;
            if w == 0 {
                continue;
            }
            sum_w += w;
            sum_wx += w * (cx as i32 + dx) as u32;
            sum_wy += w * (cy as i32 + dy) as u32;
        }
    }

    let peak = px(0, 0);
    if sum_w == 0 {
        // Degenerate (flat window) — keep the integer peak position.
        return RefinedStar {
            x: star.x as f32,
            y: star.y as f32,
            flux: 0,
            peak,
        };
    }
    RefinedStar {
        x: sum_wx as f32 / sum_w as f32,
        y: sum_wy as f32 / sum_w as f32,
        flux: sum_w,
        peak,
    }
}

// ============================================================
// Cross-block star tracking (temporal confirmation + static rejection)
// ============================================================
//
// Each FF block is extracted independently, so a single block's candidates
// include noise, aircraft and meteor streaks. A real star shows up in block
// after block, drifting slowly. The tracker *follows* each source (its
// position is updated on every match) and counts how often it was seen:
//
//  - `confirmed`: seen in >= `CONFIRM_HITS` blocks → fed to the plate solver.
//  - `is_static`: stayed within `STATIC_MAX_DISP_PX` of its first position
//    for `STATIC_MIN_AGE` → hot pixel / persistent artifact, excluded.
//
// At ~0.166°/px plate scale (120° diagonal FOV over 734 px), sidereal
// motion is ~1.5 px/min at the celestial equator, i.e. ~15 px per
// `STATIC_MIN_AGE`; the displacement test separates stars from fixed
// artifacts. Stars within ~11° of the pole (displacement over MIN_AGE below
// STATIC_MAX_DISP_PX, e.g. Polaris) never leave that radius and are flagged
// static too — an acceptable loss for solving unless the pole is in view.

/// Max distance between a tracked source's last position and a new
/// detection for them to be the same source. Must cover centroid jitter plus
/// one block of sidereal drift (~0.25 px).
const MATCH_RADIUS_PX: f32 = 2.5;
/// Sightings required before a source is passed to the solver.
const CONFIRM_HITS: u32 = 3;
/// How long a source must be tracked before the static test applies.
const STATIC_MIN_AGE: Duration = Duration::from_secs(600);
/// Total displacement from the first-seen position below which a long-lived
/// source counts as static (a drifting star moves ~15 px over MIN_AGE).
const STATIC_MAX_DISP_PX: f32 = 3.0;
/// Entries not re-sighted for this many blocks are dropped (tolerates a few
/// missed blocks for sources flickering around the detection threshold).
const TRACK_EXPIRE_BLOCKS: u32 = 6;
/// Hard cap on tracked entries.
const TRACK_MAX_ENTRIES: usize = 512;

struct Track {
    x: f32,
    y: f32,
    x0: f32,
    y0: f32,
    first_seen: Instant,
    last_seen: Instant,
    hits: u32,
}

/// Per-star result of [`StarTracker::update`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrackStatus {
    /// Number of blocks this source has been sighted in (including this one).
    pub hits: u32,
    /// Long-lived and fixed in the image → hot pixel / artifact.
    pub is_static: bool,
    /// Seen often enough, and not static → usable for plate solving.
    pub confirmed: bool,
}

/// Tracks star detections across extractions (one per FF block).
///
/// State is in-memory only: after a restart it takes `CONFIRM_HITS` blocks
/// before anything is confirmed, and `STATIC_MIN_AGE` before anything is
/// flagged static.
pub struct StarTracker {
    tracks: Vec<Track>,
    /// Drop a track not re-sighted within this long (`TRACK_EXPIRE_BLOCKS`
    /// block periods).
    expire: Duration,
    /// Sightings required to flag static: about half the blocks in
    /// `STATIC_MIN_AGE`.
    static_min_hits: u32,
}

impl StarTracker {
    /// `block_period` is the time between consecutive extractions
    /// (FF frames per block ÷ fps); expiry and the static hit threshold are
    /// derived from it so they follow the frame rate.
    pub fn new(block_period: Duration) -> Self {
        let period = block_period.max(Duration::from_millis(1));
        Self {
            tracks: Vec::new(),
            expire: period * TRACK_EXPIRE_BLOCKS,
            static_min_hits: ((STATIC_MIN_AGE.as_secs_f64() / period.as_secs_f64()) / 2.0)
                .max(1.0) as u32,
        }
    }

    /// Match `stars` against tracked sources and update tracking state.
    ///
    /// Returns one [`TrackStatus`] per star, in input order. Matching is
    /// one-to-one and greedy in input order (callers pass stars sorted by
    /// flux descending, so bright stars claim their tracks first).
    pub fn update(&mut self, stars: &[RefinedStar], now: Instant) -> Vec<TrackStatus> {
        // Drop stale tracks first so a source unseen for longer than the
        // expiry window restarts from scratch.
        self.tracks
            .retain(|t| now.duration_since(t.last_seen) <= self.expire);

        let r2 = MATCH_RADIUS_PX * MATCH_RADIUS_PX;
        let mut claimed = vec![false; self.tracks.len()];
        let mut out = Vec::with_capacity(stars.len());

        for s in stars {
            // Nearest unclaimed track within the match radius.
            let mut best: Option<(usize, f32)> = None;
            for (i, t) in self.tracks.iter().enumerate() {
                if claimed[i] {
                    continue;
                }
                let dx = t.x - s.x;
                let dy = t.y - s.y;
                let d2 = dx * dx + dy * dy;
                if d2 <= r2 && best.is_none_or(|(_, bd)| d2 < bd) {
                    best = Some((i, d2));
                }
            }

            match best {
                Some((i, _)) => {
                    claimed[i] = true;
                    let t = &mut self.tracks[i];
                    t.x = s.x;
                    t.y = s.y;
                    t.last_seen = now;
                    t.hits += 1;

                    let dx = t.x - t.x0;
                    let dy = t.y - t.y0;
                    let is_static = now.duration_since(t.first_seen) >= STATIC_MIN_AGE
                        && t.hits >= self.static_min_hits
                        && dx * dx + dy * dy <= STATIC_MAX_DISP_PX * STATIC_MAX_DISP_PX;
                    out.push(TrackStatus {
                        hits: t.hits,
                        is_static,
                        confirmed: !is_static && t.hits >= CONFIRM_HITS,
                    });
                }
                None => {
                    if self.tracks.len() < TRACK_MAX_ENTRIES {
                        self.tracks.push(Track {
                            x: s.x,
                            y: s.y,
                            x0: s.x,
                            y0: s.y,
                            first_seen: now,
                            last_seen: now,
                            hits: 1,
                        });
                        claimed.push(true);
                    }
                    out.push(TrackStatus {
                        hits: 1,
                        ..TrackStatus::default()
                    });
                }
            }
        }

        out
    }
}

/// Convert star pixel positions to solver-compatible centroids.
///
/// Returns `(x, y)` pairs relative to the image center, suitable for
/// passing directly to `Database::solve()` with `center = (0, 0)`.
///
/// The centroids are in pixel units; pass the focal length in pixels
/// as `focal_length_px` to the solver.
pub fn stars_to_centroids(stars: &[Star], img_width: usize, img_height: usize) -> Vec<(f32, f32)> {
    let cx = img_width as f32 / 2.0;
    let cy = img_height as f32 / 2.0;
    stars
        .iter()
        .map(|s| (s.x as f32 - cx, s.y as f32 - cy))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_from_uniform_hist() {
        // Uniform histogram: median = 128, threshold = 128 * 4 = clamped to 200
        let mut hist = [1u32; 256];
        let t = compute_threshold(&hist);
        assert_eq!(t, 200); // 128 * 4 > 200, clamped

        // All zeros except bin 2: median = 2, threshold = 8
        hist = [0u32; 256];
        hist[2] = 1000;
        let t = compute_threshold(&hist);
        assert_eq!(t, 8);
    }

    #[test]
    fn threshold_minimum_floor() {
        // All zeros in bin 0: median = 0, threshold = max(0*4, 5) = 5
        let mut hist = [0u32; 256];
        hist[0] = 1000;
        let t = compute_threshold(&hist);
        assert_eq!(t, 5);
    }

    #[test]
    fn extract_single_peak() {
        let w = 10;
        let h = 10;
        let mut vmap = vec![0u8; w * h];
        vmap[5 * w + 5] = 10;
        let stars = extract_candidates(&vmap, w, h, 5);
        assert_eq!(stars.len(), 1);
        assert_eq!(stars[0].x, 5);
        assert_eq!(stars[0].y, 5);
        assert_eq!(stars[0].votes, 10);
    }

    #[test]
    fn extract_rejects_below_min_votes() {
        let w = 10;
        let h = 10;
        let mut vmap = vec![0u8; w * h];
        vmap[5 * w + 5] = 3;
        let stars = extract_candidates(&vmap, w, h, 5);
        assert!(stars.is_empty());
    }

    #[test]
    fn extract_tiebreak_topleft() {
        // Two equal adjacent peaks — top-left wins
        let w = 10;
        let h = 10;
        let mut vmap = vec![0u8; w * h];
        vmap[3 * w + 3] = 10;
        vmap[3 * w + 4] = 10;
        let stars = extract_candidates(&vmap, w, h, 5);
        assert_eq!(stars.len(), 1);
        assert_eq!(stars[0].x, 3);
    }

    #[test]
    fn refine_subpixel_centroid_and_flux() {
        // Two equal pixels above a flat background of 10 → centroid between them
        let w = 12;
        let h = 12;
        let mut f = vec![10u8; w * h];
        f[5 * w + 5] = 110;
        f[5 * w + 6] = 110;
        let stars = vec![Star { x: 5, y: 5, votes: 1 }];
        let r = refine_stars(&f, w, h, &stars);
        assert_eq!(r.len(), 1);
        assert!((r[0].x - 5.5).abs() < 1e-3);
        assert!((r[0].y - 5.0).abs() < 1e-3);
        assert_eq!(r[0].flux, 200);
        assert_eq!(r[0].peak, 110);
    }

    #[test]
    fn refine_flat_window_is_degenerate() {
        let w = 12;
        let h = 12;
        let f = vec![10u8; w * h];
        let stars = vec![Star { x: 5, y: 5, votes: 1 }];
        let r = refine_stars(&f, w, h, &stars);
        assert_eq!(r[0].flux, 0);
        assert!((r[0].x - 5.0).abs() < 1e-6);
        assert!((r[0].y - 5.0).abs() < 1e-6);
    }

    #[test]
    fn refine_sorts_by_flux_and_clamps_border() {
        let w = 16;
        let h = 16;
        let mut f = vec![0u8; w * h];
        f[5 * w + 5] = 50; // faint
        f[10 * w + 10] = 200; // bright
        // Border candidate at (1,1): window clamps to (2,2) without panicking
        let stars = vec![
            Star { x: 5, y: 5, votes: 1 },
            Star { x: 1, y: 1, votes: 1 },
            Star { x: 10, y: 10, votes: 1 },
        ];
        let r = refine_stars(&f, w, h, &stars);
        assert_eq!(r[0].flux, 200);
        assert_eq!(r[1].flux, 50);
        assert_eq!(r[2].flux, 0);
    }

    fn star_at(x: f32, y: f32) -> RefinedStar {
        RefinedStar {
            x,
            y,
            flux: 500,
            peak: 200,
        }
    }

    #[test]
    fn tracker_flags_persistent_source_as_static() {
        let mut tr = StarTracker::new(Duration::from_secs(10));
        let t0 = Instant::now();
        let mut st = TrackStatus::default();
        // Same position every ~10s for 11 minutes (small centroid jitter)
        for i in 0..66u64 {
            let jitter = if i % 2 == 0 { 0.2 } else { -0.2 };
            st = tr.update(&[star_at(100.0 + jitter, 50.0)], t0 + Duration::from_secs(i * 10))[0];
        }
        assert!(st.is_static);
        assert!(!st.confirmed);
    }

    #[test]
    fn tracker_confirms_drifting_star_without_flagging_static() {
        let mut tr = StarTracker::new(Duration::from_secs(10));
        let t0 = Instant::now();
        // Drift 0.25 px per block ≈ equatorial sidereal rate at this scale
        for i in 0..120u64 {
            let st = tr.update(
                &[star_at(100.0 + i as f32 * 0.25, 50.0)],
                t0 + Duration::from_secs(i * 10),
            )[0];
            assert!(!st.is_static, "moving star flagged static at block {i}");
            assert_eq!(st.hits, i as u32 + 1, "track lost at block {i}");
            assert_eq!(st.confirmed, i as u32 + 1 >= CONFIRM_HITS);
        }
    }

    #[test]
    fn tracker_tolerates_jitter_and_missed_blocks() {
        let mut tr = StarTracker::new(Duration::from_secs(10));
        let t0 = Instant::now();
        // ±1 px centroid jitter, with the star missing every 3rd block.
        let mut hits = 0;
        for i in 0..30u64 {
            if i % 3 == 2 {
                tr.update(&[], t0 + Duration::from_secs(i * 10));
                continue;
            }
            let jitter = if i % 2 == 0 { 1.0 } else { -1.0 };
            hits = tr.update(&[star_at(100.0 + jitter, 50.0)], t0 + Duration::from_secs(i * 10))[0].hits;
        }
        assert_eq!(hits, 20, "track should survive jitter and gaps");
    }

    #[test]
    fn tracker_does_not_confirm_one_off_detections() {
        let mut tr = StarTracker::new(Duration::from_secs(10));
        let t0 = Instant::now();
        let st = tr.update(&[star_at(10.0, 10.0)], t0)[0];
        assert!(!st.confirmed);
        // Different place next block → separate track, still unconfirmed.
        let st = tr.update(&[star_at(200.0, 100.0)], t0 + Duration::from_secs(10))[0];
        assert_eq!(st.hits, 1);
        assert!(!st.confirmed);
    }

    #[test]
    fn tracker_matches_one_to_one() {
        let mut tr = StarTracker::new(Duration::from_secs(10));
        let t0 = Instant::now();
        tr.update(&[star_at(100.0, 50.0)], t0);
        // Two detections near one track: only one may claim it.
        let st = tr.update(
            &[star_at(100.5, 50.0), star_at(101.0, 50.0)],
            t0 + Duration::from_secs(10),
        );
        assert_eq!(st[0].hits, 2);
        assert_eq!(st[1].hits, 1);
    }

    #[test]
    fn tracker_resets_after_gap() {
        let mut tr = StarTracker::new(Duration::from_secs(10));
        let t0 = Instant::now();
        let s = [star_at(100.0, 50.0)];
        // Seen once, then a 5-minute gap (track expires), then continuous.
        tr.update(&s, t0);
        for i in 0..59u64 {
            let now = t0 + Duration::from_secs(300 + i * 10);
            let st = tr.update(&s, now)[0];
            // Age since the restart at t=300 stays below 600s → never static,
            // even though the total span exceeds STATIC_MIN_AGE.
            assert!(!st.is_static, "flagged at {}s despite tracking reset", 300 + i * 10);
        }
    }

    #[test]
    fn stars_to_centroids_centering() {
        let stars = vec![
            Star {
                x: 320,
                y: 180,
                votes: 10,
            },
            Star {
                x: 0,
                y: 0,
                votes: 5,
            },
        ];
        let c = stars_to_centroids(&stars, 640, 360);
        assert!((c[0].0).abs() < 1e-6); // center → (0, 0)
        assert!((c[0].1).abs() < 1e-6);
        assert!((c[1].0 - (-320.0)).abs() < 1e-6);
        assert!((c[1].1 - (-180.0)).abs() < 1e-6);
    }
}
