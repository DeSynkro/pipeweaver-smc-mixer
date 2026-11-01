use midir::{
    MidiInput, MidiInputConnection, MidiInputPort, MidiOutput, MidiOutputConnection, MidiOutputPort,
};
use pipeweaver_ipc::commands::{APICommand, DaemonRequest};
use pipeweaver_shared::{Mix, MuteState, MuteTarget};
use pipeweaver_smc_core::config::{Config, MidiMode};
use pipeweaver_smc_core::engine::{ChannelKind, Engine, Snapshot};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::Sender;

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

pub fn run_controls(config: Config, engine: Arc<Engine>, commands: Sender<DaemonRequest>) {
    let callback_engine = engine.clone();
    let callback_commands = commands.clone();
    let mut pressed = [false; 128];

    let callback = move |message: &[u8]| {
        if message.len() < 3 || message[0] & 0xF0 != 0xB0 {
            return;
        }
        let control = message[1];
        let value = message[2];

        match control {
            30..=47 => handle_axis(control, value, &callback_engine, &callback_commands),
            64..=79 => {
                let down = value > 64;
                if pressed[control as usize] == down {
                    return;
                }
                pressed[control as usize] = down;
                if down {
                    handle_button(control, &callback_engine, &callback_commands);
                }
            }
            _ => {}
        }
    };

    let port = match open_control(&config, callback) {
        Some(port) => port,
        None => {
            log::warn!("MIDI control: SMC-Mixer port not available");
            return;
        }
    };

    let mut output = open_output(&config);
    match &output {
        Some(_) => log::info!("MIDI output connected"),
        None => log::warn!("MIDI output: SMC-Mixer port not available"),
    }
    log::info!("MIDI control connected: {} ({:?})", port.name, port.link);

    let mut leds = [u8::MAX; 32];
    loop {
        if let Some(output) = output.as_mut() {
            let snapshot = engine.snapshot();
            send_leds(output, &snapshot, &mut leds);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn handle_axis(control: u8, value: u8, engine: &Engine, commands: &Sender<DaemonRequest>) {
    let (index, mix) = match control {
        40..=47 => ((control - 40) as usize, Mix::A),
        30..=37 => ((control - 30) as usize, Mix::B),
        _ => return,
    };

    let channel = engine.snapshot().channels.get(index).cloned();
    let Some(channel) = channel else {
        return;
    };
    if !channel.assigned {
        return;
    }

    let percent = ((value as u16 * 100 + 63) / 127) as u8;
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

fn handle_button(control: u8, engine: &Engine, commands: &Sender<DaemonRequest>) {
    let index = match control {
        64..=71 => (control - 64) as usize,
        72..=79 => (control - 72) as usize,
        _ => return,
    };

    let channel = engine.snapshot().channels.get(index).cloned();
    let Some(channel) = channel else {
        return;
    };
    if !channel.assigned {
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
            let mix = if channel.mute_b { Mix::A } else { Mix::B };
            DaemonRequest::Pipewire(APICommand::SetTargetMix(channel.id, mix))
        }
        _ => return,
    };

    let _ = commands.try_send(request);
}

fn send_leds(output: &mut MidiOutputConnection, snapshot: &Snapshot, last: &mut [u8; 32]) {
    let mut desired = [LED_OFF; 32];
    for (index, channel) in snapshot.channels.iter().enumerate().take(8) {
        if !channel.assigned {
            continue;
        }
        desired[16 + index] = if channel.mute_a { LED_ON } else { LED_OFF };
        desired[8 + index] = if channel.mute_b { LED_ON } else { LED_OFF };
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
