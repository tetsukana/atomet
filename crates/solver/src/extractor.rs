//! Star extractor — vote map processing and star candidate extraction.
//!
//! Converts the output of the SIMD-based morphological peak detection
//! (vote map) into a list of star candidates suitable for plate solving.

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
