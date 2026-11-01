<p align="center">
  <img src="res/logos/logo.webp" alt="Pipeweaver SMC Mixer" width="540">
</p>

<p align="center">
  <strong>A dedicated control surface for the M-VAVE SMC-Mixer and Pipeweaver.</strong><br>
  Eight strips, one fixed mapping, no MIDI learn.
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT"></a>
  <a href="VERSION"><img src="https://img.shields.io/badge/version-1.0.0-orange.svg" alt="Version 1.0.0"></a>
  <a href="https://ko-fi.com/desynkro"><img src="https://img.shields.io/badge/Ko--fi-support%20me-ff5e5b?logo=kofi&amp;logoColor=white" alt="Support me on Ko-fi"></a>
</p>

<p align="center">
  Built by <a href="https://desynkro.com">DeSynkro</a> with assistance from <a href="https://opencode.ai">OpenCode</a>.
</p>

Pipeweaver SMC Mixer turns the M-VAVE SMC-Mixer into a dedicated hardware surface for Pipeweaver. The faceplate on screen matches the mixer in front of you. The faders and knobs drive Pipeweaver's Personal and Audience mixes, the buttons handle mutes, swaps and links, and the transport row controls your media player.

This is a focused app. It only supports the SMC-Mixer and it only talks to Pipeweaver. There is no mapping editor, no plugin system and no setup wizard. Plug the mixer in, start Pipeweaver and go.

## Why this exists

When I first moved to Linux, I had to navigate my audio setup without the convenience of my old Beacn Mix Create, as Beacn never release a Linux build.

I looked up online and ended up finding the SMC-Mixer by M-VAVE, a small, affordable controller with eight strips, eight knobs and a row of transport buttons. Fantastic little Audio Midi Table.

I spent weeks trying to get it to work with Linux and to my dismay, I never managed to get it working, until a gentleman by the name of FrostyCoolSlug released [Pipeweaver](https://github.com/pipeweaver/pipeweaver).

Pipeweaver is a flexible audio mixer for Linux and I was able to pair them well together. The mapping is fixed, the labels follow Pipeweaver, and the screen looks like the device. 
If you own an SMC-Mixer and use Pipeweaver, it should feel like the mixer was made for it.

It's a very niche software for a niche situation, but I thought i'd share if anyone wants to have a go in making it compatible with their own device or falls under the same niche I did.

## Requirements

- **Linux** with Wayland or X11.
- **Pipeweaver** installed and running. The app talks to it on `localhost:14565`.
- **M-VAVE SMC-Mixer**, connected over USB or Bluetooth.
- **The SMC preset** with the CC and LED mapping this app expects. Setup notes will live in `docs/preset.md` once the mapping is confirmed.
- For the AppImage: a recent glibc. Builds are made on Arch, so older distributions may need to build from source.

## Quick Start

There is no release yet. The app is being rebuilt in milestones.

Build and run from source:

```sh
./build/build.sh
./build/install-desktop.sh
./build/run.sh --quiet
```

The install step adds the desktop entry and hicolor icons, so the taskbar and
launcher use the app logo instead of a generic icon.

Start Pipeweaver first. The app starts in the tray when launched with `--quiet`, or shows the window when launched normally.

When the first release is ready, the AppImage will be on the releases page:

```sh
chmod +x Pipeweaver-SMC-Mixer-x86_64.AppImage
./Pipeweaver-SMC-Mixer-x86_64.AppImage
```

## Features

- **Fixed SMC mapping:** faders, knobs, M/S/R and the square button, all on the documented CCs. No learn mode.
- **Personal and Audience:** faders control the A mix, knobs control the B mix, R swaps them per strip.
- **Targets:** target faders, mutes and explicit A/B routing. The bar follows the active mix.
- **Split:** share the eight strips between sources and targets, from 8/0 down to 4/4. Default is 6/2.
- **Off-surface channels:** extra sources and targets stay reachable in a drawer inside the panel.
- **LED feedback:** M, S, R and link LEDs follow the state. The arrow LEDs stay local.
- **Transport:** play, pause, record, seek, track skip and the split buttons.
- **Record button:** tap to show or hide the window, hold for one second to open the Pipeweaver UI.
- **One window, two screens:** the mixer faceplate, and a Config screen in the same chassis.
- **Frameless and aspect locked:** the faceplate is the window. It resizes without stretching.
- **Light theme:** switch the whole app between the dark chassis and a light one.
- **Tray and autostart:** close to tray, reopen with a click, start minimized on login.
- **No telemetry:** no accounts, no analytics, no cloud.

## Setup

1. Start Pipeweaver and make sure the daemon is online.
2. Connect the SMC-Mixer over USB or Bluetooth.
3. Load the SMC preset that matches the mapping table.
4. Start Pipeweaver SMC Mixer.
5. Open Config and choose the MIDI port and connection mode (Auto, Wired or Bluetooth).

Auto prefers the wired port and falls back to Bluetooth. The mixer can stay connected while you switch modes; the app reconnects automatically.

## What data is stored

The app keeps a small config file at:

```
~/.config/pipeweaver/pipeweaver-smc-mixer.json
```

It stores the MIDI port, connection mode, mix labels, split, window width, light theme choice and start-minimized preference. Nothing else.

No audio, no daemon data and no telemetry leave your machine. The app only talks to Pipeweaver on localhost and to the mixer over MIDI.

## Tech Stack

| Part | Technology |
| --- | --- |
| Language | Rust |
| UI | egui and eframe |
| MIDI | midir |
| Pipeweaver | Pipeweaver IPC and websocket crates |
| Media control | MPRIS through zbus |
| Tray | ksni |
| Packaging | AppImage |

## Troubleshooting

### Pipeweaver shows as offline

Start the Pipeweaver daemon and check that it answers on `localhost:14565`. The app reconnects on its own when the daemon comes back.

### No MIDI port in Config

Check the cable or the Bluetooth pairing, then refresh the app. Auto mode tries the wired port first.

### The buttons do nothing

The mixer is probably not running the right preset. The app expects the CC and LED numbers in its fixed mapping. Load the preset and try again.

### The AppImage does not start

It was built on a newer distribution. Build from source on your machine if your glibc is older.

### The window ends up in a tile

Pipeweaver SMC Mixer is a fixed-aspect device panel, so it opens with a fixed
size. Krohnkite and similar tiling managers treat that as a floating window and
leave it alone. If yours still tiles it, add an exception for
`pipeweaver-smc-mixer` in the tiling settings.


## Support

If this is useful to you, you can support me on [Ko-fi](https://ko-fi.com/desynkro). More of my work is at [desynkro.com](https://desynkro.com).

## License

MIT. See [LICENSE](LICENSE).

## Credits

- Built by [DeSynkro](https://desynkro.com)
- Assisted by [OpenCode](https://opencode.ai)
- Faceplate artwork drawn by my wife, adapted for this app
- [Pipeweaver](https://github.com/pipeweaver/pipeweaver) by the Pipeweaver project
- M-VAVE for the SMC-Mixer
- Thank you to everyone who tested, listened and helped 😄️

<p align="center">
  <a href="https://desynkro.com" target="_blank">
    <img src="res/logos/desynkro.png" width="240"
         alt="DeSynkro"
         style="background:#0d1117;border-radius:8px;padding:8px 14px">
  </a>
</p>
