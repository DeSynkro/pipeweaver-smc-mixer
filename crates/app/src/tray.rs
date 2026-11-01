use crate::theme;
use eframe::egui;
use image::GenericImageView;
use ksni::menu::StandardItem;
use ksni::{Category, Icon, MenuItem, Status, ToolTip, Tray, TrayMethods};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

pub struct TrayContext {
    pub ui_on: Arc<AtomicBool>,
    pub focus: Arc<AtomicBool>,
    pub minimized: Arc<AtomicBool>,
    pub egui: Arc<Mutex<Option<egui::Context>>>,
}

enum TrayMessage {
    Toggle,
    Quit,
}

pub async fn spawn(stop: Arc<AtomicBool>, context: Arc<TrayContext>) -> anyhow::Result<()> {
    let (tx, mut rx) = mpsc::channel(20);
    let icon = TrayIcon::new(tx);
    let handle = icon
        .disable_dbus_name(true)
        .assume_sni_available(true)
        .spawn()
        .await?;
    let mut interval = tokio::time::interval(Duration::from_millis(100));

    loop {
        tokio::select! {
            Some(message) = rx.recv() => {
                match message {
                    TrayMessage::Toggle => {
                        if context.minimized.swap(false, Ordering::Relaxed) {
                            if let Some(ctx) = context.egui.lock().unwrap().as_ref() {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                            }
                            context.ui_on.store(true, Ordering::Relaxed);
                            context.focus.store(true, Ordering::Relaxed);
                        } else {
                            let was_on = context.ui_on.fetch_xor(true, Ordering::Relaxed);
                            if !was_on {
                                context.focus.store(true, Ordering::Relaxed);
                            }
                        }
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
            }
        }
    }

    if !handle.is_closed() {
        handle.shutdown();
    }
    Ok(())
}

struct TrayIcon {
    tx: mpsc::Sender<TrayMessage>,
}

impl TrayIcon {
    fn new(tx: mpsc::Sender<TrayMessage>) -> Self {
        Self { tx }
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
            let image = image::load_from_memory_with_format(
                theme::ICON_PNG,
                image::ImageFormat::Png,
            )
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
        vec![
            StandardItem {
                label: String::from("Toggle UI"),
                activate: Box::new(|this: &mut TrayIcon| {
                    let _ = this.tx.try_send(TrayMessage::Toggle);
                }),
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
        let _ = self.tx.try_send(TrayMessage::Toggle);
    }
}
