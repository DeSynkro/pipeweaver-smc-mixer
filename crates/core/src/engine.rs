use crate::config::{SURFACE_SOURCES_MAX, SURFACE_SOURCES_MIN};
use pipeweaver_ipc::commands::DaemonStatus;
use pipeweaver_shared::{DeviceType, Mix, MuteState, MuteTarget, OrderGroup};
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
    pub applications: Applications,
    pub routing: Routing,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            connected: false,
            channels: vec![Channel::default(); STRIP_COUNT],
            off_surface: Vec::new(),
            applications: Applications::default(),
            routing: Routing::default(),
        }
    }
}

/// One PipeWire node behind an application entry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppNode {
    pub node_id: u32,
    pub title: Option<String>,
}

/// An application name as Pipeweaver groups it, with its current route.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppEntry {
    pub name: String,
    pub route: Option<Ulid>,
    pub nodes: Vec<AppNode>,
}

/// Applications grouped by the process that owns them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppGroup {
    pub process: String,
    pub apps: Vec<AppEntry>,
}

/// A device an application can be routed to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RouteChoice {
    pub id: Ulid,
    pub name: String,
}

/// The application lists and route choices shown on the Pipeweaver screen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Applications {
    pub output: Vec<AppGroup>,
    pub input: Vec<AppGroup>,
    pub output_routes: Vec<RouteChoice>,
    pub input_routes: Vec<RouteChoice>,
}

/// One source or target column/row in the routing table.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoutingDevice {
    pub id: Ulid,
    pub name: String,
    pub colour: (u8, u8, u8),
    pub mix: Option<Mix>,
}

/// Sources, targets and the enabled source-target pairs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Routing {
    pub sources: Vec<RoutingDevice>,
    pub targets: Vec<RoutingDevice>,
    pub routes: HashSet<(Ulid, Ulid)>,
}

pub struct Engine {
    snapshot: RwLock<Arc<Snapshot>>,
    meters: RwLock<HashMap<Ulid, u8>>,
    source_slots: RwLock<usize>,
    status: RwLock<Option<DaemonStatus>>,
}

impl Engine {
    pub fn new(source_slots: usize) -> Arc<Self> {
        Arc::new(Self {
            snapshot: RwLock::new(Arc::new(Snapshot::default())),
            meters: RwLock::new(HashMap::new()),
            source_slots: RwLock::new(source_slots.clamp(SURFACE_SOURCES_MIN, SURFACE_SOURCES_MAX)),
            status: RwLock::new(None),
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

    pub fn apply(&self, status: DaemonStatus) {
        // Hold the split while rebuilding so a concurrent split change cannot
        // publish a snapshot built with the previous slot count.
        let source_slots = self.source_slots.read().unwrap();
        let snapshot = Arc::new(build(&status, *source_slots));
        *self.status.write().unwrap() = Some(status);
        *self.snapshot.write().unwrap() = snapshot;
    }

    pub fn set_source_slots(&self, source_slots: usize) {
        let source_slots = source_slots.clamp(SURFACE_SOURCES_MIN, SURFACE_SOURCES_MAX);
        {
            let mut current = self.source_slots.write().unwrap();
            if *current == source_slots {
                return;
            }
            *current = source_slots;
        }
        let status = self.status.read().unwrap().clone();
        if let Some(status) = status {
            let connected = self.snapshot.read().unwrap().connected;
            let mut next = build(&status, source_slots);
            next.connected = connected;
            *self.snapshot.write().unwrap() = Arc::new(next);
        }
    }

    pub fn source_slots(&self) -> usize {
        *self.source_slots.read().unwrap()
    }

    pub fn set_meter(&self, id: Ulid, percent: u8) {
        self.meters.write().unwrap().insert(id, percent);
    }

    pub fn meter(&self, id: Ulid) -> u8 {
        self.meters.read().unwrap().get(&id).copied().unwrap_or(0)
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
        if !assigned.contains(id)
            && let Some(channel) = source_channel(status, *id)
        {
            off_surface.push(channel);
        }
    }
    for id in &targets {
        if !assigned.contains(id)
            && let Some(channel) = target_channel(status, *id)
        {
            off_surface.push(channel);
        }
    }

    Snapshot {
        connected: true,
        channels,
        off_surface,
        applications: build_applications(status),
        routing: build_routing(status),
    }
}

fn build_routing(status: &DaemonStatus) -> Routing {
    let sources = ordered_source_ids(status)
        .into_iter()
        .filter_map(|id| routing_source(status, id))
        .collect();
    let targets = ordered_target_ids(status)
        .into_iter()
        .filter_map(|id| routing_target(status, id))
        .collect();
    let routes = status
        .audio
        .profile
        .routes
        .iter()
        .flat_map(|(source, targets)| targets.iter().map(move |target| (*source, *target)))
        .collect();

    Routing {
        sources,
        targets,
        routes,
    }
}

fn routing_source(status: &DaemonStatus, id: Ulid) -> Option<RoutingDevice> {
    let devices = &status.audio.profile.devices.sources;

    for device in &devices.physical_devices {
        if device.description.id == id {
            return Some(RoutingDevice {
                id,
                name: device.description.name.clone(),
                colour: colour(
                    device.description.colour.red,
                    device.description.colour.green,
                    device.description.colour.blue,
                ),
                mix: None,
            });
        }
    }
    for device in &devices.virtual_devices {
        if device.description.id == id {
            return Some(RoutingDevice {
                id,
                name: device.description.name.clone(),
                colour: colour(
                    device.description.colour.red,
                    device.description.colour.green,
                    device.description.colour.blue,
                ),
                mix: None,
            });
        }
    }

    None
}

fn routing_target(status: &DaemonStatus, id: Ulid) -> Option<RoutingDevice> {
    let devices = &status.audio.profile.devices.targets;

    for device in &devices.physical_devices {
        if device.description.id == id {
            return Some(RoutingDevice {
                id,
                name: device.description.name.clone(),
                colour: colour(
                    device.description.colour.red,
                    device.description.colour.green,
                    device.description.colour.blue,
                ),
                mix: Some(device.mix),
            });
        }
    }
    for device in &devices.virtual_devices {
        if device.description.id == id {
            return Some(RoutingDevice {
                id,
                name: device.description.name.clone(),
                colour: colour(
                    device.description.colour.red,
                    device.description.colour.green,
                    device.description.colour.blue,
                ),
                mix: Some(device.mix),
            });
        }
    }

    None
}

fn build_applications(status: &DaemonStatus) -> Applications {
    Applications {
        output: app_groups(status, DeviceType::Source),
        input: app_groups(status, DeviceType::Target),
        output_routes: status
            .audio
            .profile
            .devices
            .sources
            .virtual_devices
            .iter()
            .map(|device| RouteChoice {
                id: device.description.id,
                name: device.description.name.clone(),
            })
            .collect(),
        input_routes: status
            .audio
            .profile
            .devices
            .targets
            .virtual_devices
            .iter()
            .map(|device| RouteChoice {
                id: device.description.id,
                name: device.description.name.clone(),
            })
            .collect(),
    }
}

fn app_groups(status: &DaemonStatus, side: DeviceType) -> Vec<AppGroup> {
    let mapping = &status.audio.profile.application_mapping[side];
    let mut groups = Vec::new();

    for (process, apps) in &status.audio.applications[side] {
        let mut entries: Vec<AppEntry> = apps
            .iter()
            .map(|(name, nodes)| AppEntry {
                name: name.clone(),
                route: mapping
                    .get(process)
                    .and_then(|apps| apps.get(name))
                    .copied()
                    .or_else(|| nodes.first().and_then(|node| node.target_id)),
                nodes: nodes
                    .iter()
                    .map(|node| AppNode {
                        node_id: node.node_id,
                        title: node.title.clone(),
                    })
                    .collect(),
            })
            .collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        groups.push(AppGroup {
            process: process.clone(),
            apps: entries,
        });
    }

    groups.sort_by(|a, b| a.process.cmp(&b.process));
    groups
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
                colour: colour(
                    device.description.colour.red,
                    device.description.colour.green,
                    device.description.colour.blue,
                ),
                fader: device.volumes.volume[Mix::A],
                knob: device.volumes.volume[Mix::B],
                mute_a: device.mute_states.mute_state.contains(&MuteTarget::TargetA),
                mute_b: device.mute_states.mute_state.contains(&MuteTarget::TargetB),
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
                colour: colour(
                    device.description.colour.red,
                    device.description.colour.green,
                    device.description.colour.blue,
                ),
                fader: device.volumes.volume[Mix::A],
                knob: device.volumes.volume[Mix::B],
                mute_a: device.mute_states.mute_state.contains(&MuteTarget::TargetA),
                mute_b: device.mute_states.mute_state.contains(&MuteTarget::TargetB),
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
                colour: colour(
                    device.description.colour.red,
                    device.description.colour.green,
                    device.description.colour.blue,
                ),
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
                colour: colour(
                    device.description.colour.red,
                    device.description.colour.green,
                    device.description.colour.blue,
                ),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_slots_clamp_and_change() {
        let engine = Engine::new(6);
        assert_eq!(engine.source_slots(), 6);
        engine.set_source_slots(4);
        assert_eq!(engine.source_slots(), 4);
        engine.set_source_slots(2);
        assert_eq!(engine.source_slots(), 4);
        engine.set_source_slots(9);
        assert_eq!(engine.source_slots(), 8);
    }

    #[test]
    fn builds_the_routing_table_in_device_order() {
        let source_a = Ulid::new();
        let source_b = Ulid::new();
        let target = Ulid::new();
        let mut status = DaemonStatus::default();

        for (id, name) in [(source_a, "Browser"), (source_b, "Game")] {
            status
                .audio
                .profile
                .devices
                .sources
                .virtual_devices
                .push(Default::default());
            let device = status
                .audio
                .profile
                .devices
                .sources
                .virtual_devices
                .last_mut()
                .unwrap();
            device.description.id = id;
            device.description.name = name.to_string();
        }
        status.audio.profile.devices.sources.device_order[OrderGroup::Pinned].push(source_b);

        status
            .audio
            .profile
            .devices
            .targets
            .virtual_devices
            .push(Default::default());
        {
            let device = &mut status.audio.profile.devices.targets.virtual_devices[0];
            device.description.id = target;
            device.description.name = "Stream".to_string();
            device.mix = Mix::B;
        }
        status
            .audio
            .profile
            .routes
            .insert(source_a, HashSet::from([target]));

        let engine = Engine::new(8);
        engine.apply(status);
        let routing = &engine.snapshot().routing;

        assert_eq!(routing.sources.len(), 2);
        assert_eq!(routing.sources[0].id, source_b);
        assert_eq!(routing.sources[1].id, source_a);
        assert_eq!(routing.targets.len(), 1);
        assert_eq!(routing.targets[0].name, "Stream");
        assert_eq!(routing.targets[0].mix, Some(Mix::B));
        assert!(routing.routes.contains(&(source_a, target)));
        assert!(!routing.routes.contains(&(source_b, target)));
    }

    #[test]
    fn builds_the_application_list_and_its_routes() {
        use pipeweaver_ipc::commands::Application;
        use pipeweaver_shared::DeviceType;

        let target = Ulid::new();
        let source = Ulid::new();

        let mut status = DaemonStatus::default();
        status
            .audio
            .profile
            .devices
            .targets
            .virtual_devices
            .push(Default::default());
        {
            let device = &mut status.audio.profile.devices.targets.virtual_devices[0];
            device.description.id = target;
            device.description.name = "Personal".to_string();
        }
        status
            .audio
            .profile
            .devices
            .sources
            .virtual_devices
            .push(Default::default());
        {
            let device = &mut status.audio.profile.devices.sources.virtual_devices[0];
            device.description.id = source;
            device.description.name = "Browser".to_string();
        }

        status.audio.applications[DeviceType::Source]
            .entry("firefox".to_string())
            .or_default()
            .insert(
                "Firefox".to_string(),
                vec![Application {
                    node_id: 42,
                    name: "Firefox".to_string(),
                    volume: 100,
                    muted: false,
                    title: Some("Mozilla Firefox".to_string()),
                    target: None,
                    target_id: Some(target),
                }],
            );
        status.audio.profile.application_mapping[DeviceType::Source]
            .entry("firefox".to_string())
            .or_default()
            .insert("Firefox".to_string(), source);

        status.audio.applications[DeviceType::Target]
            .entry("obs".to_string())
            .or_default()
            .insert(
                "OBS".to_string(),
                vec![Application {
                    node_id: 43,
                    name: "OBS".to_string(),
                    volume: 100,
                    muted: false,
                    title: None,
                    target: None,
                    target_id: Some(target),
                }],
            );
        status.audio.profile.application_mapping[DeviceType::Target]
            .entry("obs".to_string())
            .or_default()
            .insert("OBS".to_string(), target);

        let engine = Engine::new(8);
        engine.apply(status);
        let snapshot = engine.snapshot();

        assert_eq!(snapshot.applications.output_routes.len(), 1);
        assert_eq!(snapshot.applications.output_routes[0].name, "Browser");
        assert_eq!(snapshot.applications.output.len(), 1);
        assert_eq!(snapshot.applications.output[0].process, "firefox");
        assert_eq!(snapshot.applications.output[0].apps[0].name, "Firefox");
        assert_eq!(snapshot.applications.output[0].apps[0].route, Some(source));
        assert_eq!(
            snapshot.applications.output[0].apps[0].nodes[0]
                .title
                .as_deref(),
            Some("Mozilla Firefox")
        );

        assert_eq!(snapshot.applications.input_routes[0].name, "Personal");
        assert_eq!(snapshot.applications.input[0].apps[0].route, Some(target));
    }
}
