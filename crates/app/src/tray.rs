use crate::theme;
use eframe::egui;
use image::GenericImageView;
use ksni::menu::{CheckmarkItem, StandardItem, SubMenu};
use ksni::{Category, Icon, MenuItem, Status, ToolTip, Tray, TrayMethods};
use pipeweaver_smc_core::config::{Config, MidiMode};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

pub struct TrayContext {
    pub ui_on: Arc<AtomicBool>,
    pub focus: Arc<AtomicBool>,
    pub minimized: Arc<AtomicBool>,
    pub egui: Arc<Mutex<Option<egui::Context>>>,
    pub midi_mode: Arc<AtomicU8>,
    pub pickup: Arc<AtomicBool>,
    pub record_hex: Arc<AtomicBool>,
    pub config_path: PathBuf,
}

enum TrayMessage {
    Show,
    SettingsChanged,
    Quit,
}

pub async fn spawn(
    stop: Arc<AtomicBool>,
    context: Arc<TrayContext>,
    ready: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    let (tx, mut rx) = mpsc::channel(20);
    let icon = TrayIcon::new(tx, context.clone());
    let handle = icon.disable_dbus_name(true).spawn().await?;
    ready.store(true, Ordering::Relaxed);
    let mut interval = tokio::time::interval(Duration::from_millis(100));
    let mut last_midi_mode = context.midi_mode.load(Ordering::Relaxed);
    let mut last_pickup = context.pickup.load(Ordering::Relaxed);
    let mut last_record_hex = context.record_hex.load(Ordering::Relaxed);

    loop {
        tokio::select! {
            Some(message) = rx.recv() => {
                match message {
                    TrayMessage::Show => {
                        context.minimized.store(false, Ordering::Relaxed);
                        if let Some(ctx) = context.egui.lock().unwrap().as_ref() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                        }
                        context.ui_on.store(true, Ordering::Relaxed);
                        context.focus.store(true, Ordering::Relaxed);
                    }
                    TrayMessage::SettingsChanged => {
                        let mut config = Config::load(&context.config_path);
                        config.midi_mode =
                            MidiMode::from_code(context.midi_mode.load(Ordering::Relaxed));
                        config.pickup = context.pickup.load(Ordering::Relaxed);
                        config.record_hex = context.record_hex.load(Ordering::Relaxed);
                        if let Err(error) = config.save(&context.config_path) {
                            log::warn!("Could not save config from tray: {error:#}");
                        }
                        last_midi_mode = context.midi_mode.load(Ordering::Relaxed);
                        last_pickup = context.pickup.load(Ordering::Relaxed);
                        last_record_hex = context.record_hex.load(Ordering::Relaxed);
                        let _ = handle.update(|_| {}).await;
                    }
                    TrayMessage::Quit => {
                        stop.store(true, Ordering::Relaxed);
                        if let Some(ctx) = context.egui.lock().unwrap().as_ref() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        break;
                    }
                }
            }
            _ = interval.tick() => {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let midi_mode = context.midi_mode.load(Ordering::Relaxed);
                let pickup = context.pickup.load(Ordering::Relaxed);
                let record_hex = context.record_hex.load(Ordering::Relaxed);
                if midi_mode != last_midi_mode
                    || pickup != last_pickup
                    || record_hex != last_record_hex
                {
                    last_midi_mode = midi_mode;
                    last_pickup = pickup;
                    last_record_hex = record_hex;
                    let _ = handle.update(|_| {}).await;
                }
            }
        }
    }

    ready.store(false, Ordering::Relaxed);
    if !handle.is_closed() {
        handle.shutdown();
    }
    Ok(())
}

struct TrayIcon {
    tx: mpsc::Sender<TrayMessage>,
    context: Arc<TrayContext>,
}

impl TrayIcon {
    fn new(tx: mpsc::Sender<TrayMessage>, context: Arc<TrayContext>) -> Self {
        Self { tx, context }
    }

    fn show(&self) {
        let _ = self.tx.try_send(TrayMessage::Show);
    }

    fn settings_changed(&self) {
        let _ = self.tx.try_send(TrayMessage::SettingsChanged);
    }

    fn set_midi_mode(&mut self, mode: MidiMode) {
        self.context.midi_mode.store(mode.code(), Ordering::Relaxed);
        self.settings_changed();
    }

    fn toggle_pickup(&mut self) {
        let next = !self.context.pickup.load(Ordering::Relaxed);
        self.context.pickup.store(next, Ordering::Relaxed);
        self.settings_changed();
    }

    fn toggle_record_hex(&mut self) {
        let next = !self.context.record_hex.load(Ordering::Relaxed);
        self.context.record_hex.store(next, Ordering::Relaxed);
        self.settings_changed();
    }
}

impl Tray for TrayIcon {
    fn id(&self) -> String {
        theme::APP_ID.to_string()
    }

    fn category(&self) -> Category {
        Category::SystemServices
    }

    fn title(&self) -> String {
        theme::APP_NAME.to_string()
    }

    fn status(&self) -> Status {
        Status::Active
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        static ICON: LazyLock<Icon> = LazyLock::new(|| {
            let image =
                image::load_from_memory_with_format(theme::ICON_PNG, image::ImageFormat::Png)
                    .expect("load tray icon")
                    .resize_exact(128, 128, image::imageops::FilterType::Lanczos3);
            let (width, height) = image.dimensions();
            let mut data = image.into_rgba8().into_vec();
            for pixel in data.chunks_exact_mut(4) {
                pixel.rotate_right(1);
            }
            Icon {
                width: width as i32,
                height: height as i32,
                data,
            }
        });
        vec![ICON.clone()]
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: theme::APP_NAME.to_string(),
            description: String::from("SMC-Mixer control surface for Pipeweaver"),
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let midi_mode = MidiMode::from_code(self.context.midi_mode.load(Ordering::Relaxed));
        let pickup = self.context.pickup.load(Ordering::Relaxed);
        let record_hex = self.context.record_hex.load(Ordering::Relaxed);

        vec![
            StandardItem {
                label: String::from("Show"),
                activate: Box::new(|this: &mut TrayIcon| this.show()),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            SubMenu {
                label: String::from("MIDI Connection"),
                submenu: vec![
                    CheckmarkItem {
                        label: String::from("Auto"),
                        checked: midi_mode == MidiMode::Auto,
                        activate: Box::new(|this: &mut TrayIcon| {
                            this.set_midi_mode(MidiMode::Auto)
                        }),
                        ..Default::default()
                    }
                    .into(),
                    CheckmarkItem {
                        label: String::from("Wired"),
                        checked: midi_mode == MidiMode::Wired,
                        activate: Box::new(|this: &mut TrayIcon| {
                            this.set_midi_mode(MidiMode::Wired)
                        }),
                        ..Default::default()
                    }
                    .into(),
                    CheckmarkItem {
                        label: String::from("Bluetooth"),
                        checked: midi_mode == MidiMode::Bluetooth,
                        activate: Box::new(|this: &mut TrayIcon| {
                            this.set_midi_mode(MidiMode::Bluetooth)
                        }),
                        ..Default::default()
                    }
                    .into(),
                ],
                ..Default::default()
            }
            .into(),
            CheckmarkItem {
                label: String::from("Soft Takeover"),
                checked: pickup,
                activate: Box::new(|this: &mut TrayIcon| this.toggle_pickup()),
                ..Default::default()
            }
            .into(),
            CheckmarkItem {
                label: String::from("Record Button Switches to Hex"),
                checked: record_hex,
                activate: Box::new(|this: &mut TrayIcon| this.toggle_record_hex()),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: String::from("Quit"),
                activate: Box::new(|this: &mut TrayIcon| {
                    let _ = this.tx.try_send(TrayMessage::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.show();
    }
}
