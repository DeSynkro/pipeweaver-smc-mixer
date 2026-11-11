mod app;
mod knob_mode;
mod media;
mod meters;
mod midi;
mod power;
mod single_instance;
mod theme;
mod tray;

use anyhow::Result;
use clap::Parser;
use eframe::egui;
use pipeweaver_ipc::client::Client;
use pipeweaver_ipc::clients::web::web_client::WebClient;
use pipeweaver_smc_core::Engine;
use pipeweaver_smc_core::config::{Config, autostart_path, set_autostart, state_dir};
use pipeweaver_websocket_client::{BroadcastMessage, spawn_pipeweaver_handler};
use simplelog::{
    ColorChoice, CombinedLogger, ConfigBuilder, LevelFilter, SharedLogger, TermLogger,
    TerminalMode, WriteLogger,
};
use std::fs::{self, OpenOptions};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
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
    if autostart_path().exists()
        && let Err(error) = set_autostart(true, initial.start_minimized)
    {
        log::warn!("Could not update autostart entry: {error:#}");
    }
    let start_minimized = args.quiet || initial.start_minimized;

    let stop = Arc::new(AtomicBool::new(false));
    let power_blackout = power::Blackout::new();
    let pipeweaver_request = Arc::new(AtomicBool::new(false));
    let ui_on = Arc::new(AtomicBool::new(true));
    let focus = Arc::new(AtomicBool::new(false));
    let minimized = Arc::new(AtomicBool::new(false));
    let midi_mode = Arc::new(AtomicU8::new(initial.midi_mode.code()));
    let pickup = Arc::new(AtomicBool::new(initial.pickup));
    let knob_mode = Arc::new(knob_mode::KnobMode::default());
    let record_hex = Arc::new(AtomicBool::new(initial.record_hex));
    let hex_latch = Arc::new(AtomicBool::new(initial.hex_latch));
    let hex_active = Arc::new(AtomicBool::new(false));
    let linked_knob_disabled = Arc::new(AtomicBool::new(initial.linked_knob_disabled));
    let linked_reverse_disabled = Arc::new(AtomicBool::new(initial.linked_reverse_disabled));
    let (media_tx, media_rx) = tokio::sync::mpsc::unbounded_channel();

    let tray_context = Arc::new(tray::TrayContext {
        ui_on: ui_on.clone(),
        focus: focus.clone(),
        minimized: minimized.clone(),
        egui: Arc::new(Mutex::new(None)),
        midi_mode: midi_mode.clone(),
        pickup: pickup.clone(),
        record_hex: record_hex.clone(),
        hex_latch: hex_latch.clone(),
        hex_active: hex_active.clone(),
        media: media_tx.clone(),
        config_path: config_path.clone(),
    });
    let tray_ready = Arc::new(AtomicBool::new(false));
    let tray_handle = tokio::spawn({
        let stop = stop.clone();
        let ui_on = ui_on.clone();
        let egui = tray_context.egui.clone();
        let context = tray_context.clone();
        let ready = tray_ready.clone();
        async move {
            if let Err(error) = tray::spawn(stop.clone(), context, ready.clone()).await {
                log::warn!("Tray unavailable: {error:#}");
            }
            ready.store(false, Ordering::Relaxed);
            if !stop.load(Ordering::Relaxed) {
                // If the tray goes away, never leave the app hidden.
                ui_on.store(true, Ordering::Relaxed);
                if let Some(ctx) = egui.lock().unwrap().as_ref() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                }
            }
        }
    });

    if start_minimized {
        // Give the tray a moment to register. Starting hidden without one
        // would leave the app with no way back.
        for _ in 0..20 {
            if tray_ready.load(Ordering::Relaxed) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    ui_on.store(
        start_visible(start_minimized, tray_ready.load(Ordering::Relaxed)),
        Ordering::Relaxed,
    );

    #[cfg(unix)]
    {
        let stop = stop.clone();
        let egui_ctx = tray_context.egui.clone();
        tokio::spawn(async move {
            use tokio::signal::unix::SignalKind;
            tokio::select! {
                _ = async {
                    match tokio::signal::unix::signal(SignalKind::terminate()) {
                        Ok(mut term) => {
                            term.recv().await;
                        }
                        Err(_) => std::future::pending::<()>().await,
                    }
                } => {}
                _ = async {
                    match tokio::signal::unix::signal(SignalKind::interrupt()) {
                        Ok(mut int) => {
                            int.recv().await;
                        }
                        Err(_) => std::future::pending::<()>().await,
                    }
                } => {}
            }
            stop.store(true, Ordering::Relaxed);
            // The session may kill the MIDI thread before it can black out.
            midi::emergency_blackout();
            if let Some(ctx) = egui_ctx.lock().unwrap().as_ref() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    }

    tokio::spawn(power::run(power_blackout.clone()));

    let midi_link = Arc::new(AtomicU8::new(midi::Link::Offline.code()));
    {
        let link = midi_link.clone();
        let config_path = config_path.clone();
        tokio::spawn(async move {
            loop {
                let config = Config::load(&config_path);
                let detected = midi::detect(&config);
                log::debug!("MIDI port scan: {:?}", detected);
                // Only absence is trustworthy here: a listed port does not
                // mean the app managed to open it. The real connected state
                // comes from the control loop.
                if detected == midi::Link::Offline {
                    link.store(midi::Link::Offline.code(), Ordering::Relaxed);
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
    }

    let engine = Engine::new(initial.surface_sources);
    let mic_muted = Arc::new(AtomicBool::new(false));
    tokio::spawn(media::run(
        media_rx,
        ui_on.clone(),
        focus.clone(),
        mic_muted.clone(),
        minimized.clone(),
        tray_context.egui.clone(),
        pipeweaver_request.clone(),
    ));
    let (command_tx, mut command_rx) = tokio::sync::mpsc::channel(1024);
    {
        let (broadcast_tx, mut broadcast_rx) = tokio::sync::broadcast::channel(64);
        // The handler still takes a command receiver, but commands now go
        // over HTTP. Dropping the sender leaves the channel inert.
        let (_, handler_rx) = tokio::sync::mpsc::channel(1);
        let stop_handler = spawn_pipeweaver_handler(handler_rx, broadcast_tx).await;

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
                        // Keep the queue: a single slow HTTP call must not
                        // discard discrete actions such as mute, link, swap
                        // or routing. Volume updates are idempotent and FIFO,
                        // so the newest one still wins once the daemon is back.
                        log::warn!("Pipeweaver command timed out; keeping the queue");
                    }
                }
            }
        });

        let engine = engine.clone();
        tokio::spawn(async move {
            let _stop_handler = stop_handler;
            let mut client = WebClient::new(String::from("http://localhost:14565/api/command"));
            loop {
                match broadcast_rx.recv().await {
                    Ok(BroadcastMessage::Online) => engine.set_connected(true),
                    Ok(BroadcastMessage::Offline) => {
                        engine.set_connected(false);
                        engine.clear_meters();
                    }
                    Ok(BroadcastMessage::Status(status)) => {
                        engine.set_connected(true);
                        engine.apply(*status);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        // Patches were dropped, so the local state is stale.
                        // Fetch a full status over HTTP and keep the loop
                        // alive; closing the receiver here used to freeze the
                        // whole surface until the app restarted.
                        log::warn!(
                            "Pipeweaver status stream lagged by {skipped} updates; resyncing"
                        );
                        match tokio::time::timeout(Duration::from_secs(2), client.get_status())
                            .await
                        {
                            Ok(Ok(status)) => {
                                engine.set_connected(true);
                                engine.apply(status);
                            }
                            Ok(Err(error)) => {
                                log::warn!("Pipeweaver status resync failed: {error:#}");
                            }
                            Err(_) => log::warn!("Pipeweaver status resync timed out"),
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
    let swap = Arc::new(AtomicU8::new(0));
    let ui_reset = Arc::new(AtomicU8::new(0));
    let last_midi = Arc::new(AtomicU64::new(0));
    let led_test = Arc::new(AtomicBool::new(false));
    let startup_animation = Arc::new(AtomicBool::new(initial.startup_animation));
    let startup_animation_kind = Arc::new(AtomicU8::new(
        midi::StartupAnimation::from_label(&initial.startup_animation_name).code(),
    ));
    let animation_preview = Arc::new(AtomicBool::new(false));
    let ui_commands = command_tx.clone();
    let midi_handle = {
        let config = initial.clone();
        let engine = engine.clone();
        let swap = swap.clone();
        let midi_ui_reset = ui_reset.clone();
        let pickup = pickup.clone();
        let midi_knob_mode = knob_mode.clone();
        let midi_media = media_tx.clone();
        let midi_record_hex = record_hex.clone();
        let midi_hex_latch = hex_latch.clone();
        let midi_hex_active = hex_active.clone();
        let midi_linked_knob_disabled = linked_knob_disabled.clone();
        let midi_linked_reverse_disabled = linked_reverse_disabled.clone();
        let midi_link = midi_link.clone();
        let midi_last_midi = last_midi.clone();
        let midi_led_test = led_test.clone();
        let midi_startup_animation = startup_animation.clone();
        let midi_startup_animation_kind = startup_animation_kind.clone();
        let midi_animation_preview = animation_preview.clone();
        let midi_stop = stop.clone();
        let midi_power_blackout = power_blackout.clone();
        std::thread::spawn(move || {
            midi::run_controls(
                config,
                engine,
                command_tx,
                swap,
                pickup,
                midi_knob_mode,
                midi_ui_reset,
                midi_media,
                midi_record_hex,
                midi_hex_latch,
                midi_hex_active,
                midi_linked_knob_disabled,
                midi_linked_reverse_disabled,
                midi_link,
                midi_last_midi,
                midi_led_test,
                midi_stop,
                midi_power_blackout,
                midi_startup_animation,
                midi_startup_animation_kind,
                midi_animation_preview,
            )
        })
    };
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
            let ui_reset = ui_reset.clone();
            let tray_ready = tray_ready.clone();
            let pickup = pickup.clone();
            let ui_knob_mode = knob_mode.clone();
            let ui_commands = ui_commands.clone();
            let media_tx = media_tx.clone();
            let record_hex = record_hex.clone();
            let hex_latch = hex_latch.clone();
            let hex_active = hex_active.clone();
            let linked_knob_disabled = linked_knob_disabled.clone();
            let linked_reverse_disabled = linked_reverse_disabled.clone();
            let midi_mode = midi_mode.clone();
            let mic_muted = mic_muted.clone();
            let minimized = minimized.clone();
            let last_midi = last_midi.clone();
            let led_test = led_test.clone();
            let startup_animation = startup_animation.clone();
            let startup_animation_kind = startup_animation_kind.clone();
            let animation_preview = animation_preview.clone();
            let pipeweaver_request = pipeweaver_request.clone();

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
                        midi_mode,
                        swap,
                        ui_reset,
                        tray_ready,
                        pickup,
                        ui_knob_mode,
                        ui_commands,
                        media_tx,
                        record_hex,
                        hex_latch,
                        hex_active,
                        linked_knob_disabled,
                        linked_reverse_disabled,
                        mic_muted,
                        minimized,
                        last_midi,
                        led_test,
                        startup_animation,
                        startup_animation_kind,
                        animation_preview,
                        pipeweaver_request,
                    )))
                }),
            )?;
        } else {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    if record_hex.load(Ordering::Relaxed) {
        media::stop_hex().await;
    }
    let _ = tray_handle.await;
    let _ = midi_handle.join();
    log::info!("Stopped {}", theme::APP_NAME);
    Ok(())
}

fn start_visible(start_minimized: bool, tray_ready: bool) -> bool {
    !start_minimized || !tray_ready
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

#[cfg(test)]
mod tests {
    use super::start_visible;

    #[test]
    fn never_starts_hidden_without_a_tray() {
        assert!(!start_visible(true, true));
        assert!(start_visible(true, false));
        assert!(start_visible(false, true));
        assert!(start_visible(false, false));
    }
}
