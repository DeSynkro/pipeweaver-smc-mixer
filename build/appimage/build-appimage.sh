#!/usr/bin/env bash
# Build a single-file AppImage in build/appimage/.
#
# Requirements: Rust toolchain and network access on the first run to fetch
# appimagetool. The AppImage bundles this app only; Pipeweaver and the
# SMC-Mixer are still required at runtime.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

name="Pipeweaver-SMC-Mixer"
work_dir="${APPIMAGE_WORK_DIR:-$root/build/appimage}"
binary_dir="${BINARY_DIR:-$root/build/binary}"
appdir="$work_dir/AppDir"
tools="$work_dir/tools"
output="$work_dir/$name-x86_64.AppImage"

if [ ! -f "res/icons/pipeweaver-smc-mixer-256.png" ]; then
    ./res/icons/generate_icons.sh
fi

./build/build.sh release

rm -rf "$appdir"
mkdir -p \
    "$appdir/usr/bin" \
    "$appdir/usr/share/applications" \
    "$appdir/usr/share/icons/hicolor/256x256/apps"

install -m 0755 "$binary_dir/pipeweaver-smc-mixer" \
    "$appdir/usr/bin/pipeweaver-smc-mixer"
install -m 0644 res/icons/pipeweaver-smc-mixer-256.png \
    "$appdir/usr/share/icons/hicolor/256x256/apps/pipeweaver-smc-mixer.png"
install -m 0644 res/icons/pipeweaver-smc-mixer-256.png \
    "$appdir/pipeweaver-smc-mixer.png"

cat > "$appdir/pipeweaver-smc-mixer.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=Pipeweaver SMC Mixer
GenericName=Mixer control surface
Comment=Control Pipeweaver with the M-VAVE SMC-Mixer
Exec=pipeweaver-smc-mixer
Icon=pipeweaver-smc-mixer
Terminal=false
Categories=AudioVideo;Audio;Mixer;
Keywords=Pipeweaver;SMC;Mixer;Audio;
StartupWMClass=pipeweaver-smc-mixer
X-KDE-Wayland-Identifier=pipeweaver-smc-mixer
EOF

install -m 0644 "$appdir/pipeweaver-smc-mixer.desktop" \
    "$appdir/usr/share/applications/pipeweaver-smc-mixer.desktop"

cat > "$appdir/AppRun" <<'EOF'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
# Match build/run.sh: KWin consumes the first click of a native Wayland window.
if [ -n "${DISPLAY:-}" ]; then
    unset WAYLAND_DISPLAY
fi
exec "$HERE/usr/bin/pipeweaver-smc-mixer" "$@"
EOF
chmod 0755 "$appdir/AppRun"

mkdir -p "$tools"
tool="$tools/appimagetool-x86_64.AppImage"
if [ ! -x "$tool" ]; then
    curl -L --fail -o "$tool" \
        https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
    chmod +x "$tool"
fi

cd "$work_dir"
rm -f "$output"
APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64 "$tool" "$appdir" "$output"
chmod +x "$output"
echo "Built: $output"
