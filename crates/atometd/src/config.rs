use crate::paths;
use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

/// Full application state — used at runtime by all tasks and the WebUI.
/// Only the `PersistConfig` subset is saved to disk; the rest resets on reboot.
#[derive(Serialize, Debug, Clone)]
pub struct AppState {
    // --- Volatile (reset on reboot) ---
    pub fps: u32,

    pub ae_enable: bool,
    /// Manual exposure in microseconds. 0 = auto AE.
    /// Max at 25fps ≈ 40000us (one frame period).
    pub exposure_us: u32,
    /// Manual analog gain ×1024. 0 = auto/minimum (1x).
    /// gc2053 max ≈ 15872 (~15.5x).
    pub analog_gain: u32,
    /// Manual digital gain. 0 = auto. ISP internal fixed-point.
    pub digital_gain: u32,

    // --- Persisted (survives reboot) ---
    pub night_mode: bool,
    pub ircut_on: bool,
    pub led_on: bool,
    pub irled_on: bool,
    pub record_enabled: bool,
    pub record_start_hour: u32,
    pub record_end_hour: u32,
    pub timelapse_enabled: bool,
    pub timelapse_start_hour: u32,
    pub timelapse_end_hour: u32,
    pub detection_record_enabled: bool,
    pub detection_enabled: bool,
    pub detection_start_hour: u32,
    pub detection_end_hour: u32,
    /// Automatic star extraction + plate solving on FF blocks. Gated by the
    /// same kind of schedule as detection (hours, civil or astro night).
    pub solver_enabled: bool,
    pub solver_start_hour: u32,
    pub solver_end_hour: u32,
    pub solver_use_twilight: bool,
    pub solver_twilight_astro: bool,
    pub auto_daynight: bool,
    pub show_timestamp: bool,
    pub show_watermark: bool,
    pub timestamp_position: u32,
    pub webhook_discord_url: String,
    pub webhook_slack_url: String,
    pub webhook_generic_url: String,
    pub webhook_notify_startup: bool,
    pub webhook_notify_detection_start: bool,
    pub webhook_notify_detection_end: bool,
    /// POSIX TZ string (e.g. "JST-9", "UTC0", "EST5EDT,M3.2.0,M11.1.0").
    /// Default = Japan.
    pub timezone: String,
    /// Observer latitude in degrees (north +). Used for civil-twilight scheduling.
    pub latitude: f64,
    /// Observer longitude in degrees (east +).
    pub longitude: f64,
    /// If true, the corresponding feature ignores start/end hours and is active
    /// only during civil night (sun altitude ≤ -6°, i.e. between evening civil
    /// dusk and morning civil dawn).
    pub record_use_twilight: bool,
    pub timelapse_use_twilight: bool,
    pub detection_use_twilight: bool,
    /// When true (and use_twilight is also true), use astronomical-night
    /// threshold (sun ≤ -18°) with civil-night fallback on high-latitude
    /// summer days where astro night never occurs.
    pub record_twilight_astro: bool,
    pub timelapse_twilight_astro: bool,
    pub detection_twilight_astro: bool,
    /// If true, atometd triggers a `reboot` at `scheduled_reboot_hour:scheduled_reboot_minute` once
    /// per day. Guard rails: only fires when uptime > 5 min and only inside the
    /// target minute.
    pub scheduled_reboot_enabled: bool,
    /// Local hour (0–23) at which the daily reboot fires.
    pub scheduled_reboot_hour: u32,
    /// Local minute (0–59) at which the daily reboot fires.
    pub scheduled_reboot_minute: u32,
    /// When true, daynight task drives IR Cut (off at night / on at day).
    pub auto_ircut: bool,
    /// When true, daynight task drives IR LED (on at night / off at day).
    pub auto_irled: bool,
    /// ISP top-bypass bits. Empty Vec = "don't override" (kernel default
    /// applies). When set, length must be 32 and atometd applies it once at
    /// startup. Each element: 0 = module active, 1 = bypassed.
    pub isp_bypass: Vec<u32>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            night_mode: false,
            ircut_on: true,
            led_on: true,
            irled_on: false,
            fps: 25,
            ae_enable: true,
            exposure_us: 0,
            analog_gain: 0,
            digital_gain: 0,
            record_enabled: false,
            record_start_hour: 0,
            record_end_hour: 24,
            timelapse_enabled: false,
            timelapse_start_hour: 0,
            timelapse_end_hour: 24,
            detection_record_enabled: true,
            detection_enabled: false,
            detection_start_hour: 0,
            detection_end_hour: 24,
            solver_enabled: true,
            solver_start_hour: 0,
            solver_end_hour: 24,
            solver_use_twilight: true,
            solver_twilight_astro: true,
            auto_daynight: false,
            show_timestamp: true,
            show_watermark: false,
            timestamp_position: 0,
            webhook_discord_url: String::new(),
            webhook_slack_url: String::new(),
            webhook_generic_url: String::new(),
            webhook_notify_startup: false,
            webhook_notify_detection_start: false,
            webhook_notify_detection_end: false,
            timezone: default_timezone(),
            latitude: default_latitude(),
            longitude: default_longitude(),
            record_use_twilight: false,
            timelapse_use_twilight: false,
            detection_use_twilight: false,
            record_twilight_astro: false,
            timelapse_twilight_astro: false,
            detection_twilight_astro: false,
            scheduled_reboot_enabled: false,
            scheduled_reboot_hour: 4,
            scheduled_reboot_minute: 0,
            auto_ircut: true,
            auto_irled: false,
            isp_bypass: Vec::new(),
        }
    }
}

pub type SharedAppState = Arc<ArcSwap<AppState>>;

/// Check if `current_hour` (0-23) falls within [start, end).
/// Wrap-around is supported in two equivalent forms:
///   - start > end:  e.g. start=20, end=6  → 20:00–06:00 next day
///   - end > 24:     e.g. start=20, end=30 → same range, "+1 day" UI form
pub fn is_within_schedule(current_hour: u32, start: u32, end: u32) -> bool {
    let end = if end > 24 { end - 24 } else { end };
    if start <= end {
        current_hour >= start && current_hour < end
    } else {
        current_hour >= start || current_hour < end
    }
}

/// Sun altitude (degrees) at the given UTC instant for an observer at
/// (lat_deg N+, lon_deg E+). Returns also the solar declination so callers
/// that need it (e.g. fallback logic) don't have to recompute it.
///
/// Uses the NOAA low-precision solar position algorithm. Accurate to better
/// than ~0.5°, which is far finer than the twilight bands we test against.
fn sun_position(
    now_utc: chrono::DateTime<chrono::Utc>,
    lat_deg: f64,
    lon_deg: f64,
) -> (f64, f64) {
    use chrono::{Datelike, Timelike};
    let mut y = now_utc.year();
    let mut m = now_utc.month() as i32;
    let d = now_utc.day() as i32;
    if m <= 2 {
        y -= 1;
        m += 12;
    }
    let a = y.div_euclid(100);
    let b = 2 - a + a.div_euclid(4);
    let jd0 = (365.25 * (y + 4716) as f64).floor()
        + (30.6001 * (m + 1) as f64).floor()
        + d as f64
        + b as f64
        - 1524.5;
    let frac = (now_utc.hour() as f64
        + now_utc.minute() as f64 / 60.0
        + now_utc.second() as f64 / 3600.0)
        / 24.0;
    let jd = jd0 + frac;
    let n = jd - 2451545.0;

    let l_deg = (280.460 + 0.9856474 * n).rem_euclid(360.0);
    let g = ((357.528 + 0.9856003 * n).rem_euclid(360.0)).to_radians();
    let lambda = (l_deg + 1.915 * g.sin() + 0.020 * (2.0 * g).sin()).to_radians();
    let eps = (23.439 - 0.0000004 * n).to_radians();
    let delta = (eps.sin() * lambda.sin()).asin();
    let alpha_deg = (eps.cos() * lambda.sin())
        .atan2(lambda.cos())
        .to_degrees()
        .rem_euclid(360.0);

    let gmst_h = (18.697374558 + 24.06570982441908 * n).rem_euclid(24.0);
    let lst_deg = (gmst_h * 15.0 + lon_deg).rem_euclid(360.0);
    let h = (lst_deg - alpha_deg).to_radians();

    let lat = lat_deg.to_radians();
    let sin_alt = lat.sin() * delta.sin() + lat.cos() * delta.cos() * h.cos();
    (sin_alt.asin().to_degrees(), delta.to_degrees())
}

/// True if sun is at altitude ≤ -6° (civil night).
pub fn is_civil_night(now_utc: chrono::DateTime<chrono::Utc>, lat_deg: f64, lon_deg: f64) -> bool {
    sun_position(now_utc, lat_deg, lon_deg).0 <= -6.0
}

/// True if sun is at altitude ≤ -18° (astronomical night).
pub fn is_astro_night(now_utc: chrono::DateTime<chrono::Utc>, lat_deg: f64, lon_deg: f64) -> bool {
    sun_position(now_utc, lat_deg, lon_deg).0 <= -18.0
}

/// True if astronomical night is ever reached today at this latitude (given
/// the current solar declination). Independent of longitude/time-of-day:
/// at lower culmination the sun altitude is `|φ + δ| - 90°`, so astro night
/// is reachable iff that value is ≤ -18°, i.e. `|φ + δ| ≤ 72°`.
///
/// Used as a high-latitude-summer fallback: when astro mode is selected but
/// the sun never goes that deep tonight, fall back to civil night.
pub fn is_astro_attainable(now_utc: chrono::DateTime<chrono::Utc>, lat_deg: f64) -> bool {
    // Reuse declination from sun_position (longitude doesn't affect it).
    let (_, delta_deg) = sun_position(now_utc, lat_deg, 0.0);
    (lat_deg + delta_deg).abs() <= 72.0
}

/// Single dispatch used by record / timelapse / detection.
///
/// - `use_twilight=false`: hours-based schedule.
/// - `use_twilight=true, use_astro=false`: civil night (sun ≤ -6°).
/// - `use_twilight=true, use_astro=true`:  astronomical night (sun ≤ -18°),
///   falling back to civil night on days where astro is unattainable
///   (high-latitude summer).
pub fn schedule_active(
    now_local: chrono::DateTime<chrono::Local>,
    use_twilight: bool,
    use_astro: bool,
    start: u32,
    end: u32,
    lat: f64,
    lon: f64,
) -> bool {
    use chrono::Timelike;
    if !use_twilight {
        return is_within_schedule(now_local.hour(), start, end);
    }
    let now_utc = now_local.with_timezone(&chrono::Utc);
    if use_astro {
        if is_astro_attainable(now_utc, lat) {
            is_astro_night(now_utc, lat, lon)
        } else {
            is_civil_night(now_utc, lat, lon)
        }
    } else {
        is_civil_night(now_utc, lat, lon)
    }
}

/// Subset of AppState that is persisted to TOML across reboots.
#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct PersistConfig {
    #[serde(default)]
    pub night_mode: bool,
    #[serde(default = "default_true")]
    pub ircut_on: bool,
    #[serde(default = "default_true")]
    pub led_on: bool,
    #[serde(default)]
    pub irled_on: bool,
    #[serde(default)]
    pub record_enabled: bool,
    #[serde(default)]
    pub record_start_hour: u32,
    #[serde(default = "default_24")]
    pub record_end_hour: u32,
    #[serde(default)]
    pub timelapse_enabled: bool,
    #[serde(default)]
    pub timelapse_start_hour: u32,
    #[serde(default = "default_24")]
    pub timelapse_end_hour: u32,
    #[serde(default = "default_true")]
    pub detection_record_enabled: bool,
    #[serde(default)]
    pub detection_enabled: bool,
    #[serde(default)]
    pub detection_start_hour: u32,
    #[serde(default = "default_24")]
    pub detection_end_hour: u32,
    #[serde(default = "default_true")]
    pub solver_enabled: bool,
    #[serde(default)]
    pub solver_start_hour: u32,
    #[serde(default = "default_24")]
    pub solver_end_hour: u32,
    #[serde(default = "default_true")]
    pub solver_use_twilight: bool,
    #[serde(default = "default_true")]
    pub solver_twilight_astro: bool,
    #[serde(default)]
    pub auto_daynight: bool,
    #[serde(default)]
    pub timestamp_position: u32,
    #[serde(default)]
    pub webhook_discord_url: String,
    #[serde(default)]
    pub webhook_slack_url: String,
    #[serde(default)]
    pub webhook_generic_url: String,
    #[serde(default)]
    pub webhook_notify_startup: bool,
    #[serde(default)]
    pub webhook_notify_detection_start: bool,
    #[serde(default)]
    pub webhook_notify_detection_end: bool,
    #[serde(default = "default_timezone")]
    pub timezone: String,
    #[serde(default = "default_latitude")]
    pub latitude: f64,
    #[serde(default = "default_longitude")]
    pub longitude: f64,
    #[serde(default)]
    pub record_use_twilight: bool,
    #[serde(default)]
    pub timelapse_use_twilight: bool,
    #[serde(default)]
    pub detection_use_twilight: bool,
    #[serde(default)]
    pub record_twilight_astro: bool,
    #[serde(default)]
    pub timelapse_twilight_astro: bool,
    #[serde(default)]
    pub detection_twilight_astro: bool,
    #[serde(default)]
    pub scheduled_reboot_enabled: bool,
    #[serde(default = "default_reboot_hour")]
    pub scheduled_reboot_hour: u32,
    #[serde(default)]
    pub scheduled_reboot_minute: u32,
    #[serde(default = "default_true")]
    pub auto_ircut: bool,
    #[serde(default)]
    pub auto_irled: bool,
    #[serde(default)]
    pub isp_bypass: Vec<u32>,
}

fn default_true() -> bool {
    true
}

fn default_24() -> u32 {
    24
}

fn default_timezone() -> String {
    "JST-9".to_string()
}

fn default_latitude() -> f64 {
    35.6812
}

fn default_longitude() -> f64 {
    139.7671
}

fn default_reboot_hour() -> u32 {
    4
}

impl From<&AppState> for PersistConfig {
    fn from(s: &AppState) -> Self {
        Self {
            night_mode: s.night_mode,
            ircut_on: s.ircut_on,
            led_on: s.led_on,
            irled_on: s.irled_on,
            record_enabled: s.record_enabled,
            record_start_hour: s.record_start_hour,
            record_end_hour: s.record_end_hour,
            timelapse_enabled: s.timelapse_enabled,
            timelapse_start_hour: s.timelapse_start_hour,
            timelapse_end_hour: s.timelapse_end_hour,
            detection_record_enabled: s.detection_record_enabled,
            detection_enabled: s.detection_enabled,
            detection_start_hour: s.detection_start_hour,
            detection_end_hour: s.detection_end_hour,
            solver_enabled: s.solver_enabled,
            solver_start_hour: s.solver_start_hour,
            solver_end_hour: s.solver_end_hour,
            solver_use_twilight: s.solver_use_twilight,
            solver_twilight_astro: s.solver_twilight_astro,
            auto_daynight: s.auto_daynight,
            timestamp_position: s.timestamp_position,
            webhook_discord_url: s.webhook_discord_url.clone(),
            webhook_slack_url: s.webhook_slack_url.clone(),
            webhook_generic_url: s.webhook_generic_url.clone(),
            webhook_notify_startup: s.webhook_notify_startup,
            webhook_notify_detection_start: s.webhook_notify_detection_start,
            webhook_notify_detection_end: s.webhook_notify_detection_end,
            timezone: s.timezone.clone(),
            latitude: s.latitude,
            longitude: s.longitude,
            record_use_twilight: s.record_use_twilight,
            timelapse_use_twilight: s.timelapse_use_twilight,
            detection_use_twilight: s.detection_use_twilight,
            record_twilight_astro: s.record_twilight_astro,
            timelapse_twilight_astro: s.timelapse_twilight_astro,
            detection_twilight_astro: s.detection_twilight_astro,
            scheduled_reboot_enabled: s.scheduled_reboot_enabled,
            scheduled_reboot_hour: s.scheduled_reboot_hour,
            scheduled_reboot_minute: s.scheduled_reboot_minute,
            auto_ircut: s.auto_ircut,
            auto_irled: s.auto_irled,
            isp_bypass: s.isp_bypass.clone(),
        }
    }
}

impl AppState {
    pub fn with_config(cfg: PersistConfig) -> Self {
        AppState {
            night_mode: cfg.night_mode,
            ircut_on: cfg.ircut_on,
            led_on: cfg.led_on,
            irled_on: cfg.irled_on,
            record_enabled: cfg.record_enabled,
            record_start_hour: cfg.record_start_hour,
            record_end_hour: cfg.record_end_hour,
            timelapse_enabled: cfg.timelapse_enabled,
            timelapse_start_hour: cfg.timelapse_start_hour,
            timelapse_end_hour: cfg.timelapse_end_hour,
            detection_record_enabled: cfg.detection_record_enabled,
            detection_enabled: cfg.detection_enabled,
            detection_start_hour: cfg.detection_start_hour,
            detection_end_hour: cfg.detection_end_hour,
            solver_enabled: cfg.solver_enabled,
            solver_start_hour: cfg.solver_start_hour,
            solver_end_hour: cfg.solver_end_hour,
            solver_use_twilight: cfg.solver_use_twilight,
            solver_twilight_astro: cfg.solver_twilight_astro,
            auto_daynight: cfg.auto_daynight,
            timestamp_position: cfg.timestamp_position,
            webhook_discord_url: cfg.webhook_discord_url,
            webhook_slack_url: cfg.webhook_slack_url,
            webhook_generic_url: cfg.webhook_generic_url,
            webhook_notify_startup: cfg.webhook_notify_startup,
            webhook_notify_detection_start: cfg.webhook_notify_detection_start,
            webhook_notify_detection_end: cfg.webhook_notify_detection_end,
            timezone: cfg.timezone,
            latitude: cfg.latitude,
            longitude: cfg.longitude,
            record_use_twilight: cfg.record_use_twilight,
            timelapse_use_twilight: cfg.timelapse_use_twilight,
            detection_use_twilight: cfg.detection_use_twilight,
            record_twilight_astro: cfg.record_twilight_astro,
            timelapse_twilight_astro: cfg.timelapse_twilight_astro,
            detection_twilight_astro: cfg.detection_twilight_astro,
            scheduled_reboot_enabled: cfg.scheduled_reboot_enabled,
            scheduled_reboot_hour: cfg.scheduled_reboot_hour,
            scheduled_reboot_minute: cfg.scheduled_reboot_minute.min(59),
            auto_ircut: cfg.auto_ircut,
            auto_irled: cfg.auto_irled,
            isp_bypass: cfg.isp_bypass,
            ..Default::default()
        }
    }
}

/// ISP readback + system stats — volatile, broadcast via sysstat WebSocket.
#[derive(Serialize, Debug, Clone)]
pub struct SystemInfo {
    /// System
    pub cpu: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub uptime: u64,
}

impl Default for SystemInfo {
    fn default() -> Self {
        Self {
            cpu: 0.0,
            mem_used: 0,
            mem_total: 0,
            uptime: 0,
        }
    }
}

#[derive(Default, Serialize, Debug, Clone)]
pub struct IspInfo {
    pub ae_mode: u32,
    pub it: u32,
    pub ag: u32,
    pub ag_i: u32,
    pub sdg: u32,
    pub idg: u32,
    pub idg_i: u32,
    pub max_it: u32,
    pub max_ag: u32,
    pub max_sdg: u32,
    pub max_idg: u32,
    pub min_it: u32,
    pub min_ag: u32,
    pub min_sdg: u32,
    pub min_idg: u32,
    pub fps_actual: u32,
    pub histogram: Vec<u32>,
}

pub async fn load_config() -> AppState {
    let path = Path::new(paths::CONFIG_TOML);
    if path.exists() {
        match tokio::fs::read_to_string(path).await {
            Ok(contents) => match toml::from_str::<PersistConfig>(&contents) {
                Ok(cfg) => {
                    log::info!("Loaded config from {}", paths::CONFIG_TOML);
                    return AppState::with_config(cfg);
                }
                Err(e) => log::warn!("Failed to parse config: {}", e),
            },
            Err(e) => log::warn!("Failed to read config: {}", e),
        }
    }
    log::info!("Using default config");
    AppState::default()
}

/// Persist the config durably:
///   1. write to `<path>.tmp`
///   2. fsync the file
///   3. atomic rename to final path
///   4. fsync the parent directory (so the rename itself reaches SD)
///
/// Survives sudden power-loss without leaving a half-written file.
pub async fn save_config(state: &AppState) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;

    let cfg = PersistConfig::from(state);
    let contents = toml::to_string_pretty(&cfg).map_err(std::io::Error::other)?;
    let path = Path::new(paths::CONFIG_TOML);
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("config path has no parent"))?;
    tokio::fs::create_dir_all(parent).await?;

    let tmp = parent.join("atomet.toml.tmp");
    {
        let mut f = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)
            .await?;
        f.write_all(contents.as_bytes()).await?;
        f.sync_all().await?;
    }
    tokio::fs::rename(&tmp, path).await?;
    // Persist the directory entry so the rename itself survives power-loss.
    if let Ok(dir) = tokio::fs::File::open(parent).await {
        let _ = dir.sync_all().await;
    }
    log::info!("Saved config to {} (fsynced)", paths::CONFIG_TOML);
    Ok(())
}
