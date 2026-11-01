mod app;
mod meters;
mod midi;
mod single_instance;
mod theme;
mod tray;

use anyhow::Result;
use clap::Parser;
use eframe::egui;
use pipeweaver_smc_core::config::{Config, state_dir};
use pipeweaver_smc_core::Engine;
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

    let tray_context = Arc::new(tray::TrayContext {
        ui_on: ui_on.clone(),
        focus: focus.clone(),
        egui: Arc::new(Mutex::new(None)),
    });
    let tray_handle = tokio::spawn(tray::spawn(stop.clone(), tray_context.clone()));

    let midi_link = Arc::new(AtomicU8::new(midi::Link::Offline.code()));
    {
        let link = midi_link.clone();
        let config = initial.clone();
        tokio::spawn(async move {
            loop {
                let detected = midi::detect(&config);
                log::debug!("MIDI link: {:?}", detected);
                link.store(detected.code(), Ordering::Relaxed);
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
    }

    let engine = Engine::new(initial.surface_sources);
    let (command_tx, command_rx) = tokio::sync::mpsc::channel(128);
    {
        let (broadcast_tx, mut broadcast_rx) = tokio::sync::broadcast::channel(10);
        let stop_handler = spawn_pipeweaver_handler(command_rx, broadcast_tx).await;
        let engine = engine.clone();
        let command_tx = command_tx.clone();
        tokio::spawn(async move {
            let _command_tx = command_tx;
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
    let ui_commands = command_tx.clone();
    {
        let config = initial.clone();
        let engine = engine.clone();
        let swap = swap.clone();
        let pickup = pickup.clone();
        std::thread::spawn(move || midi::run_controls(config, engine, command_tx, swap, pickup));
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
