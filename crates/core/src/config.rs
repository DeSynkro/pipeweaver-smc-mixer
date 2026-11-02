use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static SAVE_LOCK: Mutex<()> = Mutex::new(());

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
    false
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
        let mut config: Self = match serde_json::from_str(&data) {
            Ok(config) => config,
            Err(error) => {
                let backup = path.with_extension("json.bad");
                if let Err(copy_error) = fs::copy(path, &backup) {
                    log::warn!(
                        "Could not back up corrupt config {}: {copy_error}",
                        path.display()
                    );
                } else {
                    log::warn!(
                        "Could not parse config {}: {error}; backed up to {}",
                        path.display(),
                        backup.display()
                    );
                }
                Self::default()
            }
        };
        config.surface_sources = config
            .surface_sources
            .clamp(SURFACE_SOURCES_MIN, SURFACE_SOURCES_MAX);
        config
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let _guard = SAVE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
        config_home()
            .join("pipeweaver")
            .join(crate::identity::CONFIG_FILE)
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

fn autostart_executable(
    appimage: Option<std::ffi::OsString>,
    current_exe: Option<PathBuf>,
) -> PathBuf {
    // Inside an AppImage, current_exe() points at the temporary mount. The
    // autostart entry must point at the .AppImage file itself.
    appimage
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or(current_exe)
        .unwrap_or_else(|| PathBuf::from(crate::identity::APP_ID))
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
    let exe = autostart_executable(env::var_os("APPIMAGE"), env::current_exe().ok())
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
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
        exe,
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

#[cfg(test)]
mod tests {
    use super::{
        Config, SURFACE_SOURCES_DEFAULT, SURFACE_SOURCES_MAX, SURFACE_SOURCES_MIN,
        autostart_executable,
    };
    use std::ffi::OsString;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_config_path(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "pipeweaver-smc-mixer-{label}-{}-{stamp}.json",
            std::process::id()
        ))
    }

    #[test]
    fn autostart_uses_the_appimage_path_when_present() {
        let exe = autostart_executable(
            Some(OsString::from("/apps/Pipeweaver-SMC-Mixer.AppImage")),
            Some(PathBuf::from("/tmp/.mount/usr/bin/pipeweaver-smc-mixer")),
        );
        assert_eq!(exe, PathBuf::from("/apps/Pipeweaver-SMC-Mixer.AppImage"));
    }

    #[test]
    fn autostart_falls_back_when_appimage_is_empty() {
        let exe = autostart_executable(
            Some(OsString::new()),
            Some(PathBuf::from("/usr/bin/pipeweaver-smc-mixer")),
        );
        assert_eq!(exe, PathBuf::from("/usr/bin/pipeweaver-smc-mixer"));
    }

    #[test]
    fn corrupt_config_is_backed_up_and_defaults_are_used() {
        let path = temp_config_path("corrupt");
        fs::write(&path, "{ not valid json").unwrap();

        let config = Config::load(&path);

        assert_eq!(config.surface_sources, SURFACE_SOURCES_DEFAULT);
        let backup = path.with_extension("json.bad");
        assert!(backup.exists());

        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(&backup);
    }

    #[test]
    fn concurrent_saves_leave_a_readable_config() {
        let path = temp_config_path("concurrent");
        let first = Config::default();
        let mut second = Config::default();
        second.surface_sources = SURFACE_SOURCES_MAX;

        let first_path = path.clone();
        let second_path = path.clone();
        let first_thread = std::thread::spawn(move || first.save(&first_path).unwrap());
        let second_thread = std::thread::spawn(move || second.save(&second_path).unwrap());
        first_thread.join().unwrap();
        second_thread.join().unwrap();

        let loaded = Config::load(&path);
        assert!((SURFACE_SOURCES_MIN..=SURFACE_SOURCES_MAX).contains(&loaded.surface_sources));

        let _ = fs::remove_file(&path);
    }
}
