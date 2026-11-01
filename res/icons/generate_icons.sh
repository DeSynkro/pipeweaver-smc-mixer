#!/usr/bin/env bash
# Render the app icon sizes used by the window, tray, desktop entry and packaging.
set -euo pipefail

cd "$(dirname "$0")"

rsvg-convert -w 1024 icon.svg -o Icon.png
rsvg-convert -w 1024 icon-light.svg -o Icon-light.png

for size in 48 128 256 512; do
    rsvg-convert -w "${size}" icon.svg -o "pipeweaver-smc-mixer-${size}.png"
done

echo "Generated Icon.png, Icon-light.png and pipeweaver-smc-mixer-{48,128,256,512}.png"
