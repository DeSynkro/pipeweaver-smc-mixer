# Releasing

This is the maintainer checklist for cutting a release. The changelog is in
[CHANGELOG.md](CHANGELOG.md).

## Version

The current version is in `VERSION`.

Update these together when the version changes:

- `Cargo.toml` — `version = "<version>"`
- `VERSION` — `<version>`
- `README.md` — the version badge
- `CHANGELOG.md` — add the new section

## Tag

Create the tag manually after the release commit:

```sh
git tag -a v<version> -m "Pipeweaver SMC Mixer <version>"
git push codeberg v<version>
```

## Build

Both build paths are supported. The Docker build is the recommended release
artifact because it uses Debian bookworm and a glibc 2.36 baseline. The native
build links against the host and may require a newer glibc.

Docker:

```sh
./build/build.sh docker
```

Output:

```
build/docker/out/appimage/Pipeweaver-SMC-Mixer-x86_64.AppImage
```

Native:

```sh
./build/build.sh appimage
```

Output:

```
build/appimage/Pipeweaver-SMC-Mixer-x86_64.AppImage
```

Collect the artifacts under versioned names:

```sh
mkdir -p build/release
cp build/docker/out/appimage/Pipeweaver-SMC-Mixer-x86_64.AppImage \
   build/release/Pipeweaver-SMC-Mixer-<version>-x86_64.AppImage
cp build/appimage/Pipeweaver-SMC-Mixer-x86_64.AppImage \
   build/release/Pipeweaver-SMC-Mixer-<version>-native-x86_64.AppImage
```

If you only ship one, use the Docker build and the versioned filename without
the `-native` suffix.

## Verify

Run the automated checks:

```sh
cargo fmt --check
cargo test
```

Then launch the release AppImage and check:

- the window opens and the tray works;
- Config opens and the Pipeweaver status is correct;
- Pipeweaver connects and reconnects;
- MIDI controls respond;
- autostart works from the AppImage;
- the hardware parity checklist has been run.

## Checksums

A `SHA256SUMS` file lets people verify the download. It is optional, but easy
to produce:

```sh
cd build/release
sha256sum *.AppImage > SHA256SUMS
```

A user can verify with:

```sh
sha256sum -c SHA256SUMS
```

## Upload

Create a release on Codeberg and attach:

- the AppImage(s);
- `SHA256SUMS`;
- the text from `CHANGELOG.md`.

The release tag should match the git tag, for example `v<version>`.

## Bookkeeping

- `Cargo.lock` is committed.
- No files from `build/` are staged; generated output is ignored.
- `LICENSE` is present and the project is MIT.
- Credits are in the README.
- The old app is kept installed for rollback until the new release is proven.
