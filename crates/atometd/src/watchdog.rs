use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// --- Hardware Watchdog (Linux only) ---

#[cfg(target_os = "linux")]
mod hw {
    use std::fs::File;
    use std::os::fd::AsRawFd;

    const WDIOC_SETOPTIONS: libc::c_ulong = 0x40045704;
    const WDIOC_KEEPALIVE: libc::c_ulong = 0x40045705;
    const WDIOC_SETTIMEOUT: libc::c_ulong = 0xc0045706;
    const WDIOS_ENABLECARD: libc::c_int = 0x0002;

    pub struct HwWatchdog {
        fd: libc::c_int,
        _file: File,
    }

    impl HwWatchdog {
        pub fn init(timeout_secs: libc::c_int) -> std::io::Result<Self> {
            let file = File::open("/dev/watchdog")?;
            let fd = file.as_raw_fd();
            unsafe {
                let option_ptr: *const libc::c_int = &WDIOS_ENABLECARD;
                if libc::ioctl(fd, WDIOC_SETOPTIONS, option_ptr) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                let timeout_ptr: *const libc::c_int = &timeout_secs;
                if libc::ioctl(fd, WDIOC_SETTIMEOUT, timeout_ptr) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::ioctl(fd, WDIOC_KEEPALIVE, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(Self { fd, _file: file })
        }

        pub fn feed(&self) -> std::io::Result<()> {
            unsafe {
                if libc::ioctl(self.fd, WDIOC_KEEPALIVE, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        }
    }
}

#[cfg(target_os = "linux")]
pub use hw::HwWatchdog;

#[cfg(not(target_os = "linux"))]
pub struct HwWatchdog;

#[cfg(not(target_os = "linux"))]
impl HwWatchdog {
    pub fn init(_timeout_secs: i32) -> std::io::Result<Self> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Hardware watchdog not available on this platform",
        ))
    }

    pub fn feed(&self) -> std::io::Result<()> {
        Ok(())
    }
}

// --- Thread Watchdog ---

fn now_millis() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u32
}

#[derive(Clone)]
pub struct WatchdogHandle {
    last_tick: Arc<AtomicU32>,
    pub name: &'static str,
}

impl WatchdogHandle {
    pub fn new(name: &'static str) -> Self {
        Self {
            last_tick: Arc::new(AtomicU32::new(now_millis())),
            name,
        }
    }

    /// Call periodically from the monitored task to signal liveness.
    pub fn tick(&self) {
        self.last_tick.store(now_millis(), Ordering::Relaxed);
    }

    fn elapsed_ms(&self) -> u32 {
        now_millis().saturating_sub(self.last_tick.load(Ordering::Relaxed))
    }
}

pub struct WatchdogSupervisor {
    handles: Vec<WatchdogHandle>,
    timeout: Duration,
    hw_watchdog: Option<HwWatchdog>,
}

impl WatchdogSupervisor {
    pub fn new(timeout: Duration, hw_watchdog: Option<HwWatchdog>) -> Self {
        Self {
            handles: Vec::new(),
            timeout,
            hw_watchdog,
        }
    }

    /// Create a new handle for a task. The handle must be ticked periodically.
    pub fn register(&mut self, name: &'static str) -> WatchdogHandle {
        let handle = WatchdogHandle::new(name);
        self.handles.push(handle.clone());
        handle
    }

    /// Run the supervisor loop. Checks all handles and feeds HW watchdog when healthy.
    pub async fn run(self, shutdown: Arc<AtomicBool>) {
        let check_interval = self.timeout / 2;
        loop {
            if shutdown.load(Ordering::Relaxed) {
                log::info!("Watchdog supervisor shutting down");
                break;
            }

            let mut all_healthy = true;
            for handle in &self.handles {
                let elapsed = handle.elapsed_ms();
                if elapsed > self.timeout.as_millis() as u32 {
                    log::error!(
                        "Watchdog timeout: task '{}' has not ticked for {}ms (limit: {}ms)",
                        handle.name,
                        elapsed,
                        self.timeout.as_millis()
                    );
                    all_healthy = false;
                }
            }

            if all_healthy {
                if let Some(ref hw) = self.hw_watchdog
                    && let Err(e) = hw.feed()
                {
                    log::error!("Failed to feed hardware watchdog: {}", e);
                }
            } else {
                log::error!("Not feeding hardware watchdog due to unhealthy tasks");
            }

            tokio::time::sleep(check_interval).await;
        }
    }
}
