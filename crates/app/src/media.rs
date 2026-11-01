use anyhow::{Context, Result};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc::UnboundedReceiver;

/// A desktop media action triggered by a transport button.
#[derive(Debug, Clone, Copy)]
pub enum MediaCommand {
    Play,
    Pause,
    Previous,
    Next,
    SeekBack,
    SeekForward,
    OpenPipeweaver,
    ToggleApp,
    HexStart,
    HexStop,
    ToggleMicMute,
}

pub async fn run(
    mut rx: UnboundedReceiver<MediaCommand>,
    ui_on: Arc<AtomicBool>,
    focus: Arc<AtomicBool>,
    mic_muted: Arc<AtomicBool>,
) {
    if let Some(state) = query_mic_mute() {
        mic_muted.store(state, Ordering::Relaxed);
    }
    while let Some(command) = rx.recv().await {
        let result = match command {
            MediaCommand::OpenPipeweaver => open_pipeweaver(),
            MediaCommand::ToggleApp => {
                let was_on = ui_on.fetch_xor(true, Ordering::Relaxed);
                if !was_on {
                    focus.store(true, Ordering::Relaxed);
                }
                Ok(())
            }
            MediaCommand::HexStart => hex_dictation("start"),
            MediaCommand::HexStop => hex_dictation("stop"),
            MediaCommand::ToggleMicMute => toggle_mic_mute(&mic_muted),
            MediaCommand::Play
            | MediaCommand::Pause
            | MediaCommand::Previous
            | MediaCommand::Next
            | MediaCommand::SeekBack
            | MediaCommand::SeekForward => send_media(command).await,
        };
        if let Err(error) = result {
            log::warn!("Media command failed: {error:#}");
        }
    }
}

fn open_pipeweaver() -> Result<()> {
    std::process::Command::new("xdg-open")
        .arg("http://localhost:14565")
        .spawn()
        .context("open the Pipeweaver UI")?;
    Ok(())
}

fn toggle_mic_mute(muted: &AtomicBool) -> Result<()> {
    let toggled = std::process::Command::new("wpctl")
        .args(["set-mute", "@DEFAULT_AUDIO_SOURCE@", "toggle"])
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !toggled {
        let status = std::process::Command::new("pactl")
            .args(["set-source-mute", "@DEFAULT_SOURCE@", "toggle"])
            .status()
            .context("toggle the default microphone mute")?;
        if !status.success() {
            anyhow::bail!("could not toggle the default microphone mute");
        }
    }
    if let Some(state) = query_mic_mute() {
        muted.store(state, Ordering::Relaxed);
    }
    Ok(())
}

fn query_mic_mute() -> Option<bool> {
    if let Ok(output) = std::process::Command::new("wpctl")
        .args(["get-volume", "@DEFAULT_AUDIO_SOURCE@"])
        .output()
    {
        if output.status.success() {
            return Some(String::from_utf8_lossy(&output.stdout).contains("[MUTED]"));
        }
    }
    if let Ok(output) = std::process::Command::new("pactl")
        .args(["get-source-mute", "@DEFAULT_SOURCE@"])
        .output()
    {
        if output.status.success() {
            return Some(
                String::from_utf8_lossy(&output.stdout)
                    .to_ascii_lowercase()
                    .contains("yes"),
            );
        }
    }
    None
}

fn hex_dictation(action: &str) -> Result<()> {
    let output = std::process::Command::new("hex")
        .args(["dictation", action])
        .output()
        .context("run hex dictation")?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("hex dictation {action} failed: {}", message.trim());
    }
    Ok(())
}

async fn send_media(command: MediaCommand) -> Result<()> {
    let connection = zbus::Connection::session()
        .await
        .context("connect to the session bus")?;
    let dbus = zbus::fdo::DBusProxy::new(&connection).await?;
    let names = dbus.list_names().await?;
    let mut players: Vec<String> = names
        .into_iter()
        .map(|name| name.to_string())
        .filter(|name| name.starts_with("org.mpris.MediaPlayer2."))
        .collect();
    if players.is_empty() {
        anyhow::bail!("no MPRIS media player is running");
    }

    // Prefer whichever player is actually playing; otherwise use the first.
    let mut chosen = None;
    for name in &players {
        let Ok(proxy) = player_proxy(&connection, name).await else {
            continue;
        };
        if let Ok(status) = proxy.get_property::<String>("PlaybackStatus").await {
            if status == "Playing" {
                chosen = Some(name.clone());
                break;
            }
        }
    }
    let name = chosen.unwrap_or_else(|| players.remove(0));
    let proxy = player_proxy(&connection, &name).await?;

    let result = match command {
        MediaCommand::Play => proxy.call_method("Play", &()).await,
        MediaCommand::Pause => proxy.call_method("Pause", &()).await,
        MediaCommand::Previous => proxy.call_method("Previous", &()).await,
        MediaCommand::Next => proxy.call_method("Next", &()).await,
        MediaCommand::SeekBack => proxy.call_method("Seek", &(-5_000_000i64)).await,
        MediaCommand::SeekForward => proxy.call_method("Seek", &(5_000_000i64)).await,
        MediaCommand::OpenPipeweaver | MediaCommand::ToggleApp => unreachable!(),
        MediaCommand::HexStart | MediaCommand::HexStop => unreachable!(),
        MediaCommand::ToggleMicMute => unreachable!(),
    };
    result.with_context(|| format!("media command on {name}"))?;
    Ok(())
}

async fn player_proxy<'a>(connection: &zbus::Connection, name: &str) -> Result<zbus::Proxy<'a>> {
    zbus::Proxy::new(
        connection,
        name.to_string(),
        "/org/mpris/MediaPlayer2",
        "org.mpris.MediaPlayer2.Player",
    )
    .await
    .context("create an MPRIS player proxy")
}
