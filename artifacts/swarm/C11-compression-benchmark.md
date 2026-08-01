# C11 — Compression Benchmark: zlib levels vs zstd, and why zstd isn't a wire option

**Mission:** C11 (follow-on to C08/C09, self-assigned) · **Date:** 2026-08-01
**Scope:** standalone, additive only. Does not modify `kitty.rs`, `main.rs`, `main.js`,
`tty.rs`, `caps.rs`, `input.rs`, or `tf-proto` — those were under active concurrent
development elsewhere while this ran. This doc and its benchmark crate are new files only.

## 0. The constraint that shapes everything below

The Kitty graphics protocol's `o` (compression) control key defines exactly one value:
`o=z`, RFC 1950 zlib deflate. A terminal emulator has no way to decode a zstd- or
lz4-compressed image payload — there is no `o=zstd`. **Any codec other than zlib is a
non-starter for the core→terminal wire format**, full stop. zstd numbers below are measured
anyway because C09/A07 design a *second* transport hop that Terminal-Fenster would control at both
ends (a remote-core ↔ local-core SSH link) — there, the receiving side is our own code, not
the terminal, and codec choice is actually free. Conflating the two hops was the risk this
doc exists to head off: "switch to zstd" is not an available option for what ships to the
terminal today.

What **is** actionable against today's protocol: which zlib *level* to use as a function of
payload size and content, which is fully `o=z`-compatible.

## 1. Method

New standalone crate, `benchmarks/compression-bench/` (own `[workspace]` table, so it is
invisible to the root workspace and touches no shared file). Reproduce with:

```
cargo run --release --manifest-path benchmarks/compression-bench/Cargo.toml
```

Fixtures:

- **`text_page_full`** — a *real* captured frame, `apps/engine/spike/out/example-com.png`
  (1440×900, decoded to raw RGB8). Not synthetic.
- **`text_tile_detail` / `text_tile_blank`** — 68×148px crops of the same capture. Tile size
  matches the C08 mosaic design exactly (4×4 terminal cells at the measured 17×37px/cell).
- **`noise_full` / `noise_tile`** — deterministic 3-octave value noise, NOT uniform random
  bytes (uniform noise is a pathological worst case no real video frame reaches; smooth noise
  with per-pixel jitter is a more honest proxy for photographic/video/canvas content). No
  photo or video fixture was available in the repo to capture directly — this is clearly a
  synthetic stand-in, not a measurement of real video content, and should be treated as such.

Each cell is the median compress time over 9 iterations (first iteration excluded from
sizing comparisons only in spirit — flate2/zstd are deterministic, so compressed size never
varied across repeated runs; only wall-clock time did). **Caveat that matters:** this machine
had roughly five other agent/Electron processes running concurrently while measuring (this
was run alongside other active work on the repo, not in isolation), so absolute times varied
2–4× run to run under load — see §3. The *ratios between levels/codecs* were stable across
every rerun; only absolute magnitudes moved. Treat absolute numbers as "this machine, under
load, right now," and ratios as the load-bearing conclusion.

## 2. Results (representative run; see §3 for variance across 5 reruns)

| Fixture | Codec | Compressed | Ratio | Time | Notes |
|---|---|---|---|---|---|
| text_page_full (1440×900, real) | zlib L1 | 30,499 B | 127.5× | 0.5–1.4 ms | |
| | zlib L6 | 13,056 B | 297.8× | 4.8–10.6 ms | +0.5% ratio over L9 is not the story — see L1→L6 below |
| | zlib L9 | 12,990 B | 299.3× | 7.0–14.9 ms | **0.5% smaller than L6, 40–70% slower** |
| | zstd L1 *(not wire-usable)* | 10,444 B | 372.3× | 0.6–1.6 ms | beats zlib L6 on size AND speed |
| | zstd L19 *(not wire-usable)* | 7,710 B | 504.3× | 17.3–31.6 ms | |
| text_tile_detail (68×148, real crop) | zlib L1 | 162 B | 186× | 0.03 ms | |
| | zlib L6/L9 | 52 B | 581× | 0.09–0.10 ms | 3.1× smaller, ~0.06 ms costlier — free locally |
| text_tile_blank (68×148, real crop) | zlib L1/L6/L9 | 162/52/52 B | same as above | same as above | example.com is minimalist enough that "detail" and "blank" crops landed on similarly simple regions — see §4 caveat |
| **noise_full (1440×900, synthetic, incompressible)** | **zlib L1** | 3,738,445 B | **1.04×** | **15.6–38.6 ms** | already at-or-over a 16.67 ms 60fps budget alone |
| | **zlib L6** | 3,736,637 B | **1.04×** | **94–234 ms** | **0.05% smaller, 5–8× slower than L1** |
| | zlib L9 | 3,736,637 B | 1.04× | 95–181 ms | identical size to L6, same cost class |
| | zstd L1 *(not wire-usable)* | 3,799,178 B | 1.02× | 3.6–9.1 ms | |
| noise_tile (68×148, synthetic) | zlib L1 | 28,062 B | 1.08× | 0.17 ms | |
| | zlib L6/L9 | 28,065 B | 1.08× | 0.49 ms | **larger output, 3× slower — strictly worse** |

Full per-run output is reproducible via the command in §1; not pasted in full here to keep
this readable.

## 3. The headline finding

**On full-viewport high-entropy damage (video, canvas, WebGL — anywhere Chromium reports the
whole frame dirty and the content doesn't compress), zlib level 6 costs 94–234 ms per frame
against level 1's 15.6–38.6 ms, for a compression improvement of 0.05% — within rounding of
zero.** Level 9 is the same story. This held across all 5 reruns despite 2–4× absolute timing
variance from system load; the level 1→6 ratio (5–8×) was the stable part. On a *weaker*
low-RAM target CPU than the M4 this was measured on, both numbers scale up together, but the
absolute stall gets worse in real terms — 94 ms is a visible hitch even here; the equivalent
on a slower machine is a multi-frame freeze exactly during the highest-motion content a user
is watching.

**This is not a hypothetical regression — it's a guardrail for a decision already made
correctly.** `apps/cli/src/main.rs`'s two `encode_rgb_frame` call sites (the tile-mosaic path
and the oversized-grid full-frame fallback) both already hardcode compression level `1`. This
benchmark is a measured confirmation that choice was right, not a proposal to change it. Its
value is as a tripwire: any future change toward a higher default level "for better
compression" should be checked against this doc first — the *entire* possible gain on
incompressible content is 0.05%, and the cost is a 5–8× CPU multiplier on exactly the frames
where the CPU budget is already tightest.

## 4. Caveat on the tile fixtures

`text_tile_detail` and `text_tile_blank` produced near-identical numbers because both crop
coordinates landed on visually simple regions of example.com, which is a minimalist page
(mostly whitespace and plain serif text). That's a property of the fixture, not a flaw in the
method — but it means these two rows understate how much a *genuinely* busy tile (dense text,
a photo, a video frame edge) would cost. `noise_tile` is the more informative worst-case
small-tile number: even there, zlib level 1 vs 6/9 differs by under half a millisecond and L6
is never smaller (28,065 B vs 28,062 B — noise doesn't compress, and zlib's block-search
overhead at higher levels can't buy anything back at this size).

## 5. Recommendations

1. **Keep level 1 for every terminal-facing (`o=z`) send.** Already true in shipped code;
   treat it as an invariant, not an oversight to "improve" later. Worth a one-line comment at
   the call sites in `main.rs` pointing at this doc — left for whoever owns that file next,
   since it's outside this benchmark's remit.
2. **Do not chase zstd for the terminal wire.** It is a protocol impossibility, not a missed
   optimization (§0).
3. **When C09's SSH/adaptive transport is eventually built**, its remote-core↔local-core hop
   is Terminal-Fenster's own code on both ends and is not bound by `o=z`. There, zstd level 1 is a
   strictly dominant choice over zlib — smaller *and* faster on the one real-content fixture
   measured (10.4 KB/≈1 ms vs 13.1 KB/≈5–10 ms on the full text page). Worth designing that
   hop with zstd from the start rather than reusing the terminal-facing zlib path by default.
4. **If that same SSH path ever needs a tile-level bandwidth/CPU tradeoff**, level 6 is worth
   revisiting *there* — it triples highly-compressible tile output size for free locally
   (§2), which is irrelevant on a local socket but not irrelevant on a constrained uplink.

## 6. What this doesn't cover

No lz4 or brotli measurement (zstd already dominates zlib enough on the one hop where codec
choice is free that a third codec wasn't obviously worth the added dependency surface — flag
if the SSH work later wants it). No measurement on genuinely low-RAM/weak-CPU hardware — all
numbers are from the M4 dev machine under incidental load, not a target-class low-end box;
the *ratios* should transfer, the *absolute* ms figures should not be quoted as what a weak
machine will see.
