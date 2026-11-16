pub mod ae_ctrl;
mod clockstat;
mod config;
mod daynight;
mod detection;
mod ff;
mod fits;
mod font;
mod gpio;
mod history;
mod isp;
mod luma;
mod muxer;
mod osd;
mod paths;
mod record;
mod showers;
mod solve;
mod stream;
mod system;
mod watchdog;
mod wcs;
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
    // 1. Logger init (console + paths::LOG_FILE)
    // Rotate log if over 1MB
    const LOG_MAX_BYTES: u64 = 1024 * 1024;
    let _ = std::fs::create_dir_all(paths::LOG_DIR);
    if let Ok(meta) = std::fs::metadata(paths::LOG_FILE)
        && meta.len() > LOG_MAX_BYTES
    {
        let _ = std::fs::rename(paths::LOG_FILE, paths::LOG_FILE_OLD);
    }
    let log_file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(paths::LOG_FILE);

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

    // Capture panic info to log + stderr before abort kicks in.
    // (panic = "abort" otherwise leaves no trace in atomet.log.)
    // stderr is flushed explicitly because the runtime may exit before
    // any line buffer drains.
    std::panic::set_hook(Box::new(|info| {
        use std::io::Write;
        let msg = format!("PANIC: {info}");
        let _ = writeln!(std::io::stderr(), "{msg}");
        let _ = std::io::stderr().flush();
        log::error!("{msg}");
    }));

    log::info!("atometd starting");

    // 2. Load config
    let app_state_data = load_config().await;
    system::apply_timezone(&app_state_data.timezone);
    // Re-apply persisted ISP bypass bits before any frame is captured.
    if app_state_data.isp_bypass.len() == 32 {
        match ae_ctrl::AtometDev::open() {
            Ok(dev) => {
                let mut bp = ae_ctrl::TopBypass::default();
                for (i, &v) in app_state_data.isp_bypass.iter().enumerate() {
                    bp.bits[i] = v;
                }
                if let Err(e) = dev.set_bypass(&bp) {
                    log::warn!("Failed to apply persisted ISP bypass: {}", e);
                } else {
                    log::info!("Applied persisted ISP bypass bits");
                }
            }
            Err(e) => log::warn!("ISP bypass restore skipped (cannot open /dev/atomet): {}", e),
        }
    }
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

        // Apply saved GPIO state explicitly in both directions so persisted
        // OFF values aren't masked by hardware defaults.
        let _ = if app_state_data.ircut_on { gpio::ircut_on().await } else { gpio::ircut_off().await };
        let _ = if app_state_data.irled_on { gpio::irled_on().await } else { gpio::irled_off().await };
        let _ = if app_state_data.led_on {
            gpio::led_on(gpio::Led::Blue).await
        } else {
            gpio::led_off(gpio::Led::Blue).await
        };
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

        // Disable ISP digital gain (cap to 0 = 1x)
        match crate::ae_ctrl::AtometDev::open() {
            Ok(_dev) => {
                /*if let Err(e) = dev.set_max_idg(256) {
                    log::error!("Failed to disable ISP DG: {}", e);
                } else {
                    log::info!("ISP digital gain disabled (max_idg=256)");
                }*/
            }
            Err(e) => log::error!("Failed to open /dev/atomet for DG disable: {}", e),
        }

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

    // Detection signals (for triggered recording)
    let detection_active = Arc::new(AtomicBool::new(false));
    let detection_tracking = Arc::new(AtomicBool::new(false));

    // Detection-triggered recording task
    let shutdown_clone = Arc::clone(&shutdown);
    let rx_detection_record = rx_hevc.resubscribe();
    let detection_active_clone = Arc::clone(&detection_active);
    let detection_tracking_clone = Arc::clone(&detection_tracking);
    tokio::spawn(record::record_detection_task(
        shutdown_clone,
        wd_detection_record,
        rx_detection_record,
        detection_active_clone,
        detection_tracking_clone,
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
    const MASK_SIZE: usize = 80 * 45;
    let mask_data: Arc<ArcSwap<Vec<u8>>> =
        Arc::new(ArcSwap::from_pointee(match std::fs::read(paths::MASK_BIN) {
            Ok(data) if data.len() == MASK_SIZE => {
                let masked = data.iter().filter(|&&v| v != 0).count();
                log::info!(
                    "Loaded mask from {} ({} cells masked)",
                    paths::MASK_BIN,
                    masked
                );
                data
            }
            _ => {
                log::info!(
                    "No mask loaded ({}), detection/solve use full frame",
                    paths::MASK_BIN
                );
                vec![0u8; MASK_SIZE]
            }
        }));

    // Detection debug broadcast channel
    let (detection_tx, _) = broadcast::channel::<String>(32);
    // Meteor-only events (no per-frame debug stream) for the FF saver.
    let (meteor_tx, _) = broadcast::channel::<String>(32);

    // Detection task
    let shutdown_clone = Arc::clone(&shutdown);
    let app_state_clone = Arc::clone(&app_state);
    let detection_tx_clone = detection_tx.clone();
    let meteor_tx_clone = meteor_tx.clone();
    let luma_rx_detection = luma_rx.clone();
    let mask_clone = Arc::clone(&mask_data);
    let detection_active_clone = Arc::clone(&detection_active);
    let detection_tracking_clone = Arc::clone(&detection_tracking);
    tokio::task::spawn_blocking(move || {
        detection::detection_task(
            luma_rx_detection,
            app_state_clone,
            shutdown_clone,
            wd_detection,
            detection_tx_clone,
            meteor_tx_clone,
            mask_clone,
            detection_active_clone,
            detection_tracking_clone,
        );
    });

    // FF stacker: 256-frame temporal stack producing maxpixel/maxframe/avepixel/stdpixel.
    let wd_ff_stack = supervisor.register("ff_stack");
    let (ff_block_tx, ff_block_rx) = mpsc::channel::<ff::FfBlock>(2);
    let luma_rx_ff = luma_rx.clone();
    let shutdown_clone = Arc::clone(&shutdown);
    let app_state_ff = Arc::clone(&app_state);
    tokio::task::spawn_blocking(move || {
        ff::ff_stack_worker(luma_rx_ff, ff_block_tx, shutdown_clone, wd_ff_stack, app_state_ff);
    });

    // Plate solver database, used by the FF saver (automatic solve).
    let solver_db = Arc::new(solve::load_database());

    // FF smart saver: saves detected / interesting / baseline blocks.
    let (ff_event_tx, _) = broadcast::channel::<String>(16);
    let ff_detection_rx = meteor_tx.subscribe();
    let shutdown_clone = Arc::clone(&shutdown);
    tokio::spawn(ff::ff_save_smart_task(
        ff_block_rx,
        ff_detection_rx,
        ff_event_tx.clone(),
        shutdown_clone,
        Arc::clone(&mask_data),
        Arc::clone(&solver_db),
        app_state_data.fps,
        Arc::clone(&app_state),
    ));

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

    // Scheduled reboot task
    let app_state_clone = Arc::clone(&app_state);
    let shutdown_clone = Arc::clone(&shutdown);
    tokio::spawn(system::scheduled_reboot_task(
        app_state_clone,
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

    // Event history for reconnecting web clients
    let event_history = history::EventHistory::default();
    tokio::spawn(history::history_task(
        event_history.clone(),
        detection_tx.subscribe(),
        ff_event_tx.subscribe(),
        Arc::clone(&app_state),
        Arc::clone(&shutdown),
    ));

    // 11. axum server on port 80
    let web_state = web::WebState {
        history: event_history,
        app_state,
        sysstat_tx,
        detection_tx,
        ff_tx: ff_event_tx,
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
