# C04 — Unicode fallback quality: measured analysis and proposed algorithms

**Scope:** review of `crates/bg-term/src/unicode.rs` (164 lines, `render_half_blocks` at
`unicode.rs:20-50`). Proposals only. No core source was modified.

**Verdict up front.** The mission's premise was that higher sub-cell resolution (quadrants,
sextants, octants, braille) is the path to quality. Measurement does not support that. On the
measured 17x37 cell, going from 2 sub-cells to 8 is worth **+0.45 dB PSNR**, while fixing the
*sampling rule* alone is worth **+2.66 dB**, and fixing the *colour-selection rule* is worth the
difference between text that is legible and text that is a uniform grey smear (**11.6% vs 100%**
of the original stroke contrast). Separately, a scan of all 759 font files on this machine found
**zero real glyphs for sextants or octants** — they render as tofu. The recommendation is
therefore: fix sampling, add contrast-aware selection, move half-block to quadrant, and gate
octants behind a runtime probe rather than adopting them.

---

## 1. Method

Everything below is measured, not modelled, unless explicitly marked. The harness is pure
Python 3.14.2 / numpy 2.4.2 (both already present; nothing was installed, no large downloads,
consistent with the 98%-disk constraint). Rendering is simulated exactly rather than screenshotted,
because the machine is at a lock screen — this is also the CI-able form.

**Geometry** is the real measured Ghostty 1.3.1 geometry from `caps.rs:17`: cell 17x37 px,
viewport 2482x814 px. That divides exactly: `2482/17 = 146` columns, `814/37 = 22` rows, so
**146 x 22 = 3212 cells of exactly 629 px each**, 2,020,348 px per frame. All numbers below use
that grid.

**Test page** is a synthetic 2482x814 sRGB page built to stress the cases a browser actually
produces: a 32px bold header bar, nine lines of 16px body copy (Arial), a five-row table drawn
with 1px rules, filled and outlined buttons, a two-axis gradient panel, a band-limited-noise
"photo", a 1px checkerboard, and eight stripe fields at periods 2, 3, 4, 6, 8, 12, 16 and 24 px.
A second, unconfounded chart of 23 isolated 1px hairlines on white (four grey levels, no text)
was used wherever text would contaminate the measurement.

**Reconstruction** models what the terminal displays: each sub-cell region is filled with its
assigned colour and compared against the original at source resolution. Sub-cell boundaries are
handled by exact geometric coverage (see §4), because 17 and 37 are both prime and no sub-grid
divides either evenly. The forward/inverse pair was validated: at a 37x17 sub-grid (one sub-cell
per source pixel) the round-trip error is exactly `0.0`.

**Metrics** are PSNR on sRGB, SSIM on luma, mean CIELAB ΔE, and two task-shaped measures defined
in §6: *hairline survival* (does a 1px rule remain above the 2.3 ΔE just-noticeable difference)
and *stroke contrast retention* (does 16px text keep its L\* standard deviation).

A caution that shapes the whole report: **PSNR is a biased referee here.** MSE's optimal constant
for any region is the arithmetic mean in whatever space MSE is measured, so a renderer that
averages will always beat a renderer that preserves extremes on PSNR, regardless of which is more
readable. §6 shows the two objectives are actively anti-correlated on this workload.

---

## 2. What the current code does

`render_half_blocks` point-samples. At `unicode.rs:22-27`:

```rust
let sx = (x * w / cols).min(w - 1);
let sy = (y * h / (rows * 2)).min(h - 1);
```

With `w=2482, cols=146` this is `sx = 17x` exactly, and with `h=814, rows*2=44` it is
`sy = 18.5y` truncated. Three consequences follow, all measurable:

**16 of every 17 source columns are discarded**, and roughly 17 of every 18 rows. The sample
pitch is 17 px horizontally, so the Nyquist limit is 1/34 cycles/px: only features with a period
of 34 px or more survive. Body text at 16px has a stem pitch around 6–9 px, i.e. 0.11–0.17
cycles/px, four to six times above Nyquist. There is no filter, so those components fold rather
than attenuate.

**The image is displaced up-and-left by half a sub-cell.** Truncation selects the *first* pixel of
each group, not its centre, giving a fixed phase offset of 8.5 px horizontally and 9.25 px
vertically.

**Expected error is exactly doubled.** For a sub-cell of K pixels with mean μ and summed-channel
variance σ², the area average yields SSE = Σ‖pᵢ − μ‖² = Kσ², while a point sample p_s yields
Kσ² + K‖p_s − μ‖², whose expectation over a uniformly chosen s is **2Kσ²**. That is a
**3.01 dB** theoretical penalty. Measured, isolating the sampling rule with no colour collapse:

| sub-grid 1x2, no colour collapse | PSNR | mean ΔE |
|---|---|---|
| point-sample (shipped) | 10.01 dB | 15.94 |
| exact-area box | 12.67 dB | 12.90 |

**+2.66 dB measured against +3.01 dB predicted.** The shortfall is expected: the shipped sampler
uses a biased (first-pixel) phase rather than a random one.

Two smaller observations, offered as notes rather than defects. `format!` inside the inner loop
(`unicode.rs:36`) heap-allocates a `String` on every colour change; writing into the output buffer
with `write!` or a hand-rolled integer formatter removes a per-cell allocation from the hot path.
And row advance uses a bare `\r\n`, which is correct for a full-width render but will misplace
output if the image is ever drawn into a sub-rectangle.

---

## 3. Effective resolution of each glyph repertoire on a 17x37 cell

Each cell carries two truecolor slots (48 bits) plus a glyph index. The sub-grid determines how
many spatially independent regions those two colours can be distributed across.

| repertoire | sub-grid (w x h) | sub-cells | sample pitch (px) | sub-cell aspect | anisotropy | samples/frame | vs 2.02 Mpx source |
|---|---|---|---|---|---|---|---|
| half block (current) | 1 x 2 | 2 | 17.0 x 18.50 | 1 : 1.088 | 1.088 | 146 x 44 = 6,424 | 1 : 314 |
| quadrant | 2 x 2 | 4 | 8.5 x 18.50 | 1 : 2.176 | 2.176 | 292 x 44 = 12,848 | 1 : 157 |
| sextant | 2 x 3 | 6 | 8.5 x 12.33 | 1 : 1.451 | 1.451 | 292 x 66 = 19,272 | 1 : 105 |
| octant | 2 x 4 | 8 | 8.5 x 9.25 | 1 : 1.088 | 1.088 | 292 x 88 = 25,696 | 1 : 79 |
| braille | 2 x 4 | 8 | 8.5 x 9.25 | 1 : 1.088 | 1.088 | 292 x 88 = 25,696 | 1 : 79 (1-bit, see §8) |

Anisotropy is `max(a, 1/a)` for `a = (W·cy)/(H·cx)`. Two facts fall out of the 17:37 cell shape
that are specific to this terminal and worth stating plainly. First, **quadrants are the
aspect-worst option**: at 1:2.18 their sub-cells are as elongated as the cell itself, twice as
anisotropic as half-blocks. Second, **octants are aspect-optimal**, tying half-blocks at 1.088
while carrying four times the samples. Sextants sit in between at 1.451.

Refinement matters too. 1x2 ⊂ 2x2 ⊂ 2x4, so quadrants and octants strictly refine what
half-blocks can express. The 2x3 sextant lattice does *not* refine 2x2 (2 does not divide 3); it
is a different lattice, not a superset.

**Repertoire completeness, verified against UCD 16.0.0** (`unicodedata.unidata_version` on the
local Python). All three block repertoires are complete — every possible sub-cell bitmask has a
code point, so glyph selection is unconstrained 2-colour partitioning with no "nearest available
shape" fallback:

| repertoire | patterns | in the dedicated block | supplied by pre-existing chars |
|---|---|---|---|
| quadrant (2x2) | 16 | 10 (U+2596–U+259F) | 6 (space, U+2580, U+2584, U+258C, U+2590, U+2588) |
| sextant (2x3) | 64 | 60 (U+1FB00–U+1FB3B) | 4 (space, U+258C, U+2590, U+2588) |
| octant (2x4) | 256 | 230 (U+1CD00–U+1CDE5) | 26 |
| braille (2x4) | 256 | 256 (U+2800–U+28FF) | 0 |

The 26 octant gaps were enumerated and resolved individually; the complete tables are in
Appendix A. The awkward ones are worth naming because they are easy to get wrong: the four
single-octant patterns come from U+1CEA8, U+1CEAB, U+1CEA3 and U+1CEA0 (Symbols for Legacy
Computing Supplement), the two mid-column pairs from U+1FBE6 and U+1FBE7, and the quarter/
three-quarter row bands from U+1FB82, U+1FB85, U+2582 and U+2586.

---

## 4. Proper area averaging on a cell that divides evenly into nothing

17 and 37 are both prime. No sub-grid divides either. A 2x4 octant sub-cell is 8.5 x 9.25 px, so
source columns 8, and source rows 9, 18 and 27, each straddle a boundary. Integer-block averaging
is therefore wrong, and gets more wrong as the grid gets finer.

The correct construction is an exact-area (fractional-endpoint) box resample. It is separable, so
it is two 1-D sparse matrices that depend only on the frame and grid sizes and are rebuilt only on
resize:

```
overlap(src_n, dst_n) -> O, where O[j][i] = |[i, i+1) ∩ [j·s, (j+1)·s)|,  s = src_n / dst_n

Properties (assert these):
  column sums == 1   (partition of unity: every source pixel is fully accounted for)
  row sums    == s   (every destination bin covers exactly s source pixels)
  at most 2 non-zero entries per column, ceil(s)+1 per row
```

Forward averaging uses `W = O / s`; reconstruction uses `Oᵀ` directly. Because the same matrix
serves both directions, a single test pins the whole machinery: at a 37x17 sub-grid the
round-trip must be bit-exact (measured: max error `0.0`).

This also handles the general case where the frame is not an exact multiple of the cell grid,
which is the normal situation on any terminal that is not Ghostty at this exact size.

**Averaging space.** Averaging must happen in linear light, not on sRGB code values, because a
sub-cell average stands in for light the display will emit. The physical check:

```
1px black/white stripe field, 50% coverage
  linear-light mean -> sRGB 187.5   (preserves emitted light — correct)
  sRGB-space   mean -> sRGB 127.5   (preserves code value)
```

Honesty requires flagging that the *metrics disagree with the physics*, and why. On the full test
page, sRGB-space averaging scores **higher** PSNR (13.36 vs 12.67 dB) and nearly identical ΔE
(12.76 vs 12.90). That is circular, not evidence: MSE measured in sRGB is minimised by averaging
in sRGB, by definition. The measured difference on real page content is under 0.2 ΔE, well below
JND, so this is a correctness-and-cheapness call rather than a quality emergency — but linear is
the defensible default. Cost is one 256-entry decode LUT and one ~4096-entry encode LUT; the
naive `powf` path measured 27.56 ms/frame in numpy and must not be shipped.

---

## 5. The resolution ladder is nearly flat, and the chromatic constraint is not the bottleneck

Splitting total error into a spatial term (how finely the cell is subdivided) and a chromatic term
(collapsing N sub-cell colours down to 2) is the decisive experiment. The "oracle" column lets
every sub-cell keep its own true colour, so it is the floor that sub-grid can ever reach.

| sub-grid | samples/cell | oracle PSNR | actual 2-colour PSNR | chromatic cost |
|---|---|---|---|---|
| 1x1 | 1 | 12.33 | 12.33 | 0.00 dB |
| 1x2 half | 2 | 12.67 | 12.67 | 0.00 dB |
| 2x2 quadrant | 4 | 12.97 | 12.87 | 0.09 dB |
| 2x3 sextant | 6 | 13.24 | 13.07 | 0.17 dB |
| 2x4 octant | 8 | 13.34 | 13.12 | 0.22 dB |
| 4x8 (no repertoire exists) | 32 | 14.11 | — | — |

Two conclusions, both counter to the mission's premise.

**The two-colours-per-cell constraint costs almost nothing** — 0.22 dB at octant resolution. The
elaborate 2-means clustering that this problem seems to call for is not worth building. Measured
directly:

| sub-grid | split rule | PSNR | SSIM |
|---|---|---|---|
| 2x2 quadrant | exhaustive optimal (16 masks) | 12.874 | 0.6552 |
| 2x2 quadrant | luma-midpoint threshold | 12.852 | **0.6557** |
| 2x4 octant | exhaustive optimal (256 masks) | 13.121 | 0.6693 |
| 2x4 octant | luma-midpoint threshold | 13.091 | **0.6701** |

A plain luma threshold is within 0.03 dB of the provably optimal partition and slightly *better*
on SSIM, at a fraction of the cost (the 256-mask exhaustive search measured 52.6 ms/frame in
numpy versus 3.0 ms for the 16-mask one — a 17x cost increase for 0.25 dB).

**The sub-grid itself buys little.** Even the oracle only gains 1.01 dB going from 1 to 8 samples
per cell, and 1.78 dB going to 32. At a 17:1 downscale the content is so far above Nyquist that
the within-sub-cell variance barely falls when you subdivide. Per region, from the full page:

| renderer | overall | text 16px | table+rules | gradient | photo | hi-freq |
|---|---|---|---|---|---|---|
| A point half (shipped) | 10.01 | 10.52 | 18.93 | 14.94 | 10.75 | 3.22 |
| C area half, linear | 12.67 | 13.81 | 23.89 | 19.05 | 15.01 | 5.47 |
| D area quadrant | 12.87 | 13.84 | 23.93 | 19.33 | 15.02 | 5.72 |
| E area sextant | 13.07 | 14.12 | 24.13 | 23.01 | 15.03 | 5.81 |
| F area octant | 13.12 | 14.26 | 24.16 | 21.14 | 15.31 | 5.83 |

Gradients are the one case where finer sub-grids pay properly (+2.1 dB half to octant). Text and
tables gain under half a dB.

**Aliasing**, measured in linear luminance on the stripe fields, is where area averaging earns its
keep. `falseSD` is the spatial standard deviation of a field whose true content is uniform, so it
is pure fabricated structure:

| stripe period | true Y | point Y | point falseSD | area Y | area falseSD |
|---|---|---|---|---|---|
| 2 px | 0.500 | 0.545 | 0.498 | 0.571 | 0.110 |
| 3 px | 0.500 | 0.493 | 0.500 | 0.532 | 0.059 |
| 4 px | 0.505 | 0.734 | 0.437 | 0.533 | 0.066 |
| 8 px | 0.495 | 0.603 | 0.486 | 0.554 | 0.067 |
| 16 px | 0.495 | 0.475 | 0.494 | 0.554 | 0.228 |

The point sampler fabricates **±0.50 full-swing false structure** at essentially every period —
it is emitting black-and-white noise where the truth is flat grey. Area averaging cuts that by
5–8x on fine periods. The theory matches: a 17-px box filter attenuates the 0.125 cyc/px component
(8px stems, the worst aliasing offender for web text) by `|sin(πfK)/(K·sin(πf))| = 0.0588`, i.e.
**−24.6 dB**, before it can fold. Point sampling attenuates it by 0 dB.

---

## 6. PSNR is anti-correlated with legibility, and this is the finding that matters

Both of the above improvements make the numbers better and the page *less readable*. Two
task-shaped measurements on the unconfounded 23-hairline chart and the 16px body block:

| renderer | hairlines seen | position err | PSNR | text stroke contrast retained |
|---|---|---|---|---|
| ORIGINAL | 23/23 | 0.0 px | — | 100% (L\* sd 22.68) |
| point half (SHIPPED) | 1/23 | 8.0 px | 24.24 | 105.1% (fake — see below) |
| area half | **0/23** | — | 24.70 | **11.6%** |
| area quadrant | **0/23** | — | 24.78 | — |
| area octant, MSE-optimal | 6/23 | 4.0 px | **24.89** | 22.5% |
| octant + contrast-preserve τ=0.05 | **22/23** | 3.9 px | 15.85 | — |
| octant + contrast-preserve τ=0.10 | **21/23** | 3.7 px | 16.85 | **100.0%** |
| octant + contrast-preserve τ=0.20 | 7/23 | 3.9 px | 24.07 | — |

Read that first block again: **the best-PSNR renderer (area octant, 24.89 dB) is the one that
loses the most hairlines, and area half-block at 24.70 dB loses every single one.** Area averaging
washes 16px body text down to **11.6%** of its original stroke contrast — a uniform grey smear
that scores well because grey is the L2-optimal answer.

The shipped point sampler's 105.1% stroke contrast is a trap, not a win. It exceeds the original
because point sampling returns either a stroke pixel or a background pixel at full swing; that is
the same ±0.50 aliasing noise from §5, and on a scrolling page it boils frame to frame.

### The fix: contrast-aware glyph selection

The mechanism is a change to the colour rule, not the glyph repertoire. Two paths per cell, chosen
by a cheap discriminator.

For a cell containing thin high-contrast structure — text strokes, table rules, borders, focus
rings — the L2-optimal choice deliberately erases it. A 1px rule inside an 8.5px sub-cell is 12%
coverage, so the sub-cell mean sits 88% toward background and the MSE-optimal assignment puts it
in the background cluster. Instead, set the two colours from the *ink* and *paper* populations
(not from sub-cell means, which are already contaminated), and threshold on **ink coverage** with
a deliberately low τ:

```
per cell (629 px, all statistics from the single accumulation pass of §7):
  med       = median luma over the cell                    (robust background estimate)
  dev_i     = |luma_i - med|;  peak = max dev
  ink_i     = dev_i > max(0.35 * peak, 0.06)               (per-pixel ink mask)
  inkfrac   = mean(ink)
  thin      = peak > 0.06 && 0 < inkfrac < 0.30            (discriminator)

  if thin:                                                  # text / rules / borders
      fg      = mean colour of ink pixels                   # full contrast, not a blend
      bg      = mean colour of non-ink pixels
      cov_k   = exact-area coverage of ink within sub-cell k
      mask_k  = cov_k > τ                                   # τ ≈ 0.10, NOT 0.5
  else:                                                     # continuous tone
      mask_k  = luma_k > (min_k luma + max_k luma) / 2      # §5: within 0.03 dB of optimal
      fg, bg  = means of the two groups
```

τ is the single knob on a clean Pareto frontier, and its meaning is explicit: it is the minimum
ink coverage a sub-cell needs before it is drawn as ink. At τ = 0.5 this degenerates to the
MSE-optimal rule and structure vanishes. τ ≈ 0.10 recovers **21/23** hairlines and **100.0%** of
text stroke contrast. The cost is PSNR, because a 1px rule is deliberately thickened to a whole
8.5px sub-cell.

On the realistic mixed-content page rather than the hairline-only chart, τ = 0.10 **strictly
dominates what ships today on both axes**: 9/11 structural rules visible versus 4/11, and 11.12 dB
versus 10.01 dB. The PSNR-for-legibility trade only shows up on pathological hairline-dense
content, and even there it is 21/23 versus 1/23 rules for 7 dB.

Recommended default τ = 0.10, exposed as a tunable, with `doctor` reporting the value.

---

## 7. Performance budget

The whole design reduces to one pass over pixels producing a handful of per-sub-cell accumulators,
after which every decision is made from at most 8 aggregates per cell.

```
per source pixel (2,020,348 per frame):
  3 x u16 LUT lookup (sRGB -> linear)
  accumulate into the owning sub-cell: sum_r, sum_g, sum_b, sum_y, sum_y2   (5 adds)
  accumulate the ink mask and, if line detection is wanted, one column and one row bin
=> ~8 ops/px = 16.2 Mops/frame = 0.97 Gops/s at 60 fps, SIMD-friendly, parallel by cell-row

memory: 8.08 MB BGRA/frame = 485 MB/s at 60 fps (well under M4 bandwidth)
per-cell state: 3212 cells x 8 sub-cells = 25,696 aggregates/frame (1.54 M/s)
```

Measured stage costs in numpy, as a loose upper bound on a Rust implementation:

| stage | numpy | note |
|---|---|---|
| sRGB → linear via `powf` | 27.56 ms | must be a LUT; do not ship this |
| exact-area sub-cell means, 2x4 | 5.19 ms | separable sparse matmul |
| exhaustive optimal split, 16 masks | 3.00 ms | quadrant |
| exhaustive optimal split, 256 masks | 52.59 ms | octant — and §5 says skip it entirely |

The luma-threshold rule is O(N) per cell with no search, which removes the only stage that was
anywhere near the 16.65 ms frame budget.

---

## 8. Glyph availability is the binding constraint

This is where the octant recommendation dies on contact with the target platform. Every font file
on the machine was scanned via `fontTools` (759 files across `/System/Library/Fonts`,
`/System/Library/Fonts/Supplemental`, `/Library/Fonts`, `~/Library/Fonts`):

| probe | fonts covering it |
|---|---|
| quadrant U+2596 | **100** |
| braille U+28FF | 7 (five are Apple Braille specialty faces; plus Apple Symbols and LastResort) |
| sextant U+1FB00 | **1 — LastResort.otf only** |
| octant U+1CD00 | **1 — LastResort.otf only** |
| legacy quarter-blocks U+1FB82 | **1 — LastResort.otf only** |

LastResort is Apple's tofu font. Confirmed by rasterisation rather than by reputation: U+1CD00,
U+1FB00 and U+2596 all produce byte-identical statistics (87x87 bbox, 0.365 total ink, 0.221
interior ink — a hollow box). **Sextants and octants are unrenderable on stock macOS.**

Font-drawn quadrants, by contrast, tile correctly. Menlo's U+2596 measures 0.194 ink against
U+2588's 0.803, a ratio of **0.242 versus the ideal 0.250** — about 3% off, attributable to
rasterisation and bbox rounding.

The nuance that keeps octants alive at all: kitty, Ghostty, WezTerm and foot draw block and
box-drawing glyphs *procedurally*, bypassing the font entirely (UNVERIFIED on this machine — the
lock screen blocks visual confirmation; it is documented behaviour for kitty and Ghostty and should
be re-checked). Those terminals all have a graphics protocol, so BlackGlass would normally never
select the Unicode backend there — **except under tmux**, which blocks kitty graphics passthrough
but not text. That is the one real octant opportunity.

There is no reliable in-band probe for glyph availability. Width probing via `CSI 6n` cannot
discriminate, because tofu also advances one cell. The honest mechanism is `XTVERSION`
(`CSI > 0 q`), which is a genuine query/response and so consistent with the `caps.rs:5-8`
philosophy of asking the terminal rather than trusting `$TERM`, combined with a small allowlist,
a `--glyphs=` override, and a one-time `doctor` calibration card that prints candidate glyphs and
asks the user whether they see solid blocks or empty boxes. Default must be quadrants.

### Braille: recommend against

Braille dots do not tile the cell. They are round dots separated by gaps, and only the foreground
colour is painted, so an "on" sub-cell renders as `δ·fg + (1−δ)·bg` where δ is the dot's ink
coverage. Maximum achievable intra-cell contrast is therefore **δ**, against 1.0 for solid block
glyphs. Measured on the only general-purpose font here that carries both braille and a full block:

```
δ = braille_ink(U+28FF) / fullblock_ink(U+2588), Apple Symbols = 0.131
```

With foreground and background clamped to gamut (an earlier unclamped fit hid the effect entirely
by driving fg out of range — worth flagging, since it is an easy way to accidentally "prove"
braille is fine):

| δ | PSNR | SSIM | max intra-cell contrast |
|---|---|---|---|
| 0.131 (measured, Apple Symbols) | 11.71 | 0.639 | 13.1% |
| 0.35 (typical DejaVu-class, UNVERIFIED) | 12.21 | 0.646 | 35.0% |
| 1.00 (hypothetical solid = octant) | 13.09 | 0.670 | 100% |

Braille at its measured coverage is **1.41 dB worse than octants and 0.96 dB worse than plain
area-averaged half-blocks**, on the same 2x4 lattice. It also has one non-specialty font on this
system, its dots come from a fallback face so advance width and baseline will not match the
terminal cell, and screen readers announce braille art as gibberish. It is defensible only as an
opt-in mode for line art and plots, never as the page renderer.

### Box-drawing: recommend against as a separate mechanism

Box-drawing was tested as a hairline rescue: detect a thin cell-spanning rule and substitute
U+2500/U+2502/corners at full contrast. The complete 16-combination set exists (space, the four
stubs U+2574–U+2577, U+2500, U+2502, the four corners, the four tees, U+253C), and Menlo draws
U+2502 as a 3px stroke and U+2500 as a 2px stroke within the 17x37 cell.

It works, but it is dominated by the contrast-preserving rule of §6. Box-drawing snaps the rule to
the cell centre, giving up to ±8.5 px horizontal and ±18.5 px vertical position error, and two
rules closer than 17 px merge into one. The measured position error was 8–18 px versus **3.7 px**
for contrast-preserving octants, which reuse the sub-cell grid already being computed, need no
additional repertoire, and cost no extra detection pass. Box-drawing is worth keeping only for
deliberate chrome that BlackGlass itself draws, not for reconstructing page content.

### Dithering

With two colours per cell, smooth gradients band; §5 shows gradients are also the one region where
finer sub-grids help. Error diffusion is the wrong tool because BlackGlass renders a live scrolling
page and diffused error is history-dependent, so a static region would boil. An ordered Bayer
threshold is a deterministic function of position, so a static region is stable frame to frame and
scrolling shifts a coherent pattern: replace `mask_k = cov_k > τ` with `mask_k = cov_k > B(x,y)`
for a 4x4 Bayer matrix, bounding flat-region coverage error at 1/16 instead of 1/2. Gate it to
continuous-tone cells (`thin == false`) only; never dither text. Not yet measured — proposed.

---

## 9. Recommendations, in priority order

1. **Replace point sampling with exact-area box resampling** (§2, §4). +2.66 dB measured, cuts
   fabricated aliasing structure from ±0.50 to ±0.06, and removes the half-sub-cell displacement.
   No new glyphs, no font risk, no capability probe. This is the single largest available win and
   it is worth more than every repertoire change combined.
2. **Add contrast-aware glyph selection with τ ≈ 0.10** (§6). Restores 100% of text stroke
   contrast and 21/23 hairlines. Without this, recommendation 1 makes the page *less* readable
   while making the metrics better. These two must ship together.
3. **Move the sub-grid from 1x2 to 2x2 quadrant.** +0.20 dB, doubles horizontal sampling, and is
   the only upgrade with universal font support (100 fonts). Use the luma-midpoint threshold, not
   2-means: it is within 0.03 dB of optimal and 17x cheaper.
4. **Do the colour work in linear light** with LUTs both ways (§4). Physically correct; the
   measured quality delta on real content is under 0.2 ΔE, so treat this as correctness rather
   than as a quality lever, and never ship the `powf` path.
5. **Gate sextants and octants behind `XTVERSION` plus a `doctor` calibration card** (§8). They
   are worth +0.45 dB where they render and are unrenderable as tofu on stock macOS. Default off.
6. **Do not adopt braille or box-drawing substitution** (§8), for the measured reasons above.

Acceptance tests, all CI-able with no terminal and no screenshot:

```
area-resample round-trip at 1 sub-cell per pixel is bit-exact                      (verified: 0.0)
overlap matrix column sums == 1 and row sums == src/dst for prime cell dims
uniform stripe field at periods 2..24 renders with spatial SD < 0.15               (point: 0.50)
23-hairline chart: >= 20 of 23 rules exceed 2.3 dE after render                    (area-only: 0)
16px body block retains >= 90% of original L* standard deviation                   (area-only: 11.6%)
OCTANT/SEXTANT tables are complete permutations of all 2^N masks, all distinct
sanitize_for_terminal keeps its existing guarantees (unchanged, already good)
```

The existing `sanitize_for_terminal` (`unicode.rs:64-79`) needs no changes. Its C1 handling and
char-not-byte truncation are correct and the tests around them are the right tests.

---

## 10. Threats to validity

The test page is synthetic. It was built to match real web content (16px Arial body copy, 1px
CSS-style rules, gradients, photographic noise) but it is not a screenshot of a real page, because
the lock screen prevents capturing one. The *relative* ordering of renderers is robust — it held
on both the mixed page and the unconfounded hairline chart — but absolute PSNR values are
content-dependent and should not be quoted as targets.

Reconstruction assumes the terminal draws sub-cell boundaries at exact geometric positions. Real
terminals snap to integer pixels, adding a sub-pixel error not modelled here. This slightly
flatters every area-based renderer equally.

Procedural glyph drawing in kitty, Ghostty, WezTerm and foot is documented behaviour but was not
verified on this machine, for the reason above. It is the single assumption that would most change
recommendation 5, and it is cheap to check the moment a display is available.

The braille δ = 0.131 is measured on Apple Symbols, the only viable general-purpose carrier here.
The 0.35 figure for DejaVu-class terminal fonts is an estimate and is marked UNVERIFIED; no such
font is installed. The measurement recipe is in §8 and takes seconds to re-run against any font.

Dithering (§8) is proposed and unmeasured.

**Licensing.** No third-party code was consulted or copied. `chafa` is the obvious prior art here
and is LGPL-3.0-or-later, which is incompatible with this workspace's `MIT OR Apache-2.0`
(`Cargo.toml:8`); its glyph-selection tables must not be vendored. The tables in Appendix A were
derived independently from the Unicode Character Database via `unicodedata`, which carries the
Unicode license.

---

## Appendix A — verified lookup tables

Generated from UCD 16.0.0 and asserted complete (64/64 and 256/256, all code points distinct).
Bit *k* of the index is sub-cell *k+1* in Unicode's row-major numbering: for sextants
1 2 / 3 4 / 5 6, for octants 1 2 / 3 4 / 5 6 / 7 8.

```rust
pub const SEXTANT: [char; 64] = [
    '\u{0020}', '\u{1FB00}', '\u{1FB01}', '\u{1FB02}', '\u{1FB03}', '\u{1FB04}', '\u{1FB05}', '\u{1FB06}',
    '\u{1FB07}', '\u{1FB08}', '\u{1FB09}', '\u{1FB0A}', '\u{1FB0B}', '\u{1FB0C}', '\u{1FB0D}', '\u{1FB0E}',
    '\u{1FB0F}', '\u{1FB10}', '\u{1FB11}', '\u{1FB12}', '\u{1FB13}', '\u{258C}',  '\u{1FB14}', '\u{1FB15}',
    '\u{1FB16}', '\u{1FB17}', '\u{1FB18}', '\u{1FB19}', '\u{1FB1A}', '\u{1FB1B}', '\u{1FB1C}', '\u{1FB1D}',
    '\u{1FB1E}', '\u{1FB1F}', '\u{1FB20}', '\u{1FB21}', '\u{1FB22}', '\u{1FB23}', '\u{1FB24}', '\u{1FB25}',
    '\u{1FB26}', '\u{1FB27}', '\u{2590}',  '\u{1FB28}', '\u{1FB29}', '\u{1FB2A}', '\u{1FB2B}', '\u{1FB2C}',
    '\u{1FB2D}', '\u{1FB2E}', '\u{1FB2F}', '\u{1FB30}', '\u{1FB31}', '\u{1FB32}', '\u{1FB33}', '\u{1FB34}',
    '\u{1FB35}', '\u{1FB36}', '\u{1FB37}', '\u{1FB38}', '\u{1FB39}', '\u{1FB3A}', '\u{1FB3B}', '\u{2588}',
];
```

The 26 octant patterns not in U+1CD00–U+1CDE5, which are the part an implementer will otherwise
get wrong:

| mask | char | name |
|---|---|---|
| 0x00 | U+0020 | SPACE |
| 0x01 | U+1CEA8 | LEFT HALF UPPER ONE QUARTER BLOCK |
| 0x02 | U+1CEAB | RIGHT HALF UPPER ONE QUARTER BLOCK |
| 0x03 | U+1FB82 | UPPER ONE QUARTER BLOCK |
| 0x05 | U+2598 | QUADRANT UPPER LEFT |
| 0x0A | U+259D | QUADRANT UPPER RIGHT |
| 0x0F | U+2580 | UPPER HALF BLOCK |
| 0x14 | U+1FBE6 | MIDDLE LEFT ONE QUARTER BLOCK |
| 0x28 | U+1FBE7 | MIDDLE RIGHT ONE QUARTER BLOCK |
| 0x3F | U+1FB85 | UPPER THREE QUARTERS BLOCK |
| 0x40 | U+1CEA3 | LEFT HALF LOWER ONE QUARTER BLOCK |
| 0x50 | U+2596 | QUADRANT LOWER LEFT |
| 0x55 | U+258C | LEFT HALF BLOCK |
| 0x5A | U+259E | QUADRANT UPPER RIGHT AND LOWER LEFT |
| 0x5F | U+259B | QUADRANT UPPER LEFT AND UPPER RIGHT AND LOWER LEFT |
| 0x80 | U+1CEA0 | RIGHT HALF LOWER ONE QUARTER BLOCK |
| 0xA0 | U+2597 | QUADRANT LOWER RIGHT |
| 0xA5 | U+259A | QUADRANT UPPER LEFT AND LOWER RIGHT |
| 0xAA | U+2590 | RIGHT HALF BLOCK |
| 0xAF | U+259C | QUADRANT UPPER LEFT AND UPPER RIGHT AND LOWER RIGHT |
| 0xC0 | U+2582 | LOWER ONE QUARTER BLOCK |
| 0xF0 | U+2584 | LOWER HALF BLOCK |
| 0xF5 | U+2599 | QUADRANT UPPER LEFT AND LOWER LEFT AND LOWER RIGHT |
| 0xFA | U+259F | QUADRANT UPPER RIGHT AND LOWER LEFT AND LOWER RIGHT |
| 0xFC | U+2586 | LOWER THREE QUARTERS BLOCK |
| 0xFF | U+2588 | FULL BLOCK |

All other masks map to `U+1CD00 + rank`, where rank counts masks in ascending order excluding the
26 above. The quadrant table (all 16) is in §3 and needs no exceptions.

## Appendix B — reproducing the measurements

```
python3 tables.py     # asserts 64/64 sextants, 256/256 octants against UCD 16.0.0
python3 fontscan.py   # scans every installed font for the five probe code points
python3 raster.py     # confirms LastResort tofu; measures block/braille/box ink coverage
python3 mkpage.py     # builds the 2482x814 test page
python3 exp1.py       # renderer matrix, PSNR/SSIM by region
python3 exp2.py       # spatial-vs-chromatic decomposition; averaging space
python3 exp4.py       # aliasing in linear luma; gamut-clamped braille
python3 exp6.py       # 23-hairline chart; text stroke contrast retention
```

Harness is `harness.py` (overlap matrices, renderers, PSNR/SSIM). It has no dependencies beyond
numpy and PIL, both already present. Written under
`/private/tmp/claude-501/-Users-adeebbashir/a6555dd0-1471-4951-aa0d-5958b606ca83/scratchpad/`,
which is session-scoped; the scripts should be moved into `benchmarks/` by whoever owns that path
if these measurements are to be kept in CI.
