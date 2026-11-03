# Roadmap

Post-1.0 work. 1.0 covered the app, the AppImage/Docker builds and the
initial documentation. 1.1 added endless knobs and the Hex toggle; 1.1.1
added the first batch of fixes and cleanup.

## Later

- Decide and fix off-surface R semantics while linked. Linked mode is a
  ratio, so the current two-command swap may not land cleanly.
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
