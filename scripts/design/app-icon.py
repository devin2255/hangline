#!/usr/bin/env python3
"""Render three Hangline app-icon concepts at 1024 px (with a 3x supersampled drawing canvas)."""
from __future__ import annotations

import math
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

OUT = Path(__file__).resolve().parent
SS = 3  # supersampling factor
W = 1024 * SS


def hx(value: str) -> tuple[int, int, int]:
    value = value.lstrip("#")
    return tuple(int(value[i : i + 2], 16) for i in (0, 2, 4))


def s(value: float) -> int:
    return int(round(value * SS))


def linear_gradient(c1: str, c2: str, vertical: bool = True) -> Image.Image:
    a, b = hx(c1), hx(c2)
    t = np.linspace(0.0, 1.0, W, dtype=np.float32)
    rows = np.repeat(t[:, None], W, axis=1) if vertical else np.repeat(t[None, :], W, axis=0)
    arr = np.zeros((W, W, 4), dtype=np.uint8)
    for i in range(3):
        arr[..., i] = np.clip(a[i] + (b[i] - a[i]) * rows, 0, 255).astype(np.uint8)
    arr[..., 3] = 255
    return Image.fromarray(arr, "RGBA")


def radial_glow(cx: float, cy: float, radius: float, color: str, strength: float) -> Image.Image:
    r, g, b = hx(color)
    yy, xx = np.mgrid[0:W, 0:W]
    dist = np.sqrt((xx - cx) ** 2 + (yy - cy) ** 2) / radius
    alpha = np.clip(1.0 - dist, 0.0, 1.0) ** 2 * strength
    arr = np.zeros((W, W, 4), dtype=np.uint8)
    arr[..., 0], arr[..., 1], arr[..., 2] = r, g, b
    arr[..., 3] = (alpha * 255).astype(np.uint8)
    return Image.fromarray(arr, "RGBA")


def rounded_mask(box: tuple[int, int, int, int], radius: int) -> Image.Image:
    mask = Image.new("L", (W, W), 0)
    ImageDraw.Draw(mask).rounded_rectangle(box, radius=radius, fill=255)
    return mask


def rounded_gradient(box, radius, c1, c2, vertical=True) -> Image.Image:
    layer = Image.new("RGBA", (W, W), (0, 0, 0, 0))
    layer.paste(linear_gradient(c1, c2, vertical), (0, 0), rounded_mask(box, radius))
    return layer


def blurred_shadow(box, radius, blur, alpha, offset) -> Image.Image:
    layer = Image.new("RGBA", (W, W), (0, 0, 0, 0))
    moved = (box[0] + offset[0], box[1] + offset[1], box[2] + offset[0], box[3] + offset[1])
    ImageDraw.Draw(layer).rounded_rectangle(moved, radius=radius, fill=(0, 0, 0, alpha))
    return layer.filter(ImageFilter.GaussianBlur(blur))


def bezier(start, control, end, samples=320):
    points = []
    for i in range(samples + 1):
        t = i / samples
        x = (1 - t) ** 2 * start[0] + 2 * (1 - t) * t * control[0] + t**2 * end[0]
        y = (1 - t) ** 2 * start[1] + 2 * (1 - t) * t * control[1] + t**2 * end[1]
        points.append((x, y))
    return points


def ribbon_polygon(points, width: float):
    """Smooth polygon around a polyline (avoids Pillow line-joint artifacts)."""
    half = width / 2
    left, right = [], []
    for i, (x, y) in enumerate(points):
        if i == 0:
            dx, dy = points[1][0] - x, points[1][1] - y
        elif i == len(points) - 1:
            dx, dy = x - points[-2][0], y - points[-2][1]
        else:
            dx, dy = points[i + 1][0] - points[i - 1][0], points[i + 1][1] - points[i - 1][1]
        length = math.hypot(dx, dy) or 1.0
        nx, ny = -dy / length, dx / length
        left.append((x + nx * half, y + ny * half))
        right.append((x - nx * half, y - ny * half))
    return [(x * SS, y * SS) for x, y in left + right[::-1]]


def rope(image, start, control, end, width, base, highlight, endpoint, shadow_alpha=95):
    points = bezier(start, control, end)
    polygon = ribbon_polygon(points, width)
    # soft shadow below
    shadow_layer = Image.new("RGBA", (W, W), (0, 0, 0, 0))
    ImageDraw.Draw(shadow_layer).polygon(
        [(x, y + s(11)) for x, y in polygon], fill=(0, 0, 0, shadow_alpha)
    )
    image.alpha_composite(shadow_layer.filter(ImageFilter.GaussianBlur(s(10))))
    # body with subtle top-light highlight ribbon
    ImageDraw.Draw(image).polygon(polygon, fill=base)
    highlight_polygon = []
    half = s(width / 2)
    for x, y in polygon:
        pass
    # highlight: thin ribbon along the upper half using the same normal offset
    p = [(x / SS, y / SS) for x, y in polygon]
    # rebuild from the centreline for a clean thin ribbon
    points_center = points
    left_ish, right_ish = [], []
    for i, (x, y) in enumerate(points_center):
        if i == 0:
            dx, dy = points_center[1][0] - x, points_center[1][1] - y
        elif i == len(points_center) - 1:
            dx, dy = x - points_center[-2][0], y - points_center[-2][1]
        else:
            dx, dy = points_center[i + 1][0] - points_center[i - 1][0], points_center[i + 1][1] - points_center[i - 1][1]
        length = math.hypot(dx, dy) or 1.0
        nx, ny = -dy / length, dx / length
        left_ish.append(((x + nx * (width * 0.30)) * SS, (y + ny * (width * 0.30)) * SS))
        right_ish.append(((x + nx * (width * 0.06)) * SS, (y + ny * (width * 0.06)) * SS))
    ImageDraw.Draw(image).polygon(left_ish + right_ish[::-1], fill=highlight)
    # rounded caps + brass end knobs
    draw = ImageDraw.Draw(image)
    for point in points_center[0], points_center[-1]:
        px, py = point[0] * SS, point[1] * SS
        draw.ellipse((px - s(width / 2), py - s(width / 2), px + s(width / 2), py + s(width / 2)), fill=base)
        draw.ellipse((px - s(width * 0.28), py - s(width * 0.28), px + s(width * 0.28), py + s(width * 0.28)), fill=endpoint)


def card(box, radius, top, bottom, border=2, border_color=(255, 255, 255, 60)):
    layer = Image.new("RGBA", (W, W), (0, 0, 0, 0))
    layer.paste(linear_gradient(top, bottom, vertical=True), (0, 0), rounded_mask(box, radius))
    ImageDraw.Draw(layer).rounded_rectangle(box, radius=radius, outline=border_color, width=s(border))
    return layer


def landscape(draw, box, palette, radius=8):
    x0, y0, x1, y1 = box
    mask = rounded_mask(box, s(radius))
    region = Image.new("RGBA", (W, W), (0, 0, 0, 0))
    region.paste(linear_gradient(palette["sky_top"], palette["sky_bottom"], vertical=True), (0, 0), mask)
    rdraw = ImageDraw.Draw(region)
    sr = (x1 - x0) * 0.10
    scx, scy = x0 + (x1 - x0) * 0.77, y0 + (y1 - y0) * 0.30
    rdraw.ellipse((scx - sr, scy - sr, scx + sr, scy + sr), fill=palette["sun"])
    back = [
        (x0, y1),
        (x0 + (x1 - x0) * 0.22, y0 + (y1 - y0) * 0.55),
        (x0 + (x1 - x0) * 0.42, y0 + (y1 - y0) * 0.72),
        (x0 + (x1 - x0) * 0.60, y0 + (y1 - y0) * 0.46),
        (x0 + (x1 - x0) * 0.82, y0 + (y1 - y0) * 0.66),
        (x1, y0 + (y1 - y0) * 0.58),
        (x1, y1),
    ]
    front = [
        (x0, y1),
        (x0 + (x1 - x0) * 0.20, y0 + (y1 - y0) * 0.76),
        (x0 + (x1 - x0) * 0.46, y0 + (y1 - y0) * 0.60),
        (x0 + (x1 - x0) * 0.72, y0 + (y1 - y0) * 0.78),
        (x1, y0 + (y1 - y0) * 0.70),
        (x1, y1),
    ]
    rdraw.polygon(back, fill=palette["ridge_back"])
    rdraw.polygon(front, fill=palette["ridge_front"])
    # clip everything to the rounded photo area
    region.putalpha(Image.composite(region.getchannel("A"), Image.new("L", (W, W), 0), mask))
    draw._image.alpha_composite(region)  # type: ignore[attr-defined]


def photo_card(angle, palette, card_top, card_bottom, card_box=(352, 382, 672, 834), photo_box=(378, 410, 646, 688), radius=26, shadow_alpha=105, shadow_blur=22, border_color=(255, 255, 255, 70)):
    scaled_card = tuple(s(v) for v in card_box)
    scaled_photo = tuple(s(v) for v in photo_box)
    layer = Image.new("RGBA", (W, W), (0, 0, 0, 0))
    layer.alpha_composite(blurred_shadow(scaled_card, s(radius), s(shadow_blur), shadow_alpha, (0, s(16))))
    layer.alpha_composite(card(scaled_card, s(radius), card_top, card_bottom, border=2, border_color=border_color))
    draw = ImageDraw.Draw(layer)
    landscape(draw, scaled_photo, palette, radius=8)
    x0, y0 = card_box[0], card_box[1]
    bar1 = (s(x0 + 26), s(y0 + 332), s(x0 + 26 + 176), s(y0 + 332 + 18))
    bar2 = (s(x0 + 26), s(y0 + 368), s(x0 + 26 + 112), s(y0 + 368 + 14))
    draw.rounded_rectangle(bar1, radius=s(9), fill=palette["caption"])
    draw.rounded_rectangle(bar2, radius=s(7), fill=palette["caption_faint"])
    return layer.rotate(angle, resample=Image.BICUBIC, center=(s(512), s(600)))


def clothespin(angle, box, brass_top, brass_bottom):
    x0, y0, x1, y1 = box
    w = x1 - x0
    h = y1 - y0
    scaled = (s(x0), s(y0), s(x1), s(y1))
    layer = Image.new("RGBA", (W, W), (0, 0, 0, 0))
    layer.alpha_composite(blurred_shadow(scaled, s(w * 0.30), s(12), 125, (0, s(14))))
    layer.alpha_composite(rounded_gradient(scaled, s(w * 0.30), brass_top, brass_bottom, vertical=True))
    draw = ImageDraw.Draw(layer)
    draw.rounded_rectangle(scaled, radius=s(w * 0.30), outline=(94, 58, 30, 215), width=s(3))
    draw.rounded_rectangle(
        (s(x0 + w * 0.18), s(y0 + h * 0.08), s(x0 + w * 0.42), s(y0 + h * 0.58)),
        radius=s(w * 0.12),
        fill=(255, 236, 205, 95),
    )
    # a subtle split near the bottom makes it read as a peg rather than a pill
    draw.line(
        [(s((x0 + x1) / 2), s(y0 + h * 0.72)), (s((x0 + x1) / 2), s(y1 - w * 0.16))],
        fill=(94, 58, 30, 70),
        width=s(2),
    )
    cx, cy = s((x0 + x1) / 2), s(y0 + h * 0.56)
    draw.ellipse((cx - s(w * 0.22), cy - s(w * 0.22), cx + s(w * 0.22), cy + s(w * 0.22)), fill=(86, 92, 96, 255))
    draw.ellipse((cx - s(w * 0.11), cy - s(w * 0.11), cx + s(w * 0.11), cy + s(w * 0.11)), fill=(54, 59, 63, 255))
    return layer.rotate(angle, resample=Image.BICUBIC, center=(s(512), s(600)))


def render_a():
    canvas = Image.new("RGBA", (W, W), (0, 0, 0, 0))
    tile_box = (s(44), s(44), s(980), s(980))
    tile = rounded_gradient(tile_box, s(216), "#1A2730", "#090D11", vertical=True)
    tile.alpha_composite(radial_glow(s(512), s(250), s(560), "#2E5E5C", 0.42))
    canvas.alpha_composite(tile)
    ImageDraw.Draw(canvas).rounded_rectangle(tile_box, radius=s(216), outline=(255, 255, 255, 16), width=s(2))
    rope(canvas, (128, 336), (512, 410), (896, 336), 30, "#D8AE6C", "#F5E4B8", "#B98446")
    palette = {
        "sky_top": "#A9CBE0",
        "sky_bottom": "#F2D9AE",
        "sun": "#F3B85D",
        "ridge_back": "#6F917C",
        "ridge_front": "#4C6C5D",
        "caption": "#D5CDBB",
        "caption_faint": "#E3DDCE",
    }
    canvas.alpha_composite(photo_card(-3.5, palette, "#FCF8EF", "#EFE5D2"))
    canvas.alpha_composite(clothespin(-3.5, (484, 300, 540, 424), "#F2C892", "#B77B42"))
    return canvas.resize((1024, 1024), Image.LANCZOS)




def main():
    image = render_a()
    out = Path(__file__).resolve().parents[2] / "src-tauri" / "app-icon.png"
    image.save(out)
    print(f"wrote {out} ({out.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
