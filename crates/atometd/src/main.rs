pub mod ae_ctrl;
mod config;
mod daynight;
mod detection;
mod font;
mod gpio;
mod isp;
mod luma;
mod muxer;
mod osd;
mod record;
mod solve;
mod stream;
mod system;
mod watchdog;
mod web;
mod webhook;
mod websocket;

use arc_swap::ArcSwap;
use config::{SharedAppState, load_config, save_config};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, watch};

use simplelog::{CombinedLogger, LevelFilter, SimpleLogger, WriteLogger};
use std::fs::OpenOptions;

#[tokio::main]
async fn main() {
    println!("atometd starting...");
    // 1. Logger init (console + /media/mmc/atomet.log)
    // Rotate log if over 1MB
    const LOG_PATH: &str = "/media/mmc/atomet.log";
    const LOG_MAX_BYTES: u64 = 1024 * 1024;
    if let Ok(meta) = std::fs::metadata(LOG_PATH)
        && meta.len() > LOG_MAX_BYTES
    {
        let _ = std::fs::rename(LOG_PATH, "/media/mmc/atomet.log.1");
    }
    let log_file = OpenOptions::new().append(true).create(true).open(LOG_PATH);

    match log_file {
        Ok(file) => {
            CombinedLogger::init(vec![
                SimpleLogger::new(LevelFilter::Info, simplelog::Config::default()),
                WriteLogger::new(LevelFilter::Info, simplelog::Config::default(), file),
            ])
            .unwrap();
        }
        Err(_) => {
            // Fallback to console-only logging (e.g. no SD card)
            SimpleLogger::init(LevelFilter::Info, simplelog::Config::default()).unwrap();
        }
    }

    log::info!("atometd starting");

    // 2. Load config
    let app_state_data = load_config().await;
    let app_state: SharedAppState = Arc::new(ArcSwap::from_pointee(app_state_data.clone()));

    // 3. Hardware watchdog
    let hw_watchdog = match watchdog::HwWatchdog::init(15) {
        Ok(wd) => {
            log::info!("Hardware watchdog initialized (15s timeout)");
            Some(wd)
        }
        Err(e) => {
            log::warn!("Hardware watchdog not available: {}", e);
            None
        }
    };

    // 4. Watchdog supervisor + handles
    let mut supervisor = watchdog::WatchdogSupervisor::new(Duration::from_secs(10), hw_watchdog);
    let wd_video_worker = supervisor.register("video_worker");
    let wd_timelapse_worker = supervisor.register("timelapse_worker");
    let wd_stream = supervisor.register("stream");
    let wd_record = supervisor.register("record");
    let wd_timelapse = supervisor.register("timelapse");
    let wd_luma = supervisor.register("luma");
    let wd_detection = supervisor.register("detection");
    let wd_detection_record = supervisor.register("detection_record");
    // luma watch channel
    let (luma_tx, luma_rx) = watch::channel::<Option<luma::LumaFrame>>(None);

    // 5. Shutdown flag
    let shutdown = Arc::new(AtomicBool::new(false));

    // 6. GPIO init
    if let Err(e) = gpio::gpio_init().await {
        log::error!("GPIO init failed: {}", e);
    } else {
        log::info!("GPIO initialized");

        let _ = gpio::led_off(gpio::Led::Orange).await;

        // Apply saved GPIO state
        if app_state_data.ircut_on {
            let _ = gpio::ircut_on().await;
        }
        if app_state_data.irled_on {
            let _ = gpio::irled_on().await;
        }
    }

    // 7. ISP init (blocking)
    let isp_state = app_state_data.clone();
    let isp_ok = tokio::task::spawn_blocking(move || unsafe {
        if !isp::isp_init(&isp_state) {
            return false;
        }
        log::info!("ISP initialized");
        isp::log_isp_values();

        if !isp::framesource_init() {
            return false;
        }
        log::info!("Framesource initialized");

        if !isp::encoder_init() {
            return false;
        }
        log::info!("Encoder initialized");

        if !isp::framesource_start() {
            return false;
        }
        log::info!("Framesource started");

        true
    })
    .await
    .unwrap();

    if !isp_ok {
        log::error!("ISP pipeline initialization failed");
        return;
    }

    // 8. Broadcast channels for streaming tasks
    let (tx_hevc, rx_hevc) = broadcast::channel(100);
    let (tx_tl, rx_tl) = mpsc::channel(100);

    // 9. Spawn tasks
    // OSD task (blocking — polls OSD for updates)
    let shutdown_clone = Arc::clone(&shutdown);
    let app_state_clone = Arc::clone(&app_state);
    tokio::task::spawn_blocking(move || {
        osd::osd_poll_loop(app_state_clone, shutdown_clone);
    });

    // H.264 stream task (blocking — polls ISP encoder and broadcasts via watch channel)
    let shutdown_clone = Arc::clone(&shutdown);
    tokio::task::spawn_blocking(move || unsafe {
        isp::video_poll_worker(shutdown_clone, wd_video_worker, tx_hevc);
    });

    let shutdown_clone = Arc::clone(&shutdown);
    tokio::task::spawn_blocking(move || unsafe {
        isp::timelapse_poll_worker(shutdown_clone, wd_timelapse_worker, tx_tl);
    });

    // Luma task (Y-plane for detection / daynight)
    let shutdown_clone = Arc::clone(&shutdown);
    tokio::task::spawn_blocking(move || unsafe {
        luma::luma_poll_worker(shutdown_clone, wd_luma, luma_tx);
    });

    // Record task
    log::info!("Spawning record task");
    let shutdown_clone = Arc::clone(&shutdown);
    let rx_record = rx_hevc.resubscribe();
    tokio::spawn(record::record_regular_task(
        shutdown_clone,
        wd_record,
        rx_record,
        Arc::clone(&app_state),
    ));

    // Detection active signal (for triggered recording)
    let detection_active = Arc::new(AtomicBool::new(false));

    // Detection-triggered recording task
    let shutdown_clone = Arc::clone(&shutdown);
    let rx_detection_record = rx_hevc.resubscribe();
    let detection_active_clone = Arc::clone(&detection_active);
    tokio::spawn(record::record_detection_task(
        shutdown_clone,
        wd_detection_record,
        rx_detection_record,
        detection_active_clone,
        Arc::clone(&app_state),
    ));

    // Timelapse task
    let shutdown_clone = Arc::clone(&shutdown);
    tokio::spawn(record::record_timelapse_task(
        shutdown_clone,
        wd_timelapse,
        rx_tl,
        Arc::clone(&app_state),
    ));

    // Disk cleanup task
    let shutdown_clone = Arc::clone(&shutdown);
    tokio::spawn(record::disk_cleanup_task(shutdown_clone));

    // Stream task
    log::info!("Spawning stream task");
    let shutdown_clone = Arc::clone(&shutdown);
    tokio::spawn(stream::broadcast_task(shutdown_clone, wd_stream, rx_hevc));

    // Load mask (80×45 = 3600 bytes, one byte per cell)
    // Wrapped in ArcSwap so PUT /api/mask can hot-swap at runtime.
    const MASK_PATH: &str = "/media/mmc/mask.bin";
    const MASK_SIZE: usize = 80 * 45;
    let mask_data: Arc<ArcSwap<Vec<u8>>> =
        Arc::new(ArcSwap::from_pointee(match std::fs::read(MASK_PATH) {
            Ok(data) if data.len() == MASK_SIZE => {
                let masked = data.iter().filter(|&&v| v != 0).count();
                log::info!("Loaded mask from {} ({} cells masked)", MASK_PATH, masked);
                data
            }
            _ => {
                log::info!(
                    "No mask loaded ({}), detection/solve use full frame",
                    MASK_PATH
                );
                vec![0u8; MASK_SIZE]
            }
        }));

    // Detection debug broadcast channel
    let (detection_tx, _) = broadcast::channel::<String>(32);

    // Detection task
    let shutdown_clone = Arc::clone(&shutdown);
    let app_state_clone = Arc::clone(&app_state);
    let detection_tx_clone = detection_tx.clone();
    let luma_rx_detection = luma_rx.clone();
    let mask_clone = Arc::clone(&mask_data);
    let detection_active_clone = Arc::clone(&detection_active);
    tokio::task::spawn_blocking(move || {
        detection::detection_task(
            luma_rx_detection,
            app_state_clone,
            shutdown_clone,
            wd_detection,
            detection_tx_clone,
            mask_clone,
            detection_active_clone,
        );
    });

    // Webhook notification task
    let (webhook_event_tx, webhook_event_rx) = mpsc::channel::<webhook::WebhookEvent>(32);
    let shutdown_clone = Arc::clone(&shutdown);
    let app_state_clone = Arc::clone(&app_state);
    let webhook_detection_rx = detection_tx.subscribe();
    tokio::spawn(webhook::webhook_task(webhook_detection_rx, webhook_event_rx, app_state_clone, shutdown_clone));
    let _ = webhook_event_tx.send(webhook::WebhookEvent::Startup).await;

    // Daynight auto task
    let shutdown_clone = Arc::clone(&shutdown);
    let app_state_clone = Arc::clone(&app_state);
    tokio::spawn(daynight::daynight_task(app_state_clone, shutdown_clone));

    let wd_solve = supervisor.register("solve");

    // Watchdog supervisor
    let shutdown_clone = Arc::clone(&shutdown);
    tokio::spawn(supervisor.run(shutdown_clone));

    // Sysstat broadcast channel + task
    let (sysstat_tx, _) = broadcast::channel::<String>(16);
    let sysstat_tx_clone = sysstat_tx.clone();
    let shutdown_clone = Arc::clone(&shutdown);
    tokio::spawn(system::sysstat_broadcast_task(
        sysstat_tx_clone,
        shutdown_clone,
    ));

    // Ispstat broadcast channel + task
    let sysstat_tx_clone = sysstat_tx.clone();
    let shutdown_clone = Arc::clone(&shutdown);
    tokio::spawn(system::ispstat_broadcast_task(
        sysstat_tx_clone,
        shutdown_clone,
    ));

    // 10. Shutdown signal channel — used by both config save and axum graceful shutdown
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let shutdown_clone = Arc::clone(&shutdown);
    let app_state_clone = Arc::clone(&app_state);
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            log::info!("Received Ctrl+C — shutting down");
            shutdown_clone.store(true, Ordering::Relaxed);
            let state = app_state_clone.load_full();
            if let Err(e) = save_config(&state).await {
                log::error!("Failed to save config on shutdown: {}", e);
            }
            let _ = shutdown_tx.send(());
        }
    });

    // 11. axum server on port 80
    let stack_capture = Arc::new(AtomicBool::new(false));

    // Solve task
    let shutdown_clone = Arc::clone(&shutdown);
    let app_state_clone = Arc::clone(&app_state);
    let detection_tx_clone = detection_tx.clone();
    let luma_rx_solve = luma_rx.clone();
    let stack_capture_clone = Arc::clone(&stack_capture);
    let mask_clone = Arc::clone(&mask_data);
    tokio::task::spawn_blocking(move || {
        solve::solve_task(
            luma_rx_solve,
            app_state_clone,
            shutdown_clone,
            wd_solve,
            detection_tx_clone,
            stack_capture_clone,
            mask_clone,
        );
    });

    let web_state = web::WebState {
        app_state,
        shutdown,
        sysstat_tx,
        detection_tx,
        stack_capture: Arc::clone(&stack_capture),
        mask: Arc::clone(&mask_data),
        webhook_tx: webhook_event_tx,
    };
    let app = web::build_router(web_state);

    log::info!("Starting HTTP server on 0.0.0.0:80");
    let listener = match tokio::net::TcpListener::bind("0.0.0.0:80").await {
        Ok(l) => l,
        Err(e) => {
            log::error!("Failed to bind port 80: {}", e);
            return;
        }
    };

    if let Err(e) = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        let _ = shutdown_rx.await;
        log::info!("HTTP server shutting down");
    })
    .await
    {
        log::error!("Server error: {}", e);
    }

    log::info!("atometd stopped");
}
