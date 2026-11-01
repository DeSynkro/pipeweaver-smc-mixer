pub mod config;
pub mod engine;
pub mod identity;

pub use config::{
    Config, MidiMode, SURFACE_SOURCES_DEFAULT, SURFACE_SOURCES_MAX, SURFACE_SOURCES_MIN,
};
pub use engine::{Channel, ChannelKind, Engine, STRIP_COUNT, Snapshot};
pub use identity::{APP_ID, APP_NAME, APP_VERSION};
