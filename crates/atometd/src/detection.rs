use arc_swap::ArcSwap;
use imgproc::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{broadcast, watch};

use crate::config::SharedAppState;
use crate::luma;
use crate::watchdog::WatchdogHandle;

const INTERVAL: Duration = Duration::from_secs(1);

const IMG_WIDTH: usize = 640;
const IMG_HEIGHT: usize = 360;
const BLOB_MERGE_GAP: i32 = 2;

// Cell grid (8×8 px per cell → 80×45 cells)
const CELL_SIZE: usize = 8;
const CELLS_X: usize = IMG_WIDTH / CELL_SIZE; // 80
const CELLS_Y: usize = IMG_HEIGHT / CELL_SIZE; // 45

// Tuning — detection
const SPATIAL_THRESHOLD: f32 = 5.0; // spatial MAD z-score threshold (per-frame)
const TEMPORAL_THRESHOLD: f32 = 4.0; // temporal mean/stddev z-score threshold (per-cell EMA)
const EMA_ALPHA: f32 = 0.05; // EMA decay (time constant ≈ 20 frames)

// Stacking
const LUMA_PREBUF: usize = 5; // pre-detection luma frames to keep (for stack init)
const STACK_TIMEOUT: u32 = 15; // frames after last detection before saving stack
const SAVE_BASE: &str = "/media/mmc/detections";

// Tuning — tracker
const TRACK_MAX_DIST: f32 = 5.0; // max centroid distance to associate blob to track (cells)
const TRACK_MAX_MISSED: u32 = 2; // drop track after this many consecutive missed frames
const TRACK_MIN_FRAMES: usize = 4; // minimum history length to attempt confirmation
const TRACK_MIN_DISP_SQ: f32 = 4.0; // minimum displacement² (cells²) to confirm
const TRACK_MIN_LINEARITY: f32 = 8.0; // λ1/λ2 eigenvalue ratio for linear motion

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
// Spatial MAD z-score (histogram-based, O(n))
// ============================================================

/// Returns (median, MAD) of a u8 slice using histogram method.
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

fn median_mad(data: &[u8]) -> (u8, u8) {
    let mut hist = [0u32; 256];
    for &v in data {
        hist[v as usize] += 1;
    }

    let mid = data.len() / 2;

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
    for &v in data {
        let d = (v as i16 - median as i16).unsigned_abs() as usize;
        hist_dev[d] += 1;
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
// Tracker
// ============================================================

struct Track {
    id: u32,
    history: Vec<(f32, f32)>, // centroid history in cell coords
    missed: u32,
    confirmed: bool,
    first_frame: u32,
}

impl Track {
    fn new(id: u32, cx: f32, cy: f32, frame: u32) -> Self {
        Self {
            id,
            history: vec![(cx, cy)],
            missed: 0,
            confirmed: false,
            first_frame: frame,
        }
    }

    fn last(&self) -> (f32, f32) {
        *self.history.last().unwrap()
    }

    /// PCA on centroid history. Returns Some(linearity) if displacement and
    /// history length are sufficient.
    fn linearity(&self) -> Option<f32> {
        let n = self.history.len();
        if n < TRACK_MIN_FRAMES {
            return None;
        }

        // Check total displacement
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

        Some(lambda1 / lambda2)
    }

    fn is_confirmed(&self) -> bool {
        self.linearity().is_some_and(|l| l >= TRACK_MIN_LINEARITY)
    }
}

/// Greedy nearest-neighbour association. Returns matched (track_idx, centroid_idx) pairs,
/// plus unmatched centroid indices.
fn associate(tracks: &[Track], centroids: &[(f32, f32)]) -> (Vec<(usize, usize)>, Vec<usize>) {
    let mut matched: Vec<(usize, usize)> = Vec::new();
    let mut used_c = vec![false; centroids.len()];
    let mut used_t = vec![false; tracks.len()];

    // Build (dist², track_idx, centroid_idx) candidates
    let mut candidates: Vec<(f32, usize, usize)> = Vec::new();
    for (ti, track) in tracks.iter().enumerate() {
        let (tx, ty) = track.last();
        for (ci, &(cx, cy)) in centroids.iter().enumerate() {
            let d = (cx - tx).powi(2) + (cy - ty).powi(2);
            if d <= TRACK_MAX_DIST * TRACK_MAX_DIST {
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

pub fn detection_task(
    mut luma_rx: watch::Receiver<Option<luma::LumaFrame>>,
    app_state: SharedAppState,
    shutdown: Arc<AtomicBool>,
    wd: WatchdogHandle,
    debug_tx: broadcast::Sender<String>,
    mask: Arc<ArcSwap<Vec<u8>>>,
    detection_active: Arc<AtomicBool>,
    detection_tracking: Arc<AtomicBool>,
) {
    let handle = tokio::runtime::Handle::current();
    let frame_size = IMG_WIDTH * IMG_HEIGHT;

    let mut prev_frame = vec![0u8; frame_size];
    let mut diff_frame = vec![0u8; frame_size];
    let mut cell_map = vec![0u8; CELLS_X * CELLS_Y];
    let mut cell_visited = vec![0u8; CELLS_X * CELLS_Y];
    let mut cell_mean = vec![0f32; CELLS_X * CELLS_Y];
    let mut cell_var = vec![1f32; CELLS_X * CELLS_Y];
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

        if !app_state.load().detection_enabled {
            std::thread::sleep(INTERVAL);
            continue;
        }

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

        let t = std::time::Instant::now();
        let mask_snap = mask.load();

        unsafe {
            let frame = luma.data.as_ptr();
            let diff_ptr = diff_frame.as_mut_ptr();
            sub_saturate(frame, prev_frame.as_ptr(), diff_ptr, frame_size);

            // Bin to 80×45 cell map
            let cell_ptr = cell_map.as_mut_ptr();
            bin_8x8(diff_ptr as *const u8, IMG_WIDTH, IMG_HEIGHT, cell_ptr);

            // Spatial MAD z-score
            let (median, mad) = median_mad(&cell_map);
            let median_f = median as f32;
            let sigma_s = mad as f32 * 1.4826; // MAD → σ

            // Temporal EMA z-score — seed mean on first frame
            if frame_index == 0 {
                for i in 0..CELLS_X * CELLS_Y {
                    cell_mean[i] = cell_map[i] as f32;
                }
            }

            let mut max_z_s = 0f32;
            let mut max_z_t = 0f32;
            for i in 0..CELLS_X * CELLS_Y {
                let v = cell_map[i] as f32;

                let z_s = (v - median_f) / sigma_s.max(1.0);

                let mu = &mut cell_mean[i];
                let var = &mut cell_var[i];
                let delta = v - *mu;
                *mu += EMA_ALPHA * delta;
                let delta2 = v - *mu;
                *var = (1.0 - EMA_ALPHA) * (*var + EMA_ALPHA * delta * delta2);
                let z_t = delta / var.sqrt().max(1.0);

                if z_s > max_z_s {
                    max_z_s = z_s;
                }
                if z_t > max_z_t {
                    max_z_t = z_t;
                }

                cell_map[i] =
                    if mask_snap[i] == 0 && z_s > SPATIAL_THRESHOLD && z_t > TEMPORAL_THRESHOLD {
                        0xff
                    } else {
                        0
                    };
            }

            // Blob detection
            let cell_blobs =
                find_bounding_rectangles(&cell_map, &mut cell_visited, CELLS_X, CELLS_Y);
            let merged_cells = merge_blobs(&cell_blobs);

            // Centroids of current blobs (cell coords)
            let centroids: Vec<(f32, f32)> = merged_cells.iter().map(|b| b.centroid()).collect();

            // Associate blobs to tracks
            let (matched, unmatched_c) = associate(&tracks, &centroids);

            // Update matched tracks
            let mut matched_track_ids: Vec<bool> = vec![false; tracks.len()];
            for (ti, ci) in &matched {
                tracks[*ti].history.push(centroids[*ci]);
                tracks[*ti].missed = 0;
                matched_track_ids[*ti] = true;
            }

            // Increment missed counter for unmatched tracks
            for (ti, matched) in matched_track_ids.iter().enumerate() {
                if !matched {
                    tracks[ti].missed += 1;
                }
            }

            // Spawn new tracks for unmatched centroids
            for ci in unmatched_c {
                let (cx, cy) = centroids[ci];
                tracks.push(Track::new(next_track_id, cx, cy, frame_index));
                next_track_id = next_track_id.wrapping_add(1);
            }

            // Check for newly confirmed tracks and send meteor events
            let mut confirmed_ids: Vec<u32> = Vec::new();
            let now = chrono::Local::now();
            let ts = now.format("%Y%m%d_%H%M%S").to_string();
            let ts_dir = now.format("%Y/%m/%d/%H%M%S").to_string();
            for tr in tracks.iter_mut() {
                if tr.confirmed {
                    confirmed_ids.push(tr.id);
                    continue;
                }
                if tr.is_confirmed() {
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
                        "meteor confirmed: id={} frames={} speed={:.1}px/frame",
                        tr.id,
                        n,
                        speed
                    );
                    let _ = debug_tx.send(msg);

                    // Initialize brightest stack from pre-detection ring buffer
                    if !stack_active {
                        stack_data.fill(0);
                        let slots = if luma_ring_full {
                            LUMA_PREBUF
                        } else {
                            luma_ring_idx
                        };
                        for i in 0..slots {
                            let buf = &luma_ring[i * frame_size..(i + 1) * frame_size];
                            brightest(
                                buf.as_ptr(),
                                stack_data.as_ptr(),
                                stack_data.as_mut_ptr(),
                                frame_size,
                            );
                        }
                        stack_active = true;
                        stack_ts = ts_dir.clone();
                        stack_countdown = STACK_TIMEOUT;
                    }
                }
            }

            // While stacking: max-blend current luma, tick countdown
            if stack_active {
                brightest(
                    frame,
                    stack_data.as_ptr(),
                    stack_data.as_mut_ptr(),
                    frame_size,
                );
                if tracks.iter().any(|tr| tr.confirmed && tr.missed == 0) {
                    stack_countdown = STACK_TIMEOUT;
                } else {
                    stack_countdown -= 1;
                }
                if stack_countdown == 0 {
                    let path = format!("{}/{}/stack.png", SAVE_BASE, stack_ts);
                    if let Err(e) = save_png(&stack_data, IMG_WIDTH, IMG_HEIGHT, &path) {
                        log::error!("failed to save stack: {}", e);
                    } else {
                        log::info!("saved detection stack: {}", path);
                        let msg = format!(
                            r#"{{"type":"meteor_stack","path":"{}","ts":"{}"}}"#,
                            path, stack_ts.replace('/', "")
                        );
                        let _ = debug_tx.send(msg);
                    }
                    stack_active = false;
                }
            }

            tracks.retain(|tr| tr.missed <= TRACK_MAX_MISSED);

            // Signal recording task
            detection_tracking.store(!tracks.is_empty(), Ordering::Relaxed);
            let has_active = tracks.iter().any(|tr| tr.confirmed);
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
                let msg = format!(
                    r#"{{"type":"det","f":{},"b":[{}],"l":[],"t":[{}],"c":[{}],"mzs":{:.2},"mzt":{:.2}}}"#,
                    frame_index, b_json, t_json, c_json, max_z_s, max_z_t
                );
                let _ = debug_tx.send(msg);
            }

            std::ptr::copy_nonoverlapping(frame, prev_frame.as_mut_ptr(), frame_size);
        }

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
