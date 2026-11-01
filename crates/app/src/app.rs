use crate::theme::{self, Map};
use eframe::egui;
use egui::{
    Align, Align2, Color32, Context, FontId, Id, Layout, Rect, RichText, Sense, Stroke,
    StrokeKind, UiBuilder, Vec2, ViewportCommand, pos2, vec2,
};
use pipeweaver_smc_core::config::Config;
use pipeweaver_smc_core::{Engine, Snapshot, STRIP_COUNT};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    Mixer,
    Config,
}

fn monitor_rect(frame: &eframe::Frame) -> Option<Rect> {
    let window = frame.winit_window()?;
    let monitor = window.current_monitor()?;
    let scale = monitor.scale_factor();
    let position = monitor.position().to_logical::<f32>(scale);
    let size = monitor.size().to_logical::<f32>(scale);
    Some(Rect::from_min_size(
        pos2(position.x, position.y),
        vec2(size.width, size.height),
    ))
}

fn clamp_to_monitor(pos: egui::Pos2, size: egui::Vec2, monitor: Rect) -> egui::Pos2 {
    pos2(
        pos.x
            .clamp(monitor.min.x, (monitor.max.x - size.x).max(monitor.min.x)),
        pos.y
            .clamp(monitor.min.y, (monitor.max.y - size.y).max(monitor.min.y)),
    )
}

pub struct SmcApp {
    stop: Arc<AtomicBool>,
    ui_on: Arc<AtomicBool>,
    focus: Arc<AtomicBool>,
    config: Config,
    config_path: PathBuf,
    screen: Screen,
    pipeweaver_online: bool,
    engine: Arc<Engine>,
    mixer_online: bool,
    mixer_net: Arc<AtomicBool>,
    window_width: f32,
    restore_width: f32,
    restore_pos: Option<egui::Pos2>,
    maximized: bool,
    resizing: bool,
    resize_start: f32,
    resize_pointer: Option<f32>,
    enable_resize_at: Option<Instant>,
    pending_move: Option<(egui::Pos2, f32, Instant)>,
}

impl SmcApp {
    pub fn new(
        config: Config,
        config_path: PathBuf,
        stop: Arc<AtomicBool>,
        ui_on: Arc<AtomicBool>,
        focus: Arc<AtomicBool>,
        engine: Arc<Engine>,
        mixer_net: Arc<AtomicBool>,
    ) -> Self {
        let window_width = config.window_width();
        Self {
            stop,
            ui_on,
            focus,
            config,
            config_path,
            screen: Screen::Mixer,
            pipeweaver_online: false,
            engine,
            mixer_online: false,
            mixer_net,
            window_width,
            restore_width: window_width,
            restore_pos: None,
            maximized: false,
            resizing: false,
            resize_start: window_width,
            resize_pointer: None,
            enable_resize_at: Some(Instant::now() + Duration::from_millis(700)),
            pending_move: None,
        }
    }

    fn save_config(&mut self) {
        self.config.window_width = if self.maximized {
            self.restore_width
        } else {
            self.window_width
        };
        if let Err(error) = self.config.save(&self.config_path) {
            log::warn!("Could not save config: {error:#}");
        }
    }

    /// Move after a resize has been acknowledged. Sending both in one frame
    /// makes X11 keep the old size.
    fn schedule_move(&mut self, pos: egui::Pos2, width: f32) {
        self.pending_move = Some((pos, width, Instant::now() + Duration::from_millis(400)));
    }

    fn hide(&mut self, ctx: &Context) {
        self.save_config();
        self.ui_on.store(false, Ordering::Relaxed);
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }

    fn set_window_width(&mut self, width: f32, ctx: &Context, monitor: Option<Rect>) {
        let width = width.clamp(theme::MIN_WINDOW_WIDTH, theme::MAX_WINDOW_WIDTH);
        let size = vec2(width, width / theme::ASPECT);
        let was_maximized = self.maximized;
        self.window_width = width;
        self.maximized = false;
        if was_maximized {
            ctx.send_viewport_cmd(ViewportCommand::Maximized(false));
        }
        if let (Some(monitor), Some(outer)) =
            (monitor, ctx.input(|input| input.viewport().outer_rect))
        {
            let pos = clamp_to_monitor(outer.min, size, monitor);
            if (pos - outer.min).length_sq() > 1.0 {
                self.schedule_move(pos, width);
            }
        }
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
        self.save_config();
    }

    fn toggle_maximize(&mut self, ctx: &Context, monitor: Option<Rect>) {
        let can_position = ctx.input(|input| input.viewport().outer_rect).is_some();

        if self.maximized {
            self.maximized = false;
            let width = self.restore_width;
            let size = vec2(width, width / theme::ASPECT);
            self.window_width = width;
            ctx.send_viewport_cmd(ViewportCommand::Maximized(false));
            if can_position {
                ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
                if let (Some(monitor), Some(pos)) = (monitor, self.restore_pos.take()) {
                    let pos = clamp_to_monitor(pos, size, monitor);
                    self.schedule_move(pos, width);
                }
            }
            return;
        }

        self.restore_width = self.window_width;
        self.restore_pos = ctx.input(|input| input.viewport().outer_rect).map(|rect| rect.min);
        self.maximized = true;

        if !can_position || monitor.is_none() {
            // A Wayland client cannot place its own window, so let the
            // compositor maximize it and letterbox the shell inside.
            ctx.send_viewport_cmd(ViewportCommand::Maximized(true));
            return;
        }

        let monitor = monitor.unwrap();
        let max_width = (monitor.width() - 80.0).max(theme::MIN_WINDOW_WIDTH);
        let max_height = (monitor.height() - 120.0).max(400.0);
        let width = max_width
            .min(max_height * theme::ASPECT)
            .clamp(theme::MIN_WINDOW_WIDTH, theme::MAX_WINDOW_WIDTH);
        let size = vec2(width, width / theme::ASPECT);
        self.window_width = width;
        let pos = clamp_to_monitor(monitor.center() - size * 0.5, size, monitor);
        self.schedule_move(pos, width);
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
    }

    fn paint_shell(&self, ui: &egui::Ui, map: &Map) {
        let (uri, bytes) = if self.config.light_panel {
            (theme::SHELL_LIGHT_URI, theme::SHELL_LIGHT_SVG)
        } else {
            (theme::SHELL_DARK_URI, theme::SHELL_DARK_SVG)
        };
        egui::Image::from_bytes(uri, bytes)
            .fit_to_exact_size(map.size(vec2(theme::SHELL_W, theme::SHELL_H)))
            .paint_at(ui, map.rect);
    }

    fn paint_channels(&self, ui: &egui::Ui, map: &Map, snapshot: &Snapshot) {
        let palette = theme::palette(self.config.light_panel);
        let painter = ui.painter();
        let text_size = (8.5 * map.scale).max(6.0);

        for (index, channel) in snapshot.channels.iter().enumerate().take(STRIP_COUNT) {
            if !channel.assigned {
                continue;
            }

            let base = 100.5 + 215.0 * index as f32;
            let colour = if channel.muted {
                palette.dim
            } else {
                Color32::from_rgb(channel.colour.0, channel.colour.1, channel.colour.2)
            };

            let plate = map.rect(Rect::from_min_size(
                pos2(base + 10.0, 271.5),
                vec2(82.0, 20.0),
            ));
            painter.rect_filled(
                Rect::from_min_size(plate.min, vec2(plate.width(), (2.5 * map.scale).max(1.0))),
                0,
                colour,
            );
            painter.with_clip_rect(plate).text(
                plate.center(),
                Align2::CENTER_CENTER,
                &channel.name,
                FontId::proportional(text_size),
                palette.text,
            );

            let slot_top = 350.0;
            let slot_height = 451.0;
            let y = slot_top + (1.0 - channel.fader as f32 / 100.0) * slot_height;
            let cap = Rect::from_center_size(
                map.point(base + 51.0, y),
                map.size(vec2(54.0, 13.0)),
            );
            painter.rect_filled(cap, map.radius(3.0), colour);
            painter.rect_stroke(
                cap,
                map.radius(3.0),
                Stroke::new((1.0 * map.scale).max(1.0), palette.border),
                StrokeKind::Inside,
            );

            let angle = (-135.0 + 270.0 * channel.knob as f32 / 100.0).to_radians();
            let centre = map.point(base + 51.5, 170.0);
            let direction = vec2(angle.sin(), -angle.cos());
            painter.line_segment(
                [
                    centre + direction * (36.0 * map.scale),
                    centre + direction * (54.0 * map.scale),
                ],
                Stroke::new((3.0 * map.scale).max(1.5), colour),
            );
        }
    }

    fn paint_bezel(
        &mut self,
        ui: &mut egui::Ui,
        map: &Map,
        ctx: &Context,
        monitor: Option<Rect>,
    ) {
        let drag = map.rect(theme::bezel_drag());
        let response = ui.interact(
            drag,
            Id::unique(("bezel-drag",)),
            Sense::click_and_drag(),
        );
        if response.drag_started() {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }
        if response.double_clicked() {
            self.toggle_maximize(ctx, monitor);
        }

        let palette = theme::palette(self.config.light_panel);
        let font = FontId::proportional((15.0 * map.scale).max(9.0));
        let dot = (7.0 * map.scale).max(4.0);
        let text_gap = 8.0 * map.scale;
        let group_gap = 80.0 * map.scale;

        let pw_width = ui
            .painter()
            .layout_no_wrap("PIPEWEAVER".to_owned(), font.clone(), palette.accent)
            .size()
            .x;
        let smc_width = ui
            .painter()
            .layout_no_wrap("SMC-MIXER".to_owned(), font.clone(), palette.text)
            .size()
            .x;
        let total = pw_width + text_gap + dot + group_gap + smc_width + text_gap + dot;
        let start_x = map.rect.center().x - total * 0.5;
        let line_y = map.point(0.0, theme::BEZEL_CENTER).y;

        let pipeweaver_dot = pos2(start_x + pw_width + text_gap + dot * 0.5, line_y);
        let smc_x = start_x + pw_width + text_gap + dot + group_gap;
        let mixer_dot = pos2(smc_x + smc_width + text_gap + dot * 0.5, line_y);

        let hit = Vec2::splat(dot.max(14.0));
        ui.interact(
            Rect::from_center_size(pipeweaver_dot, hit),
            Id::unique(("pipeweaver-state",)),
            Sense::hover(),
        )
        .on_hover_text(if self.pipeweaver_online {
            "Pipeweaver: connected"
        } else {
            "Pipeweaver: not connected"
        });
        ui.interact(
            Rect::from_center_size(mixer_dot, hit),
            Id::unique(("mixer-state",)),
            Sense::hover(),
        )
        .on_hover_text(if self.mixer_online {
            "SMC mixer: connected"
        } else {
            "SMC mixer: not detected"
        });

        {
            let painter = ui.painter();
            painter.text(
                pos2(start_x, line_y),
                Align2::LEFT_CENTER,
                "PIPEWEAVER",
                font.clone(),
                palette.accent,
            );
            painter.circle_filled(
                pipeweaver_dot,
                dot * 0.5,
                if self.pipeweaver_online {
                    palette.green
                } else {
                    palette.red
                },
            );
            painter.text(
                pos2(smc_x, line_y),
                Align2::LEFT_CENTER,
                "SMC-MIXER",
                font,
                palette.text,
            );
            painter.circle_filled(
                mixer_dot,
                dot * 0.5,
                if self.mixer_online {
                    palette.green
                } else {
                    palette.red
                },
            );
        }

        for (index, tooltip) in ["Minimize", "Maximize", "Close"].iter().enumerate() {
            let rect = map.rect(theme::window_control_rect(index));
            let response = ui.interact(
                rect,
                Id::unique(("window-control", index)),
                Sense::click(),
            );
            let hovered = response.hovered();
            let clicked = response.clicked();
            response.on_hover_text(*tooltip);

            let painter = ui.painter();
            if hovered {
                let fill = if index == 2 {
                    palette.red.gamma_multiply(0.25)
                } else {
                    palette.card_hover
                };
                painter.circle_filled(rect.center(), rect.width() * 0.5, fill);
            }
            let colour = if index == 2 && hovered {
                palette.red
            } else {
                palette.text
            };
            let centre = rect.center();
            let reach = rect.width() * 0.20;
            let stroke = Stroke::new((1.5 * map.scale).max(1.0), colour);
            match index {
                0 => {
                    painter.line_segment(
                        [
                            pos2(centre.x - reach, centre.y),
                            pos2(centre.x + reach, centre.y),
                        ],
                        stroke,
                    );
                }
                1 => {
                    painter.rect_stroke(
                        Rect::from_center_size(centre, vec2(reach * 2.0, reach * 2.0)),
                        1.5,
                        stroke,
                        StrokeKind::Inside,
                    );
                }
                _ => {
                    painter.line_segment(
                        [
                            pos2(centre.x - reach, centre.y - reach),
                            pos2(centre.x + reach, centre.y + reach),
                        ],
                        stroke,
                    );
                    painter.line_segment(
                        [
                            pos2(centre.x + reach, centre.y - reach),
                            pos2(centre.x - reach, centre.y + reach),
                        ],
                        stroke,
                    );
                }
            }

            if clicked {
                match index {
                    0 => ctx.send_viewport_cmd(ViewportCommand::Minimized(true)),
                    1 => self.toggle_maximize(ctx, monitor),
                    _ => self.hide(ctx),
                }
            }
        }
    }

    fn paint_nav(&mut self, ui: &mut egui::Ui, map: &Map) {
        let entries = [
            (Screen::Mixer, theme::nav_mixer(), "MIXER"),
            (Screen::Config, theme::nav_config(), "CONFIG"),
        ];
        let palette = theme::palette(self.config.light_panel);
        for (screen, rect, label) in entries {
            let rect = map.rect(rect);
            let response = ui.interact(rect, Id::unique(("nav", label)), Sense::click());
            let active = self.screen == screen;
            let on_config = self.screen == Screen::Config;
            let painter = ui.painter();
            if on_config {
                painter.rect_filled(rect, map.radius(10.0), palette.card);
                painter.rect_stroke(
                    rect,
                    map.radius(10.0),
                    Stroke::new((1.2 * map.scale).max(1.0), palette.border),
                    StrokeKind::Inside,
                );
            }
            if response.hovered() && !active {
                painter.rect_filled(
                    rect.shrink(3.0 * map.scale),
                    map.radius(10.0),
                    palette.hover,
                );
            }
            if active {
                painter.rect_stroke(
                    rect.shrink(3.0 * map.scale),
                    map.radius(10.0),
                    Stroke::new((1.5 * map.scale).max(1.0), palette.accent),
                    StrokeKind::Inside,
                );
            }
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                label,
                FontId::proportional((15.0 * map.scale).max(8.0)),
                if active {
                    palette.accent
                } else {
                    palette.text
                },
            );
            if response.clicked() {
                self.screen = screen;
            }
        }
    }

    fn paint_config(
        &mut self,
        ui: &mut egui::Ui,
        map: &Map,
        ctx: &Context,
        monitor: Option<Rect>,
    ) {
        let palette = theme::palette(self.config.light_panel);
        let panel = map.rect(theme::config_panel());
        let painter = ui.painter();
        painter.rect_filled(panel, map.radius(18.0), palette.panel);
        painter.rect_stroke(
            panel,
            map.radius(18.0),
            Stroke::new(1.5 * map.scale, palette.border),
            StrokeKind::Inside,
        );

        let inner = panel.shrink(28.0 * map.scale);
        let width = self.window_width;
        let mut light = self.config.light_panel;
        let mut reset = false;
        let heading = (26.0 * map.scale).max(16.0);
        let label = (12.0 * map.scale).max(9.0);
        let section = (13.0 * map.scale).max(10.0);

        ui.scope_builder(
            UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::LEFT)),
            |ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                ui.label(
                    RichText::new("Config")
                        .size(heading)
                        .strong()
                        .color(palette.text),
                );
                ui.label(
                    RichText::new(
                        "The shell is in place. Connection, surface and diagnostics arrive next.",
                    )
                    .size(label)
                    .color(palette.dim),
                );
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);

                ui.columns(2, |columns| {
                    columns[0].label(
                        RichText::new("Appearance")
                            .size(section)
                            .strong()
                            .color(palette.text),
                    );
                    columns[0].label(
                        RichText::new("Panel finish")
                            .size(label)
                            .color(palette.dim),
                    );
                    columns[0].checkbox(&mut light, "Light theme");

                    columns[0].add_space(12.0);
                    columns[0].label(
                        RichText::new("Window")
                            .size(section)
                            .strong()
                            .color(palette.text),
                    );
                    columns[0].label(
                        RichText::new(format!("Width: {width:.0} px"))
                            .size(label)
                            .color(palette.text),
                    );
                    columns[0].label(
                        RichText::new("Aspect ratio locked at 1987:967")
                            .size(label)
                            .color(palette.dim),
                    );
                    if columns[0].button("Reset size").clicked() {
                        reset = true;
                    }

                    columns[1].label(
                        RichText::new("Status")
                            .size(section)
                            .strong()
                            .color(palette.text),
                    );
                    columns[1].label(
                        RichText::new(if self.pipeweaver_online {
                            "Pipeweaver: connected"
                        } else {
                            "Pipeweaver: not connected"
                        })
                        .size(label)
                        .color(palette.dim),
                    );
                    columns[1].label(
                        RichText::new(if self.mixer_online {
                            "MIDI: SMC-Mixer detected"
                        } else {
                            "MIDI: waiting"
                        })
                        .size(label)
                        .color(palette.dim),
                    );

                    columns[1].add_space(12.0);
                    columns[1].label(
                        RichText::new("About")
                            .size(section)
                            .strong()
                            .color(palette.text),
                    );
                    columns[1].label(
                        RichText::new(format!(
                            "Pipeweaver SMC Mixer {}",
                            theme::VERSION
                        ))
                        .size(label)
                        .color(palette.text),
                    );
                    columns[1].label(
                        RichText::new("Shell milestone")
                            .size(label)
                            .color(palette.dim),
                    );
                });
            },
        );

        if light != self.config.light_panel {
            self.config.light_panel = light;
            ctx.set_visuals(theme::visuals(light));
            self.save_config();
        }
        if reset {
            self.set_window_width(1280.0, ctx, monitor);
        }
    }

    fn paint_grip(
        &mut self,
        ui: &mut egui::Ui,
        map: &Map,
        ctx: &Context,
        monitor: Option<Rect>,
    ) {
        let rect = map.rect(theme::resize_grip());
        let response = ui.interact(rect, Id::unique(("resize-grip",)), Sense::drag());
        if response.hovered() {
            ctx.set_cursor_icon(egui::CursorIcon::ResizeNwSe);
        }
        if response.drag_started() {
            self.resizing = true;
            self.resize_start = self.window_width;
            self.resize_pointer = response.interact_pointer_pos().map(|pos| pos.x);
        }
        if response.dragged() {
            let pointer = ctx
                .input(|input| input.pointer.interact_pos())
                .map(|pos| pos.x);
            if let (Some(start), Some(now)) = (self.resize_pointer, pointer) {
                let width = (self.resize_start + now - start)
                    .clamp(theme::MIN_WINDOW_WIDTH, theme::MAX_WINDOW_WIDTH);
                let height = width / theme::ASPECT;
                self.window_width = width;
                self.maximized = false;
                let size = vec2(width, height);
                ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
            }
        }
        if response.drag_stopped() {
            self.resizing = false;
            self.resize_pointer = None;
            if let (Some(monitor), Some(outer)) =
                (monitor, ctx.input(|input| input.viewport().outer_rect))
            {
                let size = vec2(self.window_width, self.window_width / theme::ASPECT);
                let pos = clamp_to_monitor(outer.min, size, monitor);
                if (pos - outer.min).length_sq() > 1.0 {
                    self.schedule_move(pos, self.window_width);
                }
            }
            self.save_config();
        }

        let painter = ui.painter();
        let colour = theme::palette(self.config.light_panel).dim;
        for step in 0..3 {
            let offset = 6.0 + step as f32 * 5.0;
            painter.line_segment(
                [
                    map.point(1967.0 - offset, 950.0),
                    map.point(1967.0, 950.0 - offset),
                ],
                Stroke::new((1.2 * map.scale).max(1.0), colour),
            );
        }
    }
}

impl eframe::App for SmcApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let monitor = monitor_rect(frame);
        let snapshot = self.engine.snapshot();
        self.pipeweaver_online = snapshot.connected;
        self.mixer_online = self.mixer_net.load(Ordering::Relaxed);

        // The window is created locked so tiling managers keep it floating.
        // Once it has been mapped, allow programmatic resizing again.
        if let Some(at) = self.enable_resize_at {
            if Instant::now() >= at {
                ctx.send_viewport_cmd(ViewportCommand::MinInnerSize(vec2(
                    theme::MIN_WINDOW_WIDTH,
                    theme::MIN_WINDOW_WIDTH / theme::ASPECT,
                )));
                ctx.send_viewport_cmd(ViewportCommand::MaxInnerSize(vec2(
                    f32::INFINITY,
                    f32::INFINITY,
                )));
                ctx.send_viewport_cmd(ViewportCommand::Resizable(true));
                ctx.send_viewport_cmd(ViewportCommand::InnerSize(vec2(
                    self.window_width,
                    self.window_width / theme::ASPECT,
                )));
                self.enable_resize_at = None;
            }
        }

        if self.focus.swap(false, Ordering::Relaxed) {
            ctx.send_viewport_cmd(ViewportCommand::Focus);
        }
        if self.stop.load(Ordering::Relaxed) || !self.ui_on.load(Ordering::Relaxed) {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        if ctx.input(|input| input.viewport().close_requested()) {
            self.save_config();
            self.ui_on.store(false, Ordering::Relaxed);
        }
        if let Some(rect) = ctx.input(|input| input.viewport().inner_rect) {
            if !self.resizing {
                self.window_width = rect.width();
            }
            if let Some((pos, width, deadline)) = self.pending_move {
                if (rect.width() - width).abs() <= 1.0 || Instant::now() >= deadline {
                    ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos));
                    self.pending_move = None;
                }
            }
        }
        ctx.request_repaint_after(Duration::from_millis(50));

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::TRANSPARENT))
            .show(ui, |ui| {
                let map = Map::new(ui.max_rect());
                self.paint_shell(ui, &map);
                self.paint_channels(ui, &map, &snapshot);
                self.paint_bezel(ui, &map, &ctx, monitor);
                if self.screen == Screen::Config {
                    self.paint_config(ui, &map, &ctx, monitor);
                }
                self.paint_nav(ui, &map);
                self.paint_grip(ui, &map, &ctx, monitor);
            });
    }
}
