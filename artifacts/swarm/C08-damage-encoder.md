# C08 — Damage-Based Partial Update Path

**Mission:** design the damage/partial-update path — tiling strategy, tile-vs-full-frame cost
model, image/placement id management for multiple tiles, tearing avoidance, and the exact way
the CLI should start using `bgra_rect_to_rgb`.

**Owned output:** this file only. Every change to `crates/`, `apps/cli/`, `apps/engine/` below is
written as an instruction for the commander, not applied. Baseline left untouched.

**Date:** 2026-07-31 · macOS 26.1 · Apple M4 · Electron 43.2.0 / Chromium 150

---

## Implementation status (updated 2026-08-01)

C08 splits cleanly into two halves. **Both are now implemented in code;** the compositor still
needs interactive Ghostty confirmation before its wire-byte wins are claimed as measured.

**Done — damage is consumed on the capture/compositing side.** Following the B02 spike
(which confirmed Chromium reports device-pixel partial dirty rects), the engine crops
`onPaint` to the dirty rect and sends only those pixels; the core composites each rect into a
persistent RGB framebuffer (`kitty::blit_bgra_into_rgb`, `Renderer` in `apps/cli/src/main.rs`).
Backpressure coalescing unions dirty rects across dropped paints (B07 / `frame-pipeline.js`)
so partial updates stay correct under a slow PTY. Verified by unit tests and by the e2e
input-injection test — 13/13 checks pass.

**Implemented — hybrid dense base + sparse tile overlays + DEC 2026 (2026-08-01).** `Renderer`
OR-accumulates damage into a cell-aligned tile bitset. Sparse damage re-transmits only dirty
position-bound image ids (`kitty::tile_image_id`) at z=1. At 60% dirty it clears live overlays
and replaces one monolithic base instead of churning hundreds of terminal image objects. A real
1×1 runtime probe enables Kitty `t=s` shared memory for that dense local base; SSH, multiplexers,
a failed probe, or four unconsumed objects use direct/zlib fallback. `RESTORE_SEQ` clears DEC 2026
first so a crash inside a synchronized block cannot freeze the terminal. Tests cover the mode
switch, stale-overlay deletion, raw 0600 POSIX objects, bounded fallback, and exact direct-wire
pixel round trips.

This hybrid supersedes the original “mosaic unconditionally” conclusion in §5.2. That model
measured compressed bytes and encode CPU, but not the terminal-side cost of replacing 222 image
objects, and it predates the much cheaper shared-memory medium. Its sparse-damage measurements
remain valid; its no-mode-switch recommendation is retained below as historical design evidence.

**Still required before claiming the A10 wire win:** interactive Ghostty confirmation —
the three commands in `RELEASE.md` must show displayed-FPS improvement and no stale tiles,
tearing, or bad teardown while switching between a shared base and direct tile overlays. Do not
treat the implementation as visually proven until that check lands.

---

## 0. TL;DR

1. **Chromium's own damage rect is the design.** I captured the real `paint` dirty rects from the
   production OSR path at the real Ghostty geometry. Typing one character reports
   `(604,411,10,19)` — **0.01%** of the frame. An 80×80 animated region reports 0.32%. Scrolling
   reports **100%**, every time. Damage tracking is worth 25–74× on interaction and exactly
   nothing on scroll. (§2)

2. **Original design result (superseded for dense frames): a persistent tile mosaic is cheap in
   bytes/CPU.** Redrawing the *entire* screen as 222 separate tile images
   costs **1.006–1.010×** a single full-frame image, measured across all 8 captured frames; at
   coarser tiles it is 2–4% *cheaper* than one image. There is therefore **no tile-vs-full-frame
   mode switch in that cost model. The shipping hybrid adds a dense switch because terminal
   image-object churn and `t=s` were outside that model. (§5.2 and implementation status)

3. **Two protocol paths are closed and one is open.** Ghostty returns
   `"ERROR: unimplemented action"` for `a=f`/`a=a`/`a=c`, so the kitty *animation-frame* partial
   update (the obvious way to patch an image in place) does not exist on our only verified
   terminal — and `q=2` would swallow the error, leaving a silently frozen screen. Re-transmitting
   an image id *deletes all of that image's placements*, so ids must be bound to screen position,
   never recycled across positions. (§3)

4. **The current CLI wastes 3.2 ms per frame before it encodes anything.**
   `Renderer::on_frame` runs `bgra_to_rgb` over the whole 2,020,348-pixel frame
   (`apps/cli/src/main.rs:837`) on every frame regardless of damage. Measured: **3,196 µs**. The
   damage path replaces it with 14–29 µs. (§8.2)

5. **`bgra_rect_to_rgb` is not bounds-safe and the CLI must clamp before calling it.** A rect
   overhanging the right edge **silently reads across into the next row** (skewed pixels, no
   error); a rect overhanging the bottom **panics**. `Rect::clamp_to` already exists and is
   correct. (§8.3)

6. **Tearing needs DEC mode 2026, which the repo does not use anywhere.** Ghostty defines
   `.{ .name = "synchronized_output", .value = 2026 }`. Without it a multi-tile frame renders
   progressively *and* each re-transmitted tile briefly has no placement at all. Adopting 2026
   also **mandates** adding `\x1b[?2026l` to `RESTORE_SEQ` — dying inside a synchronized update
   leaves the user's terminal frozen. (§7)

**Single most actionable item:** §8, steps A–D — feed `FrameHeader.dirty_*` through `clamp_to`
into a position-bound tile mosaic. Measured on real captures: **83× fewer wire bytes and 74×
less encode CPU** on a keystroke, with no change to the kitty wire grammar C01 already verified.

---

## 1. Evidence base

Everything below is measured on this machine, not modelled. Two harnesses, both outside the repo,
neither modifying any repo file.

| Harness | Path | What it did |
|---|---|---|
| OSR capture | `$SCRATCH/capture2.js` | Standalone Electron script; offscreen `BrowserWindow` at 2482×814, `force-device-scale-factor=1`, logs every `paint` dirty rect and dumps checkpoint frames as raw BGRA |
| Encoder bench | `$SCRATCH/c08bench` | Detached cargo crate with a path dependency on `tf-term`; calls the **real** `kitty::bgra_rect_to_rgb` + `kitty::encode_rgb_frame` at the level the CLI actually passes (`1`, `apps/cli/src/main.rs:866`) |

Fidelity check: captured frames are **8,081,392 bytes at 2482×814**, byte-for-byte the payload
size in the end-to-end Ghostty run cited in the brief (8,081,424 − 32-byte header). This is the
production `paint` → `image.toBitmap()` path, not `capturePage` (which returns 4964×1628 at the
Retina scale factor and would have been the wrong measurement).

Content: `https://en.wikipedia.org/wiki/Terminal_emulator` — dense body text, the realistic case.

Geometry, from the brief and confirmed arithmetic: cell 17×37 px, window 2482×851, page area
2482×814. `2482 = 146 × 17` and `814 = 22 × 37` exactly, so the page is exactly **146 × 22 cells**
on this display. §8.6 explains why the code must not *rely* on that being exact.

**A calibration note on the headline number.** The verified e2e run reports 53,999 wire bytes in
0.74 ms. That was `example.com` — a nearly blank page. The same encoder on a real content page
measures **442,805 bytes in 7.66 ms** (8.2× the bytes, 10.4× the time). Every budget below uses
the real-page figure.

### Primary sources

| Source | URL | Used for |
|---|---|---|
| Kitty graphics protocol (RST, authoritative) | https://raw.githubusercontent.com/kovidgoyal/kitty/master/docs/graphics-protocol.rst | placement ids, re-transmit semantics, deletion, crop keys, quota |
| Ghostty command parser | https://raw.githubusercontent.com/ghostty-org/ghostty/main/src/terminal/kitty/graphics_command.zig | which keys Ghostty parses |
| Ghostty command executor | https://raw.githubusercontent.com/ghostty-org/ghostty/main/src/terminal/kitty/graphics_exec.zig | which actions Ghostty *implements* |
| Ghostty modes table | https://raw.githubusercontent.com/ghostty-org/ghostty/main/src/terminal/modes.zig | DEC 2026 support |

---

## 2. What Chromium actually reports as damage

72 paints captured. Dirty rects by phase, deduplicated:

| Phase | paints | dirty rect `(x,y,w,h)` | % of frame |
|---|---|---|---|
| page load | 7 | `(0,0,2482,814)` ×2, `(0,23,2482,791)`, `(511,136,1476,444)`, `(495,23,1222,133)`, `(1739,136,248,476)`, `(489,202,20,133)` | 100 / 97.2 / 32.4 / 8.0 / 5.8 / 0.13 |
| scroll +3 px | 1 | `(0,0,2482,814)` | **100.00** |
| scroll +37 px | 1 | `(0,0,2482,814)` | **100.00** |
| scroll +400 px | 10 | `(0,0,2482,814)` ×1, `(2466,0,16,814)` ×9 | 100 / 0.64 |
| DOM insert | 5 | `(0,0,2482,814)`, `(600,0,1882,814)`, `(2466,0,16,814)` ×3 | 100 / 75.8 / 0.64 |
| type 1 char | 1 | `(604,411,10,19)` | **0.01** |
| type 24 chars | 1 | `(604,411,232,19)` | **0.22** |
| 80×80 animation | 46 | `(2382,714,80,80)` ×46 | **0.32** |

Four things fall out of this table and they determine the entire design.

**Scrolling is 100% damage, reported by Chromium itself.** A07 §0.1 reached the same conclusion by
pixel-diffing and measured a 0.2% saving; this is the independent confirmation from the producer
side. No tiling scheme, no bounding box, and no delta scheme recovers anything on scroll, because
there is genuinely nothing to recover — every pixel moved. Stop trying.

**Interaction is 0.01–0.32% damage.** This is where all the value is, and the win is enormous
(§5.3). Note that the 46 consecutive identical rects from the animation case are exactly the
steady-state pattern that a browser sitting on a page with a spinner or a caret will produce
forever.

**Chromium emits one rect per paint, not a region list.** So the accumulator (§6.3) only ever has
to union rects, never manage a general region algebra.

**Damage is frequently a thin horizontal band or a thin vertical bar.** `(604,411,232,19)` is
19 px tall — half a cell. `(2466,0,16,814)` is 16 px wide — under one cell. Cell-snapping inflates
these substantially (19 px → 37 px, 16 px → 17 px), which is a real and accepted cost of the
mosaic model, quantified in §5.4.

---

## 3. What the protocol permits — and the two paths it closes

### 3.1 Closed: animation-frame partial update (`a=f`)

The spec does define a genuine partial-update mechanism: *"If the frame has data for only a part
of the image, you can specify the rectangle for it using the `x, y, s, v` keys"*. It is the
obvious answer to this mission and it is **unavailable**.

Ghostty parses the action — `transmit_animation_frame, // f` exists in its `Action` enum — but
`graphics_exec.zig` routes it to:

```zig
.transmit_animation_frame,
.control_animation,
.compose_animation,
=> .{ .message = "ERROR: unimplemented action" },
```

Combined with C01's D1 finding that `q=2` suppresses failure responses in both kitty and Ghostty,
an `a=f`-based renderer on Ghostty would emit bytes, receive nothing, and display nothing — a
silently frozen page with no diagnostic. **Do not build on `a=f`.** Mark it re-evaluable only if
Ghostty implements it and we have the `q=1` diagnostic channel C01 recommends.

### 3.2 Closed: recycling image ids across screen positions

Spec, verbatim: *"When re-transmitting image data for a specific id, the existing image and all
its placements must be deleted."*

This is the single most dangerous sentence in the protocol for a tiled renderer. If tile A at
screen position P was drawn with image id 1042, and later some unrelated damage rect is assigned
id 1042 at position Q, then **the pixels at P are erased** — a hole in the page that nothing will
repaint until the next full redraw. An id pool, a ring buffer, or an LRU of ids is therefore
incorrect by construction unless it is paired with a full-redraw keyframe on every wrap.

The consequence is the central invariant of this design (§6.1): **an image id is bound to a screen
rectangle for the lifetime of the layout, never to a transient damage rect.**

### 3.3 Open: what we may actually use

| Mechanism | Spec status | Ghostty | Verified by us |
|---|---|---|---|
| `a=T,f=24,t=d,o=z,s,v,i,C=1,m` at the cursor | normative | implemented | **YES** — C01 byte-audit + e2e Ghostty run |
| Placement anchored at cursor cell, *"from the upper left corner of the current cell"* | normative | implemented | inferred from working e2e |
| `p=` placement id; *"two placements with the same image id and placement id — the second one will replace the first"* | normative | `Display.placement_id` parsed | **UNVERIFIED on hardware** |
| `a=p` + `x,y,w,h` source-rect crop | normative | parsed + `display()` implemented | **UNVERIFIED on hardware** (A07 §0.2 depends on this) |
| `z=` z-index, *"negative z-index values mean images will be drawn under the text"* | normative | parsed | **UNVERIFIED on hardware** |
| `a=d,d=i,i=<id>,p=<pid>` — *"only the placement with the specified image id and placement id will be deleted"* | normative | `Delete.id.placement_id` parsed | **UNVERIFIED on hardware** |
| DEC 2026 synchronized output | — | `.{ .name = "synchronized_output", .value = 2026 }` | **UNVERIFIED on hardware** |
| `a=f` animation frames | normative | **rejected at exec** | closed, §3.1 |

### 3.4 Storage quota — correcting A07

A07 §3.7 records the 320 MB figure as *"UNVERIFIED — it is not in the specification text"*. It is
in the specification text. Fetching the authoritative RST returns: *"For example the quota in
kitty is 320MB per buffer."* and *"when the terminal is running out of quota space for new images,
existing images without placements will be preferentially deleted."*

That second sentence is load-bearing for us in a good way: **every mosaic tile always has a live
placement**, so mosaic tiles sit in the *last* eviction tier. Worst-case residency for the
recommended grid is 222 tiles × 68×148 px × 4 B = **8.9 MB**, ~2.8% of kitty's stated quota. The
spec states no minimum image *count* a terminal must support, so tile count is capped in §5.5 as a
defensive measure rather than a spec-driven one.

---

## 4. The design: a persistent cell-aligned tile mosaic

The page is not one image that gets replaced. The page is a **fixed mosaic of tiles**, each tile
an independent kitty image with a permanent id derived from its grid coordinate, each with exactly
one placement pinned to a cell position on the alternate screen. A frame update re-transmits only
the tiles that intersect the damage. Tiles nobody touched keep displaying, for free, forever.

```
page 2482 x 814 px  =  146 x 22 cells   (cell 17x37)
tile 4 x 4 cells    =  68 x 148 px      -> grid 37 x 6 = 222 tiles

        col 0      col 1      col 2   ...        col 36
      +----------+----------+----------+ ... +----------+
row 0 | id 1000  | id 1001  | id 1002  |     | id 1036  |   cells rows 1-4
      +----------+----------+----------+ ... +----------+
row 1 | id 1037  | id 1038  | id 1039  |     | id 1073  |   cells rows 5-8
      +----------+----------+----------+ ... +----------+
 ...
row 5 | id 1185  | ...                              1221 |   cells rows 21-22 (74px, clamped)
      +----------+----------+----------+ ... +----------+
                                                                status bar: cell row 23
```

Why this shape and not the alternatives:

**Why not one image plus overlay patches?** Overlays are correct while they accumulate and become
incorrect the moment you recycle an overlay id (§3.2) — the region it covered reverts to a stale
base. Bounding the pool then forces a full-frame keyframe on every wrap, which reintroduces the
442 KB / 7.7 ms hitch you were trying to avoid. The mosaic has no base to go stale and needs no
garbage collection at all.

**Why not a bounding box of all damage?** Two small damages at opposite corners union to the whole
screen. Measured (§5.4): two 68×148 rects at opposite diagonal corners cost 588 B split and
442,815 B merged — a 753× penalty for one merge decision. A grid bounds this by construction.

**Why cell-aligned?** Kitty places an image *"at the current cursor position, from the upper left
corner of the current cell."* Sub-cell placement exists (`X`/`Y`, *"the offsets must be smaller
than the size of the cell"*), but requiring it means every tile carries two more control keys and
two more failure modes. Aligning the grid to cells removes the need entirely: `ESC[row;colH`
followed by the exact `a=T` command C01 already byte-audited. **The damage path introduces no new
kitty grammar.** That property is worth more than the pixels it costs.

**Why does the mosaic not cost anything?** Because tiles are compressed independently and most of
a web page is locally uniform, per-tile zlib recovers roughly what it loses in cross-tile context.
Measured across all 8 frames (§5.2): 1.006–1.010× at 4×4 cells, and *cheaper than one image* at
8×4 and coarser.

---

## 5. Cost model and the decision rule

### 5.1 The cost function, with measured constants

For a rectangle `r` transmitted as one kitty image:

```
C(r)  =  C_fix  +  ceil(4/3 · (1 + 9/4096) · 3 · rho · area(r))

C_fix = 75 B        49 B escape envelope + control keys, 16 B minimum payload, 10 B cursor move
rho                 zlib level-1 compression ratio of the content in r
```

Collapsing the constants gives the form to actually implement, where `beta` is **wire bytes per
source pixel**, tracked as an EWMA of the previous frame's `wire_bytes / pixels_encoded`:

```
C(r)  =  75  +  beta · area(r)              beta = 4.01 · rho
```

Calibration against the measured encoder, all at level 1 on the real page:

| measurement | source | derived |
|---|---|---|
| 1×1 px tile → 65 B wire | bench §1 | envelope + min payload = 65 B; +10 B cursor = **C_fix 75 B** |
| full frame 6,061,044 B raw → 442,805 B wire, ratio 18.1× | bench §1 | rho = 0.0552, **beta = 0.219 B/px** |
| model check: 75 + 0.219 × 2,020,348 | — | 442,531 B vs measured 442,805 B — **0.06% error** |

`beta` is strongly content-dependent and must be measured, not assumed:

| content | rect | wire B | beta (B/px) |
|---|---|---|---|
| dense body text | 255×37 | 3,596 | 0.381 |
| mixed page, whole frame | 2482×814 | 442,805 | 0.219 |
| mixed page, 4×4-cell tile | 68×148 | 284 | 0.028 |
| flat colour | 85×111 | 272 | 0.029 |

### 5.2 Historical tile-vs-full-frame model (dense conclusion superseded)

Under the mosaic, a "full frame" *is* the mosaic with every tile dirty. So with `N` tiles total,
`n` dirty, uniform tile area `A_t`:

```
C_tiles  =  n · (C_fix + beta · A_t)
C_full   =  N · (C_fix + beta · A_t)
C_tiles / C_full  =  n / N   <=  1      for all n
```

The tiled path is never worse. The only way this could be false is if the mosaic itself were more
expensive than a single monolithic image — the "mosaic tax". Measured on all 8 captured frames,
whole-screen redraw, level 1, including 10 B/tile of cursor positioning:

| frame | 1 image (B) | 4×4 cells, 222 tiles | tax | 8×4 cells, 114 tiles | tax | 16×8 cells, 30 tiles | tax |
|---|---|---|---|---|---|---|---|
| a_base | 442,805 | 447,347 | 1.010× | 432,492 | 0.977× | 425,230 | 0.960× |
| b_scroll3 | 446,550 | 451,014 | 1.010× | 436,494 | 0.977× | 429,094 | 0.961× |
| c_scroll37 | 453,556 | 458,239 | 1.010× | 443,769 | 0.978× | 437,781 | 0.965× |
| d_scroll400 | 628,291 | 633,309 | 1.008× | 616,579 | 0.981× | 612,988 | 0.976× |
| e_type0 | 620,961 | 624,554 | 1.006× | 608,092 | 0.979× | 604,730 | 0.974× |
| f_type1 | 621,337 | 624,786 | 1.006× | 608,288 | 0.979× | 604,914 | 0.974× |
| g_type24 | 624,714 | 628,199 | 1.006× | 611,582 | 0.979× | 607,976 | 0.973× |
| h_corner | 625,374 | 628,987 | 1.006× | 612,654 | 0.980× | 609,236 | 0.974× |

**8 of 8: the tax at 4×4 cells never exceeds 1.010×, and at 8×4 and coarser the mosaic is
consistently 2–4% cheaper than a single image.** The worst case of the mosaic model is a 1%
regression against today's behaviour; the best case is 83×.

**Original conclusion (now superseded): do not implement a mode switch.** There is one code path — mark tiles dirty, send
dirty tiles. A07's `FrameStrategy::FullFrame` and `FrameStrategy::DirtyTiles` arms collapse into
one. This is the main simplification C08 contributes over A07 §3.8.

For completeness, the general rule if a future backend has a materially different mosaic tax `T`:

```
send the mosaic  iff   n/N  <  1/T       ;  with measured T <= 1.010, threshold = 0.990
```

At `n/N > 0.990` — 220 of 222 tiles — you may fall back to one image for a <1% saving. Not worth
the second code path; **use the mosaic unconditionally.**

### 5.3 Measured end-to-end, real transitions

Current full-frame path (convert whole frame + encode whole frame) versus the mosaic path
(convert + encode only touched tiles), both at level 1:

| transition | full-frame B | full-frame ms | mosaic B | mosaic ms | tiles | byte win | CPU win |
|---|---|---|---|---|---|---|---|
| scroll +37 px | 453,556 | 7.57 | 443,769 | 8.55 | 114 | 1.02× | 0.89× |
| **type 1 char** | 621,337 | 8.97 | **7,488** | **0.12** | 1 | **83×** | **74×** |
| **type 24 chars** | 624,714 | 8.29 | **22,389** | **0.34** | 3 | **28×** | **25×** |
| **80×80 corner anim** | 625,374 | 7.80 | **2,196** | **0.14** | 4 | **285×** | **56×** |

(mosaic column uses 8×4-cell tiles; the 4×4 default in §5.5 is comparable — 7,681 / 14,126 /
1,586 B for the same three interaction cases.)

Scroll is the honest negative result: the mosaic is ~11% *slower* in CPU on a 100%-damage frame
because 114 separate deflate streams have more setup than one. It is within noise on bytes. This
is the price of the model and it is small.

Where the CPU win matters: the measured p50 frame gap is 16.65 ms. A full-frame encode is
7.6–9.0 ms of that, i.e. **half the frame budget spent re-encoding pixels that did not change.**

### 5.4 Merge versus split, when damage is disjoint

Only relevant to the optional patch layer (§9); the mosaic itself cannot merge because ids are
position-bound. Two 68×148 rects on the same row, separated by a growing gap:

| gap (tiles) | union px | union area / summed area | split B | merged B | winner |
|---|---|---|---|---|---|
| 0 (adjacent) | 136×148 | 1.00× | 588 | **483** | MERGE |
| 1 | 204×148 | 1.50× | **588** | 679 | SPLIT |
| 2 | 272×148 | 2.00× | **588** | 871 | SPLIT |
| 4 | 408×148 | 3.00× | **588** | 1,255 | SPLIT |
| 7 | 612×148 | 4.50× | **5,793** | 12,814 | SPLIT |

Merging wins by 18% at zero inflation and loses by 15% at 1.5×. Linear interpolation puts the
crossover at **1.27×**. The rule:

```
merge(a, b)  iff   area(a ∪ b)  <=  1.25 · (area(a) + area(b))
```

Conservative on the measured crossover, and cheap: it is two multiplications on `Rect::union`,
which `tf-term` already provides (`crates/tf-term/src/lib.rs:39`).

### 5.5 Choosing the tile size

Two competing pressures, both measured. Full-redraw encode time falls as tiles get coarser;
sparse-damage cost falls as tiles get finer.

| tile (cells) | px | tiles | full-redraw B | tax | full-redraw ms | 1-char typing B |
|---|---|---|---|---|---|---|
| 1×1 | 17×37 | 3,212 | 798,464 | 1.803× | 38.28 | ~200 |
| 2×2 | 34×74 | 803 | 527,930 | 1.192× | 14.59 | 2,174 |
| 4×2 | 68×74 | 407 | 470,519 | 1.063× | 10.50 | — |
| **4×4** | **68×148** | **222** | **447,347** | **1.010×** | **8.24** | **7,681** |
| 8×4 | 136×148 | 114 | 432,492 | 0.977× | 7.50 | 7,478 |
| 8×8 | 136×296 | 57 | 425,311 | 0.960× | 6.75 | — |
| 16×8 | 272×296 | 30 | 425,230 | 0.960× | 6.57 | 18,521 |

Selection rule, in order:

```
1.  A_t >= 20 · C_fix / beta_worst              fixed-overhead efficiency
        = 20 · 75 / 0.4  = 3,750 px             -> >= 2x2 cells here
2.  T_full(A_t) <= 0.5 · frame_budget           leave half the 16.65 ms for write + status
        = 8.3 ms                                 -> >= 4x4 cells here
3.  N = ceil(cols/tcw) · ceil(rows/tch) <= 256  defensive cap on terminal image count
4.  subject to 1-3, choose the SMALLEST A_t     sparse damage dominates real usage
```

On this display that resolves to **4×4 cells = 68×148 px, 222 tiles**. Rule 2 excludes 2×2
(14.59 ms is 88% of the frame budget). Rule 3 is not binding here but bounds a 300×80-cell
terminal to 8×4 tiles (`ceil(300/8)·ceil(80/4) = 38·20 = 760` exceeds 256, so it would step up to
16×8 → 19×10 = 190).

Constant to expose so this is retunable without a rebuild: `TERMINAL_FENSTER_TILE_CELLS=4x4`.

---

## 6. Image and placement id management

### 6.1 The invariant

> **An image id is bound to a screen rectangle for the lifetime of the layout. It is never reused
> at a different position while its placement is live.**

This is forced by the spec sentence in §3.2 and it is the one rule that, if broken, produces holes
in the page that no amount of retrying will fix.

### 6.2 Allocation

```rust
// crates/tf-term/src/kitty.rs — proposed additions (commander's call)

/// Base of the id block owned by the page mosaic. Tile (col, row) always gets the same id.
pub const PAGE_TILE_ID_BASE: u32 = 1000;
/// Upper bound of the block, so overlays/chrome can be namespaced above it without collision.
pub const PAGE_TILE_ID_MAX: u32 = 1999;
/// Single placement id used for every tile. Non-zero so the placement is individually
/// addressable for deletion; scoped per-image, so one value is sufficient for all tiles.
pub const TILE_PLACEMENT_ID: u32 = 1;

#[inline]
pub fn tile_image_id(col: u32, row: u32, grid_cols: u32) -> u32 {
    PAGE_TILE_ID_BASE + row * grid_cols + col
}
```

`PAGE_IMAGE_ID = 1000` (`kitty.rs:30`) becomes `tile_image_id(0,0,_)`, so the constant keeps its
value and the single-image path remains a special case of the mosaic (grid 1×1).

Rules:

- **Same tile, new pixels** — re-transmit the same id with `a=T` at the same cursor cell. The spec
  deletes the old image *and its placement*, then the `a=T` creates the replacement at the cursor.
  Net effect is an in-place update, self-cleaning, no leak. C01 item 10 already validated this
  reasoning for the single-image case; it generalises unchanged.
- **Tile no longer needed** (grid shrank on resize) — `a=d,d=I,i=<id>` to free data *and*
  placement. Add `q=2` per C01 D3 so no fire-and-forget delete can elicit a reply into stdin.
- **Layout change** (resize, DPI change, backend switch) — the id↔position binding is invalidated
  wholesale. Emit `a=d,d=A` (which the spec defines as *"all placements visible on screen"*, C01
  D6), rebuild the grid, mark every tile dirty. Do **not** attempt to remap ids.

### 6.3 Damage accumulation

The dirty set is a `Vec<bool>` of length `N`, OR-accumulated. It must survive frame coalescing on
**both** sides of the socket:

- **Engine side** — B07 §5 correctly identifies that `main.js:96-97` discards the dirty rect of
  every coalesced frame. That bug is currently invisible only because the consumer ignores the
  rect; the moment this design lands it becomes a stale-tile bug on screen. **B07 §10.1 must land
  before or with C08.** The engine must union the dropped frame's dirty rect into the kept one.
- **CLI side** — the same hazard exists locally. `Renderer::on_frame` can run several times per
  `present()` (the poll loop drains all buffered messages before presenting,
  `apps/cli/src/main.rs:532-557`). `self.dirty: bool` therefore already loses damage. Replacing it
  with the tile bitset fixes this by construction: the bitset is only cleared in `present()`.

Fail-safe: if the dirty rect is absent, zero-area, or fails `clamp_to`, set `all_dirty`. **The
degenerate case must be a full redraw, never a skipped update.** A skipped update is a permanently
wrong screen; a full redraw is one slow frame.

### 6.4 Placement id

Every tile carries `p=1`. Because placement ids are scoped per image and each tile has its own
image id, one constant is enough, and it makes each placement individually deletable via
`a=d,d=i,i=<id>,p=1`.

Strictly, `p` is redundant for the `a=T` path — re-transmit already clears placements. It is
**not** redundant for any future `a=p` path, including A07's scroll-crop optimisation: the spec
states that *"not specifying a placement id or using p=0 for multiple put commands with the same
non-zero image id results in multiple placements"*. An `a=p`-per-frame renderer without `p` leaks
one placement per frame, unbounded, at 60 Hz. **Any `a=p` command we ever emit must carry a
non-zero `p`.**

Because `p` is one key beyond the grammar C01 verified end-to-end, §10 lists it as a discrete
conformance step. If it fails on any target terminal, drop it — the `a=T` path is correct without
it.

---

## 7. Avoiding visible tearing

Five distinct mechanisms; the first two are new hazards created by this design.

**7.1 Progressive render of a multi-tile frame.** A 114-tile update is ~430 KB across ~180 APC
commands. The terminal parses and composites as bytes arrive, so the user sees the page repaint
tile by tile. Today's single-image frame does not have this problem.

**7.2 The placement gap on re-transmit.** Re-transmitting id `i` deletes the old image *and its
placement* before the new placement exists. If the terminal renders in that window, the tile is
briefly blank. At 60 Hz across many tiles this reads as sparkle.

Both are solved by the same thing: **DEC private mode 2026, synchronized output.** Wrap the entire
per-frame byte block:

```
\x1b[?2026h   ... cursor moves, tile transmissions, status bar ...   \x1b[?2026l
```

Ghostty defines it (`modes.zig`: `.{ .name = "synchronized_output", .value = 2026 }`). The repo
uses it nowhere — `grep -rn "2026" crates/ apps/` returns only the `\u{2026}` ellipsis in
`unicode.rs`.

**Detection:** DECRQM, exactly as `caps.rs:182` already does for mode 1016. Send `\x1b[?2026$p`,
feed the reply to the existing `parse_decrqm_supported` (`caps.rs:202`) — it is mode-agnostic and
already tests values 0/1/2/3/4 correctly. Three lines.

**7.3 The hazard 2026 introduces — this must not be missed.** If the process dies between
`?2026h` and `?2026l`, the terminal stays in synchronized mode and **the user's screen is frozen**.
That is a worse failure than anything in the current teardown path.

> `RESTORE_SEQ` in `crates/tf-term/src/tty.rs:27` **must** gain `\x1b[?2026l` as its **first**
> element, before the kitty delete and before leaving the alternate screen. Adopting 2026 without
> this change is a net regression in the crate's stated top invariant.

The signal-handler path already writes `RESTORE_SEQ` with async-signal-safe `write(2)`
(`tty.rs:55-60`), so no new machinery is needed — only the extra bytes.

**7.4 Partial writes.** `present()` issues one `write_all` + `flush`
(`apps/cli/src/main.rs:899-901`) and stdout is left blocking, so this is already correct. Keep it:
**one `write_all` per frame, never one per tile.** The 2026 wrap makes an interrupted write
recoverable rather than visible, but it does not make many small writes acceptable — each is a
syscall and a scheduling opportunity.

**7.5 Cross-frame interleaving.** C01 D9 notes that a signal arriving mid-transfer injects the
teardown delete into a partially transmitted chunked command. With 2026 the same applies to the
synchronized block. The mitigation is unchanged and cheap: emit `?2026l` first in `RESTORE_SEQ`
(7.3), which closes the block before anything else is written.

**Fallback when 2026 is unsupported** (Apple Terminal has no DECRQM at all, so it will report
unsupported): order tile writes **top-to-bottom, left-to-right** so a partial render looks like a
progressive top-down repaint rather than random sparkle. This costs nothing — it is the natural
iteration order of the grid — and is the one visual-quality knob available without the mode.

---

## 8. Exactly how the CLI should use `bgra_rect_to_rgb`

`bgra_rect_to_rgb` (`crates/tf-term/src/kitty.rs:54`) currently has **no callers anywhere in the
repo** — C01 D10 flagged it as dead code. This section is the instruction set that wakes it up.
All of it is in `apps/cli/src/main.rs`, which the commander owns.

### 8.1 The three-argument trap

```rust
kitty::bgra_rect_to_rgb(bgra, img_w, rect, out)
//                            ^^^^^
```

`img_w` is the **stride of the source frame**, i.e. `FrameHeader.width` — *not* the tile width.
Passing `rect.w` compiles, runs, produces no error, and yields diagonally skewed garbage. The
call site should read `self.page_w`, and there should be a debug assertion that
`self.bgra.len() == (self.page_w * self.page_h * 4) as usize`.

### 8.2 Stop converting the whole frame

`Renderer::on_frame` currently calls `kitty::bgra_to_rgb` over all 2,020,348 pixels
(`apps/cli/src/main.rs:837`). Measured cost:

| conversion | pixels | measured |
|---|---|---|
| `bgra_to_rgb`, full frame | 2,020,348 | **3,196 µs** |
| `bgra_rect_to_rgb`, 8×4-cell tile | 20,128 | 29 µs |
| `bgra_rect_to_rgb`, typing rect | 9,435 | 14 µs |

`Renderer` must keep the **BGRA** frame and convert per tile inside `present()`. That deletes
3.2 ms from every frame before any encoding happens.

### 8.3 Clamp before every call — this is a correctness requirement, not hygiene

`bgra_rect_to_rgb` does no bounds checking (`kitty.rs:57-67`). Measured behaviour on a 10×10
image:

| input rect | result |
|---|---|
| `Rect::new(8, 0, 5, 1)` — 3 px past the right edge | **no error; silently reads into the next row** → skewed pixels |
| `Rect::new(0, 8, 10, 5)` — 3 rows past the bottom | **panic**: `range end index 440 out of range for slice of length 400` |
| `Rect::new(0, 0, 0, 0)` — zero area | encoder emits 61 bytes of `s=0,v=0`, an invalid command that `q=2` silences (C01 D7) |

The horizontal case is the dangerous one: no panic, no error, wrong pixels on screen.
`FrameHeader.dirty_*` arrives over a socket and is parsed without validation
(`tf-proto/src/lib.rs:33-48`), so this is also the B06 attack surface reaching a memory-adjacent
primitive.

`Rect::clamp_to` (`lib.rs:53`) is already correct — verified: `(8,0,5,1).clamp_to(10,10)` →
`Some(Rect{x:8,y:0,w:2,h:1})`, `(99,99,5,5).clamp_to(10,10)` → `None`.

> **Rule: every `bgra_rect_to_rgb` call site must take its rect from `clamp_to(page_w, page_h)`
> and must reject `None` and `is_empty()`.**

### 8.4 `Renderer` — proposed shape

```rust
struct TileGrid { tw: u32, th: u32, cols: u32, rows: u32 }

impl TileGrid {
    fn count(&self) -> u32 { self.cols * self.rows }
    /// Unclamped tile rect; callers must still clamp_to(page_w, page_h).
    fn rect(&self, idx: u32) -> Rect {
        let (i, j) = (idx % self.cols, idx / self.cols);
        Rect::new(i * self.tw, j * self.th, self.tw, self.th)
    }
}

struct Renderer {
    backend: Backend,
    page_w: u32, page_h: u32,
    cell_w: u32, cell_h: u32,
    grid: TileGrid,
    bgra: Vec<u8>,          // latest frame, BGRA, owned. NOT converted on arrival.
    dirty: Vec<bool>,       // len == grid.count(); OR-accumulated across coalesced frames
    all_dirty: bool,        // first frame, resize, or unusable damage rect
    status_dirty: bool,     // status bar changed without any page damage
    sync_output: bool,      // DEC 2026 supported (from caps)
    rgb: Vec<u8>,           // scratch, reused across tiles
    out: Vec<u8>,           // scratch, one write_all per frame
    frame_times: Vec<Instant>,
}
```

### 8.5 `on_frame` — accumulate, never encode

```rust
fn on_frame(&mut self, payload: &[u8], status: &mut Status) {
    let Some(h) = proto::FrameHeader::parse(payload) else { return };
    if payload.len() < proto::FRAME_HEADER_LEN { return }
    let pixels = &payload[proto::FRAME_HEADER_LEN..];
    if pixels.len() < h.expected_payload() { return }   // truncated: drop, as today

    if h.width != self.page_w || h.height != self.page_h {
        self.relayout(h.width, h.height);               // rebuild grid; queue a=d,d=A; all_dirty
    }

    // One memcpy. No colour conversion here at all.
    self.bgra.clear();
    self.bgra.extend_from_slice(&pixels[..h.expected_payload()]);

    // MANDATORY validation. See 8.3.
    match Rect::new(h.dirty_x, h.dirty_y, h.dirty_w, h.dirty_h).clamp_to(h.width, h.height) {
        Some(d) if !d.is_empty() => self.mark_dirty(d),
        _ => self.all_dirty = true,                     // fail safe = redraw everything
    }

    status.frames += 1;
    // ... existing fps bookkeeping unchanged ...
}

fn mark_dirty(&mut self, d: Rect) {
    // d.w >= 1 and d.h >= 1 guaranteed by the is_empty() check above, so no underflow.
    let (c0, c1) = (d.x / self.grid.tw, (d.x + d.w - 1) / self.grid.tw);
    let (r0, r1) = (d.y / self.grid.th, (d.y + d.h - 1) / self.grid.th);
    for j in r0..=r1.min(self.grid.rows - 1) {
        for i in c0..=c1.min(self.grid.cols - 1) {
            self.dirty[(j * self.grid.cols + i) as usize] = true;
        }
    }
}
```

### 8.6 `present` — the damage path

```rust
fn present(&mut self, status: &mut Status, rows: u16) {
    if self.bgra.is_empty() { return }
    let n_dirty = if self.all_dirty { self.grid.count() as usize }
                  else { self.dirty.iter().filter(|d| **d).count() };
    if n_dirty == 0 && !self.status_dirty { return }   // idle => zero bytes, zero syscalls

    self.out.clear();
    if self.sync_output { self.out.extend_from_slice(b"\x1b[?2026h") }

    let t0 = Instant::now();
    let mut wire = 0usize;
    let mut px = 0u64;

    if matches!(self.backend, Backend::Kitty) {
        // Top-to-bottom, left-to-right: without mode 2026 this degrades to a progressive
        // top-down repaint rather than random sparkle. See 7.5.
        for idx in 0..self.grid.count() {
            if !self.all_dirty && !self.dirty[idx as usize] { continue }
            let Some(r) = self.grid.rect(idx).clamp_to(self.page_w, self.page_h) else { continue };
            if r.is_empty() { continue }

            // Cursor to the tile's top-left CELL. 1-based. Tiles are cell-aligned by
            // construction because tw/th are whole multiples of cell_w/cell_h.
            let col = r.x / self.cell_w + 1;
            let row = r.y / self.cell_h + 1;
            self.out.extend_from_slice(format!("\x1b[{row};{col}H").as_bytes());

            // page_w is the SOURCE STRIDE, not the tile width. See 8.1.
            kitty::bgra_rect_to_rgb(&self.bgra, self.page_w, r, &mut self.rgb);

            let place = kitty::Placement {
                image_id: kitty::tile_image_id(r.x / self.grid.tw, r.y / self.grid.th, self.grid.cols),
                ..Default::default()
            };
            if let Ok(st) = kitty::encode_rgb_frame(&self.rgb, r.w, r.h, place, 1, &mut self.out) {
                wire += st.wire_bytes;
                px += r.area();
            }
        }
    } else {
        // Unicode backend has no per-tile addressing; redraw whole, as today.
    }

    // status bar, unchanged (sanitized title/url) ...

    if self.sync_output { self.out.extend_from_slice(b"\x1b[?2026l") }
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(&self.out);      // exactly one write per frame
    let _ = stdout.flush();

    status.last_wire_bytes = wire;
    status.last_encode_ms = t0.elapsed().as_secs_f64() * 1000.0;
    status.beta = if px > 0 { 0.8 * status.beta + 0.2 * (wire as f64 / px as f64) } else { status.beta };
    status.damage_ratio = n_dirty as f64 / self.grid.count() as f64;  // A10 §4.1 damage_area_ratio

    self.dirty.iter_mut().for_each(|d| *d = false);
    self.all_dirty = false;
    self.status_dirty = false;
}
```

### 8.7 One geometry fix in `cmd_open`

`apps/cli/src/main.rs:254` computes `page_h = vp_h - cell_h`. If `vp_h` is not a whole multiple of
`cell_h` — which is only true by luck on this display, since `viewport_px()` returns the *window*
pixel size (`caps.rs:73-88`) and window chrome or padding can make it arbitrary — then the bottom
tile row ends mid-cell and bleeds pixels into the status-bar row.

```rust
// snap the page to a whole number of cells so the mosaic grid is exact and the
// status row is never overpainted
let page_rows = (vp_h / cell_h as u32).saturating_sub(1).max(1);   // reserve one row for status
let page_h    = page_rows * cell_h as u32;
let page_cols = (vp_w / cell_w as u32).max(1);
let page_w    = page_cols * cell_w as u32;
```

This also makes the tile grid derivable from **cells**, which is the robust formulation:
`grid.cols = ceil(page_cols / tile_cells_w)`, `grid.rows = ceil(page_rows / tile_cells_h)`.

### 8.8 Compression level

Level 1 remains right for the local path. Measured on the full frame:

| level | wire B | encode µs |
|---|---|---|
| 0 | 8,099,190 | 5,338 |
| **1** | **442,805** | **3,600** |
| 3 | 368,891 | 10,364 |
| 6 | 360,885 | 22,120 |

Level 3 buys 17% fewer bytes for 2.9× the CPU — a bad trade locally, a good one over SSH (A07).
On *small* damage rects the CPU cost of a higher level is negligible in absolute terms (typing
rect: level 1 = 3,596 B in 44 µs; level 6 = 3,044 B in 125 µs), so a reasonable refinement is
**level 1 when `n_dirty` is large, level 6 when `n_dirty` is small**, gated on A07's remote flag.
Not required for v1. Independently, C01 D5 applies unchanged: skip `o=z` when deflate expands.

---

## 9. Optional v2: sub-tile patches

The mosaic quantises a 10×19 px caret repaint up to a 68×148 px tile — 7,681 B where the raw
damage would cost 395 B. That is a 19× overhead on top of an 83× win, so v1 is worth shipping
alone. If the animation/remote case justifies more:

Give each tile a **second** image id, `patch_image_id(t) = PAGE_TILE_ID_BASE + 1000 + t`, drawn at
`z=1` over its tile. Maintain per tile `accum[t]` = union of all damage to that tile since the tile
was last fully sent.

```
if area(accum[t]) <= 0.5 · area(tile):
        send accum[t] as patch_image_id(t) at z=1        # cheap
else:
        send the whole tile as tile_image_id(t) at z=0
        emit a=d,d=I,i=patch_image_id(t),q=2             # MUST delete the stale patch first
        accum[t] = empty
```

Correctness rests on one property: **the patch always covers a superset of the previous patch's
area**, because `accum[t]` only grows until the tile is fully redrawn. So reusing the patch id can
never uncover a stale region — the failure mode of §3.2 is structurally excluded. No garbage
collection, no keyframes, two images per tile.

Blocked on verifying `z` ordering on real hardware (§10). Do not build this before v1 ships.

---

## 10. Conformance tests the commander must run

None of the multi-tile behaviour has been exercised against a real terminal — the machine is at a
lock screen, so this is protocol-level and log-level evidence only. These are the checks that
convert it to verified, in dependency order. Each is a `terminal-fenster doctor` subcommand or a
one-shot script; all are CI-able against Ghostty.

| # | Test | Pass criterion | Blocks |
|---|---|---|---|
| 1 | Transmit 4 tiles at 4 distinct cursor cells with 4 distinct ids, `a=T,C=1` | all 4 visible simultaneously, none displaced | everything |
| 2 | Re-transmit tile 2 only | tiles 1,3,4 unchanged; tile 2 updated in place | §6.2 |
| 3 | Re-transmit tile 2 with a *different* id at the same cell | old placement gone, new one drawn — confirms §3.2 empirically | §6.1 |
| 4 | Add `p=1` to the `a=T` of test 1 | identical result to test 1 | §6.4 |
| 5 | `a=d,d=i,i=<tile2>,p=1,q=2` | tile 2 erased, others untouched | §6.2 |
| 6 | DECRQM `\x1b[?2026$p` | reply `\x1b[?2026;{1,2,3}$y` | §7 |
| 7 | 114-tile update wrapped in `?2026h/?2026l` | screen updates in one visual step | §7.1 |
| 8 | SIGINT during a synchronized block | terminal usable, not frozen — validates the `RESTORE_SEQ` change | §7.3 |
| 9 | Full mosaic at 222 tiles held for 60 s at 60 fps | no terminal memory growth, no eviction, no `OK`/error bytes into stdin | §3.4 |
| 10 | `a=p` with `x,y,w,h` crop and `p=1` | crop displayed; repeated puts do not stack placements | A07 §0.2 |

Test 10 matters beyond this document: A07's headline recommendation (scroll by re-cropping a tall
pre-rendered image, 55 B/frame) depends on it. I should flag a practical obstacle A07 does not
address — **Electron OSR paints the viewport, not the document.** Every captured frame here is
exactly 2482×814, the window size; there is no tall off-screen surface to crop from. Producing one
means resizing the `BrowserWindow` to the full document height, which re-lays-out the page and
costs a full re-render. That does not make A07 wrong, but the tall-image cache is a much larger
piece of work than the 55 B/frame figure suggests, and C08's mosaic does not depend on it.

---

## 11. What I could not verify

- **All multi-tile behaviour on real hardware.** Lock screen; no interactive terminal. §10 exists
  because of this. Everything in §4–§8 is derived from the normative spec text, the two reference
  implementations' source at `master`/`main`, and measurements of *our own encoder's output* — not
  from watching Ghostty draw a mosaic.
- **iTerm2 3.6.9** — TCC blocks automation, consistent with the brief and C01. Its kitty graphics
  support, its `p`/`z`/crop handling, and its 2026 support are all UNVERIFIED.
- **Apple Terminal** — no graphics protocol at all, so the mosaic is inapplicable; it falls to the
  Unicode backend, which has no per-tile addressing. The damage path is a no-op there and
  `present()` must not assume otherwise (§8.6 keeps the whole-frame branch).
- **tmux passthrough** — `wrap_tmux` still has no callers (C01 D10). A 222-tile frame through
  DCS passthrough with ESC-doubling is untested and would roughly double the escape overhead.
- **Content generality** — one page (Wikipedia article), one viewport, one zoom. `beta` on
  image-heavy or video pages will be far higher and the mosaic tax may differ. The EWMA in §8.6
  exists so the system measures rather than assumes.
- **The 1.27× merge crossover** is interpolated between two measured points (1.00× and 1.50×), not
  measured at the crossover itself, and is content-dependent. The recommended 1.25 threshold is
  deliberately on the safe side of it.
