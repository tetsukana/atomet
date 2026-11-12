use arc_swap::ArcSwap;
use axum::Router;
use axum::body::Bytes;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::http::header::{self, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;
use axum::routing::get;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::sync::{broadcast, watch};
use tower_http::services::ServeDir;

use crate::config::SharedAppState;
use crate::luma;
use crate::paths;
use crate::webhook::WebhookEvent;
use crate::websocket;
use solver::Database;

/// Shared state passed to axum handlers via `with_state`.
#[derive(Clone)]
pub struct WebState {
    pub app_state: SharedAppState,
    pub shutdown: Arc<AtomicBool>,
    pub sysstat_tx: broadcast::Sender<String>,
    pub detection_tx: broadcast::Sender<String>,
    pub ff_tx: broadcast::Sender<String>,
    pub mask: Arc<ArcSwap<Vec<u8>>>,
    pub webhook_tx: tokio::sync::mpsc::Sender<WebhookEvent>,
    /// Latest luma frame (640×360 Y-plane), for on-demand still capture.
    pub luma_rx: watch::Receiver<Option<luma::LumaFrame>>,
    /// Plate solver database (shared with the realtime solve task).
    pub solver_db: Arc<Option<Database>>,
    /// Stars from the latest FF block (what the automatic solve used).
    pub latest_stars: Arc<arc_swap::ArcSwapOption<crate::ff::LatestStars>>,
}

/// Build the axum router.
pub fn build_router(state: WebState) -> Router {
    let web_dir = resolve_web_dir();
    log::info!("Serving static files from: {}", web_dir);

    Router::new()
        .route("/ws", get(websocket::ws_handler))
        .route("/api/state", get(api_get_state))
        .route("/api/files", get(api_list_files))
        .route("/api/mask", get(api_get_mask).put(api_put_mask))
        .route("/api/solve/capture", get(api_solve_capture))
        .route("/api/solve", axum::routing::post(api_solve))
        .nest_service("/files/records", ServeDir::new(paths::RECORDS_DIR))
        .nest_service("/files/timelapse", ServeDir::new(paths::TIMELAPSE_DIR))
        .nest_service("/files/detections", ServeDir::new(paths::DETECTIONS_DIR))
        .nest_service("/files/ff", ServeDir::new(paths::FF_DIR))
        .nest_service("/files/stills", ServeDir::new(paths::STILLS_DIR))
        .fallback_service(ServeDir::new(web_dir).append_index_html_on_directories(true))
        .layer(axum::middleware::from_fn(fix_content_type))
        .with_state(state)
}

/// Explicitly set Content-Type for static assets.
/// tower-http's ServeDir may omit the header on some platforms/versions.
async fn fix_content_type(request: Request, next: Next) -> Response {
    let path = request.uri().path().to_owned();
    let mut response = next.run(request).await;

    if !response.status().is_success() {
        return response;
    }

    // Only override if Content-Type is missing or empty
    if response.headers().contains_key(header::CONTENT_TYPE) {
        return response;
    }

    let ext = path.rsplit('.').next().unwrap_or("");
    let content_type = match ext {
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "html" => "text/html; charset=utf-8",
        "json" => "application/json",
        "wasm" => "application/wasm",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "mp4" => "video/mp4",
        _ => return response,
    };

    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
}

async fn api_get_state(
    axum::extract::State(state): axum::extract::State<WebState>,
) -> axum::Json<serde_json::Value> {
    let app = state.app_state.load();
    axum::Json(serde_json::to_value(&**app).unwrap_or_default())
}

/// List files in a media directory, walking the date-hierarchy tree.
/// Query: ?dir=records | timelapse | detections
/// Returns files with paths relative to the base dir (e.g. "2024/03/25/14/30.mp4").
async fn api_list_files(
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> axum::Json<serde_json::Value> {
    let base = match params.get("dir").map(|s| s.as_str()) {
        Some("records") => paths::RECORDS_DIR,
        Some("timelapse") => paths::TIMELAPSE_DIR,
        Some("detections") => paths::DETECTIONS_DIR,
        Some("ff") => paths::FF_DIR,
        Some("stills") => paths::STILLS_DIR,
        _ => return axum::Json(serde_json::json!({ "files": [] })),
    };

    let base_path = Path::new(base);
    let mut files: Vec<serde_json::Value> = vec![];
    collect_files_recursive(base_path, base_path, &mut files);

    // Newest first
    files.sort_by(|a, b| {
        b["modified"]
            .as_u64()
            .unwrap_or(0)
            .cmp(&a["modified"].as_u64().unwrap_or(0))
    });

    axum::Json(serde_json::json!({ "files": files }))
}

/// Recursively collect files under `dir`, storing paths relative to `base`.
fn collect_files_recursive(
    base: &Path,
    dir: &Path,
    out: &mut Vec<serde_json::Value>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files_recursive(base, &path, out);
        } else if let Ok(meta) = entry.metadata() {
            let rel = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let size = meta.len();
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            out.push(serde_json::json!({
                "name": rel,
                "size": size,
                "modified": modified,
            }));
        }
    }
}

const MASK_SIZE: usize = 80 * 45; // 3600 bytes, one byte per cell

async fn api_get_mask() -> Response<axum::body::Body> {
    match tokio::fs::read(paths::MASK_BIN).await {
        Ok(data) if data.len() == MASK_SIZE => Response::builder()
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .body(axum::body::Body::from(data))
            .unwrap(),
        _ => {
            // Return all-zeros (no mask)
            let empty = vec![0u8; MASK_SIZE];
            Response::builder()
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .body(axum::body::Body::from(empty))
                .unwrap()
        }
    }
}

async fn api_put_mask(
    axum::extract::State(state): axum::extract::State<WebState>,
    body: Bytes,
) -> StatusCode {
    if body.len() != MASK_SIZE {
        return StatusCode::BAD_REQUEST;
    }
    let data = body.to_vec();
    // Hot-swap in-memory mask (detection + solve pick up immediately)
    state.mask.store(Arc::new(data.clone()));
    let masked = data.iter().filter(|&&v| v != 0).count();
    log::info!("Mask updated live ({} cells masked)", masked);
    // Persist to disk
    if let Some(parent) = Path::new(paths::MASK_BIN).parent()
        && let Err(e) = tokio::fs::create_dir_all(parent).await
    {
        log::error!("Failed to create mask dir: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR;
    }
    match tokio::fs::write(paths::MASK_BIN, &data).await {
        Ok(()) => {
            log::info!("Mask saved to {}", paths::MASK_BIN);
            StatusCode::NO_CONTENT
        }
        Err(e) => {
            log::error!("Failed to save mask: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

/// Return the latest FF block's mean frame together with its extracted stars.
///
/// A binary blob so the image and its star list come from the *same*
/// frame:
///   bytes 0..4      : u32 little-endian metadata length L
///   bytes 4..4+L    : UTF-8 JSON { width, height, threshold, stars:[[x,y,flux],…] }
///   bytes 4+L..     : raw WIDTH×HEIGHT Y-plane — the latest FF block's mean frame
async fn api_solve_capture(
    axum::extract::State(state): axum::extract::State<WebState>,
) -> Response<axum::body::Body> {
    // Serve the latest FF block's mean frame and the stars the automatic
    // solve used, so the page and the solver always agree.
    let Some(latest) = state.latest_stars.load_full() else {
        return Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .body(axum::body::Body::from("no FF block yet — wait for the next block"))
            .unwrap();
    };

    let stars_json: Vec<String> = latest
        .stars
        .iter()
        .map(|s| format!("[{:.1},{:.1},{}]", s[0], s[1], s[2] as u32))
        .collect();
    let meta = format!(
        r#"{{"width":{},"height":{},"threshold":{},"stars":[{}]}}"#,
        luma::WIDTH,
        luma::HEIGHT,
        latest.threshold,
        stars_json.join(","),
    );

    let meta_bytes = meta.into_bytes();
    let mut body = Vec::with_capacity(4 + meta_bytes.len() + latest.avepixel.len());
    body.extend_from_slice(&(meta_bytes.len() as u32).to_le_bytes());
    body.extend_from_slice(&meta_bytes);
    body.extend_from_slice(&latest.avepixel);

    Response::builder()
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .body(axum::body::Body::from(body))
        .unwrap()
}

/// Plate-solve an edited star list. Body: `{ "stars": [[x,y], …] }` in 640×360 px.
async fn api_solve(
    axum::extract::State(state): axum::extract::State<WebState>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> axum::Json<serde_json::Value> {
    if state.solver_db.is_none() {
        return axum::Json(serde_json::json!({
            "ok": false,
            "error": "solver database not loaded",
        }));
    }

    let stars_px: Vec<(f32, f32)> = body
        .get("stars")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|p| {
                    let p = p.as_array()?;
                    let x = p.first()?.as_f64()? as f32;
                    let y = p.get(1)?.as_f64()? as f32;
                    Some((x, y))
                })
                .collect()
        })
        .unwrap_or_default();

    if stars_px.len() < 4 {
        return axum::Json(serde_json::json!({
            "ok": false,
            "error": "need at least 4 stars",
            "count": stars_px.len(),
        }));
    }

    let db = Arc::clone(&state.solver_db);
    let result = tokio::task::spawn_blocking(move || {
        crate::solve::solve_stars(db.as_ref().as_ref().unwrap(), &stars_px, None)
    })
    .await;

    match result {
        Ok(Some(o)) => axum::Json(serde_json::json!({
            "ok": true,
            "ra": o.ra,
            "dec": o.dec,
            "roll": o.roll,
            "votes": o.votes,
            "matched": o.matched,
            "outer": o.outer,
            "rms": o.rms_px,
        })),
        Ok(None) => axum::Json(serde_json::json!({ "ok": true, "solution": null })),
        Err(e) => {
            log::error!("solve task panicked: {}", e);
            axum::Json(serde_json::json!({ "ok": false, "error": "solve task panicked" }))
        }
    }
}

fn resolve_web_dir() -> &'static str {
    if Path::new(paths::WEB_OVERRIDE).exists() {
        paths::WEB_OVERRIDE
    } else {
        "/var/www/atomet"
    }
}
