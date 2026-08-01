// Standalone compression benchmark for Terminal-Fenster's frame transport.
//
// Scope note (important, read before trusting the numbers): the Kitty graphics protocol's
// `o` control key defines exactly one compression value, `z` (RFC 1950 zlib deflate). A
// terminal emulator has no way to decode a zstd- or lz4-compressed image payload, so this
// benchmark's zstd numbers are NOT a candidate for the core->terminal wire format used today.
// They are measured anyway because C09/A07 design a future transport hop that Terminal-Fenster
// controls at both ends (e.g. a remote-core <-> local-core SSH link), where the receiving
// side is our own code, not the terminal, and any codec is fair game there.
//
// What IS actionable against today's protocol: which zlib *level* to use, and whether to
// compress at all, as a function of payload size/content -- both fully compatible with
// `o=z` as specified.

use std::io::Write;
use std::time::{Duration, Instant};

struct Fixture {
    name: &'static str,
    width: usize,
    height: usize,
    rgb: Vec<u8>,
}

fn load_example_com_png() -> (usize, usize, Vec<u8>) {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../apps/engine/spike/out/example-com.png");
    let decoder = png::Decoder::new(std::fs::File::open(path).expect("example-com.png missing"));
    let mut reader = decoder.read_info().expect("decode png header");
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).expect("decode png frame");
    let bytes = &buf[..info.buffer_size()];
    let rgb = match info.color_type {
        png::ColorType::Rgb => bytes.to_vec(),
        png::ColorType::Rgba => bytes.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect(),
        other => panic!("unexpected PNG color type {other:?}"),
    };
    (info.width as usize, info.height as usize, rgb)
}

fn crop(src: &[u8], src_w: usize, x: usize, y: usize, w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * 3);
    for row in y..y + h {
        let start = (row * src_w + x) * 3;
        out.extend_from_slice(&src[start..start + w * 3]);
    }
    out
}

// Deterministic smooth value noise, 3 octaves, as a stand-in for photographic/video/canvas
// content. NOT a captured real frame -- there was no bundled photo/video fixture in the repo,
// so pure structured noise is the honest synthetic proxy for "hard to compress" content. It is
// intentionally NOT uniform-random bytes: uniform noise is a pathological worst case no real
// video frame reaches (real frames have local spatial correlation), so it would understate how
// much a compressor can do on genuinely high-entropy real content.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E3779B97F4A7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

fn value_noise(width: usize, height: usize, seed: u64) -> Vec<u8> {
    let grid_w = 32usize;
    let grid_h = 24usize;
    let mut state = seed;
    let grid: Vec<[f32; 3]> = (0..grid_w * grid_h)
        .map(|_| {
            [
                (splitmix64(&mut state) % 256) as f32,
                (splitmix64(&mut state) % 256) as f32,
                (splitmix64(&mut state) % 256) as f32,
            ]
        })
        .collect();
    let sample = |gx: usize, gy: usize, c: usize| -> f32 {
        grid[(gy.min(grid_h - 1)) * grid_w + gx.min(grid_w - 1)][c]
    };
    let mut out = vec![0u8; width * height * 3];
    for y in 0..height {
        let fy = y as f32 / height as f32 * (grid_h - 1) as f32;
        let gy0 = fy as usize;
        let ty = fy - gy0 as f32;
        for x in 0..width {
            let fx = x as f32 / width as f32 * (grid_w - 1) as f32;
            let gx0 = fx as usize;
            let tx = fx - gx0 as f32;
            for c in 0..3 {
                let v00 = sample(gx0, gy0, c);
                let v10 = sample(gx0 + 1, gy0, c);
                let v01 = sample(gx0, gy0 + 1, c);
                let v11 = sample(gx0 + 1, gy0 + 1, c);
                let top = v00 + (v10 - v00) * tx;
                let bot = v01 + (v11 - v01) * tx;
                // High-frequency jitter layered on top of the smooth base so the result has
                // real per-pixel variation (like sensor/dithering noise on real video), not
                // just a smooth gradient a compressor would flatten trivially.
                let base = top + (bot - top) * ty;
                let jitter = ((x * 2654435761 + y * 40503 + c * 97) as u32 % 41) as f32 - 20.0;
                out[(y * width + x) * 3 + c] = (base + jitter).clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

fn median(mut xs: Vec<Duration>) -> Duration {
    xs.sort();
    xs[xs.len() / 2]
}

fn bench_zlib(data: &[u8], level: u32, iters: usize) -> (usize, Duration) {
    let mut sizes = Vec::new();
    let mut times = Vec::new();
    for _ in 0..iters {
        let start = Instant::now();
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(level));
        enc.write_all(data).unwrap();
        let out = enc.finish().unwrap();
        times.push(start.elapsed());
        sizes.push(out.len());
    }
    (sizes[0], median(times))
}

fn bench_zstd(data: &[u8], level: i32, iters: usize) -> (usize, Duration) {
    let mut sizes = Vec::new();
    let mut times = Vec::new();
    for _ in 0..iters {
        let start = Instant::now();
        let out = zstd::encode_all(data, level).unwrap();
        times.push(start.elapsed());
        sizes.push(out.len());
    }
    (sizes[0], median(times))
}

fn fmt_row(
    fixture: &str,
    codec: &str,
    raw_len: usize,
    compressed: usize,
    dur: Duration,
    on_protocol: bool,
) {
    let ratio = raw_len as f64 / compressed as f64;
    let mbps = (raw_len as f64 / (1024.0 * 1024.0)) / dur.as_secs_f64();
    println!(
        "{:<22} {:<14} {:>9} B -> {:>9} B  ratio {:>6.2}x  {:>7.3} ms  {:>7.1} MB/s  {}",
        fixture,
        codec,
        raw_len,
        compressed,
        ratio,
        dur.as_secs_f64() * 1000.0,
        mbps,
        if on_protocol { "" } else { "[not usable on the kitty wire -- see header]" }
    );
}

fn run_all(fixture: &Fixture, iters: usize) {
    let raw_len = fixture.rgb.len();
    for &level in &[1u32, 6, 9] {
        let (size, dur) = bench_zlib(&fixture.rgb, level, iters);
        fmt_row(fixture.name, &format!("zlib L{level}"), raw_len, size, dur, true);
    }
    for &level in &[1i32, 3, 9, 19] {
        let (size, dur) = bench_zstd(&fixture.rgb, level, iters);
        fmt_row(fixture.name, &format!("zstd L{level}"), raw_len, size, dur, false);
    }
    println!();
}

fn main() {
    let iters = 9;
    println!("Terminal-Fenster compression benchmark -- zlib (kitty o=z, protocol-usable) vs zstd (reference only)\n");

    let (w, h, page_rgb) = load_example_com_png();
    println!("Loaded real capture: example-com.png {w}x{h} RGB8 ({} bytes)\n", page_rgb.len());

    // Tile size matches the C08 mosaic design: 4x4 terminal cells at 17x37px/cell = 68x148px.
    let tile_w = 68usize;
    let tile_h = 148usize;
    let detail_tile = crop(&page_rgb, w, 40, 80, tile_w, tile_h); // near page heading/text
    let blank_tile = crop(&page_rgb, w, 40, 600, tile_w, tile_h); // lower body-copy area

    let noise_full = value_noise(w, h, 0xC0FFEE);
    let noise_tile = crop(&noise_full, w, 700, 400, tile_w, tile_h);

    let fixtures = vec![
        Fixture { name: "text_page_full (real)", width: w, height: h, rgb: page_rgb.clone() },
        Fixture { name: "text_tile_detail", width: tile_w, height: tile_h, rgb: detail_tile },
        Fixture { name: "text_tile_blank", width: tile_w, height: tile_h, rgb: blank_tile },
        Fixture { name: "noise_full (synthetic)", width: w, height: h, rgb: noise_full },
        Fixture { name: "noise_tile (synthetic)", width: tile_w, height: tile_h, rgb: noise_tile },
    ];

    for f in &fixtures {
        println!("=== {} ({}x{}, {} B raw) ===", f.name, f.width, f.height, f.rgb.len());
        run_all(f, iters);
    }
}
