use isvp_sys::{
    IMP_ISP_Tuning_GetExpr, IMP_ISP_Tuning_GetTotalGain, IMP_ISP_Tuning_SetISPRunningMode,
    IMPISPExpr, IMPISPRunningMode_IMPISP_RUNNING_MODE_DAY,
    IMPISPRunningMode_IMPISP_RUNNING_MODE_NIGHT,
};
use std::mem::MaybeUninit;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::time::sleep;

use crate::config::SharedAppState;
use crate::gpio;

// Thresholds (0–100 scale, integration-time based)
const THRESHOLD_LOW: f32 = 30.0; // DAY → NIGHT below this %
const THRESHOLD_HIGH: f32 = 70.0; // NIGHT → DAY above this %
const SAMPLE_COUNT: usize = 10;
const INTERVAL: Duration = Duration::from_secs(2);
const MIN_TRANSITION_SECS: u64 = 10;

#[derive(PartialEq, Clone, Copy)]
enum Mode {
    Unknown,
    Day,
    Night,
}

pub async fn daynight_task(app_state: SharedAppState, shutdown: Arc<AtomicBool>) {
    let mut history = [50.0f32; SAMPLE_COUNT];
    let mut history_idx = 0usize;
    let mut mode = Mode::Unknown;
    let mut last_transition = Instant::now()
        .checked_sub(Duration::from_secs(MIN_TRANSITION_SECS + 1))
        .unwrap_or_else(Instant::now);

    log::info!("Daynight task started");

    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        if !app_state.load().auto_daynight {
            sleep(INTERVAL).await;
            continue;
        }

        let brightness = match read_brightness().await {
            Some(v) => v,
            None => {
                sleep(INTERVAL).await;
                continue;
            }
        };

        history[history_idx] = brightness;
        history_idx = (history_idx + 1) % SAMPLE_COUNT;
        let avg = history.iter().sum::<f32>() / SAMPLE_COUNT as f32;

        let target = match mode {
            Mode::Day => {
                if avg < THRESHOLD_LOW {
                    Mode::Night
                } else {
                    Mode::Day
                }
            }
            Mode::Night => {
                if avg > THRESHOLD_HIGH {
                    Mode::Day
                } else {
                    Mode::Night
                }
            }
            Mode::Unknown => {
                if avg < THRESHOLD_LOW {
                    Mode::Night
                } else if avg > THRESHOLD_HIGH {
                    Mode::Day
                } else {
                    Mode::Unknown
                }
            }
        };

        if target != mode && target != Mode::Unknown {
            let elapsed = last_transition.elapsed().as_secs();
            if elapsed >= MIN_TRANSITION_SECS || mode == Mode::Unknown {
                let night = target == Mode::Night;
                let s = app_state.load();
                log::info!(
                    "Auto daynight: → {} (brightness={:.1}%, avg={:.1}%, auto_ircut={}, auto_irled={})",
                    if night { "NIGHT" } else { "DAY" },
                    brightness,
                    avg,
                    s.auto_ircut,
                    s.auto_irled
                );

                apply_scene(night, s.auto_ircut, s.auto_irled).await;

                // Reset history so stale samples don't bias the new mode
                history.fill(if night { 0.0 } else { 100.0 });

                let current = app_state.load_full();
                let mut updated = (*current).clone();
                updated.night_mode = night;
                if updated.auto_ircut {
                    updated.ircut_on = !night;
                }
                if updated.auto_irled {
                    updated.irled_on = night;
                }
                app_state.store(Arc::new(updated));

                mode = target;
                last_transition = Instant::now();
            }
        }

        sleep(INTERVAL).await;
    }
}

/// Apply a day/night transition, honouring per-feature auto flags.
///
/// ISP running-mode switch is unconditional (it is the whole point of auto
/// daynight). IR Cut and IR LED are only touched when the user has them in
/// Auto mode.
pub async fn apply_scene(night: bool, auto_ircut: bool, auto_irled: bool) {
    if night {
        if auto_ircut {
            let _ = gpio::ircut_off().await;
        }
        if auto_irled {
            let _ = gpio::irled_on().await;
        }
        tokio::task::spawn_blocking(|| unsafe {
            IMP_ISP_Tuning_SetISPRunningMode(IMPISPRunningMode_IMPISP_RUNNING_MODE_NIGHT);
        })
        .await
        .ok();
    } else {
        // Day: ISP day mode first so AE normalises, then ircut on, irled off.
        tokio::task::spawn_blocking(|| unsafe {
            IMP_ISP_Tuning_SetISPRunningMode(IMPISPRunningMode_IMPISP_RUNNING_MODE_DAY);
        })
        .await
        .ok();
        if auto_ircut {
            let _ = gpio::ircut_on().await;
        }
        if auto_irled {
            let _ = gpio::irled_off().await;
        }
    }
}

/// Read scene brightness (0–100%) via ISP API.
///
/// Uses integration_time / integration_time_max ratio from GetExpr:
///   bright scene → short exposure → high brightness value
///   dark scene   → long exposure  → low brightness value
///
/// Total gain (GetTotalGain, [24.8] fixed-point) compensates for amplification:
///   high gain = dark scene = brightness reduced.
///
/// This correctly reflects scene brightness even in Night ISP mode because
/// integration_time is constrained by physical sensor limits, unlike
/// IMP_ISP_Tuning_GetAeLuma which is maintained at AE target by the control loop.
async fn read_brightness() -> Option<f32> {
    tokio::task::spawn_blocking(|| unsafe {
        // --- Integration time ---
        let mut expr = MaybeUninit::<IMPISPExpr>::zeroed();
        if IMP_ISP_Tuning_GetExpr(expr.as_mut_ptr()) != 0 {
            log::warn!("IMP_ISP_Tuning_GetExpr failed");
            return None;
        }
        let expr = expr.assume_init();
        let it = expr.g_attr.integration_time as f32;
        let it_max = expr.g_attr.integration_time_max as f32;
        if it_max <= 0.0 {
            return None;
        }

        let exposure_ratio = it / it_max;
        let base_brightness = (1.0 - exposure_ratio) * 100.0;

        // --- Total gain [24.8]: 256 = 1×, 512 = 2×, etc. ---
        let mut total_gain: u32 = 0;
        let gain_x = if IMP_ISP_Tuning_GetTotalGain(&mut total_gain) == 0 {
            (total_gain as f32 / 256.0).max(1.0)
        } else {
            1.0
        };

        Some((base_brightness / gain_x).clamp(0.0, 100.0))
    })
    .await
    .ok()
    .flatten()
}
