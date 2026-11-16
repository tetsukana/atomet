//! Server-side history of detection-related events, so a web client that
//! (re)connects (e.g. after switching tabs / reloading) can restore its logs.
//!
//! Scope: the history lives for one detection period. It is cleared when
//! detection (enabled + schedule) goes from inactive to active, and kept
//! after the period ends so the morning-after log is still there.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::broadcast;

/// Cap per list; a night rarely exceeds this, and it bounds memory.
const MAX_EVENTS: usize = 500;

#[derive(Default)]
struct Inner {
    /// `meteor` / `meteor_end` (realtime + FF pass), `ff_saved`, in arrival order.
    events: VecDeque<String>,
    /// Latest state-like events (replace, not accumulate).
    last_stars: Option<String>,
    last_solver_status: Option<String>,
}

#[derive(Clone, Default)]
pub struct EventHistory(Arc<Mutex<Inner>>);

impl EventHistory {
    fn push(&self, msg: String) {
        let mut g = self.0.lock().unwrap();
        if g.events.len() >= MAX_EVENTS {
            g.events.pop_front();
        }
        g.events.push_back(msg);
    }

    fn clear(&self) {
        let mut g = self.0.lock().unwrap();
        g.events.clear();
        g.last_stars = None;
        g.last_solver_status = None;
    }

    /// Messages to replay to a newly connected client, oldest first.
    pub fn snapshot(&self) -> Vec<String> {
        let g = self.0.lock().unwrap();
        let mut out: Vec<String> = g.events.iter().cloned().collect();
        out.extend(g.last_stars.clone());
        out.extend(g.last_solver_status.clone());
        out
    }

    fn record_ff(&self, msg: String) {
        if msg.contains(r#""type":"ff_saved""#) || msg.contains(r#""type":"meteor""#) {
            self.push(msg);
        } else if msg.contains(r#""type":"stars""#) {
            self.0.lock().unwrap().last_stars = Some(msg);
        } else if msg.contains(r#""type":"solver_status""#) {
            self.0.lock().unwrap().last_solver_status = Some(msg);
        }
    }
}

/// Collect events from the detection and FF broadcast channels.
pub async fn history_task(
    history: EventHistory,
    mut detection_rx: broadcast::Receiver<String>,
    mut ff_rx: broadcast::Receiver<String>,
    app_state: crate::config::SharedAppState,
    shutdown: Arc<AtomicBool>,
) {
    let mut was_active = false;
    let mut tick = tokio::time::interval(Duration::from_secs(1));

    while !shutdown.load(Ordering::Relaxed) {
        tokio::select! {
            r = detection_rx.recv() => match r {
                Ok(msg) => {
                    // Skip the per-frame `det` debug stream.
                    if msg.starts_with(r#"{"type":"meteor""#) {
                        history.push(msg);
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            },
            r = ff_rx.recv() => match r {
                Ok(msg) => history.record_ff(msg),
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            },
            _ = tick.tick() => {
                let s = app_state.load();
                let active = s.detection_enabled
                    && crate::config::schedule_active(
                        chrono::Local::now(),
                        s.detection_use_twilight,
                        s.detection_twilight_astro,
                        s.detection_start_hour,
                        s.detection_end_hour,
                        s.latitude,
                        s.longitude,
                    );
                if active && !was_active {
                    history.clear();
                    log::info!("Detection period started: event history cleared");
                }
                was_active = active;
            }
        }
    }
}
