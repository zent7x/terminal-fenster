# A07 — SSH / Remote Operation: Implementation Spec

**Status:** research complete, measurements empirical on target hardware (M4, macOS 26.1)
**Date:** 2026-07-31
**Scope:** topologies, bandwidth math, damage strategy, pty transport, adaptive control, multiplexers

---

## 0. TL;DR — the three findings that change the architecture

1. **Dirty-rectangle damage tracking does not help scrolling. Measured saving: 0.2%.** A 1-pixel scroll dirties 78% of a 64×68 tile grid. Tiled encoding of that damage costs *more* wire bytes (447 KiB) than just re-sending the whole frame (385 KiB), because small tiles destroy zlib's cross-tile context and add per-chunk framing. Damage tracking is worth 81–99% on *typing* and ~0% on *scrolling*. Scrolling is the dominant browsing motion. Therefore damage tracking alone does not make remote operation viable.

2. **The kitty protocol's `a=p` source-rectangle crop makes scrolling cost 55 bytes.** `x, y, w, h` on a *put* action select a source rectangle **of an already-transmitted image**, so a tall pre-rendered page can be uploaded once and scrolled by re-placing a different crop. Measured: 55 bytes/frame vs 387,568 bytes/frame. This is a **7000×** reduction and it is the single most important optimization in this document.

3. **The kitty protocol has no lossy codec.** `f` accepts only `24` (RGB), `32` (RGBA), `100` (PNG). There is no JPEG, no WebP. Measured, WebP q60 is **3.6× smaller** than the best kitty-legal encoding (zlib-1 over raw RGB). That 3.6× is unreachable in topology (a) and reachable in topology (b). This alone justifies shipping a local renderer.

**Primary recommendation:** ship **topology (b′)** — engine remote, renderer local, connected by `ssh -T` (no pty, raw binary pipe, arbitrary codec) — with topology (a) as the zero-install fallback. See §1.4.

---

## 1. Topologies

### 1.1 (a) Engine + renderer both remote; escape sequences over the SSH pty

```
[remote] chromium → compositor → encoder → kitty escapes → pty slave
                                                              │ ONLCR/OPOST
                                                        sshd ─┴─ SSH chan
                                                              │
[local]  ssh client → local terminal (Ghostty/iTerm2) → GPU
```

**Wire format:** base64 inside `ESC _ G ... ESC \`, chunked at 4096 B.

| Property | Value |
|---|---|
| Local install | none (`ssh host terminal-fenster`) |
| Codec ceiling | zlib-deflate over raw RGB, or PNG. **No lossy.** |
| Base64 penalty | +33.3% on every byte, unavoidable |
| Measured wire/frame (1440×900) | 226–388 KB |
| Input latency | 1 RTT (keystroke → remote → frame → local) |
| Failure mode | terminal parser becomes the bottleneck before the network does on LAN |
| Complexity | **lowest** |

**Verdict:** correct default for demos, LAN, and "I have no local install" cases. Unusable below ~25 Mbit for anything but the source-rect scroll path.

### 1.2 (b) Engine remote, frames streamed to a local renderer process

```
[remote] chromium → compositor → WebP/AV1 encoder → stdout (binary, NO pty)
                                                       │  ssh -T   (raw pipe)
[local]  terminal-fenster-render → decode → kitty escapes → local pty (~100 MB/s) → terminal
```

Two sub-variants:

- **(b) side channel** — `ssh -L`/`direct-tcpip` port forward, or a second SSH session. Extra auth, extra connection, firewall exposure. Avoid.
- **(b′) same connection, no pty** — `ssh -T host terminal-fenster-engine`. `-T` disables pty allocation, so stdio is a **clean 8-bit-transparent pipe**: no line discipline, no `ONLCR`, no `OPOST`, no base64 required. This is strictly better than (b) and nearly as simple as (a).

| Property | Value |
|---|---|
| Local install | one binary (`terminal-fenster-render`) |
| Codec ceiling | **anything** — WebP, AVIF, H.264, custom |
| Base64 penalty | **none on the WAN hop** (base64 only on the local pty, which runs at ~100 MB/s) |
| Measured wire/frame (1440×900, WebP q60) | **60–108 KB** |
| vs. topology (a) | **3.6× fewer bytes** (387,568 → 108,078 on the Wikipedia frame) |
| Input latency | 1 RTT, same as (a) |
| Complexity | moderate — needs a framed binary protocol + version negotiation |

**Extra win:** the local renderer holds the terminal state machine, so it can repair after `SIGWINCH`, tmux redraw, or pane switch **without a WAN round trip**. In topology (a) every redraw is a full frame across the WAN.

### 1.3 (c) Engine local, only automation remote

Only CDP/automation JSON crosses the link: ~1–50 KB/s. Bandwidth is a non-issue and latency only affects script step time.

This is not a remote-browsing topology — it is remote *control* of a local browser. Useful for CI and agent workloads. It does not serve the case "my shell is on a box in another region and I want to see a web page." Keep it as a distinct product mode, not a fallback.

### 1.4 Comparison

| | (a) all-remote | (b′) remote engine + local renderer | (c) local engine |
|---|---|---|---|
| Bytes/frame, 1440×900 | 226–388 KB | **60–108 KB** | 0 |
| Achievable fps @ 10 Mbit | 3.2–5.5 | **11.6–20.4** | n/a |
| Achievable fps @ 5 Mbit | 1.6–2.8 | **5.8–10.2** | n/a |
| Scroll cost (source-rect) | 55 B | 55 B (local only, ~0 WAN) | 0 |
| Local install | none | one binary | full browser |
| Video-capable | no | yes (with a real video codec) | yes |
| Works through tmux | yes (passthrough) | yes (passthrough, local) | yes |

**Recommendation:** implement (a) first because it is the smallest viable product and validates the escape-sequence layer; design the frame pipeline so the encoder is pluggable, then add (b′) by swapping the sink. Do not build (b) with a side channel.

---

## 2. Bandwidth math

### 2.1 Raw baseline

1440×900 × 3 B = **3,888,000 B/frame** (24bpp). At 30 fps = **116.6 MB/s**. At 32bpp = 155.5 MB/s. Confirmed impossible — this exceeds the measured local pty ceiling (§4.2) by ~15×.

### 2.2 The `o` compression key — VERIFIED

From the kitty protocol specification:

> "The client can send compressed image data to the terminal emulator, by specifying the `o` key. Currently, only RFC 1950 ZLIB based deflate compression is supported, which is specified using `o=z`."

**`o` exists. Its only legal value is `z`.** RFC 1950 zlib wrapper (not raw deflate, not gzip) — so the payload begins with the 2-byte zlib header (`0x78 0x01` at level 1, `0x78 0x9C` at level 6, `0x78 0xDA` at level 9) and ends with a 4-byte Adler-32.

Format key `f`: `24` = RGB, `32` = RGBA (**default**), `100` = PNG. Nothing else.

`S` key: size of data. When PNG **and** compression are combined the spec explicitly requires `S`:

> "Note that if you use both PNG and compression, then you must provide the `S` key with the size of the PNG data."

For `t=d` (direct) transmission the terminal reads until the final chunk, so `S` is not load-bearing; for `t=f`/`t=t`/`t=s` it is the byte count to read from the object. **Treat `S` as the *compressed* byte count whenever `o=z` is set.**

Source: https://sw.kovidgoyal.net/kitty/graphics-protocol/ · https://github.com/kovidgoyal/kitty/blob/master/docs/graphics-protocol.rst (docs GPL-3.0 — read, do not copy)

### 2.3 Measured codec comparison — real pages, real bytes

Method: headless Chrome 1440×900 screenshots of three real pages; every codec run on the identical RGB buffer. Full harness in §8.

**Wikipedia article (text + images, worst case of the three):**

| codec | bytes | KiB | ratio | enc ms | MB/s @30fps | after base64 |
|---|---|---|---|---|---|---|
| RAW rgb24 | 3,888,000 | 3796.9 | 1.00 | — | 116.6 | 5062.5 KiB |
| **zlib L1, f=24 o=z** | **294,883** | **288.0** | **13.2** | **6.8** | **8.85** | **384.0 KiB** |
| zlib L6, f=24 o=z | 283,354 | 276.7 | 13.7 | 18.1 | 8.50 | 369.0 KiB |
| zlib L9, f=24 o=z | 275,806 | 269.3 | 14.1 | 117.2 | 8.27 | 359.1 KiB |
| zlib L1, f=32 o=z | 322,798 | 315.2 | 12.0 | 8.4 | 9.68 | 420.3 KiB |
| PNG L1, f=100 | 435,851 | 425.6 | 8.9 | 11.9 | 13.08 | 567.5 KiB |
| PNG L6, f=100 | 369,808 | 361.1 | 10.5 | 16.9 | 11.09 | 481.5 KiB |
| QOI | 387,904 | 378.8 | 10.0 | 2.2 | 11.64 | 505.1 KiB |
| QOI + zlib L1 | 224,991 | 219.7 | 17.3 | 3.9 | 6.75 | 293.0 KiB |
| JPEG q50 | 137,902 | 134.7 | 28.2 | 2.0 | 4.14 | *not kitty-legal* |
| JPEG q85 | 235,410 | 229.9 | 16.5 | 2.0 | 7.06 | *not kitty-legal* |
| **WebP q60** | **108,078** | **105.5** | **36.0** | **106.0** | **3.24** | *not kitty-legal* |
| WebP q80 | 132,920 | 129.8 | 29.3 | 28.0 | 3.99 | *not kitty-legal* |
| WebP lossless | 205,710 | 200.9 | 18.9 | 25.4 | 6.17 | *not kitty-legal* |

**Hacker News (dense text, light chrome):** zlib L1 = 205,595 B (18.9×); WebP q60 = 78,116 B (49.8×).
**GitHub repo page:** zlib L1 = 168,934 B (23.0×); WebP q60 = 61,376 B (63.4×).

### 2.4 Conclusions from the codec table

- **`f=24 o=z` at zlib level 1 is the correct kitty-legal default.** L6 buys 4% for 2.7× the CPU; L9 buys 6.5% for **17×** the CPU (117 ms — misses a 30 fps budget on its own). Use L1.
- **Never use `f=32`.** RGBA is 9.5% larger compressed and 33% larger raw for an opaque page render. Drop alpha in the compositor.
- **Never use `f=100` (PNG).** PNG L1 is 48% *larger* than `f=24 o=z` L1 and 75% slower to encode. PNG is only worth it for paletted/low-color content.
- **QOI is not competitive as a kitty transport.** Bare QOI (387 KB) is worse than zlib-1 (294 KB) because QOI has no entropy coder. QOI+zlib (225 KB) *does* beat zlib alone by 24% at only 3.9 ms — but QOI is not a legal `f` value, so it can only be used in topology (b′). There, WebP beats it 2×. **Skip QOI.** (QOI reference impl is MIT — usable, just not useful here.)
- **WebP q60 is 2.7× better than the best kitty-legal option** and is the reason topology (b′) exists. Note the 106 ms encode at `method=2` — use `method=0/1` and re-measure; libwebp is BSD-3-Clause and safe to link.

### 2.5 Escape-sequence framing overhead — computed exactly

Chunking rule, verbatim from the spec:

> "the data will need to be chunked up for transfer. This is done using the `m` key. The pixel data must first be RFC 4648 base64 encoded then chunked up into chunks no larger than `4096` bytes. All chunks, except the last, must have a size that is a multiple of 4."

Per-chunk framing: first chunk = `ESC _ G <control> ; <data> ESC \` (3 + |ctrl| + 1 + data + 2); continuation chunks = `ESC _ G m=1 ; <data> ESC \` (9 + data), final has `m=0`.

| compressed B | base64 B | chunks | framing B | wire B | overhead |
|---|---|---|---|---|---|
| 500 | 668 | 1 | 54 | 722 | 7.48% |
| 5,000 | 6,668 | 2 | 63 | 6,731 | 0.94% |
| 50,000 | 66,668 | 17 | 198 | 66,866 | 0.30% |
| 150,000 | 200,000 | 49 | 486 | 200,486 | 0.24% |
| 290,000 | 386,668 | 95 | 900 | 387,568 | 0.23% |
| 400,000 | 533,336 | 131 | 1,224 | 534,560 | 0.23% |

**Chunk framing is negligible (<0.3%). Base64 is not (+33.3%).** Total wire cost ≈ `ceil(C/3)*4 * 1.0023`.

### 2.6 Achievable fps by link speed (kitty-legal, topology (a))

| strategy | 1 Gbit | 100 Mbit | 25 Mbit | 10 Mbit | 5 Mbit | 1.5 Mbit |
|---|---|---|---|---|---|---|
| full frame zlib-1 (wiki, worst) | 317 | 31.7 | 7.9 | 3.2 | 1.6 | 0.5 |
| full frame zlib-1 (github, best) | 554 | 55.4 | 13.8 | 5.5 | 2.8 | 0.8 |
| full frame PNG f=100 L1 | 215 | 21.5 | 5.4 | 2.1 | 1.1 | 0.3 |
| **typing: single dirty tile** | >1000 | >1000 | >1000 | >1000 | >1000 | **301** |
| **scroll: source-rect re-place** | >1000 | >1000 | >1000 | >1000 | >1000 | **>1000** |

Full-frame remote browsing is **not viable below ~25 Mbit**. The two specialized paths are viable on a 1.5 Mbit link. Design around the specialized paths; treat full-frame as the fallback of last resort.

---

## 3. Damage / dirty-rect strategy

### 3.1 Measured: scroll (Wikipedia article, 64×68 px tiles, 322 tiles)

| scroll dy | tiles changed | % | tiled cost | merged-rect cost | full frame | **saving** |
|---|---|---|---|---|---|---|
| 1 px | 250/322 | 77.6% | 454,965 | 393,315 | 393,982 | **0.2%** |
| 3 px | 253/322 | 78.6% | 457,326 | 393,870 | 394,528 | **0.2%** |
| 10 px | 258/322 | 80.1% | 460,943 | 394,012 | 394,724 | **0.2%** |
| 40 px | 265/322 | 82.3% | 450,069 | 383,898 | 384,627 | **0.2%** |
| 100 px | 274/322 | 85.1% | 468,599 | 395,956 | 396,736 | **0.2%** |
| 300 px | 273/322 | 84.8% | 610,037 | 544,890 | 545,498 | **0.1%** |

**A one-pixel scroll dirties 78% of the screen.** Tiled encoding is 15% *worse* than full-frame. Damage tracking contributes nothing to scrolling.

### 3.2 Measured: typing (same page, characters entering a text box)

| transition | tiles changed | % | tiled cost | merged | full frame | **saving** |
|---|---|---|---|---|---|---|
| 0→1 char | 1/322 | 0.3% | 568 | 568 | 45,756 | **98.8%** |
| 1→2 char | 10/322 | 3.1% | 9,981 | 9,674 | 51,011 | **81.0%** |
| 2→40 char | 6/322 | 1.9% | 5,473 | 1,778 | 51,451 | **96.5%** |

Damage tracking is worth **81–99%** on typing. A keystroke costs **568 bytes** — usable on a modem.

### 3.3 Measured: tile-size sweep (scroll dy=3)

| tile | changed | % | tiled wire cost |
|---|---|---|---|
| 16×17 | 2550/4770 | 53.5% | 740.6 KiB |
| 32×34 | 798/1215 | 65.7% | 550.9 KiB |
| 64×68 | 253/322 | 78.6% | 446.6 KiB |
| 128×136 | 73/84 | 86.9% | 397.2 KiB |
| 240×180 | 28/30 | 93.3% | 375.8 KiB |
| 1440×900 (full) | 1/1 | 100% | 385.3 KiB |

**Smaller tiles are strictly worse when damage is broad** — 16×17 tiles cost 1.9× a full frame. Fine granularity only pays when damage is sparse. Use **64×68** (8 cells × 4 rows) as the tracking granularity and switch modes on the damage fraction.

### 3.4 Measured: crossover point

Synthetic damage of N random 64×68 tiles, tiled cost vs full frame (393,177 B):

| damaged tiles | damage % | tiled wire | winner |
|---|---|---|---|
| 1 | 0.3% | 2,932 | TILED |
| 8 | 2.8% | 14,809 | TILED |
| 32 | 11.2% | 53,097 | TILED |
| 64 | 22.4% | 106,703 | TILED |
| 128 | 44.8% | 198,278 | TILED |
| 286 | 100% | 449,603 | **FULL** |

Tiled cost scales at **≈1.57 KB per 64×68 tile**. Crossover ≈ **85% damage**. Use a conservative threshold of **60%** — beyond that, tiled mode wins only marginally while costing far more escape sequences and terminal parse work.

### 3.5 XOR inter-frame delta is a trap — do not implement

| case | XOR+zlib | plain frame zlib | verdict |
|---|---|---|---|
| static (dy=0) | 22,634 | 393,177 | 0.06× — good, but dirty-rect is better |
| scroll dy=1 | 512,329 | 393,912 | **1.30× WORSE** |
| scroll dy=3 | 569,526 | 394,458 | **1.44× WORSE** |
| scroll dy=10 | 654,888 | 394,654 | **1.66× WORSE** |
| scroll dy=100 | 741,685 | 396,666 | **1.87× WORSE** |
| typing 0→1 | 22,874 | 45,686 | 0.50× — but dirty-rect gives 568 B |

XOR of shifted content destroys spatial correlation and produces high-entropy noise that zlib cannot compress. It is never the best option: dirty-rect beats it 40× on typing and it is nearly 2× worse than doing nothing on scroll.

### 3.6 Scroll-blit — the actual answer

Send only the newly exposed strip:

| dy | strip | strip wire B | full frame wire B | **saving** |
|---|---|---|---|---|
| 1 px | 1×1440 | 1,119 | 393,982 | **99.7%** |
| 3 px | 3×1440 | 1,794 | 394,528 | **99.5%** |
| 10 px | 10×1440 | 2,176 | 394,724 | **99.4%** |
| 17 px (1 row) | 17×1440 | 2,399 | 394,440 | **99.4%** |
| 34 px (2 rows) | 34×1440 | 2,863 | 387,887 | **99.3%** |
| 100 px | 100×1440 | 19,847 | 396,736 | **95.0%** |
| 300 px | 300×1440 | 237,103 | 545,498 | **56.5%** |

But there is a better version still.

### 3.7 Source-rectangle re-placement — 55 bytes per scroll frame

The `x, y, w, h` keys on a **put** action (`a=p`) select a source rectangle of an **already-stored** image. Verbatim from the spec:

> "You can choose a source rectangle (in pixels) as the part of the image to display. This is done with the keys: `x, y, w, h` which specify the top-left corner, width and height of the source rectangle."

> "Every transmitted image can be displayed an arbitrary number of times on the screen, in different locations, using different parts of the source image, as needed."

So: render the page taller than the viewport (e.g. 1440×3000), transmit **once**, then scroll by re-placing a different crop:

```
\x1b_Ga=p,i=1,p=1,x=0,y=1234,w=1440,h=900,c=180,r=53,q=2\x1b\\
```

**55 bytes.** At 60 fps that is 3.2 KiB/s. Versus 387,568 B/frame for a full re-transmit — a **7047×** reduction.

Byte-level: `1B 5F 47` (`ESC _ G`) … `1B 5C` (`ESC \`).

**Constraints and caveats:**
- Only valid while the page content is static. Any DOM mutation, animation, lazy-load, or `position: fixed` header invalidates the cached tall image. Treat it as a **fast path with invalidation**, not the general case.
- Overscroll beyond the pre-rendered height requires a new transmit. Pre-render 2–3 viewport heights ahead and re-upload in the background during scroll idle.
- **Storage quota is implementation-defined.** The spec says only: *"when the terminal is running out of quota space for new images, existing images without placements will be preferentially deleted."* A commonly cited figure of 320 MB is **UNVERIFIED** — it is not in the specification text. A 1440×3000 RGB image is 13 MB uncompressed; budget conservatively and re-transmit on `ENOENT`-style error responses.
- **Note:** `c` and `r` are documented under frame composition as 1-based frame indices for `a=f`; in the placement context they are the display columns/rows. The docs overload these letters. **Verify empirically against Ghostty 1.3.1 and iTerm2 3.6.9 before shipping** — this is the one key in the fast path whose semantics I could not disambiguate from the prose alone. Marked **PARTIALLY VERIFIED**.

### 3.8 Recommended damage state machine

```rust
enum FrameStrategy {
    Idle,                                  // 0 bytes
    ScrollReplace { y: u32 },              // ~55 B    — cached tall image still valid
    ScrollBlit    { strip_h: u32 },        // 1–20 KB  — vertical-only damage, no cache
    DirtyTiles    { tiles: Vec<TileId> },  // ~1.57 KB/tile
    FullFrame,                             // 226–388 KB
}

fn choose(d: &Damage, cache: &TallImageCache) -> FrameStrategy {
    if d.is_empty()                                  { return Idle; }
    if let Some(dy) = d.pure_vertical_scroll() {
        if cache.covers(d.scroll_y)                  { return ScrollReplace { y: d.scroll_y }; }
        if dy.abs() < 200                            { return ScrollBlit { strip_h: dy.unsigned_abs() }; }
    }
    if d.tile_fraction() < 0.60                      { return DirtyTiles { tiles: d.tiles() }; }
    FullFrame
}
```

Detecting `pure_vertical_scroll` should come from the compositor (Chromium already computes scroll deltas), **not** from pixel diffing — pixel-diffing a scroll is exactly the case that produces 78% false damage.

---

## 4. SSH pty throughput and binary safety

### 4.1 Does the line discipline mangle binary? — MEASURED

Test: fork a pty, child writes bytes `0x00..0xFF` to the slave, parent reads the master, compare.

| master termios | sent | received | result |
|---|---|---|---|
| **default (cooked, OPOST on)** | 256 B | 257 B | **`0x0A` → `0x0D 0x0A`**; all 255 other bytes intact |
| **`cfmakeraw()`** | 256 B | 256 B | **byte-identical, no mangling** |

**Exactly one byte is corrupted in cooked mode: `0x0A` (LF), expanded to CRLF by `ONLCR`.** On macOS 26.1 no tab expansion (`OXTABS`/`XTABS`) occurred, and `0x11`/`0x13` (XON/XOFF) passed through untouched in the output direction.

### 4.2 Is base64 safe? — PROVEN

The RFC 4648 base64 alphabet is exactly:

```
+/0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz  (and '=' padding)
```

Intersection with the dangerous set `{0x00, 0x09, 0x0A, 0x0D, 0x11, 0x13, 0x1B, 0x7F}` = **∅**.

**Base64 payloads survive a cooked pty intact.** This is why the kitty protocol mandates base64 — it is not incidental, it is what makes the protocol pty-safe without requiring raw mode. Raw binary would be corrupted by `ONLCR` at every `0x0A`.

**Rules:**
- Topology (a): base64 is mandatory and sufficient. Still call `cfmakeraw()`/`tcsetattr` on your own tty (any TUI does) — belt and braces, and it disables `ISIG` so `0x03` in a payload can't raise SIGINT on the input path.
- Topology (b′): use `ssh -T` (**no pty allocated**) and skip base64 on the WAN entirely — saves 33.3%. Re-encode to base64 only when writing to the *local* pty, which runs at ~100 MB/s.
- Never send raw deflate output through a pty without base64.

### 4.3 Practical pty throughput ceiling — MEASURED (Apple M4)

| write chunk | throughput |
|---|---|
| 4,096 B (kitty's chunk limit) | **93.1 MB/s** |
| 16,384 B | 93.6 MB/s |
| 65,536 B | 102.9 MB/s |

**~93–103 MB/s.** Kitty's 4096-byte chunking costs ~10% versus 64 KB writes — acceptable, and not the binding constraint.

`kern.tty.ptmx_max = 511` on this host — the max number of ptys, not a buffer size.

**The pty is not the bottleneck.** At 388 KB/frame the pty could sustain ~240 fps. The bottleneck is (1) the WAN, and (2) the terminal's escape parser and image decoder, which are far slower than 93 MB/s and for which no primary-source figure exists — **UNVERIFIED, measure per-terminal in A0x benchmarks.**

### 4.4 SSH channel flow control — a real latency-dependent ceiling

From `openssh-portable/channels.h`:

```c
#define CHAN_SES_PACKET_DEFAULT  (32*1024)
#define CHAN_SES_WINDOW_DEFAULT  (64*CHAN_SES_PACKET_DEFAULT)   /* 2 MiB */
#define CHAN_TCP_PACKET_DEFAULT  (32*1024)
#define CHAN_TCP_WINDOW_DEFAULT  (64*CHAN_TCP_PACKET_DEFAULT)   /* 2 MiB */
```

SSH does per-channel flow control. Maximum goodput ≈ `window / RTT`:

| RTT | ceiling from the 2 MiB window |
|---|---|
| 10 ms | 200 MB/s |
| 50 ms | 40 MB/s |
| 100 ms | 20 MB/s |
| 250 ms (intercontinental) | 8 MB/s |
| 500 ms (satellite) | 4 MB/s |

Not binding at our target of 1–3 MB/s, but it interacts badly with bufferbloat: **filling the 2 MiB window with frame data delays keystrokes behind up to 2 MiB of queued pixels.** At 5 Mbit that is 3.4 seconds of input lag. This is the strongest argument for the frame-ACK pacing in §5.3 — never let more than one frame be in flight.

OpenSSH is BSD-style licensed (GitHub reports `NOASSERTION`; the tree is a mix of BSD-2/BSD-3 and public-domain files) — permissive, but we only reference constants, not code.

### 4.5 Escape-sequence size limits in terminals

| layer | limit | source |
|---|---|---|
| kitty protocol chunk | **4096 B** payload, non-final chunks a multiple of 4 | spec, verbatim |
| tmux DCS/input buffer | `INPUT_BUF_DEFAULT_SIZE = 1048576` (1 MiB), adjustable via `input_set_buffer_size()`; oversize sets `INPUT_DISCARD` | `tmux/tmux.h`, `input.c` |
| iTerm2 inline image | 1,048,576 B per sequence; **older tmux: 256 B for the entire sequence** | iterm2.com/documentation-images.html |
| Ghostty | no documented limit | **UNVERIFIED** |

Staying inside kitty's 4096 B chunking keeps you inside every one of these by a wide margin. **Do not "optimize" by sending larger chunks** — it is the only thing protecting you from tmux's discard path.

---

## 5. Adaptive strategy: measuring bandwidth and latency in-band

### 5.1 Capability + geometry handshake (do this once at startup)

```
\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[c
```

Verbatim from the spec:

> "If you get back a response to the graphics query, the terminal emulator supports the protocol, if you get back a response to the device attributes query without a response to the graphics query, it does not."

The trailing `ESC [ c` (DA1) is the **fallback sentinel** — it guarantees a reply even from terminals that ignore `_G` entirely, so you never block forever. `AAAA` is base64 for three zero bytes = one black RGB pixel.

Expected graphics reply: `\x1b_Gi=31;OK\x1b\\`

Geometry (both needed):
- `\x1b[14t` → `\x1b[4;<height>;<width>t` — text area in pixels
- `\x1b[16t` → `\x1b[6;<height>;<width>t` — **cell** size in pixels (more precise, less widely supported)
- `ioctl(TIOCGWINSZ)` → `ws_xpixel`/`ws_ypixel` — works locally, **returns 0 through most SSH sessions**, so the CSI queries are mandatory for remote

Sources: kitty graphics-protocol docs; https://www.xfree86.org/current/ctlseqs.html

### 5.2 Measuring bandwidth in-band — two-point regression

A single timed round trip conflates RTT, per-query overhead, and terminal parse time. Use **two payload sizes and take the slope** — fixed costs cancel:

```rust
// Send a=q with a dummy image of N bytes; terminal must consume all N before replying.
let (n1, t1) = timed_query(64 * 1024);
let (n2, t2) = timed_query(512 * 1024);
let goodput  = (n2 - n1) as f64 / (t2 - t1).as_secs_f64();  // bytes/sec, fixed costs cancelled
let rtt      = t1 - Duration::from_secs_f64(n1 as f64 / goodput);
```

Use `a=q` (query action) rather than `a=T`, because query explicitly does not store or display:

> "the terminal emulator will try to load the image and respond with either OK or an error, as above, but it will not replace an existing image with the same id, nor will it store the image."

Send with `q=0` so you get the OK back. Run the probe once at startup and again after any 5× change in observed frame ACK time.

### 5.3 Continuous estimation — frame ACK pacing (the important part)

Append a DA1 to the final chunk of every frame:

```
… ESC \  ESC [ c
```

The reply `\x1b[?...c` arrives only after the terminal has **parsed and consumed the entire frame**. That single timestamp measures network goodput *and* terminal backpressure together — which is exactly the quantity you must rate-limit on.

```rust
struct Pacer {
    in_flight: usize,
    max_in_flight: usize,   // 1 on WAN, 2 on LAN
    ewma_frame_ms: f64,     // alpha = 0.2
    budget_bytes: usize,
}

impl Pacer {
    fn may_send(&self) -> bool { self.in_flight < self.max_in_flight }

    fn on_ack(&mut self, wire_bytes: usize, elapsed_ms: f64) {
        self.in_flight -= 1;
        self.ewma_frame_ms = 0.8 * self.ewma_frame_ms + 0.2 * elapsed_ms;
        let goodput = wire_bytes as f64 / (elapsed_ms / 1000.0);
        // 70% safety margin keeps the SSH window from filling and delaying keystrokes
        self.budget_bytes = (goodput * self.target_interval_s() * 0.70) as usize;
    }
}
```

**`max_in_flight = 1` is a hard requirement on WAN links.** With the 2 MiB SSH channel window (§4.4), allowing two 388 KB frames in flight adds up to 780 KB of head-of-line blocking in front of the next keystroke.

### 5.4 Quality/fps ladder

Given `budget_bytes` per frame, pick the highest rung that fits. Since the kitty-legal codec has no quality knob, **resolution and frame rate are the only levers in topology (a)**:

| rung | strategy | typical wire | needs |
|---|---|---|---|
| 0 | source-rect re-place | 55 B | any link |
| 1 | dirty tiles, ≤8 tiles | ≤13 KB | ≥1 Mbit |
| 2 | scroll-blit strip ≤64 px | ≤10 KB | ≥1 Mbit |
| 3 | full frame, 1× scale, zlib-1 | 226–388 KB | ≥25 Mbit for 8 fps |
| 4 | full frame, **0.5× scale** (720×450) | ~60–100 KB | ≥10 Mbit |
| 5 | full frame, 0.5× + 4:2:0-style chroma subsample before RGB pack | ~40–70 KB | ≥5 Mbit |

Downscaling to 0.5× is a genuine ~4× byte reduction and the terminal upscales it into the same cell rectangle via `c`/`r`. **In topology (b′) replace rungs 3–5 with WebP quality 80/60/40 instead** — better image quality at the same byte budget.

Adaptation policy: raise one rung after 10 consecutive frames under 60% of budget; drop **two** rungs immediately on any frame that exceeds budget or on an ACK slower than 2× the EWMA. Fast down, slow up.

### 5.5 Latency-driven interactivity rules

- Below ~150 ms measured RTT, echo keystrokes only after the server frame (correct rendering).
- Above ~150 ms, this feels broken. Either accept it or implement local predictive echo in topology (b′) — the local renderer can overlay a cursor/character speculatively and reconcile on the next real frame. This is the Mosh model. **Non-trivial; defer to a later milestone.**
- Never queue input behind pixels: if the engine and renderer share one SSH channel, input must ride an interleaved control frame with strict priority, or a separate multiplexed stream.

---

## 6. Multiplexer interaction (tmux)

### 6.1 `allow-passthrough` is mandatory

```tmux
set -g allow-passthrough on     # passthrough only while the pane is visible
# set -g allow-passthrough all  # passthrough even when the pane is invisible
```

- Introduced in tmux 3.2; **as of tmux 3.3 it must be explicitly set to `on` or `all`** — the default is off.
- `on` silently drops sequences when the pane is not visible. `all` does not. **`all` is dangerous for us**: writing images for an invisible pane corrupts the visible one, since tmux is not tracking image state. **Use `on`.**

Source: https://github.com/tmux/tmux/wiki/FAQ (tmux is ISC-licensed)

### 6.2 The DCS wrapper — exact bytes

```
ESC P t m u x ;  <payload, every 0x1B doubled>  ESC \
1B  50 74 6D 75 78 3B                            1B 5C
```

FAQ, verbatim: *"Any `\033` characters in the wrapped sequence must be doubled."*

Applied to a kitty chunk:

```rust
fn tmux_wrap(inner: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(b"\x1bPtmux;");
    for &b in inner {
        if b == 0x1B { out.push(0x1B); }   // double every ESC
        out.push(b);
    }
    out.extend_from_slice(b"\x1b\\");
}
```

### 6.3 Overhead — negligible

Per kitty chunk: 7 B prefix + 2 B ST + 2 B (the two ESCs in `ESC_G…ESC\` each doubled) = **+11 bytes on ~4105** = **0.27%**.

A 290 KB compressed frame = 95 chunks = **+1045 bytes**. Immaterial.

### 6.4 Real tmux hazards (these matter far more than the byte overhead)

1. **tmux does not model kitty images.** It maintains its own cell grid; a pane redraw, pane switch, window resize, or copy-mode entry repaints cells and the images vanish. **Fix: Unicode placeholders (`U=1`).** Place with codepoint **U+10EEEE**, image ID encoded in the foreground color (256-color or truecolor), and row/column in combining diacritics from the spec's `rowcolumn-diacritics.txt`. tmux then stores the placeholder cells in *its own* grid and reproduces them across redraws for free. Spec example:
   ```
   printf "\e[38;5;42m\U10EEEE\U0305\U0305\U10EEEE\U0305\U030D\e[39m\n"
   ```
   (2×2 placeholder for image ID 42; `U+0305` and `U+030D` are the row/column diacritics.)
   **Placeholders are mandatory for tmux support.** Without them, remote+tmux is unusable.

2. **Input buffer discard.** `INPUT_BUF_DEFAULT_SIZE = 1048576`; `input_input()` sets `INPUT_DISCARD` when the accumulated DCS exceeds it. Kitty's 4096-byte chunking keeps you 250× under. Never merge chunks.

3. **Reported truncation at ~60 characters** (tmux/tmux#4377, tmux 3.6-next, traced through `input_dcs_dispatch` → `screen_write_raw_string`). Issue shows as closed with no maintainer commentary or linked fix in the page content — **status UNVERIFIED.** Add an integration test that pushes a 4 KB passthrough chunk through the exact tmux version in CI and asserts byte-exact arrival at the outer terminal.

4. **tmux 3.6 added native SIXEL** (`--enable-sixel` build flag), **not** kitty graphics. tmux does not render TGP itself; it only passes it through. tmux 3.6a had a crash removing SIXEL images in the alternate screen, **fixed in 3.6b**, which also fixes **CVE-2026-11623**. Require ≥3.6b if users build with sixel.

5. **screen** has no equivalent passthrough. Treat screen as unsupported and detect it (`$TERM=screen*`, `$STY` set) to fail loudly rather than emit garbage.

---

## 7. Terminal support matrix

| terminal | kitty graphics | notes |
|---|---|---|
| kitty | reference impl | GPL-3.0 — **do not copy code**, implement from the spec |
| Ghostty 1.3.1 | yes, via a Zig subsystem parsing `_G` | **Animation frames (`a=f`, `a=a`) unsupported** — maintainer confirmed, discussion #5218 → issue #5255. Feature matrix is not documented; the animation gap may not be the only one. Exact 1.3.1 support level **UNVERIFIED — enumerate empirically.** MIT-licensed, so its implementation *is* safe to read. |
| iTerm2 3.6.9 | **UNVERIFIED** for kitty; ships its own OSC 1337 (§7.1) | |
| Apple Terminal 465 | **none** (no kitty, no sixel, no OSC 1337) | per brief; must degrade to text-only |

### 7.1 iTerm2 OSC 1337 — the lossy escape hatch

```
ESC ] 1337 ; File=inline=1;size=<n>;width=<w>px;height=<h>px : <base64> BEL
1B  5D                                                                07
```
(`BEL` 0x07 may be replaced by ST = `ESC \` = `1B 5C`.)

Docs: *"Any image format that macOS supports will display inline, including PDF, PICT, or any number of bitmap data formats (PNG, GIF, etc.)."*

**This means JPEG works on iTerm2.** From the §2.3 table, JPEG q50 = 137,902 B vs kitty-legal zlib-1 = 294,883 B — a **2.1× win** on iTerm2 in topology (a), where WebP is otherwise unreachable. WebP support is plausible (macOS ImageIO decodes WebP) but **not documented — UNVERIFIED, test before relying on it.**

Limits: 1,048,576 B per sequence in iTerm2 and modern tmux; **256 B in older tmux**. iTerm2 3.5+ added multipart sequences for tmux.

Recommendation: implement OSC 1337 + JPEG as an **iTerm2-specific fast path**. It is ~40 lines and doubles throughput for that terminal.

### 7.2 Licenses of everything referenced

| project | SPDX | usable? |
|---|---|---|
| kovidgoyal/kitty | **GPL-3.0** | **Spec/docs only. Do not copy or vendor code.** Protocol itself is a public interface. |
| ghostty-org/ghostty | MIT | yes — safe to read and adapt with attribution |
| tmux/tmux | ISC | yes |
| phoboslab/qoi | MIT | yes (but §2.4: not worth using) |
| webmproject/libwebp | BSD-3-Clause | yes — link for topology (b′) |
| openssh-portable | BSD-style (`NOASSERTION` on GitHub; mixed BSD-2/BSD-3/public-domain) | constants referenced only |
| zlib | zlib license | yes |

**Action:** implement the kitty graphics encoder from the published specification only. Do not read kitty's `graphics.c`. If a reference implementation is needed, read **Ghostty's (MIT)**.

---

## 8. Measurement methodology and reproduction

All numbers in §2.3, §3.1–3.6, §4.1–4.3 are measured on the target host (macOS 26.1, Apple M4, 24 GB), not estimated.

- **Page corpus:** `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome --headless --disable-gpu --hide-scrollbars --window-size=1440,900 --screenshot=out.png <url>` against `en.wikipedia.org/wiki/Terminal_emulator`, `news.ycombinator.com`, `github.com/kovidgoyal/kitty`. Screenshots in `/tmp/bgshots/`.
- **Scroll simulation:** one 1440×3000 capture, cropped to 1440×900 windows at varying `y` — an exact model of scrolling, free of render nondeterminism.
- **Typing simulation:** identical page rendered with 0/1/2/40 characters in a fixed-size box.
- **Codecs:** Pillow 11.3.0 (PNG/JPEG/WebP), stdlib `zlib`, `qoi` from PyPI, NumPy for tile diffing.
- **pty tests:** `pty.fork()`, byte census `0x00..0xFF`, `tty.setraw()` vs default; throughput via 48 MB at 4 K/16 K/64 K write sizes.
- Harnesses: `/tmp/bench.py`, `/tmp/damage.py`, `/tmp/damage2.py`, `/tmp/ptytest.py`, `/tmp/ptythru.py`, `/tmp/framing.py`. **Move these into `benchmarks/` before they are lost — `/tmp` is volatile.**

### 8.1 Explicitly UNVERIFIED

- Ghostty 1.3.1's exact kitty feature matrix (only "no animation frames" is confirmed, and not version-pinned).
- Whether iTerm2 3.6.9 implements kitty graphics at all.
- iTerm2 WebP decode support.
- Kitty image storage quota (the widely repeated "320 MB" is not in the spec).
- Terminal-side escape-parse/decode throughput for any terminal (the likely real bottleneck on LAN).
- tmux #4377's ~60-character passthrough truncation: current status and whether it affects 3.6b.
- Real SSH-over-WAN goodput: **not measured** — sshd is disabled on this host and enabling it requires admin. The 93–103 MB/s figure is the **local pty ceiling only**. Measure real SSH before committing to the ladder in §5.4.
- `c`/`r` key semantics in the placement context (docs overload them with animation frame indices).

---

## 9. Implementation checklist

1. Encoder emits `f=24, o=z` at **zlib level 1**. Never `f=32`, never `f=100`.
2. Chunk base64 at exactly **4096 B**, non-final chunks a multiple of 4, `m=1`/`m=0`.
3. Get scroll deltas from the **compositor**, never from pixel diffing.
4. Implement the §3.8 strategy ladder. **Source-rect re-placement is the highest-value item in the entire project** — build it first.
5. Damage granularity **64×68 px**; switch to full-frame above **60%** damage.
6. **Do not implement XOR inter-frame delta.** It is measurably worse than doing nothing on scroll.
7. Startup handshake: `_G a=q` + DA1 sentinel; geometry via `CSI 14 t` and `CSI 16 t` (`TIOCGWINSZ` returns zeros over SSH).
8. Frame ACK pacing with **`max_in_flight = 1`** on WAN. Non-negotiable — it is what keeps keystrokes out from behind 2 MiB of queued pixels.
9. Unicode placeholders (`U=1`, U+10EEEE) — **required** for tmux; make it the default placement mode everywhere for redraw resilience.
10. tmux: wrap in `ESC P tmux;` with doubled ESCs; document `set -g allow-passthrough on`; detect `screen` and fail loudly.
11. `ssh -T` for topology (b′): no pty, no base64 on the WAN, WebP encoder → 3.6× fewer bytes.
12. iTerm2: OSC 1337 + JPEG fast path, ~2.1× over kitty-legal on that terminal.
13. Implement the kitty encoder **from the spec text only**. Do not read GPL-3.0 kitty source. Use Ghostty (MIT) if a reference is needed.

---

## 10. Sources

- Kitty terminal graphics protocol — https://sw.kovidgoyal.net/kitty/graphics-protocol/ and https://github.com/kovidgoyal/kitty/blob/master/docs/graphics-protocol.rst (GPL-3.0)
- tmux FAQ, passthrough — https://github.com/tmux/tmux/wiki/FAQ (ISC)
- tmux source, `INPUT_BUF_DEFAULT_SIZE`, `input_dcs_dispatch` — https://github.com/tmux/tmux/blob/master/tmux.h, `/input.c`
- tmux passthrough truncation report — https://github.com/tmux/tmux/issues/4377
- tmux 3.6b / CVE-2026-11623 — https://seclists.org/oss-sec/2026/q2/934
- OpenSSH channel window constants — https://github.com/openssh/openssh-portable/blob/master/channels.h
- iTerm2 inline images — https://iterm2.com/documentation-images.html
- Ghostty kitty graphics gaps — https://github.com/ghostty-org/ghostty/discussions/5218 (→ issue #5255)
- xterm control sequences (`CSI 14 t`, `CSI 16 t`) — https://www.xfree86.org/current/ctlseqs.html
- RFC 1950 (zlib), RFC 4648 (base64)
