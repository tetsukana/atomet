//! Clock quality for the FF sidecar.
//!
//! `S43clockstat` samples `ntpq` every 5 minutes and writes a small JSON file
//! (tmpfs). atometd does not talk NTP itself; it only reads that file and
//! attaches it to the sidecar so a server can judge how far the timestamps of
//! this station can be trusted.

use crate::paths;
use serde_json::{Value, json};
use std::time::{SystemTime, UNIX_EPOCH};

/// A sample older than this is not trusted (the script samples every 300 s).
const MAX_AGE_S: f64 = 900.0;

/// Returns the `clock` object for the sidecar. Never fails: a missing,
/// unreadable or stale file yields `synced: false` with a `reason`.
pub fn read() -> Value {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);

    let text = match std::fs::read_to_string(paths::CLOCK_JSON) {
        Ok(t) => t,
        Err(_) => return json!({ "synced": false, "reason": "no_data" }),
    };
    let mut v: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return json!({ "synced": false, "reason": "parse_error" }),
    };

    let sampled = v.get("sampled_unix").and_then(Value::as_f64);
    let Some(sampled) = sampled else {
        return json!({ "synced": false, "reason": "parse_error" });
    };
    let age = (now - sampled).max(0.0);
    v["age_s"] = json!(age.round());
    if age > MAX_AGE_S {
        // Keep the raw values for post-mortem, but do not claim a sync.
        v["synced"] = json!(false);
        v["reason"] = json!("stale");
    }
    v
}
