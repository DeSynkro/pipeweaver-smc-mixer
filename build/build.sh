#!/usr/bin/env bash
# Build the app and copy the binary into build/binary.
#
#   ./build/build.sh            # release
#   ./build/build.sh debug      # debug
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

profile="${1:-release}"
case "$profile" in
    debug)
        cargo build
        built="build/target/debug/pipeweaver-smc-mixer"
        ;;
    release)
        cargo build --release
        built="build/target/release/pipeweaver-smc-mixer"
        ;;
    *)
        echo "usage: $0 [debug|release]" >&2
        exit 2
        ;;
esac

mkdir -p build/binary
install -m 0755 "$built" build/binary/pipeweaver-smc-mixer
echo "Built: $root/build/binary/pipeweaver-smc-mixer"
