#!/usr/bin/env python3
"""Generate the SMC-Mixer shell art used by the app.

The original artwork lives in SMC-MIXER.svg. This script keeps the traced
transport icons and rebuilds the eight strips from one set of measurements,
then wraps everything in a top bezel for the window chrome.

Geometry (SVG units, viewBox 1987x967):
  bezel             y 2..56
  control area      y 56..965 (the original 911 high faceplate)
  strip i origin x  = 100.5 + 215.0 * i  (left edge of the fader well)
  fader well        (origin, 263.5 + 56, 102, 511)
  knob              centre (origin + 51.5, 114 + 56), r 73.5
  navigation plates (BT and SHIFT positions + 56)
  transport         (160.5 + 161*i, 814.5 + 56, 86, 40)

Run from the repository root:
    python3 res/svg/generate_faceplates.py
"""

from pathlib import Path
import math
import re

HERE = Path(__file__).resolve().parent
SOURCE = HERE / "SMC-MIXER.svg"

VIEW_W = 1987
VIEW_H = 967
TOP = 56

STRIP_X0 = 100.5
STRIP_PITCH = 215.0
FADER_Y = 263.5
FADER_W = 102.0
FADER_H = 511.0
SLOT_DX = 43.5
SLOT_W = 15.0
SLOT_TOP = 294.0
SLOT_H = 451.0
KNOB_DX = 51.5
KNOB_Y = 114.0
KNOB_R = 73.5
BTN_DX = 128.0
BTN_SIZE = 61.0
BTN_Y = [308.5, 424.5, 540.5, 656.5]

TRANSPORT_X0 = 160.5
TRANSPORT_PITCH = 161.0
TRANSPORT_Y = 814.5
TRANSPORT_W = 86.0
TRANSPORT_H = 40.0

DARK = {
    "name": "dark",
    "shell": "#0d1014",
    "shell_line": "#1e252e",
    "shell_edge": "#2f3742",
    "panel": "#0d1014",
    "panel_line": "#1e252e",
    "edge": "#2f3742",
    "well": "#101419",
    "well_line": "#2c343f",
    "slot": "#07090c",
    "slot_line": "#232a33",
    "tick": "#2a333e",
    "plate": "#0c0f13",
    "knob": "#141922",
    "knob_ring": "#3a4552",
    "knob_face": "#11161d",
    "icon": "#c6ceda",
    "face": "#151a21",
}

LIGHT = {
    "name": "light",
    "shell": "#eef2f7",
    "shell_line": "#dbe3ec",
    "shell_edge": "#c3ceda",
    "panel": "#ffffff",
    "panel_line": "#e6ebf1",
    "edge": "#c7d1dd",
    "well": "#f4f7fa",
    "well_line": "#d5dde7",
    "slot": "#e3e9f0",
    "slot_line": "#cbd4df",
    "tick": "#c2cbd7",
    "plate": "#ffffff",
    "knob": "#eef2f7",
    "knob_ring": "#b9c5d3",
    "knob_face": "#f8fafc",
    "icon": "#3c4756",
    "face": "#f8fafd",
}


def read_transport(source: str) -> list[tuple[str, str]]:
    """Return (button path, icon path) pairs ordered left to right."""
    lines = source.splitlines()
    pairs = []
    for index in range(3, 25, 2):
        pairs.append((lines[index], lines[index + 1]))

    def button_x(line: str) -> float:
        match = re.search(r'd="M([0-9.]+)', line)
        return float(match.group(1)) if match else 0.0

    pairs.sort(key=lambda pair: button_x(pair[0]))
    return pairs


def read_bt_shift(source: str) -> list[str]:
    lines = source.splitlines()
    candidates = [
        lines[i]
        for i in range(len(lines) - 1)
        if 'fill="#D9D9D9" stroke="black"' in lines[i]
        and "1777.5" in lines[i]
    ]
    if len(candidates) != 2:
        raise SystemExit(f"expected BT and SHIFT plates, found {len(candidates)}")
    return candidates


def recolour(path: str, palette: dict, fill: str, stroke: str | None = None) -> str:
    path = re.sub(r'fill="#D9D9D9"', f'fill="{fill}"', path)
    if stroke is None:
        path = re.sub(r'stroke="black"', "", path)
        path = re.sub(r'fill="black"', f'fill="{palette["icon"]}"', path)
    else:
        path = re.sub(r'stroke="black"', f'stroke="{stroke}"', path)
    return path


def fmt(value: float) -> str:
    return f"{value:g}"


def strip_svg(index: int, palette: dict) -> str:
    base = STRIP_X0 + STRIP_PITCH * index
    knob_x = base + KNOB_DX
    slot_x = base + SLOT_DX
    out = [f'  <g id="strip-{index + 1}">']

    out.append(
        f'    <circle cx="{fmt(knob_x)}" cy="{fmt(KNOB_Y)}" r="{fmt(KNOB_R)}" '
        f'fill="{palette["knob"]}" stroke="{palette["edge"]}" stroke-width="3"/>'
    )
    out.append(
        f'    <circle cx="{fmt(knob_x)}" cy="{fmt(KNOB_Y)}" r="59" '
        f'fill="{palette["knob_face"]}" stroke="{palette["knob_ring"]}" stroke-width="2"/>'
    )
    for step in range(11):
        angle = math.radians(-135.0 + 27.0 * step)
        x1 = knob_x + math.sin(angle) * 63.0
        y1 = KNOB_Y - math.cos(angle) * 63.0
        x2 = knob_x + math.sin(angle) * 69.0
        y2 = KNOB_Y - math.cos(angle) * 69.0
        out.append(
            f'    <line x1="{fmt(x1)}" y1="{fmt(y1)}" x2="{fmt(x2)}" y2="{fmt(y2)}" '
            f'stroke="{palette["tick"]}" stroke-width="2" stroke-linecap="round"/>'
        )

    out.append(
        f'    <rect x="{fmt(base)}" y="{fmt(FADER_Y)}" '
        f'width="{fmt(FADER_W)}" height="{fmt(FADER_H)}" rx="14" '
        f'fill="{palette["well"]}" stroke="{palette["edge"]}" stroke-width="2"/>'
    )
    out.append(
        f'    <rect x="{fmt(slot_x)}" y="{fmt(SLOT_TOP)}" '
        f'width="{fmt(SLOT_W)}" height="{fmt(SLOT_H)}" rx="7" '
        f'fill="{palette["slot"]}" stroke="{palette["slot_line"]}" stroke-width="1"/>'
    )
    for step in range(11):
        y = SLOT_TOP + SLOT_H * step / 10.0
        out.append(
            f'    <line x1="{fmt(base + 31)}" y1="{fmt(y)}" '
            f'x2="{fmt(base + 38)}" y2="{fmt(y)}" '
            f'stroke="{palette["tick"]}" stroke-width="1.5"/>'
        )
        out.append(
            f'    <line x1="{fmt(base + 64)}" y1="{fmt(y)}" '
            f'x2="{fmt(base + 71)}" y2="{fmt(y)}" '
            f'stroke="{palette["tick"]}" stroke-width="1.5"/>'
        )

    for y in BTN_Y:
        out.append(
            f'    <rect x="{fmt(base + BTN_DX)}" y="{fmt(y)}" '
            f'width="{fmt(BTN_SIZE)}" height="{fmt(BTN_SIZE)}" rx="10" '
            f'fill="{palette["well"]}" stroke="{palette["well_line"]}" stroke-width="2"/>'
        )

    out.append("  </g>")
    return "\n".join(out)


def build(source: str, palette: dict) -> str:
    transport = read_transport(source)
    nav_plates = read_bt_shift(source)

    parts = [
        '<?xml version="1.0" encoding="UTF-8"?>',
        f'<svg width="{VIEW_W}" height="{VIEW_H}" viewBox="0 0 {VIEW_W} {VIEW_H}" '
        'fill="none" xmlns="http://www.w3.org/2000/svg">',
        f'  <rect x="2" y="2" width="1983" height="963" rx="42" '
        f'fill="{palette["shell"]}" stroke="{palette["shell_edge"]}" stroke-width="4"/>',
        f'  <rect x="13" y="13" width="1961" height="941" rx="34" '
        f'fill="none" stroke="{palette["shell_line"]}" stroke-width="2"/>',
    ]

    if palette["name"] == "light":
        parts.append(
            f'  <rect x="13" y="{TOP}" width="1961" height="{954 - TOP}" rx="30" '
            f'fill="{palette["panel"]}" stroke="{palette["edge"]}" stroke-width="2"/>'
        )

    parts.append(
        f'  <line x1="13" y1="{TOP}" x2="1974" y2="{TOP}" '
        f'stroke="{palette["shell_edge"]}" stroke-width="2"/>'
    )

    parts.append(f'  <g transform="translate(0,{TOP})">')
    parts.extend(strip_svg(index, palette) for index in range(8))

    parts.append('  <g id="transport">')
    for button, icon in transport:
        parts.append(recolour(button, palette, palette["face"], stroke=palette["edge"]))
        parts.append(recolour(icon, palette, palette["icon"]))
    parts.append("  </g>")

    parts.append('  <g id="navigation">')
    for button in nav_plates:
        parts.append(recolour(button, palette, palette["face"], stroke=palette["edge"]))
    parts.append("  </g>")
    parts.append("  </g>")

    parts.append("</svg>")
    return "\n".join(parts) + "\n"


def main() -> None:
    source = SOURCE.read_text()
    for palette in (DARK, LIGHT):
        target = HERE / f"SMC-MIXER-{palette['name']}.svg"
        target.write_text(build(source, palette))
        print(f"Wrote {target} ({target.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
