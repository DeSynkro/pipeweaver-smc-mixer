pub mod config;
pub mod engine;
pub mod identity;

pub use config::{Config, MidiMode};
pub use engine::{Channel, ChannelKind, Engine, Snapshot, STRIP_COUNT};
pub use identity::{APP_ID, APP_NAME, APP_VERSION};
