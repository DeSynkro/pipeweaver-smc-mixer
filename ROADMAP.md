# Roadmap

Post-1.0 work. 1.0 covered the app, the AppImage/Docker builds and the
initial documentation. 1.1 added endless knobs, the Hex toggle and the
first batch of fixes.

## 1.1.1

### Fixes

- Make autostart honour "Start minimized".
- Publish the actual MIDI link state instead of inferring it from port enumeration.
- Decide and fix off-surface R semantics on linked sources.
- Move blocking process calls off the Tokio runtime and add timeouts.
- Quote the desktop-entry path in `install-desktop.sh`.
- Pin and verify `appimagetool` in the AppImage build.

### Cleanup

- Fix the remaining clippy warnings.
- Remove or wire up unused identity/config items.
- Deduplicate the app name and id constants.
- Remove the dead handler command channel.

## Later

- Clip off-surface card titles.
- Throttle drawer slider commands while held.
- Only play the startup animation when an output port is open.
- Make soft takeover more tolerant of feedback lag.
- Prune meters and clear them when the meter socket closes.
- Persist window position and maximize state.
- Reap `xdg-open` child processes.
- Harden engine lock handling against poisoning and the connected/offline race.
- Security hardening: localhost trust, symlink/temp handling, MIDI port
  impersonation, websocket/MPRIS timeouts, dependency pinning, signed checksums.
- More tests: soft takeover, LED patterns, mapping handlers, config corruption,
  broadcast lag and shutdown cleanup.
