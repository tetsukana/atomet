use isvp_sys::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::watch;

use crate::watchdog::WatchdogHandle;

pub const WIDTH: usize = 640;
pub const HEIGHT: usize = 360;

/// A single luminance (Y-plane) frame from framesource channel 1.
/// `data` is Arc-wrapped so consumers receive it without copying.
#[derive(Clone)]
pub struct LumaFrame {
    pub data: Arc<Vec<u8>>, // WIDTH * HEIGHT bytes
    pub _idx: u64,
}

/// Poll Y-plane frames from ISP channel 1 and publish via watch channel.
///
/// Only the latest frame is retained; slow consumers simply see the newest
/// frame when they wake up (frames are never queued).
///
/// Must run inside `tokio::task::spawn_blocking`.
pub unsafe fn luma_poll_worker(
    shutdown: Arc<AtomicBool>,
    wd: WatchdogHandle,
    tx: watch::Sender<Option<LumaFrame>>,
) {
    let frame_size = WIDTH * HEIGHT;

    if unsafe { IMP_FrameSource_SetFrameDepth(1, 2) } < 0 {
        log::error!("luma: IMP_FrameSource_SetFrameDepth(1) failed");
        return;
    }

    let mut idx: u64 = 0;
    log::info!("Luma poll worker started ({}×{} Y-plane)", WIDTH, HEIGHT);

    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        wd.tick();

        let mut w: *mut IMPFrameInfo = std::ptr::null_mut();
        if unsafe { IMP_FrameSource_GetFrame(1, &mut w) } != 0 {
            continue;
        }

        // Copy Y-plane (first frame_size bytes of the NV12 buffer) into owned
        // heap storage before releasing the ISP buffer back to the driver.
        let data = unsafe {
            if idx == 0 {
                log::info!(
                    "luma frame info: width={} height={} size={} (expected {})",
                    (*w).width,
                    (*w).height,
                    (*w).size,
                    WIDTH * HEIGHT * 3 / 2
                );
            }
            let src = (*w).virAddr as *const u8;
            let mut buf = Vec::with_capacity(frame_size);
            buf.extend_from_slice(std::slice::from_raw_parts(src, frame_size));
            Arc::new(buf)
        };

        unsafe { IMP_FrameSource_ReleaseFrame(1, w) };

        let frame = LumaFrame { data, _idx: idx };
        idx = idx.wrapping_add(1);

        // Err only when every receiver has been dropped → exit cleanly
        if tx.send(Some(frame)).is_err() {
            break;
        }
    }
}
