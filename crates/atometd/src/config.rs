use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

const CONFIG_PATH: &str = "/media/mmc/atomet.toml";

/// Full application state — used at runtime by all tasks and the WebUI.
/// Only the `PersistConfig` subset is saved to disk; the rest resets on reboot.
#[derive(Serialize, Debug, Clone)]
pub struct AppState {
    // --- Volatile (reset on reboot) ---
    pub night_mode: bool,
    pub ircut_on: bool,
    pub led_on: bool,
    pub irled_on: bool,
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
    pub record_enabled: bool,
    pub detection_enabled: bool,
    pub solve_field_enabled: bool,
    pub auto_daynight: bool,
    pub show_timestamp: bool,
    pub show_watermark: bool,
    pub timestamp_position: u32,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            night_mode: false,
            ircut_on: true,
            led_on: true,
            irled_on: false,
            fps: 25,
            ae_enable: false,
            exposure_us: 0,
            analog_gain: 0,
            digital_gain: 0,
            record_enabled: false,
            detection_enabled: false,
            solve_field_enabled: false,
            auto_daynight: false,
            show_timestamp: true,
            show_watermark: false,
            timestamp_position: 0,
        }
    }
}

pub type SharedAppState = Arc<ArcSwap<AppState>>;

/// Subset of AppState that is persisted to TOML across reboots.
#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct PersistConfig {
    #[serde(default)]
    pub record_enabled: bool,
    #[serde(default)]
    pub detection_enabled: bool,
    #[serde(default)]
    pub solve_field_enabled: bool,
    #[serde(default)]
    pub auto_daynight: bool,
    #[serde(default)]
    pub timestamp_position: u32,
}

impl From<&AppState> for PersistConfig {
    fn from(s: &AppState) -> Self {
        Self {
            record_enabled: s.record_enabled,
            detection_enabled: s.detection_enabled,
            solve_field_enabled: s.solve_field_enabled,
            auto_daynight: s.auto_daynight,
            timestamp_position: s.timestamp_position,
        }
    }
}

impl AppState {
    pub fn with_config(cfg: PersistConfig) -> Self {
        AppState {
            record_enabled: cfg.record_enabled,
            detection_enabled: cfg.detection_enabled,
            solve_field_enabled: cfg.solve_field_enabled,
            auto_daynight: cfg.auto_daynight,
            timestamp_position: cfg.timestamp_position,
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
    let path = Path::new(CONFIG_PATH);
    if path.exists() {
        match tokio::fs::read_to_string(path).await {
            Ok(contents) => match toml::from_str::<PersistConfig>(&contents) {
                Ok(cfg) => {
                    log::info!("Loaded config from {}", CONFIG_PATH);
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

pub async fn save_config(state: &AppState) -> std::io::Result<()> {
    let cfg = PersistConfig::from(state);
    let contents = toml::to_string_pretty(&cfg).map_err(std::io::Error::other)?;
    tokio::fs::write(CONFIG_PATH, contents).await?;
    log::info!("Saved config to {}", CONFIG_PATH);
    Ok(())
}
