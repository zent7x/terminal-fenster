# C02 — Sixel Backend Design (Implementation Spec)

**Mission:** C02 · **Status:** Complete, encoder prototyped and cross-validated · **Date:** 2026-07-31
**Host:** macOS 26.1, Apple M4, arm64
**Owned output:** this file only. Every change to `crates/`, `apps/cli/`, and
`apps/engine/src/main.js` is **specified here as an instruction for the commander** (§11), not applied.

**Evidence classes:** `[MEASURED]` = produced and timed on this machine this session ·
`[SPEC]` = primary protocol documentation · `[REPO]` = read from this checkout ·
`[PROXY]` = measured on a synthetic frame that stands in for Chromium output (see §1.2) ·
`[UNVERIFIED]` = could not confirm; do not build on it.

---

## 0. Executive answer

| Question | Answer |
|---|---|
| Is the design implementable in Rust with no new dependencies? | **Yes.** The prototype is 1 file, `std` only. No `flate2` (Sixel has no compression), no `libsixel`. |
| Does the emitted wire format actually decode? | **Yes, byte-exactly.** Three 2482×814 frames encoded by the prototype and decoded by ImageMagick 7.1.2-18's independent SIXEL reader reproduce the quantiser's own reconstruction **byte for byte** `[MEASURED]` (§8.3). |
| Palette strategy | **Fixed 256-register palette: 40-step neutral ramp + 6×6×6 RGB cube**, snapped to Sixel's percent grid. Nearest-colour via a 32,768-entry LUT built once (5.7–6.1 ms). No per-frame quantiser. |
| Biggest surprise | **Sixel colour is 101 levels per channel, not 256** (`Px` is percent). Defining the palette in 8-bit and converting adds a silent ±1 to every channel of every pixel. Defining it on the percent grid makes the round trip exact (§4). |
| Second biggest surprise | **Dithering costs 2.6× the wire bytes and makes pixel error worse.** Ordered dither destroys the runs that Sixel's RLE depends on. **Default OFF** (§6). |
| Full-frame cost at 2482×814 | encode p50 **12.62 ms**, wire **669,407 B** on a text page `[MEASURED, PROXY]`. At 60 fps that is **40.2 MB/s** of PTY. **Full-frame Sixel at 60 fps is not viable.** |
| Damage cost | Caret blink **1,363 B / 0.030 ms**; one text line **33,682 B / 0.578 ms** `[MEASURED, PROXY]`. Damage is a **490×** byte reduction. |
| Damage mechanism | Sixel has **no image ids, no placements, no partial-update primitive**. Damage = **redraw whole cell-aligned tiles at the text cursor**. 16×8-cell tiles (30/screen) cost **+7.6%** on a full redraw (§9.4). |
| vs Kitty on the same frame | Sixel **669,407 B** vs Kitty **53,999 B** `[REPO: B01-architecture-rfc.md:46]` — **12.4× more wire**. Encode cost is comparable (12.62 ms vs ≥11.53 ms `[REPO: B05:211]`). |
| Who is this for? | **xterm, mlterm, Konsole, WezTerm, foot, contour, Windows Terminal.** Not Ghostty (refuses Sixel), not Apple Terminal (nothing). iTerm2 is **contested** — see §2.2. |

**The single most actionable recommendation is in §14.**

---

## 1. Evidence base

### 1.1 Reproduction harness

Written this session; it is the regression suite for every number below. Keep it.

| File | Purpose |
|---|---|
| `…/scratchpad/sixel_proto.rs` | Complete reference encoder: palette, LUT, dither, band encoder, RLE. `rustc -O -o sixel_proto sixel_proto.rs`, `std` only. |
| `…/scratchpad/make_proxies.py` | Generates the three proxy frames at the real measured viewport (2482×814). |
| `…/scratchpad/proxy_{text,ui,photo}.{png,rgb,six}` | Inputs and encoder outputs. |
| `…/scratchpad/example66.six` | The §8 worked example, 59 bytes. |
| `…/scratchpad/pctramp.six` | The 101-column probe that established the percent→8-bit mapping (§4). |

Full scratchpad path prefix:
`/private/tmp/claude-501/-Users-adeebbashir/a6555dd0-1471-4951-aa0d-5958b606ca83/scratchpad/`

Modes: `example`, `encode <rgb> <w> <h> [out] [amp]`, `bench <rgb> <w> <h> <reps>`,
`damage <rgb> <w> <h> <cellw> <cellh>`, `tiles <rgb> <w> <h> <cellw> <cellh>`,
`dither-sweep <rgb> <w> <h>`, `palette-cost <rgb> <w> <h>`.

### 1.2 What is real and what is a proxy

The commander's brief records that Chromium child processes fail under the agent Bash
sandbox, and the machine is at a lock screen. **I did not capture a real Chromium frame.**
The three test frames are synthetic, built with PIL to have the properties that actually
drive a Sixel encoder — large flat regions, antialiased dark-on-light text sitting on the
neutral axis, a few saturated brand colours, and a separate continuous-tone case:

| Proxy | Stands for | Regs used | Colour passes/band |
|---|---|---|---|
| `proxy_text` | a document/article — **the dominant real case** | 53 | 20.7 |
| `proxy_ui` | app chrome, flat panels — the RLE best case | 38 | 15.6 |
| `proxy_photo` | continuous-tone gradient — the palette worst case | 254 | 51.1 |

They are labelled `[PROXY]` everywhere below. **Re-run `bench` against a real captured
BGRA frame before any of these numbers are promoted to a performance gate.** The encoder
itself is not a proxy: it is a working implementation whose output a third-party decoder
accepts byte-exactly.

### 1.3 Facts taken from the repo

| Fact | Source |
|---|---|
| DA1 param `4` is the Sixel capability bit; already parsed correctly | `crates/bg-term/src/caps.rs:172`, `parse_da1_has_sixel` |
| Ghostty 1.3.1 DA1 = `?62;22;52c` → no Sixel; Apple Terminal = `?1;2c` → no Sixel | `caps.rs:281-289` (tests), A04 §3.2 |
| `Backend::Sixel` exists and `best_backend()` can return it | `crates/bg-term/src/lib.rs:83`, `caps.rs:56-63` |
| `BLACKGLASS_BACKEND=sixel` is accepted | `apps/cli/src/main.rs:211` |
| **`Renderer::present` routes everything that is not Kitty to the Unicode half-block path** | `apps/cli/src/main.rs:858-859` — the `_ =>` arm. Selecting Sixel today silently renders half-blocks. Also flagged by B01 §250. |
| Frame header already carries a dirty rect (`dirty_x/y/w/h`, u32 BE) | `crates/bg-proto/src/lib.rs:24-27,42-45` |
| Kitty baseline on the identical 2482×814 frame: 53,999 wire bytes | B01-architecture-rfc.md:46 |
| Kitty encode really costs ≥11.53 ms, not 0.74 ms | B05-shared-texture-analysis.md:211 |
| PTY write throughput, 1 KiB chunks: 400,000 B → 0.987 ms p50; 40,000 B → 0.138 ms p50 | A10-performance-plan.md §0.1 |

---

## 2. Scope

### 2.1 Why build this at all

A04 §3.3 concluded: *do not implement Sixel*, because on the three measured terminals it
buys nothing. That conclusion is correct **for those three terminals** and this document
does not overturn it. Sixel earns its place only as the answer to a different question —
what BlackGlass does on the terminals it has not yet met. Sixel is the single most widely
implemented raster protocol in that set: xterm, mlterm, Konsole, WezTerm, foot, contour,
Windows Terminal. The alternative for those users today is the Unicode half-block
fallback, which A04 and `unicode.rs:8-10` both describe as unable to render body text.

So: **Sixel is not on the critical path, and it must not be allowed to delay Kitty work.**
It is the second pixel-exact backend, and §11 sequences it behind a wiring fix that is
worth doing regardless.

### 2.2 An unresolved conflict the commander must settle

The mission brief states iTerm2 3.6.9 is **UNVERIFIED** because macOS TCC blocks
automation. A04 §3.2 reports **`[EMPIRICAL]`** iTerm2 measurements — DA1
`\x1b[?64;1;2;4;6;17;18;21;22;52c` (has `4`), 256 colour registers, max geometry
1120×850 — apparently captured before TCC blocked the path, with raw results in
`out-iterm2.json`.

These cannot both be current. This matters concretely: **if A04 is right, there is a
Sixel-capable terminal on this machine and the backend can be end-to-end tested locally;
if the brief is right, every claim in this document is verified only against ImageMagick's
decoder and never against a terminal.** Resolve before implementation starts. My design
assumes the pessimistic case and therefore leans entirely on decoder-based and
protocol-query-based verification, which is CI-able either way.

---

## 3. Wire format — complete grammar

Primary sources: DEC VT330/VT340 Programmer Reference Vol. 2 ch. 14 (vt100.net) and
xterm `ctlseqs`. Both fetched this session; quotations below are short and attributed.

### 3.1 Envelope

```
DCS P1 ; P2 ; P3 q <sixel data> ST
ESC  P                q            ESC \
0x1b 0x50           0x71           0x1b 0x5c
```

This is a **DCS** (Device Control String), not an APC like Kitty. Consequence for us:
a terminal that does not understand DCS still consumes bytes until `ST`, so a blind emit
is *usually* safe — but unlike Kitty's APC there is no in-band `OK` response, so **Sixel
support can only be established from DA1, never from the graphics protocol itself.**

| Param | Meaning | We send |
|---|---|---|
| **P1** | pixel aspect ratio. Omitted/`0`/`1` → 2:1, `2` → 5:1, `3`/`4` → 3:1, `5`/`6` → 2:1, `7`–`9` → 1:1 `[SPEC]` | `0`, then override via raster attributes |
| **P2** | background select. `0` or `2` (default) → *"Pixel positions specified as 0 are set to the current background color"*; `1` → *"Pixel positions specified as 0 remain at their current color"* `[SPEC: vt100.net ch.14]` | **`1`** |
| **P3** | horizontal grid size. *"The VT300 ignores this parameter"* `[SPEC]` | `0` |

**P2=1 is a deliberate choice, and it is load-bearing twice.** (a) It stops the terminal
painting a background rectangle before our pixels land, which is a visible flash on every
frame. (b) A damage tile whose height is not a multiple of 6 has a partial final band; with
P2=0 the unused rows of that band are painted background and **clobber the row of page
underneath the tile**. With P2=1 they are left alone. We additionally zero-pad the final
band's bits and emit an exact `Pv`, so the correctness does not rest on P2 alone.

### 3.2 Raster attributes — always emit first

```
" Pan ; Pad ; Ph ; Pv
```
`Pan`/`Pad` = aspect numerator/denominator, `Ph`/`Pv` = image width/height in pixels
`[SPEC]`. We always emit `"1;1;<w>;<h>`. Omitting it leaves the image at P1's 2:1 aspect
and makes the terminal grow the image as it decodes — a visible artefact and a
reallocation storm in the decoder.

### 3.3 Colour

```
# Pc                            select register Pc
# Pc ; Pu ; Px ; Py ; Pz        define register Pc
```
`Pu=1` → HLS (`Px` hue 0–360°, `Py` lightness 0–100, `Pz` saturation 0–100).
`Pu=2` → RGB, *"Px (0-100% red), Py (0-100% green), Pz (0-100% blue)"* `[SPEC: vt100.net ch.14]`.

**The components are percent, not 0–255.** This is the single most common Sixel bug and it
fails silently — the image renders, just washed out. §4 turns this from a footnote into a
palette design constraint.

### 3.4 The six-vertical-pixel encoding and the +63 offset

Each data character encodes **one column of 6 vertical pixels**. The spec: *"Each sixel
data character represents a binary value equal to the character code value minus hex 3F"*,
and **the least significant bit is the top pixel** `[SPEC: vt100.net ch.14]`.

```
value = byte - 0x3F            byte = 0x3F + value        value ∈ 0..=63
legal bytes: '?' (0x3F, all six off)  …  '~' (0x7E, all six on)

bit 0 (LSB) = row 0 = TOP        bit 5 (MSB) = row 5 = BOTTOM
```

| bits set | value | byte | char | pattern (top→bottom) |
|---|---|---|---|---|
| none | 0 | 0x3F | `?` | `......` |
| 0 | 1 | 0x40 | `@` | `#.....` |
| 0,1,2 | 7 | 0x46 | `F` | `###...` |
| 3,4,5 | 56 | 0x77 | `w` | `...###` |
| 0..5 | 63 | 0x7E | `~` | `######` |

The offset exists so every byte lands in printable ASCII and survives a 7-bit path.
`0x3F + value` can never collide with the four in-band control characters below, because
they are all `< 0x3F`.

### 3.5 In-band control characters

| Char | Hex | Name | Meaning |
|---|---|---|---|
| `!` | 0x21 | Graphics Repeat Introducer | `! Pn <char>` — repeat `<char>` `Pn` times |
| `$` | 0x24 | Graphics Carriage Return | *"active position returns to the left page border of the same sixel line"* `[SPEC]` — this is what lets a second colour overlay the **same** band |
| `-` | 0x2D | Graphics New Line | *"active position moves to the left margin of the next sixel line"* `[SPEC]` — down one 6-pixel band |
| `#` | 0x23 | Colour Introducer | §3.3 |
| `"` | 0x22 | Raster Attributes | §3.2 |

`$` and `-` together define the encoder's whole control flow: **for each band, for each
colour present, one full-width pass separated by `$`; `-` at the end of the band.**

---

## 4. The percent colour model — a measured constraint, not a footnote

`Px,Py,Pz` are percent, so a channel has **101 reachable levels**, giving 101³ = 1,030,301
expressible colours rather than 16.7 M. What is *not* in the spec is the rounding rule the
decoder uses to get back to 8 bits, and that rule determines whether our palette is
representable at all.

**Probe** `[MEASURED]`: `pctramp.six` — a 101-column, one-band image where column *i* uses
register *i* defined as `#i;2;i;i;i`. Decoded with ImageMagick 7.1.2-18:

```
pct :   0   1   2   3   4  …  30  …  50  …  70  …  99  100
8bit:   0   3   5   8  10  …  77  …  128 …  179 …  252  255
```

All 101 outputs are distinct, and the mapping is exactly **round-half-up**:

```rust
fn pct_to_u8(p: u8) -> u8 { ((p as u32 * 255 + 50) / 100) as u8 }
```

(Note `round(76.5)` is 76 under banker's rounding but the decoder gives 77 — a naive
`round()` in the palette builder reintroduces the very error this is meant to remove.)

**Consequence, and this is the design rule:** define every palette entry **in percent** and
derive its 8-bit value with `pct_to_u8`. Then the encoder's nearest-colour search optimises
against colours the terminal can actually produce, and the round trip is exact.

**Verification of the rule** `[MEASURED]`, all three proxies at 2482×814:

| Palette defined in | Decoded output vs quantiser's reconstruction |
|---|---|
| 8-bit, converted at emit time | differs on **281,313 / 29,946 / 142,773** bytes, max delta **1** |
| percent grid (`pct_to_u8`) | **BYTE_IDENTICAL = True** on all three, 6,061,044 B each |

A max delta of 1 is cosmetically irrelevant. It is worth eliminating anyway because it
makes the encoder's error model *exact*, which turns "does this frame match?" into a
byte comparison — the only kind of test that will still be trustworthy in a year.

---

## 5. Palette and quantiser

### 5.1 Why a fixed palette, not a per-frame one

The obvious design — median-cut or octree over each frame — is wrong here for three
reasons, in increasing order of severity:

1. **Cost.** A per-frame quantiser over 2 M pixels is milliseconds we do not have; §10
   shows the encoder already overruns the frame budget without one.
2. **Temporal instability.** A palette derived from frame *N* differs from frame *N+1*, so
   flat regions shimmer between neighbouring registers even where the page did not change.
3. **It breaks damage updates outright.** A damage tile is quantised against *some*
   palette; the pixels around it were drawn against the palette of an earlier frame. If
   those differ, every tile boundary becomes a visible seam. A fixed palette makes tile
   output a pure function of tile content — which is exactly the property tiling needs.

Reason 3 is decisive. **The palette must not depend on frame content.**

### 5.2 The palette

256 registers, all on the percent grid:

| Range | Contents | Percent values |
|---|---|---|
| `0..=39` | 40-step neutral ramp | `round(i*100/39)`, i = 0..39 |
| `40..=255` | 6×6×6 RGB cube | each axis ∈ {0, 20, 40, 60, 80, 100} → 8-bit {0, 51, 102, 153, 204, 255} |

The neutral ramp is the whole point. Web pages are overwhelmingly antialiased dark-on-light
text, and those pixels lie on or near the neutral axis. A bare 6×6×6 cube offers **six**
grey levels (step 51, max error 25) and renders body text as mush. Forty steps give a max
neutral error of **3** — invisible — for 40 of 256 registers. Measured mean absolute error
per channel `[MEASURED, PROXY]`: **2.33** on `proxy_text`, **1.65** on `proxy_ui`.

Cube index: `40 + r*36 + g*6 + b`. Pure red → register **220**, pure blue → **45**
(both used in §8).

### 5.3 Nearest-colour lookup

A 15-bit LUT, built once per session and then O(1) per pixel:

```
index = (r>>3)<<10 | (g>>3)<<5 | (b>>3)        32,768 entries, 32 KiB
```

Two details that matter:

- Each 5-bit bucket is reconstructed at its **centre** (`v*255/31`), not its floor.
  Using the floor skews the entire image dark by ~4/255.
- Distance is **luma-weighted** squared error, `2·dr² + 4·dg² + db²`. Unweighted RGB
  distance happily swaps a near-neutral grey for a tinted cube entry, which is precisely
  the error the eye catches in text.

Build cost `[MEASURED]`: **5.7–6.1 ms**, once, at backend init — not per frame. Quantising
a full 2482×814 frame with it: **p50 3.76 ms** (`proxy_text`, 25 reps).

### 5.4 Register negotiation — do not assume 256

xterm's compiled default is small (16 in common builds); 256 is typical elsewhere but not
guaranteed. Negotiate with XTSMGRAPHICS `[SPEC: xterm ctlseqs]`:

```
CSI ? Pi ; Pa ; Pv S        Pi=1 colour registers, Pi=2 sixel geometry
                            Pa=1 read, Pa=2 reset to default, Pa=3 set to Pv, Pa=4 read maximum
reply: CSI ? Pi ; Ps ; Pv S     Ps: 0 success, 1 bad Pi, 2 bad Pa, 3 failure
```

Sequence at init: read max (`CSI ? 1 ; 4 ; 0 S`) → request `min(256, max)` via
`CSI ? 1 ; 3 ; <n> S` → read back (`CSI ? 1 ; 1 ; 0 S`) and **believe the read-back, not the
request**. Also read `Pi=2` for max geometry; A04 §3.2 measured iTerm2 reporting
1120×850 there and notes it tracks the window, so **re-query after every resize** and tile
if the page exceeds it.

If fewer than 256 registers are granted, fall back to a 16-entry palette (10 greys + 6
saturated hues). Measured cost of that fallback on `proxy_text` `[MEASURED, PROXY]`: MAE
rises **2.33 → 7.86**, max error **43 → 127**. Text stays legible (the 10 greys carry it);
colour does not. Wire size actually *drops* to 464,321 B because there are fewer colour
passes per band. `doctor` must say plainly that the terminal granted a reduced palette
rather than letting the user think the renderer is broken.

---

## 6. Dithering — measured, and the answer is "off"

The textbook move for a 256-colour palette is ordered dithering. I implemented it (8×8
Bayer, amplitude and a neutral-axis guard band, dither phase keyed to **page** coordinates
so a tile dithers identically to the full frame and cannot seam) and measured it.

`proxy_text`, 2482×814 `[MEASURED, PROXY]`:

| amplitude | neutral guard | MAE/chan | max err | wire bytes | vs no dither |
|---|---|---|---|---|---|
| **0** | — | **2.33** | 43 | **669,407** | — |
| 16 | 0 | 3.22 | 53 | 1,346,048 | **+101%** |
| 32 | 0 | 5.62 | 55 | 1,753,013 | **+162%** |
| 51 | 0 | 8.78 | 59 | 2,134,002 | **+219%** |
| 32 | 16 | 2.53 | 55 | 805,046 | +20% |
| 51 | 16 | 2.74 | 59 | 818,852 | +22% |

`proxy_photo`: 477,357 B → 1,322,918 B at amplitude 32 (**+177%**), MAE 11.50 → 12.86.

Two things are going on. Pixel-wise error gets *worse* by construction — that is expected
and not itself an argument, since dither trades measurable error for perceptual smoothness.
The argument is the second column: **Sixel is a run-length format, and dithering exists
precisely to break up runs.** Every 2-pixel alternation the dither introduces is a
terminated RLE token. On a protocol whose entire compression story is `!Pn`, that costs
2–3× the bytes — and §10 shows bytes are the thing we are short of.

**Decision: dithering defaults to OFF.** Keep the implementation behind
`BLACKGLASS_SIXEL_DITHER=<amplitude>` for photo-viewing, and if it is ever enabled by
default, only with the neutral guard (which preserves text at +20% rather than +162%).
This inverts the usual advice, so the reason belongs in the code comment, not just here.

---

## 7. The band encoder

### 7.1 Algorithm

```
emit  ESC P 0;1;0 q  "1;1;W;H
for each register used anywhere in the image:  emit  #Pc;2;pr;pg;pb
for band in 0 .. ceil(H/6):
    rows = min(6, H - band*6)
    clear the per-colour column planes touched by the previous band
    for r in 0..rows:                      # build 6-bit columns
        for x in 0..W:
            plane[idx[y][x]][x] |= 1 << r  # LSB = TOP row of the band
    for each colour c present in this band, ascending:
        if not first: emit '$'             # back to left margin, SAME band
        emit '#c'
        emit RLE of plane[c][0 .. last_nonzero]
    if not last band: emit '-'             # down one band
emit ESC \
```

**Complexity is O(W·H)** for plane construction — each pixel is touched once — plus
O(W · colours_per_band) for emission. That second term is why colours-per-band is the cost
driver, and why the photo proxy (51.1 colours/band) encodes at nearly 2× the text proxy
(20.7). `[MEASURED, PROXY]`

Two allocation notes for the implementation: the colour-plane buffer is
`256 × W` bytes (635 KiB at 2482 wide), allocated **once** and reused across bands and
frames; only the planes actually touched in the previous band are cleared, not all 256.

### 7.2 Trailing-zero trim

Per colour pass, emission stops at the last non-zero column. Every trimmed column is a
pixel that some *other* colour pass in the same band paints, so nothing is lost. On flat
pages this removes most of the tail of most passes.

### 7.3 RLE thresholds — derived, then measured

`! Pn c` costs `2 + digits(Pn)` bytes; a literal run costs `n`. So RLE wins when
`n > 2 + digits(n)` → **emit RLE at n ≥ 4**, literals below (at n = 3 it is a tie; literals
avoid an extra token).

Capping `Pn` guards against decoders with narrow repeat parsing. Measured cost of the cap
on `proxy_text` `[MEASURED, PROXY]`:

| max `Pn` | wire bytes | RLE tokens | cost vs unbounded |
|---|---|---|---|
| 63 | 727,388 | 126,831 | +10.4% |
| **255** | **669,407** | 110,656 | **+1.6%** |
| unbounded | 658,639 | 108,346 | — |

**Cap at 255.** 1.6% is cheap insurance; 63 is not worth it.

### 7.4 Palette definitions: re-send every image

Registers are terminal-global state, and at least one terminal family offers *private*
colour registers per graphic — meaning definitions may not persist between images at all.
I could not confirm the mode number or its default from the `ctlseqs` text in this session
(commonly cited as private mode `1070`) `[UNVERIFIED]`.

The design does not need the answer. Measured cost of defining every used register in
every image `[MEASURED, PROXY]`: **754 bytes on `proxy_text`, 0.11% of the frame**
(669,407 vs 668,653 without). Re-sending is correct under either behaviour and costs
nothing. **Always define; never rely on persistence.** This is the cheapest correctness
purchase in the document.

---

## 8. Worked byte-level example — 6×6, two colours

### 8.1 Input

A 6×6 image: rows 0–2 pure red `(255,0,0)`, rows 3–5 pure blue `(0,0,255)`. One band
(6 rows = exactly one sixel line), two colours, width 6.

Quantisation picks cube registers **220** (red: `40 + 5*36 = 220`) and **45**
(blue: `40 + 5 = 45`). In percent: red `100;0;0`, blue `0;0;100`.

Column bit patterns, LSB = top:

```
red   rows 0,1,2 → bits 0,1,2 → 0b000111 = 7   → byte 0x3F+7  = 0x46 = 'F'
blue  rows 3,4,5 → bits 3,4,5 → 0b111000 = 56  → byte 0x3F+56 = 0x77 = 'w'
```

All 6 columns are identical within each colour, and 6 ≥ 4, so both passes use RLE: `!6w`,
`!6F`. Passes are emitted in ascending register order (45 before 220), separated by `$`
because they share the same band. There is no `-`: this is the only band.

### 8.2 Output — 59 bytes `[MEASURED]`

Printable form (`ESC` shown as `ESC`):

```
ESC P 0;1;0 q "1;1;6;6 #45;2;0;0;100 #220;2;100;0;0 #45 !6w $ #220 !6F ESC \
```

Actual bytes, no spaces:

```
1b 50 30 3b 31 3b 30 71 22 31 3b 31 3b 36 3b 36 23 34 35 3b 32 3b 30 3b
30 3b 31 30 30 23 32 32 30 3b 32 3b 31 30 30 3b 30 3b 30 23 34 35 21 36
77 24 23 32 32 30 21 36 46 1b 5c
```

Field by field:

| Bytes | ASCII | Meaning |
|---|---|---|
| `1b 50` | `ESC P` | DCS introducer |
| `30 3b 31 3b 30` | `0;1;0` | P1=0 aspect, **P2=1 leave 0-bits unchanged**, P3=0 |
| `71` | `q` | DCS final — sixel data follows |
| `22 31 3b 31 3b 36 3b 36` | `"1;1;6;6` | raster attrs: 1:1 aspect, 6×6 px |
| `23 34 35 3b 32 3b 30 3b 30 3b 31 30 30` | `#45;2;0;0;100` | define reg 45 = RGB 0%,0%,100% |
| `23 32 32 30 3b 32 3b 31 30 30 3b 30 3b 30` | `#220;2;100;0;0` | define reg 220 = RGB 100%,0%,0% |
| `23 34 35` | `#45` | select blue |
| `21 36 77` | `!6w` | repeat `w` (0x77 → value 56 → bits 3,4,5) six times |
| `24` | `$` | graphics CR — left margin, **same band** |
| `23 32 32 30` | `#220` | select red |
| `21 36 46` | `!6F` | repeat `F` (0x46 → value 7 → bits 0,1,2) six times |
| `1b 5c` | `ESC \` | ST |

### 8.3 Independent verification

```
$ ./sixel_proto example                     # writes example66.six, 59 bytes
$ magick sixel:example66.six example66.png
$ magick example66.png -format "%wx%h" info:
6x6
$ magick example66.png txt: | head -40
```

Decoded by **ImageMagick 7.1.2-18 Q16-HDRI**, an implementation with no shared code with
ours: dimensions **6×6**, rows 0–2 exactly `(255,0,0)`, rows 3–5 exactly `(0,0,255)`.
**Pixel-exact** `[MEASURED]`.

Scaled up, the same test on the three 2482×814 proxies gives
`BYTE_IDENTICAL = True` against the quantiser's reconstruction — 6,061,044 bytes each,
zero differing bytes (§4).

ImageMagick is used **only as a test oracle**. No third-party code is reused; the encoder
is written from the DEC and xterm specifications. libsixel (MIT) was not read or vendored.

---

## 9. Damage updates

### 9.1 The hard constraint

Sixel has **no image ids, no placements, no z-index, no deletion primitive, and no partial
update**. Everything Kitty gives us for damage (`a=p`, `i=`, `z=`, lowercase deletes) is
absent. The only positioning mechanism is **the text cursor**: an image is drawn where the
cursor is.

Therefore, for Sixel, *damage* means exactly one thing: **move the cursor, redraw a whole
rectangle of the page.**

### 9.2 Cell snapping

The cursor addresses **character cells**, so a damage rect must be snapped **outward** to
cell boundaries before it can be drawn:

```
x0 = (rx / cell_w) * cell_w                    y0 = (ry / cell_h) * cell_h
x1 = ceil((rx+rw) / cell_w) * cell_w  clamped   y1 = ceil((ry+rh) / cell_h) * cell_h  clamped
```

then `CSI <row>;<col> H` with `row = y0/cell_h + 1`, `col = x0/cell_w + 1`.

Cell size comes from `Capabilities::cell` (`caps.rs:186-191`), which already falls back to
`winsize.cell_size()` when `CSI 16 t` goes unanswered. **A Sixel backend must refuse to
start if cell size is unknown**, because a wrong cell size does not degrade the image, it
puts every damage tile in the wrong place.

The rect need *not* be snapped to 6 vertically: bands are relative to the image origin, and
the partial final band is handled by exact `Pv` plus P2=1 (§3.1).

### 9.3 Measured damage cost

`proxy_text`, page 2482×814, Ghostty's measured 17×37 cell `[MEASURED, PROXY]`:

| damage | snapped | wire bytes | encode ms | regs | % of full frame |
|---|---|---|---|---|---|
| full frame | 2482×814 | 669,407 | 20.03 | 53 | 100% |
| caret blink 2×20 | 17×37 | **1,363** | **0.030** | 33 | **0.20%** |
| one text line | 918×37 | 33,682 | 0.578 | 34 | 5.03% |
| dropdown 420×320 | 442×370 | 80,305 | 2.013 | 45 | 12.00% |
| paragraph 900×200 | 918×222 | 175,508 | 3.063 | 45 | 26.22% |
| status strip | 2482×74 | 60,746 | 2.016 | 34 | 9.07% |
| half-page scroll | 2482×407 | 303,811 | 10.813 | 52 | 45.39% |

A caret blink is **491× cheaper** than a full frame in bytes and **667×** in CPU. This is
the same conclusion A10 §0.1 reached for the PTY write path, arrived at independently from
the encoder side: **damage-driven updates are load-bearing, not an optimisation.**

Note the engine already ships a dirty rect in the frame header
(`bg-proto/src/lib.rs:24-27`) and the consumer currently ignores it
(`main.rs` — `encode_rgb_frame` over the full page). B07 §5 specifies the fix on the engine
side (union dirty rects across coalesced frames). **The Sixel backend should be built
against B07's corrected dirty rect, not against the current always-full-frame behaviour.**

### 9.4 Tiling

Damage rects from Chromium are arbitrary; redrawing an arbitrary rect per frame means the
encoder's cost is unbounded and unpredictable. The standard fix in this space — A02 §161
credits brow6el with it — is a **fixed cell-aligned tile grid**: redraw only tiles that
intersect the dirty rect. Fixed tiles also make the per-frame cost quantised and
schedulable, and (with §5.1's fixed palette) make each tile's bytes a pure function of its
content, so tiles can be cached and skipped when unchanged.

The cost of tiling is per-tile envelope + raster attributes + palette definitions. Measured
on a **full** redraw, which is the worst case for that overhead `[MEASURED, PROXY]`:

| tile (cells) | tile px | tiles/screen | full redraw, text | overhead | full redraw, ui | overhead | avg tile B (text) | worst 1-tile ms |
|---|---|---|---|---|---|---|---|---|
| 8×4 | 136×148 | 114 | 774,450 | +15.7% | 132,669 | +35.4% | 6,793 | 0.496 |
| **16×8** | **272×296** | **30** | **720,456** | **+7.6%** | 111,569 | +13.8% | 24,015 | 1.054 |
| 32×12 | 544×444 | 10 | 686,230 | +2.5% | 107,580 | +9.8% | 68,623 | 3.070 |
| 64×24 | 1088×888 | 3 | 676,139 | +1.0% | 100,672 | +2.7% | 225,379 | 9.817 |

**Recommend 16×8 cells (30 tiles/screen).** It pays 7.6% on the dominant text case for a
worst-case single-tile encode of ~1 ms, which keeps a typical few-tile damage update inside
a couple of milliseconds. 8×4 more than doubles the overhead on flat UI content for little
latency gain; 32×12 and above make one tile cost as much as three frames of budget. This
lands on the same 30–40 tiles/screen that A02 §161 reports brow6el converged on, which is
mild independent corroboration.

### 9.5 Cursor placement hazard — DECSDM, and the probe that sidesteps it

Where a Sixel image lands depends on **DECSDM (private mode 80)**, and its sense was
**inverted in xterm patch #369** to match real VT340 hardware. xterm's own text now reads:
*"Disable Sixel Display Mode (DECSDM) … Turns on 'Sixel Scrolling'"* `[SPEC: ctlseqs]`.
foot and mintty both had to flip to follow. So:

- DECSDM **reset** → sixel scrolling on → image drawn **at the cursor** ← what we need
- DECSDM **set** → image drawn at a fixed origin, cursor unmoved ← damage tiles all land at 0,0

**The same escape sequence means opposite things across terminal versions.** Emitting
`CSI ? 80 l` blind is therefore not safe.

Mitigation, in the spirit of `caps.rs:1-8` ("ask the terminal, don't pattern-match
`$TERM`") — **a runtime placement probe**, which is CI-able and needs no screenshot:

1. Park the cursor at a known row *R* well away from the top, on the alternate screen.
2. Emit a minimal image: 1 cell wide, *N* bands tall, one register.
3. Query cursor position (`CSI 6 n`) and parse the `CSI r ; c R` reply.
4. **Row advanced by the image height in cells → scrolling mode → the image landed at the
   cursor → damage tiles are usable.** Row unchanged → display mode → images land at a
   fixed origin → **disable tiling and draw full frames only**, or refuse the backend.
5. Record the raw reply in `Capabilities::raw_replies` so `doctor` can show it, exactly as
   the existing probes do (`caps.rs:150,157,163,170`).

Two further placement rules regardless of mode:

- **Never let an image's bottom reach the last row.** Sixel scrolling will scroll the
  screen, which moves every other tile and destroys the status bar. Keep all page content
  within `rows - 1`, as `present()` already assumes for its status line.
- Bracket every image with `ESC 7` / `ESC 8` (DECSC/DECRC) so cursor state is restored
  whatever the terminal did with it. Private mode 8452 ("sixel scrolling leaves cursor to
  the right") also perturbs this; A04 §3.2 measured iTerm2 reporting it *permanently reset*,
  and I could not locate it in the `ctlseqs` text this session `[UNVERIFIED]`. Save/restore
  makes its value irrelevant.

---

## 10. Performance budget

All at 2482×814, the real measured Ghostty viewport, `[MEASURED, PROXY]`, 25 reps, p50, on
an otherwise idle machine. Under concurrent load the same encode was observed at up to
19.1 ms, so treat these as a floor.

| | proxy_text | proxy_ui | proxy_photo |
|---|---|---|---|
| quantise | 3.76 ms | 4.61 ms | 3.97 ms |
| encode | 12.62 ms | 10.08 ms | 23.92 ms |
| **CPU total** | **16.37 ms** | **14.69 ms** | **27.90 ms** |
| wire bytes | 669,407 | 98,007 | 477,357 |
| est. PTY write¹ | ~1.65 ms | ~0.24 ms | ~1.18 ms |
| bytes/s at 60 fps | **40.2 MB/s** | 5.9 MB/s | 28.6 MB/s |

¹ interpolated from A10 §0.1's PTY table at 1 KiB chunks (400,000 B → 0.987 ms p50, i.e.
~405 MB/s). Not independently measured for these payload sizes.

**Three conclusions.**

1. **Full-frame Sixel cannot run at 60 fps.** 16.37 ms of CPU is the entire frame budget
   before Chromium, IPC, or the write. Confirmed from the other direction by bandwidth:
   40.2 MB/s sustained through a PTY that A10 measured at 283–866 MB/s leaves nothing for
   anything else.
2. **For Sixel the bottleneck inverts relative to Kitty.** Kitty's problem is the write
   (A10 §0.1: 10.8 ms p50 for one full-res base64 frame). Sixel's output is ~10× smaller
   than base64 RGBA, so its write is ~1.65 ms — but its *encode* is 12.62 ms. Optimisation
   effort on this backend belongs in the band encoder, not the write path. Do not
   generalise A10's headline to Sixel without re-measuring.
3. **Against Kitty on the identical frame:** 669,407 B vs 53,999 B `[REPO: B01:46]` =
   **12.4× more wire**, at comparable CPU (12.62 ms vs B05's re-measured ≥11.53 ms). Sixel
   is strictly worse where Kitty is available — which is exactly why §2.1 scopes it to
   terminals where Kitty is not.

**Target with damage + tiling:** a typical interaction touches 1–3 tiles. At 24,015 B and
~1.05 ms per 16×8 tile, that is ~72 KB and ~3 ms — comfortably inside budget, and the
reason §14 makes tiling non-optional rather than a later optimisation.

---

## 11. Instructions for the commander (I did not make these changes)

**Do this first, and independently of Sixel — it is a correctness bug today.**

> **11.1** `apps/cli/src/main.rs:858-859` — `Renderer::present` matches `Backend::Kitty`
> and sends everything else to the Unicode path via `_ =>`. So `blackglass doctor` can
> report `sixel` (`caps.rs:56-63`) and `BLACKGLASS_BACKEND=sixel` is accepted
> (`main.rs:211`), while `open` silently renders half-blocks. B01 §250 flags the same
> thing. Make the match **exhaustive over `Backend`** so adding a variant is a compile
> error rather than a silent downgrade, and until a Sixel encoder exists, have the Sixel
> arm log "falling back to unicode" once.

Then, for the backend itself:

> **11.2** New file `crates/bg-term/src/sixel.rs`, registered in `crates/bg-term/src/lib.rs`
> alongside `pub mod kitty;` (`lib.rs:12-17`). No new dependencies — Sixel has no
> compression, so `flate2` is not needed.
>
> **11.3** `crates/bg-term/src/caps.rs` — add XTSMGRAPHICS negotiation (§5.4) and the
> placement probe (§9.5), pushing raw replies into `raw_replies` as the existing probes do.
> Add `sixel_registers: Option<u32>`, `sixel_max_geometry: Option<(u16,u16)>`, and
> `sixel_draws_at_cursor: bool` to `Capabilities`. **`best_backend()` must not return
> `Backend::Sixel` unless cell size is known** (§9.2).
>
> **11.4** `apps/cli/src/main.rs` — consume `FrameHeader::dirty_*`
> (`bg-proto/src/lib.rs:24-27`), which the renderer currently ignores. Build against B07's
> corrected coalescing (B07 §5/§10.1) so the dirty rect is the union across dropped frames;
> without that fix, tiles will miss updates.

Proposed API, mirroring `kitty.rs` conventions (`Placement`, `EncodeStats`, `&mut Vec<u8>`
output, no allocation in the hot path):

```rust
/// A palette entry: what we transmit (percent) and what will be displayed (8-bit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reg { pub pct: [u8; 3], pub rgb: [u8; 3] }

#[inline] pub fn pct_to_u8(p: u8) -> u8;              // (p*255 + 50) / 100, round-half-up

pub const PALETTE_LEN: usize = 256;
pub fn build_palette() -> Vec<Reg>;                    // 40 greys + 6x6x6 cube
pub fn build_palette16() -> Vec<Reg>;                  // reduced-register fallback
pub fn build_lut(pal: &[Reg]) -> Vec<u8>;              // 32_768 entries; call once

#[derive(Debug, Clone, Copy)]
pub struct DitherOpts { pub amplitude: i32, pub neutral_guard: i32 }
impl Default for DitherOpts { /* amplitude: 0 — see C02 §6 */ }

/// `x0,y0` are the buffer's top-left in PAGE space so a tile dithers identically
/// to the full frame and cannot seam.
pub fn quantize(rgb: &[u8], w: u32, h: u32, x0: u32, y0: u32,
                lut: &[u8], d: DitherOpts, out: &mut Vec<u8>);

#[derive(Debug, Clone, Copy)]
pub struct SixelOpts { pub define_palette: bool, pub max_run: usize }  // default: true, 255

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodeStats {
    pub wire_bytes: usize, pub bands: usize,
    pub colour_passes: usize, pub palette_bytes: usize, pub rle_tokens: usize,
}

pub fn encode_indexed(idx: &[u8], w: u32, h: u32, pal: &[Reg],
                      opts: &SixelOpts, out: &mut Vec<u8>) -> EncodeStats;

/// Cursor positioning + DECSC/DECRC bracketing for one tile (C02 §9.2, §9.5).
pub fn place_tile(cell: (u16, u16), rect: Rect, out: &mut Vec<u8>);

/// Snap a damage rect outward to cell boundaries, clamped to the page.
pub fn snap_to_cells(rect: Rect, cell: (u16, u16), page_w: u32, page_h: u32) -> Rect;
```

`Rect` and its `union`/`clamp_to` already exist in `lib.rs:20-66` and are exactly what
tiling needs — no new geometry type.

---

## 12. Test plan

Mirroring the existing crate's style — every test asserts a *measured or specified* fact,
and the comment says which. No test may be weakened to pass.

**Wire format**
1. `encode_indexed` output starts `\x1bP0;1;0q` and ends `\x1b\\`.
2. Raster attributes `"1;1;<w>;<h>` are present and match the arguments exactly.
3. `pct_to_u8` reproduces the measured probe table: `0→0, 1→3, 30→77, 50→128, 70→179, 100→255`
   (§4 — guards against banker's rounding).
4. Every palette entry round-trips: `pct_to_u8(reg.pct[i]) == reg.rgb[i]` for all 256 × 3.
5. Every emitted data byte is in `0x3F..=0x7E`, or is one of `! $ - # "`, or a digit/`;`.
6. **The §8 worked example is asserted byte-for-byte** against the 59-byte literal.

**Encoding**
7. Bit order (LSB = top) — **all three verified against the prototype and the decoder**
   `[MEASURED]`. A 1×6 image with only the **top** pixel black emits `#0@$#39}`, and the
   decoder puts black at `y=0`; only the **bottom** pixel black emits `#0_$#39^`, black at
   `y=5`; all six black emits `#0~` with a single colour pass and no `$`.
   (`@`=0x40 value 1, `_`=0x5F value 32, `~`=0x7E value 63.) Inverting the bit order still
   produces a *valid* image, just vertically mirrored, so only this test catches it.
8. RLE threshold: a run of 3 emits 3 literals; a run of 4 emits `!4`.
9. `max_run` splits: a run of 300 with `max_run: 255` emits `!255` then `!45`.
10. Two colours in one band are separated by `$` and **not** by `-`.
11. Bands are separated by `-`, and there is no trailing `-` after the last band.
12. Height not a multiple of 6 (e.g. 6×10) emits `Pv=10` and zero-pads the final band.
13. Trailing all-zero columns are trimmed from each colour pass.

**Palette / quantiser**
14. Cube index arithmetic: pure red → register 220, pure blue → 45 (§8).
15. LUT reconstructs bucket centres — a mid-grey input does not skew dark.
16. `DitherOpts::default().amplitude == 0` (§6 — this is a decision, and it should break
    loudly if someone flips it).
17. Dither phase is page-relative: quantising a sub-rect at `(x0,y0)` yields identical
    indices to the same region of the full-frame quantisation. (Guards the seam bug.)

**Damage / placement**
18. `snap_to_cells` expands outward and clamps: `(300,300,2,20)` with cell 17×37 →
    `(289,296,17,37)` (§9.3).
19. `place_tile` emits DECSC … `CSI r;c H` … DECRC with 1-based coordinates.
20. A tile never extends into the last terminal row (§9.5).

**Cross-implementation (integration, gated on ImageMagick being present)**
21. Encode each proxy, decode with `magick sixel:… `, assert the decode equals the
    quantiser's reconstruction **byte for byte** (§8.3). This is the test that would have
    caught the percent-grid bug, and it is cheap.

---

## 13. Risks and unverified items

| # | Item | Status | Mitigation |
|---|---|---|---|
| 1 | **No Sixel-capable terminal is confirmed on this machine.** Everything is verified against ImageMagick's decoder, not a terminal. | Blocking for end-to-end sign-off | Resolve the iTerm2 conflict (§2.2). Otherwise CI in a container with `xterm -ti vt340` or `foot`. |
| 2 | DECSDM sense differs across xterm <369 / ≥369 and followers | `[SPEC]`, direction confirmed; per-terminal behaviour `[UNVERIFIED]` | Runtime placement probe, §9.5. Never emit `CSI ? 80 h/l` blind. |
| 3 | Private per-graphic colour registers (mode number commonly cited as 1070) | `[UNVERIFIED]` — not located in `ctlseqs` this session | Design is correct either way: redefine used registers every image, measured at 0.11% (§7.4). |
| 4 | Mode 8452 (cursor right of image) | `[UNVERIFIED]` here; A04 measured iTerm2 reporting it permanently reset | DECSC/DECRC bracketing makes it irrelevant. |
| 5 | All performance numbers are `[PROXY]`, not real Chromium frames | Known | Re-run `bench`/`damage`/`tiles` against a captured BGRA frame before any gate is set from them. |
| 6 | PTY write times for Sixel payload sizes are **interpolated** from A10, not measured | Known | Extend `benchmarks/probes/pty3.c` with 100 KB and 670 KB payloads. |
| 7 | tmux Sixel needs a build flag and is capped per pane | `[REPO: A07:553, A04:560]` | Require ≥3.6b; detect and degrade. Out of scope here. |
| 8 | Encode timing was 16.4 ms idle but 19–24 ms under load | `[MEASURED]` | Budget from the loaded figure, not the idle one. |

**Licensing** (per the standing rule): the encoder is written from the DEC VT330/VT340
reference and xterm `ctlseqs`, both open specifications. **No third-party code is reused.**
libsixel (MIT) was deliberately not read or vendored. ImageMagick is used only as an
external test oracle invoked as a binary — no linking, no code copied.

---

## 14. Recommendation

**Make tiling part of the initial Sixel implementation, not a follow-up — and land the
`Renderer::present` exhaustive-match fix (§11.1) this week, before any Sixel code exists.**

The measurements force it. A full-frame Sixel redraw costs 16.4 ms of CPU and 669 KB, which
is 40.2 MB/s at 60 fps — the backend is dead on arrival in that form, so a "monolithic
first, tile later" plan never produces a shippable intermediate and every hour spent on it
is spent on something that will be rewritten. With 16×8-cell tiles the same interaction
costs ~24 KB and ~1 ms per touched tile, a 490× byte reduction on a caret blink, for a 7.6%
overhead on the rare full redraw. Tiling is what makes the backend exist.

The `present()` fix is separable and more urgent than Sixel itself: today `doctor` can
truthfully report `sixel`, the user can select it, and the renderer silently draws
half-blocks instead. That is a diagnostic tool telling the user something false, it costs
one match arm to fix, and it converts every future backend addition from a silent downgrade
into a compile error.

Two supporting decisions that are cheap now and expensive later: **define the palette on
the percent grid** (§4), which makes byte-exact decoder tests possible and is the only
reason test 21 can exist; and **default dithering to off** (§6), which is counterintuitive
enough that it needs the measured table next to it in the code or someone will "fix" it
back and triple the bandwidth.

---

## 15. Sources

**Primary specification**
- DEC VT330/VT340 Programmer Reference Vol. 2, ch. 14 — <https://vt100.net/docs/vt3xx-gp/chapter14.html> — DCS envelope, P1/P2/P3, the −0x3F encoding and LSB-is-top, `! $ - # "`, raster attributes, HLS/RGB percent ranges.
- xterm control sequences — <https://invisible-island.net/xterm/ctlseqs/ctlseqs.html> — DECSDM (private mode 80) set/reset wording, XTSMGRAPHICS `CSI ? Pi ; Pa ; Pv S`. Permissive licence (Thomas Dickey); safe reference.
- xterm change log — <https://dickey.his.com/xterm/xterm.log.html> — patch #369 inverting DECSDM.
- foot issue #361 / PR #632 — <https://codeberg.org/dnkl/foot/issues/361>, <https://codeberg.org/dnkl/foot/pulls/632> — independent confirmation that implementations had to flip DECSDM.
- libsixel — <https://saitoha.github.io/libsixel/> — MIT; noted for licence provenance only, **not read or reused**.

**Tools used as oracles**
- ImageMagick 7.1.2-18 Q16-HDRI, `SIXEL rw-` coder — independent decoder for §4, §8.3.

**Sibling artefacts relied on**
- `A04-terminal-capability-matrix.md` §3 — Sixel wire format survey, DA1 detection, iTerm2/Ghostty/Apple Terminal measurements.
- `A10-performance-plan.md` §0.1 — PTY write throughput table.
- `A02-competitor-matrix.md` §161, §248-250 — brow6el's tiled-Sixel dirty-rect scheme.
- `B01-architecture-rfc.md` §46, §250 — Kitty wire-byte baseline; the `present()` backend gap.
- `B05-shared-texture-analysis.md` §209-211 — re-measured Kitty encode cost.
- `B07-frame-scheduler.md` §5, §10.1 — dirty-rect union across coalesced frames.
