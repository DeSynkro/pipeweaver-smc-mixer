mod app;
mod single_instance;
mod theme;
mod tray;

use anyhow::Result;
use clap::Parser;
use eframe::egui;
use pipeweaver_smc_core::config::{Config, state_dir};
use simplelog::{
    ColorChoice, CombinedLogger, ConfigBuilder, LevelFilter, SharedLogger, TermLogger,
    TerminalMode, WriteLogger,
};
use std::fs::{self, OpenOptions};
use std::sync::atomic::{AtomicBool, Ordering};
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

    while !stop.load(Ordering::Relaxed) {
        if ui_on.load(Ordering::Relaxed) {
            let config = Config::load(&config_path);
            let options = window_options(&config);

            let stop = stop.clone();
            let ui_on = ui_on.clone();
            let focus = focus.clone();
            let config_path = config_path.clone();
            let tray_context = tray_context.clone();

            eframe::run_native(
                theme::APP_NAME,
                options,
                Box::new(move |cc| {
                    cc.egui_ctx.set_visuals(theme::visuals());
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
            // Keeping the window non-resizable while it is mapped makes tiling
            // managers such as Krohnkite leave it floating. Resizing is
            // re-enabled once the window has settled, and the X11 window type
            // is a second signal.
            .with_resizable(false)
            .with_window_type(egui::X11WindowType::Utility)
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
