# Changelog

## 1.2.0 - 2026-11-11

### Added

- A Pipeweaver screen, opened from the third navigation button. It splits the
  page into an application list on top and the routing table below, so both
  stay visible at once.
- The application list shows running output and input applications grouped by
  process, with each application's current route. A dropdown reassigns it to a
  virtual source or target, or back to Default. Applications that stop stay
  listed for ten seconds with a countdown ring, so their route can still be
  changed; the countdown pauses while the dropdown is open.
- The routing table shows sources as columns and targets as rows, with the
  current routes checked in the target's mix colour. It fills the available
  width and scales its rows, columns and labels, and scrolls when it outgrows
  the pane.
- An "Open Pipeweaver" link on the Pipeweaver page, which opens the daemon's
  own UI.
- Eight selectable startup animations: Classic, Sweep, Chase, Wave, Sparkle,
  Pulse, Ripple and Rain. Config gets an animation dropdown, and selecting a
  style previews it immediately. Chase sweeps the Square, R, S and M rows in
  that order.
- The app blacks out the 32 surface LEDs before suspend and power-off. It
  holds logind delay inhibitors, sends the note-offs on a separate short-lived
  MIDI connection as soon as the event arrives, and keeps the normal MIDI
  thread as a second path. Normal termination and SIGTERM also send the
  blackout.
- The app button now toggles the window on a short release and switches to the
  Pipeweaver screen when held for one second. This applies to both the hardware
  button (CC 61) and the left on-screen arrow.

### Changed

- The former Pipeweaver button (CC 62) and the right on-screen arrow are
  unbound.
- The navigation has a third button, and its frames stay visible on the Mixer
  screen.
- The startup animation setting is stored by name. Existing configs default to
  Classic.

### Fixed

- The surface LEDs could stay lit through a PC shutdown because the session
  killed the MIDI thread before it could send the blackout. The emergency
  connection removes that dependency. The firmware-owned transport LEDs are
  unchanged and still cannot be controlled.
- The Pipeweaver navigation frame was invisible while the Mixer screen was
  active.

### Notes

- The shutdown blackout is best effort. If the app was already closed, was
  hard-killed, or the mixer was disconnected, there is no process left to send
  the note-offs.

## 1.1.1 - 2026-11-03

### Added

- Optional linked-source restrictions in Config: "Disable knobs while linked"
  and "Disable reverse function (R Button) while linked". Both are off by
  default, so linked sources keep the Audience knob and R active and can still
  change the A/B ratio or swap the mixes.

### Changed

- Config repaints at 20 FPS instead of 60, and minimized windows drop to
  1 FPS until they are restored.
- The `image` dependency now builds PNG support only, dropping unused codecs
  and a large part of the dependency tree.
- Removed duplicated identity constants, unused identity/config items and the
  dead websocket command channel.
- Cleared the remaining clippy warnings; the workspace now builds clean with
  `-D warnings`.

### Fixed

- Autostart now follows "Start minimized" instead of always passing `--quiet`.
  Existing entries are updated when the setting changes and reconciled at
  startup.
- The MIDI indicator and Status line now reflect the connection the app
  actually opened, not just whether a port with the right name exists.
- Opening Config no longer blanks the mixer LEDs while it re-reads the knob
  mode.
- Hex, `wpctl` and `pactl` no longer block the async runtime. They run with
  timeouts and are killed if they hang, and `xdg-open` is reaped in the
  background.
- The desktop entry quotes the binary path, so install paths containing
  spaces work.

### Build

- The AppImage build pins appimagetool 1.9.1 and verifies its SHA-256. Native
  and Docker builds share the same pinned tool.

## 1.1.0 - 2026-11-03

### Added

- Endless knobs through the mixer's relative knob mode. The app reads the
  mode from the mixer and, once the preset used by the app is relative,
  every detent is one step in either direction, so the knobs never reach
  the 0/127 limit.
- An "Endless knobs" checkbox in Config that reflects the mixer's detected
  mode. Switching it shows a warning, saves a full mixer backup, writes
  only the eight knob records of the preset the app uses and verifies the
  result. The info panel can restore the last backup.
- Toggle Hex recording: press the record button once to start dictating
  and again to stop. It is available in Config and the tray, and hold
  remains the default.
- Mix-coloured accents when channel colours are on: values, fader ticks
  and the knob pointer carry the A/B colour, and hover glows follow the
  mix being edited.

### Changed

- Hex mode is off by default.
- Soft takeover now sits under Features; About sits under Startup; the
  separate Surface section is gone and the off-surface list lives with
  its setting.
- The link LED and the on-screen link button now light when a strip is
  linked, instead of when it is unlinked.
- The tray no longer has an Endless Knobs item. It gains Toggle Hex
  Recording when Hex mode is on.
- Wording now describes the knob mode as the mixer's internal setting,
  not a Pipeweaver setting.

### Fixed

- Recover from a Pipeweaver status broadcast lag: fetch a full status and
  keep consuming instead of freezing the surface.
- Keep queued commands when one HTTP command times out, so mutes, links,
  swaps and routes are not silently discarded.
- Turn the LEDs off before reconnecting or switching MIDI device.
- Reset soft takeover after on-screen R, link and target A/B changes.
- Keep the window reachable when no tray is available: start visible,
  quit on close instead of hiding, and reopen if the tray disappears.
- Stop Hex dictation on quit, MIDI mode changes and mixer loss, while
  hiding to the tray or opening Config no longer interrupts it.
- Fix the Reset size button hitbox so only the visible button responds.

## 1.0.0 - 2026-11-02

First release.

### Added

- Dedicated M-VAVE SMC-Mixer control surface for Pipeweaver.
- Eight strips with source/target split, faders, knobs, mutes, swap and link.
- Target volume, mute and A/B routing.
- Off-surface drawer for channels that do not fit on the eight strips.
- Pipeweaver connection, live names, colours, volumes, mutes, mixes and meters.
- Transport controls through MPRIS: play, pause, seek, previous and next.
- Record button for Hex dictation, with an optional microphone-mute mode.
- Split up/down, app show/hide and Pipeweaver shortcut on the arrow buttons.
- Tray menu with MIDI connection, soft takeover and Hex mode.
- Autostart, start minimized and startup LED animation.
- Soft takeover, light theme, channel colours and off-surface drawer toggle.
- Offline Mixer screen and LEDs that turn off when Pipeweaver disconnects or the app stops.
- AppImage and Docker Compose build paths.

### Fixed

- Formatted the core crate so `cargo fmt --check` passes.
- Corrupt config files are backed up as `.json.bad` instead of being silently overwritten.
- Config saves from the UI and tray are serialized so they cannot race.
