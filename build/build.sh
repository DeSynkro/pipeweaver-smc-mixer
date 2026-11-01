#!/usr/bin/env bash
# Build the app.
#
#   ./build/build.sh            # menu
#   ./build/build.sh binary     # release binary
#   ./build/build.sh release    # release binary (same as binary)
#   ./build/build.sh debug      # debug binary
#   ./build/build.sh appimage   # native AppImage
#   ./build/build.sh docker     # Docker Compose build
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

target_dir="${CARGO_TARGET_DIR:-build/target}"
binary_dir="${BINARY_DIR:-build/binary}"

build_binary() {
    local profile="${1:-release}"
    local built
    local out

    case "$profile" in
        debug)
            cargo build
            built="$target_dir/debug/pipeweaver-smc-mixer"
            ;;
        binary | release)
            cargo build --release
            built="$target_dir/release/pipeweaver-smc-mixer"
            ;;
        *)
            echo "usage: $0 [binary|release|debug|appimage|docker]" >&2
            return 2
            ;;
    esac

    out="$binary_dir"
    mkdir -p "$out"
    out="$(cd "$out" && pwd)"
    install -m 0755 "$built" "$out/pipeweaver-smc-mixer"
    echo "Built: $out/pipeweaver-smc-mixer"
}

mode="${1:-}"
if [ -z "$mode" ]; then
    if [ -t 0 ]; then
        while true; do
            echo "What would you like to build?"
            echo "  1) Binary (default)"
            echo "  2) AppImage"
            echo "  3) Docker"
            read -r -p "Choice [1]: " choice || choice=""
            case "${choice:-1}" in
                1 | "") mode=binary; break ;;
                2) mode=appimage; break ;;
                3) mode=docker; break ;;
                *) echo "Enter 1, 2 or 3." ;;
            esac
        done
    else
        mode=binary
    fi
fi

case "$mode" in
    binary | release | debug)
        build_binary "$mode"
        ;;
    appimage)
        exec ./build/appimage/build-appimage.sh
        ;;
    docker)
        exec ./build/docker/build-in-docker.sh
        ;;
    *)
        echo "usage: $0 [binary|release|debug|appimage|docker]" >&2
        exit 2
        ;;
esac
