use crate::ae_ctrl::AeParams;
use crate::config::{IspInfo, SystemInfo};
use isvp_sys::{
    IMP_ISP_Tuning_GetAeHist_Origin, IMP_ISP_Tuning_GetEVAttr, IMPISPAEHistOrigin, IMPISPEVAttr,
};
use std::mem::MaybeUninit;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::broadcast;
use tokio::time::{Duration, interval};

/// Reboot the device.
pub fn reboot() {
    log::info!("Rebooting system...");
    let _ = std::process::Command::new("reboot").spawn();
}

// -----------------------------------------------------------------------------
// /proc readers
// -----------------------------------------------------------------------------

/// Returns (idle_jiffies, total_jiffies) from the first line of /proc/stat.
fn read_cpu_stat() -> (u64, u64) {
    let Ok(content) = std::fs::read_to_string("/proc/stat") else {
        return (0, 0);
    };
    let line = content.lines().next().unwrap_or("");
    // cpu  user nice system idle iowait irq softirq steal ...
    let fields: Vec<u64> = line
        .split_whitespace()
        .skip(1) // skip "cpu"
        .filter_map(|s| s.parse().ok())
        .collect();
    if fields.len() < 4 {
        return (0, 0);
    }
    let idle = fields[3] + fields.get(4).copied().unwrap_or(0); // idle + iowait
    let total = fields.iter().sum();
    (idle, total)
}

fn parse_meminfo_field(content: &str, field: &str) -> u64 {
    content
        .lines()
        .find(|l| l.starts_with(field))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

pub fn uptime_secs() -> f64 {
    std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next().map(String::from))
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0)
}

/// Read ISP exposure values: (expr_us, analog_gain, digital_gain).
/// Gain values are in the ISP's internal fixed-point format.
fn read_ev_attr() -> (u32, u32, u32) {
    unsafe {
        let mut ev = MaybeUninit::<IMPISPEVAttr>::zeroed();
        if IMP_ISP_Tuning_GetEVAttr(ev.as_mut_ptr()) == 0 {
            let ev = ev.assume_init();
            (ev.expr_us, ev.again, ev.dgain)
        } else {
            (0, 0, 0)
        }
    }
}

fn read_ae_attr() -> AeParams {
    let dev = crate::ae_ctrl::AtometDev::open().unwrap();
    dev.get_ae().unwrap()
}

fn read_histogram() -> [u32; 256] {
    unsafe {
        let mut hist = MaybeUninit::<IMPISPAEHistOrigin>::zeroed();
        if IMP_ISP_Tuning_GetAeHist_Origin(hist.as_mut_ptr()) == 0 {
            hist.assume_init().ae_hist
        } else {
            [0u32; 256]
        }
    }
}

// -----------------------------------------------------------------------------
// Broadcast task
// -----------------------------------------------------------------------------

/// Reads CPU + memory + ISP once per second, updates SharedRuntimeState,
/// and broadcasts a JSON sysstat message to all connected WebSocket clients.
pub async fn sysstat_broadcast_task(tx: broadcast::Sender<String>, shutdown: Arc<AtomicBool>) {
    let mut ticker = interval(Duration::from_secs(1));
    let mut prev_idle: u64 = 0;
    let mut prev_total: u64 = 0;
    let mut sys_info = SystemInfo::default();

    loop {
        ticker.tick().await;
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        // CPU usage
        let (idle, total) = read_cpu_stat();
        let cpu_pct = if prev_total > 0 && total > prev_total {
            let diff_total = total - prev_total;
            let diff_idle = idle - prev_idle;
            100.0 * (1.0 - diff_idle as f32 / diff_total as f32)
        } else {
            0.0
        };
        prev_idle = idle;
        prev_total = total;

        // Memory
        let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
        let mem_total = parse_meminfo_field(&meminfo, "MemTotal:");
        let mem_avail = {
            let a = parse_meminfo_field(&meminfo, "MemAvailable:");
            if a > 0 {
                a
            } else {
                parse_meminfo_field(&meminfo, "MemFree:")
                    + parse_meminfo_field(&meminfo, "Cached:")
                    + parse_meminfo_field(&meminfo, "Buffers:")
            }
        };
        let mem_used = mem_total.saturating_sub(mem_avail);

        sys_info.cpu = (cpu_pct * 10.0).round() / 10.0;
        sys_info.mem_used = mem_used;
        sys_info.mem_total = mem_total;
        sys_info.uptime = uptime_secs() as u64;
        // Update shared debug info

        let json = serde_json::json!({
            "type": "sysstat",
            "data": &sys_info,
        });

        let _ = tx.send(json.to_string());
    }
}

pub async fn ispstat_broadcast_task(tx: broadcast::Sender<String>, shutdown: Arc<AtomicBool>) {
    let mut ticker = interval(Duration::from_millis(250));
    let mut isp_info = IspInfo::default();

    loop {
        ticker.tick().await;
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        // ISP exposure values
        let ae = read_ae_attr();

        let (_expr, again, dgain) = read_ev_attr();

        let histogram = read_histogram();

        isp_info.ae_mode = ae.ae_mode;
        isp_info.it = ae.it_value;
        isp_info.ag = ae.ag_value;
        isp_info.ag_i = again;
        isp_info.sdg = ae.sdg_value;
        isp_info.idg = ae.idg_value;
        isp_info.idg_i = dgain;
        isp_info.max_it = ae.max_it;
        isp_info.max_ag = ae.max_ag;
        isp_info.max_sdg = ae.max_sdg;
        isp_info.max_idg = ae.max_idg;
        isp_info.min_it = ae.min_it;
        isp_info.min_ag = ae.min_ag;
        isp_info.min_sdg = ae.min_sdg;
        isp_info.min_idg = ae.min_idg;
        isp_info.fps_actual = 0;
        isp_info.histogram = histogram.to_vec();

        let json = serde_json::json!({
            "type": "ispstat",
            "data": &isp_info,
        });

        let _ = tx.send(json.to_string());
    }
}
