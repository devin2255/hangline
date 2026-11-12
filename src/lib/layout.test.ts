import { describe, expect, it } from "vitest";
import {
  CARD_PIN_OVERLAP,
  CARD_VISUAL_INSET,
  PIN_ABOVE,
  ROPE_COLORS,
  ROPE_HIGHLIGHT_COOL,
  ROPE_HIGHLIGHT_WARM,
  ROPE_PALETTE,
  ROPE_SAG_ANIM_MS,
  ROPE_SAG_BASE,
  ROPE_SAG_MAX,
  ROPE_STYLES,
  ROPE_TOP,
  capacity,
  cardTopFromRope,
  cardVisualSize,
  cardWidth,
  cardX,
  edgeFadeOpacity,
  fileNameOf,
  isRopeColor,
  isRopeStyle,
  lineShift,
  lineWidth,
  maxItems,
  maxScrollOffset,
  nextScrollOffset,
  panelHeight,
  photoMaxHeight,
  photoSize,
  pinTop,
  ropeHighlightFor,
  ropePath,
  ropeSag,
  ropeSagEase,
  ropeY,
  showCardImage,
  tiltFor,
  visibleSlice,
} from "./layout";
import { TILT } from "./motion";

describe("layout", () => {
  it("matches spec geometry", () => {
    expect(cardWidth("small")).toBe(120);
    expect(cardWidth("medium")).toBe(150);
    expect(cardWidth("large")).toBe(202);
    expect(panelHeight("medium")).toBe(210);
    expect(capacity(1024, "medium")).toBe(4); // floor((1024-200)/174)=4
    expect(capacity(500, "medium")).toBe(3); // 下限 3
  });

  it("rounds photo height and total panel height per size", () => {
    // 104 * 0.8 = 83.2 -> 83, 104 * 1.35 = 140.4 -> 140.
    expect(photoMaxHeight("small")).toBe(83);
    expect(photoMaxHeight("medium")).toBe(104);
    expect(photoMaxHeight("large")).toBe(140);
    expect(panelHeight("small")).toBe(189);
    expect(panelHeight("large")).toBe(246);
  });

  it("caps capacity at 12 and floors at 3", () => {
    // The brief's "1920 -> 12" cannot hold: (1920 - 200) / 174 = 9.88 -> 9. The 12 cap starts at
    // 2288 dp, because (2288 - 200) / 174 = 12 exactly. 2560 is a common 1440p logical width.
    expect(capacity(1920, "medium")).toBe(9);
    expect(capacity(2287, "medium")).toBe(11);
    expect(capacity(2288, "medium")).toBe(12);
    expect(capacity(2560, "medium")).toBe(12);
    expect(capacity(3840, "medium")).toBe(12);
    // Degenerate widths never go below the 3-card floor.
    expect(capacity(0, "medium")).toBe(3);
    expect(capacity(-500, "small")).toBe(3);
  });

  it("uses the size-specific card width for capacity", () => {
    expect(capacity(1024, "small")).toBe(5); // floor(824 / 144)
    expect(capacity(1024, "large")).toBe(3); // floor(824 / 226)
    expect(capacity(1024, "medium")).toBe(4);
  });

  it("lays cards out symmetrically around the panel centre", () => {
    // Four medium cards: group = 4*150 + 3*24 = 672, left = (1024-672)/2 = 176.
    expect(cardX(0, 4, "medium", 1024)).toBe(176);
    expect(cardX(1, 4, "medium", 1024)).toBe(350);
    expect(cardX(3, 4, "medium", 1024)).toBe(698);
    // Nine medium cards at 1920: group = 1542, left = 189.
    expect(cardX(0, 9, "medium", 1920)).toBe(189);
    expect(cardX(8, 9, "medium", 1920)).toBe(1581);
    // The group is centred: equal margins on both sides.
    const lastRight = cardX(8, 9, "medium", 1920) + cardWidth("medium");
    expect(1920 - lastRight).toBe(cardX(0, 9, "medium", 1920));
  });

  it("right-aligns an overflowing line instead of clipping both sides", () => {
    // 3 large cards need 654 dp, more than a 500 dp panel: the cards start at the left edge and
    // the viewport shifts the whole line left so the newest (right) card stays fully visible
    // (the oldest edge is the only clipped part; there is no native scrollbar).
    expect(cardX(0, 3, "large", 500)).toBe(0);
    expect(cardX(2, 3, "large", 500)).toBe(452);
    // The line is exactly the card group; no phantom space on either side.
    expect(lineWidth(500, 3, "large")).toBe(654);
    expect(lineWidth(1024, 4, "medium")).toBe(1024);
    // A line that fits is never shifted; the overflowing 500 dp line is shifted by -154 dp.
    expect(lineShift(1024, 1024)).toBe(0);
    expect(lineShift(1920, 1542)).toBe(0);
    expect(lineShift(500, 654)).toBe(-154);
    // Degenerate inputs cannot produce a NaN transform.
    expect(lineShift(Number.NaN, 654)).toBe(0);
  });

  it("fits the photo inside the size budget and preserves the aspect", () => {
    const wide = photoSize(1920, 1080, "medium"); // 16:9, width-limited
    expect(wide.width).toBeCloseTo(136, 0);
    expect(wide.height).toBeCloseTo(76.5, 0);
    const tall = photoSize(800, 1600, "medium"); // 1:2, height-limited
    expect(tall.height).toBeCloseTo(104, 0);
    expect(tall.width).toBeCloseTo(52, 0);
    const square = photoSize(1000, 1000, "small");
    expect(square.width).toBeCloseTo(photoMaxHeight("small"), 5);
    expect(square.height).toBeCloseTo(photoMaxHeight("small"), 5);
    // Unknown dimensions (recordings before their first frame, undecodable files) fill the box.
    expect(photoSize(0, 0, "medium")).toEqual({
      width: cardWidth("medium") - 14,
      height: photoMaxHeight("medium"),
    });
    expect(photoSize(Number.NaN, 720, "medium")).toEqual({
      width: cardWidth("medium") - 14,
      height: photoMaxHeight("medium"),
    });
  });

  it("sizes the visual card as the photo plus the 4 dp inset on both sides", () => {
    const card = cardVisualSize({ width: 800, height: 1600 }, "medium");
    expect(card.width).toBeCloseTo(60, 0); // 52 + 2 * 4
    expect(card.height).toBeCloseTo(112, 0); // 104 + 2 * 4
    expect(CARD_VISUAL_INSET).toBe(4);
    const wide = cardVisualSize({ width: 1920, height: 1080 }, "medium");
    expect(wide.width).toBeCloseTo(136 + 2 * CARD_VISUAL_INSET, 0);
    expect(wide.height).toBeCloseTo(76.5 + 2 * CARD_VISUAL_INSET, 0);
  });

  it("tilts deterministically from the item id within the TILT token", () => {
    // FNV-1a 32-bit of "a" is 0xe40c292c -> (0.8908*2-1)*2.5 = 1.954 -> 1.95 degrees.
    expect(tiltFor("a")).toBeCloseTo(1.95, 2);
    expect(tiltFor("a")).toBe(tiltFor("a"));
    expect(tiltFor("b")).toBeCloseTo(2.01, 2);
    expect(TILT.maxDeg).toBe(2.5);
    const samples = ["one", "two", "three", "four", "five", "six", "seven", "eight"];
    const angles = samples.map(tiltFor);
    for (const angle of angles) {
      expect(angle).toBeGreaterThanOrEqual(-TILT.maxDeg);
      expect(angle).toBeLessThanOrEqual(TILT.maxDeg);
    }
    // Not a constant: different ids must produce different tilts.
    expect(new Set(angles).size).toBeGreaterThan(4);
  });

  it("computes the dynamic rope sag from the visible card count and the panel width", () => {
    // Empty line is almost straight; more cards bend further; the cap is a hard stop.
    expect(ROPE_SAG_BASE).toBe(12);
    expect(ROPE_SAG_MAX).toBe(52);
    expect(ropeSag(1600, 0)).toBe(12);
    expect(ropeSag(1600, 3)).toBe(21.6);
    expect(ropeSag(1600, 12)).toBe(50.4);
    expect(ropeSag(1600, 12)).toBeGreaterThan(ropeSag(1600, 3));
    expect(ropeSag(1600, 3)).toBeGreaterThan(ropeSag(1600, 0));
    expect(ropeSag(1600, 20)).toBe(52);
    expect(ropeSag(1600, Number.NaN)).toBe(12);
    // Width factor: >=1600 is 1, 1200/1600 = 0.75, a narrow panel floors at 0.45.
    expect(ropeSag(1920, 12)).toBe(50.4);
    expect(ropeSag(1200, 12)).toBe(37.8);
    expect(ropeSag(500, 12)).toBe(22.68);
    expect(ropeSag(-100, 0)).toBe(5.4);
    expect(ropeSag(0, 12)).toBe(22.68);
  });

  it("keeps the quadratic curve and the SVG path exact for an animated sag", () => {
    // P0=(0,10), P1=(w/2, 10+2*sag), P2=(w,10); the midpoint dips by the full sag.
    expect(ropeY(1920, 0, 52)).toBe(10);
    expect(ropeY(1920, 960, 52)).toBe(62);
    expect(ropeY(1920, 1920, 52)).toBe(10);
    expect(ropeY(1920, 960, 12)).toBe(22);
    // Off-panel coordinates clamp to the rope ends.
    expect(ropeY(1920, -10, 52)).toBe(10);
    expect(ropeY(1920, 5000, 52)).toBe(10);
    expect(ropeY(1920, 960, Number.NaN)).toBe(10);
    expect(ropePath(1920, 52)).toBe("M0,10 Q960,114 1920,10");
    expect(ropePath(1000, 12)).toBe("M0,10 Q500,34 1000,10");
    expect(ropePath(1920, Number.NaN)).toBe("M0,10 Q960,10 1920,10");
  });

  it("eases the sag transition with the CSS cubic-bezier(0.22, 1, 0.36, 1) curve", () => {
    expect(ROPE_SAG_ANIM_MS).toBe(220);
    expect(ropeSagEase(0)).toBe(0);
    expect(ropeSagEase(1)).toBe(1);
    expect(ropeSagEase(-1)).toBe(0);
    expect(ropeSagEase(2)).toBe(1);
    expect(ropeSagEase(Number.NaN)).toBe(0);
    // Monotonic and front-loaded (ease-out): most of the travel happens early.
    let previous = 0;
    for (let step = 1; step <= 20; step += 1) {
      const value = ropeSagEase(step / 20);
      expect(value).toBeGreaterThanOrEqual(previous);
      previous = value;
    }
    expect(ropeSagEase(0.5)).toBeGreaterThan(0.8);
  });

  it("keeps 12 cards at the deepest sag inside every panel size", () => {
    // The worst case is the full 52 dp cap at the line's midpoint (arrow of the curve).
    for (const size of ["small", "medium", "large"] as const) {
      const ropeMid = ropeY(2288, 1144, ROPE_SAG_MAX);
      expect(ropeMid).toBe(ROPE_TOP + ROPE_SAG_MAX);
      const visual = cardVisualSize({ width: 0, height: 0 }, size);
      const cardBottom = cardTopFromRope(ropeMid) + visual.height;
      // The card (and its pin) still leaves headroom inside the fixed panel height.
      expect(cardBottom).toBeLessThanOrEqual(panelHeight(size));
      expect(panelHeight(size) - cardBottom).toBeGreaterThanOrEqual(30);
    }
  });

  it("exposes the four materials and the eight low-saturation palettes", () => {
    expect(ROPE_STYLES).toEqual(["silk", "linen", "brass", "aurora"]);
    expect(ROPE_COLORS).toHaveLength(8);
    expect(new Set(ROPE_COLORS).size).toBe(8);
    expect(ROPE_PALETTE.ink).toBe("#1C1C1E");
    expect(ROPE_PALETTE.ivory).toBe("#EFE9DD");
    expect(ROPE_PALETTE.champagne).toBe("#C8A96A");
    expect(ROPE_PALETTE.fog).toBe("#8E9196");
    expect(ROPE_PALETTE.sage).toBe("#8A9A7B");
    expect(ROPE_PALETTE.haze).toBe("#7A8FA6");
    expect(ROPE_PALETTE.terracotta).toBe("#B46A55");
    expect(ROPE_PALETTE.violet).toBe("#8B7B9B");
    // Warm palettes get the warm white sheen; cool palettes pure white.
    expect(ropeHighlightFor("champagne")).toBe(ROPE_HIGHLIGHT_WARM);
    expect(ropeHighlightFor("ivory")).toBe(ROPE_HIGHLIGHT_WARM);
    expect(ropeHighlightFor("terracotta")).toBe(ROPE_HIGHLIGHT_WARM);
    expect(ropeHighlightFor("ink")).toBe(ROPE_HIGHLIGHT_COOL);
    // Untrusted setting values narrow to the wire enums (old settings / hand-edited JSON).
    expect(isRopeStyle("silk")).toBe(true);
    expect(isRopeStyle("velvet")).toBe(false);
    expect(isRopeStyle(undefined)).toBe(false);
    expect(isRopeColor("haze")).toBe(true);
    expect(isRopeColor("black")).toBe(false);
  });

  it("hangs the pin and the card from the rope exactly like Tendedero", () => {
    const rope = ropeY(1920, 960, ropeSag(1920, 12));
    expect(PIN_ABOVE).toBe(9.5);
    expect(CARD_PIN_OVERLAP).toBe(12);
    // pinTop = ropeY - 9.5; cardTop = pinTop + (26 - 12) = ropeY + 4.5.
    expect(pinTop(rope)).toBeCloseTo(rope - PIN_ABOVE, 2);
    expect(cardTopFromRope(rope)).toBeCloseTo(rope - PIN_ABOVE + (26 - CARD_PIN_OVERLAP), 2);
    expect(cardTopFromRope(rope)).toBeCloseTo(rope + 4.5, 2);
  });

  it("fades the cards out near both ends of the line", () => {
    expect(edgeFadeOpacity(0, 1920)).toBe(0);
    expect(edgeFadeOpacity(45, 1920)).toBeCloseTo(0.5, 1);
    expect(edgeFadeOpacity(96, 1920)).toBe(1);
    expect(edgeFadeOpacity(1824, 1920)).toBe(1);
    // Symmetric near the right edge, clamped outside the line.
    expect(edgeFadeOpacity(1875, 1920)).toBeCloseTo(0.5, 1);
    expect(edgeFadeOpacity(1920, 1920)).toBe(0);
    expect(edgeFadeOpacity(-40, 1920)).toBe(0);
  });

  it("extracts a Windows file name for the fallback tile", () => {
    expect(fileNameOf("C:\\Users\\a\\Pictures\\Hangline 2026.png")).toBe("Hangline 2026.png");
    expect(fileNameOf("C:/Users/a/Pictures/shot.jpg")).toBe("shot.jpg");
    expect(fileNameOf("shot.png")).toBe("shot.png");
    expect(fileNameOf("C:\\Users\\a\\")).toBe("C:\\Users\\a\\");
  });

  it("renders a recording image only when its first-frame preview exists", () => {
    // P3-5: a recording without a preview points at an mp4, so the <img> must stay unmounted and
    // the card falls back to the filename tile + play badge.
    expect(showCardImage("recording", true, "thumb")).toBe(true);
    expect(showCardImage("recording", false, "original")).toBe(false);
    // A broken preview returns to the tile instead of loading the video as an image.
    expect(showCardImage("recording", true, "failed")).toBe(false);
    // Images keep the existing thumb → original chain.
    expect(showCardImage("capture", false, "original")).toBe(true);
    expect(showCardImage("capture", true, "failed")).toBe(false);
    expect(showCardImage("watchedFolder", true, "thumb")).toBe(true);
  });
});

describe("visible window and horizontal scrolling", () => {
  it("maps keepOnLine to the number of cards kept on the line", () => {
    // "fit" is just the panel capacity; a limit larger than the capacity keeps more cards so the
    // older ones can be scrolled in (design spec §3.1: keep 25/50/100).
    expect(maxItems("fit", 9)).toBe(9);
    expect(maxItems("fit", 3)).toBe(3);
    expect(maxItems(25, 9)).toBe(25);
    expect(maxItems(100, 12)).toBe(100);
    expect(maxItems(25, 12)).toBe(25);
  });

  it("keeps the newest cards on the right and scrolls older in", () => {
    // 30 items, keep 25 (maxItems = max(capacity 10, keep 25) = 25), capacity 10.
    // The kept window is the newest 25 items (absolute 5..30); offset 0 shows its newest 10.
    // (An early plan draft wrote {20,25}/{15,20}: those ends would drop the 5 newest items and
    // render 5 cards at capacity 10. Controller fix a2a8d15 aligned the plan with the values below.)
    expect(visibleSlice(30, 25, 10, 0)).toEqual({ start: 20, end: 30 });
    expect(visibleSlice(30, 25, 10, 5)).toEqual({ start: 15, end: 25 });
    // The oldest kept card is items[5]; scrolling further is clamped.
    expect(visibleSlice(30, 25, 10, 15)).toEqual({ start: 5, end: 15 });
  });

  it("clamps the scroll offset into the kept window", () => {
    expect(maxScrollOffset(30, 25, 10)).toBe(15);
    expect(visibleSlice(30, 25, 10, 99)).toEqual({ start: 5, end: 15 });
    expect(visibleSlice(30, 25, 10, -4)).toEqual({ start: 20, end: 30 });
    expect(visibleSlice(30, 25, 10, 3.9)).toEqual({ start: 17, end: 27 });
  });

  it("shows the whole line when fewer items exist than the capacity", () => {
    // total < keep (and total < capacity): nothing to scroll, no negative indices.
    expect(visibleSlice(5, 25, 10, 0)).toEqual({ start: 0, end: 5 });
    expect(visibleSlice(5, 25, 10, 4)).toEqual({ start: 0, end: 5 });
    expect(maxScrollOffset(5, 25, 10)).toBe(0);
    expect(visibleSlice(0, 25, 10, 0)).toEqual({ start: 0, end: 0 });
    expect(visibleSlice(0, 25, 10, 7)).toEqual({ start: 0, end: 0 });
    expect(maxScrollOffset(0, 25, 10)).toBe(0);
  });

  it("never keeps fewer cards than one panel capacity", () => {
    // keep < capacity: the capacity wins, so the wheel has nothing to scroll.
    expect(visibleSlice(30, 5, 10, 0)).toEqual({ start: 20, end: 30 });
    expect(visibleSlice(30, 3, 10, 2)).toEqual({ start: 20, end: 30 });
    expect(maxScrollOffset(30, 5, 10)).toBe(0);
  });

  it("keeps the ratio correct when the capacity changes (cardSize / window width)", () => {
    // total 40, keep 25: offset 15 is the last valid step for 10 cards.
    expect(visibleSlice(40, 25, 10, 15)).toEqual({ start: 15, end: 25 });
    // Fewer cards fit: the offset is clamped and the window shrinks to the new capacity.
    expect(maxScrollOffset(40, 25, 9)).toBe(16);
    expect(visibleSlice(40, 25, 9, 15)).toEqual({ start: 16, end: 25 });
    // More cards fit than before: the offset is clamped to the new maximum.
    expect(maxScrollOffset(40, 25, 12)).toBe(13);
    expect(visibleSlice(40, 25, 12, 15)).toEqual({ start: 15, end: 27 });
    // A capacity wider than total shows every item.
    expect(visibleSlice(8, 100, 12, 0)).toEqual({ start: 0, end: 8 });
  });

  it("uses the dominant wheel axis and steps exactly one card", () => {
    // deltaY dominates (or ties): +1 card = one step towards older cards.
    expect(nextScrollOffset(0, 0, 120, 15)).toBe(1);
    expect(nextScrollOffset(5, 30, 120, 15)).toBe(6);
    expect(nextScrollOffset(5, 120, 120, 15)).toBe(6);
    // deltaX dominates on a horizontal trackpad gesture.
    expect(nextScrollOffset(5, 120, 30, 15)).toBe(6);
    // Negative deltas scroll back towards the newest card.
    expect(nextScrollOffset(5, 0, -120, 15)).toBe(4);
    expect(nextScrollOffset(5, 100, -200, 15)).toBe(4);
    // Clamped at both ends and a zero delta is a no-op.
    expect(nextScrollOffset(0, 0, -120, 15)).toBe(0);
    expect(nextScrollOffset(15, 0, 120, 15)).toBe(15);
    expect(nextScrollOffset(7, 0, 0, 15)).toBe(7);
    // A zero-sized line never scrolls.
    expect(nextScrollOffset(0, 0, 120, 0)).toBe(0);
  });
});
