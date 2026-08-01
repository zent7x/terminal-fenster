//! Round-trip verification of the Kitty graphics encoder.
//!
//! The unit tests in `kitty.rs` check the *shape* of the output — the APC envelope, chunk
//! sizes, which chunk is final, that flat content compresses. None of them decode the payload
//! back to pixels, so a channel swap, a wrong stride, or an off-by-one in the crop would pass
//! every existing test while putting garbage on screen.
//!
//! This test closes that gap without a terminal: it takes the bytes the encoder emits, parses
//! the APC blocks exactly as a terminal's parser would, base64-decodes and (when `o=z`)
//! inflates the payload, and asserts it equals the input RGB byte-for-byte. Every mosaic tile
//! is one of these frames, so proving the single-image round trip is correct is the
//! terminal-independent half of the damage-path verification C08 still owes.

use flate2::read::ZlibDecoder;
use std::io::Read;
use tf_term::kitty::{copy_rgb_rect, encode_rgb_frame, Placement};
use tf_term::Rect;

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Decode the standard base64 alphabet, skipping padding and any non-alphabet byte.
fn b64_decode(s: &[u8]) -> Vec<u8> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::new();
    let mut acc = 0u32;
    let mut bits = 0u32;
    for &c in s {
        let Some(v) = val(c) else { continue };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

fn inflate(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    ZlibDecoder::new(data)
        .read_to_end(&mut out)
        .expect("payload must be valid zlib");
    out
}

/// Parse every `ESC _ G <controls> ; <payload> ESC \` block the encoder emitted, in order.
/// Returns the first block's control string and the concatenated base64 payload across all
/// chunks — which is exactly the reassembly a real terminal performs for a chunked image.
fn parse_apc(out: &[u8]) -> (String, Vec<u8>) {
    let mut controls = String::new();
    let mut payload = Vec::new();
    let mut i = 0;
    let mut first = true;
    while let Some(rel) = find(&out[i..], b"\x1b_G") {
        let start = i + rel;
        let semi = start + find(&out[start..], b";").expect("APC block has a ';'");
        let end = semi + find(&out[semi..], b"\x1b\\").expect("APC block has a terminator");
        if first {
            controls = String::from_utf8_lossy(&out[start + 3..semi]).into_owned();
            first = false;
        }
        payload.extend_from_slice(&out[semi + 1..end]);
        i = end + 2;
    }
    assert!(!first, "output contained no APC graphics block");
    (controls, payload)
}

/// Reconstruct the RGB an encoded frame carries, and return the first block's controls so the
/// caller can assert the declared geometry.
fn decode_frame(encoded: &[u8]) -> (String, Vec<u8>) {
    let (controls, b64) = parse_apc(encoded);
    let raw = b64_decode(&b64);
    let rgb = if controls.contains("o=z") {
        inflate(&raw)
    } else {
        raw
    };
    (controls, rgb)
}

/// A deterministic RGB image where every channel of every pixel is a function of its position,
/// so a swap or a stride error cannot accidentally reproduce the input.
fn gradient(w: u32, h: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            v.push((x * 7 + 1) as u8); // R
            v.push((y * 13 + 2) as u8); // G
            v.push((x ^ y) as u8); // B
        }
    }
    v
}

#[test]
fn compressed_frame_round_trips_to_the_exact_input() {
    let (w, h) = (12u32, 9u32);
    let rgb = gradient(w, h);
    let mut out = Vec::new();
    encode_rgb_frame(&rgb, w, h, Placement::default(), 1, &mut out).unwrap();

    let (controls, decoded) = decode_frame(&out);
    assert!(
        controls.contains("o=z"),
        "level 1 must mark the payload compressed"
    );
    assert!(
        controls.contains("s=12"),
        "declared width must match: {controls}"
    );
    assert!(
        controls.contains("v=9"),
        "declared height must match: {controls}"
    );
    assert_eq!(decoded, rgb, "decoded pixels must equal the input exactly");
}

#[test]
fn uncompressed_frame_round_trips() {
    let (w, h) = (8u32, 8u32);
    let rgb = gradient(w, h);
    let mut out = Vec::new();
    encode_rgb_frame(&rgb, w, h, Placement::default(), 0, &mut out).unwrap();

    let (controls, decoded) = decode_frame(&out);
    assert!(
        !controls.contains("o=z"),
        "level 0 must not claim compression"
    );
    assert_eq!(decoded, rgb);
}

#[test]
fn chunked_frame_reassembles_across_apc_blocks() {
    // Big enough that the base64 exceeds one 4096-byte chunk, so reassembly is exercised.
    let (w, h) = (200u32, 200u32);
    let rgb = gradient(w, h);
    let mut out = Vec::new();
    let stats = encode_rgb_frame(&rgb, w, h, Placement::default(), 1, &mut out).unwrap();
    assert!(
        stats.chunks > 1,
        "test needs a multi-chunk frame to be meaningful"
    );

    let (_controls, decoded) = decode_frame(&out);
    assert_eq!(decoded.len(), (w * h * 3) as usize);
    assert_eq!(decoded, rgb, "a chunked frame must reassemble to the input");
}

#[test]
fn a_mosaic_tile_cropped_from_the_framebuffer_round_trips() {
    // The path the damage mosaic actually uses: crop a tile out of the persistent RGB canvas
    // with copy_rgb_rect, encode it, and confirm the pixels that reach the wire are the tile's
    // pixels — not a neighbour's, and not row-skewed.
    let (page_w, page_h) = (40u32, 30u32);
    let page = gradient(page_w, page_h);
    let tile = Rect::new(9, 7, 6, 5); // non-origin, so a stride bug shows immediately

    let mut tile_rgb = Vec::new();
    copy_rgb_rect(&page, page_w, tile, &mut tile_rgb);

    let mut out = Vec::new();
    encode_rgb_frame(&tile_rgb, tile.w, tile.h, Placement::default(), 1, &mut out).unwrap();
    let (controls, decoded) = decode_frame(&out);
    assert!(
        controls.contains("s=6") && controls.contains("v=5"),
        "tile dims: {controls}"
    );

    // Independently reconstruct the expected tile straight from the page and compare.
    let mut expected = Vec::new();
    let stride = (page_w * 3) as usize;
    for row in 0..tile.h as usize {
        let start = (tile.y as usize + row) * stride + tile.x as usize * 3;
        expected.extend_from_slice(&page[start..start + tile.w as usize * 3]);
    }
    assert_eq!(
        decoded, expected,
        "the wire pixels must be exactly the tile's pixels"
    );
}
