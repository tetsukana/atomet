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
use tokio::sync::broadcast;
use tower_http::services::ServeDir;

use crate::config::SharedAppState;
use crate::webhook::WebhookEvent;
use crate::websocket;

/// Shared state passed to axum handlers via `with_state`.
#[derive(Clone)]
pub struct WebState {
    pub app_state: SharedAppState,
    pub shutdown: Arc<AtomicBool>,
    pub sysstat_tx: broadcast::Sender<String>,
    pub detection_tx: broadcast::Sender<String>,
    pub stack_capture: Arc<AtomicBool>,
    pub mask: Arc<ArcSwap<Vec<u8>>>,
    pub webhook_tx: tokio::sync::mpsc::Sender<WebhookEvent>,
}

/// Build the axum router.
pub fn build_router(state: WebState) -> Router {
    let web_dir = resolve_web_dir();
    log::info!("Serving static files from: {}", web_dir);

    Router::new()
        .route("/ws", get(websocket::ws_handler))
        .route("/api/state", get(api_get_state))
        .route("/api/files", get(api_list_files))
        .route("/api/debug/stack", get(api_debug_stack))
        .route("/api/mask", get(api_get_mask).put(api_put_mask))
        .nest_service("/files/records", ServeDir::new("/media/mmc/records"))
        .nest_service("/files/timelapse", ServeDir::new("/media/mmc/timelapse"))
        .nest_service("/files/detections", ServeDir::new("/media/mmc/detections"))
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
        Some("records") => "/media/mmc/records",
        Some("timelapse") => "/media/mmc/timelapse",
        Some("detections") => "/media/mmc/detections",
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

async fn api_debug_stack(
    axum::extract::State(state): axum::extract::State<WebState>,
) -> axum::Json<serde_json::Value> {
    use std::sync::atomic::Ordering;
    if state
        .stack_capture
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        axum::Json(serde_json::json!({ "status": "capturing" }))
    } else {
        axum::Json(serde_json::json!({ "status": "already in progress" }))
    }
}

const MASK_PATH: &str = "/media/mmc/mask.bin";
const MASK_SIZE: usize = 80 * 45; // 3600 bytes, one byte per cell

async fn api_get_mask() -> Response<axum::body::Body> {
    match tokio::fs::read(MASK_PATH).await {
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
    match tokio::fs::write(MASK_PATH, &data).await {
        Ok(()) => {
            log::info!("Mask saved to {}", MASK_PATH);
            StatusCode::NO_CONTENT
        }
        Err(e) => {
            log::error!("Failed to save mask: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

fn resolve_web_dir() -> &'static str {
    if Path::new("/media/mmc/web").exists() {
        "/media/mmc/web"
    } else {
        "/var/www/atomet"
    }
}
