//! Small dependency-free benchmark for the dense-frame BGRA -> retained-RGB hot loop.
//! Run with: cargo run --release -p tf-term --example convert-bench -- 200

use std::hint::black_box;
use std::time::{Duration, Instant};
use tf_term::kitty::blit_bgra_into_rgb;

fn percentile(samples: &[Duration], pct: usize) -> Duration {
    let idx = (pct * samples.len()).div_ceil(100).saturating_sub(1);
    samples[idx.min(samples.len() - 1)]
}

fn main() {
    let iterations = std::env::args()
        .nth(1)
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(200);
    let (w, h) = (2108u32, 1406u32);
    let mut bgra = vec![0u8; w as usize * h as usize * 4];
    for (i, byte) in bgra.iter_mut().enumerate() {
        *byte = ((i * 73 + i / 17 + 19) % 251) as u8;
    }
    let mut rgb = vec![0u8; w as usize * h as usize * 3];

    for _ in 0..10 {
        blit_bgra_into_rgb(&bgra, &mut rgb, w, 0, 0, w, h);
    }
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let started = Instant::now();
        blit_bgra_into_rgb(&bgra, &mut rgb, w, 0, 0, w, h);
        samples.push(started.elapsed());
        black_box(&rgb);
    }
    samples.sort_unstable();
    let p50 = percentile(&samples, 50);
    let p99 = percentile(&samples, 99);
    let input_gib = bgra.len() as f64 / 1024.0_f64.powi(3);
    let gib_s = input_gib / p50.as_secs_f64();
    let checksum: u64 = rgb.iter().map(|b| *b as u64).sum();
    println!(
        "bgra_to_retained_rgb geometry={w}x{h} iterations={iterations} p50_ms={:.3} p99_ms={:.3} input_gib_s={gib_s:.2} checksum={checksum}",
        p50.as_secs_f64() * 1000.0,
        p99.as_secs_f64() * 1000.0,
    );
}
