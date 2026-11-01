# Changelog

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
