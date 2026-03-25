use isvp_sys::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio::time::{Duration, interval};

use crate::isp::VideoFrame;
use crate::watchdog::WatchdogHandle;

struct TcpClient {
    stream: TcpStream,
    initialized: bool,
}

// TCP server that broadcasts raw H.265 frames to connected clients for Go2RTC streaming.
pub async fn broadcast_task(
    shutdown: Arc<AtomicBool>,
    wd: WatchdogHandle,
    mut rx: broadcast::Receiver<Arc<VideoFrame>>,
) {
    let listener = TcpListener::bind("127.0.0.1:12345")
        .await
        .expect("Failed to bind TCP listener");

    let mut clients: Vec<TcpClient> = Vec::new();
    let mut wd_timer = interval(Duration::from_secs(1));

    while !shutdown.load(Ordering::Relaxed) {
        tokio::select! {
            Ok((stream, addr)) = listener.accept() => {
                log::info!("TCP Client connected: {}", addr);
                clients.push(TcpClient { stream, initialized: false });
            }

            Ok(frame) = rx.recv() => {

                let has_idr = frame.packs.iter().any(|pack| {
                    pack.nal_type == IMPEncoderH265NaluType_IMP_H265_NAL_SLICE_IDR_W_RADL
                        || pack.nal_type == IMPEncoderH265NaluType_IMP_H265_NAL_SLICE_IDR_N_LP // IDR_W_RADL or IDR_N_LP
                });

                let mut closed_clients = Vec::new();
                for pack in &frame.packs {
                    for (i, client) in clients.iter_mut().enumerate() {
                        if closed_clients.contains(&i) {
                            continue;
                        }

                        if !client.initialized {
                            if has_idr {
                                client.initialized = true;
                            } else {
                                continue; // Skip until we see an IDR frame
                            }
                        }

                        match tokio::time::timeout(
                            Duration::from_millis(50),
                            client.stream.write_all(&pack.data)
                        ).await {
                            Err(_) => {
                                log::warn!("Client {} timed out", i);
                                closed_clients.push(i);
                            }
                            Ok(Err(e)) => {
                                log::warn!("Client {} error: {}", i, e);
                                closed_clients.push(i);
                            }
                            Ok(Ok(_)) => {}
                        }
                    }
                }

                // Remove closed clients
                for i in closed_clients.iter().rev() {
                    clients.remove(*i);
                }
            }

            _ = wd_timer.tick() => {
                wd.tick();
            }

            _ = tokio::time::sleep(Duration::from_millis(100)), if shutdown.load(Ordering::Relaxed) => {
                break;
            }
        }
    }
}
