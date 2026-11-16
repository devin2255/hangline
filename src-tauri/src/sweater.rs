//! v1.6.0 "Sweater Season": derive a small, cosy yarn palette from a screenshot.
//!
//! The renderer lives in the frontend; Rust only needs to answer "which three yarns should this
//! card use?" without sending the screenshot anywhere. The thumbnail PNG (already cached by the
//! thumbnailer) is sampled down to a bounded number of pixels, filtered to colourful mids, and
//! reduced to three yarn colours: one main, one contrast and one warm neutral.
//!
//! The command path falls back to [`fallback_palette`] for undecodable thumbnails and for
//! near-monochrome screenshots, so a card never loses its sweater because of a decode failure.

use std::collections::HashMap;
use std::path::Path;

use image::{GenericImageView, RgbaImage};

/// Upper bound on sampled pixels: enough for a stable palette, cheap enough to run on the UI path.
const MAX_SAMPLES: usize = 4096;

/// Minimum saturation of a pixel that may contribute a yarn colour.
const MIN_SATURATION: f32 = 0.12;

/// Minimum/maximum brightness accepted for a yarn source (excludes near-black/near-white regions).
const MIN_VALUE: f32 = 0.14;
const MAX_VALUE: f32 = 0.94;

/// Backstop palette families. Indexed by a stable item hash so two similar fallbacks stay distinct.
const FALLBACKS: [[&str; 3]; 6] = [
    ["#8FA98F", "#D9A55C", "#E8DFCC"],
    ["#7FA8C4", "#C57B57", "#F2E3C9"],
    ["#9B8FB8", "#7F9CB8", "#EFE6D8"],
    ["#A34B4E", "#3E6B4F", "#EEE3CC"],
    ["#C9A05C", "#6C8070", "#F2E8D8"],
    ["#6D7BA8", "#B8935F", "#E9E1D2"],
];

/// Returns the three yarn colours for the image at `path`.
pub fn palette_for_path(path: &Path) -> Result<Vec<String>, String> {
    let image = image::open(path).map_err(|error| format!("cannot decode {}: {error}", path.display()))?;
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return Err(format!("{} has no pixels", path.display()));
    }
    Ok(palette_for_image(&image.to_rgba8()))
}

/// Extracts up to three yarn-like colours from `image`.
///
/// Every returned color is a `#RRGGBB` string. The result always has exactly three entries:
/// `[main, contrast, neutral]`. `main`/`contrast` carry the screenshot identity; `neutral` is a
/// warm cream tint that keeps the knitted frame readable at small card sizes.
pub fn palette_for_image(image: &RgbaImage) -> Vec<String> {
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return fallback_palette(0);
    }

    // Uniform stride sample. Thumbnails are roughly 320px wide, so this is comfortably bounded.
    let total = width as usize * height as usize;
    let step = (total / MAX_SAMPLES).max(1);
    let mut histogram: HashMap<u32, (u32, f32, f32, f32)> = HashMap::new();
    for index in (0..total).step_by(step) {
        let x = (index % width as usize) as u32;
        let y = (index / width as usize) as u32;
        let pixel = image.get_pixel(x, y);
        if pixel[3] < 128 {
            continue;
        }
        let (hue, saturation, value) = rgb_to_hsv(pixel[0], pixel[1], pixel[2]);
        if saturation < MIN_SATURATION || value < MIN_VALUE || value > MAX_VALUE {
            continue;
        }
        let key = ((pixel[0] as u32 >> 4) << 8) | ((pixel[1] as u32 >> 4) << 4) | (pixel[2] as u32 >> 4);
        let entry = histogram.entry(key).or_insert((0, 0.0, 0.0, 0.0));
        entry.0 += 1;
        entry.1 += pixel[0] as f32;
        entry.2 += pixel[1] as f32;
        entry.3 += pixel[2] as f32;
        // hue is intentionally not stored; the averaged RGB below is what the yarn uses.
        let _ = hue;
    }

    let mut candidates: Vec<(u32, [f32; 3])> = histogram
        .into_values()
        .map(|(count, r, g, b)| (count, [r / count as f32, g / count as f32, b / count as f32]))
        .collect();
    candidates.sort_by(|a, b| b.0.cmp(&a.0));

    let mut picked: Vec<[f32; 3]> = Vec::new();
    for (count, color) in candidates {
        if count < 12 && !picked.is_empty() {
            continue;
        }
        let far_enough = picked
            .iter()
            .all(|existing| color_distance(existing, &color) > 42.0);
        if far_enough {
            picked.push(color);
        }
        if picked.len() >= 2 {
            break;
        }
    }

    if picked.is_empty() {
        // Near-monochrome screenshot (UI whites, grey terminal, etc.): use the name-derived
        // fallback so every card still gets a stable sweater.
        return fallback_palette(fnv1a(&[width as u64, height as u64, 1]));
    }

    let main = soften(picked[0]);
    let contrast = picked
        .get(1)
        .map(|color| soften(*color))
        .unwrap_or_else(|| lighten(main, 0.28));
    let neutral = mix(main, [239.0, 230.0, 216.0], 0.84);
    vec![hex(main), hex(contrast), hex(neutral)]
}

/// Stable fallback palette for a seed (normally the item's BLAKE3-truncated hash).
pub fn fallback_palette(seed: u64) -> Vec<String> {
    let family = FALLBACKS[(seed as usize) % FALLBACKS.len()];
    family.iter().map(|color| (*color).to_string()).collect()
}

fn rgb_to_hsv(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let hue = if delta == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / delta).rem_euclid(6.0))
    } else if max == g {
        60.0 * (((b - r) / delta) + 2.0)
    } else {
        60.0 * (((r - g) / delta) + 4.0)
    };
    let saturation = if max == 0.0 { 0.0 } else { delta / max };
    (hue, saturation, max)
}

/// Pulls a raw screenshot colour towards a soft, warm yarn tone.
fn soften(color: [f32; 3]) -> [f32; 3] {
    let (hue, saturation, value) = rgb_to_hsv(color[0] as u8, color[1] as u8, color[2] as u8);
    let (s, v) = (saturation * 0.62, (value * 0.45 + 0.36).clamp(0.32, 0.80));
    hsv_to_rgb(hue, s, v)
}

fn lighten(color: [f32; 3], amount: f32) -> [f32; 3] {
    [
        color[0] + (255.0 - color[0]) * amount,
        color[1] + (255.0 - color[1]) * amount,
        color[2] + (255.0 - color[2]) * amount,
    ]
}

fn mix(a: [f32; 3], b: [f32; 3], amount: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * amount,
        a[1] + (b[1] - a[1]) * amount,
        a[2] + (b[2] - a[2]) * amount,
    ]
}

fn hsv_to_rgb(hue: f32, saturation: f32, value: f32) -> [f32; 3] {
    let c = value * saturation;
    let h = (hue / 60.0).rem_euclid(6.0);
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = value - c;
    [(r1 + m) * 255.0, (g1 + m) * 255.0, (b1 + m) * 255.0]
}

fn color_distance(a: &[f32; 3], b: &[f32; 3]) -> f32 {
    let dr = a[0] - b[0];
    let dg = a[1] - b[1];
    let db = a[2] - b[2];
    (dr * dr + dg * dg + db * db).sqrt()
}

fn hex(color: [f32; 3]) -> String {
    let clamp = |value: f32| value.clamp(0.0, 255.0).round() as u8;
    format!("#{:02X}{:02X}{:02X}", clamp(color[0]), clamp(color[1]), clamp(color[2]))
}

fn fnv1a(values: &[u64]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for value in values {
        hash ^= *value;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn solid(width: u32, height: u32, color: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(width, height, Rgba(color))
    }

    #[test]
    fn palette_is_always_three_valid_hex_colours() {
        let mut image = solid(64, 64, [0, 0, 0, 0]);
        for y in 0..64 {
            for x in 0..64 {
                let color = if (x + y) % 2 == 0 { [180, 90, 70, 255] } else { [245, 235, 215, 255] };
                image.put_pixel(x, y, Rgba(color));
            }
        }
        let palette = palette_for_image(&image);
        assert_eq!(palette.len(), 3);
        for color in &palette {
            assert_eq!(color.len(), 7, "unexpected hex length: {color}");
            assert!(color.starts_with('#'));
            assert!(color[1..].chars().all(|ch| ch.is_ascii_hexdigit()));
        }
        let main = &palette[0];
        assert_ne!(main, &palette[2], "main and neutral must differ");
    }

    #[test]
    fn near_monochrome_images_fall_back_deterministically() {
        let image = solid(80, 50, [200, 200, 198, 255]);
        let palette = palette_for_image(&image);
        assert_eq!(palette.len(), 3);
        assert_eq!(palette, fallback_palette(fnv1a(&[80, 50, 1])));
    }

    #[test]
    fn fallback_is_stable_and_varies_by_seed() {
        let a = fallback_palette(0);
        let b = fallback_palette(0);
        let c = fallback_palette(1);
        assert_eq!(a, b, "same seed must produce the same palette");
        assert_ne!(a, c, "different seeds should rotate the fallback family");
        assert_eq!(a.len(), 3);
        assert_ne!(a[0], a[1], "main and contrast must differ");
    }

    #[test]
    fn colored_image_prefers_a_colorful_main_yarn() {
        let mut image = solid(64, 64, [245, 240, 230, 255]);
        for y in 10..48 {
            for x in 10..48 {
                image.put_pixel(x, y, Rgba([64, 108, 160, 255]));
            }
        }
        let palette = palette_for_image(&image);
        let main = &palette[0];
        let r = u8::from_str_radix(&main[1..3], 16).expect("hex r");
        let b = u8::from_str_radix(&main[5..7], 16).expect("hex b");
        assert!(b > r, "blue screenshot should produce a blue-leaning main yarn: {main}");
    }
}
