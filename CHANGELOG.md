# Changelog

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
