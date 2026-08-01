//! Measures the mosaic's wire-byte advantage with the *real* encoder — the terminal-independent
//! half of proving the damage-tracking win (the on-screen half is P0.1, a Ghostty run).
//!
//! C08 modelled ~83x fewer bytes on a keystroke off-repo; this makes it a committed, re-runnable
//! number against `kitty::encode_rgb_frame` at the level the CLI ships (1). It encodes a full
//! frame, then only the cell-aligned tiles a small edit damages, and prints the ratio.
//!
//! The page matters: random noise does not compress, so measuring it would understate the win by
//! making the full frame look artificially expensive-per-pixel in a way real pages are not. This
//! uses a locally-uniform page (flat background + a few solid blocks) — the property real web
//! pages have and the reason per-tile zlib recovers what it loses in cross-tile context (C08 §5.2).
//!
//! Run:  cargo run -p bg-term --example mosaic-bandwidth --release

use bg_term::kitty::{copy_rgb_rect, encode_rgb_frame, Placement};
use bg_term::Rect;

// Ghostty geometry from the C08 measurements: 17x37 cell, 4x4-cell tiles => 68x148 px.
const CELL_W: u32 = 17;
const CELL_H: u32 = 37;
const TILE_CW: u32 = 4;
const TILE_CH: u32 = 4;
const PAGE_W: u32 = 1440;
const PAGE_H: u32 = 888; // 24 cells * 37, keeps the page an exact cell multiple

fn realistic_page(w: u32, h: u32) -> Vec<u8> {
    // Flat near-white background with a few solid blocks standing in for text columns and an
    // image — locally uniform, like a real page, so zlib behaves as it would in production.
    let mut rgb = vec![0xF6u8; (w * h * 3) as usize];
    let blocks = [
        (40u32, 40u32, 900u32, 120u32, [0x20u8, 0x20, 0x28]),
        (40, 200, 620, 500, [0x33, 0x33, 0x3a]),
        (700, 200, 660, 360, [0x10, 0x40, 0x80]),
    ];
    for (bx, by, bw, bh, col) in blocks {
        for y in by..(by + bh).min(h) {
            let row = (y * w + bx) as usize * 3;
            for x in 0..(bw.min(w - bx)) as usize {
                let i = row + x * 3;
                rgb[i] = col[0];
                rgb[i + 1] = col[1];
                rgb[i + 2] = col[2];
            }
        }
    }
    rgb
}

fn wire_bytes(rgb: &[u8], w: u32, h: u32) -> usize {
    let mut out = Vec::new();
    encode_rgb_frame(rgb, w, h, Placement::default(), 1, &mut out)
        .expect("encode")
        .wire_bytes
}

// Wire bytes to repaint the tiles that intersect `damage`, each encoded independently as the
// mosaic does. Tiles are cell-aligned; edge tiles clamp to the page.
fn tiled_wire_bytes(page: &[u8], damage: Rect) -> (usize, usize) {
    let tw = TILE_CW * CELL_W;
    let th = TILE_CH * CELL_H;
    let col0 = damage.x / tw;
    let col1 = (damage.x + damage.w - 1) / tw;
    let row0 = damage.y / th;
    let row1 = (damage.y + damage.h - 1) / th;
    let mut total = 0;
    let mut tiles = 0;
    let mut buf = Vec::new();
    for row in row0..=row1 {
        for col in col0..=col1 {
            let x = col * tw;
            let y = row * th;
            let rw = tw.min(PAGE_W - x);
            let rh = th.min(PAGE_H - y);
            copy_rgb_rect(page, PAGE_W, Rect::new(x, y, rw, rh), &mut buf);
            total += wire_bytes(&buf, rw, rh);
            tiles += 1;
        }
    }
    (total, tiles)
}

fn main() {
    let page = realistic_page(PAGE_W, PAGE_H);
    let full = wire_bytes(&page, PAGE_W, PAGE_H);

    // Three representative edits, in device pixels.
    let cases = [
        ("caret / 1 char", Rect::new(604, 411, 10, 19)),
        ("word (24 px)", Rect::new(604, 411, 24, 19)),
        ("80x80 anim", Rect::new(700, 300, 80, 80)),
    ];

    println!("Mosaic wire-byte advantage (real kitty encoder, level 1)");
    println!(
        "  page {PAGE_W}x{PAGE_H}, tile {}x{} px",
        TILE_CW * CELL_W,
        TILE_CH * CELL_H
    );
    println!("  full frame: {full} B\n");
    println!(
        "  {:<16} {:>6} {:>10} {:>10}",
        "edit", "tiles", "tiled B", "ratio"
    );
    println!("  {}", "-".repeat(46));
    for (name, dmg) in cases {
        let (tiled, tiles) = tiled_wire_bytes(&page, dmg);
        let ratio = full as f64 / tiled as f64;
        println!("  {name:<16} {tiles:>6} {tiled:>10} {:>9.1}x", ratio);
    }
    println!(
        "\n  On-screen correctness of these updates still needs a Ghostty run (roadmap P0.1);"
    );
    println!("  this measures only the bytes that would cross the wire.");
}
