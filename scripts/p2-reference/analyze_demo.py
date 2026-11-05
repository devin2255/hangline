"""Dev-only: measure the Tendedero demo GIF (frames, reveal/tuck timing, card boxes and tilt).

Reads `.superpowers/p2-reference/demo-light.gif` (downloaded by `fetch.mjs`) and writes
`demo-measurements.json` next to it. The GIF and the other Tendedero promotional assets are
Tendedero's (MIT + name/icon restriction); they are never committed. Requires Pillow.

Deterministic: the same GIF input produces the same JSON output (no randomness, no clock).
Nothing here is shipped with the app; it only feeds `docs/experiments/P2-reference.md`.

Method
------
The demo is a promotional recording of a full-width line: cards hang from a rope over a smooth
pastel gradient and a fake menu bar. Card content is separated from the background by a fixed
colour rule -- near-white, dark, or saturated pixels -- inside a fixed panel region (below the
menu bar and inside the recording's rounded white corners). The mouse cursor satisfies the same
rule, but it is only a few dozen pixels wide, so a card needs a >= 40 px wide column run, and
per-column top/bottom edges use the median over the run, which ignores the cursor. The three card
x-bands are fixed from the settled layout, so tracking survives the whole-line slide at
reveal/tuck (where adjacent cards can blur into one run) and the per-card swing.

Reveal/tuck timing comes from the vertical motion of the card centroids (PCA of each band): the
line starts to appear at `revealFrame`, first reaches its resting height at `revealDownFrame`
(the spring then overshoots downwards and settles), and tucks away between `tuckStartFrame` and
`tuckEndFrame`. Tilt is the PCA major-axis angle of each card band in screen degrees (clockwise
positive, matching SwiftUI `.rotationEffect`), sampled every frame while the card is present.

Coordinates: `cards[].boxPx` and `cards[].centerPx` are GIF pixels (the PANEL offset is added
back when emitting). `referenceCenterYPx` and the samples inside `tiltTracks` stay in mask
coordinates relative to the top-left of PANEL; add PANEL[0] / PANEL[1] to compare them with GIF
pixels. The script guards the expected input (960x340, 130 frames) and exits with a clear
`SystemExit` message when the GIF or a detection step does not match.

Usage: python scripts/p2-reference/analyze_demo.py
"""

import json
import math
import os
import statistics

from PIL import Image, ImageChops

HERE = os.path.dirname(os.path.abspath(__file__))
REF_DIR = os.path.normpath(os.path.join(HERE, "..", "..", ".superpowers", "p2-reference"))
GIF_PATH = os.path.join(REF_DIR, "demo-light.gif")
OUT_PATH = os.path.join(REF_DIR, "demo-measurements.json")

# Part of the GIF that contains the cards: below the fake menu bar, inside the rounded corners.
PANEL = (8, 42, 952, 316)
PANEL_X, PANEL_Y = PANEL[0], PANEL[1]
MASK_W, MASK_H = PANEL[2] - PANEL[0], PANEL[3] - PANEL[1]

# Card content detection (see module docstring). Tuned by eye against frames 0/45/80/100.
DARK_MAX = 115  # max(r, g, b) below this -> dark content (chart card, text, cursor)
WHITE_MIN = 246  # min(r, g, b) above this -> near-white content (paper card, glass rim)
SATURATION_MIN = 70  # max(r, g, b) - min(r, g, b) above this -> saturated content (photo)

MIN_RUN_COLUMNS = 8  # a card column must contain at least this many content pixels
MIN_RUN_WIDTH = 40  # ... and a card must be at least this many columns wide
MIN_CARD_AREA = 300  # a band with fewer content pixels is treated as empty
MIN_PILL_PIXELS = 50  # dark text pixels under the middle card -> the Copied pill is visible
SWAY_TOLERANCE_PX = 2.0  # centroid distance to the resting height that counts as settled
SETTLE_HOLD_FRAMES = 10  # ... and it must stay there this many frames
SAMPLE_FRAMES_AFTER_SETTLE = 8  # measure card boxes once the arrival swing has mostly died out

# The measurement targets the official `docs/images/demo-light.gif` exactly; anything else is a
# setup error and must fail loudly instead of producing plausible-looking but wrong numbers.
EXPECTED_SIZE = (960, 340)
EXPECTED_FRAMES = 130


def load_frames(path):
    """Return (size, rgb_frames, durations_ms) for every frame of the GIF."""
    gif = Image.open(path)
    size = gif.size
    frames = []
    durations = []
    for index in range(gif.n_frames):
        gif.seek(index)
        durations.append(int(gif.info.get("duration", 40)))
        # Copy because Pillow reuses/advances the internal frame buffer on the next seek.
        frames.append(gif.convert("RGB").copy())
    gif.close()
    return size, frames, durations


def content_mask(frame):
    """Binary L mask (255 = card content) for one frame inside PANEL (mask coordinates)."""
    region = frame.crop(PANEL)
    red, green, blue = region.split()
    high = ImageChops.lighter(ImageChops.lighter(red, green), blue)
    low = ImageChops.darker(ImageChops.darker(red, green), blue)
    dark = high.point(lambda value: 255 if value < DARK_MAX else 0)
    white = low.point(lambda value: 255 if value > WHITE_MIN else 0)
    saturated = ImageChops.subtract(high, low).point(
        lambda value: 255 if value > SATURATION_MIN else 0
    )
    return ImageChops.lighter(ImageChops.lighter(dark, white), saturated)


def column_counts(mask):
    """Approximate number of content pixels per column (BOX resize of a binary mask)."""
    profile = mask.resize((MASK_W, 1), Image.BOX)
    return [round(value * MASK_H / 255.0) for value in profile.tobytes()]


def column_runs(counts):
    """Contiguous column runs holding at least MIN_RUN_COLUMNS content pixels."""
    runs = []
    for x, count in enumerate(counts):
        if count >= MIN_RUN_COLUMNS:
            if runs and x == runs[-1][1] + 1:
                runs[-1][1] = x
            else:
                runs.append([x, x])
    return [(start, end) for start, end in runs if end - start + 1 >= MIN_RUN_WIDTH]


def area_of(mask):
    """Exact count of content pixels."""
    return mask.histogram()[255]


def moments(mask, band):
    """Pixel moments of the content inside a band; None when the band is empty.

    Returns count, centroid (mask coordinates) and the PCA major-axis angle in screen degrees
    (clockwise positive, i.e. the SwiftUI `.rotationEffect` sign).
    """
    left, top, right, _bottom = band
    width = right - left
    data = mask.crop(band).tobytes()
    count = 0
    sum_x = sum_y = sum_xx = sum_yy = sum_xy = 0
    position = data.find(255)
    while position != -1:
        x = position % width
        y = position // width
        count += 1
        sum_x += x
        sum_y += y
        sum_xx += x * x
        sum_yy += y * y
        sum_xy += x * y
        position = data.find(255, position + 1)
    if count < MIN_CARD_AREA:
        return None
    cx = sum_x / count
    cy = sum_y / count
    cxx = sum_xx / count - cx * cx
    cyy = sum_yy / count - cy * cy
    cxy = sum_xy / count - cx * cy
    return {
        "count": count,
        "cx": left + cx,
        "cy": top + cy,
        "tiltDeg": math.degrees(0.5 * math.atan2(2 * cxy, cxx - cyy)),
    }


def card_box(mask, band):
    """Card box in mask coordinates, robust to the mouse cursor.

    The x extent comes from columns that hold enough content to be a card edge (the cursor is
    only a few columns wide); the y edges are medians over those columns, so a cursor sitting on
    the card cannot move them.
    """
    left, top, right, _bottom = band
    width = right - left
    data = mask.crop(band).tobytes()
    col_top = [None] * width
    col_bottom = [None] * width
    col_count = [0] * width
    position = data.find(255)
    while position != -1:
        x = position % width
        y = position // width
        col_count[x] += 1
        if col_top[x] is None:
            col_top[x] = y
        col_bottom[x] = y
        position = data.find(255, position + 1)
    card_columns = [x for x in range(width) if col_count[x] >= MIN_RUN_COLUMNS]
    if not card_columns:
        return None
    x0, x1 = card_columns[0], card_columns[-1]
    tops = [col_top[x] for x in card_columns]
    bottoms = [col_bottom[x] for x in card_columns]
    return {
        "x0": left + x0,
        "x1": left + x1,
        "y0": top + int(statistics.median(tops)),
        "y1": top + int(statistics.median(bottoms)),
    }


def busy_ratio(frame):
    """Share of saturated or dark pixels at 240x85 -- the heuristic from the task brief."""
    pixels = frame.resize((240, 85)).load()
    busy = 0
    for y in range(85):
        for x in range(240):
            red, green, blue = pixels[x, y]
            if max(red, green, blue) - min(red, green, blue) > 40 or (
                red + green + blue
            ) / 3 < 90:
                busy += 1
    return round(busy / (240 * 85), 4)


def main():
    size, frames, durations = load_frames(GIF_PATH)
    if size != EXPECTED_SIZE or len(frames) != EXPECTED_FRAMES:
        raise SystemExit(
            "analyze_demo: unexpected GIF "
            f"({size[0]}x{size[1]}, {len(frames)} frames); expected "
            f"{EXPECTED_SIZE[0]}x{EXPECTED_SIZE[1]}, {EXPECTED_FRAMES} frames "
            "(run fetch.mjs, do not point this at another asset)"
        )
    masks = [content_mask(frame) for frame in frames]
    areas = [area_of(mask) for mask in masks]
    frame_ms = durations[0]

    def ms(index):
        return sum(durations[:index])

    # --- global card presence -------------------------------------------------------------
    present = [area >= MIN_CARD_AREA for area in areas]
    reveal_frame = next(index for index, has_card in enumerate(present) if has_card)
    tuck_end_frame = next(
        index for index in range(len(present) - 1, -1, -1) if present[index]
    )

    # --- fixed card x-bands from an early settled frame -----------------------------------
    band_frame = next(
        index
        for index in range(reveal_frame + 18, len(frames))
        if len(column_runs(column_counts(masks[index]))) == 3
    )
    runs = column_runs(column_counts(masks[band_frame]))
    mids = [(runs[card][1] + runs[card + 1][0]) // 2 for card in range(len(runs) - 1)]
    bands = [
        (max(0, runs[0][0] - 15), 0, mids[0] + 1, MASK_H),
        (mids[0] + 1, 0, mids[1] + 1, MASK_H),
        (mids[1] + 1, 0, min(MASK_W, runs[-1][1] + 16), MASK_H),
    ]

    # --- per-frame per-card tracks (mask coordinates) -------------------------------------
    tracks = [[] for _ in bands]
    for index, mask in enumerate(masks):
        for card, band in enumerate(bands):
            stats = moments(mask, band)
            if stats is None:
                continue
            tracks[card].append(
                {
                    "frame": index,
                    "tiltDeg": round(stats["tiltDeg"], 2),
                    "cx": round(stats["cx"], 1),
                    "cy": round(stats["cy"], 1),
                }
            )
    if any(len(track) < 20 for track in tracks):
        raise SystemExit("analyze_demo: card tracking failed (fewer than 20 samples)")

    # --- resting height, settle, tuck -----------------------------------------------------
    reference_lo = reveal_frame + 20
    reference_hi = min(reveal_frame + 40, tuck_end_frame)
    reference_cy = [
        statistics.median(
            sample["cy"]
            for sample in tracks[card]
            if reference_lo <= sample["frame"] <= reference_hi
        )
        for card in range(len(bands))
    ]

    def sample_at(card, index):
        return next((entry for entry in tracks[card] if entry["frame"] == index), None)

    def settled_at(index):
        for card in range(len(bands)):
            entry = sample_at(card, index)
            if entry is None or abs(entry["cy"] - reference_cy[card]) > SWAY_TOLERANCE_PX:
                return False
        return True

    def center_offset(index):
        values = []
        for card in range(len(bands)):
            entry = sample_at(card, index)
            values.append(
                (entry["cy"] if entry is not None else reference_cy[card]) - reference_cy[card]
            )
        return statistics.median(values)

    # First frame where every card has descended to (or past) its resting height.
    reveal_down_frame = next(
        index
        for index in range(reveal_frame, tuck_end_frame)
        if all(
            (entry := sample_at(card, index)) is not None
            and entry["cy"] >= reference_cy[card]
            for card in range(len(bands))
        )
    )
    reveal_settled_frame = next(
        index
        for index in range(reveal_down_frame, tuck_end_frame - SETTLE_HOLD_FRAMES)
        if all(settled_at(other) for other in range(index, index + SETTLE_HOLD_FRAMES))
    )
    # Deepest overshoot past the resting height between first arrival and settle.
    overshoot_candidates = range(reveal_down_frame, reveal_settled_frame + 1)
    reveal_overshoot_frame = max(overshoot_candidates, key=center_offset)
    reveal_overshoot_px = center_offset(reveal_overshoot_frame)
    tuck_start_frame = next(
        index
        for index in range(reveal_settled_frame + 1, tuck_end_frame)
        if all(
            (entry := sample_at(card, index)) is not None
            and entry["cy"] < reference_cy[card] - 3
            for card in range(len(bands))
        )
    )

    # --- representative card geometry -----------------------------------------------------
    sample_frame = next(
        index
        for index in range(reveal_settled_frame + SAMPLE_FRAMES_AFTER_SETTLE, tuck_start_frame)
        if len(column_runs(column_counts(masks[index]))) == 3
    )
    cards = []
    for card, band in enumerate(bands):
        box = card_box(masks[sample_frame], band)
        stats = moments(masks[sample_frame], band)
        cards.append(
            {
                "index": card,
                "boxPx": [
                    PANEL_X + box["x0"],
                    PANEL_Y + box["y0"],
                    PANEL_X + box["x1"],
                    PANEL_Y + box["y1"],
                ],
                "widthPx": box["x1"] - box["x0"] + 1,
                "heightPx": box["y1"] - box["y0"] + 1,
                "centerPx": [
                    round((box["x0"] + box["x1"]) / 2 + PANEL_X, 1),
                    round((box["y0"] + box["y1"]) / 2 + PANEL_Y, 1),
                ],
                "tiltDeg": round(stats["tiltDeg"], 2),
                "areaPx": stats["count"],
            }
        )

    # --- Copied pill under the middle card ------------------------------------------------
    photo = cards[1]
    photo_box_mask_bottom = photo["boxPx"][3] - PANEL_Y
    pill_region = (
        bands[1][0],
        min(photo_box_mask_bottom + 8, MASK_H - 1),
        bands[1][2],
        min(photo_box_mask_bottom + 45, MASK_H),
    )
    pill_frames = []
    for index in range(reveal_settled_frame, tuck_start_frame):
        pixels = masks[index].crop(pill_region).tobytes()
        if pixels.count(255) >= MIN_PILL_PIXELS:
            pill_frames.append(index)
    pill = None
    if pill_frames:
        pill = {
            "startFrame": pill_frames[0],
            "endFrame": pill_frames[-1],
            "startMs": ms(pill_frames[0]),
            "endMs": ms(pill_frames[-1]),
            "durationMs": ms(pill_frames[-1] + 1) - ms(pill_frames[0]),
        }

    # --- tilt summary (arrival swing + sway) ----------------------------------------------
    def tilt_range(card, start, end):
        angles = [
            entry["tiltDeg"]
            for entry in tracks[card]
            if start <= entry["frame"] <= end
        ]
        if not angles:
            raise SystemExit(
                f"analyze_demo: no tilt samples for card {card} in frames {start}-{end}"
            )
        return round(min(angles), 2), round(max(angles), 2)

    tilt_summary = []
    for card in range(len(bands)):
        swing_lo, swing_hi = tilt_range(card, reveal_down_frame, reveal_settled_frame)
        # Stop before the tuck starts: a half-clipped card still has content in the band.
        steady_lo, steady_hi = tilt_range(card, reveal_settled_frame, tuck_start_frame - 1)
        tilt_summary.append(
            {
                "card": card,
                "settledTiltDeg": cards[card]["tiltDeg"],
                "arrivalSwingDeg": [swing_lo, swing_hi],
                "steadySwayDeg": [steady_lo, steady_hi],
            }
        )

    result = {
        "source": os.path.basename(GIF_PATH),
        "tool": "scripts/p2-reference/analyze_demo.py",
        "size": [size[0], size[1]],
        "frames": len(frames),
        "durationsMs": durations,
        "totalMs": sum(durations),
        "frameIntervalMs": frame_ms,
        "panelPx": list(PANEL),
        "revealFrame": reveal_frame,
        "revealMs": ms(reveal_frame),
        "revealDownFrame": reveal_down_frame,
        "revealDownMs": ms(reveal_down_frame),
        "revealOvershootFrame": reveal_overshoot_frame,
        "revealOvershootMs": ms(reveal_overshoot_frame),
        "revealOvershootPx": round(reveal_overshoot_px, 1),
        "revealSettledFrame": reveal_settled_frame,
        "revealSettledMs": ms(reveal_settled_frame),
        "tuckStartFrame": tuck_start_frame,
        "tuckStartMs": ms(tuck_start_frame),
        "tuckEndFrame": tuck_end_frame,
        "tuckEndMs": ms(tuck_end_frame),
        "tuckHiddenMs": ms(tuck_end_frame + 1),
        "sampleFrame": sample_frame,
        "sampleMs": ms(sample_frame),
        "referenceCenterYPx": [round(value, 1) for value in reference_cy],
        "cards": cards,
        "cardGapPx": [
            cards[1]["boxPx"][0] - cards[0]["boxPx"][2] - 1,
            cards[2]["boxPx"][0] - cards[1]["boxPx"][2] - 1,
        ],
        "copiedPill": pill,
        "contentAreasPx": areas,
        "busyRatios": [busy_ratio(frame) for frame in frames],
        "tiltSummary": tilt_summary,
        "tiltTracks": [
            {
                "card": card,
                "samples": [[entry["frame"], entry["tiltDeg"], entry["cx"], entry["cy"]] for entry in track],
            }
            for card, track in enumerate(tracks)
        ],
    }
    with open(OUT_PATH, "w", encoding="utf-8") as handle:
        json.dump(result, handle, indent=2, ensure_ascii=False)
        handle.write("\n")

    summary = {
        key: result[key]
        for key in (
            "frames",
            "size",
            "totalMs",
            "revealMs",
            "revealDownMs",
            "revealOvershootMs",
            "revealOvershootPx",
            "revealSettledMs",
            "tuckStartMs",
            "tuckEndMs",
            "tuckHiddenMs",
            "sampleFrame",
            "cardGapPx",
            "copiedPill",
        )
    }
    print(json.dumps(summary, indent=2))
    print(f"cards: {json.dumps(cards)}")
    print(f"tiltSummary: {json.dumps(tilt_summary)}")
    print(f"wrote {OUT_PATH}")


if __name__ == "__main__":
    try:
        main()
    except (StopIteration, statistics.StatisticsError) as error:
        raise SystemExit(
            "analyze_demo: could not measure this GIF (a detection step found no match: "
            f"{type(error).__name__}); expected the {EXPECTED_FRAMES}-frame "
            f"{EXPECTED_SIZE[0]}x{EXPECTED_SIZE[1]} Tendedero demo."
        ) from error
