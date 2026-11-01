use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MidiMode {
    #[default]
    Auto,
    Wired,
    Bluetooth,
}

impl MidiMode {
    pub fn code(self) -> u8 {
        match self {
            Self::Auto => 0,
            Self::Wired => 1,
            Self::Bluetooth => 2,
        }
    }

    pub fn from_code(code: u8) -> Self {
        match code {
            1 => Self::Wired,
            2 => Self::Bluetooth,
            _ => Self::Auto,
        }
    }
}

pub const SURFACE_SOURCES_MIN: usize = 4;
pub const SURFACE_SOURCES_MAX: usize = 8;
pub const SURFACE_SOURCES_DEFAULT: usize = 6;

fn default_sources() -> usize {
    SURFACE_SOURCES_DEFAULT
}

fn default_labels() -> [String; 2] {
    [String::from("Personal"), String::from("Audience")]
}

fn default_width() -> f32 {
    1280.0
}

fn default_pickup() -> bool {
    true
}

fn default_record_hex() -> bool {
    true
}

fn default_startup_animation() -> bool {
    true
}

fn default_schema() -> u32 {
    1
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    pub midi_device: String,
    pub midi_mode: MidiMode,
    pub surface_sources: usize,
    pub mix_labels: [String; 2],
    pub light_panel: bool,
    #[serde(default)]
    pub channel_colours: bool,
    #[serde(default = "default_pickup")]
    pub pickup: bool,
    #[serde(default)]
    pub hide_off_surface: bool,
    #[serde(default = "default_record_hex")]
    pub record_hex: bool,
    #[serde(default = "default_startup_animation")]
    pub startup_animation: bool,
    pub start_minimized: bool,
    pub window_width: f32,
    pub schema: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            midi_device: String::new(),
            midi_mode: MidiMode::Auto,
            surface_sources: default_sources(),
            mix_labels: default_labels(),
            light_panel: false,
            channel_colours: false,
            pickup: default_pickup(),
            hide_off_surface: false,
            record_hex: default_record_hex(),
            startup_animation: default_startup_animation(),
            start_minimized: false,
            window_width: default_width(),
            schema: default_schema(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Self {
        let Ok(data) = fs::read_to_string(path) else {
            return Self::default();
        };
        let mut config: Self = serde_json::from_str(&data).unwrap_or_default();
        config.surface_sources = config
            .surface_sources
            .clamp(SURFACE_SOURCES_MIN, SURFACE_SOURCES_MAX);
        config
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let parent = path
            .parent()
            .context("config path has no parent directory")?;
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        let tmp = path.with_extension("json.tmp");
        let data = serde_json::to_string_pretty(self)?;
        fs::write(&tmp, data).with_context(|| format!("write {}", tmp.display()))?;
        fs::rename(&tmp, path).with_context(|| format!("replace {}", path.display()))?;
        Ok(())
    }

    pub fn path() -> PathBuf {
        config_home().join("pipeweaver").join(crate::identity::CONFIG_FILE)
    }

    pub fn window_width(&self) -> f32 {
        self.window_width.clamp(1000.0, 3840.0)
    }
}

pub fn config_home() -> PathBuf {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn autostart_path() -> PathBuf {
    config_home()
        .join("autostart")
        .join(format!("{}.desktop", crate::identity::APP_ID))
}

pub fn set_autostart(enabled: bool) -> Result<()> {
    let path = autostart_path();
    if !enabled {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        return Ok(());
    }
    let parent = path.parent().context("autostart path has no parent")?;
    fs::create_dir_all(parent)?;
    let exe = std::env::current_exe()
        .unwrap_or_else(|_| PathBuf::from(crate::identity::APP_ID));
    let entry = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={}\n\
         Comment=SMC-Mixer control surface for Pipeweaver\n\
         Exec=env -u WAYLAND_DISPLAY \"{}\" --quiet\n\
         Icon={}\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n",
        crate::identity::APP_NAME,
        exe.display(),
        crate::identity::APP_ID,
    );
    fs::write(&path, entry)?;
    Ok(())
}

pub fn state_dir() -> PathBuf {
    env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join(crate::identity::APP_ID)
}

pub fn runtime_dir() -> PathBuf {
    env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(state_dir)
}
