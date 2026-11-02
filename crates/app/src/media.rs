use anyhow::{Context, Result};
use egui::ViewportCommand;
use std::process::Output;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::process::Command;
use tokio::sync::mpsc::UnboundedReceiver;

const MIC_TIMEOUT: Duration = Duration::from_secs(2);
const HEX_TIMEOUT: Duration = Duration::from_secs(5);
const OPEN_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug)]
struct CommandTimeout {
    program: String,
    timeout: Duration,
}

impl std::fmt::Display for CommandTimeout {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} timed out after {:?}",
            self.program, self.timeout
        )
    }
}

impl std::error::Error for CommandTimeout {}

/// Run a helper program with a hard deadline. The child is killed if the
/// deadline passes, so a stuck helper cannot hold the media queue open.
async fn run_output(program: &str, args: &[&str], timeout: Duration) -> Result<Output> {
    let mut command = Command::new(program);
    command.args(args).kill_on_drop(true);
    let started = Instant::now();
    match tokio::time::timeout(timeout, command.output()).await {
        Ok(Ok(output)) => {
            let elapsed = started.elapsed();
            if elapsed > Duration::from_secs(1) {
                log::debug!("{program} took {elapsed:?}");
            }
            Ok(output)
        }
        Ok(Err(error)) => Err(error).with_context(|| format!("run {program}")),
        Err(_) => Err(anyhow::Error::new(CommandTimeout {
            program: program.to_string(),
            timeout,
        })),
    }
}

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
    minimized: Arc<AtomicBool>,
    egui: Arc<Mutex<Option<egui::Context>>>,
) {
    if let Some(state) = query_mic_mute().await {
        mic_muted.store(state, Ordering::Relaxed);
    }
    while let Some(command) = rx.recv().await {
        let result = match command {
            MediaCommand::OpenPipeweaver => open_pipeweaver().await,
            MediaCommand::ToggleApp => {
                if minimized.swap(false, Ordering::Relaxed) {
                    if let Some(ctx) = egui.lock().unwrap().as_ref() {
                        ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
                        ctx.send_viewport_cmd(ViewportCommand::Focus);
                    }
                    ui_on.store(true, Ordering::Relaxed);
                    focus.store(true, Ordering::Relaxed);
                } else {
                    let was_on = ui_on.fetch_xor(true, Ordering::Relaxed);
                    if !was_on {
                        focus.store(true, Ordering::Relaxed);
                    }
                }
                Ok(())
            }
            MediaCommand::HexStart => hex_dictation("start").await,
            MediaCommand::HexStop => hex_dictation("stop").await,
            MediaCommand::ToggleMicMute => toggle_mic_mute(&mic_muted).await,
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

async fn open_pipeweaver() -> Result<()> {
    let mut child = Command::new("xdg-open")
        .arg("http://localhost:14565")
        .kill_on_drop(true)
        .spawn()
        .context("open the Pipeweaver UI")?;
    // Reap the child without blocking the media queue.
    tokio::spawn(async move {
        let _ = tokio::time::timeout(OPEN_TIMEOUT, child.wait()).await;
    });
    Ok(())
}

async fn toggle_mic_mute(muted: &AtomicBool) -> Result<()> {
    let wpctl = run_output(
        "wpctl",
        &["set-mute", "@DEFAULT_AUDIO_SOURCE@", "toggle"],
        MIC_TIMEOUT,
    )
    .await;
    let toggled = match wpctl {
        Ok(output) => output.status.success(),
        Err(error) if error.downcast_ref::<CommandTimeout>().is_some() => {
            // Do not fall back after a timeout: wpctl may already have
            // changed the mute state. A second toggle would undo it.
            return Err(error);
        }
        Err(_) => false,
    };
    if !toggled {
        let output = run_output(
            "pactl",
            &["set-source-mute", "@DEFAULT_SOURCE@", "toggle"],
            MIC_TIMEOUT,
        )
        .await
        .context("toggle the default microphone mute")?;
        if !output.status.success() {
            anyhow::bail!("could not toggle the default microphone mute");
        }
    }
    if let Some(state) = query_mic_mute().await {
        muted.store(state, Ordering::Relaxed);
    }
    Ok(())
}

async fn query_mic_mute() -> Option<bool> {
    if let Ok(output) = run_output(
        "wpctl",
        &["get-volume", "@DEFAULT_AUDIO_SOURCE@"],
        MIC_TIMEOUT,
    )
    .await
    {
        if output.status.success() {
            return Some(String::from_utf8_lossy(&output.stdout).contains("[MUTED]"));
        }
    }
    if let Ok(output) = run_output(
        "pactl",
        &["get-source-mute", "@DEFAULT_SOURCE@"],
        MIC_TIMEOUT,
    )
    .await
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

/// Stop Hex dictation during shutdown, with a bounded wait.
pub async fn stop_hex() {
    if let Err(error) = hex_dictation("stop").await {
        log::warn!("Could not stop Hex dictation: {error:#}");
    }
}

async fn hex_dictation(action: &str) -> Result<()> {
    let output = run_output("hex", &["dictation", action], HEX_TIMEOUT)
        .await
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

#[cfg(test)]
mod tests {
    use super::{CommandTimeout, run_output};
    use std::time::{Duration, Instant};

    #[tokio::test]
    async fn a_hanging_command_is_cut_off() {
        let started = Instant::now();
        let error = run_output("sleep", &["5"], Duration::from_millis(100))
            .await
            .unwrap_err();
        assert!(error.downcast_ref::<CommandTimeout>().is_some());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn a_normal_command_returns_its_output() {
        let output = run_output("printf", &["ok"], Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(output.stdout, b"ok");
    }

    #[tokio::test]
    async fn a_missing_command_is_not_a_timeout() {
        let error = run_output("pipeweaver-no-such-helper", &[], Duration::from_secs(1))
            .await
            .unwrap_err();
        assert!(error.downcast_ref::<CommandTimeout>().is_none());
    }
}
