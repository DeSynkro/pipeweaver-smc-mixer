# Docker build

Builds the app inside Debian bookworm without installing the Rust toolchain or
the GUI/audio development headers on the host.

## Use

```sh
./build/docker/build-in-docker.sh
```

The wrapper prompts for binary + AppImage, binary only, or AppImage only. Pass
`binary`, `appimage` or `both` to skip the menu.

It uses plain `docker compose` when that works without sudo, otherwise
`sudo docker compose`. Set `COMPOSE="..."` or `DOCKER="..."` to override.

## Outputs

Docker builds stay separate from native builds:

- `build/docker/out/binary/pipeweaver-smc-mixer`
- `build/docker/out/appimage/Pipeweaver-SMC-Mixer-x86_64.AppImage`
- `build/docker/out/target/` — Cargo target directory used by the container
- `build/docker/cargo-cache/` — Cargo registry cache

The normal `build/binary/` and `build/appimage/` outputs are not touched by a
Docker build.

## Binary compatibility

The container is based on Debian 12 (bookworm), which uses glibc 2.36. A binary
built against glibc 2.36 requires glibc 2.36 or newer at runtime. It will not
run on a system with an older glibc.

Building natively on a newer distribution can raise that requirement further.
The AppImage does not bundle glibc or the graphics/audio libraries, so it has
the same baseline.

Docker makes the baseline explicit and repeatable, but it does not remove the
dependency. To support older distributions, change the `FROM` line to an older
base (for example Debian 11 bullseye, glibc 2.31) and test the result.

A Docker-built binary can also differ from a native build because it links
against bookworm's libraries rather than the host's.
