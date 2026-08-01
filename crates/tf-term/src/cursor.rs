//! Terminal pointer overlay — a small Kitty image drawn above page content.

use crate::kitty::{self, Placement};

pub const CURSOR_IMAGE_ID: u32 = 2100;
pub const CURSOR_W: u32 = 18;
pub const CURSOR_H: u32 = 24;

/// Build a high-contrast arrow pointer in packed RGB.
pub fn arrow_rgb() -> Vec<u8> {
    let w = CURSOR_W as usize;
    let h = CURSOR_H as usize;
    let mut rgb = vec![0u8; w * h * 3];
    let outline = [20u8, 20, 20];
    let fill = [255u8, 70, 70];
    let hot_x = 1usize;
    let hot_y = 1usize;

    for y in 0..h {
        for x in 0..w {
            let dx = x as i32 - hot_x as i32;
            let dy = y as i32 - hot_y as i32;
            let inside = (dx >= 0 && dy >= 0 && dx + dy < 14) || (dx == 0 && (0..16).contains(&dy));
            if !inside {
                continue;
            }
            let edge = dx == 0
                || dy == 0
                || dx + dy == 13
                || (dx == 1 && dy >= 11)
                || (dy == 1 && dx >= 11);
            let c = if edge { outline } else { fill };
            let i = (y * w + x) * 3;
            rgb[i] = c[0];
            rgb[i + 1] = c[1];
            rgb[i + 2] = c[2];
        }
    }
    rgb
}

fn placement_at(x: u32, y: u32) -> Placement {
    let px = x.saturating_sub(1);
    let py = y.saturating_sub(1);
    Placement {
        image_id: CURSOR_IMAGE_ID,
        cols: None,
        rows: None,
        z: 1_000,
        no_cursor_move: true,
        pixel_x: Some(px),
        pixel_y: Some(py),
    }
}

/// Upload the cursor sprite with its hotspot at `(x, y)` page pixels.
pub fn encode_at(x: u32, y: u32, rgb: &[u8], out: &mut Vec<u8>) -> std::io::Result<()> {
    kitty::encode_rgb_frame(rgb, CURSOR_W, CURSOR_H, placement_at(x, y), 0, out)?;
    Ok(())
}

/// Move an already-uploaded cursor without retransmitting pixels.
pub fn place_at(x: u32, y: u32, out: &mut Vec<u8>) {
    kitty::place_image(CURSOR_W, CURSOR_H, placement_at(x, y), out);
}

pub fn delete(out: &mut Vec<u8>) {
    kitty::delete_image(CURSOR_IMAGE_ID, out);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrow_has_opaque_pixels() {
        let rgb = arrow_rgb();
        assert!(rgb.iter().any(|&b| b > 200));
    }
}
