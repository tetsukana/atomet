use arc_swap::ArcSwap;
use imgproc::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{broadcast, watch};

use crate::config::{SharedAppState, schedule_active};
use crate::luma;
use crate::paths;
use crate::watchdog::WatchdogHandle;

const INTERVAL: Duration = Duration::from_secs(1);

const IMG_WIDTH: usize = 640;
const IMG_HEIGHT: usize = 360;
const BLOB_MERGE_GAP: i32 = 2;

// Cell grid (8×8 px per cell → 80×45 cells)
const CELL_SIZE: usize = 8;
const CELLS_X: usize = IMG_WIDTH / CELL_SIZE; // 80
const CELLS_Y: usize = IMG_HEIGHT / CELL_SIZE; // 45
const N_CELLS: usize = CELLS_X * CELLS_Y;

// Tuning — detection
const SPATIAL_THRESHOLD: f32 = 6.0; // spatial MAD z-score threshold (per-frame)
const TEMPORAL_THRESHOLD: f32 = 5.0; // temporal mean/stddev z-score threshold (per-cell EMA)
const MIN_CELL_DELTA: f32 = 6.0; // absolute floor: cell mean-diff must exceed frame median by this (counts)
const EMA_ALPHA: f32 = 0.05; // EMA decay (time constant ≈ 20 frames)
const EMA_CLAMP_SIGMA: f32 = 3.0; // winsorize EMA updates at ±kσ so events don't poison stats
const WARMUP_FRAMES: u32 = 3; // frames to settle stats after (re)start before detecting

// Blob sanity: reject blobs covering a huge fraction of the frame
// (lightning, cloud edges, illumination changes)
const MAX_BLOB_CELLS: i32 = (N_CELLS / 8) as i32;

// Stacking
// Must cover SHORT_MAX_FRAMES + (TRACK_MAX_MISSED + 1) retirement lag so the
// meteor frames are still in the ring when a short track is post-mortemed.
const LUMA_PREBUF: usize = 8; // pre-detection luma frames to keep (for stack init)
const STACK_TIMEOUT: u32 = 15; // frames after last detection before saving stack

// Tuning — short-meteor (streak) path. Tracks that die unconfirmed within
// SHORT_MAX_FRAMES are judged by single-frame streak morphology instead of
// trajectory: a meteor smears into a line within one exposure, while star
// twinkle / hot pixels / cosmic-ray hits / aircraft strobes stay point-like.
const SHORT_MAX_FRAMES: usize = 3; // history length considered "short"
const STREAK_DIFF_THRESHOLD: u8 = 10; // pixel diff floor inside blob ROI
const STREAK_MIN_ELONG: f32 = 6.0; // pixel-level λ1/λ2 to accept a streak
const STREAK_MIN_LEN_PX: f32 = 16.0; // full-extent length floor (≈ 2 cells)
const STREAK_MIN_FLUX: u64 = 300; // sum of diff≥threshold px; rejects faint noise/hot-pixel lines
// A short-path streak whose end comes this close to the image border is sensor-edge
// noise (bright rows/columns at the border), not a meteor: on 2026-11-14 all 6 short
// events were such lines (ends within 6 px of the border, 1 frame, 16-21 px/frame).
// Meteors that cross the border are still caught by the tracked path or the FF pass.
const STREAK_EDGE_PX: f32 = 6.0;
// Short streaks must also be this long. On the dark, high-gain night of 2026-11-19/20 the
// detector raised 4,550 short events (median 19 px, i.e. just above STREAK_MIN_LEN_PX) and none was a
// meteor; on the quiet night before 64% of the short events were >= 30 px.
const SHORT_MIN_LEN_PX: f32 = 30.0;

// Tuning — tracker
const TRACK_MAX_DIST: f32 = 5.0; // max residual distance to predicted position (cells)
const TRACK_MAX_MISSED: u32 = 2; // drop track after this many consecutive missed frames
const TRACK_MIN_FRAMES: usize = 4; // minimum history length to attempt confirmation
const TRACK_MIN_DISP_SQ: f32 = 4.0; // minimum displacement² (cells²) to confirm
const TRACK_MIN_LINEARITY: f32 = 12.0; // λ1/λ2 eigenvalue ratio for linear motion
const TRACK_MAX_HISTORY: usize = 64; // sliding window cap on centroid history
const TRACK_MAX_EVENT_FRAMES: u32 = 75; // ~3s @ 25fps — longer-lived = aircraft/satellite
const TRACK_MIN_MONOTONIC: f32 = 0.8; // fraction of steps advancing along the principal axis
const TRACK_MIN_SPEED_CELLS: f32 = 0.0625; // 0.5 px/frame — slower = satellite/aircraft
// Faster than this is noise blobs chained across the frame: on 2026-11-19/20 all 735 confirmed
// tracks were exactly 4 frames at a median 36 px/frame (the linking limit, TRACK_MAX_DIST), while
// the real meteors of the quiet nights were <= 14 px/frame (90th percentile) and the real ones
// seen in the FF pass 1.9-10.7 px/frame.
const TRACK_MAX_SPEED_CELLS: f32 = 3.0; // 24 px/frame
// A track whose points mostly lie within this many cells (12 px) of the image border follows
// sensor-edge noise (bright rows/columns), e.g. x = 1..12 px running down the left edge.
const TRACK_EDGE_CELLS: f32 = 1.5;

/// Frame rate the per-frame tuning constants above were set at. The ones that
/// are really durations or speeds (EMA_ALPHA, TRACK_MAX_EVENT_FRAMES,
/// TRACK_MIN_SPEED_CELLS) are rescaled to the configured sensor fps so the
/// real-time behaviour stays the same; at 25 fps nothing changes. The other
/// constants (ring depth, history lengths, spatial gates) are deliberately
/// left in frames.
const REF_FPS: f32 = 25.0;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Timing {
    /// Per-frame EMA weight with the same time constant in seconds.
    ema_alpha: f32,
    /// Meteor duration cap in frames (same seconds as at REF_FPS).
    max_event_frames: u32,
    /// Minimum speed in cells/frame (same cells/s as at REF_FPS).
    min_speed_cells: f32,
    /// Maximum speed in cells/frame (same cells/s as at REF_FPS).
    max_speed_cells: f32,
}

impl Timing {
    fn new(fps: u32) -> Self {
        let fps = fps.clamp(1, 60) as f32;
        // How many REF_FPS frames fit in one frame at `fps`.
        let r = REF_FPS / fps;
        if (r - 1.0).abs() < 1e-6 {
            return Timing {
                ema_alpha: EMA_ALPHA,
                max_event_frames: TRACK_MAX_EVENT_FRAMES,
                min_speed_cells: TRACK_MIN_SPEED_CELLS,
                max_speed_cells: TRACK_MAX_SPEED_CELLS,
            };
        }
        Timing {
            ema_alpha: 1.0 - (1.0 - EMA_ALPHA).powf(r),
            max_event_frames: ((TRACK_MAX_EVENT_FRAMES as f32 / r).round() as u32).max(1),
            min_speed_cells: TRACK_MIN_SPEED_CELLS * r,
            max_speed_cells: TRACK_MAX_SPEED_CELLS * r,
        }
    }
}

// ============================================================
// Blob
// ============================================================

#[derive(Copy, Clone, Debug)]
pub struct Blob {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub pix_cnt: usize,
}

impl Blob {
    fn new(x: i32, y: i32, width: i32, height: i32, pix_cnt: usize) -> Self {
        Self {
            x,
            y,
            width,
            height,
            pix_cnt,
        }
    }

    fn centroid(&self) -> (f32, f32) {
        (
            self.x as f32 + self.width as f32 / 2.0,
            self.y as f32 + self.height as f32 / 2.0,
        )
    }
}

#[derive(Copy, Clone)]
struct Point {
    x: usize,
    y: usize,
}

pub fn find_bounding_rectangles(
    image: &[u8],
    visited: &mut [u8],
    width: usize,
    height: usize,
) -> Vec<Blob> {
    let mut blobs = Vec::new();
    visited.fill(0);

    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            if image[idx] == 0xff && visited[idx] == 0 {
                let mut min_x = x;
                let mut max_x = x;
                let mut min_y = y;
                let mut max_y = y;
                let mut pix_cnt = 0;
                let mut stack = vec![Point { x, y }];
                visited[idx] = 1;

                while let Some(curr) = stack.pop() {
                    pix_cnt += 1;
                    min_x = min_x.min(curr.x);
                    max_x = max_x.max(curr.x);
                    min_y = min_y.min(curr.y);
                    max_y = max_y.max(curr.y);

                    for dy in -1..=1i32 {
                        for dx in -1..=1i32 {
                            if dx == 0 && dy == 0 {
                                continue;
                            }
                            let nx = curr.x as i32 + dx;
                            let ny = curr.y as i32 + dy;
                            if nx >= 0 && nx < width as i32 && ny >= 0 && ny < height as i32 {
                                let nx = nx as usize;
                                let ny = ny as usize;
                                let n_idx = ny * width + nx;
                                if image[n_idx] == 0xff && visited[n_idx] == 0 {
                                    visited[n_idx] = 1;
                                    stack.push(Point { x: nx, y: ny });
                                }
                            }
                        }
                    }
                }

                blobs.push(Blob::new(
                    min_x as i32,
                    min_y as i32,
                    (max_x - min_x + 1) as i32,
                    (max_y - min_y + 1) as i32,
                    pix_cnt,
                ));
            }
        }
    }
    blobs
}

fn merge_blobs(blobs: &[Blob]) -> Vec<Blob> {
    if blobs.is_empty() {
        return Vec::new();
    }

    let n = blobs.len();
    let mut parent: Vec<usize> = (0..n).collect();

    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    fn union(parent: &mut [usize], a: usize, b: usize) {
        let ra = find(parent, a);
        let rb = find(parent, b);
        if ra != rb {
            parent[rb] = ra;
        }
    }

    for i in 0..n {
        for j in (i + 1)..n {
            let a = &blobs[i];
            let b = &blobs[j];
            let overlap_x = (a.x - BLOB_MERGE_GAP) < b.x + b.width + BLOB_MERGE_GAP
                && (b.x - BLOB_MERGE_GAP) < a.x + a.width + BLOB_MERGE_GAP;
            let overlap_y = (a.y - BLOB_MERGE_GAP) < b.y + b.height + BLOB_MERGE_GAP
                && (b.y - BLOB_MERGE_GAP) < a.y + a.height + BLOB_MERGE_GAP;
            if overlap_x && overlap_y {
                union(&mut parent, i, j);
            }
        }
    }

    let mut merged: Vec<Blob> = Vec::new();
    let mut roots: Vec<usize> = Vec::new();

    for (i, v) in blobs.iter().enumerate().take(n) {
        let root = find(&mut parent, i);
        if let Some(pos) = roots.iter().position(|&r| r == root) {
            let m = &mut merged[pos];
            let new_x = m.x.min(v.x);
            let new_y = m.y.min(v.y);
            let new_x1 = (m.x + m.width).max(v.x + v.width);
            let new_y1 = (m.y + m.height).max(v.y + v.height);
            m.x = new_x;
            m.y = new_y;
            m.width = new_x1 - new_x;
            m.height = new_y1 - new_y;
            m.pix_cnt += v.pix_cnt;
        } else {
            roots.push(root);
            merged.push(*v);
        }
    }

    merged
}

// ============================================================
// PNG encoder (uncompressed deflate)
// ============================================================

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFFFFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB88320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let (mut s1, mut s2) = (1u32, 0u32);
    for &b in data {
        s1 = (s1 + b as u32) % 65521;
        s2 = (s2 + s1) % 65521;
    }
    (s2 << 16) | s1
}

fn png_chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(tag);
    out.extend_from_slice(data);
    let mut crc_buf = Vec::with_capacity(4 + data.len());
    crc_buf.extend_from_slice(tag);
    crc_buf.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_buf).to_be_bytes());
}

/// Encode grayscale u8 image as PNG using uncompressed (stored) deflate blocks.
fn encode_png_gray(data: &[u8], width: usize, height: usize) -> Vec<u8> {
    // Raw filtered data: prepend filter byte 0 to each row
    let mut raw = Vec::with_capacity((width + 1) * height);
    for y in 0..height {
        raw.push(0u8); // filter: None
        raw.extend_from_slice(&data[y * width..(y + 1) * width]);
    }

    // zlib wrap with stored deflate blocks (no compression)
    let mut zlib = Vec::new();
    zlib.push(0x78); // CMF: deflate, window 32K
    zlib.push(0x01); // FLG: (0x7801 % 31 == 0) ✓, no dict
    const BLOCK: usize = 65535;
    let mut pos = 0;
    while pos < raw.len() {
        let end = (pos + BLOCK).min(raw.len());
        let len = (end - pos) as u16;
        zlib.push(if end == raw.len() { 0x01 } else { 0x00 }); // BFINAL, BTYPE=00
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(&raw[pos..end]);
        pos = end;
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    // Assemble PNG
    let mut png = Vec::new();
    png.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]); // signature

    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(height as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 0, 0, 0, 0]); // 8-bit, grayscale, deflate, no filter, no interlace
    png_chunk(&mut png, b"IHDR", &ihdr);
    png_chunk(&mut png, b"IDAT", &zlib);
    png_chunk(&mut png, b"IEND", &[]);
    png
}

fn save_png(data: &[u8], width: usize, height: usize, path: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(
        std::path::Path::new(path)
            .parent()
            .unwrap_or(std::path::Path::new("/")),
    )?;
    std::fs::write(path, encode_png_gray(data, width, height))
}

/// Save the brightest stack off-thread so flash I/O never stalls the
/// detection loop. Detections are rare, so the one-off clone + thread
/// spawn cost is negligible.
fn spawn_save_stack(stack: Vec<u8>, ts_dir: String, debug_tx: broadcast::Sender<String>) {
    std::thread::spawn(move || {
        let path = format!("{}/{}/stack.png", paths::DETECTIONS_DIR, ts_dir);
        match save_png(&stack, IMG_WIDTH, IMG_HEIGHT, &path) {
            Err(e) => log::error!("failed to save stack: {}", e),
            Ok(()) => {
                log::info!("saved detection stack: {}", path);
                let msg = format!(
                    r#"{{"type":"meteor_stack","path":"{}","ts":"{}"}}"#,
                    path,
                    ts_dir.replace('/', "")
                );
                let _ = debug_tx.send(msg);
            }
        }
    });
}

/// Max-blend the pre-detection ring buffer into a freshly zeroed stack.
fn init_stack_from_ring(
    stack_data: &mut [u8],
    luma_ring: &[u8],
    ring_full: bool,
    ring_idx: usize,
    frame_size: usize,
) {
    stack_data.fill(0);
    let slots = if ring_full { LUMA_PREBUF } else { ring_idx };
    for i in 0..slots {
        let buf = &luma_ring[i * frame_size..(i + 1) * frame_size];
        unsafe {
            brightest(
                buf.as_ptr(),
                stack_data.as_ptr(),
                stack_data.as_mut_ptr(),
                frame_size,
            );
        }
    }
}

// ============================================================
// Spatial MAD z-score (histogram-based, O(n))
// ============================================================

/// Returns (median, MAD) of the unmasked cells of a u8 slice using the
/// histogram method. Masked cells (wind-blown trees, building edges) would
/// otherwise inflate the MAD and desensitize the whole frame.
fn median_mad(data: &[u8], mask: &[u8]) -> (u8, u8) {
    let mut hist = [0u32; 256];
    let mut count = 0usize;
    for (&v, &m) in data.iter().zip(mask) {
        if m == 0 {
            hist[v as usize] += 1;
            count += 1;
        }
    }
    if count == 0 {
        return (0, 0);
    }

    let mid = count / 2;

    let mut sum = 0usize;
    let mut median = 0u8;
    for (i, v) in hist.iter().enumerate() {
        sum += *v as usize;
        if sum > mid {
            median = i as u8;
            break;
        }
    }

    let mut hist_dev = [0u32; 256];
    for (&v, &m) in data.iter().zip(mask) {
        if m == 0 {
            let d = (v as i16 - median as i16).unsigned_abs() as usize;
            hist_dev[d] += 1;
        }
    }

    let mut sum = 0usize;
    let mut mad = 0u8;
    for (i, v) in hist_dev.iter().enumerate() {
        sum += *v as usize;
        if sum > mid {
            mad = i as u8;
            break;
        }
    }

    (median, mad)
}

// ============================================================
// Streak morphology (pixel-level, per-blob ROI)
// ============================================================

#[derive(Copy, Clone, Default)]
struct StreakStats {
    flux: u64,       // sum of diff values above threshold
    cx: f32,         // intensity-weighted centroid, px
    cy: f32,
    elongation: f32, // λ1/λ2 of the 2nd-moment covariance
    length: f32,     // full-extent estimate √(12·λ1), px
    ux: f32,         // principal axis unit vector
    uy: f32,
}

/// True if either end of the streak's principal-axis segment lies within
/// `STREAK_EDGE_PX` of the image border (px, 640×360).
fn streak_near_edge(s: &StreakStats) -> bool {
    let half = s.length / 2.0;
    let (w, h) = (IMG_WIDTH as f32 - 1.0, IMG_HEIGHT as f32 - 1.0);
    [-half, half].iter().any(|&t| {
        let (x, y) = (s.cx + s.ux * t, s.cy + s.uy * t);
        x < STREAK_EDGE_PX || y < STREAK_EDGE_PX || x > w - STREAK_EDGE_PX || y > h - STREAK_EDGE_PX
    })
}

/// Intensity-weighted second moments of the saturating diff (cur − prev)
/// inside the blob's pixel ROI (blob is in cell coords; padded by one cell).
/// Raw moments accumulate in u64 (exact); covariance/eigen in f64.
/// Cost: scalar over a ~32×32 px ROI, only on frames that produced blobs —
/// negligible, no SIMD needed.
fn streak_stats(cur: &[u8], prev: &[u8], blob: &Blob) -> StreakStats {
    let x0 = (blob.x.max(0) as usize * CELL_SIZE).saturating_sub(CELL_SIZE);
    let y0 = (blob.y.max(0) as usize * CELL_SIZE).saturating_sub(CELL_SIZE);
    let x1 = (((blob.x + blob.width) as usize + 1) * CELL_SIZE).min(IMG_WIDTH);
    let y1 = (((blob.y + blob.height) as usize + 1) * CELL_SIZE).min(IMG_HEIGHT);

    let (mut m00, mut m10, mut m01) = (0u64, 0u64, 0u64);
    let (mut m20, mut m11, mut m02) = (0u64, 0u64, 0u64);
    for y in y0..y1 {
        let row_c = &cur[y * IMG_WIDTH..y * IMG_WIDTH + IMG_WIDTH];
        let row_p = &prev[y * IMG_WIDTH..y * IMG_WIDTH + IMG_WIDTH];
        for x in x0..x1 {
            let w = row_c[x].saturating_sub(row_p[x]);
            if w >= STREAK_DIFF_THRESHOLD {
                let w = w as u64;
                m00 += w;
                m10 += w * x as u64;
                m01 += w * y as u64;
                m20 += w * (x * x) as u64;
                m11 += w * (x * y) as u64;
                m02 += w * (y * y) as u64;
            }
        }
    }
    if m00 == 0 {
        return StreakStats::default();
    }

    let m00f = m00 as f64;
    let cx = m10 as f64 / m00f;
    let cy = m01 as f64 / m00f;
    let sxx = m20 as f64 / m00f - cx * cx;
    let sxy = m11 as f64 / m00f - cx * cy;
    let syy = m02 as f64 / m00f - cy * cy;

    let trace = sxx + syy;
    let det = sxx * syy - sxy * sxy;
    let disc = ((trace * trace / 4.0) - det).max(0.0).sqrt();
    let l1 = (trace / 2.0 + disc).max(0.0);
    // λ2 floor of (0.5 px)² keeps the ratio finite for 1-px features;
    // those are rejected by the length gate anyway.
    let l2 = (trace / 2.0 - disc).max(0.0625);

    let theta = 0.5 * (2.0 * sxy).atan2(sxx - syy);

    StreakStats {
        flux: m00,
        cx: cx as f32,
        cy: cy as f32,
        elongation: (l1 / l2) as f32,
        // Uniform line segment of length L has axial variance L²/12.
        length: (12.0 * l1).sqrt() as f32,
        ux: theta.cos() as f32,
        uy: theta.sin() as f32,
    }
}

// ============================================================
// Tracker
// ============================================================

struct TrackMetrics {
    linearity: f32, // λ1/λ2 of the centroid-history covariance
    monotonic: f32, // fraction of steps advancing along the principal axis
    speed: f32,     // mean speed over the history window, cells/frame
}

struct Track {
    id: u32,
    history: Vec<(f32, f32)>, // centroid history in cell coords (sliding window)
    missed: u32,
    confirmed: bool,
    rejected: bool, // demoted (aircraft/satellite) — never re-confirms
    first_frame: u32,
    best_streak: StreakStats, // stats from the brightest (max-flux) frame
}

impl Track {
    fn new(id: u32, cx: f32, cy: f32, frame: u32, streak: StreakStats) -> Self {
        Self {
            id,
            history: vec![(cx, cy)],
            missed: 0,
            confirmed: false,
            rejected: false,
            first_frame: frame,
            best_streak: streak,
        }
    }

    fn last(&self) -> (f32, f32) {
        *self.history.last().unwrap()
    }

    /// Constant-velocity extrapolation from the last two points.
    /// Falls back to the last position for single-point tracks.
    fn predicted(&self) -> (f32, f32) {
        let n = self.history.len();
        if n >= 2 {
            let (x1, y1) = self.history[n - 1];
            let (x0, y0) = self.history[n - 2];
            (2.0 * x1 - x0, 2.0 * y1 - y0)
        } else {
            self.last()
        }
    }

    /// Append a centroid, capping history to a sliding window so long-lived
    /// tracks (aircraft, satellites) don't grow unboundedly and the PCA in
    /// linearity() stays O(TRACK_MAX_HISTORY).
    fn push(&mut self, p: (f32, f32)) {
        if self.history.len() >= TRACK_MAX_HISTORY {
            self.history.remove(0);
        }
        self.history.push(p);
    }

    /// PCA on centroid history plus motion-consistency metrics. Returns
    /// None if displacement or history length are insufficient.
    fn metrics(&self) -> Option<TrackMetrics> {
        let n = self.history.len();
        if n < TRACK_MIN_FRAMES {
            return None;
        }

        // Check total displacement (within the sliding window)
        let (x0, y0) = self.history[0];
        let (x1, y1) = self.last();
        let disp_sq = (x1 - x0).powi(2) + (y1 - y0).powi(2);
        if disp_sq < TRACK_MIN_DISP_SQ {
            return None;
        }

        // Centroid of history
        let nf = n as f32;
        let cx = self.history.iter().map(|p| p.0).sum::<f32>() / nf;
        let cy = self.history.iter().map(|p| p.1).sum::<f32>() / nf;

        // 2×2 covariance
        let (mut sxx, mut sxy, mut syy) = (0f32, 0f32, 0f32);
        for &(x, y) in &self.history {
            let dx = x - cx;
            let dy = y - cy;
            sxx += dx * dx;
            sxy += dx * dy;
            syy += dy * dy;
        }

        let trace = sxx + syy;
        let det = sxx * syy - sxy * sxy;
        let disc = ((trace * trace / 4.0) - det).max(0.0).sqrt();
        let lambda1 = trace / 2.0 + disc;
        let lambda2 = (trace / 2.0 - disc).max(1e-6);

        // Per-step progression along the principal axis. A source blinking
        // between two spots is perfectly linear but not monotonic; meteors
        // advance in one direction. Small negative steps (-0.05 cells) are
        // tolerated as centroid jitter.
        let theta = 0.5 * (2.0 * sxy).atan2(sxx - syy);
        let (ux, uy) = (theta.cos(), theta.sin());
        let proj = |p: &(f32, f32)| (p.0 - cx) * ux + (p.1 - cy) * uy;
        let dir = if proj(&self.history[n - 1]) >= proj(&self.history[0]) {
            1.0
        } else {
            -1.0
        };
        let forward = self
            .history
            .windows(2)
            .filter(|w| (proj(&w[1]) - proj(&w[0])) * dir > -0.05)
            .count();

        Some(TrackMetrics {
            linearity: lambda1 / lambda2,
            monotonic: forward as f32 / (n - 1) as f32,
            speed: disp_sq.sqrt() / (n - 1) as f32,
        })
    }

    /// True when at least half of the history lies within `TRACK_EDGE_CELLS` of the image border.
    fn hugs_border(&self) -> bool {
        let (w, h) = (CELLS_X as f32, CELLS_Y as f32);
        let near = self
            .history
            .iter()
            .filter(|p| p.0 < TRACK_EDGE_CELLS || p.1 < TRACK_EDGE_CELLS || p.0 > w - TRACK_EDGE_CELLS || p.1 > h - TRACK_EDGE_CELLS)
            .count();
        near * 2 >= self.history.len()
    }

    /// `min_speed_cells` / `max_speed_cells`: speed window in cells/frame (see `Timing`).
    fn is_confirmed(&self, min_speed_cells: f32, max_speed_cells: f32) -> bool {
        !self.rejected
            && !self.hugs_border()
            && self.metrics().is_some_and(|m| {
                m.linearity >= TRACK_MIN_LINEARITY
                    && m.monotonic >= TRACK_MIN_MONOTONIC
                    && m.speed >= min_speed_cells
                    && m.speed <= max_speed_cells
            })
    }
}

/// Greedy nearest-neighbour association against velocity-predicted positions.
/// The gate widens with consecutive misses to tolerate brief dropouts.
/// Returns matched (track_idx, centroid_idx) pairs, plus unmatched centroid indices.
fn associate(tracks: &[Track], centroids: &[(f32, f32)]) -> (Vec<(usize, usize)>, Vec<usize>) {
    let mut matched: Vec<(usize, usize)> = Vec::new();
    let mut used_c = vec![false; centroids.len()];
    let mut used_t = vec![false; tracks.len()];

    // Build (dist², track_idx, centroid_idx) candidates
    let mut candidates: Vec<(f32, usize, usize)> = Vec::new();
    for (ti, track) in tracks.iter().enumerate() {
        let (tx, ty) = track.predicted();
        let gate = TRACK_MAX_DIST * (1.0 + track.missed as f32);
        let gate_sq = gate * gate;
        for (ci, &(cx, cy)) in centroids.iter().enumerate() {
            let d = (cx - tx).powi(2) + (cy - ty).powi(2);
            if d <= gate_sq {
                candidates.push((d, ti, ci));
            }
        }
    }
    candidates.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

    for (_, ti, ci) in candidates {
        if !used_t[ti] && !used_c[ci] {
            matched.push((ti, ci));
            used_t[ti] = true;
            used_c[ci] = true;
        }
    }

    let unmatched_c = (0..centroids.len()).filter(|&i| !used_c[i]).collect();
    (matched, unmatched_c)
}

// ============================================================
// Detection task
// ============================================================

#[allow(clippy::too_many_arguments)]
pub fn detection_task(
    mut luma_rx: watch::Receiver<Option<luma::LumaFrame>>,
    app_state: SharedAppState,
    shutdown: Arc<AtomicBool>,
    wd: WatchdogHandle,
    debug_tx: broadcast::Sender<String>,
    meteor_tx: broadcast::Sender<String>,
    mask: Arc<ArcSwap<Vec<u8>>>,
    detection_active: Arc<AtomicBool>,
    detection_tracking: Arc<AtomicBool>,
) {
    let handle = tokio::runtime::Handle::current();
    let frame_size = IMG_WIDTH * IMG_HEIGHT;

    // Previous frame is held by reference to the channel's LumaFrame —
    // no per-frame 225KB copy into a prev buffer.
    let mut prev_luma: Option<luma::LumaFrame> = None;
    let mut cell_map = vec![0u8; N_CELLS];
    let mut cell_visited = vec![0u8; N_CELLS];
    let mut cell_mean = vec![0f32; N_CELLS];
    let mut cell_var = vec![1f32; N_CELLS];
    let mut warmup: u32 = WARMUP_FRAMES;
    let mut last_cfg_fps: u32 = 0;
    let mut tracks: Vec<Track> = Vec::new();
    let mut next_track_id: u32 = 0;
    // Pre-allocated ring buffer for pre-detection luma frames (no per-frame alloc)
    let mut luma_ring = vec![0u8; frame_size * LUMA_PREBUF];
    let mut luma_ring_idx: usize = 0;
    let mut luma_ring_full = false;
    // Brightest stack (pre-allocated, reused across detections)
    let mut stack_data = vec![0u8; frame_size];
    let mut stack_active = false;
    let mut stack_ts = String::new();
    let mut stack_countdown: u32 = 0;
    let mut frame_index: u32 = 0;
    let mut elapsed_acc: u64 = 0;
    const LOG_INTERVAL: u32 = 100;

    log::info!("Detection task started");

    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        wd.tick();

        let s = app_state.load();
        let now_local = chrono::Local::now();
        if !s.detection_enabled
            || !schedule_active(now_local, s.detection_use_twilight, s.detection_twilight_astro, s.detection_start_hour, s.detection_end_hour, s.latitude, s.longitude)
        {
            detection_active.store(false, Ordering::Relaxed);
            detection_tracking.store(false, Ordering::Relaxed);
            // Flush any in-progress stack so a detection right at schedule
            // end isn't lost, then reset state so resume starts clean
            // (a stale prev frame would otherwise fire the whole frame).
            if stack_active {
                spawn_save_stack(stack_data.clone(), stack_ts.clone(), debug_tx.clone());
                stack_active = false;
            }
            tracks.clear();
            prev_luma = None;
            luma_ring_idx = 0;
            luma_ring_full = false;
            warmup = WARMUP_FRAMES;
            std::thread::sleep(INTERVAL);
            continue;
        }
        let timing = Timing::new(s.fps);
        // The configured fps changed: the sensor re-times and auto-exposure
        // re-settles, so the old per-cell baselines and tracks are stale.
        // Flush and re-learn for ~2 s (same reset as the schedule-off branch).
        if last_cfg_fps != 0 && s.fps != last_cfg_fps {
            log::info!("fps changed {} -> {}: resetting detector baselines", last_cfg_fps, s.fps);
            if stack_active {
                spawn_save_stack(stack_data.clone(), stack_ts.clone(), debug_tx.clone());
                stack_active = false;
            }
            tracks.clear();
            prev_luma = None;
            luma_ring_idx = 0;
            luma_ring_full = false;
            warmup = WARMUP_FRAMES + 2 * s.fps;
        }
        last_cfg_fps = s.fps;
        drop(s);

        if handle.block_on(luma_rx.changed()).is_err() {
            break;
        }
        let luma = match luma_rx.borrow_and_update().clone() {
            Some(f) => f,
            None => continue,
        };

        // Write current luma into ring slot (no allocation)
        luma_ring[luma_ring_idx * frame_size..(luma_ring_idx + 1) * frame_size]
            .copy_from_slice(&luma.data);
        luma_ring_idx = (luma_ring_idx + 1) % LUMA_PREBUF;
        if luma_ring_idx == 0 {
            luma_ring_full = true;
        }

        // First frame after (re)start: prime prev and wait for the next one.
        let Some(prev) = prev_luma.as_ref() else {
            prev_luma = Some(luma);
            continue;
        };

        let t = std::time::Instant::now();
        let mask_snap = mask.load();

        // Fused saturating-subtract + 8×8 binning (single pass over both
        // frames, no intermediate diff buffer).
        unsafe {
            sub_bin_8x8(
                luma.data.as_ptr(),
                prev.data.as_ptr(),
                IMG_WIDTH,
                IMG_HEIGHT,
                cell_map.as_mut_ptr(),
            );
        }

        // Warmup: seed temporal stats, no detection yet. Covers both initial
        // startup and resume after a schedule pause.
        if warmup > 0 {
            for i in 0..N_CELLS {
                cell_mean[i] = cell_map[i] as f32;
                cell_var[i] = 1.0;
            }
            warmup -= 1;
            prev_luma = Some(luma);
            frame_index = frame_index.wrapping_add(1);
            continue;
        }

        // Spatial MAD z-score (masked cells excluded from the statistics)
        let (median, mad) = median_mad(&cell_map, &mask_snap);
        let median_f = median as f32;
        let sigma_s = (mad as f32 * 1.4826).max(1.0); // MAD → σ, floored

        let mut max_z_s = 0f32;
        let mut max_z_t = 0f32;
        for i in 0..N_CELLS {
            let v = cell_map[i] as f32;

            let z_s = (v - median_f) / sigma_s;

            let mu = &mut cell_mean[i];
            let var = &mut cell_var[i];
            let delta = v - *mu;
            let sigma_t = var.sqrt().max(1.0);
            let z_t = delta / sigma_t;

            // Winsorized EMA update: clamp the innovation at ±kσ so a meteor
            // (or aircraft) passing through a cell doesn't inflate its
            // mean/variance and desensitize it for the next ~1/α frames.
            let d = delta.clamp(-EMA_CLAMP_SIGMA * sigma_t, EMA_CLAMP_SIGMA * sigma_t);
            *mu += timing.ema_alpha * d;
            *var = (1.0 - timing.ema_alpha) * (*var + timing.ema_alpha * d * d);

            if z_s > max_z_s {
                max_z_s = z_s;
            }
            if z_t > max_z_t {
                max_z_t = z_t;
            }

            cell_map[i] =
                if mask_snap[i] == 0
                    && z_s > SPATIAL_THRESHOLD
                    && z_t > TEMPORAL_THRESHOLD
                    && v - median_f >= MIN_CELL_DELTA
                {
                    0xff
                } else {
                    0
                };
        }

        // Blob detection
        let cell_blobs = find_bounding_rectangles(&cell_map, &mut cell_visited, CELLS_X, CELLS_Y);
        // Sanity filter: drop frame-scale blobs (lightning, clouds,
        // illumination steps) before they can spawn tracks.
        let merged_cells: Vec<Blob> = merge_blobs(&cell_blobs)
            .into_iter()
            .filter(|b| b.width * b.height <= MAX_BLOB_CELLS)
            .collect();

        // Pixel-level streak morphology per blob (ROI-limited). Also yields
        // sub-cell centroids, which tightens the trajectory PCA for slow or
        // short tracks compared to cell-center centroids.
        let streaks: Vec<StreakStats> = merged_cells
            .iter()
            .map(|b| streak_stats(&luma.data, &prev.data, b))
            .collect();

        // Centroids of current blobs (cell coords, sub-cell precision)
        let centroids: Vec<(f32, f32)> = merged_cells
            .iter()
            .zip(&streaks)
            .map(|(b, s)| {
                if s.flux > 0 {
                    (s.cx / CELL_SIZE as f32, s.cy / CELL_SIZE as f32)
                } else {
                    b.centroid()
                }
            })
            .collect();

        // Associate blobs to tracks (velocity-predicted gating)
        let (matched, unmatched_c) = associate(&tracks, &centroids);

        // Update matched tracks
        let mut matched_track_ids: Vec<bool> = vec![false; tracks.len()];
        for (ti, ci) in &matched {
            tracks[*ti].push(centroids[*ci]);
            tracks[*ti].missed = 0;
            if streaks[*ci].flux > tracks[*ti].best_streak.flux {
                tracks[*ti].best_streak = streaks[*ci];
            }
            matched_track_ids[*ti] = true;
        }

        // Increment missed counter for unmatched tracks
        for (ti, was_matched) in matched_track_ids.iter().enumerate() {
            if !was_matched {
                tracks[ti].missed += 1;
            }
        }

        // Spawn new tracks for unmatched centroids
        for ci in unmatched_c {
            let (cx, cy) = centroids[ci];
            tracks.push(Track::new(next_track_id, cx, cy, frame_index, streaks[ci]));
            next_track_id = next_track_id.wrapping_add(1);
        }

        // Demote confirmed tracks that outlive the meteor duration cap —
        // aircraft and satellites are linear too, but last much longer.
        // Demoted tracks stay in the list as association sinks so their
        // blobs don't spawn fresh tracks that re-confirm.
        for tr in tracks.iter_mut() {
            if tr.confirmed && frame_index.wrapping_sub(tr.first_frame) > timing.max_event_frames {
                tr.confirmed = false;
                tr.rejected = true;
                log::info!(
                    "track {} demoted after {} frames (aircraft/satellite)",
                    tr.id,
                    frame_index.wrapping_sub(tr.first_frame)
                );
            }
        }

        // Check for newly confirmed tracks and send meteor events
        let mut confirmed_ids: Vec<u32> = Vec::new();
        let now = chrono::Local::now();
        let ts = now.format("%Y%m%d_%H%M%S").to_string();
        let ts_dir = now.format("%Y/%m/%d/%H%M%S").to_string();
        for tr in tracks.iter_mut() {
            if tr.rejected {
                continue;
            }
            if tr.confirmed {
                confirmed_ids.push(tr.id);
                continue;
            }
            if tr.is_confirmed(timing.min_speed_cells, timing.max_speed_cells) {
                tr.confirmed = true;
                confirmed_ids.push(tr.id);

                let n = tr.history.len();
                let (x0, y0) = tr.history[0];
                let (x1, y1) = tr.last();
                let dist = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt();
                let speed = dist * CELL_SIZE as f32 / n as f32; // px/frame
                let traj = tr
                    .history
                    .iter()
                    .map(|(x, y)| {
                        format!("[{:.0},{:.0}]", x * CELL_SIZE as f32, y * CELL_SIZE as f32)
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                let msg = format!(
                    r#"{{"type":"meteor","id":{},"speed":{:.2},"frames":{},"first_frame":{},"last_frame":{},"trajectory":[{}],"ts":"{}"}}"#,
                    tr.id, speed, n, tr.first_frame, frame_index, traj, ts
                );
                log::info!(
                    "meteor confirmed: id={} frames={} speed={:.1}px/frame flux={} elong={:.1} len={:.0}px mzs={:.1} mzt={:.1} med={} mad={}",
                    tr.id,
                    n,
                    speed,
                    tr.best_streak.flux,
                    tr.best_streak.elongation,
                    tr.best_streak.length,
                    max_z_s,
                    max_z_t,
                    median,
                    mad
                );
                let _ = meteor_tx.send(msg.clone());
                let _ = debug_tx.send(msg);

                // Initialize brightest stack from pre-detection ring buffer
                if !stack_active {
                    init_stack_from_ring(
                        &mut stack_data,
                        &luma_ring,
                        luma_ring_full,
                        luma_ring_idx,
                        frame_size,
                    );
                    stack_active = true;
                    stack_ts = ts_dir.clone();
                    stack_countdown = STACK_TIMEOUT;
                }
            }
        }

        // While stacking: max-blend current luma, tick countdown
        if stack_active {
            unsafe {
                brightest(
                    luma.data.as_ptr(),
                    stack_data.as_ptr(),
                    stack_data.as_mut_ptr(),
                    frame_size,
                );
            }
            if tracks.iter().any(|tr| tr.confirmed && tr.missed == 0) {
                stack_countdown = STACK_TIMEOUT;
            } else {
                stack_countdown = stack_countdown.saturating_sub(1);
            }
            if stack_countdown == 0 {
                spawn_save_stack(stack_data.clone(), stack_ts.clone(), debug_tx.clone());
                stack_active = false;
            }
        }

        // Retire stale tracks. Short unconfirmed tracks (too few frames for
        // trajectory confirmation) get a post-mortem streak-morphology check:
        // a meteor smears into a line within a single exposure, so elongation
        // + length discriminate it from twinkle / hot pixels / strobes even
        // when the track never reached TRACK_MIN_FRAMES.
        let mut ti = 0;
        while ti < tracks.len() {
            if tracks[ti].missed <= TRACK_MAX_MISSED {
                ti += 1;
                continue;
            }
            let tr = tracks.swap_remove(ti);
            if tr.rejected {
                continue;
            }
            if tr.confirmed {
                // Final event with the full trajectory — the confirmation-time
                // event only carried the first few points.
                let n = tr.history.len();
                let (x0, y0) = tr.history[0];
                let (x1, y1) = tr.last();
                let dist = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt();
                let speed = dist * CELL_SIZE as f32 / n as f32;
                let traj = tr
                    .history
                    .iter()
                    .map(|(x, y)| {
                        format!("[{:.0},{:.0}]", x * CELL_SIZE as f32, y * CELL_SIZE as f32)
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                let msg = format!(
                    r#"{{"type":"meteor_end","id":{},"speed":{:.2},"frames":{},"first_frame":{},"last_frame":{},"trajectory":[{}],"ts":"{}"}}"#,
                    tr.id, speed, n, tr.first_frame, frame_index, traj, ts
                );
                let _ = meteor_tx.send(msg.clone());
                let _ = debug_tx.send(msg);
                continue;
            }
            if tr.history.len() > SHORT_MAX_FRAMES {
                continue;
            }
            let s = tr.best_streak;
            if s.flux < STREAK_MIN_FLUX || s.elongation < STREAK_MIN_ELONG || s.length < STREAK_MIN_LEN_PX.max(SHORT_MIN_LEN_PX) {
                continue;
            }
            if streak_near_edge(&s) {
                log::debug!("short streak rejected at the border: id={} c=({:.0},{:.0}) len={:.0}px", tr.id, s.cx, s.cy, s.length);
                continue;
            }

            // Trajectory from the streak's principal axis (px endpoints).
            let half = s.length / 2.0;
            let (ax0, ay0) = (s.cx - s.ux * half, s.cy - s.uy * half);
            let (ax1, ay1) = (s.cx + s.ux * half, s.cy + s.uy * half);
            let n = tr.history.len();
            let msg = format!(
                r#"{{"type":"meteor","id":{},"speed":{:.2},"frames":{},"first_frame":{},"last_frame":{},"trajectory":[[{:.0},{:.0}],[{:.0},{:.0}]],"ts":"{}","short":true,"elongation":{:.1},"length":{:.1}}}"#,
                tr.id,
                s.length / n as f32, // lower bound: streak length over visible frames
                n,
                tr.first_frame,
                tr.first_frame + (n as u32 - 1),
                ax0,
                ay0,
                ax1,
                ay1,
                ts,
                s.elongation,
                s.length
            );
            log::info!(
                "short meteor: id={} frames={} len={:.0}px elong={:.1} flux={}",
                tr.id,
                n,
                s.length,
                s.elongation,
                s.flux
            );
            let _ = meteor_tx.send(msg.clone());
            let _ = debug_tx.send(msg);

            // Ring depth (LUMA_PREBUF=8) still covers the meteor frames at
            // retirement time, so the stack init captures the event itself.
            if !stack_active {
                init_stack_from_ring(
                    &mut stack_data,
                    &luma_ring,
                    luma_ring_full,
                    luma_ring_idx,
                    frame_size,
                );
                stack_active = true;
                stack_ts = ts_dir.clone();
                stack_countdown = STACK_TIMEOUT;
            }
        }

        // Signal recording task. detection_active now also covers the
        // stacking window: short meteors are only recognized at retirement
        // (after the event ended), so without this the recorder would never
        // see them; for confirmed meteors it extends the clip by up to
        // STACK_TIMEOUT frames of trailing context.
        detection_tracking.store(!tracks.is_empty(), Ordering::Relaxed);
        let has_active = tracks.iter().any(|tr| tr.confirmed) || stack_active;
        detection_active.store(has_active, Ordering::Relaxed);

        // Send DetectionDebug JSON
        if debug_tx.receiver_count() > 0 {
            let b_json = merged_cells
                .iter()
                .map(|b| format!("[{},{},{},{}]", b.x, b.y, b.width, b.height))
                .collect::<Vec<_>>()
                .join(",");
            // tracks: [id, cx_px, cy_px, frame_count]
            let t_json = tracks
                .iter()
                .map(|tr| {
                    let (cx, cy) = tr.last();
                    format!(
                        "[{},{:.0},{:.0},{}]",
                        tr.id,
                        cx * CELL_SIZE as f32,
                        cy * CELL_SIZE as f32,
                        tr.history.len()
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            let c_json = confirmed_ids
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(",");
            // Streak principal-axis segments (px) for blobs that look line-like
            let l_json = streaks
                .iter()
                .filter(|s| {
                    s.flux >= STREAK_MIN_FLUX && s.elongation >= STREAK_MIN_ELONG && s.length >= STREAK_MIN_LEN_PX
                })
                .map(|s| {
                    let half = s.length / 2.0;
                    format!(
                        "[{:.0},{:.0},{:.0},{:.0}]",
                        s.cx - s.ux * half,
                        s.cy - s.uy * half,
                        s.cx + s.ux * half,
                        s.cy + s.uy * half
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            let msg = format!(
                r#"{{"type":"det","f":{},"b":[{}],"l":[{}],"t":[{}],"c":[{}],"mzs":{:.2},"mzt":{:.2}}}"#,
                frame_index, b_json, l_json, t_json, c_json, max_z_s, max_z_t
            );
            let _ = debug_tx.send(msg);
        }

        // Current frame becomes prev — a move, not a 225KB copy.
        prev_luma = Some(luma);

        elapsed_acc += t.elapsed().as_micros() as u64;
        frame_index = frame_index.wrapping_add(1);
        if frame_index.is_multiple_of(LOG_INTERVAL) {
            log::info!(
                "detection avg={:.2}ms/frame",
                elapsed_acc as f64 / LOG_INTERVAL as f64 / 1000.0
            );
            elapsed_acc = 0;
        }
    }
}
#[cfg(test)]
mod edge_tests {
    use super::*;

    /// Streak from (x0,y0) to (x1,y1), as the principal-axis segment.
    fn seg(x0: f32, y0: f32, x1: f32, y1: f32) -> StreakStats {
        let (dx, dy) = (x1 - x0, y1 - y0);
        let len = (dx * dx + dy * dy).sqrt();
        StreakStats { cx: (x0 + x1) / 2.0, cy: (y0 + y1) / 2.0, length: len, ux: dx / len, uy: dy / len, ..Default::default() }
    }

    /// The six false short events of 2026-11-14 (all at the top/left border).
    #[test]
    fn border_noise_is_rejected() {
        for (x0, y0, x1, y1) in [
            (4.0, 4.0, 21.0, 0.0),
            (1.0, 83.0, 1.0, 99.0),
            (-1.0, 19.0, 4.0, 1.0),
            (121.0, 1.0, 143.0, 0.0),
            (3.0, 3.0, 20.0, 0.0),
            (3.0, 6.0, 17.0, -2.0),
        ] {
            assert!(streak_near_edge(&seg(x0, y0, x1, y1)), "({x0},{y0})->({x1},{y1})");
        }
    }

    #[test]
    fn interior_and_other_borders() {
        assert!(!streak_near_edge(&seg(300.0, 180.0, 320.0, 190.0)));
        assert!(!streak_near_edge(&seg(100.0, 40.0, 130.0, 70.0)));
        // 7 px from the left border and 8 px from the top: kept
        assert!(!streak_near_edge(&seg(7.0, 8.0, 40.0, 60.0)));
        // bottom / right borders are covered too
        assert!(streak_near_edge(&seg(200.0, 340.0, 210.0, 359.0)));
        assert!(streak_near_edge(&seg(620.0, 100.0, 639.0, 110.0)));
    }
}

#[cfg(test)]
mod timing_tests {
    use super::*;

    #[test]
    fn reference_fps_is_unchanged() {
        let t = Timing::new(25);
        assert_eq!(t.ema_alpha, EMA_ALPHA);
        assert_eq!(t.max_event_frames, TRACK_MAX_EVENT_FRAMES);
        assert_eq!(t.min_speed_cells, TRACK_MIN_SPEED_CELLS);
        assert_eq!(t.max_speed_cells, TRACK_MAX_SPEED_CELLS);
    }

    #[test]
    fn lower_fps_keeps_real_time_values() {
        let t = Timing::new(15);
        // 3 s cap: 75 frames @ 25 fps -> 45 frames @ 15 fps
        assert_eq!(t.max_event_frames, 45);
        // same cells/s: 0.0625 * 25 = 1.5625 cells/s -> per frame at 15 fps
        assert!((t.min_speed_cells * 15.0 - TRACK_MIN_SPEED_CELLS * 25.0).abs() < 1e-5);
        // same EMA time constant: decay over one second matches
        let per_sec_ref = (1.0 - EMA_ALPHA).powi(25);
        let per_sec = (1.0 - t.ema_alpha).powi(15);
        assert!((per_sec - per_sec_ref).abs() < 1e-4);
    }
}

#[cfg(test)]
mod track_gate_tests {
    use super::*;

    fn track(points: &[(f32, f32)]) -> Track {
        let mut t = Track::new(1, points[0].0, points[0].1, 0, StreakStats::default());
        for &p in &points[1..] {
            t.push(p);
        }
        t
    }

    fn confirmed(points: &[(f32, f32)]) -> bool {
        let timing = Timing::new(25);
        track(points).is_confirmed(timing.min_speed_cells, timing.max_speed_cells)
    }

    #[test]
    fn ordinary_meteor_track_is_confirmed() {
        // ~1.8 cells/frame (14 px/frame), straight, well inside the image
        assert!(confirmed(&[(30.0, 20.0), (31.5, 21.0), (33.0, 22.0), (34.5, 23.0)]));
    }

    #[test]
    fn noise_chained_at_the_linking_limit_is_rejected() {
        // 5 cells/frame = 40 px/frame: what 735 noise tracks looked like on the dark night
        assert!(!confirmed(&[(30.0, 20.0), (35.0, 20.0), (40.0, 20.0), (45.0, 20.0)]));
    }

    #[test]
    fn track_along_the_border_is_rejected() {
        // runs down the left edge (x ~ 0.2 cells = 2 px)
        assert!(!confirmed(&[(0.2, 10.0), (0.2, 12.0), (0.1, 14.0), (0.1, 16.0)]));
    }
}
