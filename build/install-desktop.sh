#!/usr/bin/env bash
# Install the desktop entry and hicolor icons for the current user.
#
#   ./build/install-desktop.sh [path-to-binary]
#
# Defaults to the local build binary. Packaging replaces this with the
# installed path.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bin="${1:-$root/build/binary/pipeweaver-smc-mixer}"

if [ ! -x "$bin" ]; then
    echo "Not executable: $bin" >&2
    exit 1
fi

icons="$HOME/.local/share/icons/hicolor"
apps="$HOME/.local/share/applications"

for size in 48 128 256 512; do
    dir="$icons/${size}x${size}/apps"
    mkdir -p "$dir"
    install -m 0644 "$root/res/icons/pipeweaver-smc-mixer-$size.png" \
        "$dir/pipeweaver-smc-mixer.png"
done

mkdir -p "$apps"
cat > "$apps/pipeweaver-smc-mixer.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Pipeweaver SMC Mixer
GenericName=Mixer control surface
Comment=Control Pipeweaver with the M-VAVE SMC-Mixer
Exec=env -u WAYLAND_DISPLAY "$bin"
Icon=pipeweaver-smc-mixer
Terminal=false
Categories=AudioVideo;Audio;Mixer;
Keywords=Pipeweaver;SMC;Mixer;Audio;
StartupWMClass=pipeweaver-smc-mixer
X-KDE-Wayland-Identifier=pipeweaver-smc-mixer
EOF

chmod 0644 "$apps/pipeweaver-smc-mixer.desktop"

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "$icons" >/dev/null 2>&1 || true
fi
if command -v kbuildsycoca6 >/dev/null 2>&1; then
    kbuildsycoca6 --noincremental >/dev/null 2>&1 || true
fi

echo "Installed $apps/pipeweaver-smc-mixer.desktop"
echo "Installed icons under $icons"
