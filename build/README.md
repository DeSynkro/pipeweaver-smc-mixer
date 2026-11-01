# Build directory

Everything Cargo and the packaging tools produce lives here.

- `build/target/`: Cargo's target directory
- `build/binary/pipeweaver-smc-mixer`: the copied executable
- `build/build.sh`: build menu; builds the native binary, or runs the AppImage
  or Docker builder. Pass `binary`, `appimage` or `docker` to skip the menu.
- `build/run.sh`: run helper, builds first if needed
- `build/install-desktop.sh`: installs the desktop entry and hicolor icons for
  the current user, so the taskbar and launcher show the app logo
- `build/appimage/`: AppImage builder, AppDir, tools and the finished
  `Pipeweaver-SMC-Mixer-x86_64.AppImage`
- `build/appimage/build-appimage.sh`: builds the release binary and packages it
  as a single-file AppImage; the first run downloads `appimagetool`
- `build/docker/`: Dockerfile, Compose file, README and wrapper for building
  without installing the Rust toolchain or development headers on the host
- `build/docker/build-in-docker.sh`: interactive Compose build helper for the
  binary, the AppImage, or both
- `build/docker/out/`: Docker build outputs, kept separate from the native
  `build/binary/` and `build/appimage/` outputs
- `build/docker/cargo-cache/`: Cargo registry cache for the container

The target directory is configured in `.cargo/config.toml`, the only file that
must stay outside this folder.
