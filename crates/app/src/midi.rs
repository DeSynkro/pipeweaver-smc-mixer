use midir::MidiInput;
use pipeweaver_smc_core::config::{Config, MidiMode};

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

fn choose<'a>(names: &'a [String], mode: MidiMode, wanted: &str) -> Option<&'a str> {
    let pick = |predicate: &dyn Fn(&str) -> bool| {
        names
            .iter()
            .find(|name| predicate(name))
            .map(|name| name.as_str())
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

pub fn detect(config: &Config) -> bool {
    let Ok(input) = MidiInput::new("pipeweaver-smc-mixer") else {
        return false;
    };
    let names: Vec<String> = input
        .ports()
        .iter()
        .filter_map(|port| input.port_name(port).ok())
        .collect();

    choose(&names, config.midi_mode, &config.midi_device).is_some()
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
