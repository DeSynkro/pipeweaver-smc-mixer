use crate::media::MediaCommand;
use egui::ViewportCommand;
use midir::{
    MidiInput, MidiInputConnection, MidiInputPort, MidiOutput, MidiOutputConnection, MidiOutputPort,
};
use pipeweaver_ipc::commands::{APICommand, DaemonRequest};
use pipeweaver_shared::{Mix, MuteState, MuteTarget};
use pipeweaver_smc_core::config::{Config, MidiMode};
use pipeweaver_smc_core::engine::{ChannelKind, Engine, Snapshot};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
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

fn is_wired(name: &str) -> bool {
    name.contains("SMC-Mixer") && (name.contains("Private") || name.contains("SINCO"))
}

fn is_bluetooth(name: &str) -> bool {
    name.contains("SMC-Mixer") && (name.contains("Bluetooth") || name.contains("bluez"))
}

fn choose_index(names: &[String], mode: MidiMode, wanted: &str) -> Option<usize> {
    let pick = |predicate: &dyn Fn(&str) -> bool| names.iter().position(|name| predicate(name));

    match mode {
        MidiMode::Wired => pick(&is_wired),
        MidiMode::Bluetooth => pick(&is_bluetooth),
        MidiMode::Auto => pick(&is_wired)
            .or_else(|| pick(&is_bluetooth))
            .or_else(|| pick(&|name| name.contains("SMC-Mixer") && !name.contains("Master")))
            .or_else(|| {
                (!wanted.is_empty())
                    .then(|| pick(&|name| device_key(name) == wanted))
                    .flatten()
            }),
    }
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
    let output = MidiOutput::new("pipeweaver-smc-mixer").ok()?;
    let ports = output_ports(&output);
    let names: Vec<String> = ports.iter().map(|(_, name)| name.clone()).collect();
    let index = choose_index(&names, config.midi_mode, &config.midi_device)?;
    let (port, _) = ports[index].clone();
    output.connect(&port, "pipeweaver-smc-mixer").ok()
}

pub fn run_controls(
    config: Config,
    engine: Arc<Engine>,
    commands: Sender<DaemonRequest>,
    swap: Arc<AtomicU8>,
    pickup: Arc<AtomicBool>,
    media: UnboundedSender<MediaCommand>,
    ui_on: Arc<AtomicBool>,
    focus: Arc<AtomicBool>,
    record_hex: Arc<AtomicBool>,
    minimized: Arc<AtomicBool>,
    egui: Arc<Mutex<Option<egui::Context>>>,
    midi_link: Arc<AtomicU8>,
) {
    let config_path = Config::path();
    let mut config = config;

    loop {
        let callback_engine = engine.clone();
        let callback_commands = commands.clone();
        let callback_swap = swap.clone();
        let callback_pickup = pickup.clone();
        let callback_media = media.clone();
        let callback_ui_on = ui_on.clone();
        let callback_focus = focus.clone();
        let callback_record_hex = record_hex.clone();
        let callback_minimized = minimized.clone();
        let callback_egui = egui.clone();
        let mut pickup_state = PickupState::default();
        let mut pickup_was = false;
        let mut pressed = [false; 128];
        let mut hex_active = false;

        let callback = move |message: &[u8]| {
            if message.len() < 3 || message[0] & 0xF0 != 0xB0 {
                return;
            }
            let control = message[1];
            let value = message[2];
            let pickup_enabled = callback_pickup.load(Ordering::Relaxed);
            if pickup_enabled && !pickup_was {
                pickup_state.reset_all();
            }
            pickup_was = pickup_enabled;

            match control {
                30..=47 => handle_axis(
                    control,
                    value,
                    &callback_engine,
                    &callback_commands,
                    &callback_swap,
                    &mut pickup_state,
                    pickup_enabled,
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
                            &mut pickup_state,
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
        match &output {
            Some(_) => log::info!("MIDI output connected"),
            None => log::warn!("MIDI output: SMC-Mixer port not available"),
        }
        log::info!("MIDI control connected: {} ({:?})", port.name, port.link);

        let mut leds = [u8::MAX; 32];
        let mut last_check = Instant::now();
        let mut last_link = midi_link.load(Ordering::Relaxed);
        loop {
            if let Some(output) = output.as_mut() {
                let snapshot = engine.snapshot();
                send_leds(output, &snapshot, &swap, &mut leds);
            }
            std::thread::sleep(Duration::from_millis(50));
            if last_check.elapsed() >= Duration::from_secs(1) {
                last_check = Instant::now();
                let link = midi_link.load(Ordering::Relaxed);
                if link != last_link {
                    last_link = link;
                    break;
                }
                let latest = Config::load(&config_path);
                if latest.midi_mode != config.midi_mode || latest.midi_device != config.midi_device
                {
                    config = latest;
                    break;
                }
            }
        }
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

fn handle_axis(
    control: u8,
    value: u8,
    engine: &Engine,
    commands: &Sender<DaemonRequest>,
    swap: &AtomicU8,
    pickup: &mut PickupState,
    pickup_enabled: bool,
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
    let Some(percent) = pickup.filter(
        index,
        axis,
        value,
        target_percent,
        channel.id,
        pickup_enabled,
    ) else {
        return;
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
    pickup: &mut PickupState,
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
        pickup.reset_strip(index);
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
            pickup.reset_knob(index);
            DaemonRequest::Pipewire(APICommand::SetSourceVolumeLinked(
                channel.id,
                !channel.linked,
            ))
        }
        _ => return,
    };

    let _ = commands.try_send(request);
}

fn send_leds(
    output: &mut MidiOutputConnection,
    snapshot: &Snapshot,
    swap: &AtomicU8,
    last: &mut [u8; 32],
) {
    let swapped = swap.load(Ordering::Relaxed);
    let mut desired = [LED_OFF; 32];
    for (index, channel) in snapshot.channels.iter().enumerate().take(8) {
        if !channel.assigned {
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
            ChannelKind::Source if !channel.linked => LED_ON,
            _ => LED_OFF,
        };
    }

    for (note, value) in desired.iter().enumerate() {
        if last[note] != *value {
            let _ = output.send(&[0x90, note as u8, *value]);
            last[note] = *value;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::device_key;

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
}
