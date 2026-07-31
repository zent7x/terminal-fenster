//! Unicode half-block fallback renderer.
//!
//! For terminals with no graphics protocol at all (Apple Terminal, most SSH-into-minimal-box
//! situations) we still have to show *something* useful. The upper-half-block glyph `U+2580`
//! with a truecolor foreground and background packs two vertical pixels into one cell:
//! foreground paints the top half, background the bottom.
//!
//! This is explicitly low fidelity. At a 17x37 cell that is a ~17x18 downsample per cell --
//! enough to read layout, find a button, and see that a page loaded, not enough to read body
//! text. `doctor` says so plainly rather than letting the user think the tool is broken.

const UPPER_HALF: &str = "\u{2580}";

/// Render an RGB image as half-block cells.
///
/// `cols`/`rows` are the character-cell dimensions of the target area; the image is
/// point-sampled to `cols x (rows*2)` pixels.
pub fn render_half_blocks(rgb: &[u8], w: u32, h: u32, cols: u32, rows: u32, out: &mut String) {
    if w == 0 || h == 0 || cols == 0 || rows == 0 {
        return;
    }
    let px = |x: u32, y: u32| -> (u8, u8, u8) {
        let sx = (x * w / cols).min(w - 1);
        let sy = (y * h / (rows * 2)).min(h - 1);
        let i = ((sy * w + sx) * 3) as usize;
        (rgb[i], rgb[i + 1], rgb[i + 2])
    };

    for row in 0..rows {
        let mut last: Option<((u8, u8, u8), (u8, u8, u8))> = None;
        for col in 0..cols {
            let top = px(col, row * 2);
            let bottom = px(col, row * 2 + 1);
            // Only re-emit SGR when the colour pair changes. On a page with large flat
            // regions this removes the majority of the escape bytes.
            if last != Some((top, bottom)) {
                out.push_str(&format!(
                    "\x1b[38;2;{};{};{}m\x1b[48;2;{};{};{}m",
                    top.0, top.1, top.2, bottom.0, bottom.1, bottom.2
                ));
                last = Some((top, bottom));
            }
            out.push_str(UPPER_HALF);
        }
        out.push_str("\x1b[0m");
        if row + 1 < rows {
            out.push_str("\r\n");
        }
    }
}

/// Sanitize untrusted text before it is written to the terminal.
///
/// A page title or URL is attacker-controlled. Writing it raw lets a hostile page emit
/// escape sequences through us: retitle the window, drive OSC 52 to overwrite the user's
/// clipboard, or in some terminals worse. Everything outside printable ranges is replaced.
///
/// C1 (U+0080..U+009F) is included because a terminal decoding UTF-8 will treat those as
/// single-byte control introducers -- `U+009B` is CSI.
pub fn sanitize_for_terminal(s: &str, max_len: usize) -> String {
    let mut out = String::with_capacity(s.len().min(max_len));
    for ch in s.chars() {
        if out.chars().count() >= max_len {
            out.push('\u{2026}');
            break;
        }
        let c = ch as u32;
        let dangerous = c < 0x20            // C0 controls, includes ESC 0x1b
            || c == 0x7f                    // DEL
            || (0x80..=0x9f).contains(&c)   // C1 controls, includes CSI 0x9b
            || c == 0x2028 || c == 0x2029;  // line/paragraph separators
        out.push(if dangerous { '\u{fffd}' } else { ch });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_block_emits_expected_cell_count() {
        let rgb = vec![0u8; 8 * 8 * 3];
        let mut s = String::new();
        render_half_blocks(&rgb, 8, 8, 4, 2, &mut s);
        assert_eq!(s.matches(UPPER_HALF).count(), 8, "4 cols x 2 rows");
    }

    #[test]
    fn half_block_encodes_top_and_bottom_separately() {
        // Top row red, bottom row blue -> one cell with red fg and blue bg.
        let mut rgb = Vec::new();
        rgb.extend_from_slice(&[255, 0, 0]);
        rgb.extend_from_slice(&[0, 0, 255]);
        let mut s = String::new();
        render_half_blocks(&rgb, 1, 2, 1, 1, &mut s);
        assert!(s.contains("\x1b[38;2;255;0;0m"), "fg should be the top pixel");
        assert!(s.contains("\x1b[48;2;0;0;255m"), "bg should be the bottom pixel");
    }

    #[test]
    fn half_block_resets_sgr_each_row() {
        let rgb = vec![9u8; 4 * 4 * 3];
        let mut s = String::new();
        render_half_blocks(&rgb, 4, 4, 2, 2, &mut s);
        assert_eq!(s.matches("\x1b[0m").count(), 2, "must reset per row, not leak colour");
    }

    #[test]
    fn half_block_dedupes_runs_of_identical_colour() {
        let rgb = vec![0u8; 100 * 2 * 3]; // uniform
        let mut s = String::new();
        render_half_blocks(&rgb, 100, 2, 100, 1, &mut s);
        // 100 identical cells should need exactly one colour change, not 100.
        assert_eq!(s.matches("\x1b[38;2;").count(), 1);
    }

    #[test]
    fn zero_size_is_a_noop_not_a_panic() {
        let mut s = String::new();
        render_half_blocks(&[], 0, 0, 10, 10, &mut s);
        assert!(s.is_empty());
    }

    #[test]
    fn sanitize_strips_escape_injection_from_a_hostile_title() {
        // A page title trying to smuggle an OSC 52 clipboard write through us.
        let evil = "Nice Page\x1b]52;c;ZXZpbA==\x07";
        let clean = sanitize_for_terminal(evil, 200);
        assert!(!clean.contains('\x1b'), "ESC must never survive");
        assert!(!clean.contains('\x07'), "BEL must never survive");
        assert!(clean.starts_with("Nice Page"));
    }

    #[test]
    fn sanitize_strips_c1_controls() {
        // U+009B is CSI as a single code point; a naive filter that only checks < 0x20
        // would let this through and allow cursor control.
        let evil = "a\u{009b}31mred";
        let clean = sanitize_for_terminal(evil, 100);
        assert!(!clean.contains('\u{009b}'));
    }

    #[test]
    fn sanitize_keeps_legitimate_unicode() {
        let s = "日本語 — café 😀";
        assert_eq!(sanitize_for_terminal(s, 100), s);
    }

    #[test]
    fn sanitize_truncates_with_ellipsis() {
        let long = "x".repeat(500);
        let clean = sanitize_for_terminal(&long, 10);
        assert_eq!(clean.chars().count(), 11); // 10 + ellipsis
        assert!(clean.ends_with('\u{2026}'));
    }

    #[test]
    fn sanitize_counts_chars_not_bytes_when_truncating() {
        let s = "日".repeat(50);
        let clean = sanitize_for_terminal(&s, 5);
        assert_eq!(clean.chars().count(), 6);
    }
}
