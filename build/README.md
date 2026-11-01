# Build directory

Everything Cargo and the packaging tools produce lives here.

- `build/target/`: Cargo's target directory
- `build/binary/pipeweaver-smc-mixer`: the copied executable
- `build/build.sh`: build helper, release by default
- `build/run.sh`: run helper, builds first if needed

The target directory is configured in `.cargo/config.toml`, the only file that
must stay outside this folder.
