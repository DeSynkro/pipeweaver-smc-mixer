use midir::{MidiInput, MidiInputConnection, MidiInputPort};
use pipeweaver_ipc::commands::{APICommand, DaemonRequest};
use pipeweaver_shared::Mix;
use pipeweaver_smc_core::config::{Config, MidiMode};
use pipeweaver_smc_core::engine::{ChannelKind, Engine};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::Sender;

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

fn named_ports(input: &MidiInput) -> Vec<(MidiInputPort, String)> {
    input
        .ports()
        .iter()
        .filter_map(|port| input.port_name(port).ok().map(|name| (port.clone(), name)))
        .collect()
}

fn choose_named(
    named: &[(MidiInputPort, String)],
    mode: MidiMode,
    wanted: &str,
) -> Option<(MidiInputPort, String)> {
    let pick = |predicate: &dyn Fn(&str) -> bool| {
        named
            .iter()
            .find(|(_, name)| predicate(name))
            .map(|(port, name)| (port.clone(), name.clone()))
    };

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

pub fn detect(config: &Config) -> Link {
    let Ok(input) = MidiInput::new("pipeweaver-smc-mixer") else {
        return Link::Offline;
    };
    let named = named_ports(&input);

    match choose_named(&named, config.midi_mode, &config.midi_device) {
        Some((_, name)) if is_bluetooth(&name) => Link::Bluetooth,
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
    let named = named_ports(&input);
    let (port, name) = choose_named(&named, config.midi_mode, &config.midi_device)?;
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

pub fn run_controls(config: Config, engine: Arc<Engine>, commands: Sender<DaemonRequest>) {
    let callback = move |message: &[u8]| handle_message(message, &engine, &commands);

    match open_control(&config, callback) {
        Some(port) => {
            log::info!("MIDI control connected: {} ({:?})", port.name, port.link);
            loop {
                std::thread::sleep(Duration::from_secs(3600));
            }
        }
        None => log::warn!("MIDI control: SMC-Mixer port not available"),
    }
}

fn handle_message(message: &[u8], engine: &Engine, commands: &Sender<DaemonRequest>) {
    if message.len() < 3 || message[0] & 0xF0 != 0xB0 {
        return;
    }

    let (index, mix) = match message[1] {
        40..=47 => ((message[1] - 40) as usize, Mix::A),
        30..=37 => ((message[1] - 30) as usize, Mix::B),
        _ => return,
    };

    let channel = engine.snapshot().channels.get(index).cloned();
    let Some(channel) = channel else {
        return;
    };
    if !channel.assigned {
        return;
    }

    let percent = ((message[2] as u16 * 100 + 63) / 127) as u8;
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
