use crate::config::SharedAppState;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{broadcast, mpsc};

/// System-level webhook events (non-meteor).
pub enum WebhookEvent {
    Startup,
    DetectionStart,
    DetectionEnd,
}

/// Get system hostname (cached after first call).
fn hostname() -> &'static str {
    use std::sync::OnceLock;
    static HOST: OnceLock<String> = OnceLock::new();
    HOST.get_or_init(|| {
        let mut buf = [0u8; 64];
        let name = unsafe {
            if libc::gethostname(buf.as_mut_ptr() as *mut _, buf.len()) == 0 {
                let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
                std::str::from_utf8(&buf[..len]).unwrap_or("atomet").to_string()
            } else {
                "atomet".to_string()
            }
        };
        name
    })
}

/// Build a ureq Agent configured to use native-tls (OpenSSL).
fn make_agent() -> ureq::Agent {
    use ureq::config::Config;
    use ureq::tls::{TlsConfig, TlsProvider};

    let config = Config::builder()
        .tls_config(
            TlsConfig::builder()
                .provider(TlsProvider::NativeTls)
                .build(),
        )
        .build();

    config.new_agent()
}

/// Async task that listens for meteor events on `detection_rx` and
/// system events on `event_rx`, sending webhook notifications to all
/// configured endpoints.
pub async fn webhook_task(
    mut detection_rx: broadcast::Receiver<String>,
    mut event_rx: mpsc::Receiver<WebhookEvent>,
    app_state: SharedAppState,
    shutdown: Arc<AtomicBool>,
) {
    log::info!("webhook_task started");

    while !shutdown.load(Ordering::Relaxed) {
        tokio::select! {
            Ok(json) = detection_rx.recv() => {
                if json.contains(r#""type":"meteor""#) {
                    handle_meteor(&app_state, json);
                } else if json.contains(r#""type":"meteor_stack""#) {
                    handle_meteor_stack(&app_state, json);
                }
            }
            Some(event) = event_rx.recv() => {
                handle_event(&app_state, event);
            }
            else => continue,
        }
    }

    log::info!("webhook_task shutting down");
}

fn has_any_url(app_state: &SharedAppState) -> (String, String, String) {
    let state = app_state.load();
    (
        state.webhook_discord_url.clone(),
        state.webhook_slack_url.clone(),
        state.webhook_generic_url.clone(),
    )
}

fn handle_meteor(app_state: &SharedAppState, json: String) {
    let (discord_url, slack_url, generic_url) = has_any_url(app_state);
    if discord_url.is_empty() && slack_url.is_empty() && generic_url.is_empty() {
        return;
    }

    let meteor: serde_json::Value = match serde_json::from_str(&json) {
        Ok(v) => v,
        Err(_) => return,
    };

    let id = meteor["id"].as_u64().unwrap_or(0);
    let speed = meteor["speed"].as_f64().unwrap_or(0.0);
    let frames = meteor["frames"].as_u64().unwrap_or(0);
    let ts = meteor["ts"].as_str().unwrap_or("unknown").to_string();

    tokio::task::spawn_blocking(move || {
        let agent = make_agent();
        if !discord_url.is_empty() {
            let body = format_meteor_discord(id, speed, frames, &ts);
            send_json(&agent, &discord_url, &body);
        }
        if !slack_url.is_empty() {
            let text = format_meteor_slack(id, speed, frames, &ts);
            send_json(&agent, &slack_url, &serde_json::json!({ "text": text }));
        }
        if !generic_url.is_empty() {
            post_json(&agent, &generic_url, &json).ok();
        }
    });
}

/// Called when the detection stack PNG has been saved.
/// Sends the image to Discord (multipart), Slack (text-only), and Generic (base64).
fn handle_meteor_stack(app_state: &SharedAppState, json: String) {
    let (discord_url, slack_url, generic_url) = has_any_url(app_state);
    if discord_url.is_empty() && slack_url.is_empty() && generic_url.is_empty() {
        return;
    }

    let msg: serde_json::Value = match serde_json::from_str(&json) {
        Ok(v) => v,
        Err(_) => return,
    };

    let path = msg["path"].as_str().unwrap_or("").to_string();
    let ts = msg["ts"].as_str().unwrap_or("unknown").to_string();

    if path.is_empty() {
        return;
    }

    tokio::task::spawn_blocking(move || {
        let agent = make_agent();

        if !discord_url.is_empty() {
            send_discord_image(&agent, &discord_url, &path, &ts);
        }
        if !generic_url.is_empty() {
            send_generic_image(&agent, &generic_url, &path, &ts);
        }
        // Slack incoming webhooks don't support file uploads
    });
}

fn handle_event(app_state: &SharedAppState, event: WebhookEvent) {
    let (discord_url, slack_url, generic_url) = has_any_url(app_state);
    if discord_url.is_empty() && slack_url.is_empty() && generic_url.is_empty() {
        return;
    }

    let state = app_state.load();
    let (enabled, title, color, generic_type) = match event {
        WebhookEvent::Startup => (
            state.webhook_notify_startup,
            "System Started",
            5763719, // green
            "startup",
        ),
        WebhookEvent::DetectionStart => (
            state.webhook_notify_detection_start,
            "Detection Started",
            3447003, // blue
            "detection_start",
        ),
        WebhookEvent::DetectionEnd => (
            state.webhook_notify_detection_end,
            "Detection Stopped",
            15158332, // grey
            "detection_end",
        ),
    };

    if !enabled {
        return;
    }

    let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

    tokio::task::spawn_blocking(move || {
        let agent = make_agent();
        let discord_body = serde_json::json!({
            "username": hostname(),
            "embeds": [{
                "title": title,
                "color": color,
                "footer": { "text": format!("atomet | {}", ts) },
            }]
        });

        if !discord_url.is_empty() {
            send_json(&agent, &discord_url, &discord_body);
        }
        if !slack_url.is_empty() {
            let text = format!("*{}* | {}", title, ts);
            send_json(&agent, &slack_url, &serde_json::json!({ "text": text }));
        }
        if !generic_url.is_empty() {
            let body = serde_json::json!({ "type": generic_type, "ts": ts });
            send_json(&agent, &generic_url, &body);
        }
    });
}

// ── Formatters ──────────────────────────────────────────────

fn format_meteor_discord(id: u64, speed: f64, frames: u64, ts: &str) -> serde_json::Value {
    serde_json::json!({
        "username": hostname(),
        "embeds": [{
            "title": "Meteor Detected",
            "color": 3447003,
            "fields": [
                { "name": "ID",     "value": id.to_string(),              "inline": true },
                { "name": "Speed",  "value": format!("{:.1} px/f", speed), "inline": true },
                { "name": "Frames", "value": frames.to_string(),          "inline": true },
            ],
            "footer": { "text": format!("atomet | {}", ts) },
        }]
    })
}

fn format_meteor_slack(id: u64, speed: f64, frames: u64, ts: &str) -> String {
    format!(
        "*Meteor Detected*\nID: {} | Speed: {:.1} px/f | Frames: {} | {}",
        id, speed, frames, ts
    )
}

// ── Senders ─────────────────────────────────────────────────

fn post_json(agent: &ureq::Agent, url: &str, body: &str) -> Result<(), ureq::Error> {
    agent
        .post(url)
        .header("Content-Type", "application/json")
        .send(body.as_bytes())?;
    Ok(())
}

fn send_json(agent: &ureq::Agent, url: &str, body: &serde_json::Value) {
    match post_json(agent, url, &body.to_string()) {
        Ok(()) => log::info!("Webhook sent to {}", url.get(..40).unwrap_or(url)),
        Err(e) => log::error!("Webhook failed: {}", e),
    }
}

/// Send an image to Discord via multipart/form-data with an embed.
/// Manually constructs the multipart body to avoid getrandom dependency.
fn send_discord_image(agent: &ureq::Agent, url: &str, path: &str, ts: &str) {
    let payload = serde_json::json!({
        "username": hostname(),
        "embeds": [{
            "title": "Detection Stack",
            "color": 3447003,
            "image": { "url": "attachment://stack.png" },
            "footer": { "text": format!("atomet | {}", ts) },
        }]
    });

    let image_data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            log::error!("Failed to read stack image '{}': {}", path, e);
            return;
        }
    };

    let boundary = format!("----atomet-{}", ts.replace(['-', ' ', ':'], ""));
    let payload_str = payload.to_string();

    let mut body: Vec<u8> = Vec::new();
    // payload_json part
    body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"payload_json\"\r\n");
    body.extend_from_slice(b"Content-Type: application/json\r\n\r\n");
    body.extend_from_slice(payload_str.as_bytes());
    body.extend_from_slice(b"\r\n");
    // file part
    body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"file\"; filename=\"stack.png\"\r\n");
    body.extend_from_slice(b"Content-Type: image/png\r\n\r\n");
    body.extend_from_slice(&image_data);
    body.extend_from_slice(b"\r\n");
    // closing boundary
    body.extend_from_slice(format!("--{}--\r\n", boundary).as_bytes());

    let content_type = format!("multipart/form-data; boundary={}", boundary);

    match agent
        .post(url)
        .header("Content-Type", &content_type)
        .send(&body[..])
    {
        Ok(_) => log::info!("Discord image webhook sent"),
        Err(e) => log::error!("Discord image webhook failed: {}", e),
    }
}

/// Send an image to Generic webhook as base64-encoded JSON.
fn send_generic_image(agent: &ureq::Agent, url: &str, path: &str, ts: &str) {
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            log::error!("Failed to read stack image '{}': {}", path, e);
            return;
        }
    };

    use base64ct::{Base64, Encoding};
    let b64 = Base64::encode_string(&data);

    let body = serde_json::json!({
        "type": "meteor_stack",
        "ts": ts,
        "image": b64,
        "image_type": "image/png",
    });

    send_json(agent, url, &body);
}
