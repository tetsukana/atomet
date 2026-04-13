use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;

use crate::web::WebState;

pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<WebState>) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: WebState) {
    let (mut sender, mut receiver) = socket.split();

    // Send current app state on connect
    let app = state.app_state.load();
    let msg = serde_json::json!({ "type": "appstate", "data": &**app });
    if let Ok(text) = serde_json::to_string(&msg) {
        let _ = sender.send(Message::Text(text)).await;
    }

    let app_state = state.app_state.clone();
    let mut sysstat_rx = state.sysstat_tx.subscribe();
    let mut detection_rx = state.detection_tx.subscribe();

    loop {
        tokio::select! {
            // Forward sysstat broadcasts to this client
            Ok(json) = sysstat_rx.recv() => {
                if sender.send(Message::Text(json)).await.is_err() { break; }
            }
            // Forward detection debug frames to this client
            Ok(json) = detection_rx.recv() => {
                if sender.send(Message::Text(json)).await.is_err() { break; }
            }
            // Receive commands from this client
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Some(resp) = handle_command(&text, &app_state, &state.webhook_tx).await && sender.send(Message::Text(resp)).await.is_err() { break; }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
}

async fn handle_command(
    text: &str,
    app_state: &crate::config::SharedAppState,
    webhook_tx: &tokio::sync::mpsc::Sender<crate::webhook::WebhookEvent>,
) -> Option<String> {
    let Ok(cmd) = serde_json::from_str::<serde_json::Value>(text) else {
        log::warn!("Invalid JSON command: {}", text);
        return None;
    };

    let Some(cmd_type) = cmd.get("type").and_then(|v| v.as_str()) else {
        log::warn!("Command missing 'type' field");
        return None;
    };

    // --- Debug ioctl commands (no AppState mutation) ---
    match cmd_type {
        "get_bypass" => {
            return Some(ioctl_get_bypass());
        }
        "set_bypass" => {
            let bits = cmd.get("bits").and_then(|v| v.as_array());
            return Some(ioctl_set_bypass(bits));
        }
        "get_ae_attr" => {
            return Some(ioctl_get_ae());
        }
        "set_day_night_mode" => {
            let mode = cmd.get("value").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            return Some(ioctl_set_day_night(mode));
        }
        "set_ae_enable" => {
            let enable = cmd.get("value").and_then(|v| v.as_bool()).unwrap_or(true);
            return Some(ioctl_set_ae_enable(enable));
        }
        "set_exposure_us" => {
            let us = cmd.get("value").and_then(|v| v.as_u64()).unwrap_or(40000) as u32;
            let lines = crate::ae_ctrl::us_to_lines(us);
            return Some(ioctl_set_ae_it(lines));
        }
        "set_analog_gain" => {
            let db10 = cmd.get("value").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let isp_gain = crate::ae_ctrl::db10_to_isp_gain(db10);
            return Some(ioctl_set_ae_ag(isp_gain));
        }
        "set_digital_gain" => {
            let db10 = cmd.get("value").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let isp_gain = crate::ae_ctrl::db10_to_isp_gain(db10);
            return Some(ioctl_set_ae_dg(isp_gain));
        }
        "set_max_digital_gain" => {
            let db10 = cmd.get("value").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let isp_gain = crate::ae_ctrl::db10_to_isp_gain(db10);
            let resp = match crate::ae_ctrl::AtometDev::open() {
                Ok(dev) => match dev.set_max_idg(isp_gain) {
                    Ok(()) => serde_json::json!({ "type": "max_idg", "db10": db10 }),
                    Err(e) => serde_json::json!({ "type": "error", "msg": format!("{}", e) }),
                },
                Err(e) => serde_json::json!({ "type": "error", "msg": format!("{}", e) }),
            };
            return Some(resp.to_string());
        }
        "debug_ioctl" => {
            return Some(ioctl_debug_numbers());
        }
        "capture_fits" => {
            unsafe { crate::isp::capture_luma_1080p() };
            return Some("".to_owned());
        }
        "reboot" => {
            log::info!("Reboot command received, rebooting system...");
            crate::system::reboot();
            return None;
        }
        _ => {}
    }

    let mut state = (**app_state.load()).clone();

    match cmd_type {
        "set_ircut" => {
            if let Some(on) = cmd.get("value").and_then(|v| v.as_bool()) {
                state.ircut_on = on;
                // Apply to hardware
                let _ = if on {
                    crate::gpio::ircut_on().await
                } else {
                    crate::gpio::ircut_off().await
                };
                log::info!("IR cut set to {}", on);
            }
        }
        "set_irled" => {
            if let Some(on) = cmd.get("value").and_then(|v| v.as_bool()) {
                state.irled_on = on;
                let _ = if on {
                    crate::gpio::irled_on().await
                } else {
                    crate::gpio::irled_off().await
                };
                log::info!("IR LED set to {}", on);
            }
        }
        "set_led" => {
            if let Some(on) = cmd.get("value").and_then(|v| v.as_bool()) {
                state.led_on = on;
                let _ = if on {
                    crate::gpio::led_on(crate::gpio::Led::Blue).await
                } else {
                    crate::gpio::led_off(crate::gpio::Led::Blue).await
                };
                log::info!("LED set to {}", on);
            }
        }
        "set_record" => {
            if let Some(on) = cmd.get("value").and_then(|v| v.as_bool()) {
                state.record_enabled = on;
                log::info!("Record set to {}", on);
            }
        }
        "set_record_schedule" => {
            if let (Some(s), Some(e)) = (
                cmd.get("start").and_then(|v| v.as_u64()),
                cmd.get("end").and_then(|v| v.as_u64()),
            ) {
                state.record_start_hour = s as u32;
                state.record_end_hour = e as u32;
                log::info!("Record schedule set to {}–{}", s, e);
            }
        }
        "set_timelapse" => {
            if let Some(on) = cmd.get("value").and_then(|v| v.as_bool()) {
                state.timelapse_enabled = on;
                log::info!("Timelapse set to {}", on);
            }
        }
        "set_timelapse_schedule" => {
            if let (Some(s), Some(e)) = (
                cmd.get("start").and_then(|v| v.as_u64()),
                cmd.get("end").and_then(|v| v.as_u64()),
            ) {
                state.timelapse_start_hour = s as u32;
                state.timelapse_end_hour = e as u32;
                log::info!("Timelapse schedule set to {}–{}", s, e);
            }
        }
        "set_detection_record" => {
            if let Some(on) = cmd.get("value").and_then(|v| v.as_bool()) {
                state.detection_record_enabled = on;
                log::info!("Detection record set to {}", on);
            }
        }
        "set_detection_record_schedule" => {
            if let (Some(s), Some(e)) = (
                cmd.get("start").and_then(|v| v.as_u64()),
                cmd.get("end").and_then(|v| v.as_u64()),
            ) {
                state.detection_record_start_hour = s as u32;
                state.detection_record_end_hour = e as u32;
                log::info!("Detection record schedule set to {}–{}", s, e);
            }
        }
        "set_detection" => {
            if let Some(on) = cmd.get("value").and_then(|v| v.as_bool()) {
                let was = state.detection_enabled;
                state.detection_enabled = on;
                log::info!("Detection set to {}", on);
                if on && !was {
                    let _ = webhook_tx.try_send(crate::webhook::WebhookEvent::DetectionStart);
                } else if !on && was {
                    let _ = webhook_tx.try_send(crate::webhook::WebhookEvent::DetectionEnd);
                }
            }
        }
        "set_night_mode" => {
            if let Some(on) = cmd.get("value").and_then(|v| v.as_bool()) {
                if on {
                    crate::daynight::apply_night().await;
                    state.ircut_on = false;
                    state.irled_on = true;
                } else {
                    crate::daynight::apply_day().await;
                    state.ircut_on = true;
                    state.irled_on = false;
                }
                state.night_mode = on;
                log::info!("Night mode set to {}", on);
            }
        }
        "set_auto_daynight" => {
            if let Some(on) = cmd.get("value").and_then(|v| v.as_bool()) {
                state.auto_daynight = on;
                log::info!("Auto daynight set to {}", on);
            }
        }
        "set_show_timestamp" => {
            if let Some(on) = cmd.get("value").and_then(|v| v.as_bool()) {
                state.show_timestamp = on;
            }
        }
        "set_timestamp_position" => {
            if let Some(pos) = cmd.get("value").and_then(|v| v.as_u64()) {
                state.timestamp_position = pos as u32;
            }
        }
        "set_webhook_discord" => {
            if let Some(url) = cmd.get("value").and_then(|v| v.as_str()) {
                state.webhook_discord_url = url.to_string();
                log::info!("Discord webhook URL updated");
            }
        }
        "set_webhook_slack" => {
            if let Some(url) = cmd.get("value").and_then(|v| v.as_str()) {
                state.webhook_slack_url = url.to_string();
                log::info!("Slack webhook URL updated");
            }
        }
        "set_webhook_generic" => {
            if let Some(url) = cmd.get("value").and_then(|v| v.as_str()) {
                state.webhook_generic_url = url.to_string();
                log::info!("Generic webhook URL updated");
            }
        }
        "set_webhook_notify_startup" => {
            if let Some(on) = cmd.get("value").and_then(|v| v.as_bool()) {
                state.webhook_notify_startup = on;
            }
        }
        "set_webhook_notify_detection_start" => {
            if let Some(on) = cmd.get("value").and_then(|v| v.as_bool()) {
                state.webhook_notify_detection_start = on;
            }
        }
        "set_webhook_notify_detection_end" => {
            if let Some(on) = cmd.get("value").and_then(|v| v.as_bool()) {
                state.webhook_notify_detection_end = on;
            }
        }
        "set_fps" => {
            if let Some(v) = cmd.get("value").and_then(|v| v.as_u64()) {
                let fps = (v as u32).clamp(1, 30);
                state.fps = fps;
                unsafe { isvp_sys::IMP_ISP_Tuning_SetSensorFPS(fps, 1) };
                log::info!("FPS set to {}", fps);
            }
        }
        _ => {
            log::warn!("Unknown command type: {}", cmd_type);
        }
    }

    let new_state = Arc::new(state);
    app_state.store(Arc::clone(&new_state));

    // Persist to disk on every change
    tokio::spawn(async move {
        if let Err(e) = crate::config::save_config(&new_state).await {
            log::error!("Failed to save config: {}", e);
        }
    });

    None
}

// ---------------------------------------------------------------------------
// ioctl debug helpers
// ---------------------------------------------------------------------------

fn ioctl_get_bypass() -> String {
    let resp = match crate::ae_ctrl::AtometDev::open() {
        Ok(dev) => match dev.get_bypass() {
            Ok(bp) => serde_json::json!({
                "type": "bypass",
                "bits": bp.bits.to_vec(),
            }),
            Err(e) => serde_json::json!({ "type": "error", "msg": format!("get_bypass: {}", e) }),
        },
        Err(e) => serde_json::json!({ "type": "error", "msg": format!("open /dev/atomet: {}", e) }),
    };
    resp.to_string()
}

fn ioctl_set_bypass(bits: Option<&Vec<serde_json::Value>>) -> String {
    let Some(arr) = bits else {
        return serde_json::json!({ "type": "error", "msg": "missing 'bits' array" }).to_string();
    };
    if arr.len() != 32 {
        return serde_json::json!({ "type": "error", "msg": "bits must be [u32; 32]" }).to_string();
    }
    let mut bp = crate::ae_ctrl::TopBypass::default();
    for (i, v) in arr.iter().enumerate() {
        bp.bits[i] = v.as_u64().unwrap_or(0) as u32;
    }
    let resp = match crate::ae_ctrl::AtometDev::open() {
        Ok(dev) => match dev.set_bypass(&bp) {
            Ok(()) => {
                log::info!("Bypass set: {:?}", &bp.bits[..8]);
                serde_json::json!({ "type": "bypass", "bits": bp.bits.to_vec() })
            }
            Err(e) => {
                serde_json::json!({ "type": "error", "msg": format!("set_bypass: {}", e) })
            }
        },
        Err(e) => serde_json::json!({ "type": "error", "msg": format!("open /dev/atomet: {}", e) }),
    };
    resp.to_string()
}

fn ioctl_set_ae_enable(ae_enable: bool) -> String {
    let resp = match crate::ae_ctrl::AtometDev::open() {
        Ok(dev) => {
            let result = if ae_enable {
                dev.unfreeze_ae()
            } else {
                dev.freeze_ae()
            };
            match result {
                Ok(()) => {
                    let mode = if ae_enable { 0 } else { 1 };
                    serde_json::json!({ "type": "ae_mode", "mode": mode })
                }
                Err(e) => {
                    serde_json::json!({ "type": "error", "msg": format!("freeze/unfreeze: {}", e) })
                }
            }
        }
        Err(e) => serde_json::json!({ "type": "error", "msg": format!("open /dev/atomet: {}", e) }),
    };
    resp.to_string()
}

fn ioctl_set_ae_it(it: u32) -> String {
    let resp = match crate::ae_ctrl::AtometDev::open() {
        Ok(dev) => match dev.get_ae() {
            Ok(mut ae) => {
                ae.it_value = it;
                ae.it_manual_en = 1;
                ae.ae_mode = 1;
                match dev.set_ae(&ae) {
                    Ok(()) => {
                        serde_json::json!({ "type": "it", "value": it })
                    }
                    Err(e) => {
                        serde_json::json!({ "type": "error", "msg": format!("set_ae: {}", e) })
                    }
                }
            }
            Err(e) => {
                serde_json::json!({ "type": "error", "msg": format!("get_ae: {}", e) })
            }
        },
        Err(e) => serde_json::json!({ "type": "error", "msg": format!("open /dev/atomet: {}", e) }),
    };
    resp.to_string()
}

fn ioctl_set_ae_ag(ag: u32) -> String {
    let resp = match crate::ae_ctrl::AtometDev::open() {
        Ok(dev) => match dev.get_ae() {
            Ok(mut ae) => {
                ae.ag_value = ag;
                ae.ag_manual_en = 1;
                ae.ae_mode = 1;
                match dev.set_ae(&ae) {
                    Ok(()) => {
                        serde_json::json!({ "type": "ag", "value": ag })
                    }
                    Err(e) => {
                        serde_json::json!({ "type": "error", "msg": format!("set_ae: {}", e) })
                    }
                }
            }
            Err(e) => {
                serde_json::json!({ "type": "error", "msg": format!("get_ae: {}", e) })
            }
        },
        Err(e) => serde_json::json!({ "type": "error", "msg": format!("open /dev/atomet: {}", e) }),
    };
    resp.to_string()
}

fn ioctl_set_ae_dg(dg: u32) -> String {
    let resp = match crate::ae_ctrl::AtometDev::open() {
        Ok(dev) => match dev.get_ae() {
            Ok(mut ae) => {
                ae.idg_value = dg;
                ae.dg_manual_en = 1;
                ae.ae_mode = 1;
                match dev.set_ae(&ae) {
                    Ok(()) => {
                        serde_json::json!({ "type": "dg", "value": dg })
                    }
                    Err(e) => {
                        serde_json::json!({ "type": "error", "msg": format!("set_ae: {}", e) })
                    }
                }
            }
            Err(e) => {
                serde_json::json!({ "type": "error", "msg": format!("get_ae: {}", e) })
            }
        },
        Err(e) => serde_json::json!({ "type": "error", "msg": format!("open /dev/atomet: {}", e) }),
    };
    resp.to_string()
}

fn ioctl_get_ae() -> String {
    use crate::ae_ctrl::{lines_to_us, isp_gain_to_db10};
    let resp = match crate::ae_ctrl::AtometDev::open() {
        Ok(dev) => match dev.get_ae() {
            Ok(ae) => serde_json::json!({
                "type": "ae_attr",
                "ae_mode": ae.ae_mode,
                "exposure_us": lines_to_us(ae.it_value),
                "analog_gain_db10": isp_gain_to_db10(ae.ag_value),
                "digital_gain_db10": isp_gain_to_db10(ae.idg_value),
                "it_value": ae.it_value,
                "ag_value": ae.ag_value,
                "idg_value": ae.idg_value,
                "max_it": ae.max_it,
                "max_it_us": lines_to_us(ae.max_it),
                "max_ag_db10": isp_gain_to_db10(ae.max_ag),
                "max_idg_db10": isp_gain_to_db10(ae.max_idg),
                "total_ev": ae.total_ev,
                "it_manual_en": ae.it_manual_en,
                "ag_manual_en": ae.ag_manual_en,
                "dg_manual_en": ae.dg_manual_en,
            }),
            Err(e) => serde_json::json!({ "type": "error", "msg": format!("get_ae: {}", e) }),
        },
        Err(e) => serde_json::json!({ "type": "error", "msg": format!("open /dev/atomet: {}", e) }),
    };
    resp.to_string()
}

fn ioctl_set_day_night(mode: i32) -> String {
    let resp = match crate::ae_ctrl::AtometDev::open() {
        Ok(dev) => match dev.set_day_night(mode) {
            Ok(()) => {
                log::info!("ioctl day_night = {}", mode);
                serde_json::json!({ "type": "ok" })
            }
            Err(e) => {
                serde_json::json!({ "type": "error", "msg": format!("set_day_night: {}", e) })
            }
        },
        Err(e) => serde_json::json!({ "type": "error", "msg": format!("open /dev/atomet: {}", e) }),
    };
    resp.to_string()
}

fn ioctl_debug_numbers() -> String {
    use crate::ae_ctrl::{AeParams, TopBypass};
    let size_ae = std::mem::size_of::<AeParams>();
    let size_bp = std::mem::size_of::<TopBypass>();

    // MIPS ioctl: _IOC_READ=2, _IOC_WRITE=4, _IOC_DIRSHIFT=29
    let magic: u32 = b'M' as u32;
    let get_ae = ((2u32 << 29) | (magic << 8)) | ((size_ae as u32) << 16);
    let set_ae = (4u32 << 29) | (magic << 8) | 1 | ((size_ae as u32) << 16);
    let get_bp = (2u32 << 29) | (magic << 8) | 2 | ((size_bp as u32) << 16);
    let set_bp = (4u32 << 29) | (magic << 8) | 3 | ((size_bp as u32) << 16);
    let set_dn = (4u32 << 29) | (magic << 8) | 4 | (4u32 << 16);

    serde_json::json!({
        "type": "debug_ioctl",
        "sizeof_AeParams": size_ae,
        "sizeof_TopBypass": size_bp,
        "ATOMET_GET_AE_ATTR": format!("0x{:08X}", get_ae),
        "ATOMET_SET_AE_ATTR": format!("0x{:08X}", set_ae),
        "ATOMET_GET_TOP_BYPASS": format!("0x{:08X}", get_bp),
        "ATOMET_SET_TOP_BYPASS": format!("0x{:08X}", set_bp),
        "ATOMET_SET_DAY_NIGHT": format!("0x{:08X}", set_dn),
    })
    .to_string()
}
