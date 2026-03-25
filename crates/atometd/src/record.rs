use crate::config::SharedAppState;
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
                if !app_state.load().record_enabled {
                    // Close current file if recording was turned off
                    if let Some(muxer) = current_muxer.take() {
                        if let Err(e) = muxer.finish().await {
                            log::error!("Failed to finish muxer on disable: {}", e);
                        }
                        current_minute = 99;
                    }
                    continue;
                }

                let now = Local::now();
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

                    let path = format!("/media/mmc/records/{}.mp4", now.format("%Y%m%d_%H%M"));
                    log::info!("Switching to new record file: {}", path);

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

    log::info!("record_task shutting down");
}

pub async fn record_timelapse_task(
    shutdown: Arc<AtomicBool>,
    wd: WatchdogHandle,
    mut rx: mpsc::Receiver<Arc<VideoFrame>>,
) {
    let mut current_hour = 99;
    let mut current_muxer: Option<Mp4Muxer<BufWriter<File>>> = None;

    let mut wd_timer = interval(Duration::from_secs(1));

    while !shutdown.load(Ordering::Relaxed) {
        tokio::select! {
            Some(frame) = rx.recv() => {
                let now = Local::now();
                let is_new_hour = now.hour() != current_hour;
                let has_idr = frame.packs.iter().any(|pack| {
                    pack.nal_type == IMPEncoderH265NaluType_IMP_H265_NAL_SLICE_IDR_W_RADL
                        || pack.nal_type == IMPEncoderH265NaluType_IMP_H265_NAL_SLICE_IDR_N_LP // IDR_W_RADL or IDR_N_LP
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

                    let path = format!("/media/mmc/timelapse/{}.mp4", now.format("%Y%m%d_%H"));
                    log::info!("Switching to new record file: {}", path);

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

    log::info!("record_task shutting down");
}
