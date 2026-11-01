use pipeweaver_ipc::commands::DaemonStatus;
use pipeweaver_shared::{Mix, MuteState, MuteTarget, OrderGroup};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use ulid::Ulid;

pub const STRIP_COUNT: usize = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChannelKind {
    #[default]
    Source,
    Target,
}

#[derive(Clone, Debug)]
pub struct Channel {
    pub assigned: bool,
    pub id: Ulid,
    pub kind: ChannelKind,
    pub name: String,
    pub colour: (u8, u8, u8),
    pub fader: u8,
    pub knob: u8,
    pub mute_a: bool,
    pub mute_b: bool,
    pub mix_b: bool,
    pub linked: bool,
}

impl Default for Channel {
    fn default() -> Self {
        Self {
            assigned: false,
            id: Ulid::nil(),
            kind: ChannelKind::Source,
            name: String::new(),
            colour: (120, 130, 143),
            fader: 0,
            knob: 0,
            mute_a: false,
            mute_b: false,
            mix_b: false,
            linked: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub connected: bool,
    pub channels: Vec<Channel>,
    pub off_surface: Vec<Channel>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            connected: false,
            channels: vec![Channel::default(); STRIP_COUNT],
            off_surface: Vec::new(),
        }
    }
}

pub struct Engine {
    snapshot: RwLock<Arc<Snapshot>>,
    meters: RwLock<HashMap<Ulid, u8>>,
}

impl Engine {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            snapshot: RwLock::new(Arc::new(Snapshot::default())),
            meters: RwLock::new(HashMap::new()),
        })
    }

    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.snapshot.read().unwrap().clone()
    }

    pub fn set_connected(&self, connected: bool) {
        let mut current = self.snapshot.write().unwrap();
        if current.connected == connected {
            return;
        }
        let mut next = (**current).clone();
        next.connected = connected;
        *current = Arc::new(next);
    }

    pub fn apply(&self, status: &DaemonStatus, source_slots: usize) {
        *self.snapshot.write().unwrap() = Arc::new(build(status, source_slots));
    }

    pub fn set_meter(&self, id: Ulid, percent: u8) {
        self.meters.write().unwrap().insert(id, percent);
    }

    pub fn meter(&self, id: Ulid) -> u8 {
        self.meters
            .read()
            .unwrap()
            .get(&id)
            .copied()
            .unwrap_or(0)
    }

    pub fn clear_meters(&self) {
        self.meters.write().unwrap().clear();
    }
}

fn build(status: &DaemonStatus, source_slots: usize) -> Snapshot {
    let source_slots = source_slots.min(STRIP_COUNT);
    let sources = ordered_source_ids(status);
    let targets = ordered_target_ids(status);
    let mut channels = Vec::with_capacity(STRIP_COUNT);

    for index in 0..STRIP_COUNT {
        let channel = if index < source_slots {
            sources
                .get(index)
                .and_then(|id| source_channel(status, *id))
                .unwrap_or_default()
        } else {
            targets
                .get(index - source_slots)
                .and_then(|id| target_channel(status, *id))
                .unwrap_or_default()
        };
        channels.push(channel);
    }

    let assigned: HashSet<Ulid> = channels
        .iter()
        .filter(|channel| channel.assigned)
        .map(|channel| channel.id)
        .collect();

    let mut off_surface = Vec::new();
    for id in &sources {
        if !assigned.contains(id) {
            if let Some(channel) = source_channel(status, *id) {
                off_surface.push(channel);
            }
        }
    }
    for id in &targets {
        if !assigned.contains(id) {
            if let Some(channel) = target_channel(status, *id) {
                off_surface.push(channel);
            }
        }
    }

    Snapshot {
        connected: true,
        channels,
        off_surface,
    }
}

fn source_channel(status: &DaemonStatus, id: Ulid) -> Option<Channel> {
    let devices = &status.audio.profile.devices.sources;

    for device in &devices.physical_devices {
        if device.description.id == id {
            return Some(Channel {
                assigned: true,
                id: device.description.id,
                kind: ChannelKind::Source,
                name: device.description.name.clone(),
                colour: colour(device.description.colour.red, device.description.colour.green, device.description.colour.blue),
                fader: device.volumes.volume[Mix::A],
                knob: device.volumes.volume[Mix::B],
                mute_a: device
                    .mute_states
                    .mute_state
                    .contains(&MuteTarget::TargetA),
                mute_b: device
                    .mute_states
                    .mute_state
                    .contains(&MuteTarget::TargetB),
                mix_b: false,
                linked: device.volumes.volumes_linked.is_some(),
            });
        }
    }

    for device in &devices.virtual_devices {
        if device.description.id == id {
            return Some(Channel {
                assigned: true,
                id: device.description.id,
                kind: ChannelKind::Source,
                name: device.description.name.clone(),
                colour: colour(device.description.colour.red, device.description.colour.green, device.description.colour.blue),
                fader: device.volumes.volume[Mix::A],
                knob: device.volumes.volume[Mix::B],
                mute_a: device
                    .mute_states
                    .mute_state
                    .contains(&MuteTarget::TargetA),
                mute_b: device
                    .mute_states
                    .mute_state
                    .contains(&MuteTarget::TargetB),
                mix_b: false,
                linked: device.volumes.volumes_linked.is_some(),
            });
        }
    }

    None
}

fn target_channel(status: &DaemonStatus, id: Ulid) -> Option<Channel> {
    let devices = &status.audio.profile.devices.targets;

    for device in &devices.physical_devices {
        if device.description.id == id {
            return Some(Channel {
                assigned: true,
                id: device.description.id,
                kind: ChannelKind::Target,
                name: device.description.name.clone(),
                colour: colour(device.description.colour.red, device.description.colour.green, device.description.colour.blue),
                fader: device.volume,
                knob: device.volume,
                mute_a: device.mute_state == MuteState::Muted,
                mute_b: false,
                mix_b: device.mix == Mix::B,
                linked: false,
            });
        }
    }

    for device in &devices.virtual_devices {
        if device.description.id == id {
            return Some(Channel {
                assigned: true,
                id: device.description.id,
                kind: ChannelKind::Target,
                name: device.description.name.clone(),
                colour: colour(device.description.colour.red, device.description.colour.green, device.description.colour.blue),
                fader: device.volume,
                knob: device.volume,
                mute_a: device.mute_state == MuteState::Muted,
                mute_b: false,
                mix_b: device.mix == Mix::B,
                linked: false,
            });
        }
    }

    None
}

fn colour(red: u8, green: u8, blue: u8) -> (u8, u8, u8) {
    (red, green, blue)
}

fn ordered_source_ids(status: &DaemonStatus) -> Vec<Ulid> {
    let devices = &status.audio.profile.devices.sources;
    let order = &devices.device_order;
    ordered_ids(
        &order[OrderGroup::Pinned],
        &order[OrderGroup::Default],
        &order[OrderGroup::Hidden],
        devices
            .physical_devices
            .iter()
            .map(|device| device.description.id)
            .chain(
                devices
                    .virtual_devices
                    .iter()
                    .map(|device| device.description.id),
            ),
    )
}

fn ordered_target_ids(status: &DaemonStatus) -> Vec<Ulid> {
    let devices = &status.audio.profile.devices.targets;
    let order = &devices.device_order;
    ordered_ids(
        &order[OrderGroup::Pinned],
        &order[OrderGroup::Default],
        &order[OrderGroup::Hidden],
        devices
            .physical_devices
            .iter()
            .map(|device| device.description.id)
            .chain(
                devices
                    .virtual_devices
                    .iter()
                    .map(|device| device.description.id),
            ),
    )
}

fn ordered_ids(
    pinned: &[Ulid],
    default: &[Ulid],
    hidden: &[Ulid],
    all: impl Iterator<Item = Ulid>,
) -> Vec<Ulid> {
    let hidden: HashSet<Ulid> = hidden.iter().copied().collect();
    let mut out = Vec::new();
    for id in pinned.iter().chain(default.iter()) {
        if !hidden.contains(id) && !out.contains(id) {
            out.push(*id);
        }
    }
    for id in all {
        if !hidden.contains(&id) && !out.contains(&id) {
            out.push(id);
        }
    }
    out
}
