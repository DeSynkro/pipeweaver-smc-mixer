mod app;
mod media;
mod meters;
mod midi;
mod single_instance;
mod theme;
mod tray;

use anyhow::Result;
use clap::Parser;
use eframe::egui;
use pipeweaver_ipc::client::Client;
use pipeweaver_ipc::clients::web::web_client::WebClient;
use pipeweaver_smc_core::Engine;
use pipeweaver_smc_core::config::{Config, state_dir};
use pipeweaver_websocket_client::{BroadcastMessage, spawn_pipeweaver_handler};
use simplelog::{
    ColorChoice, CombinedLogger, ConfigBuilder, LevelFilter, SharedLogger, TermLogger,
    TerminalMode, WriteLogger,
};
use std::fs::{self, OpenOptions};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(name = "pipeweaver-smc-mixer", version, about)]
struct Args {
    /// Start in the tray without opening the window.
    #[arg(short, long, default_value_t = false)]
    quiet: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    setup_logging();
    log::info!("Starting {}", theme::APP_NAME);

    let Some(_lock) = single_instance::InstanceLock::acquire()? else {
        log::error!("Another instance is already running");
        return Ok(());
    };

    let config_path = Config::path();
    let initial = Config::load(&config_path);
    let start_minimized = args.quiet || initial.start_minimized;

    let stop = Arc::new(AtomicBool::new(false));
    let ui_on = Arc::new(AtomicBool::new(!start_minimized));
    let focus = Arc::new(AtomicBool::new(false));
    let minimized = Arc::new(AtomicBool::new(false));

    let tray_context = Arc::new(tray::TrayContext {
        ui_on: ui_on.clone(),
        focus: focus.clone(),
        minimized: minimized.clone(),
        egui: Arc::new(Mutex::new(None)),
    });
    let tray_handle = tokio::spawn(tray::spawn(stop.clone(), tray_context.clone()));

    let midi_link = Arc::new(AtomicU8::new(midi::Link::Offline.code()));
    {
        let link = midi_link.clone();
        let config_path = config_path.clone();
        tokio::spawn(async move {
            loop {
                let config = Config::load(&config_path);
                let detected = midi::detect(&config);
                log::debug!("MIDI link: {:?}", detected);
                link.store(detected.code(), Ordering::Relaxed);
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
    }

    let engine = Engine::new(initial.surface_sources);
    let mic_muted = Arc::new(AtomicBool::new(false));
    let (media_tx, media_rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(media::run(
        media_rx,
        ui_on.clone(),
        focus.clone(),
        mic_muted.clone(),
        minimized.clone(),
        tray_context.egui.clone(),
    ));
    let (command_tx, mut command_rx) = tokio::sync::mpsc::channel(1024);
    {
        let (broadcast_tx, mut broadcast_rx) = tokio::sync::broadcast::channel(10);
        let (handler_tx, handler_rx) = tokio::sync::mpsc::channel(1);
        let stop_handler = spawn_pipeweaver_handler(handler_rx, broadcast_tx).await;
        drop(handler_tx);

        // Commands go over the daemon's HTTP API with a timeout. Keeping them
        // off the websocket means a stalled command send can never block the
        // status patches that keep the surface alive.
        tokio::spawn(async move {
            let mut client = WebClient::new(String::from("http://localhost:14565/api/command"));
            while let Some(request) = command_rx.recv().await {
                match tokio::time::timeout(Duration::from_secs(2), client.send(&request)).await {
                    Ok(Ok(_)) => {}
                    Ok(Err(error)) => log::warn!("Pipeweaver command failed: {error:#}"),
                    Err(_) => {
                        log::warn!("Pipeweaver command timed out");
                        while command_rx.try_recv().is_ok() {}
                    }
                }
            }
        });

        let engine = engine.clone();
        tokio::spawn(async move {
            let _stop_handler = stop_handler;
            while let Ok(message) = broadcast_rx.recv().await {
                match message {
                    BroadcastMessage::Online => engine.set_connected(true),
                    BroadcastMessage::Offline => {
                        engine.set_connected(false);
                        engine.clear_meters();
                    }
                    BroadcastMessage::Status(status) => {
                        engine.set_connected(true);
                        engine.apply(*status);
                    }
                }
            }
        });
    }
    let swap = Arc::new(AtomicU8::new(0));
    let pickup = Arc::new(AtomicBool::new(initial.pickup));
    let record_hex = Arc::new(AtomicBool::new(initial.record_hex));
    let ui_commands = command_tx.clone();
    {
        let config = initial.clone();
        let engine = engine.clone();
        let swap = swap.clone();
        let pickup = pickup.clone();
        let midi_media = media_tx.clone();
        let midi_ui_on = ui_on.clone();
        let midi_focus = focus.clone();
        let midi_record_hex = record_hex.clone();
        let midi_minimized = minimized.clone();
        let midi_egui = tray_context.egui.clone();
        let midi_link = midi_link.clone();
        std::thread::spawn(move || {
            midi::run_controls(
                config,
                engine,
                command_tx,
                swap,
                pickup,
                midi_media,
                midi_ui_on,
                midi_focus,
                midi_record_hex,
                midi_minimized,
                midi_egui,
                midi_link,
            )
        });
    }
    tokio::spawn(meters::run(engine.clone()));

    while !stop.load(Ordering::Relaxed) {
        if ui_on.load(Ordering::Relaxed) {
            let config = Config::load(&config_path);
            let options = window_options(&config);

            let stop = stop.clone();
            let ui_on = ui_on.clone();
            let focus = focus.clone();
            let config_path = config_path.clone();
            let tray_context = tray_context.clone();
            let engine = engine.clone();
            let midi_link = midi_link.clone();
            let swap = swap.clone();
            let pickup = pickup.clone();
            let ui_commands = ui_commands.clone();
            let media_tx = media_tx.clone();
            let record_hex = record_hex.clone();
            let mic_muted = mic_muted.clone();
            let minimized = minimized.clone();

            eframe::run_native(
                theme::APP_NAME,
                options,
                Box::new(move |cc| {
                    cc.egui_ctx.set_visuals(theme::visuals(config.light_panel));
                    cc.egui_ctx
                        .all_styles_mut(|style| style.interaction.tooltip_delay = 1.0);
                    egui_extras::install_image_loaders(&cc.egui_ctx);
                    *tray_context.egui.lock().unwrap() = Some(cc.egui_ctx.clone());
                    Ok(Box::new(app::SmcApp::new(
                        config,
                        config_path,
                        stop,
                        ui_on,
                        focus,
                        engine,
                        midi_link,
                        swap,
                        pickup,
                        ui_commands,
                        media_tx,
                        record_hex,
                        mic_muted,
                        minimized,
                    )))
                }),
            )?;
        } else {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    let _ = tray_handle.await;
    log::info!("Stopped {}", theme::APP_NAME);
    Ok(())
}

fn window_options(config: &Config) -> eframe::NativeOptions {
    let width = config.window_width();
    let height = width / theme::ASPECT;
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([width, height])
            .with_min_inner_size([
                theme::MIN_WINDOW_WIDTH,
                theme::MIN_WINDOW_WIDTH / theme::ASPECT,
            ])
            .with_decorations(false)
            .with_transparent(true)
            // Keeping the window non-resizable while it is mapped makes tiling
            // managers such as Krohnkite leave it floating. Resizing is
            // re-enabled once the window has settled. The window stays a normal
            // toplevel so the taskbar entry and minimise button work.
            .with_resizable(false)
            .with_app_id(theme::APP_ID)
            .with_title(theme::APP_NAME)
            .with_icon(Arc::new(window_icon())),
        ..Default::default()
    }
}

fn window_icon() -> egui::IconData {
    let image = image::load_from_memory_with_format(theme::ICON_PNG, image::ImageFormat::Png)
        .expect("load application icon")
        .resize_exact(256, 256, image::imageops::FilterType::Lanczos3);
    let rgba = image.into_rgba8();
    let (width, height) = rgba.dimensions();
    egui::IconData {
        rgba: rgba.into_raw(),
        width,
        height,
    }
}

fn setup_logging() {
    let dir = state_dir();
    let _ = fs::create_dir_all(&dir);
    let log_path = dir.join("pipeweaver-smc-mixer.log");
    let config = ConfigBuilder::new()
        .add_filter_allow_str("pipeweaver_smc_mixer")
        .add_filter_allow_str("pipeweaver_smc_core")
        .build();

    let term = TermLogger::new(
        LevelFilter::Info,
        config.clone(),
        TerminalMode::Stderr,
        ColorChoice::Auto,
    );
    let mut loggers: Vec<Box<dyn SharedLogger>> = vec![term];

    match OpenOptions::new().create(true).append(true).open(&log_path) {
        Ok(file) => loggers.push(WriteLogger::new(LevelFilter::Info, config, file)),
        Err(error) => eprintln!("Could not open {}: {error}", log_path.display()),
    }

    let _ = CombinedLogger::init(loggers);
}
