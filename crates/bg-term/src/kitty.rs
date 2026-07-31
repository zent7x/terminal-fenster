//! Kitty graphics protocol encoder.
//!
//! Wire format is an APC escape:  `ESC _ G <control-data> ; <base64 payload> ESC \`
//!
//! Verified against Ghostty 1.3.1 on macOS 26.1: a query
//! `ESC _ G i=31,s=1,v=1,a=q,t=d,f=24; <b64 of 3 bytes> ESC \` returns
//! `ESC _ G i=31;OK ESC \`, confirming the terminal parses and accepts this grammar.
//!
//! # Why raw RGB + zlib rather than PNG
//!
//! Format `f=100` hands Kitty a PNG and lets it decode. That sounds cheaper but moves a full
//! PNG *encode* onto our critical path every frame. Chromium gives us BGRA; converting to
//! RGB and deflating is markedly cheaper than a PNG encode, and zlib is exactly what PNG
//! would use internally anyway. So we send `f=24,o=z`.
//!
//! Base64 inflates payloads by 4/3, which is why compression is not optional: an
//! uncompressed 2482x851 RGB frame is 6.3 MB, or 8.4 MB base64 -- unshippable at 60 fps.

use crate::b64;
use crate::Rect;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use std::io::Write;

/// Kitty spec: at most 4096 bytes of base64 payload per escape sequence.
pub const MAX_CHUNK: usize = 4096;

/// Image id reserved for the page surface. Ids are namespaced by us so we never collide
/// with an image some other program left behind in the same terminal.
pub const PAGE_IMAGE_ID: u32 = 1000;

/// Convert a BGRA buffer (Chromium's native offscreen format) to packed RGB.
///
/// Chromium's `NativeImage::toBitmap()` was measured to return BGRA, 4 bytes/pixel,
/// non-strided: a 1440x900 frame is exactly 5,184,000 bytes. A known pure-red page yields
/// `[0,0,255,255]`, which is what pins the channel order.
///
/// Alpha is dropped: the page is composited opaque, so transmitting it would cost 33% more
/// bytes for no visual difference.
pub fn bgra_to_rgb(bgra: &[u8], out: &mut Vec<u8>) {
    out.clear();
    out.reserve(bgra.len() / 4 * 3);
    for px in bgra.chunks_exact(4) {
        out.push(px[2]); // R
        out.push(px[1]); // G
        out.push(px[0]); // B
    }
}

/// Extract a sub-rectangle of a BGRA image as packed RGB.
///
/// Used for damage updates: transmitting only the changed region is the single biggest
/// bandwidth win available, especially over SSH.
pub fn bgra_rect_to_rgb(bgra: &[u8], img_w: u32, rect: Rect, out: &mut Vec<u8>) {
    out.clear();
    out.reserve((rect.area() * 3) as usize);
    let stride = img_w as usize * 4;
    for row in 0..rect.h as usize {
        let y = rect.y as usize + row;
        let start = y * stride + rect.x as usize * 4;
        let line = &bgra[start..start + rect.w as usize * 4];
        for px in line.chunks_exact(4) {
            out.push(px[2]);
            out.push(px[1]);
            out.push(px[0]);
        }
    }
}

fn deflate(data: &[u8], level: u32) -> std::io::Result<Vec<u8>> {
    let mut enc = ZlibEncoder::new(Vec::with_capacity(data.len() / 4), Compression::new(level));
    enc.write_all(data)?;
    enc.finish()
}

/// How an image should be placed on screen.
#[derive(Debug, Clone, Copy)]
pub struct Placement {
    /// Image id to (re)use.
    pub image_id: u32,
    /// Columns/rows the image should occupy. `None` lets Kitty derive from pixel size.
    pub cols: Option<u32>,
    pub rows: Option<u32>,
    /// Z-index. Page content sits below chrome overlays.
    pub z: i32,
    /// If true, the cursor does not move after placement (`C=1`), which is what we want:
    /// a moving cursor would scroll the screen and fight our own layout.
    pub no_cursor_move: bool,
}

impl Default for Placement {
    fn default() -> Self {
        Self { image_id: PAGE_IMAGE_ID, cols: None, rows: None, z: 0, no_cursor_move: true }
    }
}

/// Encode an RGB image as Kitty transmit-and-display escape sequences.
///
/// `rgb` must be exactly `w*h*3` bytes. Output is appended to `out`.
pub fn encode_rgb_frame(
    rgb: &[u8],
    w: u32,
    h: u32,
    place: Placement,
    compress_level: u32,
    out: &mut Vec<u8>,
) -> std::io::Result<EncodeStats> {
    assert_eq!(rgb.len(), (w as usize) * (h as usize) * 3, "rgb buffer size must match w*h*3");

    let raw_len = rgb.len();
    let (payload, compressed) = if compress_level > 0 {
        (deflate(rgb, compress_level)?, true)
    } else {
        (rgb.to_vec(), false)
    };
    let deflated_len = payload.len();

    let mut b64buf = Vec::with_capacity(payload.len() * 4 / 3 + 4);
    b64::encode_into(&payload, &mut b64buf);

    let chunks: Vec<&[u8]> = b64buf.chunks(MAX_CHUNK).collect();
    let chunk_count = chunks.len().max(1);
    let start = out.len();

    for (idx, chunk) in chunks.iter().enumerate() {
        let more = if idx + 1 < chunk_count { 1 } else { 0 };
        out.extend_from_slice(b"\x1b_G");
        if idx == 0 {
            // Control keys only appear on the first chunk; continuations carry just m=.
            out.extend_from_slice(b"a=T,f=24,t=d,q=2");
            if compressed {
                out.extend_from_slice(b",o=z");
            }
            write_kv(out, b",s=", w as i64);
            write_kv(out, b",v=", h as i64);
            write_kv(out, b",i=", place.image_id as i64);
            if let Some(c) = place.cols {
                write_kv(out, b",c=", c as i64);
            }
            if let Some(r) = place.rows {
                write_kv(out, b",r=", r as i64);
            }
            if place.z != 0 {
                write_kv(out, b",z=", place.z as i64);
            }
            if place.no_cursor_move {
                out.extend_from_slice(b",C=1");
            }
            write_kv(out, b",m=", more);
        } else {
            write_kv(out, b"m=", more);
        }
        out.push(b';');
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\x1b\\");
    }

    Ok(EncodeStats {
        raw_bytes: raw_len,
        deflated_bytes: deflated_len,
        wire_bytes: out.len() - start,
        chunks: chunk_count,
    })
}

fn write_kv(out: &mut Vec<u8>, key: &[u8], val: i64) {
    out.extend_from_slice(key);
    let mut buf = itoa(val);
    out.append(&mut buf);
}

fn itoa(mut v: i64) -> Vec<u8> {
    if v == 0 {
        return vec![b'0'];
    }
    let neg = v < 0;
    if neg {
        v = -v;
    }
    let mut d = Vec::with_capacity(12);
    while v > 0 {
        d.push(b'0' + (v % 10) as u8);
        v /= 10;
    }
    if neg {
        d.push(b'-');
    }
    d.reverse();
    d
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodeStats {
    pub raw_bytes: usize,
    pub deflated_bytes: usize,
    /// Total bytes actually written to the terminal, including escapes and base64 overhead.
    pub wire_bytes: usize,
    pub chunks: usize,
}

impl EncodeStats {
    pub fn compression_ratio(&self) -> f64 {
        if self.deflated_bytes == 0 {
            return 0.0;
        }
        self.raw_bytes as f64 / self.deflated_bytes as f64
    }
}

/// Delete a specific image and free its data.
pub fn delete_image(id: u32, out: &mut Vec<u8>) {
    out.extend_from_slice(b"\x1b_Ga=d,d=I,i=");
    let mut b = itoa(id as i64);
    out.append(&mut b);
    out.extend_from_slice(b"\x1b\\");
}

/// Delete every image. Used on teardown so nothing lingers over the user's shell.
pub fn delete_all(out: &mut Vec<u8>) {
    out.extend_from_slice(b"\x1b_Ga=d,d=A\x1b\\");
}

/// Build the support-detection query. A conforming terminal answers
/// `ESC _ G i=<id>;OK ESC \`.
pub fn support_query(id: u32) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"\x1b_Gi=");
    let mut b = itoa(id as i64);
    out.append(&mut b);
    out.extend_from_slice(b",s=1,v=1,a=q,t=d,f=24;");
    b64::encode_into(&[0, 0, 0], &mut out);
    out.extend_from_slice(b"\x1b\\");
    out
}

/// Wrap a byte sequence for tmux passthrough.
///
/// tmux requires `ESC P tmux; <payload with every ESC doubled> ESC \`, and the user must
/// have `allow-passthrough on` set. Without the doubling tmux terminates the DCS early and
/// the terminal sees a truncated graphics command.
pub fn wrap_tmux(payload: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(b"\x1bPtmux;");
    for &b in payload {
        if b == 0x1b {
            out.push(0x1b);
        }
        out.push(b);
    }
    out.extend_from_slice(b"\x1b\\");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bgra_to_rgb_swaps_channels_and_drops_alpha() {
        // One pure-red BGRA pixel as Chromium delivers it.
        let bgra = [0u8, 0, 255, 255];
        let mut rgb = Vec::new();
        bgra_to_rgb(&bgra, &mut rgb);
        assert_eq!(rgb, vec![255, 0, 0]);
    }

    #[test]
    fn bgra_to_rgb_length_is_three_quarters() {
        let bgra = vec![7u8; 1440 * 900 * 4];
        let mut rgb = Vec::new();
        bgra_to_rgb(&bgra, &mut rgb);
        assert_eq!(rgb.len(), 1440 * 900 * 3);
    }

    #[test]
    fn rect_extraction_picks_correct_pixels() {
        // 3x2 image; each pixel's blue channel encodes its index so we can assert placement.
        let mut bgra = Vec::new();
        for i in 0..6u8 {
            bgra.extend_from_slice(&[i, 0, 0, 255]); // B=i
        }
        let mut rgb = Vec::new();
        // Take the right-hand 2x1 of the top row: indices 1 and 2.
        bgra_rect_to_rgb(&bgra, 3, Rect::new(1, 0, 2, 1), &mut rgb);
        // B channel lands in RGB position 2.
        assert_eq!(rgb, vec![0, 0, 1, 0, 0, 2]);
    }

    #[test]
    fn encoded_frame_has_valid_apc_envelope() {
        let rgb = vec![0u8; 4 * 4 * 3];
        let mut out = Vec::new();
        let stats = encode_rgb_frame(&rgb, 4, 4, Placement::default(), 6, &mut out).unwrap();
        assert!(out.starts_with(b"\x1b_G"), "must open with the APC introducer");
        assert!(out.ends_with(b"\x1b\\"), "must close with ST");
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("a=T"), "transmit-and-display");
        assert!(s.contains("f=24"), "raw RGB format");
        assert!(s.contains("o=z"), "zlib compressed");
        assert!(s.contains("s=4"), "width");
        assert!(s.contains("v=4"), "height");
        assert!(s.contains("C=1"), "cursor must not move");
        assert_eq!(stats.raw_bytes, 48);
    }

    #[test]
    fn chunking_respects_the_4096_byte_limit() {
        // Random-ish data so zlib cannot collapse it to nothing and we really get chunks.
        let mut rgb = Vec::with_capacity(200 * 200 * 3);
        let mut x: u32 = 12345;
        for _ in 0..200 * 200 * 3 {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            rgb.push((x >> 16) as u8);
        }
        let mut out = Vec::new();
        let stats = encode_rgb_frame(&rgb, 200, 200, Placement::default(), 6, &mut out).unwrap();
        assert!(stats.chunks > 1, "incompressible data should need multiple chunks");

        // Every payload segment must be <= MAX_CHUNK bytes.
        let text = out.clone();
        let mut idx = 0;
        let mut seen = 0;
        while let Some(p) = find(&text[idx..], b"\x1b_G") {
            let start = idx + p;
            let semi = find(&text[start..], b";").map(|v| start + v).unwrap();
            let end = find(&text[semi..], b"\x1b\\").map(|v| semi + v).unwrap();
            let payload_len = end - semi - 1;
            assert!(payload_len <= MAX_CHUNK, "chunk of {payload_len} exceeds {MAX_CHUNK}");
            seen += 1;
            idx = end + 2;
        }
        assert_eq!(seen, stats.chunks, "chunk count must match what we reported");
    }

    #[test]
    fn only_the_last_chunk_is_marked_final() {
        let mut rgb = Vec::with_capacity(200 * 200 * 3);
        let mut x: u32 = 999;
        for _ in 0..200 * 200 * 3 {
            x = x.wrapping_mul(1103515245).wrapping_add(12345);
            rgb.push((x >> 16) as u8);
        }
        let mut out = Vec::new();
        let stats = encode_rgb_frame(&rgb, 200, 200, Placement::default(), 6, &mut out).unwrap();
        let s = String::from_utf8_lossy(&out);
        assert_eq!(s.matches("m=0").count(), 1, "exactly one terminal chunk");
        assert_eq!(s.matches("m=1").count(), stats.chunks - 1);
    }

    #[test]
    fn compression_actually_helps_on_realistic_content() {
        // A flat-colour region stands in for the large uniform areas real pages have.
        let rgb = vec![0xEEu8; 800 * 600 * 3];
        let mut out = Vec::new();
        let stats = encode_rgb_frame(&rgb, 800, 600, Placement::default(), 6, &mut out).unwrap();
        assert!(
            stats.compression_ratio() > 50.0,
            "flat content should compress hugely, got {:.1}x",
            stats.compression_ratio()
        );
    }

    #[test]
    fn support_query_matches_the_sequence_ghostty_answered() {
        let q = support_query(31);
        let s = String::from_utf8_lossy(&q);
        // This exact shape was confirmed to return `ESC _ G i=31;OK ESC \` from Ghostty 1.3.1.
        assert!(s.starts_with("\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;"));
        assert!(s.ends_with("\x1b\\"));
    }

    #[test]
    fn tmux_wrapper_doubles_escapes() {
        let mut out = Vec::new();
        wrap_tmux(b"\x1b_Gx\x1b\\", &mut out);
        assert!(out.starts_with(b"\x1bPtmux;"));
        // The inner ESCs must appear doubled or tmux truncates the DCS.
        assert!(find(&out[7..], b"\x1b\x1b_Gx").is_some());
    }

    #[test]
    fn delete_all_emits_the_teardown_sequence() {
        let mut out = Vec::new();
        delete_all(&mut out);
        assert_eq!(out, b"\x1b_Ga=d,d=A\x1b\\");
    }

    #[test]
    fn itoa_handles_edges() {
        assert_eq!(itoa(0), b"0".to_vec());
        assert_eq!(itoa(1000), b"1000".to_vec());
        assert_eq!(itoa(-42), b"-42".to_vec());
    }

    fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
        hay.windows(needle.len()).position(|w| w == needle)
    }
}
