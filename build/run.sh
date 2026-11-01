#!/usr/bin/env bash
# Run the local build, building it first if needed.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

bin="build/binary/pipeweaver-smc-mixer"
if [ ! -x "$bin" ]; then
    ./build/build.sh
fi

exec "$bin" "$@"
