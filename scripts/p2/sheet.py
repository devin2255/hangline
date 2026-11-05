#!/usr/bin/env python3
"""P2-9 contact-sheet compositor (dev-only, no app runtime).

Requires Pillow (`pip install pillow`) — the repository itself has no Python dependency; this
script is only used when regenerating `docs/experiments/P2-contact-sheet.png` from the frames that
`scripts/p2/compare.mjs` writes to `docs/experiments/p2-frames/`.

Usage (from the repository root):
    python scripts/p2/sheet.py
"""

from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[2]
FRAMES = ROOT / "docs" / "experiments" / "p2-frames"
OUT = ROOT / "docs" / "experiments" / "P2-contact-sheet.png"

# (file, label) in display order; only our own app frames, never Tendedero assets.
FRAMES_ORDER = [
    ("reveal.png", "reveal spring"),
    ("hover.png", "hover"),
    ("fly-landed.png", "fly handoff landed"),
    ("fall-after.png", "fall (after)"),
]

GAP = 18
PAD = 20
LABEL_H = 22
BACKGROUND = (18, 21, 27)
PANEL_BG = (24, 28, 35)


def main() -> None:
    images = []
    for name, label in FRAMES_ORDER:
        path = FRAMES / name
        if not path.is_file():
            raise SystemExit(f"missing frame: {path}")
        images.append((Image.open(path).convert("RGBA"), label))
    width = sum(image.width for image, _ in images) + GAP * (len(images) - 1) + PAD * 2
    height = max(image.height for image, _ in images) + PAD * 2 + LABEL_H
    canvas = Image.new("RGB", (width, height), BACKGROUND)
    draw = ImageDraw.Draw(canvas)
    x = PAD
    for image, label in images:
        panel = Image.new("RGBA", image.size, (*PANEL_BG, 255))
        panel.alpha_composite(image)
        canvas.paste(panel.convert("RGB"), (x, PAD + LABEL_H))
        draw.text((x, PAD + 4), label, fill=(220, 228, 236))
        x += image.width + GAP
    canvas.save(OUT)
    print(f"wrote {OUT} {canvas.size}")


if __name__ == "__main__":
    main()
