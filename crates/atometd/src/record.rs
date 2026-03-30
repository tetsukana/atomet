use crate::config::{SharedAppState, is_within_schedule};
use crate::isp::VideoFrame;
use crate::muxer::Mp4Muxer;
use crate::watchdog::WatchdogHandle;
use chrono::{Local, Timelike};
use isvp_sys::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::fs::File;
use tokio::io::BufWriter;
use tokio::sync::{broadcast, mpsc};
use tokio::time::{Duration, interval};

pub async fn record_regular_task(
    shutdown: Arc<AtomicBool>,
    wd: WatchdogHandle,
    mut rx: broadcast::Receiver<Arc<VideoFrame>>,
    app_state: SharedAppState,
) {
    log::info!("record_task started");
    let mut current_minute = 99;
    let mut current_muxer: Option<Mp4Muxer<BufWriter<File>>> = None;

    let mut wd_timer = interval(Duration::from_secs(1));

    while !shutdown.load(Ordering::Relaxed) {
        tokio::select! {
            Ok(frame) = rx.recv() => {
                let now = Local::now();
                let state = app_state.load();
                let active = state.record_enabled
                    && is_within_schedule(now.hour(), state.record_start_hour, state.record_end_hour);

                if !active {
                    // Close current file if recording is off or outside schedule
                    if let Some(muxer) = current_muxer.take() {
                        if let Err(e) = muxer.finish().await {
                            log::error!("Failed to finish muxer on disable: {}", e);
                        }
                        current_minute = 99;
                    }
                    continue;
                }
                let is_new_minute = now.minute() != current_minute;
                let has_idr = frame.packs.iter().any(|pack| {
                    pack.nal_type == IMPEncoderH265NaluType_IMP_H265_NAL_SLICE_IDR_W_RADL
                        || pack.nal_type == IMPEncoderH265NaluType_IMP_H265_NAL_SLICE_IDR_N_LP // IDR_W_RADL or IDR_N_LP
                });

                if (current_muxer.is_none() || is_new_minute) && has_idr {
                    if let Some(muxer) = current_muxer.take()
                        && let Err(e) = muxer.finish().await
                    {
                        log::error!("Failed to finish muxer: {}", e);
                    }

                    let dir = format!("/media/mmc/records/{}", now.format("%Y/%m/%d/%H"));
                    let path = format!("{}/{}.mp4", dir, now.format("%M"));
                    log::info!("Switching to new record file: {}", path);

                    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
                        log::error!("Failed to create dir '{}': {}", dir, e);
                        continue;
                    }

                    match File::create(&path).await {
                        Ok(file) => {
                            let writer = BufWriter::new(file);
                            match Mp4Muxer::new(writer, 1920, 1080, 90000).await {
                                Ok(muxer) => {
                                    current_muxer = Some(muxer);
                                    current_minute = now.minute();
                                }
                                Err(e) => log::error!("Failed to create muxer: {}", e),
                            }
                        }
                        Err(e) => {
                            log::error!("Failed to create record file '{}': {}", path, e);
                            current_muxer = None;
                        }
                    }
                }

                if let Some(muxer) = current_muxer.as_mut() {
                    let combined: Vec<u8> = frame
                        .packs
                        .iter()
                        .flat_map(|p| p.data.iter().copied())
                        .collect();

                    let frame_duration: u32 = 90000 / 25; // 25fps, timescale=90000

                    if let Err(e) = muxer.write_video(&combined, frame_duration, has_idr).await {
                        log::error!("Failed to write video frame: {}", e);
                    }
                } else {
                    log::warn!("No active muxer, dropping frame");
                }
            }

            _ = wd_timer.tick() => {
                wd.tick();
            }
        }
    }

    log::info!("record_regular_task shutting down");
}

/// Detection-triggered recording task.
///
/// Maintains the current GOP as a pre-buffer.  The buffer only resets
/// on IDR boundaries when no track is active and we are not recording.
/// This ensures the meteor start frame is always captured: tracking
/// begins before confirmation, and the pre-buffer spans the full
/// tracking period.  When `detection_active` becomes true, the buffer
/// is flushed to a new MP4 and subsequent frames are written live.
pub async fn record_detection_task(
    shutdown: Arc<AtomicBool>,
    wd: WatchdogHandle,
    mut rx: broadcast::Receiver<Arc<VideoFrame>>,
    detection_active: Arc<AtomicBool>,
    detection_tracking: Arc<AtomicBool>,
    app_state: SharedAppState,
) {
    log::info!("record_detection_task started");

    // Pre-buffer: accumulates frames from current GOP onward.
    // Only cleared on IDR when not tracking and not recording.
    let mut prebuf: Vec<Arc<VideoFrame>> = Vec::new();

    let mut current_muxer: Option<Mp4Muxer<BufWriter<File>>> = None;
    let mut recording = false;
    let mut tailing = false;

    let mut wd_timer = interval(Duration::from_secs(1));

    while !shutdown.load(Ordering::Relaxed) {
        tokio::select! {
            Ok(frame) = rx.recv() => {
                let has_idr = frame.packs.iter().any(|pack| {
                    pack.nal_type == IMPEncoderH265NaluType_IMP_H265_NAL_SLICE_IDR_W_RADL
                        || pack.nal_type == IMPEncoderH265NaluType_IMP_H265_NAL_SLICE_IDR_N_LP
                });

                let tracking = detection_tracking.load(Ordering::Relaxed);

                let now = Local::now();
                let state = app_state.load();
                let det = state.detection_record_enabled
                    && is_within_schedule(now.hour(), state.detection_record_start_hour, state.detection_record_end_hour)
                    && detection_active.load(Ordering::Relaxed);

                // --- Pre-buffer maintenance ---
                // Only reset on IDR when idle (no tracking, no recording)
                if has_idr && !tracking && !recording {
                    prebuf.clear();
                }
                if !recording {
                    prebuf.push(Arc::clone(&frame));
                }

                // --- State transitions ---
                if !recording && det {
                    // Detection confirmed — open file and flush pre-buffer
                    let now = Local::now();
                    let dir = format!("/media/mmc/detections/{}", now.format("%Y/%m/%d/%H%M%S"));
                    let path = format!("{}/video.mp4", dir);
                    log::info!("Detection recording start: {}", path);

                    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
                        log::error!("Failed to create dir '{}': {}", dir, e);
                        continue;
                    }

                    match File::create(&path).await {
                        Ok(file) => {
                            let writer = BufWriter::new(file);
                            match Mp4Muxer::new(writer, 1920, 1080, 90000).await {
                                Ok(mut muxer) => {
                                    for f in prebuf.drain(..) {
                                        if let Err(e) = write_frame_to_muxer(&mut muxer, &f).await {
                                            log::error!("Failed to write buffered frame: {}", e);
                                        }
                                    }
                                    current_muxer = Some(muxer);
                                    recording = true;
                                    tailing = false;
                                }
                                Err(e) => log::error!("Failed to create muxer: {}", e),
                            }
                        }
                        Err(e) => log::error!("Failed to create detection file '{}': {}", path, e),
                    }
                } else if recording {
                    if let Some(muxer) = current_muxer.as_mut() {
                        if let Err(e) = write_frame_to_muxer(muxer, &frame).await {
                            log::error!("Failed to write video frame: {}", e);
                        }
                    }

                    if det {
                        tailing = false;
                    } else if !tailing {
                        tailing = true;
                    }

                    if tailing && has_idr {
                        if let Some(muxer) = current_muxer.take() {
                            if let Err(e) = muxer.finish().await {
                                log::error!("Failed to finish detection muxer: {}", e);
                            }
                            log::info!("Detection recording finished");
                        }
                        recording = false;
                        tailing = false;
                    }
                }
            }

            _ = wd_timer.tick() => {
                wd.tick();
            }
        }
    }

    if let Some(muxer) = current_muxer.take() {
        if let Err(e) = muxer.finish().await {
            log::error!("Failed to finish detection muxer on shutdown: {}", e);
        }
    }

    log::info!("record_detection_task shutting down");
}

async fn write_frame_to_muxer(
    muxer: &mut Mp4Muxer<BufWriter<File>>,
    frame: &VideoFrame,
) -> std::io::Result<()> {
    let combined: Vec<u8> = frame
        .packs
        .iter()
        .flat_map(|p| p.data.iter().copied())
        .collect();
    let has_idr = frame.packs.iter().any(|pack| {
        pack.nal_type == IMPEncoderH265NaluType_IMP_H265_NAL_SLICE_IDR_W_RADL
            || pack.nal_type == IMPEncoderH265NaluType_IMP_H265_NAL_SLICE_IDR_N_LP
    });
    let frame_duration: u32 = 90000 / 25;
    muxer.write_video(&combined, frame_duration, has_idr).await
}

pub async fn record_timelapse_task(
    shutdown: Arc<AtomicBool>,
    wd: WatchdogHandle,
    mut rx: mpsc::Receiver<Arc<VideoFrame>>,
    app_state: SharedAppState,
) {
    let mut current_hour = 99;
    let mut current_muxer: Option<Mp4Muxer<BufWriter<File>>> = None;

    let mut wd_timer = interval(Duration::from_secs(1));

    while !shutdown.load(Ordering::Relaxed) {
        tokio::select! {
            Some(frame) = rx.recv() => {
                let now = Local::now();
                let state = app_state.load();
                let active = state.timelapse_enabled
                    && is_within_schedule(now.hour(), state.timelapse_start_hour, state.timelapse_end_hour);

                if !active {
                    if let Some(muxer) = current_muxer.take() {
                        if let Err(e) = muxer.finish().await {
                            log::error!("Failed to finish muxer on disable: {}", e);
                        }
                        current_hour = 99;
                    }
                    continue;
                }
                let is_new_hour = now.hour() != current_hour;
                let has_idr = frame.packs.iter().any(|pack| {
                    pack.nal_type == IMPEncoderH265NaluType_IMP_H265_NAL_SLICE_IDR_W_RADL
                        || pack.nal_type == IMPEncoderH265NaluType_IMP_H265_NAL_SLICE_IDR_N_LP
                });

                if !has_idr {
                    continue;
                }

                if current_muxer.is_none() || is_new_hour {
                    if let Some(muxer) = current_muxer.take()
                        && let Err(e) = muxer.finish().await
                    {
                        log::error!("Failed to finish muxer: {}", e);
                    }

                    let dir = format!("/media/mmc/timelapse/{}", now.format("%Y/%m/%d"));
                    let path = format!("{}/{}.mp4", dir, now.format("%H"));
                    log::info!("Switching to new timelapse file: {}", path);

                    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
                        log::error!("Failed to create dir '{}': {}", dir, e);
                        continue;
                    }

                    match File::create(&path).await {
                        Ok(file) => {
                            let writer = BufWriter::new(file);
                            match Mp4Muxer::new(writer, 1920, 1080, 90000).await {
                                Ok(muxer) => {
                                    current_muxer = Some(muxer);
                                    current_hour = now.hour();
                                }
                                Err(e) => log::error!("Failed to create muxer: {}", e),
                            }
                        }
                        Err(e) => {
                            log::error!("Failed to create record file '{}': {}", path, e);
                            current_muxer = None;
                        }
                    }
                }

                if let Some(muxer) = current_muxer.as_mut() {
                    let combined: Vec<u8> = frame
                        .packs
                        .iter()
                        .flat_map(|p| p.data.iter().copied())
                        .collect();

                    let frame_duration: u32 = 90000 / 30; // 30fps, timescale=90000

                    if let Err(e) = muxer.write_video(&combined, frame_duration, has_idr).await {
                        log::error!("Failed to write video frame: {}", e);
                    }
                } else {
                    log::warn!("No active muxer, dropping frame");
                }
            }

            _ = wd_timer.tick() => {
                wd.tick();
            }
        }
    }

    log::info!("timelapse_task shutting down");
}

// ============================================================
// Disk space cleanup
// ============================================================

const MMC_PATH: &str = "/media/mmc";
const FREE_THRESHOLD_BYTES: u64 = 1024 * 1024 * 1024; // 1 GB
const CLEANUP_INTERVAL_SECS: u64 = 60;

/// Deletion priority: regular → timelapse → detection
const CLEANUP_DIRS: &[&str] = &[
    "/media/mmc/records",
    "/media/mmc/timelapse",
    "/media/mmc/detections",
];

/// Periodic task that deletes the oldest date-directories when free space
/// drops below 1 GB.  Deletes regular recordings first, then timelapse,
/// then detections.
pub async fn disk_cleanup_task(shutdown: Arc<AtomicBool>) {
    log::info!("disk_cleanup_task started");
    let mut timer = interval(Duration::from_secs(CLEANUP_INTERVAL_SECS));

    while !shutdown.load(Ordering::Relaxed) {
        timer.tick().await;

        let free = match free_space_bytes(MMC_PATH) {
            Some(f) => f,
            None => continue,
        };

        if free >= FREE_THRESHOLD_BYTES {
            continue;
        }

        log::warn!(
            "Low disk space: {} MB free, threshold {} MB",
            free / (1024 * 1024),
            FREE_THRESHOLD_BYTES / (1024 * 1024),
        );

        for base_dir in CLEANUP_DIRS {
            if delete_oldest_leaf(base_dir).await {
                // Deleted something — recheck on next tick
                break;
            }
        }
    }

    log::info!("disk_cleanup_task shutting down");
}

/// Returns free bytes on the filesystem containing `path`, or None on error.
fn free_space_bytes(path: &str) -> Option<u64> {
    use std::ffi::CString;
    let c_path = CString::new(path).ok()?;
    unsafe {
        let mut stat: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c_path.as_ptr(), &mut stat) == 0 {
            Some(stat.f_bfree as u64 * stat.f_bsize as u64)
        } else {
            None
        }
    }
}

/// Walk into a date-hierarchy directory (YYYY/MM/DD/…) and delete the
/// oldest leaf directory (or file).  Returns true if something was deleted.
async fn delete_oldest_leaf(base: &str) -> bool {
    let oldest = match find_oldest_entry(base).await {
        Some(p) => p,
        None => return false,
    };

    log::info!("Cleanup: removing {}", oldest.display());
    if oldest.is_dir() {
        if let Err(e) = tokio::fs::remove_dir_all(&oldest).await {
            log::error!("Failed to remove '{}': {}", oldest.display(), e);
            return false;
        }
    } else if let Err(e) = tokio::fs::remove_file(&oldest).await {
        log::error!("Failed to remove '{}': {}", oldest.display(), e);
        return false;
    }

    // Clean up empty parent directories back to base
    let mut dir = oldest.parent().map(|p| p.to_path_buf());
    while let Some(d) = dir {
        if d == std::path::Path::new(base) {
            break;
        }
        // remove_dir fails if not empty, which is what we want
        if tokio::fs::remove_dir(&d).await.is_ok() {
            log::info!("Cleanup: removed empty dir {}", d.display());
            dir = d.parent().map(|p| p.to_path_buf());
        } else {
            break;
        }
    }

    true
}

/// Find the lexicographically smallest (= oldest) entry by walking sorted
/// directory entries at each level of the date hierarchy.
async fn find_oldest_entry(base: &str) -> Option<std::path::PathBuf> {
    let mut current = std::path::PathBuf::from(base);

    loop {
        let mut entries = match tokio::fs::read_dir(&current).await {
            Ok(rd) => rd,
            Err(_) => return None,
        };

        let mut children: Vec<std::path::PathBuf> = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            children.push(entry.path());
        }

        if children.is_empty() {
            return None;
        }

        children.sort();
        let first = &children[0];

        // If the first child is a directory that looks like a date component
        // (all digits, 2-6 chars), descend further
        let name = first.file_name()?.to_str()?;
        if first.is_dir() && name.len() <= 6 && name.chars().all(|c| c.is_ascii_digit()) {
            current = first.clone();
        } else {
            // Reached a leaf — return this entry
            return Some(first.clone());
        }
    }
}
