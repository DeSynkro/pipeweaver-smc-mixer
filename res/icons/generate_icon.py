#!/usr/bin/env python3
"""Draw the Pipeweaver MIDI app icon in dark and light variants.

The mark is a simplified mixer strip: three cyan knobs over three cyan faders
with amber handles, inside a rounded app-icon tile. It is deliberately bolder
than the old illustration so the silhouette survives 48 px.

Run from the repository root:
    python3 res/icons/generate_icon.py
Then render the sizes with res/icons/generate_icons.sh.
"""

from pathlib import Path

HERE = Path(__file__).resolve().parent

DARK = {
    "bg_top": "#171D25",
    "bg_bottom": "#0A0D12",
    "tile_stroke": "#2C3542",
    "rim": "#20BBD8",
    "cyan": "#1FB6D4",
    "cyan_dark": "#0E6F84",
    "amber": "#F2A93B",
    "amber_dark": "#B87A1E",
    "thumb_line": "#0D7C93",
}

LIGHT = {
    "bg_top": "#FBFCFE",
    "bg_bottom": "#E7ECF3",
    "tile_stroke": "#C9D3DF",
    "rim": "#1496B4",
    "cyan": "#1496B4",
    "cyan_dark": "#0A6C82",
    "amber": "#E89A28",
    "amber_dark": "#B87A1E",
    "thumb_line": "#0A6C82",
}

CX = [300, 512, 724]
KNOB_Y = 268
KNOB_R = 88
TRACK_TOP = 412
TRACK_BOTTOM = 890
TRACK_W = 54
THUMB_Y = [560, 700, 630]


def icon_svg(p: dict) -> str:
    parts = [
        '<?xml version="1.0" encoding="UTF-8"?>',
        '<svg width="1024" height="1024" viewBox="0 0 1024 1024" fill="none" '
        'xmlns="http://www.w3.org/2000/svg">',
        "  <defs>",
        '    <linearGradient id="tile" x1="0" y1="0" x2="0" y2="1">',
        f'      <stop offset="0" stop-color="{p["bg_top"]}"/>',
        f'      <stop offset="1" stop-color="{p["bg_bottom"]}"/>',
        "    </linearGradient>",
        "  </defs>",
        f'  <rect x="28" y="28" width="968" height="968" rx="228" fill="url(#tile)" '
        f'stroke="{p["tile_stroke"]}" stroke-width="6"/>',
        f'  <rect x="68" y="68" width="888" height="888" rx="192" fill="none" '
        f'stroke="{p["rim"]}" stroke-width="14" opacity="0.9"/>',
    ]

    # Knobs.
    for cx in CX:
        parts.append(
            f'  <circle cx="{cx}" cy="{KNOB_Y}" r="{KNOB_R}" fill="{p["cyan_dark"]}"/>'
        )
        parts.append(
            f'  <circle cx="{cx}" cy="{KNOB_Y}" r="{KNOB_R - 16}" fill="{p["cyan"]}"/>'
        )
        parts.append(
            f'  <circle cx="{cx}" cy="{KNOB_Y - 46}" r="15" fill="{p["amber"]}"/>'
        )

    # Faders with amber handles.
    for cx, thumb_y in zip(CX, THUMB_Y):
        parts.append(
            f'  <rect x="{cx - TRACK_W // 2}" y="{TRACK_TOP}" width="{TRACK_W}" '
            f'height="{TRACK_BOTTOM - TRACK_TOP}" rx="{TRACK_W // 2}" fill="{p["cyan"]}"/>'
        )
        parts.append(
            f'  <rect x="{cx - 78}" y="{thumb_y - 60}" width="156" height="120" rx="26" '
            f'fill="{p["amber"]}"/>'
        )
        parts.append(
            f'  <rect x="{cx - 38}" y="{thumb_y - 7}" width="76" height="14" rx="7" '
            f'fill="{p["thumb_line"]}"/>'
        )

    parts.append("</svg>")
    return "\n".join(parts) + "\n"


def main() -> None:
    for name, palette in (("icon", DARK), ("icon-light", LIGHT)):
        target = HERE / f"{name}.svg"
        target.write_text(icon_svg(palette))
        print(f"Wrote {target} ({target.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
