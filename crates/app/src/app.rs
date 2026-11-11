use crate::knob_mode::{
    KnobMode, MODE_ABSOLUTE, MODE_RELATIVE, MODE_UNKNOWN, REQUEST_DETECT, REQUEST_DISABLE,
    REQUEST_ENABLE, REQUEST_RESTORE,
};
use crate::media::MediaCommand;
use crate::midi::Link;
use crate::theme::{self, Map};
use eframe::egui;
use egui::{
    Align, Align2, Color32, Context, FontId, Id, Layout, Rect, RichText, Sense, Shape, Stroke,
    StrokeKind, UiBuilder, Vec2, ViewportCommand, pos2, vec2,
};
use pipeweaver_ipc::commands::{APICommand, DaemonRequest};
use pipeweaver_shared::{Mix, MuteState, MuteTarget};
use pipeweaver_smc_core::config::{Config, MidiMode, autostart_path, set_autostart, state_dir};
use pipeweaver_smc_core::{
    Channel, ChannelKind, Engine, STRIP_COUNT, SURFACE_SOURCES_DEFAULT, SURFACE_SOURCES_MAX,
    SURFACE_SOURCES_MIN, Snapshot,
};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc::Sender;
use ulid::Ulid;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    Mixer,
    Config,
    Pipeweaver,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum KnobAction {
    Enable,
    Disable,
    Restore,
}

#[derive(Clone, Copy)]
enum Span {
    Plain,
    Strong,
    Underline,
}

/// A wrapped paragraph made of differently styled spans.
fn rich_spans(ui: &mut egui::Ui, size: f32, palette: &theme::Palette, spans: &[(&str, Span)]) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (text, style) in spans {
            let mut rich = RichText::new(*text).size(size).color(palette.dim);
            rich = match style {
                Span::Plain => rich,
                Span::Strong => rich.strong().color(palette.text),
                Span::Underline => rich.underline().color(palette.text),
            };
            ui.label(rich);
        }
    });
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
    midi: Link,
    midi_net: Arc<AtomicU8>,
    midi_mode: Arc<AtomicU8>,
    swap: Arc<AtomicU8>,
    ui_reset: Arc<AtomicU8>,
    tray_ready: Arc<AtomicBool>,
    pickup_net: Arc<AtomicBool>,
    knob_mode: Arc<KnobMode>,
    knob_action: Option<KnobAction>,
    knob_info_open: bool,
    commands: Sender<DaemonRequest>,
    drawer_open: bool,
    drawer_t: f32,
    offline_since: Option<Instant>,
    offline_t: f32,
    reversed: HashSet<Ulid>,
    media_tx: tokio::sync::mpsc::UnboundedSender<MediaCommand>,
    record_hex: Arc<AtomicBool>,
    hex_latch_net: Arc<AtomicBool>,
    hex_active: Arc<AtomicBool>,
    linked_knob_disabled_net: Arc<AtomicBool>,
    linked_reverse_disabled_net: Arc<AtomicBool>,
    mic_muted: Arc<AtomicBool>,
    minimized: Arc<AtomicBool>,
    minimize_at: Option<Instant>,
    last_midi: Arc<AtomicU64>,
    led_test: Arc<AtomicBool>,
    startup_animation: Arc<AtomicBool>,
    animation_preview: Arc<AtomicBool>,
    record_down: bool,
    window_width: f32,
    restore_width: f32,
    restore_pos: Option<egui::Pos2>,
    maximized: bool,
    resizing: bool,
    resize_start: f32,
    resize_pointer: Option<f32>,
    enable_resize_at: Option<Instant>,
    pending_move: Option<(egui::Pos2, f32, Instant)>,
    focus_at: Option<Instant>,
}

impl SmcApp {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: Config,
        config_path: PathBuf,
        stop: Arc<AtomicBool>,
        ui_on: Arc<AtomicBool>,
        focus: Arc<AtomicBool>,
        engine: Arc<Engine>,
        midi_net: Arc<AtomicU8>,
        midi_mode: Arc<AtomicU8>,
        swap: Arc<AtomicU8>,
        ui_reset: Arc<AtomicU8>,
        tray_ready: Arc<AtomicBool>,
        pickup_net: Arc<AtomicBool>,
        knob_mode: Arc<KnobMode>,
        commands: Sender<DaemonRequest>,
        media_tx: tokio::sync::mpsc::UnboundedSender<MediaCommand>,
        record_hex: Arc<AtomicBool>,
        hex_latch_net: Arc<AtomicBool>,
        hex_active: Arc<AtomicBool>,
        linked_knob_disabled_net: Arc<AtomicBool>,
        linked_reverse_disabled_net: Arc<AtomicBool>,
        mic_muted: Arc<AtomicBool>,
        minimized: Arc<AtomicBool>,
        last_midi: Arc<AtomicU64>,
        led_test: Arc<AtomicBool>,
        startup_animation: Arc<AtomicBool>,
        animation_preview: Arc<AtomicBool>,
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
            midi: Link::Offline,
            midi_net,
            midi_mode,
            swap,
            ui_reset,
            tray_ready,
            pickup_net,
            knob_mode,
            knob_action: None,
            knob_info_open: false,
            commands,
            drawer_open: false,
            drawer_t: 0.0,
            offline_since: None,
            offline_t: 0.0,
            reversed: HashSet::new(),
            media_tx,
            record_hex,
            hex_latch_net,
            hex_active,
            linked_knob_disabled_net,
            linked_reverse_disabled_net,
            mic_muted,
            minimized,
            minimize_at: None,
            last_midi,
            led_test,
            startup_animation,
            animation_preview,
            record_down: false,
            window_width,
            restore_width: window_width,
            restore_pos: None,
            maximized: false,
            resizing: false,
            resize_start: window_width,
            resize_pointer: None,
            enable_resize_at: Some(Instant::now() + Duration::from_millis(700)),
            pending_move: None,
            focus_at: Some(Instant::now() + Duration::from_millis(500)),
        }
    }

    fn save_config(&mut self) {
        self.config.midi_mode = MidiMode::from_code(self.midi_mode.load(Ordering::Relaxed));
        self.config.pickup = self.pickup_net.load(Ordering::Relaxed);
        self.config.record_hex = self.record_hex.load(Ordering::Relaxed);
        self.config.hex_latch = self.hex_latch_net.load(Ordering::Relaxed);
        self.config.linked_knob_disabled = self.linked_knob_disabled_net.load(Ordering::Relaxed);
        self.config.window_width = if self.maximized {
            self.restore_width
        } else {
            self.window_width
        };
        if let Err(error) = self.config.save(&self.config_path) {
            log::warn!("Could not save config: {error:#}");
        }
    }

    fn set_split(&mut self, slots: usize) {
        let slots = slots.clamp(SURFACE_SOURCES_MIN, SURFACE_SOURCES_MAX);
        if slots == self.engine.source_slots() {
            return;
        }
        self.config.surface_sources = slots;
        self.engine.set_source_slots(slots);
        self.save_config();
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
        self.restore_pos = ctx
            .input(|input| input.viewport().outer_rect)
            .map(|rect| rect.min);
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
        let swapped = self.swap.load(Ordering::Relaxed);
        let scale = map.scale;
        let interactive =
            self.screen == Screen::Mixer && self.drawer_t <= 0.01 && snapshot.connected;
        let offline = self.offline_t;
        let channel_colours = self.config.channel_colours;
        let mix_colour = |is_b: bool| {
            if is_b { palette.amber } else { palette.accent }
        };

        for (index, channel) in snapshot.channels.iter().enumerate().take(STRIP_COUNT) {
            if !channel.assigned {
                continue;
            }

            let base = 100.5 + 215.0 * index as f32;
            let is_source = channel.kind == ChannelKind::Source;
            let is_swapped = is_source && swapped & (1 << index) != 0;
            let (mut fader, mut knob) = if is_swapped {
                (channel.knob, channel.fader)
            } else {
                (channel.fader, channel.knob)
            };
            let channel_colour =
                Color32::from_rgb(channel.colour.0, channel.colour.1, channel.colour.2);
            let (fader_mix_b, knob_mix_b) = if is_source {
                (is_swapped, !is_swapped)
            } else {
                (channel.mix_b, false)
            };
            let fader_mix = mix_colour(fader_mix_b);
            let knob_mix = mix_colour(knob_mix_b);
            let fader_base = if channel_colours {
                channel_colour
            } else {
                mix_colour(fader_mix_b)
            };
            let knob_base = if channel_colours {
                channel_colour
            } else {
                mix_colour(knob_mix_b)
            };
            let fader_muted = if is_source {
                if fader_mix_b {
                    channel.mute_b
                } else {
                    channel.mute_a
                }
            } else {
                channel.mute_a
            };
            let knob_muted = if is_source {
                if knob_mix_b {
                    channel.mute_b
                } else {
                    channel.mute_a
                }
            } else {
                false
            };
            let fader_colour = if fader_muted { palette.red } else { fader_base }
                .lerp_to_gamma(palette.dim, offline);
            let knob_colour = if knob_muted { palette.red } else { knob_base }
                .lerp_to_gamma(palette.dim, offline);
            let knob_disabled = !is_source;
            let labels = &self.config.mix_labels;
            let mut knob_hovered = false;

            if interactive {
                let knob_centre = map.point(base + 51.5, 170.0);
                let knob_rect = Rect::from_center_size(knob_centre, Vec2::splat(147.0 * scale));
                let knob_id = Id::unique(("surface-knob", index));
                if knob_disabled {
                    ui.interact(knob_rect, knob_id, Sense::hover())
                        .on_hover_text("Targets have no Audience control");
                } else {
                    let response = ui.interact(knob_rect, knob_id, Sense::click_and_drag());
                    knob_hovered = response.hovered();
                    if response.drag_started()
                        && let Some(pos) = response.interact_pointer_pos()
                    {
                        ui.ctx()
                            .data_mut(|data| data.insert_temp(knob_id, (knob, pos.y, knob)));
                    }
                    if response.dragged()
                        && let (Some((start_value, start_y, last)), Some(pos)) = (
                            ui.ctx()
                                .data(|data| data.get_temp::<(u8, f32, u8)>(knob_id)),
                            response.interact_pointer_pos(),
                        )
                    {
                        let delta = (start_y - pos.y) / (150.0 * scale) * 100.0;
                        let next = (start_value as f32 + delta).clamp(0.0, 100.0).round() as u8;
                        if next != last {
                            ui.ctx().data_mut(|data| {
                                data.insert_temp(knob_id, (start_value, start_y, next))
                            });
                            let _ = self.commands.try_send(DaemonRequest::Pipewire(
                                APICommand::SetSourceVolume(
                                    channel.id,
                                    if is_swapped { Mix::A } else { Mix::B },
                                    next,
                                ),
                            ));
                        }
                        knob = next;
                    }
                    let knob_label = if is_swapped { &labels[0] } else { &labels[1] };
                    response.on_hover_text(format!("Drag to set the {knob_label} level"));
                }

                let well = map.rect(Rect::from_min_size(pos2(base, 319.5), vec2(102.0, 511.0)));
                let fader_id = Id::unique(("surface-fader", index));
                let response = ui.interact(well, fader_id, Sense::click_and_drag());
                if response.drag_started() {
                    ui.ctx().data_mut(|data| data.insert_temp(fader_id, fader));
                }
                if (response.dragged() || response.clicked())
                    && let Some(pos) = response.interact_pointer_pos()
                {
                    let track_top = map.point(base + 43.5, 350.0).y;
                    let track_bottom = map.point(base + 43.5, 801.0).y;
                    let fraction =
                        ((track_bottom - pos.y) / (track_bottom - track_top)).clamp(0.0, 1.0);
                    let next = (fraction * 100.0).round() as u8;
                    let last = ui
                        .ctx()
                        .data(|data| data.get_temp::<u8>(fader_id))
                        .unwrap_or(fader);
                    if next != last {
                        ui.ctx().data_mut(|data| data.insert_temp(fader_id, next));
                        let command = if is_source {
                            DaemonRequest::Pipewire(APICommand::SetSourceVolume(
                                channel.id,
                                if is_swapped { Mix::B } else { Mix::A },
                                next,
                            ))
                        } else {
                            DaemonRequest::Pipewire(APICommand::SetTargetVolume(channel.id, next))
                        };
                        let _ = self.commands.try_send(command);
                    }
                    fader = next;
                }
                let fader_label = if is_source {
                    if is_swapped { &labels[1] } else { &labels[0] }
                } else if channel.mix_b {
                    &labels[1]
                } else {
                    &labels[0]
                };
                response.on_hover_text(format!("Drag to set the {fader_label} level"));
            }

            let plate = map.rect(Rect::from_min_size(
                pos2(base + 1.0, 286.0),
                vec2(100.0, 24.0),
            ));
            let name_font = FontId::proportional((14.0 * scale).max(8.5));
            let name_width = painter
                .layout_no_wrap(channel.name.clone(), name_font.clone(), palette.text)
                .size()
                .x;
            painter.with_clip_rect(plate).text(
                plate.center() - vec2(0.0, 2.0 * scale),
                Align2::CENTER_CENTER,
                &channel.name,
                name_font,
                palette.text,
            );
            painter.rect_filled(
                Rect::from_min_size(
                    pos2(
                        plate.center().x - name_width * 0.5 - 4.0 * scale,
                        plate.bottom() - 3.0 * scale,
                    ),
                    vec2(name_width + 8.0 * scale, (2.5 * scale).max(1.5)),
                ),
                0,
                channel_colour.lerp_to_gamma(palette.dim, offline),
            );

            let centre = map.point(base + 51.5, 170.0);
            let cap_radius = 44.0 * scale;
            if knob_disabled {
                painter.circle_filled(centre, cap_radius, palette.card.gamma_multiply(0.7));
                painter.circle_stroke(
                    centre,
                    cap_radius,
                    Stroke::new((1.5 * scale).max(1.0), palette.border),
                );
            } else {
                let arc_radius = 52.0 * scale;
                let start_angle = -135.0f32.to_radians();
                let end_angle = start_angle + 270.0f32.to_radians() * (knob as f32 / 100.0);
                let steps = 48;
                let mut points = Vec::with_capacity(steps + 1);
                for step in 0..=steps {
                    let angle =
                        start_angle + (end_angle - start_angle) * step as f32 / steps as f32;
                    points.push(centre + vec2(angle.sin(), -angle.cos()) * arc_radius);
                }
                if knob > 0 {
                    painter.add(Shape::line(
                        points,
                        Stroke::new((3.0 * scale).max(1.5), knob_colour),
                    ));
                }

                let cap_fill = if knob_hovered {
                    blend(palette.card, knob_mix, 0.25)
                } else {
                    palette.card
                };
                let cap_border = if knob_hovered {
                    knob_mix
                } else {
                    palette.border
                };
                painter.circle_filled(centre, cap_radius, cap_fill);
                painter.circle_stroke(
                    centre,
                    cap_radius,
                    Stroke::new((1.5 * scale).max(1.0), cap_border),
                );
                let pointer_angle =
                    -135.0f32.to_radians() + 270.0f32.to_radians() * (knob as f32 / 100.0);
                let tip = centre + vec2(pointer_angle.sin(), -pointer_angle.cos()) * (36.0 * scale);
                let pointer_stroke = (3.0 * scale).max(1.5);
                if channel_colours && !knob_muted {
                    // Subtle gradient from the mix colour at the centre to
                    // the channel colour at the tip. Only partway toward the
                    // mix at the centre so the tint stays understated.
                    const STEPS: usize = 14;
                    let start = knob_colour.lerp_to_gamma(knob_mix, 0.75);
                    let mut previous = centre;
                    for step in 1..=STEPS {
                        let t = step as f32 / STEPS as f32;
                        let point = centre + (tip - centre) * t;
                        let colour = start.lerp_to_gamma(knob_colour, t);
                        painter
                            .line_segment([previous, point], Stroke::new(pointer_stroke, colour));
                        previous = point;
                    }
                } else {
                    painter.line_segment([centre, tip], Stroke::new(pointer_stroke, knob_colour));
                }
                painter.text(
                    centre + vec2(0.0, 92.0 * scale),
                    Align2::CENTER_CENTER,
                    format!("{knob}%"),
                    FontId::proportional((13.0 * scale).max(9.0)),
                    if channel_colours {
                        knob_mix
                    } else {
                        palette.text
                    },
                );
            }

            let slot_x = base + 43.5;
            let fill_top = 350.0 + 451.0 * (1.0 - fader as f32 / 100.0);
            if fader > 0 {
                let fill = map.rect(Rect::from_min_size(
                    pos2(slot_x, fill_top),
                    vec2(15.0, 801.0 - fill_top),
                ));
                painter.rect_filled(fill, map.radius(6.0), fader_colour);
            }

            let meter = self.engine.meter(channel.id);
            if meter > 0 {
                let meter_height = 451.0 * meter.min(100) as f32 / 100.0;
                let meter_rect = map.rect(Rect::from_min_size(
                    pos2(base + 90.0, 801.0 - meter_height),
                    vec2(4.0, meter_height),
                ));
                painter.rect_filled(
                    meter_rect,
                    map.radius(2.0),
                    if meter > 85 {
                        palette.amber
                    } else {
                        palette.green
                    },
                );
            }

            let thumb_centre = map.point(slot_x + 7.5, fill_top);
            let thumb = Rect::from_center_size(thumb_centre, map.size(vec2(74.0, 32.0)));
            let fader_hovered = interactive
                && ui
                    .ctx()
                    .pointer_hover_pos()
                    .is_some_and(|pos| thumb.contains(pos));
            let thumb_fill = if fader_hovered {
                blend(palette.card_hover, fader_mix, 0.25)
            } else {
                palette.card_hover
            };
            let thumb_border = if fader_hovered {
                fader_mix
            } else {
                palette.border
            };
            painter.rect_filled(thumb, map.radius(6.0), thumb_fill);
            painter.rect_stroke(
                thumb,
                map.radius(6.0),
                Stroke::new((1.5 * scale).max(1.0), thumb_border),
                StrokeKind::Inside,
            );
            let value_text = format!("{fader}%");
            let value_font = FontId::proportional((14.0 * scale).max(9.0));
            let value_colour = if channel_colours {
                fader_mix
            } else {
                palette.text
            };
            let value_width = painter
                .layout_no_wrap(value_text.clone(), value_font.clone(), value_colour)
                .size()
                .x;
            let line_y = thumb.center().y;
            let line_gap = 4.0 * scale;
            let line_stroke = Stroke::new(
                (2.0 * scale).max(1.0),
                if channel_colours {
                    fader_mix
                } else {
                    fader_colour
                },
            );
            let left_start = thumb.left() + 6.0 * scale;
            let left_end = thumb.center().x - value_width * 0.5 - line_gap;
            let right_start = thumb.center().x + value_width * 0.5 + line_gap;
            let right_end = thumb.right() - 6.0 * scale;
            if left_end > left_start {
                painter.line_segment(
                    [pos2(left_start, line_y), pos2(left_end, line_y)],
                    line_stroke,
                );
            }
            if right_end > right_start {
                painter.line_segment(
                    [pos2(right_start, line_y), pos2(right_end, line_y)],
                    line_stroke,
                );
            }
            painter.text(
                thumb.center(),
                Align2::CENTER_CENTER,
                value_text,
                value_font,
                value_colour,
            );

            let slots = if is_source { 4 } else { 3 };
            for slot in 0..slots {
                let well = map.rect(Rect::from_min_size(
                    pos2(base + 128.0, 364.5 + 116.0 * slot as f32),
                    vec2(61.0, 61.0),
                ));
                let rect = well.shrink(4.0 * scale);
                let id = Id::unique(("surface-button", index, slot));
                let sense = if interactive {
                    Sense::click()
                } else {
                    Sense::hover()
                };
                let response = ui.interact(well, id, sense);
                let (label, colour, active, tooltip) = if is_source {
                    match slot {
                        0 => (
                            "M",
                            palette.red,
                            channel.mute_a,
                            format!("Mute the {} mix for this strip", labels[0]),
                        ),
                        1 => (
                            "A",
                            palette.red,
                            channel.mute_b,
                            format!("Mute the {} mix for this strip", labels[1]),
                        ),
                        2 => (
                            "R",
                            palette.red,
                            is_swapped,
                            String::from("Swap this strip: the fader and knob trade mixes"),
                        ),
                        _ => (
                            "■",
                            palette.text,
                            channel.linked,
                            String::from("Link both mixes for this strip; lit = linked"),
                        ),
                    }
                } else {
                    match slot {
                        0 => (
                            "M",
                            palette.red,
                            channel.mute_a,
                            String::from("Mute this target"),
                        ),
                        1 => (
                            "A",
                            palette.accent,
                            !channel.mix_b,
                            format!("Route this target to {}; the fader turns blue", labels[0]),
                        ),
                        _ => (
                            "B",
                            palette.amber,
                            channel.mix_b,
                            format!("Route this target to {}; the fader turns amber", labels[1]),
                        ),
                    }
                };
                let response = response.on_hover_text(tooltip);
                let fill = if active {
                    colour
                } else if response.hovered() {
                    palette.card_hover
                } else {
                    palette.card
                };
                painter.rect_filled(rect, map.radius(7.0), fill);
                painter.rect_stroke(
                    rect,
                    map.radius(7.0),
                    Stroke::new(
                        (1.5 * scale).max(1.0),
                        if active { colour } else { palette.border },
                    ),
                    StrokeKind::Inside,
                );
                let text_colour = if active {
                    on_colour(colour, palette)
                } else {
                    palette.text
                };
                if is_source && slot == 3 {
                    paint_link_icon(painter, rect, scale, text_colour);
                } else {
                    painter.text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        label,
                        FontId::proportional((20.0 * scale).max(11.0)),
                        text_colour,
                    );
                }

                if interactive && response.clicked() {
                    let request = if is_source {
                        match slot {
                            0 => {
                                let command = if channel.mute_a {
                                    APICommand::DelSourceMuteTarget(channel.id, MuteTarget::TargetA)
                                } else {
                                    APICommand::AddSourceMuteTarget(channel.id, MuteTarget::TargetA)
                                };
                                Some(DaemonRequest::Pipewire(command))
                            }
                            1 => {
                                let command = if channel.mute_b {
                                    APICommand::DelSourceMuteTarget(channel.id, MuteTarget::TargetB)
                                } else {
                                    APICommand::AddSourceMuteTarget(channel.id, MuteTarget::TargetB)
                                };
                                Some(DaemonRequest::Pipewire(command))
                            }
                            2 => {
                                if !(channel.linked && self.config.linked_reverse_disabled) {
                                    self.swap.fetch_xor(1 << index, Ordering::Relaxed);
                                    self.ui_reset.fetch_or(1 << index, Ordering::Relaxed);
                                }
                                None
                            }
                            _ => {
                                self.ui_reset.fetch_or(1 << index, Ordering::Relaxed);
                                Some(DaemonRequest::Pipewire(APICommand::SetSourceVolumeLinked(
                                    channel.id,
                                    !channel.linked,
                                )))
                            }
                        }
                    } else {
                        match slot {
                            0 => {
                                let state = if channel.mute_a {
                                    MuteState::Unmuted
                                } else {
                                    MuteState::Muted
                                };
                                Some(DaemonRequest::Pipewire(APICommand::SetTargetMuteState(
                                    channel.id, state,
                                )))
                            }
                            1 if channel.mix_b => {
                                self.ui_reset.fetch_or(1 << index, Ordering::Relaxed);
                                Some(DaemonRequest::Pipewire(APICommand::SetTargetMix(
                                    channel.id,
                                    Mix::A,
                                )))
                            }
                            2 if !channel.mix_b => {
                                self.ui_reset.fetch_or(1 << index, Ordering::Relaxed);
                                Some(DaemonRequest::Pipewire(APICommand::SetTargetMix(
                                    channel.id,
                                    Mix::B,
                                )))
                            }
                            _ => None,
                        }
                    };
                    if let Some(request) = request {
                        let _ = self.commands.try_send(request);
                    }
                }
            }
        }
    }

    fn paint_offline(&self, ui: &egui::Ui, map: &Map) {
        if self.screen != Screen::Mixer || self.offline_t <= 0.01 {
            return;
        }
        let t = self.offline_t;
        let palette = theme::palette(self.config.light_panel);
        let painter = ui.painter();

        let card = Rect::from_center_size(map.point(993.5, 462.0), map.size(vec2(660.0, 132.0)));
        let card_fill = if self.config.light_panel {
            Color32::from_rgba_unmultiplied(250, 250, 252, (236.0 * t) as u8)
        } else {
            Color32::from_rgba_unmultiplied(19, 23, 29, (236.0 * t) as u8)
        };
        painter.rect_filled(card, map.radius(16.0), card_fill);
        painter.rect_stroke(
            card,
            map.radius(16.0),
            Stroke::new((1.2 * map.scale).max(1.0), palette.border.gamma_multiply(t)),
            StrokeKind::Inside,
        );
        painter.text(
            card.center() - vec2(0.0, (16.0 * map.scale).max(10.0)),
            Align2::CENTER_CENTER,
            "Pipeweaver is not connected",
            FontId::proportional((26.0 * map.scale).max(17.0)),
            palette.text.gamma_multiply(t),
        );
        painter.text(
            card.center() + vec2(0.0, (24.0 * map.scale).max(14.0)),
            Align2::CENTER_CENTER,
            "Start Pipeweaver and it will reconnect automatically.",
            FontId::proportional((15.0 * map.scale).max(10.5)),
            palette.dim.gamma_multiply(t),
        );
    }

    fn paint_transport(&mut self, ui: &mut egui::Ui, map: &Map) {
        if self.screen != Screen::Mixer {
            if self.record_down {
                self.record_down = false;
                let _ = self.media_tx.send(MediaCommand::HexStop);
            }
            return;
        }
        if self.drawer_t > 0.01 {
            if self.record_down {
                self.record_down = false;
                let _ = self.media_tx.send(MediaCommand::HexStop);
            }
            return;
        }
        let palette = theme::palette(self.config.light_panel);
        let hover = if self.config.light_panel {
            Color32::from_black_alpha(10)
        } else {
            Color32::from_white_alpha(18)
        };
        let active = if self.config.light_panel {
            Color32::from_black_alpha(18)
        } else {
            Color32::from_white_alpha(32)
        };
        for index in 0..11 {
            let rect = map.rect(Rect::from_min_size(
                pos2(160.5 + 161.0 * index as f32, 814.5 + theme::BEZEL),
                vec2(86.0, 40.0),
            ));
            let sense = if index == 2 {
                Sense::click_and_drag()
            } else {
                Sense::click()
            };
            let response = ui.interact(rect, Id::unique(("transport", index)), sense);
            if response.hovered() {
                ui.painter().rect_filled(rect, map.radius(9.0), hover);
            }
            if response.is_pointer_button_down_on() {
                ui.painter().rect_filled(rect, map.radius(9.0), active);
            }
            if index == 2
                && !self.record_hex.load(Ordering::Relaxed)
                && self.mic_muted.load(Ordering::Relaxed)
            {
                ui.painter()
                    .rect_filled(rect, map.radius(9.0), palette.red.gamma_multiply(0.22));
                ui.painter().rect_stroke(
                    rect,
                    map.radius(9.0),
                    Stroke::new(1.5, palette.red),
                    StrokeKind::Inside,
                );
            }
            let tooltip = match index {
                0 => "Play",
                1 => "Pause",
                2 => {
                    if self.record_hex.load(Ordering::Relaxed) {
                        "Hold to dictate with Hex"
                    } else if self.mic_muted.load(Ordering::Relaxed) {
                        "Microphone muted - press to unmute"
                    } else {
                        "Microphone live - press to mute"
                    }
                }
                3 => "Seek back 5 seconds",
                4 => "Seek forward 5 seconds",
                5 => "Previous track",
                6 => "Next track",
                7 => "Give targets another strip",
                8 => "Give sources another strip",
                9 => "Show or hide this app",
                _ => "Open Pipeweaver",
            };
            let response = response.on_hover_text(tooltip);
            match index {
                0 => {
                    if response.clicked() {
                        let _ = self.media_tx.send(MediaCommand::Play);
                    }
                }
                1 => {
                    if response.clicked() {
                        let _ = self.media_tx.send(MediaCommand::Pause);
                    }
                }
                2 => {
                    if self.record_hex.load(Ordering::Relaxed) {
                        if self.hex_latch_net.load(Ordering::Relaxed) {
                            if response.clicked() {
                                if self.hex_active.swap(false, Ordering::Relaxed) {
                                    let _ = self.media_tx.send(MediaCommand::HexStop);
                                } else {
                                    self.hex_active.store(true, Ordering::Relaxed);
                                    let _ = self.media_tx.send(MediaCommand::HexStart);
                                }
                            }
                            self.record_down = false;
                        } else {
                            let down = response.is_pointer_button_down_on();
                            if down && !self.record_down {
                                self.record_down = true;
                                let _ = self.media_tx.send(MediaCommand::HexStart);
                            } else if !down && self.record_down {
                                self.record_down = false;
                                let _ = self.media_tx.send(MediaCommand::HexStop);
                            }
                        }
                    } else {
                        if self.record_down {
                            self.record_down = false;
                            let _ = self.media_tx.send(MediaCommand::HexStop);
                        }
                        if response.clicked() {
                            let _ = self.media_tx.send(MediaCommand::ToggleMicMute);
                        }
                    }
                }
                3 => {
                    if response.clicked() {
                        let _ = self.media_tx.send(MediaCommand::SeekBack);
                    }
                }
                4 => {
                    if response.clicked() {
                        let _ = self.media_tx.send(MediaCommand::SeekForward);
                    }
                }
                5 => {
                    if response.clicked() {
                        let _ = self.media_tx.send(MediaCommand::Previous);
                    }
                }
                6 => {
                    if response.clicked() {
                        let _ = self.media_tx.send(MediaCommand::Next);
                    }
                }
                7 => {
                    if response.clicked() {
                        self.set_split(self.engine.source_slots().saturating_sub(1));
                    }
                }
                8 => {
                    if response.clicked() {
                        self.set_split(self.engine.source_slots() + 1);
                    }
                }
                9 => {
                    if response.clicked() {
                        let _ = self.media_tx.send(MediaCommand::ToggleApp);
                    }
                }
                _ => {
                    if response.clicked() {
                        let _ = self.media_tx.send(MediaCommand::OpenPipeweaver);
                    }
                }
            }
        }
    }

    fn paint_bezel(&mut self, ui: &mut egui::Ui, map: &Map, ctx: &Context, monitor: Option<Rect>) {
        let drag = map.rect(theme::bezel_drag());
        let response = ui.interact(drag, Id::unique(("bezel-drag",)), Sense::click_and_drag());
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
        .on_hover_text(match self.midi {
            Link::Wired => "SMC-Mixer: wired",
            Link::Bluetooth => "SMC-Mixer: Bluetooth",
            Link::Offline => "SMC-Mixer: not detected",
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
                match self.midi {
                    Link::Wired => palette.green,
                    Link::Bluetooth => palette.accent,
                    Link::Offline => palette.red,
                },
            );
        }

        for (index, tooltip) in ["Minimize", "Maximize", "Close"].iter().enumerate() {
            let rect = map.rect(theme::window_control_rect(index));
            let response = ui.interact(rect, Id::unique(("window-control", index)), Sense::click());
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
                    0 => {
                        self.minimized.store(true, Ordering::Relaxed);
                        self.minimize_at = Some(Instant::now());
                        ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
                    }
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
            (Screen::Pipeweaver, theme::nav_pipeweaver(), "PIPEWEAVER"),
        ];
        let palette = theme::palette(self.config.light_panel);
        for (screen, rect, label) in entries {
            let rect = map.rect(rect);
            let response = ui.interact(rect, Id::unique(("nav", label)), Sense::click());
            let active = self.screen == screen;
            let on_panel = matches!(self.screen, Screen::Config | Screen::Pipeweaver);
            let painter = ui.painter();
            let shape = map.radius(18.5);
            if on_panel {
                painter.rect_filled(rect, shape, palette.card);
                painter.rect_stroke(
                    rect,
                    shape,
                    Stroke::new((1.2 * map.scale).max(1.0), palette.border),
                    StrokeKind::Inside,
                );
            }
            if response.hovered() && !active {
                painter.rect_filled(
                    rect.shrink(3.0 * map.scale),
                    map.radius(15.5),
                    palette.hover,
                );
            }
            if active {
                painter.rect_stroke(
                    rect,
                    shape,
                    Stroke::new((1.5 * map.scale).max(1.0), palette.accent),
                    StrokeKind::Inside,
                );
            }
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                label,
                FontId::proportional((15.0 * map.scale).max(8.0)),
                if active { palette.accent } else { palette.text },
            );
            if response.clicked() && self.screen != screen {
                self.screen = screen;
                if screen == Screen::Config {
                    // Pick up a mode changed outside the app (vendor editor,
                    // preset switch) without polling the mixer.
                    self.knob_mode
                        .request
                        .store(REQUEST_DETECT, Ordering::Relaxed);
                }
            }
        }
    }

    fn paint_pipeweaver(&self, ui: &egui::Ui, map: &Map) {
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

        let left = panel.left() + 28.0 * map.scale;
        painter.text(
            pos2(left, panel.top() + 28.0 * map.scale),
            Align2::LEFT_TOP,
            "Pipeweaver",
            FontId::proportional((34.0 * map.scale).max(22.0)),
            palette.text,
        );
        painter.text(
            pos2(left, panel.top() + 74.0 * map.scale),
            Align2::LEFT_TOP,
            "Applications and routing will live here.",
            FontId::proportional((16.0 * map.scale).max(12.0)),
            palette.dim,
        );
    }

    fn paint_config(&mut self, ui: &mut egui::Ui, map: &Map, ctx: &Context, monitor: Option<Rect>) {
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

        let mut inner = panel.shrink(28.0 * map.scale);
        inner.max.x = panel.max.x - 6.0 * map.scale;
        let width = self.window_width;
        let mut light = self.config.light_panel;
        let mut pickup = self.pickup_net.load(Ordering::Relaxed);
        let knob_mode = self.knob_mode.mode.load(Ordering::Relaxed);
        let knob_busy = self.knob_mode.busy.load(Ordering::Relaxed);
        let knob_detail = self.knob_mode.detail();
        let mut knob_click = None;
        let mut open_knob_info = false;
        let mut channel_colours = self.config.channel_colours;
        let mut hide_off_surface = self.config.hide_off_surface;
        let mut record_hex = self.record_hex.load(Ordering::Relaxed);
        let mut hex_latch = self.hex_latch_net.load(Ordering::Relaxed);
        let mut linked_knob_disabled = self.linked_knob_disabled_net.load(Ordering::Relaxed);
        let mut linked_reverse_disabled = self.linked_reverse_disabled_net.load(Ordering::Relaxed);
        let mut start_minimized = self.config.start_minimized;
        let mut startup_animation = self.startup_animation.load(Ordering::Relaxed);
        let autostart_present = autostart_path().exists();
        let mut start_on_login = autostart_present;
        let mut reset = false;
        let heading = (34.0 * map.scale).max(22.0);
        let label = (16.0 * map.scale).max(12.0);
        let section = (18.0 * map.scale).max(13.0);

        ui.scope_builder(
            UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::LEFT)),
            |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("config-scroll")
                    .auto_shrink([false, false])
                    .scroll_source(egui::scroll_area::ScrollSource::ALL)
                    .show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                ui.label(
                    RichText::new("Config")
                        .size(heading)
                        .strong()
                        .color(palette.text),
                );
                ui.label(
                    RichText::new(
                        "Connection, surface, startup and diagnostics.",
                    )
                    .size(label)
                    .color(palette.dim),
                );
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);

                ui.columns(2, |columns| {
                    columns[0].add_space(40.0 * map.scale);
                    columns[1].add_space(40.0 * map.scale);
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
                    columns[0].checkbox(&mut channel_colours, "Use channel colours");
                    columns[0].label(
                        RichText::new(
                            "Bars use the Pipeweaver channel colour instead of the Personal and Audience colours.",
                        )
                        .size(label)
                        .color(palette.dim),
                    );
                    columns[0].checkbox(&mut hide_off_surface, "Hide off-surface tray");
                    columns[0].label(
                        RichText::new(
                            "Hides the drawer and its tab. Off-surface channels can still be changed in Pipeweaver.",
                        )
                        .size(label)
                        .color(palette.dim),
                    );
                    let off_surface: Vec<String> = self
                        .engine
                        .snapshot()
                        .off_surface
                        .iter()
                        .map(|channel| channel.name.clone())
                        .collect();
                    columns[0].label(
                        RichText::new(if off_surface.is_empty() {
                            String::from("Off surface: none")
                        } else {
                            format!("Off surface: {}", off_surface.join(", "))
                        })
                        .size(label)
                        .color(palette.dim),
                    );

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
                    if config_button(
                        &mut columns[0],
                        "Reset size",
                        map.scale,
                        palette,
                        palette.accent,
                    ) {
                        reset = true;
                    }

                    columns[0].add_space(12.0);
                    columns[0].label(
                        RichText::new("Startup")
                            .size(section)
                            .strong()
                            .color(palette.text),
                    );
                    columns[0].checkbox(&mut start_on_login, "Start on login");
                    columns[0].label(
                        RichText::new("Adds an autostart entry for the next sign-in.")
                            .size(label)
                            .color(palette.dim),
                    );
                    columns[0].checkbox(&mut start_minimized, "Start minimized");
                    columns[0].label(
                        RichText::new("Opens in the tray when launched.")
                            .size(label)
                            .color(palette.dim),
                    );
                    columns[0].checkbox(&mut startup_animation, "Startup animation");
                    columns[0].label(
                        RichText::new("Fills the LEDs once when the app starts.")
                            .size(label)
                            .color(palette.dim),
                    );

                    columns[0].add_space(12.0);
                    columns[0].label(
                        RichText::new("About")
                            .size(section)
                            .strong()
                            .color(palette.text),
                    );
                    columns[0].label(
                        RichText::new(format!(
                            "Pipeweaver SMC Mixer {}",
                            theme::VERSION
                        ))
                        .size(label)
                        .color(palette.text),
                    );
                    columns[0].horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0 * map.scale;
                        ui.hyperlink_to(
                            RichText::new("GitHub").size(label).color(palette.accent),
                            "https://github.com/DeSynkro",
                        );
                        ui.hyperlink_to(
                            RichText::new("Codeberg").size(label).color(palette.accent),
                            "https://codeberg.org/DeSynkro",
                        );
                    });
                    let time = ctx.input(|input| input.time) as f32;
                    let phase = (time / 1.6).fract();
                    let bump = |centre: f32, width: f32| {
                        let distance = (phase - centre).abs();
                        if distance >= width {
                            0.0
                        } else {
                            0.5 + 0.5 * (distance / width * std::f32::consts::PI).cos()
                        }
                    };
                    let pulse = (bump(0.22, 0.18) + 0.5 * bump(0.52, 0.14)).min(1.0);
                    let eased = pulse * pulse * (3.0 - 2.0 * pulse);
                    let heart_size = ((13.0 + 2.4 * eased) * map.scale).max(9.0);
                    columns[0].horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0 * map.scale;
                        ui.label(
                            RichText::new("BUILT WITH")
                                .size(label)
                                .color(palette.dim),
                        );
                        let (rect, _) = ui.allocate_exact_size(
                            vec2(18.0 * map.scale, 18.0 * map.scale),
                            Sense::hover(),
                        );
                        ui.painter().text(
                            rect.center(),
                            Align2::CENTER_CENTER,
                            "♥",
                            FontId::proportional(heart_size),
                            palette.red,
                        );
                        ui.label(RichText::new("BY").size(label).color(palette.dim));
                        ui.hyperlink_to(
                            RichText::new("DESYNKRO").size(label).color(palette.accent),
                            "https://desynkro.com",
                        );
                    });

                    columns[1].label(
                        RichText::new("Connection")
                            .size(section)
                            .strong()
                            .color(palette.text),
                    );
                    columns[1].label(
                        RichText::new("MIDI connection")
                            .size(label)
                            .color(palette.dim),
                    );
                    let mut midi_mode =
                        MidiMode::from_code(self.midi_mode.load(Ordering::Relaxed));
                    let combo_width = (150.0 * map.scale).max(110.0);
                    let combo_height = (26.0 * map.scale).max(20.0);
                    columns[1].horizontal(|ui| {
                        ui.allocate_ui_with_layout(
                            vec2(combo_width, combo_height),
                            Layout::left_to_right(Align::Center),
                            |ui| {
                                ui.set_max_width(combo_width);
                                let radius =
                                    egui::CornerRadius::same((7.0 * map.scale).round() as u8);
                                let border =
                                    Stroke::new((1.2 * map.scale).max(1.0), palette.border);
                                let accent_border =
                                    Stroke::new((1.2 * map.scale).max(1.0), palette.accent);
                                {
                                    let style = ui.style_mut();
                                    style.spacing.interact_size.y =
                                        (26.0 * map.scale).max(20.0);
                                    style.visuals.widgets.inactive.bg_fill = palette.card;
                                    style.visuals.widgets.inactive.bg_stroke = border;
                                    style.visuals.widgets.inactive.corner_radius = radius;
                                    style.visuals.widgets.inactive.fg_stroke =
                                        Stroke::new(1.0, palette.accent);
                                    style.visuals.widgets.hovered.bg_fill =
                                        blend(palette.card, palette.accent, 0.18);
                                    style.visuals.widgets.hovered.bg_stroke = accent_border;
                                    style.visuals.widgets.hovered.corner_radius = radius;
                                    style.visuals.widgets.hovered.fg_stroke =
                                        Stroke::new(1.0, palette.accent);
                                    style.visuals.widgets.active.bg_fill =
                                        blend(palette.card, palette.accent, 0.24);
                                    style.visuals.widgets.active.bg_stroke = accent_border;
                                    style.visuals.widgets.active.corner_radius = radius;
                                    style.visuals.widgets.active.fg_stroke =
                                        Stroke::new(1.0, palette.accent);
                                }
                                egui::ComboBox::from_id_salt("midi-mode")
                                    .width(combo_width)
                                    .selected_text(
                                        RichText::new(match midi_mode {
                                            MidiMode::Auto => "Auto",
                                            MidiMode::Wired => "Wired",
                                            MidiMode::Bluetooth => "Bluetooth",
                                        })
                                        .color(palette.accent),
                                    )
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut midi_mode,
                                            MidiMode::Auto,
                                            "Auto (wired first)",
                                        );
                                        ui.selectable_value(
                                            &mut midi_mode,
                                            MidiMode::Wired,
                                            "Wired",
                                        );
                                        ui.selectable_value(
                                            &mut midi_mode,
                                            MidiMode::Bluetooth,
                                            "Bluetooth",
                                        );
                                    });
                            },
                        );
                    });
                    columns[1].label(
                        RichText::new(format!(
                            "MIDI port: {}",
                            if self.config.midi_device.is_empty() {
                                "Auto"
                            } else {
                                self.config.midi_device.as_str()
                            }
                        ))
                        .size(label)
                        .color(palette.dim),
                    );
                    columns[1].label(
                        RichText::new(
                            "Auto prefers wired and falls back to Bluetooth; switching reconnects automatically.",
                        )
                        .size(label)
                        .color(palette.dim),
                    );
                    if midi_mode != self.config.midi_mode {
                        self.config.midi_mode = midi_mode;
                        self.midi_mode.store(midi_mode.code(), Ordering::Relaxed);
                        self.save_config();
                    }

                    columns[1].add_space(12.0);
                    columns[1].label(
                        RichText::new("Features")
                            .size(section)
                            .strong()
                            .color(palette.text),
                    );
                    columns[1].checkbox(&mut pickup, "Soft takeover");
                    columns[1].label(
                        RichText::new(
                            "Faders and knobs wait for the current value before taking over.",
                        )
                        .size(label)
                        .color(palette.dim),
                    );
                    columns[1].horizontal(|ui| {
                        let mut checked = knob_mode == MODE_RELATIVE;
                        let unknown = knob_mode == MODE_UNKNOWN;
                        let response = ui.add_enabled(
                            !knob_busy && !unknown,
                            egui::Checkbox::new(&mut checked, "Endless knobs").indeterminate(unknown),
                        );
                        if response.changed() {
                            knob_click = Some(if checked {
                                KnobAction::Enable
                            } else {
                                KnobAction::Disable
                            });
                        }
                        if ui
                            .small_button("i")
                            .on_hover_text("How endless knobs work")
                            .clicked()
                        {
                            open_knob_info = true;
                        }
                    });
                    if knob_busy {
                        columns[1].label(
                            RichText::new("Mixer Mode: talking to the mixer...")
                                .size(label)
                                .color(palette.dim),
                        );
                    } else {
                        let (mode_text, detail_text, colour) = match knob_mode {
                            MODE_RELATIVE => (
                                "Relative Mode",
                                "Knobs send steps and never hit the ends.",
                                palette.green,
                            ),
                            MODE_ABSOLUTE => (
                                "Absolute Mode",
                                "Turning it on changes the mixer's mode to Relative mode.",
                                palette.green,
                            ),
                            _ => (
                                "Unknown",
                                "Connect the SMC-Mixer to read its mode.",
                                palette.amber,
                            ),
                        };
                        columns[1].horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            ui.label(
                                RichText::new("Mixer Mode: ")
                                    .size(label)
                                    .strong()
                                    .color(palette.text),
                            );
                            ui.label(
                                RichText::new(mode_text)
                                    .size(label)
                                    .strong()
                                    .color(colour),
                            );
                        });
                        columns[1].label(
                            RichText::new(detail_text).size(label).color(palette.dim),
                        );
                    }
                    if let Some(error) = knob_detail.error.as_deref() {
                        columns[1].label(RichText::new(error).size(label).color(palette.red));
                    } else if let Some(note) = knob_detail.note.as_deref() {
                        columns[1].label(RichText::new(note).size(label).color(palette.amber));
                    }
                    columns[1].checkbox(&mut linked_knob_disabled, "Disable knobs while linked");
                    columns[1].horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.label(
                            RichText::new("On")
                                .size(label)
                                .strong()
                                .color(palette.green),
                        );
                        ui.label(
                            RichText::new(
                                ": the knobs are disabled until the source is unlinked.",
                            )
                            .size(label)
                            .color(palette.dim),
                        );
                    });
                    columns[1].horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.label(
                            RichText::new("Off")
                                .size(label)
                                .strong()
                                .color(palette.red),
                        );
                        ui.label(
                            RichText::new(": the knobs remain active.")
                                .size(label)
                                .color(palette.dim),
                        );
                    });
                    columns[1].checkbox(
                        &mut linked_reverse_disabled,
                        "Disable reverse function (R Button) while linked",
                    );
                    columns[1].horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.label(
                            RichText::new("On")
                                .size(label)
                                .strong()
                                .color(palette.green),
                        );
                        ui.label(
                            RichText::new(": R is disabled while the sources are linked.")
                                .size(label)
                                .color(palette.dim),
                        );
                    });
                    columns[1].horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.label(
                            RichText::new("Off")
                                .size(label)
                                .strong()
                                .color(palette.red),
                        );
                        ui.label(
                            RichText::new(": R button retains its function.")
                                .size(label)
                                .color(palette.dim),
                        );
                    });
                    columns[1].horizontal(|ui| {
                        ui.checkbox(&mut record_hex, "Record button switches to");
                        ui.hyperlink_to("Hex", "https://github.com/anomalyco/hex");
                    });
                    columns[1].horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.label(
                            RichText::new("On")
                                .size(label)
                                .strong()
                                .color(palette.green),
                        );
                        ui.label(
                            RichText::new(": the record button controls Hex dictation.")
                                .size(label)
                                .color(palette.dim),
                        );
                    });
                    columns[1].horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.label(
                            RichText::new("Off")
                                .size(label)
                                .strong()
                                .color(palette.red),
                        );
                        ui.label(
                            RichText::new(
                                ": press to mute or unmute the default microphone.",
                            )
                            .size(label)
                            .color(palette.dim),
                        );
                    });
                    if record_hex {
                        columns[1].checkbox(&mut hex_latch, "Toggle Hex recording");
                        columns[1].horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            ui.label(
                                RichText::new("On")
                                    .size(label)
                                    .strong()
                                    .color(palette.green),
                            );
                            ui.label(
                                RichText::new(
                                    ": Press once to start dictating and again to stop.",
                                )
                                .size(label)
                                .color(palette.dim),
                            );
                        });
                        columns[1].horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            ui.label(
                                RichText::new("Off")
                                    .size(label)
                                    .strong()
                                    .color(palette.red),
                            );
                            ui.label(
                                RichText::new(": hold the record button while you speak.")
                                    .size(label)
                                    .color(palette.dim),
                            );
                        });
                    }
                    let mic_muted = self.mic_muted.load(Ordering::Relaxed);
                    columns[1].label(
                        RichText::new(format!(
                            "Default microphone: {}",
                            if mic_muted { "muted" } else { "live" }
                        ))
                        .size(label)
                        .color(if mic_muted { palette.red } else { palette.dim }),
                    );

                    columns[1].add_space(12.0);
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
                        RichText::new(match self.midi {
                            Link::Wired => "MIDI: SMC-Mixer (wired)",
                            Link::Bluetooth => "MIDI: SMC-Mixer (Bluetooth)",
                            Link::Offline => "MIDI: waiting",
                        })
                        .size(label)
                        .color(palette.dim),
                    );

                    columns[1].add_space(12.0);
                    columns[1].label(
                        RichText::new("Diagnostics")
                            .size(section)
                            .strong()
                            .color(palette.text),
                    );
                    columns[1].label(
                        RichText::new(format!("Config: {}", self.config_path.display()))
                            .size(label)
                            .color(palette.dim),
                    );
                    let last_midi = self.last_midi.load(Ordering::Relaxed);
                    let activity = if last_midi == 0 {
                        String::from("Last MIDI activity: none yet")
                    } else {
                        let now = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map(|duration| duration.as_millis() as u64)
                            .unwrap_or(last_midi);
                        format!(
                            "Last MIDI activity: {}s ago",
                            now.saturating_sub(last_midi) / 1000
                        )
                    };
                    columns[1].label(RichText::new(activity).size(label).color(palette.dim));
                    columns[1].horizontal(|ui| {
                        if config_button(
                            ui,
                            "Flash LEDs",
                            map.scale,
                            palette,
                            palette.accent,
                        ) {
                            self.led_test.store(true, Ordering::Relaxed);
                        }
                        if config_button(
                            ui,
                            "Preview animation",
                            map.scale,
                            palette,
                            palette.accent,
                        ) {
                            self.animation_preview.store(true, Ordering::Relaxed);
                        }
                    });
                    let snapshot = self.engine.snapshot();
                    let swapped = self.swap.load(Ordering::Relaxed);
                    for (index, channel) in snapshot.channels.iter().enumerate() {
                        if !channel.assigned {
                            continue;
                        }
                        let kind = if channel.kind == ChannelKind::Source {
                            "source"
                        } else {
                            "target"
                        };
                        let ccs = if channel.kind == ChannelKind::Source {
                            format!("fader {} · knob {}", 40 + index, 30 + index)
                        } else {
                            format!("fader {}", 40 + index)
                        };
                        let state = if channel.kind == ChannelKind::Source {
                            let mut parts = Vec::new();
                            if swapped & (1 << index) != 0 {
                                parts.push("swap");
                            }
                            if channel.linked {
                                parts.push("linked");
                            }
                            if parts.is_empty() {
                                String::new()
                            } else {
                                format!(" · {}", parts.join(" · "))
                            }
                        } else {
                            String::new()
                        };
                        columns[1].label(
                            RichText::new(format!(
                                "{}: {} · {kind} · {ccs}{state}",
                                index + 1,
                                channel.name
                            ))
                            .size(label)
                            .color(palette.dim),
                        );
                    }

                    columns[1].add_space(12.0);
                    if config_button(
                        &mut columns[1],
                        "Quit app",
                        map.scale,
                        palette,
                        palette.red,
                    ) {
                        self.save_config();
                        self.stop.store(true, Ordering::Relaxed);
                        self.ui_on.store(false, Ordering::Relaxed);
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                });
                    });
            },
        );

        if light != self.config.light_panel {
            self.config.light_panel = light;
            ctx.set_visuals(theme::visuals(light));
            self.save_config();
        }
        if pickup != self.config.pickup {
            self.config.pickup = pickup;
            self.pickup_net.store(pickup, Ordering::Relaxed);
            self.save_config();
        }
        if channel_colours != self.config.channel_colours {
            self.config.channel_colours = channel_colours;
            self.save_config();
        }
        if hide_off_surface != self.config.hide_off_surface {
            self.config.hide_off_surface = hide_off_surface;
            if hide_off_surface {
                self.drawer_open = false;
                self.drawer_t = 0.0;
            }
            self.save_config();
        }
        if start_minimized != self.config.start_minimized {
            self.config.start_minimized = start_minimized;
            self.save_config();
            if start_on_login && let Err(error) = set_autostart(true, start_minimized) {
                log::warn!("Could not update autostart: {error:#}");
            }
        }
        if startup_animation != self.startup_animation.load(Ordering::Relaxed) {
            self.startup_animation
                .store(startup_animation, Ordering::Relaxed);
            self.config.startup_animation = startup_animation;
            self.save_config();
        }
        if start_on_login != autostart_present
            && let Err(error) = set_autostart(start_on_login, start_minimized)
        {
            log::warn!("Could not update autostart: {error:#}");
        }
        if record_hex != self.record_hex.load(Ordering::Relaxed) {
            self.record_hex.store(record_hex, Ordering::Relaxed);
            self.config.record_hex = record_hex;
            if !record_hex {
                self.record_down = false;
                if self.hex_active.swap(false, Ordering::Relaxed) {
                    let _ = self.media_tx.send(MediaCommand::HexStop);
                }
            }
            self.save_config();
        }
        if hex_latch != self.hex_latch_net.load(Ordering::Relaxed) {
            self.hex_latch_net.store(hex_latch, Ordering::Relaxed);
            self.config.hex_latch = hex_latch;
            if self.hex_active.swap(false, Ordering::Relaxed) {
                let _ = self.media_tx.send(MediaCommand::HexStop);
            }
            self.save_config();
        }
        if linked_knob_disabled != self.linked_knob_disabled_net.load(Ordering::Relaxed) {
            self.linked_knob_disabled_net
                .store(linked_knob_disabled, Ordering::Relaxed);
            self.config.linked_knob_disabled = linked_knob_disabled;
            self.save_config();
        }
        if linked_reverse_disabled != self.linked_reverse_disabled_net.load(Ordering::Relaxed) {
            self.linked_reverse_disabled_net
                .store(linked_reverse_disabled, Ordering::Relaxed);
            self.config.linked_reverse_disabled = linked_reverse_disabled;
            self.save_config();
        }
        if reset {
            self.set_window_width(1280.0, ctx, monitor);
        }
        if let Some(action) = knob_click {
            self.knob_action = Some(action);
        }
        if open_knob_info {
            self.knob_info_open = true;
        }
        self.paint_knob_warning(ctx, map, palette);
        self.paint_knob_info(ctx, map, palette);
    }

    fn paint_knob_warning(&mut self, ctx: &Context, map: &Map, palette: &'static theme::Palette) {
        let Some(action) = self.knob_action else {
            return;
        };
        let detail = self.knob_mode.detail();
        let mut apply = false;
        let mut cancel = false;
        egui::Modal::new(Id::unique("knob-mode-warning")).show(ctx, |ui| {
            ui.set_max_width(480.0 * map.scale);
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            let heading = (20.0 * map.scale).max(15.0);
            let body = (15.0 * map.scale).max(11.0);
            let title = match action {
                KnobAction::Enable => "Switch the mixer to Relative mode?",
                KnobAction::Disable => "Switch the mixer back to Absolute mode?",
                KnobAction::Restore => "Restore the saved mixer configuration?",
            };
            ui.label(RichText::new(title).size(heading).strong().color(palette.text));
            let text = match action {
                KnobAction::Enable => {
                    "Endless knobs need the mixer to send step pulses instead of a knob position. That setting lives in the mixer itself, so this changes it for every app and machine that uses it. Any other app and any DAW listening on CC 30-37 will see the new behaviour too.\n\nOnly the eight knobs in the preset the app uses change. A full backup is saved first. Unchecking Endless knobs, or restoring the backup, returns to how it was."
                }
                KnobAction::Disable => {
                    "The mixer will switch to Absolute mode and send knob position, so the knobs stop at 0 and 127. Only the eight knobs in the preset the app uses are affected. A backup is saved first and the change is kept in the mixer."
                }
                KnobAction::Restore => {
                    "The full user preset block from the saved backup is written back to the mixer and kept in its memory. The selected preset is left alone."
                }
            };
            ui.label(RichText::new(text).size(body).color(palette.dim));
            if action != KnobAction::Restore {
                ui.label(
                    RichText::new(format!(
                        "Backups: {}",
                        state_dir().join("mixer-backups").display()
                    ))
                    .size(body)
                    .color(palette.dim),
                );
            } else if let Some(backup) = detail.backup.as_ref() {
                ui.label(
                    RichText::new(format!("Restoring: {}", backup.display()))
                        .size(body)
                        .color(palette.dim),
                );
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
                let apply_text = match action {
                    KnobAction::Enable => "Apply and switch to relative",
                    KnobAction::Disable => "Apply and switch to absolute",
                    KnobAction::Restore => "Restore backup",
                };
                if ui.button(apply_text).clicked() {
                    apply = true;
                }
            });
        });
        if apply {
            let request = match action {
                KnobAction::Enable => REQUEST_ENABLE,
                KnobAction::Disable => REQUEST_DISABLE,
                KnobAction::Restore => REQUEST_RESTORE,
            };
            self.knob_mode.request.store(request, Ordering::Relaxed);
            self.knob_action = None;
        } else if cancel {
            self.knob_action = None;
        }
    }

    fn paint_knob_info(&mut self, ctx: &Context, map: &Map, palette: &'static theme::Palette) {
        if !self.knob_info_open {
            return;
        }
        let detail = self.knob_mode.detail();
        let mut close = false;
        let mut restore = false;
        egui::Modal::new(Id::unique("knob-info")).show(ctx, |ui| {
            ui.set_max_width(500.0 * map.scale);
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            let heading = (20.0 * map.scale).max(15.0);
            let body = (15.0 * map.scale).max(11.0);
            ui.label(
                RichText::new("Endless knobs and the mixer's relative mode")
                    .size(heading)
                    .strong()
                    .color(palette.text),
            );
            ui.label(
                RichText::new("The following settings change the internal settings of your mixer.")
                    .size(body)
                    .strong()
                    .color(palette.red),
            );
            ui.add_space(2.0);
            rich_spans(
                ui,
                body,
                palette,
                &[
                    ("In ", Span::Plain),
                    ("Absolute", Span::Strong),
                    (
                        " mode the mixer reports the knob's physical position, 0-127. When a knob reaches either end it has nothing left to send, so it stops. ",
                        Span::Plain,
                    ),
                    ("That is a hardware limit", Span::Underline),
                    (", not something the app can work around.", Span::Plain),
                ],
            );
            rich_spans(
                ui,
                body,
                palette,
                &[
                    ("In ", Span::Plain),
                    ("Relative", Span::Strong),
                    (
                        " mode the mixer sends one small step per detent, in either direction. There is no position to run out of, so the app can keep turning the value forever.",
                        Span::Plain,
                    ),
                ],
            );
            ui.label(
                RichText::new(
                    "The setting is stored in the mixer's internal configuration, so it applies to every app and machine that uses the mixer. Only the eight knob records on CC 30-37 are changed. Buttons, faders, LEDs and the factory DAW preset are untouched.",
                )
                .size(body)
                .color(palette.dim),
            );
            rich_spans(
                ui,
                body,
                palette,
                &[
                    ("Endless Knobs requires ", Span::Plain),
                    ("Relative", Span::Strong),
                    (
                        " mode. The app only reads the mode your mixer is currently using, and changes the mixer only when you use this checkbox. With your consent, Pipeweaver SMC Mixer switches Relative mode on after the warning so Endless Knobs can work. A full backup is saved first. You can switch back to ",
                        Span::Plain,
                    ),
                    ("Absolute", Span::Strong),
                    (
                        " by unchecking the box or by restoring the backup.",
                        Span::Plain,
                    ),
                ],
            );
            if let Some(backup) = detail.backup.as_ref() {
                ui.label(
                    RichText::new(format!("Last backup: {}", backup.display()))
                        .size(body)
                        .color(palette.dim),
                );
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Close").clicked() {
                    close = true;
                }
                if detail.backup.is_some() && ui.button("Restore last backup").clicked() {
                    restore = true;
                }
            });
        });
        if restore {
            self.knob_info_open = false;
            self.knob_action = Some(KnobAction::Restore);
        } else if close {
            self.knob_info_open = false;
        }
    }

    fn paint_drawer(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot) {
        if self.screen != Screen::Mixer {
            self.drawer_open = false;
            self.drawer_t = 0.0;
            return;
        }
        if self.config.hide_off_surface || !snapshot.connected {
            self.drawer_open = false;
            self.drawer_t = 0.0;
            return;
        }
        let palette = theme::palette(self.config.light_panel);
        let sources = snapshot
            .off_surface
            .iter()
            .filter(|channel| channel.kind == ChannelKind::Source)
            .count();
        let targets = snapshot.off_surface.len() - sources;

        if snapshot.off_surface.is_empty() {
            self.drawer_open = false;
            self.drawer_t = 0.0;
            return;
        }

        let target = if self.drawer_open { 1.0 } else { 0.0 };
        self.drawer_t += (target - self.drawer_t) * 0.3;
        if (target - self.drawer_t).abs() < 0.01 {
            self.drawer_t = target;
        }
        let t = self.drawer_t;

        let screen = ui.max_rect();
        let source_word = if sources == 1 { "source" } else { "sources" };
        let target_word = if targets == 1 { "target" } else { "targets" };
        let label = format!("OFF SURFACE · {sources} {source_word} · {targets} {target_word}");
        let font = FontId::proportional(10.0);
        let text_width = ui
            .painter()
            .layout_no_wrap(label.clone(), font.clone(), palette.text)
            .size()
            .x;
        let tab = Rect::from_center_size(
            pos2(screen.center().x, screen.bottom() - 14.0),
            vec2((text_width + 54.0).max(220.0), 22.0),
        );
        let tab_response = ui.interact(tab, Id::unique(("off-surface-tab",)), Sense::click());
        if tab_response.clicked() {
            self.drawer_open = !self.drawer_open;
        }

        let mut requested_split = None;
        if t > 0.01 {
            let count = snapshot.off_surface.len() as f32;
            let card_width = 180.0;
            let gap = 10.0;
            let padding = 24.0;
            let content_width = count * card_width + (count - 1.0).max(0.0) * gap;
            let drawer_width = (content_width + 2.0 * padding).clamp(300.0, screen.width() - 20.0);
            let left = screen.center().x - drawer_width * 0.5;
            let right = left + drawer_width;
            let bottom = screen.bottom() - 28.0;
            let top = bottom - 400.0 * t;
            let drawer = Rect::from_min_max(pos2(left, top), pos2(right, bottom));
            let painter = ui.painter().clone();
            painter.rect_filled(drawer, egui::CornerRadius::same(18), palette.panel);
            painter.rect_stroke(
                drawer,
                egui::CornerRadius::same(18),
                Stroke::new(1.5, palette.border),
                StrokeKind::Inside,
            );
            let header_painter = painter.with_clip_rect(drawer);
            let button = 20.0;
            let gap_x = 5.0;
            let surface_w = 48.0;
            let ratio_w = 42.0;
            let row_w = surface_w + gap_x + button + gap_x + ratio_w + gap_x + button;
            let row_left = right - padding - row_w;

            header_painter.text(
                pos2(left + padding, top + 24.0),
                Align2::LEFT_CENTER,
                "Off surface",
                FontId::proportional(18.0),
                palette.text,
            );
            let subtitle_clip = Rect::from_min_max(
                pos2(left + padding, top + 38.0),
                pos2((row_left - 10.0).max(left + padding + 1.0), top + 54.0),
            )
            .intersect(drawer);
            painter.with_clip_rect(subtitle_clip).text(
                pos2(left + padding, top + 46.0),
                Align2::LEFT_CENTER,
                "Channels not on the eight strips. Mouse control only.",
                FontId::proportional(10.5),
                palette.dim,
            );

            {
                let current = self.engine.source_slots();
                let cy = top + 43.5;
                let surface_rect =
                    Rect::from_min_size(pos2(row_left, cy - 8.0), vec2(surface_w, 16.0));
                let dec = Rect::from_center_size(
                    pos2(row_left + surface_w + gap_x + button * 0.5, cy),
                    vec2(button, button),
                );
                let ratio = Rect::from_center_size(
                    pos2(
                        row_left + surface_w + gap_x + button + gap_x + ratio_w * 0.5,
                        cy,
                    ),
                    vec2(ratio_w, 18.0),
                );
                let inc = Rect::from_center_size(
                    pos2(
                        row_left
                            + surface_w
                            + gap_x
                            + button
                            + gap_x
                            + ratio_w
                            + gap_x
                            + button * 0.5,
                        cy,
                    ),
                    vec2(button, button),
                );

                header_painter.text(
                    surface_rect.center(),
                    Align2::CENTER_CENTER,
                    "SURFACE",
                    FontId::proportional(9.0),
                    palette.dim,
                );

                let dec_response = ui.interact(dec, Id::unique(("split-dec",)), Sense::click());
                let inc_response = ui.interact(inc, Id::unique(("split-inc",)), Sense::click());
                let ratio_response =
                    ui.interact(ratio, Id::unique(("split-reset",)), Sense::click());

                split_button(
                    &header_painter,
                    dec,
                    "<",
                    current > SURFACE_SOURCES_MIN,
                    palette,
                );
                split_button(
                    &header_painter,
                    inc,
                    ">",
                    current < SURFACE_SOURCES_MAX,
                    palette,
                );

                if ratio_response.hovered() {
                    header_painter.rect_filled(
                        ratio,
                        egui::CornerRadius::same(5),
                        palette.card_hover,
                    );
                }
                header_painter.text(
                    ratio.center(),
                    Align2::CENTER_CENTER,
                    format!("{current} / {}", STRIP_COUNT - current),
                    FontId::proportional(12.0),
                    palette.text,
                );

                if t >= 0.99 && dec_response.clicked() && current > SURFACE_SOURCES_MIN {
                    requested_split = Some(current - 1);
                }
                if t >= 0.99 && inc_response.clicked() && current < SURFACE_SOURCES_MAX {
                    requested_split = Some(current + 1);
                }
                if t >= 0.99 && ratio_response.clicked() {
                    requested_split = Some(SURFACE_SOURCES_DEFAULT);
                }
                let _ = ratio_response.on_hover_text("Sources / targets. Click to reset to 6 / 2.");
            }

            let inner = Rect::from_min_max(
                pos2(left + padding, top + 60.0),
                pos2(right - padding, bottom - 12.0),
            );
            let commands = &self.commands;
            let engine = &self.engine;
            let reversed = &mut self.reversed;
            ui.scope_builder(
                UiBuilder::new()
                    .max_rect(inner)
                    .layout(Layout::top_down(Align::LEFT)),
                |ui| {
                    ui.set_clip_rect(inner);
                    ui.spacing_mut().item_spacing = vec2(gap, 6.0);
                    egui::ScrollArea::horizontal()
                        .scroll_source(egui::scroll_area::ScrollSource::ALL)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.horizontal_top(|ui| {
                                for channel in &snapshot.off_surface {
                                    draw_card(
                                        ui,
                                        channel,
                                        palette,
                                        commands,
                                        engine.meter(channel.id),
                                        reversed,
                                        self.config.linked_reverse_disabled,
                                    );
                                }
                            });
                        });
                },
            );
        }

        if let Some(slots) = requested_split {
            self.set_split(slots);
        }

        draw_tab(
            &ui.painter().clone(),
            tab,
            &label,
            &font,
            self.drawer_open,
            palette,
        );
    }

    fn paint_grip(&mut self, ui: &mut egui::Ui, map: &Map, ctx: &Context, monitor: Option<Rect>) {
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
        if snapshot.connected {
            self.offline_since = None;
        } else if self.offline_since.is_none() {
            self.offline_since = Some(Instant::now());
        }
        let offline_target = if !snapshot.connected
            && self
                .offline_since
                .is_some_and(|at| at.elapsed() >= Duration::from_secs(1))
        {
            1.0
        } else {
            0.0
        };
        self.offline_t += (offline_target - self.offline_t) * 0.25;
        if (offline_target - self.offline_t).abs() < 0.01 {
            self.offline_t = offline_target;
        }
        self.midi = Link::from_code(self.midi_net.load(Ordering::Relaxed));
        if let Some(at) = self.minimize_at
            && Instant::now() >= at + Duration::from_millis(400)
            && ctx.input(|input| input.viewport().minimized) == Some(false)
        {
            self.minimized.store(false, Ordering::Relaxed);
            self.minimize_at = None;
        }
        let engine_slots = self.engine.source_slots();
        if engine_slots != self.config.surface_sources {
            self.config.surface_sources = engine_slots;
            self.save_config();
        }

        // The window is created locked so tiling managers keep it floating.
        // Once it has been mapped, allow programmatic resizing again.
        if let Some(at) = self.enable_resize_at
            && Instant::now() >= at
        {
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

        if let Some(at) = self.focus_at
            && Instant::now() >= at
        {
            ctx.send_viewport_cmd(ViewportCommand::Focus);
            self.focus_at = None;
        }
        if self.focus.swap(false, Ordering::Relaxed) {
            ctx.send_viewport_cmd(ViewportCommand::Focus);
        }
        if self.stop.load(Ordering::Relaxed) || !self.ui_on.load(Ordering::Relaxed) {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        if ctx.input(|input| input.viewport().close_requested()) {
            self.save_config();
            if self.tray_ready.load(Ordering::Relaxed) {
                self.ui_on.store(false, Ordering::Relaxed);
            } else {
                // No tray to come back from: closing the window quits.
                self.stop.store(true, Ordering::Relaxed);
                self.ui_on.store(false, Ordering::Relaxed);
            }
        }
        if let Some(rect) = ctx.input(|input| input.viewport().inner_rect) {
            if !self.resizing {
                self.window_width = rect.width();
            }
            if let Some((pos, width, deadline)) = self.pending_move
                && ((rect.width() - width).abs() <= 1.0 || Instant::now() >= deadline)
            {
                ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos));
                self.pending_move = None;
            }
        }
        let minimized = ctx
            .input(|input| input.viewport().minimized)
            .unwrap_or(false);
        ctx.request_repaint_after(if minimized {
            Duration::from_secs(1)
        } else {
            Duration::from_millis(50)
        });

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::TRANSPARENT))
            .show(ui, |ui| {
                let map = Map::new(ui.max_rect());
                self.paint_shell(ui, &map);
                self.paint_channels(ui, &map, &snapshot);
                self.paint_offline(ui, &map);
                self.paint_transport(ui, &map);
                self.paint_bezel(ui, &map, &ctx, monitor);
                match self.screen {
                    Screen::Config => self.paint_config(ui, &map, &ctx, monitor),
                    Screen::Pipeweaver => self.paint_pipeweaver(ui, &map),
                    Screen::Mixer => {}
                }
                self.paint_nav(ui, &map);
                self.paint_drawer(ui, &snapshot);
                self.paint_grip(ui, &map, &ctx, monitor);
            });
    }
}

fn draw_card(
    ui: &mut egui::Ui,
    channel: &Channel,
    palette: &theme::Palette,
    commands: &Sender<DaemonRequest>,
    meter: u8,
    reversed: &mut HashSet<Ulid>,
    disable_reverse_linked: bool,
) {
    let (rect, _) = ui.allocate_exact_size(vec2(180.0, 320.0), Sense::hover());
    let painter = ui.painter().clone();
    painter.rect_filled(rect, egui::CornerRadius::same(10), palette.card);
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(10),
        Stroke::new(1.0, palette.border),
        StrokeKind::Inside,
    );
    let colour = Color32::from_rgb(channel.colour.0, channel.colour.1, channel.colour.2);

    painter.text(
        pos2(rect.center().x, rect.top() + 18.0),
        Align2::CENTER_CENTER,
        &channel.name,
        FontId::proportional(15.0),
        palette.text,
    );
    painter.text(
        pos2(rect.center().x, rect.top() + 34.0),
        Align2::CENTER_CENTER,
        if channel.kind == ChannelKind::Source {
            "SOURCE"
        } else {
            "TARGET"
        },
        FontId::proportional(9.0),
        palette.dim,
    );
    painter.rect_filled(
        Rect::from_min_size(
            pos2(rect.left(), rect.top() + 44.0),
            vec2(rect.width(), 3.0),
        ),
        0,
        colour,
    );

    if channel.kind == ChannelKind::Source {
        let left_colour = if channel.mute_a {
            palette.red
        } else {
            palette.accent
        };
        let right_colour = if channel.mute_b {
            palette.red
        } else {
            palette.amber
        };
        draw_card_slider(
            ui,
            &painter,
            channel,
            palette,
            commands,
            rect,
            0,
            channel.fader,
            "Personal",
            left_colour,
            false,
            Mix::A,
        );
        draw_card_slider(
            ui,
            &painter,
            channel,
            palette,
            commands,
            rect,
            1,
            channel.knob,
            "Audience",
            right_colour,
            false,
            Mix::B,
        );

        let link =
            Rect::from_center_size(pos2(rect.center().x, rect.top() + 150.0), vec2(20.0, 18.0));
        let reverse =
            Rect::from_center_size(pos2(rect.center().x, rect.top() + 178.0), vec2(20.0, 18.0));
        if link_button(
            ui,
            &painter,
            link,
            channel.linked,
            palette,
            Id::unique(("card-link", channel.id)),
        ) {
            let _ = commands.try_send(DaemonRequest::Pipewire(APICommand::SetSourceVolumeLinked(
                channel.id,
                !channel.linked,
            )));
        }
        let is_reversed = reversed.contains(&channel.id);
        let reverse_disabled = channel.linked && disable_reverse_linked;
        if card_button_enabled(
            ui,
            &painter,
            reverse,
            "R",
            is_reversed,
            palette.accent,
            palette,
            Id::unique(("card-reverse", channel.id)),
            !reverse_disabled,
        ) {
            if is_reversed {
                reversed.remove(&channel.id);
            } else {
                reversed.insert(channel.id);
            }
            let _ = commands.try_send(DaemonRequest::Pipewire(APICommand::SetSourceVolume(
                channel.id,
                Mix::A,
                channel.knob,
            )));
            let _ = commands.try_send(DaemonRequest::Pipewire(APICommand::SetSourceVolume(
                channel.id,
                Mix::B,
                channel.fader,
            )));
        }

        let y = rect.top() + 300.0;
        let mute_a = Rect::from_center_size(pos2(rect.left() + 48.0, y), vec2(72.0, 18.0));
        let mute_b = Rect::from_center_size(pos2(rect.left() + 132.0, y), vec2(72.0, 18.0));
        if card_button(
            ui,
            &painter,
            mute_a,
            "Mute A",
            channel.mute_a,
            palette.red,
            palette,
            Id::unique(("card-mute-a", channel.id)),
        ) {
            let command = if channel.mute_a {
                APICommand::DelSourceMuteTarget(channel.id, MuteTarget::TargetA)
            } else {
                APICommand::AddSourceMuteTarget(channel.id, MuteTarget::TargetA)
            };
            let _ = commands.try_send(DaemonRequest::Pipewire(command));
        }
        if card_button(
            ui,
            &painter,
            mute_b,
            "Mute B",
            channel.mute_b,
            palette.red,
            palette,
            Id::unique(("card-mute-b", channel.id)),
        ) {
            let command = if channel.mute_b {
                APICommand::DelSourceMuteTarget(channel.id, MuteTarget::TargetB)
            } else {
                APICommand::AddSourceMuteTarget(channel.id, MuteTarget::TargetB)
            };
            let _ = commands.try_send(DaemonRequest::Pipewire(command));
        }
    } else {
        let mix_colour = if channel.mute_a {
            palette.red
        } else if channel.mix_b {
            palette.amber
        } else {
            palette.accent
        };
        draw_card_slider(
            ui,
            &painter,
            channel,
            palette,
            commands,
            rect,
            0,
            channel.fader,
            if channel.mix_b {
                "Audience"
            } else {
                "Personal"
            },
            mix_colour,
            true,
            Mix::A,
        );

        let y = rect.top() + 300.0;
        let mute = Rect::from_center_size(pos2(rect.center().x - 38.0, y), vec2(70.0, 18.0));
        let a = Rect::from_center_size(pos2(rect.center().x + 22.0, y), vec2(28.0, 18.0));
        let b = Rect::from_center_size(pos2(rect.center().x + 56.0, y), vec2(28.0, 18.0));
        if card_button(
            ui,
            &painter,
            mute,
            "Mute",
            channel.mute_a,
            palette.red,
            palette,
            Id::unique(("card-mute", channel.id)),
        ) {
            let state = if channel.mute_a {
                MuteState::Unmuted
            } else {
                MuteState::Muted
            };
            let _ = commands.try_send(DaemonRequest::Pipewire(APICommand::SetTargetMuteState(
                channel.id, state,
            )));
        }
        if card_button(
            ui,
            &painter,
            a,
            "A",
            !channel.mix_b,
            palette.accent,
            palette,
            Id::unique(("card-mix-a", channel.id)),
        ) {
            let _ = commands.try_send(DaemonRequest::Pipewire(APICommand::SetTargetMix(
                channel.id,
                Mix::A,
            )));
        }
        if card_button(
            ui,
            &painter,
            b,
            "B",
            channel.mix_b,
            palette.amber,
            palette,
            Id::unique(("card-mix-b", channel.id)),
        ) {
            let _ = commands.try_send(DaemonRequest::Pipewire(APICommand::SetTargetMix(
                channel.id,
                Mix::B,
            )));
        }
    }

    let meter_rect = Rect::from_min_size(
        pos2(rect.left() + 14.0, rect.bottom() - 6.0),
        vec2(rect.width() - 28.0, 3.0),
    );
    painter.rect_filled(meter_rect, egui::CornerRadius::same(2), palette.field);
    let fill_width = meter_rect.width() * meter.min(100) as f32 / 100.0;
    if fill_width > 0.0 {
        painter.rect_filled(
            Rect::from_min_size(meter_rect.min, vec2(fill_width, 3.0)),
            egui::CornerRadius::same(2),
            if meter > 85 {
                palette.amber
            } else {
                palette.green
            },
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_card_slider(
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    channel: &Channel,
    palette: &theme::Palette,
    commands: &Sender<DaemonRequest>,
    card: Rect,
    column: usize,
    value: u8,
    label: &str,
    colour: Color32,
    target: bool,
    mix: Mix,
) {
    let cx = if target {
        card.center().x
    } else {
        card.left() + if column == 0 { 48.0 } else { 132.0 }
    };
    let top = card.top() + 78.0;
    let bottom = card.top() + 262.0;
    let mut value = value;

    let hit = Rect::from_center_size(pos2(cx, (top + bottom) * 0.5), vec2(44.0, bottom - top));
    let response = ui.interact(
        hit,
        Id::unique(("off-slider", channel.id, column)),
        Sense::click_and_drag(),
    );
    if (response.dragged() || response.clicked())
        && let Some(pos) = ui.ctx().input(|input| input.pointer.interact_pos())
    {
        let next = (((bottom - pos.y) / (bottom - top)) * 100.0)
            .clamp(0.0, 100.0)
            .round() as u8;
        if next != value {
            value = next;
            let request = if target {
                DaemonRequest::Pipewire(APICommand::SetTargetVolume(channel.id, next))
            } else {
                DaemonRequest::Pipewire(APICommand::SetSourceVolume(channel.id, mix, next))
            };
            let _ = commands.try_send(request);
        }
    }

    let value_y = bottom - (bottom - top) * value as f32 / 100.0;
    painter.text(
        pos2(cx, card.top() + 62.0),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(9.0),
        colour,
    );
    painter.rect_filled(
        Rect::from_center_size(pos2(cx, (top + bottom) * 0.5), vec2(4.0, bottom - top)),
        egui::CornerRadius::same(2),
        palette.field,
    );
    if value > 0 {
        painter.rect_filled(
            Rect::from_center_size(
                pos2(cx, (value_y + bottom) * 0.5),
                vec2(4.0, bottom - value_y),
            ),
            egui::CornerRadius::same(2),
            colour,
        );
    }
    painter.circle_filled(pos2(cx, value_y), 7.0, colour);
    painter.circle_filled(pos2(cx, value_y), 3.0, palette.card);
    painter.text(
        pos2(cx, card.top() + 276.5),
        Align2::CENTER_CENTER,
        format!("{value}%"),
        FontId::proportional(11.0),
        palette.text,
    );
}

fn blend(a: Color32, b: Color32, t: f32) -> Color32 {
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

fn on_colour(colour: Color32, palette: &theme::Palette) -> Color32 {
    let luminance =
        0.299 * colour.r() as f32 + 0.587 * colour.g() as f32 + 0.114 * colour.b() as f32;
    if luminance > 120.0 {
        palette.panel
    } else {
        palette.text
    }
}

/// Two overlapping capsule outlines, used for the link button.
fn paint_link_icon(painter: &egui::Painter, rect: Rect, scale: f32, colour: Color32) {
    let stroke = Stroke::new((2.0 * scale).max(1.2), colour);
    let height = (rect.height() * 0.26).max(8.0 * scale);
    let width = (rect.width() * 0.38).max(12.0 * scale);
    let overlap = (width * 0.28).max(2.0 * scale);
    let offset = (width - overlap) * 0.5;
    let radius = egui::CornerRadius::same((height * 0.5).round() as u8);
    for direction in [-1.0, 1.0] {
        let centre = rect.center() + vec2(direction * offset, 0.0);
        let pill = Rect::from_center_size(centre, vec2(width, height));
        painter.rect_stroke(pill, radius, stroke, StrokeKind::Inside);
    }
}

fn config_button(
    ui: &mut egui::Ui,
    label: &str,
    scale: f32,
    palette: &theme::Palette,
    accent: Color32,
) -> bool {
    let size = vec2((120.0 * scale).max(84.0), (26.0 * scale).max(20.0));
    // `columns` uses a justified layout, which stretches the allocated
    // response to the column width. Interact with the drawn rect instead so
    // the hitbox matches the button.
    let (rect, allocated) = ui.allocate_exact_size(size, Sense::hover());
    let response = ui.interact(rect, allocated.id, Sense::click());
    let fill = if response.hovered() {
        blend(palette.card, accent, 0.18)
    } else {
        palette.card
    };
    let border = if response.hovered() {
        accent
    } else {
        palette.border
    };
    let radius = (7.0 * scale).round() as u8;
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(radius), fill);
    ui.painter().rect_stroke(
        rect,
        egui::CornerRadius::same(radius),
        Stroke::new((1.2 * scale).max(1.0), border),
        StrokeKind::Inside,
    );
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional((13.0 * scale).max(9.0)),
        accent,
    );
    response.clicked()
}

#[allow(clippy::too_many_arguments)]
fn card_button(
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    rect: Rect,
    label: &str,
    active: bool,
    active_colour: Color32,
    palette: &theme::Palette,
    id: Id,
) -> bool {
    card_button_enabled(
        ui,
        painter,
        rect,
        label,
        active,
        active_colour,
        palette,
        id,
        true,
    )
}

#[allow(clippy::too_many_arguments)]
fn card_button_enabled(
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    rect: Rect,
    label: &str,
    active: bool,
    active_colour: Color32,
    palette: &theme::Palette,
    id: Id,
    enabled: bool,
) -> bool {
    let response = ui.interact(
        rect,
        id,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let fill = if !enabled {
        palette.card
    } else if active {
        active_colour.gamma_multiply(0.3)
    } else {
        palette.card_hover
    };
    painter.rect_filled(rect, egui::CornerRadius::same(5), fill);
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(5),
        Stroke::new(
            1.0,
            if !enabled {
                palette.border
            } else if active {
                active_colour
            } else {
                palette.border
            },
        ),
        StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(9.0),
        if !enabled {
            palette.dim
        } else if active {
            active_colour
        } else {
            palette.text
        },
    );
    enabled && response.clicked()
}

fn link_button(
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    rect: Rect,
    active: bool,
    palette: &theme::Palette,
    id: Id,
) -> bool {
    let response = ui.interact(rect, id, Sense::click());
    let colour = if active { palette.green } else { palette.text };
    let fill = if active {
        palette.green.gamma_multiply(0.3)
    } else {
        palette.card_hover
    };
    painter.rect_filled(rect, egui::CornerRadius::same(5), fill);
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(5),
        Stroke::new(
            1.0,
            if active {
                palette.green
            } else {
                palette.border
            },
        ),
        StrokeKind::Inside,
    );
    let centre = rect.center();
    painter.circle_stroke(
        pos2(centre.x - 3.5, centre.y),
        3.5,
        Stroke::new(1.4, colour),
    );
    painter.circle_stroke(
        pos2(centre.x + 3.5, centre.y),
        3.5,
        Stroke::new(1.4, colour),
    );
    response.clicked()
}

fn split_button(
    painter: &egui::Painter,
    rect: Rect,
    label: &str,
    enabled: bool,
    palette: &theme::Palette,
) {
    let fill = if enabled {
        palette.card_hover
    } else {
        palette.card
    };
    let border = if enabled {
        palette.border
    } else {
        palette.border.gamma_multiply(0.4)
    };
    let text = if enabled {
        palette.text
    } else {
        palette.dim.gamma_multiply(0.6)
    };
    painter.rect_filled(rect, egui::CornerRadius::same(5), fill);
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(5),
        Stroke::new(1.0, border),
        StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(11.0),
        text,
    );
}

fn draw_tab(
    painter: &egui::Painter,
    tab: Rect,
    label: &str,
    font: &FontId,
    open: bool,
    palette: &theme::Palette,
) {
    let fill = Color32::from_rgb(0x2F, 0x37, 0x42);
    let text = Color32::from_rgb(205, 211, 220);
    let dim = Color32::from_rgb(120, 130, 143);
    let radius = egui::CornerRadius {
        nw: 8,
        ne: 8,
        sw: 0,
        se: 0,
    };
    painter.rect_filled(tab, radius, fill);
    for offset in [-4.0, 0.0, 4.0] {
        painter.line_segment(
            [
                pos2(tab.left() + 12.0, tab.center().y + offset),
                pos2(tab.left() + 20.0, tab.center().y + offset),
            ],
            Stroke::new(1.2, dim),
        );
    }
    painter.text(
        tab.center(),
        Align2::CENTER_CENTER,
        label,
        font.clone(),
        text,
    );
    let chevron = if open { 4.0 } else { -4.0 };
    painter.line_segment(
        [
            pos2(tab.right() - 18.0, tab.center().y - chevron),
            pos2(tab.right() - 13.0, tab.center().y + chevron),
        ],
        Stroke::new(1.5, palette.accent),
    );
    painter.line_segment(
        [
            pos2(tab.right() - 13.0, tab.center().y + chevron),
            pos2(tab.right() - 8.0, tab.center().y - chevron),
        ],
        Stroke::new(1.5, palette.accent),
    );
}
