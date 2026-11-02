//! SMC-Mixer knob mode: read the active configuration over SysEx and switch
//! the preset used by the app between absolute and relative knob reporting.
//!
//! The mixer stores two 3141-byte presets in block 5. Each preset has eight
//! 6-byte knob records at offset 3053: `type, curve, channel, controller,
//! min, max`. Type 0 is absolute CC; type 3 is relative CC, where the mixer
//! sends `min` in one direction and `max` in the other.
//!
//! We only ever read (`0x23`) and write (`0x22`). Erase (`0x21`) is never
//! built. Writes update a RAM buffer and only become persistent after a
//! zero-length write (the mixer's "save config").

use anyhow::{Context, Result, anyhow, bail};
use midir::{MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use pipeweaver_smc_core::config::{Config, MidiMode, state_dir};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const HEADER: [u8; 2] = [0x00, 0x59];
const MSG_ACK: u8 = 0x00;
const MSG_NAME: u8 = 0x11;
const MSG_WRITE: u8 = 0x22;
const MSG_READ: u8 = 0x23;

const BLOCK_GLOBAL: u8 = 4;
const BLOCK_USER: u8 = 5;
const GLOBAL_SIZE: usize = 128;
const USER_SIZE: usize = 6282;
const PRESET_SIZE: usize = 3141;
const KNOB_OFFSET: usize = 3053;
const KNOB_COUNT: usize = 8;
const KNOB_SIZE: usize = 6;
const CC_FIRST: u8 = 30;

/// Direction values stored while relative mode is on. The factory DAW preset
/// uses the same pair (min 65, max 1).
const RELATIVE_UP: u8 = 65;
const RELATIVE_DOWN: u8 = 1;

pub const MODE_UNKNOWN: u8 = 0;
pub const MODE_ABSOLUTE: u8 = 1;
pub const MODE_RELATIVE: u8 = 2;

pub const REQUEST_NONE: u8 = 0;
pub const REQUEST_DETECT: u8 = 1;
pub const REQUEST_ENABLE: u8 = 2;
pub const REQUEST_DISABLE: u8 = 3;
pub const REQUEST_RESTORE: u8 = 4;

#[derive(Clone, Debug, Default)]
pub struct KnobDetail {
    pub backup: Option<PathBuf>,
    pub error: Option<String>,
    pub note: Option<String>,
}

/// State shared between the UI and the MIDI thread.
pub struct KnobMode {
    pub mode: AtomicU8,
    pub busy: AtomicBool,
    pub request: AtomicU8,
    pub up: AtomicU8,
    pub down: AtomicU8,
    detail: Mutex<KnobDetail>,
}

impl Default for KnobMode {
    fn default() -> Self {
        Self {
            mode: AtomicU8::new(MODE_UNKNOWN),
            busy: AtomicBool::new(false),
            request: AtomicU8::new(REQUEST_NONE),
            up: AtomicU8::new(RELATIVE_UP),
            down: AtomicU8::new(RELATIVE_DOWN),
            detail: Mutex::new(KnobDetail::default()),
        }
    }
}

impl KnobMode {
    pub fn detail(&self) -> KnobDetail {
        self.detail
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn apply(&self, outcome: Outcome, request: u8) {
        let old = self.mode.load(Ordering::Relaxed);
        let new = match outcome.detection.as_ref() {
            Some(detection) if detection.found => {
                if detection.relative {
                    MODE_RELATIVE
                } else {
                    MODE_ABSOLUTE
                }
            }
            _ => MODE_UNKNOWN,
        };

        self.up.store(
            outcome
                .detection
                .as_ref()
                .map(|detection| detection.up)
                .unwrap_or(RELATIVE_UP),
            Ordering::Relaxed,
        );
        self.down.store(
            outcome
                .detection
                .as_ref()
                .map(|detection| detection.down)
                .unwrap_or(RELATIVE_DOWN),
            Ordering::Relaxed,
        );
        self.mode.store(new, Ordering::Relaxed);

        let mut detail = self
            .detail
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *detail = outcome.detail;
        if request == REQUEST_DETECT && old != MODE_UNKNOWN && new != MODE_UNKNOWN && old != new {
            detail.note = Some(String::from(
                "The mixer's knob mode changed since it was last read.",
            ));
        }
        if let Some(detection) = outcome.detection
            && !detection.found
            && detail.note.is_none()
        {
            detail.note = Some(String::from(
                "The knob mapping used by the app (CC 30-37) was not found on the mixer.",
            ));
        }
    }
}

struct Detection {
    found: bool,
    relative: bool,
    up: u8,
    down: u8,
}

struct Outcome {
    detection: Option<Detection>,
    detail: KnobDetail,
}

/// Handle one pending request, or refresh detection when there is none.
/// Called by the MIDI thread while no control connection is open.
pub fn process(config: &Config, state: &KnobMode) {
    let request = state.request.swap(REQUEST_NONE, Ordering::Relaxed);
    state.busy.store(true, Ordering::Relaxed);

    let outcome = match request {
        REQUEST_ENABLE => set_mode(config, true),
        REQUEST_DISABLE => set_mode(config, false),
        REQUEST_RESTORE => restore_last(config, state),
        _ => detect(config),
    };

    let request = if request == REQUEST_NONE {
        REQUEST_DETECT
    } else {
        request
    };
    match outcome {
        Ok(outcome) => state.apply(outcome, request),
        Err(error) => {
            let message = format!("{error:#}");
            // The operation failed, but the mixer may still be there. Show its
            // real mode and keep the error visible.
            match detect(config) {
                Ok(mut outcome) => {
                    outcome.detail.error = Some(message);
                    state.apply(outcome, request);
                }
                Err(_) => {
                    let previous = state.detail();
                    state.apply(
                        Outcome {
                            detection: None,
                            detail: KnobDetail {
                                error: Some(message),
                                backup: previous.backup,
                                ..Default::default()
                            },
                        },
                        request,
                    );
                }
            }
        }
    }
    state.busy.store(false, Ordering::Relaxed);
}

fn detect(config: &Config) -> Result<Outcome> {
    let mut link = ConfigLink::open(config)?;
    let detection = detect_with(&mut link)?;
    Ok(Outcome {
        detail: KnobDetail::default(),
        detection: Some(detection),
    })
}

fn detect_with(link: &mut ConfigLink) -> Result<Detection> {
    let globals = link.read(BLOCK_GLOBAL, 0, GLOBAL_SIZE)?;
    let active = usize::from(globals[0]).min(1);

    let mut records = [[0u8; KNOB_COUNT * KNOB_SIZE]; 2];
    for (preset, chunk) in records.iter_mut().enumerate() {
        let data = link.read(
            BLOCK_USER,
            (preset * PRESET_SIZE + KNOB_OFFSET) as u32,
            KNOB_COUNT * KNOB_SIZE,
        )?;
        chunk.copy_from_slice(&data);
    }

    let preset = find_cc_preset_bytes(&records, active);
    let Some(preset) = preset else {
        return Ok(Detection {
            found: false,
            relative: false,
            up: RELATIVE_UP,
            down: RELATIVE_DOWN,
        });
    };

    let chunk = &records[preset];
    let all_relative = (0..KNOB_COUNT).all(|index| chunk[index * KNOB_SIZE] == 3);
    let (up, down) = if all_relative {
        // Measured on hardware: the `max` value is clockwise, `min` is
        // counter-clockwise. The factory pair is min 65, max 1.
        relative_values(chunk[RELATIVE_MIN], chunk[RELATIVE_MAX])
    } else {
        (RELATIVE_UP, RELATIVE_DOWN)
    };

    Ok(Detection {
        found: true,
        relative: all_relative,
        up,
        down,
    })
}

const RELATIVE_MIN: usize = 4;
const RELATIVE_MAX: usize = 5;

/// The device sends `max` clockwise and `min` counter-clockwise.
fn relative_values(min: u8, max: u8) -> (u8, u8) {
    (max, min)
}

fn set_mode(config: &Config, relative: bool) -> Result<Outcome> {
    let mut link = ConfigLink::open(config)?;
    let device = link.name()?;
    let globals = link.read(BLOCK_GLOBAL, 0, GLOBAL_SIZE)?;
    let active = usize::from(globals[0]).min(1);
    let mut user = link.read(BLOCK_USER, 0, USER_SIZE)?;
    let preset = find_cc_preset_bytes(&knob_regions(&user), active)
        .ok_or_else(|| anyhow!("the knob mapping used by the app (CC 30-37) was not found"))?;

    let backup = write_backup(&device, active, &globals, &user)?;

    let base = preset * PRESET_SIZE + KNOB_OFFSET;
    patch_records(&mut user[base..base + KNOB_COUNT * KNOB_SIZE], relative);

    link.write(BLOCK_USER, 0, &user)?;
    link.commit(BLOCK_USER)?;

    let check = link.read(BLOCK_USER, (base) as u32, KNOB_COUNT * KNOB_SIZE)?;
    let expected = if relative { 3 } else { 0 };
    for index in 0..KNOB_COUNT {
        if check[index * KNOB_SIZE] != expected {
            bail!("the mixer did not keep the new knob mode");
        }
    }

    let after = detect_with(&mut link)?;
    Ok(Outcome {
        detail: KnobDetail {
            backup: Some(backup),
            ..Default::default()
        },
        detection: Some(after),
    })
}

fn restore_last(config: &Config, state: &KnobMode) -> Result<Outcome> {
    let path = state
        .detail()
        .backup
        .ok_or_else(|| anyhow!("no mixer backup has been saved yet"))?;
    let backup: MixerBackup = serde_json::from_str(
        &fs::read_to_string(&path).with_context(|| format!("read backup {}", path.display()))?,
    )
    .with_context(|| format!("parse backup {}", path.display()))?;
    let user = decode_hex(&backup.user).context("backup user block is not valid hex")?;
    if user.len() != USER_SIZE {
        bail!("backup user block has the wrong size");
    }

    let mut link = ConfigLink::open(config)?;
    link.write(BLOCK_USER, 0, &user)?;
    link.commit(BLOCK_USER)?;

    let after = detect_with(&mut link)?;
    Ok(Outcome {
        detail: KnobDetail {
            backup: Some(path),
            note: Some(String::from("Restored the saved mixer configuration.")),
            ..Default::default()
        },
        detection: Some(after),
    })
}

#[derive(Serialize, Deserialize)]
struct MixerBackup {
    format: String,
    version: u32,
    device: String,
    created_unix: u64,
    active_preset: u8,
    globals: String,
    user: String,
}

fn write_backup(device: &str, active: usize, globals: &[u8], user: &[u8]) -> Result<PathBuf> {
    let dir = state_dir().join("mixer-backups");
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let safe: String = device
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect();
    let path = dir.join(format!("{safe}-{}.json", unix_now()));
    let backup = MixerBackup {
        format: String::from("pipeweaver-smc-mixer/mixer-backup"),
        version: 1,
        device: device.to_string(),
        created_unix: unix_now(),
        active_preset: active as u8,
        globals: encode_hex(globals),
        user: encode_hex(user),
    };
    fs::write(&path, serde_json::to_string_pretty(&backup)?)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

fn knob_regions(user: &[u8]) -> [[u8; KNOB_COUNT * KNOB_SIZE]; 2] {
    let mut regions = [[0u8; KNOB_COUNT * KNOB_SIZE]; 2];
    for (preset, chunk) in regions.iter_mut().enumerate() {
        let base = preset * PRESET_SIZE + KNOB_OFFSET;
        if let Some(data) = user.get(base..base + KNOB_COUNT * KNOB_SIZE) {
            chunk.copy_from_slice(data);
        }
    }
    regions
}

fn find_cc_preset_bytes(
    records: &[[u8; KNOB_COUNT * KNOB_SIZE]; 2],
    active: usize,
) -> Option<usize> {
    let matches = |preset: usize| {
        (0..KNOB_COUNT)
            .all(|index| records[preset][index * KNOB_SIZE + 3] == CC_FIRST + index as u8)
    };
    if matches(active) {
        Some(active)
    } else if matches(0) {
        Some(0)
    } else if matches(1) {
        Some(1)
    } else {
        None
    }
}

fn patch_records(records: &mut [u8], relative: bool) {
    for index in 0..KNOB_COUNT {
        let record = &mut records[index * KNOB_SIZE..(index + 1) * KNOB_SIZE];
        record[0] = if relative { 3 } else { 0 };
        record[4] = if relative { RELATIVE_UP } else { 0 };
        record[5] = if relative { RELATIVE_DOWN } else { 127 };
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn encode_hex(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len() * 2);
    for byte in data {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn decode_hex(text: &str) -> Result<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        bail!("odd number of hex digits");
    }
    let mut out = Vec::with_capacity(text.len() / 2);
    for pair in text.as_bytes().chunks(2) {
        let digit = |byte: u8| -> Result<u8> {
            match byte {
                b'0'..=b'9' => Ok(byte - b'0'),
                b'a'..=b'f' => Ok(byte - b'a' + 10),
                b'A'..=b'F' => Ok(byte - b'A' + 10),
                _ => bail!("invalid hex digit"),
            }
        };
        out.push((digit(pair[0])? << 4) | digit(pair[1])?);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

struct ConfigLink {
    _input: MidiInputConnection<()>,
    output: MidiOutputConnection,
    replies: mpsc::Receiver<Vec<u8>>,
    port_name: String,
}

impl ConfigLink {
    fn open(config: &Config) -> Result<Self> {
        if config.midi_mode == MidiMode::Bluetooth {
            bail!("changing the mixer configuration over Bluetooth is not supported yet");
        }
        let input = MidiInput::new("pipeweaver-smc-mixer config")?;
        let ports = input.ports();
        let names: Vec<String> = ports
            .iter()
            .filter_map(|port| input.port_name(port).ok())
            .collect();
        let index = choose_config_port(&names)
            .ok_or_else(|| anyhow!("no SMC-Mixer configuration port found"))?;
        let port = ports[index].clone();
        let port_name = names[index].clone();

        let (sender, replies) = mpsc::channel();
        let connection = input
            .connect(
                &port,
                "pipeweaver-smc-mixer config",
                move |_stamp, message, _data| {
                    if message.first() == Some(&0xf0) && message.last() == Some(&0xf7) {
                        let _ = sender.send(message.to_vec());
                    }
                },
                (),
            )
            .map_err(|_| anyhow!("could not listen on {}", names[index]))?;

        let output = MidiOutput::new("pipeweaver-smc-mixer config")?;
        let out_ports = output.ports();
        let out_names: Vec<String> = out_ports
            .iter()
            .filter_map(|port| output.port_name(port).ok())
            .collect();
        let out_index = out_names
            .iter()
            .position(|name| *name == port_name)
            .or_else(|| {
                out_names
                    .iter()
                    .position(|name| name.contains("SMC-Mixer") && !name.contains("Master"))
            })
            .ok_or_else(|| anyhow!("no SMC-Mixer output for {}", port_name))?;
        let out_port = out_ports[out_index].clone();
        let output = output
            .connect(&out_port, "pipeweaver-smc-mixer config")
            .map_err(|_| anyhow!("could not write to {}", out_names[out_index]))?;

        Ok(Self {
            _input: connection,
            output,
            replies,
            port_name,
        })
    }

    fn name(&mut self) -> Result<String> {
        let packet = build(MSG_NAME, &[]);
        let (message, payload) = self.request(&packet)?;
        if message != MSG_NAME {
            bail!("unexpected reply to the name request");
        }
        Ok(String::from_utf8_lossy(&payload)
            .trim_end_matches('\0')
            .to_string())
    }

    fn read(&mut self, block: u8, addr: u32, length: usize) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(length);
        while out.len() < length {
            let size = (length - out.len()).min(512);
            let at = addr + out.len() as u32;
            let packet = build_read(block, at, size);
            let (message, payload) = self.request(&packet)?;
            if message != MSG_READ {
                bail!("unexpected reply type {message:#04x}");
            }
            if payload.len() < 8 {
                bail!("short read reply");
            }
            if payload[0] != block {
                bail!("read reply for block {}", payload[0]);
            }
            let reply_addr = u32::from_le_bytes([payload[1], payload[2], payload[3], payload[4]]);
            if reply_addr != at {
                bail!("read reply at {reply_addr}, wanted {at}");
            }
            let data = &payload[8..];
            if data.len() < size {
                bail!("read reply has {} bytes, wanted {size}", data.len());
            }
            out.extend_from_slice(&data[..size]);
        }
        Ok(out)
    }

    fn write(&mut self, block: u8, addr: u32, data: &[u8]) -> Result<()> {
        let mut offset = 0;
        while offset < data.len().max(1) {
            let end = (offset + 1000).min(data.len());
            let packet = build_write(block, addr + offset as u32, &data[offset..end]);
            let (message, _) = self.request(&packet)?;
            if message != MSG_ACK {
                bail!("unexpected reply type {message:#04x} to a write");
            }
            if data.is_empty() {
                break;
            }
            offset = end;
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }

    /// Persist a block: a zero-length write flushes the RAM buffer to flash.
    fn commit(&mut self, block: u8) -> Result<()> {
        self.write(block, 0, &[])
    }

    fn request(&mut self, packet: &[u8]) -> Result<(u8, Vec<u8>)> {
        for attempt in 0..2 {
            while self.replies.try_recv().is_ok() {}
            self.output
                .send(&to_sysex(packet))
                .with_context(|| format!("send to {}", self.port_name))?;

            let deadline = Instant::now() + Duration::from_millis(1500);
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                match self.replies.recv_timeout(left) {
                    Ok(message) => {
                        let raw = base128_decode(&message[1..message.len() - 1]);
                        match parse(&raw) {
                            Ok(reply) => return Ok(reply),
                            Err(error) => log::debug!("Ignoring mixer reply: {error:#}"),
                        }
                    }
                    Err(_) => break,
                }
            }
            log::debug!(
                "No mixer reply to {:#04x} (attempt {})",
                packet[2],
                attempt + 1
            );
        }
        bail!("the mixer did not answer")
    }
}

fn choose_config_port(names: &[String]) -> Option<usize> {
    names
        .iter()
        .position(|name| name.contains("SMC-Mixer") && name.contains("Private"))
        .or_else(|| {
            names
                .iter()
                .position(|name| name.contains("SMC-Mixer") && !name.contains("Master"))
        })
        .or_else(|| names.iter().position(|name| name.contains("SINCO")))
}

fn build(message: u8, payload: &[u8]) -> Vec<u8> {
    let mut packet = Vec::with_capacity(7 + payload.len());
    packet.extend_from_slice(&HEADER);
    packet.push(message);
    packet.push((payload.len() & 0xff) as u8);
    packet.push(((payload.len() >> 8) & 0xff) as u8);
    packet.push(((payload.len() >> 16) & 0xff) as u8);
    packet.extend_from_slice(payload);
    let sum: u32 = payload.iter().map(|byte| u32::from(*byte)).sum();
    packet.push((!(sum & 0xff) & 0xff) as u8);
    packet
}

fn build_read(block: u8, addr: u32, length: usize) -> Vec<u8> {
    let mut payload = Vec::with_capacity(8);
    payload.push(block);
    payload.extend_from_slice(&addr.to_le_bytes());
    payload.push((length & 0xff) as u8);
    payload.push(((length >> 8) & 0xff) as u8);
    payload.push(((length >> 16) & 0xff) as u8);
    build(MSG_READ, &payload)
}

fn build_write(block: u8, addr: u32, data: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(8 + data.len());
    payload.push(block);
    payload.extend_from_slice(&addr.to_le_bytes());
    payload.push((data.len() & 0xff) as u8);
    payload.push(((data.len() >> 8) & 0xff) as u8);
    payload.push(((data.len() >> 16) & 0xff) as u8);
    payload.extend_from_slice(data);
    build(MSG_WRITE, &payload)
}

fn parse(packet: &[u8]) -> Result<(u8, Vec<u8>)> {
    if packet.len() < 7 || packet[0..2] != HEADER {
        bail!("bad packet header");
    }
    let length =
        usize::from(packet[3]) | (usize::from(packet[4]) << 8) | (usize::from(packet[5]) << 16);
    if packet.len() < 6 + length + 1 {
        bail!("truncated packet");
    }
    let payload = &packet[6..6 + length];
    let sum: u32 = payload.iter().map(|byte| u32::from(*byte)).sum();
    let expected = (!(sum & 0xff) & 0xff) as u8;
    if packet[6 + length] != expected {
        bail!("bad checksum");
    }
    Ok((packet[2], payload.to_vec()))
}

fn to_sysex(packet: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(packet.len() + 4);
    out.push(0xf0);
    out.extend_from_slice(&base128_encode(packet));
    out.push(0xf7);
    out
}

/// Pack 8-bit data into 7-bit bytes. This is the continuous LSB-first stream
/// the M-VAVE firmware uses; a final partial byte is always emitted.
fn base128_encode(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() * 2);
    let mut accumulator: u32 = 0;
    let mut bits = 0u32;
    for byte in data {
        accumulator |= u32::from(*byte) << bits;
        bits += 8;
        while bits >= 7 {
            out.push((accumulator & 0x7f) as u8);
            accumulator >>= 7;
            bits -= 7;
        }
    }
    out.push((accumulator & 0x7f) as u8);
    out
}

fn base128_decode(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut bit_num = 0u32;
    let mut last = 0u8;
    for byte in data {
        let mask = if bit_num == 0 {
            0
        } else {
            (0xffff_ffffu32 >> (32 - bit_num)) as u8
        };
        let acc = byte & mask;
        if bit_num != 0 {
            out.push((last & 0x7f) | ((u16::from(acc) << (8 - bit_num)) as u8));
        }
        last = byte >> bit_num;
        bit_num = (bit_num + 1) % 8;
    }
    if last != 0 {
        out.push(last);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        KNOB_COUNT, KNOB_OFFSET, KNOB_SIZE, PRESET_SIZE, base128_decode, base128_encode, build,
        build_read, decode_hex, encode_hex, find_cc_preset_bytes, parse, patch_records,
        relative_values,
    };

    #[test]
    fn base128_round_trips() {
        for length in 0..70 {
            let data: Vec<u8> = (0..length).map(|index| (index * 37 + 11) as u8).collect();
            assert_eq!(base128_decode(&base128_encode(&data)), data);
        }
    }

    #[test]
    fn checksum_matches_the_captured_read() {
        // 00 59 23 08 00 00 04 00 00 00 00 01 00 00 fa
        let packet = build_read(4, 0, 1);
        assert_eq!(
            packet,
            vec![
                0x00, 0x59, 0x23, 0x08, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00,
                0xfa
            ]
        );
    }

    #[test]
    fn parses_its_own_packets() {
        let packet = build(0x23, &[4, 0, 0, 0, 0, 1, 0, 0, 7]);
        let (kind, payload) = parse(&packet).unwrap();
        assert_eq!(kind, 0x23);
        assert_eq!(payload, vec![4, 0, 0, 0, 0, 1, 0, 0, 7]);
    }

    #[test]
    fn finds_the_pipeweaver_preset() {
        let mut records = [[0u8; KNOB_COUNT * KNOB_SIZE]; 2];
        for index in 0..KNOB_COUNT {
            records[1][index * KNOB_SIZE + 3] = 30 + index as u8;
        }
        assert_eq!(find_cc_preset_bytes(&records, 1), Some(1));
        assert_eq!(find_cc_preset_bytes(&records, 0), Some(1));
    }

    #[test]
    fn patching_keeps_controller_and_curve() {
        let mut records = [0u8; KNOB_COUNT * KNOB_SIZE];
        for index in 0..KNOB_COUNT {
            let record = &mut records[index * KNOB_SIZE..(index + 1) * KNOB_SIZE];
            record.copy_from_slice(&[0, 2, 0, 30 + index as u8, 0, 127]);
        }
        patch_records(&mut records, true);
        for index in 0..KNOB_COUNT {
            let record = &records[index * KNOB_SIZE..(index + 1) * KNOB_SIZE];
            assert_eq!(record, &[3, 2, 0, 30 + index as u8, 65, 1]);
        }
        patch_records(&mut records, false);
        for index in 0..KNOB_COUNT {
            let record = &records[index * KNOB_SIZE..(index + 1) * KNOB_SIZE];
            assert_eq!(record, &[0, 2, 0, 30 + index as u8, 0, 127]);
        }
    }

    #[test]
    fn max_is_clockwise() {
        // Factory pair: min 65, max 1. The mixer sends 1 when turning up.
        assert_eq!(relative_values(65, 1), (1, 65));
        // A custom pair keeps the same rule.
        assert_eq!(relative_values(0, 127), (127, 0));
    }

    #[test]
    fn hex_round_trips() {
        let data = [0u8, 1, 0x7f, 0xff, 0x59];
        assert_eq!(decode_hex(&encode_hex(&data)).unwrap(), data);
    }

    #[test]
    fn preset_offsets_match_the_documented_layout() {
        assert_eq!(KNOB_OFFSET, 43 * 71);
        // Eight knobs followed by eight 5-byte faders fill the 3141-byte preset.
        assert_eq!(KNOB_OFFSET + KNOB_COUNT * KNOB_SIZE, 3101);
        assert_eq!(KNOB_OFFSET + KNOB_COUNT * KNOB_SIZE + 8 * 5, PRESET_SIZE);
    }

    /// Read-only hardware check. Run with:
    /// cargo test -p pipeweaver-smc-mixer --bin pipeweaver-smc-mixer hardware_probe -- --ignored --nocapture
    #[test]
    #[ignore = "requires a connected SMC-Mixer"]
    fn hardware_probe() {
        let config = pipeweaver_smc_core::config::Config::default();
        let mut link = super::ConfigLink::open(&config).unwrap();
        let name = link.name().unwrap();
        let globals = link
            .read(super::BLOCK_GLOBAL, 0, super::GLOBAL_SIZE)
            .unwrap();
        println!("device: {name}");
        println!("globals[0] (active preset): {}", globals[0]);
        for preset in 0..2 {
            let data = link
                .read(
                    super::BLOCK_USER,
                    (preset * PRESET_SIZE + KNOB_OFFSET) as u32,
                    KNOB_COUNT * KNOB_SIZE,
                )
                .unwrap();
            for index in 0..KNOB_COUNT {
                let record = &data[index * KNOB_SIZE..(index + 1) * KNOB_SIZE];
                println!(
                    "preset {preset} knob {index}: type {} curve {} ch {} cc {} min {} max {}",
                    record[0], record[1], record[2], record[3], record[4], record[5]
                );
            }
        }
    }
}
