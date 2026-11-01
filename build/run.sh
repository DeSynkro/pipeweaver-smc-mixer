#!/usr/bin/env bash
# Run the local build, building it first if needed.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

bin="build/binary/pipeweaver-smc-mixer"
if [ ! -x "$bin" ]; then
    ./build/build.sh release
fi

# KWin consumes the first click of a native Wayland window to focus it, and
# drops focus after a compositor-driven move. Run as an XWayland client so the
# panel behaves like a normal window. winit picks X11 when Wayland is not set.
if [ -n "${DISPLAY:-}" ]; then
    unset WAYLAND_DISPLAY
fi

exec "$bin" "$@"
