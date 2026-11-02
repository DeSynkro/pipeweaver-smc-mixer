use crate::knob_mode::{self, KnobMode, MODE_RELATIVE, REQUEST_NONE};
use crate::media::MediaCommand;
use egui::ViewportCommand;
use midir::{
    MidiInput, MidiInputConnection, MidiInputPort, MidiOutput, MidiOutputConnection, MidiOutputPort,
};
use pipeweaver_ipc::commands::{APICommand, DaemonRequest};
use pipeweaver_shared::{Mix, MuteState, MuteTarget};
use pipeweaver_smc_core::config::{Config, MidiMode};
use pipeweaver_smc_core::engine::{ChannelKind, Engine, Snapshot};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{Sender, UnboundedSender};
use ulid::Ulid;

const LED_ON: u8 = 127;
const LED_OFF: u8 = 0;

/// ALSA appends a volatile `client:port` suffix, for example
/// `SINCO:SINCO SMC-Mixer-Private 40:0`. Compare only the stable part.
fn device_key(name: &str) -> &str {
    match name.rsplit_once(' ') {
        Some((base, suffix))
            if suffix.contains(':')
                && !suffix.is_empty()
                && suffix.chars().all(|c| c.is_ascii_digit() || c == ':') =>
        {
            base
        }
        _ => name,
    }
}

fn is_smc(name: &str) -> bool {
    name.contains("SMC-Mixer")
}

fn is_private(name: &str) -> bool {
    is_smc(name) && name.contains("Private")
}

fn is_master(name: &str) -> bool {
    is_smc(name) && name.contains("Master")
}

fn is_wired(name: &str) -> bool {
    is_smc(name) && (name.contains("Private") || name.contains("SINCO"))
}

fn is_bluetooth(name: &str) -> bool {
    is_smc(name) && (name.contains("Bluetooth") || name.contains("bluez"))
}

fn choose_index(names: &[String], mode: MidiMode, wanted: &str) -> Option<usize> {
    let pick = |predicate: &dyn Fn(&str) -> bool| names.iter().position(|name| predicate(name));

    match mode {
        MidiMode::Wired => pick(&is_private).or_else(|| pick(&is_wired)),
        MidiMode::Bluetooth => pick(&is_bluetooth),
        MidiMode::Auto => pick(&is_private)
            .or_else(|| pick(&is_wired))
            .or_else(|| pick(&is_bluetooth))
            .or_else(|| pick(&|name| is_smc(name) && !name.contains("Master")))
            .or_else(|| {
                (!wanted.is_empty())
                    .then(|| pick(&|name| device_key(name) == wanted))
                    .flatten()
            }),
    }
}

fn push_matching(candidates: &mut Vec<usize>, names: &[String], predicate: &dyn Fn(&str) -> bool) {
    for (index, name) in names.iter().enumerate() {
        if predicate(name) && !candidates.contains(&index) {
            candidates.push(index);
        }
    }
}

/// The SMC-Mixer exposes the control surface on `Private` and the LED/DAW
/// feedback side on `Master`. Try `Master` first for output, then fall back to
/// `Private` or any SMC port for devices that use one bidirectional port.
fn output_candidates(names: &[String], mode: MidiMode, wanted: &str) -> Vec<usize> {
    let mut candidates = Vec::new();

    match mode {
        MidiMode::Wired => {
            push_matching(&mut candidates, names, &|name| is_master(name));
            push_matching(&mut candidates, names, &|name| is_private(name));
            push_matching(&mut candidates, names, &|name| is_smc(name));
        }
        MidiMode::Bluetooth => {
            push_matching(&mut candidates, names, &|name| is_bluetooth(name));
            push_matching(&mut candidates, names, &|name| is_smc(name));
        }
        MidiMode::Auto => {
            push_matching(&mut candidates, names, &|name| is_master(name));
            push_matching(&mut candidates, names, &|name| is_private(name));
            push_matching(&mut candidates, names, &|name| is_bluetooth(name));
            push_matching(&mut candidates, names, &|name| {
                is_smc(name) && !name.contains("Private")
            });
            if !wanted.is_empty() {
                push_matching(&mut candidates, names, &|name| device_key(name) == wanted);
            }
        }
    }

    candidates
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    Offline,
    Wired,
    Bluetooth,
}

impl Link {
    pub const fn code(self) -> u8 {
        match self {
            Self::Offline => 0,
            Self::Wired => 1,
            Self::Bluetooth => 2,
        }
    }

    pub const fn from_code(code: u8) -> Self {
        match code {
            1 => Self::Wired,
            2 => Self::Bluetooth,
            _ => Self::Offline,
        }
    }
}

fn input_ports(input: &MidiInput) -> Vec<(MidiInputPort, String)> {
    input
        .ports()
        .iter()
        .filter_map(|port| input.port_name(port).ok().map(|name| (port.clone(), name)))
        .collect()
}

fn output_ports(output: &MidiOutput) -> Vec<(MidiOutputPort, String)> {
    output
        .ports()
        .iter()
        .filter_map(|port| output.port_name(port).ok().map(|name| (port.clone(), name)))
        .collect()
}

pub fn detect(config: &Config) -> Link {
    let Ok(input) = MidiInput::new("pipeweaver-smc-mixer") else {
        return Link::Offline;
    };
    let ports = input_ports(&input);
    let names: Vec<String> = ports.into_iter().map(|(_, name)| name).collect();

    match choose_index(&names, config.midi_mode, &config.midi_device) {
        Some(index) if is_bluetooth(&names[index]) => Link::Bluetooth,
        Some(_) => Link::Wired,
        None => Link::Offline,
    }
}

pub struct ControlPort {
    _connection: MidiInputConnection<()>,
    pub name: String,
    pub link: Link,
}

fn open_control<F>(config: &Config, mut callback: F) -> Option<ControlPort>
where
    F: FnMut(&[u8]) + Send + 'static,
{
    let input = MidiInput::new("pipeweaver-smc-mixer").ok()?;
    let ports = input_ports(&input);
    let names: Vec<String> = ports.iter().map(|(_, name)| name.clone()).collect();
    let index = choose_index(&names, config.midi_mode, &config.midi_device)?;
    let (port, name) = ports[index].clone();
    let link = if is_bluetooth(&name) {
        Link::Bluetooth
    } else {
        Link::Wired
    };

    let connection = input
        .connect(
            &port,
            "pipeweaver-smc-mixer",
            move |_stamp, message, _data| callback(message),
            (),
        )
        .ok()?;

    Some(ControlPort {
        _connection: connection,
        name,
        link,
    })
}

fn open_output(config: &Config) -> Option<MidiOutputConnection> {
    let probe = MidiOutput::new("pipeweaver-smc-mixer").ok()?;
    let ports = output_ports(&probe);
    let names: Vec<String> = ports.iter().map(|(_, name)| name.clone()).collect();
    let candidates = output_candidates(&names, config.midi_mode, &config.midi_device);
    drop(probe);

    for index in candidates {
        let wanted = names[index].clone();
        // `MidiOutput::connect` consumes the output, so each candidate needs a
        // fresh client. Match by name because the port handles belong to the
        // probe client that was just dropped.
        let output = MidiOutput::new("pipeweaver-smc-mixer").ok()?;
        let ports = output_ports(&output);
        let Some((port, name)) = ports.into_iter().find(|(_, name)| *name == wanted) else {
            continue;
        };
        match output.connect(&port, "pipeweaver-smc-mixer") {
            Ok(connection) => {
                log::info!("MIDI output connected: {name}");
                return Some(connection);
            }
            Err(error) => log::warn!("MIDI output {name} failed: {error}"),
        }
    }

    None
}

pub fn run_controls(
    config: Config,
    engine: Arc<Engine>,
    commands: Sender<DaemonRequest>,
    swap: Arc<AtomicU8>,
    pickup: Arc<AtomicBool>,
    knob_mode: Arc<KnobMode>,
    ui_reset: Arc<AtomicU8>,
    media: UnboundedSender<MediaCommand>,
    ui_on: Arc<AtomicBool>,
    focus: Arc<AtomicBool>,
    record_hex: Arc<AtomicBool>,
    minimized: Arc<AtomicBool>,
    egui: Arc<Mutex<Option<egui::Context>>>,
    midi_link: Arc<AtomicU8>,
    last_midi: Arc<AtomicU64>,
    led_test: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    startup_animation: Arc<AtomicBool>,
    animation_preview: Arc<AtomicBool>,
) {
    let config_path = Config::path();
    let mut config = config;
    let mut startup_played = false;

    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        // The control port is released between iterations, so the config
        // exchange never competes with the running surface.
        knob_mode::process(&config, &knob_mode);
        let callback_engine = engine.clone();
        let callback_commands = commands.clone();
        let callback_swap = swap.clone();
        let callback_pickup = pickup.clone();
        let callback_knob_mode = knob_mode.clone();
        let callback_ui_reset = ui_reset.clone();
        let callback_media = media.clone();
        let callback_ui_on = ui_on.clone();
        let callback_focus = focus.clone();
        let callback_record_hex = record_hex.clone();
        let callback_minimized = minimized.clone();
        let callback_egui = egui.clone();
        let callback_last_midi = last_midi.clone();
        let mut axis_state = AxisState::default();
        let mut pickup_was = false;
        let mut pressed = [false; 128];
        let mut hex_active = false;

        let callback = move |message: &[u8]| {
            if message.len() < 3 || message[0] & 0xF0 != 0xB0 {
                return;
            }
            callback_last_midi.store(now_millis(), Ordering::Relaxed);
            let reset = callback_ui_reset.swap(0, Ordering::Relaxed);
            if reset != 0 {
                for index in 0..8 {
                    if reset & (1 << index) != 0 {
                        axis_state.pickup.reset_strip(index);
                        axis_state.knobs.reset(index);
                    }
                }
            }
            let control = message[1];
            let value = message[2];
            let pickup_enabled = callback_pickup.load(Ordering::Relaxed);
            if pickup_enabled && !pickup_was {
                axis_state.pickup.reset_all();
            }
            pickup_was = pickup_enabled;
            let relative = callback_knob_mode.mode.load(Ordering::Relaxed) == MODE_RELATIVE;
            let up = callback_knob_mode.up.load(Ordering::Relaxed);
            let down = callback_knob_mode.down.load(Ordering::Relaxed);

            match control {
                30..=47 => handle_axis(
                    control,
                    value,
                    &callback_engine,
                    &callback_commands,
                    &callback_swap,
                    &mut axis_state,
                    AxisModes {
                        pickup: pickup_enabled,
                        relative,
                        up,
                        down,
                    },
                ),
                52..=62 => {
                    let down = value > 64;
                    if pressed[control as usize] == down {
                        return;
                    }
                    pressed[control as usize] = down;
                    handle_transport(
                        control,
                        down,
                        &callback_media,
                        &callback_engine,
                        &callback_ui_on,
                        &callback_focus,
                        &callback_record_hex,
                        &mut hex_active,
                        &callback_minimized,
                        &callback_egui,
                    );
                }
                64..=95 => {
                    let down = value > 64;
                    if pressed[control as usize] == down {
                        return;
                    }
                    pressed[control as usize] = down;
                    if down {
                        handle_button(
                            control,
                            &callback_engine,
                            &callback_commands,
                            &callback_swap,
                            &mut axis_state,
                        );
                    }
                }
                _ => {}
            }
        };

        let port = match open_control(&config, callback) {
            Some(port) => port,
            None => {
                log::warn!("MIDI control: SMC-Mixer port not available");
                std::thread::sleep(Duration::from_secs(1));
                config = Config::load(&config_path);
                continue;
            }
        };

        let mut output = open_output(&config);
        if output.is_none() {
            log::warn!("MIDI output: SMC-Mixer port not available");
        }
        log::info!("MIDI control connected: {} ({:?})", port.name, port.link);

        let mut leds = [u8::MAX; 32];
        let mut animation_active = !startup_played && startup_animation.load(Ordering::Relaxed);
        let mut animation_elapsed = 0u64;
        let mut test_frames = 0u8;
        let mut last_check = Instant::now();
        let mut last_link = midi_link.load(Ordering::Relaxed);
        loop {
            if stop.load(Ordering::Relaxed) {
                if let Some(output) = output.as_mut() {
                    blackout(output);
                }
                return;
            }
            if knob_mode.request.load(Ordering::Relaxed) != REQUEST_NONE {
                if let Some(output) = output.as_mut() {
                    blackout(output);
                }
                break;
            }
            if animation_preview.swap(false, Ordering::Relaxed) {
                animation_active = true;
                animation_elapsed = 0;
            }
            if animation_active {
                animation_elapsed += 50;
                if let Some(pattern) = startup_pattern(animation_elapsed) {
                    if let Some(output) = output.as_mut() {
                        send_pattern(output, &pattern, &mut leds);
                    }
                } else {
                    animation_active = false;
                    startup_played = true;
                }
            } else if led_test.swap(false, Ordering::Relaxed) {
                test_frames = 10;
            } else if test_frames > 0 {
                if let Some(output) = output.as_mut() {
                    send_pattern(output, &[LED_ON; 32], &mut leds);
                }
                test_frames -= 1;
            } else if let Some(output) = output.as_mut() {
                let snapshot = engine.snapshot();
                send_leds(output, &snapshot, &swap, &mut leds);
            }
            std::thread::sleep(Duration::from_millis(50));
            if last_check.elapsed() >= Duration::from_secs(1) {
                last_check = Instant::now();
                let link = midi_link.load(Ordering::Relaxed);
                if link != last_link {
                    last_link = link;
                    if let Some(output) = output.as_mut() {
                        blackout(output);
                    }
                    break;
                }
                let latest = Config::load(&config_path);
                if latest.midi_mode != config.midi_mode || latest.midi_device != config.midi_device
                {
                    config = latest;
                    if let Some(output) = output.as_mut() {
                        blackout(output);
                    }
                    break;
                }
            }
        }
    }
}

/// Turn every surface LED off before the output is dropped.
fn blackout(output: &mut MidiOutputConnection) {
    for note in 0..32 {
        let _ = output.send(&[0x90, note, 0]);
    }
}

fn handle_transport(
    control: u8,
    down: bool,
    media: &UnboundedSender<MediaCommand>,
    engine: &Arc<Engine>,
    ui_on: &Arc<AtomicBool>,
    focus: &Arc<AtomicBool>,
    record_hex: &Arc<AtomicBool>,
    hex_active: &mut bool,
    minimized: &Arc<AtomicBool>,
    egui: &Arc<Mutex<Option<egui::Context>>>,
) {
    let hex = record_hex.load(Ordering::Relaxed);
    if !down {
        if control == 54 && *hex_active {
            *hex_active = false;
            let _ = media.send(MediaCommand::HexStop);
        }
        return;
    }
    match control {
        52 => {
            let _ = media.send(MediaCommand::Play);
        }
        53 => {
            let _ = media.send(MediaCommand::Pause);
        }
        54 => {
            if hex {
                *hex_active = true;
                let _ = media.send(MediaCommand::HexStart);
            } else {
                let _ = media.send(MediaCommand::ToggleMicMute);
            }
        }
        55 => {
            let _ = media.send(MediaCommand::SeekBack);
        }
        56 => {
            let _ = media.send(MediaCommand::SeekForward);
        }
        57 => {
            let _ = media.send(MediaCommand::Previous);
        }
        58 => {
            let _ = media.send(MediaCommand::Next);
        }
        59 => engine.set_source_slots(engine.source_slots().saturating_sub(1)),
        60 => engine.set_source_slots(engine.source_slots() + 1),
        61 => {
            if minimized.swap(false, Ordering::Relaxed) {
                if let Some(ctx) = egui.lock().unwrap().as_ref() {
                    ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(ViewportCommand::Focus);
                }
                ui_on.store(true, Ordering::Relaxed);
                focus.store(true, Ordering::Relaxed);
            } else {
                let was_on = ui_on.fetch_xor(true, Ordering::Relaxed);
                if !was_on {
                    focus.store(true, Ordering::Relaxed);
                }
            }
        }
        62 => {
            let _ = media.send(MediaCommand::OpenPipeweaver);
        }
        _ => {}
    }
}

#[derive(Clone, Copy)]
struct AxisPickup {
    id: Ulid,
    initialized: bool,
    caught: bool,
    last_raw: u8,
}

impl Default for AxisPickup {
    fn default() -> Self {
        Self {
            id: Ulid::nil(),
            initialized: false,
            caught: false,
            last_raw: 0,
        }
    }
}

#[derive(Default)]
struct PickupState {
    axes: [[AxisPickup; 2]; 8],
}

impl PickupState {
    fn reset_all(&mut self) {
        *self = Self::default();
    }

    fn reset_strip(&mut self, index: usize) {
        self.axes[index] = [AxisPickup::default(); 2];
    }

    fn reset_knob(&mut self, index: usize) {
        self.axes[index][1] = AxisPickup::default();
    }

    fn filter(
        &mut self,
        index: usize,
        axis: usize,
        raw: u8,
        target_percent: u8,
        id: Ulid,
        enabled: bool,
    ) -> Option<u8> {
        let percent = ((raw as u16 * 100 + 63) / 127) as u8;
        if !enabled {
            return Some(percent);
        }

        let state = &mut self.axes[index][axis];
        if state.id != id {
            *state = AxisPickup {
                id,
                ..Default::default()
            };
        }

        let target_raw = ((target_percent as u16 * 127 + 50) / 100) as u8;
        if !state.initialized {
            state.initialized = true;
            state.last_raw = raw;
            if raw.abs_diff(target_raw) <= 2 {
                state.caught = true;
                return Some(target_percent);
            }
            return None;
        }

        if !state.caught {
            let crossed = (state.last_raw < target_raw && raw >= target_raw)
                || (state.last_raw > target_raw && raw <= target_raw);
            if crossed || raw.abs_diff(target_raw) <= 2 {
                state.caught = true;
                state.last_raw = raw;
                return Some(target_percent);
            }
            state.last_raw = raw;
            return None;
        }

        if raw.abs_diff(target_raw) > 10 {
            state.caught = false;
            state.last_raw = raw;
            return None;
        }

        state.last_raw = raw;
        Some(percent)
    }
}

/// How long the knob must be idle before it re-reads the value Pipeweaver
/// reports. During a continuous turn the bank keeps its own accumulator so a
/// lagging round trip cannot drag the value backwards.
const KNOB_RESYNC_MS: u64 = 500;

/// Decode one relative CC step. The mixer sends `max` for clockwise and
/// `min` for counter-clockwise; with the factory pair that is 1 up and 65
/// down.
fn relative_delta(value: u8, up: u8, down: u8) -> i16 {
    if up != down {
        if value == up {
            return 1;
        }
        if value == down {
            return -1;
        }
    }
    if value > 64 { 1 } else { -1 }
}

#[derive(Clone, Copy, Default)]
struct KnobState {
    percent: f32,
    seeded: bool,
    last_event_ms: u64,
}

/// Accumulator for relative knob steps. Each detent is one step; the mixer
/// never reports a position, so there is no wall to hit.
#[derive(Default)]
struct KnobBank {
    knobs: [KnobState; 8],
}

impl KnobBank {
    fn reset(&mut self, index: usize) {
        self.knobs[index] = KnobState::default();
    }

    fn adjust_relative(
        &mut self,
        index: usize,
        value: u8,
        base_percent: u8,
        now_ms: u64,
        up: u8,
        down: u8,
    ) -> Option<u8> {
        let state = &mut self.knobs[index];
        let idle = now_ms.saturating_sub(state.last_event_ms);
        if !state.seeded || idle > KNOB_RESYNC_MS {
            state.percent = f32::from(base_percent);
            state.seeded = true;
        }
        state.last_event_ms = now_ms;

        let delta = relative_delta(value, up, down);
        let before = state.percent;
        state.percent = (state.percent + f32::from(delta)).clamp(0.0, 100.0);
        if (state.percent - before).abs() < 0.5 {
            return None;
        }
        Some(state.percent.round() as u8)
    }
}

#[derive(Default)]
struct AxisState {
    pickup: PickupState,
    knobs: KnobBank,
}

#[derive(Clone, Copy, Default)]
struct AxisModes {
    pickup: bool,
    relative: bool,
    up: u8,
    down: u8,
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn handle_axis(
    control: u8,
    value: u8,
    engine: &Engine,
    commands: &Sender<DaemonRequest>,
    swap: &AtomicU8,
    state: &mut AxisState,
    modes: AxisModes,
) {
    let (index, is_knob) = match control {
        40..=47 => ((control - 40) as usize, false),
        30..=37 => ((control - 30) as usize, true),
        _ => return,
    };

    let channel = engine.snapshot().channels.get(index).cloned();
    let Some(channel) = channel else {
        return;
    };
    if !channel.assigned {
        return;
    }
    if channel.kind == ChannelKind::Target && is_knob {
        return;
    }

    let (target_percent, mix) = match channel.kind {
        ChannelKind::Source => {
            if is_knob && channel.linked {
                return;
            }
            let swapped = swap.load(Ordering::Relaxed) & (1 << index) != 0;
            match (is_knob, swapped) {
                (false, false) => (channel.fader, Mix::A),
                (false, true) => (channel.knob, Mix::B),
                (true, false) => (channel.knob, Mix::B),
                (true, true) => (channel.fader, Mix::A),
            }
        }
        ChannelKind::Target => (channel.fader, Mix::A),
    };

    let axis = usize::from(is_knob);
    let percent = if is_knob && modes.relative {
        let Some(percent) = state.knobs.adjust_relative(
            index,
            value,
            target_percent,
            now_millis(),
            modes.up,
            modes.down,
        ) else {
            return;
        };
        percent
    } else {
        let Some(percent) =
            state
                .pickup
                .filter(index, axis, value, target_percent, channel.id, modes.pickup)
        else {
            return;
        };
        percent
    };

    let request = match channel.kind {
        ChannelKind::Source => {
            DaemonRequest::Pipewire(APICommand::SetSourceVolume(channel.id, mix, percent))
        }
        ChannelKind::Target => {
            DaemonRequest::Pipewire(APICommand::SetTargetVolume(channel.id, percent))
        }
    };
    let _ = commands.try_send(request);
}

fn handle_button(
    control: u8,
    engine: &Engine,
    commands: &Sender<DaemonRequest>,
    swap: &AtomicU8,
    state: &mut AxisState,
) {
    let index = match control {
        64..=71 => (control - 64) as usize,
        72..=79 => (control - 72) as usize,
        80..=87 => (control - 80) as usize,
        88..=95 => (control - 88) as usize,
        _ => return,
    };

    let channel = engine.snapshot().channels.get(index).cloned();
    let Some(channel) = channel else {
        return;
    };
    if !channel.assigned {
        return;
    }

    if control / 8 == 10 && channel.kind == ChannelKind::Source {
        swap.fetch_xor(1 << index, Ordering::Relaxed);
        state.pickup.reset_strip(index);
        state.knobs.reset(index);
        return;
    }

    let request = match (control / 8, channel.kind) {
        (8, ChannelKind::Source) => {
            let command = if channel.mute_a {
                APICommand::DelSourceMuteTarget(channel.id, MuteTarget::TargetA)
            } else {
                APICommand::AddSourceMuteTarget(channel.id, MuteTarget::TargetA)
            };
            DaemonRequest::Pipewire(command)
        }
        (8, ChannelKind::Target) => {
            let state = if channel.mute_a {
                MuteState::Unmuted
            } else {
                MuteState::Muted
            };
            DaemonRequest::Pipewire(APICommand::SetTargetMuteState(channel.id, state))
        }
        (9, ChannelKind::Source) => {
            let command = if channel.mute_b {
                APICommand::DelSourceMuteTarget(channel.id, MuteTarget::TargetB)
            } else {
                APICommand::AddSourceMuteTarget(channel.id, MuteTarget::TargetB)
            };
            DaemonRequest::Pipewire(command)
        }
        (9, ChannelKind::Target) => {
            if !channel.mix_b {
                return;
            }
            DaemonRequest::Pipewire(APICommand::SetTargetMix(channel.id, Mix::A))
        }
        (10, ChannelKind::Target) => {
            if channel.mix_b {
                return;
            }
            DaemonRequest::Pipewire(APICommand::SetTargetMix(channel.id, Mix::B))
        }
        (11, ChannelKind::Source) => {
            state.pickup.reset_knob(index);
            state.knobs.reset(index);
            DaemonRequest::Pipewire(APICommand::SetSourceVolumeLinked(
                channel.id,
                !channel.linked,
            ))
        }
        _ => return,
    };

    let _ = commands.try_send(request);
}

fn led_pattern(snapshot: &Snapshot, swapped: u8) -> [u8; 32] {
    let mut desired = [LED_OFF; 32];
    for (index, channel) in snapshot.channels.iter().enumerate().take(8) {
        if !snapshot.connected || !channel.assigned {
            continue;
        }
        desired[16 + index] = if channel.mute_a { LED_ON } else { LED_OFF };
        desired[8 + index] = match channel.kind {
            ChannelKind::Source if channel.mute_b => LED_ON,
            ChannelKind::Target if !channel.mix_b => LED_ON,
            _ => LED_OFF,
        };
        desired[index] = match channel.kind {
            ChannelKind::Source if swapped & (1 << index) != 0 => LED_ON,
            ChannelKind::Target if channel.mix_b => LED_ON,
            _ => LED_OFF,
        };
        desired[24 + index] = match channel.kind {
            ChannelKind::Source if channel.linked => LED_ON,
            _ => LED_OFF,
        };
    }
    desired
}

fn send_leds(
    output: &mut MidiOutputConnection,
    snapshot: &Snapshot,
    swap: &AtomicU8,
    last: &mut [u8; 32],
) {
    let desired = led_pattern(snapshot, swap.load(Ordering::Relaxed));
    send_pattern(output, &desired, last);
}

fn send_pattern(output: &mut MidiOutputConnection, desired: &[u8; 32], last: &mut [u8; 32]) {
    for (note, value) in desired.iter().enumerate() {
        if last[note] != *value {
            let _ = output.send(&[0x90, note as u8, *value]);
            last[note] = *value;
        }
    }
}

fn startup_pattern(elapsed_ms: u64) -> Option<[u8; 32]> {
    let mut pattern = [LED_OFF; 32];
    const COLUMN_MS: u64 = 100;
    const COLUMNS: u64 = 8;
    let fill = COLUMN_MS * COLUMNS;

    if elapsed_ms < fill {
        let columns = (elapsed_ms / COLUMN_MS + 1).min(COLUMNS) as usize;
        for column in 0..columns {
            for row in [0, 8, 16, 24] {
                pattern[row + column] = LED_ON;
            }
        }
        return Some(pattern);
    }

    match elapsed_ms - fill {
        0..=149 => pattern.fill(LED_ON),
        150..=249 => {}
        250..=399 => pattern.fill(LED_ON),
        400..=499 => {}
        _ => return None,
    }
    Some(pattern)
}

#[cfg(test)]
mod tests {
    use super::{
        KnobBank, LED_OFF, LED_ON, PickupState, choose_index, device_key, led_pattern,
        output_candidates, relative_delta,
    };
    use pipeweaver_smc_core::config::MidiMode;
    use pipeweaver_smc_core::engine::Snapshot;
    use ulid::Ulid;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn strips_alsa_suffix() {
        assert_eq!(
            device_key("SINCO:SINCO SMC-Mixer-Private 40:0"),
            "SINCO:SINCO SMC-Mixer-Private"
        );
        assert_eq!(
            device_key("SMC-Mixer:SMC-Mixer Bluetooth 128:1"),
            "SMC-Mixer:SMC-Mixer Bluetooth"
        );
    }

    #[test]
    fn input_prefers_private_and_output_tries_master_first() {
        let ports = names(&[
            "SINCO:SINCO SMC-Mixer-Master 44:1",
            "SINCO:SINCO SMC-Mixer-Private 44:0",
        ]);
        assert_eq!(choose_index(&ports, MidiMode::Auto, ""), Some(1));
        assert_eq!(output_candidates(&ports, MidiMode::Auto, ""), vec![0, 1]);
    }

    #[test]
    fn output_falls_back_to_a_single_bidirectional_port() {
        let ports = names(&["SINCO:SINCO SMC-Mixer 44:0"]);
        assert_eq!(output_candidates(&ports, MidiMode::Wired, ""), vec![0]);
    }

    #[test]
    fn leds_go_dark_when_pipeweaver_disconnects() {
        let mut snapshot = Snapshot::default();
        snapshot.connected = true;
        snapshot.channels[0].assigned = true;
        snapshot.channels[0].mute_a = true;
        snapshot.channels[0].linked = false;

        let live = led_pattern(&snapshot, 0);
        assert_eq!(live[16], LED_ON);
        assert_eq!(live[24], LED_OFF);

        snapshot.channels[0].linked = true;
        assert_eq!(led_pattern(&snapshot, 0)[24], LED_ON);

        snapshot.connected = false;
        assert_eq!(led_pattern(&snapshot, 0), [LED_OFF; 32]);
    }

    #[test]
    fn relative_steps_follow_the_configured_values() {
        assert_eq!(relative_delta(65, 65, 1), 1);
        assert_eq!(relative_delta(1, 65, 1), -1);
        assert_eq!(relative_delta(127, 0, 127), -1);
        assert_eq!(relative_delta(0, 0, 127), 1);
        assert_eq!(relative_delta(66, 65, 1), 1);
        assert_eq!(relative_delta(2, 65, 1), -1);
    }

    #[test]
    fn relative_knobs_step_from_the_current_value() {
        let mut bank = KnobBank::default();
        assert_eq!(bank.adjust_relative(0, 65, 50, 1_000, 65, 1), Some(51));
        assert_eq!(bank.adjust_relative(0, 1, 50, 1_050, 65, 1), Some(50));
        assert_eq!(bank.adjust_relative(0, 1, 50, 1_100, 65, 1), Some(49));
    }

    #[test]
    fn relative_knobs_clamp_at_the_limits() {
        let mut bank = KnobBank::default();
        assert_eq!(bank.adjust_relative(0, 65, 100, 1_000, 65, 1), None);
        // Long idle reseeds from the on-screen value before the step.
        assert_eq!(bank.adjust_relative(0, 1, 0, 2_000, 65, 1), None);
    }

    #[test]
    fn resetting_a_knob_clears_its_accumulator() {
        let mut bank = KnobBank::default();
        assert_eq!(bank.adjust_relative(0, 65, 50, 1_000, 65, 1), Some(51));
        bank.reset(0);
        assert_eq!(bank.adjust_relative(0, 65, 80, 1_100, 65, 1), Some(81));
    }

    #[test]
    fn resetting_a_strip_requires_takeover_again() {
        let id = Ulid::new();
        let mut pickup = PickupState::default();
        assert_eq!(pickup.filter(0, 0, 0, 50, id, true), None);
        assert_eq!(pickup.filter(0, 0, 64, 50, id, true), Some(50));
        assert_eq!(pickup.filter(0, 0, 58, 50, id, true), Some(46));

        pickup.reset_strip(0);
        assert_eq!(pickup.filter(0, 0, 58, 50, id, true), None);
    }

    #[test]
    fn relative_knobs_resync_after_a_pause() {
        let mut bank = KnobBank::default();
        assert_eq!(bank.adjust_relative(0, 65, 50, 1_000, 65, 1), Some(51));
        assert_eq!(bank.adjust_relative(0, 65, 30, 3_000, 65, 1), Some(31));
    }
}
