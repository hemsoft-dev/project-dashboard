//! Project Dashboard's mark: the four dashboard tiles in HemSoft gold on a black rounded square, the gold running from
//! bright at the top right to deep at the bottom left like the hemsoft-dev avatar's streaks.
//!
//! One drawing serves the Windows executable's icon (`build.rs`) and `assets/icon-256.png` (`examples/icon.rs`); a test
//! fails when the committed PNG no longer matches the drawing. The build script includes this file by path, so it
//! depends on nothing.

// Each consumer uses a subset: the build script only `ico`, the example only `rgba`.
#![allow(dead_code)]

/// The background: HemSoft black.
pub const BLACK: [u8; 3] = [0x0A, 0x0A, 0x0A];
/// The gold ramp, bright to deep: `#d4af37`, `#b8860b`, `#8b6914`.
pub const GOLDS: [[u8; 3]; 3] = [[0xD4, 0xAF, 0x37], [0xB8, 0x86, 0x0B], [0x8B, 0x69, 0x14]];

/// The tiles on a 32-unit grid as (left, top, right, bottom): tall top left, short top right, tall bottom right, short
/// bottom left, the layout of a dashboard.
const TILES: [(f32, f32, f32, f32); 4] = [
    (6.0, 6.0, 15.0, 17.0),
    (17.0, 6.0, 26.0, 12.0),
    (17.0, 14.0, 26.0, 26.0),
    (6.0, 19.0, 15.0, 26.0),
];
const SQUARE_RADIUS: f32 = 7.0;
const TILE_RADIUS: f32 = 1.75;
/// Samples per pixel side, for smooth edges at every size.
const SAMPLES: u32 = 4;

/// The sizes an icon resource holds, so Windows finds a crisp rendering at every DPI and in every place it shows the
/// icon: title bars, the taskbar, Alt-Tab, Explorer, shortcuts and the Start menu.
pub const ICON_SIZES: [u32; 9] = [16, 20, 24, 32, 40, 48, 64, 128, 256];

/// True when (`x`, `y`) lies inside the rounded rectangle.
fn inside(x: f32, y: f32, (left, top, right, bottom): (f32, f32, f32, f32), radius: f32) -> bool {
    if x < left || x >= right || y < top || y >= bottom {
        return false;
    }
    let cx = x.clamp(left + radius, right - radius);
    let cy = y.clamp(top + radius, bottom - radius);
    (x - cx).powi(2) + (y - cy).powi(2) <= radius * radius
}

/// The gold at grid point (`x`, `y`): brightest at the top right, deepest at the bottom left of the tiles.
pub fn gold_at(x: f32, y: f32) -> [u8; 3] {
    // 0 at the tiles' top-right corner (26, 6), 1 at their bottom-left corner (6, 26).
    let t = (((26.0 - x) + (y - 6.0)) / 40.0).clamp(0.0, 1.0);
    let (from, to, local) = if t < 0.5 {
        (GOLDS[0], GOLDS[1], t * 2.0)
    } else {
        (GOLDS[1], GOLDS[2], (t - 0.5) * 2.0)
    };
    let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * local).round() as u8;
    [mix(from[0], to[0]), mix(from[1], to[1]), mix(from[2], to[2])]
}

/// The pixel at (`x`, `y`) of the mark drawn at `size` px, as straight (not premultiplied) RGBA.
pub fn pixel(x: u32, y: u32, size: u32) -> [u8; 4] {
    let unit = size as f32 / 32.0;
    let (mut r, mut g, mut b, mut covered) = (0.0f32, 0.0f32, 0.0f32, 0u32);
    for sy in 0..SAMPLES {
        for sx in 0..SAMPLES {
            let gx = (x as f32 + (sx as f32 + 0.5) / SAMPLES as f32) / unit;
            let gy = (y as f32 + (sy as f32 + 0.5) / SAMPLES as f32) / unit;
            if !inside(gx, gy, (0.0, 0.0, 32.0, 32.0), SQUARE_RADIUS) {
                continue;
            }
            covered += 1;
            let color = if TILES.iter().any(|&tile| inside(gx, gy, tile, TILE_RADIUS)) {
                gold_at(gx, gy)
            } else {
                BLACK
            };
            r += f32::from(color[0]);
            g += f32::from(color[1]);
            b += f32::from(color[2]);
        }
    }
    if covered == 0 {
        return [0, 0, 0, 0];
    }
    let n = covered as f32;
    let alpha = (covered * 255 / (SAMPLES * SAMPLES)) as u8;
    [
        (r / n).round() as u8,
        (g / n).round() as u8,
        (b / n).round() as u8,
        alpha,
    ]
}

/// The mark at `size` px as RGBA, top row first.
pub fn rgba(size: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            out.extend_from_slice(&pixel(x, y, size));
        }
    }
    out
}
/// An `.ico` file with the mark at each of `sizes` (1 to 256 px) as a 32-bit bitmap with alpha, the form every
/// Windows version reads.
pub fn ico(sizes: &[u32]) -> Vec<u8> {
    assert!(
        !sizes.is_empty() && sizes.len() <= u16::MAX as usize,
        "an icon holds 1 to 65535 images"
    );
    let images: Vec<Vec<u8>> = sizes.iter().map(|&size| bitmap(size)).collect();
    let mut out = Vec::new();
    // ICONDIR: reserved, type 1 (icon), image count.
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (&size, image) in sizes.iter().zip(&images) {
        assert!((1..=256).contains(&size), "an icon image is 1 to 256 px");
        // ICONDIRENTRY: width and height (0 means 256), palette size, reserved, planes, bits per pixel, byte size, offset.
        let side = if size == 256 { 0 } else { size as u8 };
        out.extend_from_slice(&[side, side, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(image.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += image.len() as u32;
    }
    for image in &images {
        out.extend_from_slice(image);
    }
    out
}

/// One icon image: a BITMAPINFOHEADER, the BGRA pixels bottom row first, then the 1-bit mask (set where transparent).
fn bitmap(size: u32) -> Vec<u8> {
    let mask_stride = (size.div_ceil(8)).div_ceil(4) * 4;
    let pixels_len = size * size * 4;
    let mask_len = mask_stride * size;
    let mut out = Vec::with_capacity((40 + pixels_len + mask_len) as usize);
    for field in [
        40u32,
        size,
        size * 2,       // The height counts the pixel rows and the mask rows.
        1 | (32 << 16), // Planes and bits per pixel.
        0,              // No compression.
        pixels_len + mask_len,
        0,
        0,
        0,
        0,
    ] {
        out.extend_from_slice(&field.to_le_bytes());
    }
    for y in (0..size).rev() {
        for x in 0..size {
            let [r, g, b, a] = pixel(x, y, size);
            out.extend_from_slice(&[b, g, r, a]);
        }
    }
    for y in (0..size).rev() {
        let mut row = vec![0u8; mask_stride as usize];
        for x in 0..size {
            if pixel(x, y, size)[3] == 0 {
                row[(x / 8) as usize] |= 0x80 >> (x % 8);
            }
        }
        out.extend_from_slice(&row);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLEAR: [u8; 4] = [0, 0, 0, 0];

    fn opaque(color: [u8; 3]) -> [u8; 4] {
        [color[0], color[1], color[2], 0xFF]
    }

    #[test]
    fn the_gold_ramp_runs_bright_top_right_to_deep_bottom_left() {
        assert_eq!(gold_at(26.0, 6.0), GOLDS[0]);
        assert_eq!(gold_at(16.0, 16.0), GOLDS[1]);
        assert_eq!(gold_at(6.0, 26.0), GOLDS[2]);
    }

    #[test]
    fn the_mark_scales_its_corners_tiles_and_gaps_with_the_size() {
        for size in ICON_SIZES {
            let last = size - 1;
            let unit = size as f32 / 32.0;
            let at = |gx: f32, gy: f32| pixel((gx * unit) as u32, (gy * unit) as u32, size);
            assert_eq!(pixel(0, 0, size), CLEAR, "{size}: top-left corner is cut");
            assert_eq!(pixel(last, last, size), CLEAR, "{size}: bottom-right corner is cut");
            assert_eq!(
                pixel(size / 2, 0, size),
                opaque(BLACK),
                "{size}: the top edge's middle is black"
            );
            if size >= 32 {
                // Inside each tile is gold; the gutters between them are black.
                for (gx, gy) in [(10.0, 11.0), (21.0, 9.0), (21.0, 20.0), (10.0, 22.5)] {
                    let [r, g, b, a] = at(gx, gy);
                    assert_eq!(a, 0xFF, "{size}: ({gx}, {gy}) is opaque");
                    assert!(
                        r > 0x80 && r > b + 0x40,
                        "{size}: ({gx}, {gy}) is gold, got {r} {g} {b}"
                    );
                }
                assert_eq!(at(16.0, 10.0), opaque(BLACK), "{size}: the vertical gutter is black");
                assert_eq!(at(21.5, 13.0), opaque(BLACK), "{size}: the right gutter is black");
            }
        }
    }

    /// Reads one image's entry: its size, and where its pixels start.
    fn entry(ico: &[u8], index: usize) -> (u32, usize) {
        let entry = &ico[6 + 16 * index..][..16];
        let size = if entry[0] == 0 { 256 } else { entry[0] as u32 };
        assert_eq!(entry[1], entry[0], "square");
        assert_eq!(u16::from_le_bytes([entry[4], entry[5]]), 1, "one plane");
        assert_eq!(u16::from_le_bytes([entry[6], entry[7]]), 32, "32 bits per pixel");
        let offset = u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]) as usize;
        (size, offset)
    }

    #[test]
    fn the_icon_file_lists_every_size_and_stores_the_pixels_bottom_up() {
        let ico = ico(&ICON_SIZES);
        assert_eq!(&ico[..6], &[0, 0, 1, 0, ICON_SIZES.len() as u8, 0]);
        let mut expected_offset = 6 + 16 * ICON_SIZES.len();
        for (index, &size) in ICON_SIZES.iter().enumerate() {
            let (listed, offset) = entry(&ico, index);
            assert_eq!(listed, size);
            assert_eq!(offset, expected_offset, "{size}: images follow each other");
            let header = &ico[offset..][..40];
            assert_eq!(
                u32::from_le_bytes(header[4..8].try_into().unwrap()),
                size,
                "{size}: width"
            );
            assert_eq!(
                u32::from_le_bytes(header[8..12].try_into().unwrap()),
                size * 2,
                "{size}: height"
            );
            let pixels = &ico[offset + 40..];
            let at = |x: u32, y: u32| {
                let i = ((size - 1 - y) * size + x) as usize * 4;
                [pixels[i + 2], pixels[i + 1], pixels[i], pixels[i + 3]]
            };
            for (x, y) in [(0, 0), (size / 2, size / 2), (size / 4, size / 4), (size - 1, size / 2)] {
                assert_eq!(at(x, y), pixel(x, y, size), "{size}: pixel ({x}, {y})");
            }
            let mask_stride = size.div_ceil(8).div_ceil(4) * 4;
            let mask = &pixels[(size * size * 4) as usize..][..(mask_stride * size) as usize];
            // The bottom-up mask's last row is the image's top row, whose first pixel is a cut corner.
            assert_eq!(
                mask[((size - 1) * mask_stride) as usize] & 0x80,
                0x80,
                "{size}: the corner is masked"
            );
            expected_offset = offset + 40 + (size * size * 4 + mask_stride * size) as usize;
        }
        assert_eq!(ico.len(), expected_offset, "nothing trails the last image");
    }

    #[test]
    fn the_committed_png_is_the_current_drawing() {
        let png = image::load_from_memory(include_bytes!("../../../assets/icon-256.png"))
            .expect("assets/icon-256.png decodes")
            .into_rgba8();
        assert_eq!(png.dimensions(), (256, 256));
        assert!(
            png.as_raw() == &rgba(256),
            "assets/icon-256.png is stale: run `cargo run -p dashboard-app --example icon -- assets/icon-256.png 256`"
        );
    }

    #[test]
    #[should_panic(expected = "1 to 256 px")]
    fn an_icon_image_cannot_exceed_256_px() {
        let _ = ico(&[512]);
    }
}
