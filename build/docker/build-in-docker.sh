#!/usr/bin/env bash
# Build the app in a Docker Compose container without installing the Rust
# toolchain or the GUI/audio development headers on the host.
#
#   ./build/docker/build-in-docker.sh            # menu
#   ./build/docker/build-in-docker.sh binary     # binary only
#   ./build/docker/build-in-docker.sh appimage   # AppImage (also builds the binary)
#   ./build/docker/build-in-docker.sh both       # binary + AppImage
#
# Compose selection:
#   - COMPOSE="sudo docker compose" ./build/docker/build-in-docker.sh ...
#   - plain "docker compose" when it works without sudo
#   - "sudo docker compose" otherwise
#   - DOCKER="sudo docker" is also accepted
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
compose_file="$root/build/docker/compose.yaml"
mode="${1:-}"

if [ -z "$mode" ]; then
    if [ -t 0 ]; then
        while true; do
            echo "What should I build?"
            echo "  1) Binary + AppImage (default)"
            echo "  2) Binary only"
            echo "  3) AppImage only"
            read -r -p "Choice [1]: " choice || choice=""
            case "${choice:-1}" in
                1 | "") mode=both; break ;;
                2) mode=binary; break ;;
                3) mode=appimage; break ;;
                *) echo "Enter 1, 2 or 3." ;;
            esac
        done
    else
        mode=both
    fi
fi

case "$mode" in
    binary | appimage | both) ;;
    *)
        echo "usage: $0 [binary|appimage|both]" >&2
        exit 2
        ;;
esac

if [ -n "${COMPOSE:-}" ]; then
    read -r -a compose_cmd <<< "$COMPOSE"
elif [ -n "${DOCKER:-}" ]; then
    read -r -a compose_cmd <<< "$DOCKER compose"
elif docker info >/dev/null 2>&1 && docker compose version >/dev/null 2>&1; then
    compose_cmd=(docker compose)
elif docker info >/dev/null 2>&1 && command -v docker-compose >/dev/null 2>&1; then
    compose_cmd=(docker-compose)
elif command -v sudo >/dev/null 2>&1 && sudo docker compose version >/dev/null 2>&1; then
    compose_cmd=(sudo docker compose)
elif command -v sudo >/dev/null 2>&1 \
    && command -v docker-compose >/dev/null 2>&1 \
    && sudo docker-compose version >/dev/null 2>&1; then
    compose_cmd=(sudo docker-compose)
else
    echo "Docker Compose is not available or Docker is not usable." >&2
    echo "Set COMPOSE='...' or DOCKER='...' to override." >&2
    exit 1
fi

image="${IMAGE:-pipeweaver-smc-mixer-build}"
cache="${CARGO_CACHE_DIR:-$root/build/docker/cargo-cache}"
out="${DOCKER_OUT_DIR:-$root/build/docker/out}"
mkdir -p "$cache" "$out"
export DOCKER_UID="$(id -u)"
export DOCKER_GID="$(id -g)"

echo "Using: ${compose_cmd[*]}"
echo "Image: $image"
echo "Mode:  $mode"

"${compose_cmd[@]}" -f "$compose_file" -p pipeweaver-smc-mixer build

case "$mode" in
    binary)
        build_command="./build/build.sh release"
        ;;
    appimage | both)
        # The AppImage script builds the release binary as its first step.
        build_command="./build/appimage/build-appimage.sh"
        ;;
esac

"${compose_cmd[@]}" -f "$compose_file" -p pipeweaver-smc-mixer run --rm -T build \
    bash -c "$build_command"

echo
echo "Outputs:"
echo "  $out/binary/pipeweaver-smc-mixer"
if [ "$mode" != "binary" ]; then
    echo "  $out/appimage/Pipeweaver-SMC-Mixer-x86_64.AppImage"
fi
