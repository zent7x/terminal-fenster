# C07 — HiDPI, Scaling and Colour

**Mission:** specify how terminal pixels map to CSS pixels, how the browser scale is chosen so
text is legible rather than microscopic, how resize is handled, and how colour is managed
between Chromium and the terminal. Give the exact formula to implement.

**Status:** the scaling lever and the entire colour question are **measured**, not reasoned
about. Three Electron probes were run against the repo's own Electron 43.2.0 / Chromium
150.0.7871.129. Everything below marked with a number came out of a probe; everything
inferred is labelled as such.

---

## 0. Decisions

| Question | Decision | Confidence |
|---|---|---|
| How to scale | `win.setSize(page_w, page_h)` + `webContents.setZoomFactor(S)` | **Measured.** Bitmap stays 1:1 with terminal px; CSS viewport divides by `S` |
| How to pick `S` | `S = (cell_w × R) / 9.6`, snapped to a zoom ladder | Derived from the user's own terminal font; validated on 3 terminals |
| `S` for this machine | **1.75** (auto) — `S=1.0` renders text at **half** the physical size of a native browser window on this display | **Measured** display geometry |
| `webPreferences.offscreen.deviceScaleFactor` | **Rejected as the primary lever** — construction-only, cannot change without destroying page state | **Measured** |
| `enableDeviceEmulation({deviceScaleFactor})` | **Rejected — it breaks OSR.** Produces blank white frames and is silently dropped on navigation | **Measured** |
| Input coordinate units under zoom | **Device px — unchanged.** No `S` conversion anywhere in the core | **Measured** |
| `dirtyRect` units | **Device px** (answers B02 §4.1's open question) | **Measured** |
| Colour space of the OSR buffer | **sRGB, byte-exact**, even on this BT.2020 / 10-bit display | **Measured** |
| `--force-color-profile=srgb` | Set it — byte-identical here, but pins the behaviour on other hosts | **Measured no-op** |
| Subpixel (LCD) antialiasing | Already off in OSR — 0 fringed pixels of 4816 scanned | **Measured** |
| Alpha channel | Always 255. Kitty `f=24` (RGB) is safe and saves 25% of wire bytes | **Measured** |

**Single most important consequence:** because the bitmap stays the same size as the terminal's
pixel budget and input coordinates stay in device pixels, adding scale support touches
*only* the engine's zoom call. The frame protocol, the kitty encoder, `PointerMap`, and the
damage-rect path all keep working unchanged. This is the cheapest correct design available.

---

## 1. Evidence and how to reproduce it

Four probes were written to a scratchpad directory (outside the repo — no core files were
touched). The Chromium child-process sandbox conflict noted in the mission brief is real; every
run required the agent sandbox to be disabled. Command shape:

```
./apps/engine/node_modules/electron/dist/Electron.app/Contents/MacOS/Electron \
    <probe>.js --out=<result>.json
```

Probe sources are reproduced in Appendix A so this is re-runnable in CI after the scratchpad is
gone. The decisive raw numbers:

**Host display** (`screen.getPrimaryDisplay()`, probe 1):

```
scaleFactor      2
size             1710 x 1107      (macOS points)
colorDepth       30               (10 bits per component)
colorSpace       {primaries:BT2020, transfer:{... gamma 2.3955 ...}, range:FULL}
```

`system_profiler SPDisplaysDataType` reports the panel as `Resolution: 2880 x 1864 Retina`,
Built-in Liquid Retina. So the desktop is 1710×1107 points on a 2× backing store (3420×2214),
which the compositor then resamples to the 2880×1864 panel.

**This proves what unit Ghostty reports.** Ghostty's `CSI 14 t` returned a window of
**2482×851**. The logical desktop is only 1710 points wide, so 2482 cannot be points — it is
device (backing) pixels. The mission brief's premise is confirmed by construction, not assumed.

**Scaling levers** (probes 1–3), window 800×600, page laid out at CSS 800×600:

| Lever | OSR bitmap | `innerWidth` | `devicePixelRatio` | Content rendered? |
|---|---|---|---|---|
| baseline | 800×600 | 800 | 1 | yes |
| `setZoomFactor(2)` | **800×600** | **400** | **2** | **yes** |
| `enableDeviceEmulation({dsf:2})` | 800×600 | 800 | 2 | **NO — all white, 0 ink pixels** |
| …after a full reload | 800×600 | 800 | **1 (override dropped)** | yes, but unscaled |
| `offscreen.deviceScaleFactor:2` | **1600×1200** | 533* | 3* | yes |
| `offscreen.deviceScaleFactor:1.5` | **1200×900** | 533* | 2.25* | yes |

\* Those two rows are contaminated by a persisted zoom of 1.5 — see §5.3, which turned out to be
an important finding in its own right. The bitmap sizes are unaffected and were read from the
`paint` event's full-surface damage rect (1600×1200 and 1200×900 respectively).

**Glyph rasterisation under zoom** (probe 2): a 16px Helvetica run in a 380×24 CSS box covered
**911** ink pixels at `S=1` and **3577** at `S=2` — a ratio of 3.93, i.e. `S²`. The text is
re-rasterised at the higher resolution; nothing is being upscaled. Darkest luminance stayed 0 at
both scales, so glyph cores remain solid black rather than being washed out by resampling.

---

## 2. There are three pixel spaces, and only one of them is ours

Almost every bug in this area comes from conflating these.

**Terminal-reported pixels.** Whatever `CSI 14 t` / `CSI 16 t` / `TIOCGWINSZ` hand back. A04
established — and this is the single most dangerous fact in the whole area — that the convention
is *per terminal and per source*: Ghostty reports device pixels in both `TIOCGWINSZ` and
`CSI 14 t`; iTerm2 reports device pixels in `TIOCGWINSZ` but **points** in `CSI 14 t`; Apple
Terminal reports points in both. A04's rule stands and this document depends on it: **never mix a
`TIOCGWINSZ` value with a `CSI 14 t` value in one calculation.**

**Device (backing) pixels.** What the GPU actually puts on the panel. Related to reported pixels
by a factor `R` (§4.1) that is 1 for Ghostty and 2 for a points-reporting terminal on a 2×
display.

**CSS pixels.** What the page lays out in. Related to device pixels by our scale `S`.

The pleasant surprise is that **we never need to know which convention a terminal uses in order to
get the text size right**, because the numerator and denominator of the scale formula both come
from the same source. A terminal that halves every number it reports also halves the cell size it
reports, and the ratio is invariant. `R` only becomes necessary for *sharpness* — deciding how
many real pixels to put in the image — not for *size*. §4.1 shows the formula returning an
identical answer for iTerm2 through both the ioctl path and the CSI path.

---

## 3. Why `S = 1` is the wrong default, quantified

On this machine the terminal's character cell is 17×37 device pixels. A monospace advance width
is very close to 0.6 em across the fonts people actually use (Menlo 0.602, SF Mono 0.60,
JetBrains Mono 0.60, DejaVu Sans Mono 0.602; Consolas at 0.55 is the outlier), so the terminal's
font has an em of roughly `17 / 0.6 ≈ 28.3` device pixels.

At `S = 1`, Chromium's default 16 CSS px body text is 16 device pixels tall — **57% of the
terminal's own text**, and, because this display has a 2× backing scale factor, exactly **half**
the physical size the same paragraph would have in a normal browser window on the same screen.
That is the "microscopic" the mission describes, and it is not a matter of taste: it is a
factor-of-two error introduced by treating a backing pixel as a CSS pixel.

At `S = 1.75`, body text has an em of 28 device pixels against the terminal's 28.3 — parity. The
user reads web text at the same size as their shell text, which is the size they already chose
deliberately.

The corollary matters for the API design: **the scale must not be a constant.** Someone running a
9pt font in a dense terminal wants a dense page; someone running 18pt wants a large one. The cell
size is a direct, free, always-available measurement of that preference, expressed in exactly the
units we are about to draw in.

---

## 4. The formula

### 4.1 Inputs and the reported-pixel ratio `R`

```
cols, rows        := TIOCGWINSZ.ws_col, ws_row                    # always trustworthy
cell_w, cell_h    := CSI 16 t reply                               # preferred
                     else (ws_xpixel/cols, ws_ypixel/rows)        # same-source fallback
                     else (8, 16)                                 # last resort
status_rows       := 1                                            # current UI
```

`R` is device pixels per reported pixel. When both the ioctl-derived cell and the `CSI 16 t` cell
are available, their ratio reveals it for free — no extra round trip, no new probe:

```
R := clamp(round((ws_xpixel / cols) / cell_w_csi), 1, 3)     if both known
   := 1                                                      otherwise
```

Checked against the measured matrix:

| Terminal | ioctl cell | `CSI 16 t` cell | `R` | `S_auto = cell_w·R/9.6` | snapped |
|---|---|---|---|---|---|
| Ghostty 1.3.1 | 17×37 (device) | 17×37 (device) | 1 | 1.77 | **1.75** |
| iTerm2 3.6.9 | 14×34 (device) | ~7×17 (points) | 2 | 1.46 | **1.5** |
| Apple Terminal 465 | 7×15 (points) | *no reply* | 1 | 0.73 → clamped | **1.0** |

iTerm2 is the interesting row: `14 × 1 / 9.6 = 1.46` via the ioctl path and `7 × 2 / 9.6 = 1.46`
via the CSI path. **The formula is invariant to which consistent source you pick.** That is worth
a unit test (§9). iTerm2's numbers come from A04's measurements; its `CSI 16 t` cell is
interpolated from its `CSI 14 t` points and is marked `[UNVERIFIED]` — automation there is
TCC-blocked, as the mission brief notes.

Apple Terminal clamping to 1.0 is correct: it reports points, so its numbers are already in a
1×-equivalent space, and it has no graphics protocol anyway.

### 4.2 Pixel budget

The image must be an exact whole number of cells. Deriving it from `cols × cell_w` rather than
from the raw window width matters on terminals with padding — Ghostty's `CSI 14 t` happens to
return the grid exactly (146 × 17 = 2482, 23 × 37 = 851), but Apple Terminal's window includes a
20pt/17pt inset, and sending an image wider than the grid makes the terminal clip or rescale it
while every mouse coordinate quietly drifts.

```
page_w := cols * cell_w * R
page_h := (rows - status_rows) * cell_h * R
```

For Ghostty today: `146 × 17 × 1 = 2482` and `(23-1) × 37 × 1 = 814` — which is exactly the
2482×814 frame already verified end-to-end. **This formula reproduces current behaviour on the
verified path**, so adopting it is not a regression risk; it only changes behaviour on terminals
where the window is not an exact multiple of the cell.

### 4.3 Scale

```
LADDER  := [1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0]

em_term := (cell_w * R) / 0.6          # terminal font em, in device px
S_auto  := em_term / 16                # 16 = Chromium default font size
        == (cell_w * R) / 9.6

S := snap_to_nearest(clamp(S_user ?? S_auto, 1.0, 3.0), LADDER)
```

Snapping to a ladder rather than using the raw float is deliberate: it makes the value legible in
`doctor` output and bug reports, it puts automatic scale and user `Ctrl +/-` steps on the same
scale so they compose without drift, and it keeps layout stable enough for screenshot-based CI.

The floor of 1.0 exists because scaling *down* would mean laying out more CSS pixels than we have
device pixels, which Chromium would then have to downsample — strictly worse than just having a
wide viewport. The ceiling of 3.0 is where a 2482px-wide terminal drops to an 827px CSS viewport
and most sites switch to tablet layouts.

`cell_h` is deliberately *not* used in the primary estimator. Advance width is a tight multiple of
em across monospace fonts, whereas cell height varies from 1.15 to 1.4 em depending on the font's
line gap and any `adjust-cell-height` the user has configured. Using height would make the scale
jump when a user changed line spacing, which has nothing to do with text size. As a sanity bound
only, `cell_h·R/19.2` should agree within one ladder step; the two estimators agree on all three
measured terminals (Ghostty 1.77 vs 1.93, iTerm2 1.46 vs 1.77, Apple 0.73 vs 0.78).

### 4.4 Resulting CSS viewport

```
css_w := page_w / S          # 2482 / 1.75 = 1418
css_h := page_h / S          #  814 / 1.75 =  465
```

A 1418×465 CSS viewport is an ordinary — if short — laptop browser window. Height is the scarce
resource in a terminal, not width, and the status bar costs `cell_h / S ≈ 21` CSS px of it.

Optional policy knob, not a default: if `css_w` falls below ~500 the site will serve its mobile
layout. That is often *desirable* in a narrow terminal (single-column, larger text), so it should
be a `--layout=auto|desktop|mobile` switch rather than a silent clamp. `desktop` lowers `S` until
`css_w ≥ 1024`; `mobile` additionally needs a mobile UA string, which is out of scope here.

---

## 5. Which Chromium lever, and why

Three levers exist. All three were measured. Only one is usable.

### 5.1 `setZoomFactor` — the choice

`win.setSize(page_w, page_h)` then `webContents.setZoomFactor(S)`.

The bitmap comes back at exactly `page_w × page_h` and the page lays out at `page_w/S` CSS px
wide with `devicePixelRatio == S`. Both goals are met exactly, and critically the window size, the
bitmap size, the frame header's width/height fields, the damage rect and the terminal's pixel
grid all remain **the same number**. `S` exists only inside Chromium's layout engine. Nothing in
`crates/` or `apps/cli/` needs to learn about it.

Page zoom is a layout-and-raster operation, not a bitmap scale — confirmed by the `S²` ink
measurement in §1. Text at `S=2` is rasterised at 2× with full hinting and grayscale AA.

One fidelity wart, benign: `window.screen.width` reports the device width (800 at zoom 2 in probe
1) where a true 2×-DPR browser would report the CSS width. Sites overwhelmingly lay out from
`innerWidth` and media queries, both of which are correct; a site reading `screen.width` merely
concludes the display is larger than the window, which is the normal case anyway.

### 5.2 `enableDeviceEmulation` — rejected, it breaks OSR

Enabling `{deviceScaleFactor: 2}` set `devicePixelRatio` to 2 and then produced **a completely
white frame** — 0 ink pixels out of 37289 scanned, minimum luminance 255 — with a 1.5-second
settle and a forced `invalidate()`. A new paint genuinely arrived and it was blank. Worse, after a
subsequent `loadURL` the override was **silently dropped**: `devicePixelRatio` fell back to 1 and
content rendered again, unscaled. A `setSize` kick did not restore it either.

This is a hard rejection. Anything that can present a blank page to the user while reporting
success has no place on this path.

### 5.3 `offscreen.deviceScaleFactor` — real, but construction-only

This experimental `webPreferences` option (`electron.d.ts:22577-22583`, default 1) **does** work:
requesting 2 produced a 1600×1200 bitmap from an 800×600 window, and 1.5 produced 1200×900. It
also answers the question B02 §4.1 left open — the damage rect reported 1600×1200, i.e. **the
`dirtyRect` is in device pixels, not CSS pixels**, so crops can be taken directly against the
bitmap with no DPR correction. It composes multiplicatively with zoom (dsf 2 × zoom 1.5 →
`devicePixelRatio` 3).

It is nonetheless the wrong primary lever, for two reasons. It is fixed at `BrowserWindow`
construction, so changing scale — which happens whenever the user changes terminal font size or
drags the window to a different-DPI monitor — would mean building a new window and losing the
page, its history, its scroll position and any typed form state. And it decouples window size from
bitmap size, reintroducing exactly the two-unit-systems hazard that A04 warns about, into the
frame header, the resize command and the damage path simultaneously.

Keep it in reserve for a future non-interactive `blackglass shot --dpr=2` mode, where page state
does not need to survive and true DPR semantics are worth having.

### 5.4 The zoom persistence trap — must be handled

Chromium stores page zoom **per origin, in the on-disk session, and it leaks**. Two independent
observations:

A `setZoomFactor(1.5)` left behind by a probe run that crashed **reappeared in the next, separate
process** — a fresh Electron launch opened the same `data:` URL already at `devicePixelRatio` 1.5.
And in probe 4, a brand-new `BrowserWindow` created in the same session inherited zoom 2
(`innerWidth` 400, `getZoomFactor()` 2) without anyone setting it.

Our `S` is a function of terminal geometry, not a user preference about a website, so it must
never be persisted per-origin. Two requirements follow.

**BlackGlass owns the zoom map.** Model the effective value as
`zoom_total = S × user_site_zoom`, keep `user_site_zoom` in our own profile store (B09 owns
profile data), and treat Chromium's zoom store as scratch that we overwrite.

**Re-assert on navigation.** Call `setZoomFactor(zoom_total)` from `did-navigate` and
`did-frame-navigate`. This is cheap, it makes the value deterministic regardless of what was on
disk, and it also covers the cross-origin reset case. Probe 2 showed zoom surviving a
`data:` → `about:blank` → `data:` round trip, but that is same-ish-origin; a genuine cross-site
navigation is keyed differently and should not be relied on.

### 5.5 Input coordinates need no change at all

This was the highest-risk unknown, because getting it wrong is silent. With the window at 800×600
and zoom 2 (CSS viewport 400×300), a 40×40 target at CSS (300,200) sits at device (600,400):

```
sendInputEvent(mouseDown, x=640, y=440)   ->  hit #target,  page saw clientX 320, clientY 220
sendInputEvent(mouseDown, x=320, y=220)   ->  hit #decoy    (the 2x-offset failure mode)
```

**`sendInputEvent` takes device pixels — the same units as the OSR bitmap and the terminal's own
pixel grid — and Chromium divides by the zoom factor itself.** `PointerMap`
(`/Users/adeebbashir/projects/blackglass/apps/cli/src/main.rs:701`) is therefore already correct
and must **not** be given an `S` divisor. Adding one would reintroduce precisely the bug the
second line above demonstrates.

---

## 6. Resize

**Prefer in-band reports.** A04 §5.5 documents Ghostty's DEC private mode 2048: enable with
`ESC [ ? 2048 h` and the terminal pushes `ESC [ 48 ; rows ; cols ; height_px ; width_px t` on
every resize. Rows, columns and pixels arrive atomically in one message, which removes both the
round trip and the `SIGWINCH`/`TIOCGWINSZ` race. Keep `SIGWINCH` + `TIOCGWINSZ` as the fallback.

**Do not re-query `CSI 16 t` on every resize.** The cell size does not change when a window is
dragged; it changes only on a font-size change or a move to a different-DPI display. Re-querying
mid-session is also actively hazardous: the reply lands in the same byte stream the input decoder
is reading, so it must be recognised and consumed out of band or it will be mis-parsed as user
input. Instead derive the cell from the resize report or the ioctl on each event, and only fire a
real `CSI 16 t` query when the derived cell has moved by more than ~10%, which indicates a genuine
font or DPI change.

**Recompute `S` only when the cell changes.** A plain resize changes `page_w`/`page_h` and
therefore the CSS viewport, but not the scale.

**Debounce, trailing edge, ~100 ms.** A drag-resize emits a continuous stream of events, and each
one costs a full-surface repaint — 8,081,392 pixel bytes at the measured 2482×814. At 60 Hz that
is roughly half a gigabyte per second of work the user will never see. Coalesce to the last event.

**Drop stale frames by header, not by expectation.** After `setSize`, frames already in flight
still carry the *old* dimensions in the frame header
(`/Users/adeebbashir/projects/blackglass/apps/engine/src/main.js:89-90`). The core must read the
width and height out of the header and discard any frame whose dimensions do not match the
currently requested geometry, rather than assuming or — worse — rescaling. This needs no wire
change: the fields are already there. Holding the last good frame until a correctly-sized one
arrives is preferable to blitting a mismatched one.

The engine side already does the right thing: `main.js:233-236` clamps to `Math.max(1, …)` and
calls `invalidate()` to force a repaint rather than waiting for the page to change on its own.

**Cap the budget.** 2482×814 is ~8 MB per frame. A fullscreen 4K terminal would be ~30 MB per
frame, which is a scheduler and IPC problem rather than a scaling one — B07 and B05 own it, but
the cap belongs in the same code that computes `page_w`/`page_h`, so it is named here.

---

## 7. Colour

### 7.1 What Chromium actually hands us — measured

The concern going in was specific and well-founded: Chromium normally rasterises into the
*display's* colour space, and this display is not sRGB. `screen.getPrimaryDisplay().colorSpace`
reports **BT.2020 primaries at 10 bits per component**. If the OSR buffer inherited that, sRGB red
would arrive as roughly (220,110,60) in BT.2020 encoding, we would blit those bytes to a terminal
that assumes sRGB, and every page would look desaturated and wrong — with no error anywhere.

It does not happen. Sampled directly out of `image.toBitmap()`:

| CSS colour | measured RGBA |
|---|---|
| `#ff0000` | (255, 0, 0, 255) |
| `#00ff00` | (0, 255, 0, 255) |
| `#0000ff` | (0, 0, 255, 255) |
| `#008000` | (0, 128, 0, 255) |
| `#808080` | (128, 128, 128, 255) |
| `color(display-p3 1 0 0)` | (255, 0, 0, 255) |

**The OSR bitmap is sRGB, byte-exact, on a BT.2020 10-bit display.** `matchMedia('(color-gamut:
p3)')` also returns false, so Chromium is consistently telling the page it is drawing to sRGB.
Wide-gamut content is *clipped* into sRGB rather than gamut-mapped — P3 red is indistinguishable
from sRGB red — which loses a little saturation on wide-gamut media but is exactly the right
behaviour for an sRGB destination.

Running the identical probe with `--force-color-profile=srgb` produced **byte-identical results**
in every sample. **Set the switch anyway.** It costs nothing measurable, it documents the
assumption in the code, and it pins the behaviour on hosts where a display ICC profile might
otherwise leak into the raster path. Treat it as an assertion, not an optimisation.

### 7.2 Antialiasing

Scanning a black-on-white 16px text strip for subpixel colour fringing found **0 fringed pixels
out of 4816** (channel deltas > 6), with darkest luminance 0 confirming real glyphs were present.
Electron OSR already uses grayscale antialiasing, so `--disable-lcd-text` is unnecessary today.

It should still be asserted in CI. If a future Electron upgrade turned LCD text on, the RGB
fringes would be invisible on the kitty path at 1:1 but would corrupt the half-block backend
(which point-samples one pixel per cell and would pick up the fringe colour) and would defeat
sixel palette quantisation. A cheap regression test catches a change nobody would otherwise
notice.

### 7.3 Alpha

Every sample returned `a = 255`, including `about:blank`, which came back opaque white rather than
transparent. With the default `transparent: false` the OSR surface is fully opaque.

That justifies a concrete win: kitty format `f=24` (RGB) may be used instead of `f=32` (RGBA),
**saving 25% of pre-compression wire bytes**, and the BGRA→RGB conversion the core already
performs can simply drop the alpha byte with no correctness loss. This holds only while the window
is opaque; a future transparent mode — blending the page into the user's terminal background,
which would be a genuinely attractive feature — must revisit it.

### 7.4 Per-backend colour handling

**Kitty graphics** carries 24-bit RGB directly, and neither kitty nor Ghostty applies colour
management to transmitted images. sRGB in, sRGB shown. Nothing to do — this is the verified path.

**Sixel** encodes colour as three integers in the range **0–100**, not 0–255, so there is a
~6.6-bit-per-channel quantisation before any palette reduction, on top of a colour register limit
that A04 measured as the binding constraint. A04's conclusion that sixel buys us nothing on the
current target set stands; if it is ever implemented, it needs error-diffusion dithering, and the
dithering must run in linear light (see below).

**Unicode half-block** emits two truecolor SGR values per cell. Truecolor support is currently
inferred from `COLORTERM`
(`/Users/adeebbashir/projects/blackglass/crates/bg-term/src/caps.rs:137`), which is a heuristic
rather than a handshake — consistent with how that module honestly labels its one other heuristic.
Without truecolor, output must be quantised to the 256-colour cube.

### 7.5 The gamma trap in the half-block downsampler

`render_half_blocks` (`/Users/adeebbashir/projects/blackglass/crates/bg-term/src/unicode.rs:22-27`)
currently **point-samples** — nearest neighbour, one source pixel per output sample. There is no
gamma bug today because there is no averaging.

There is however a severe aliasing problem, and the obvious fix contains a trap worth writing down
before someone walks into it. Downsampling 2482×814 to 146×44 samples discards 99.7% of the image;
thin text strokes and 1px borders appear or vanish depending on where the sample lands. The
correct fix is box-filter area averaging — and **that averaging must be done in linear light**:
decode sRGB → linear, average, re-encode. Averaging sRGB-encoded bytes directly is the classic
error and produces results that are systematically too dark, most visibly on text, which is
exactly the content that is already struggling at this resolution.

### 7.6 Dark mode — a recommendation, not yet measured

Terminals are usually dark; a default-white page in a dark terminal is a flashbang. The terminal
can be asked directly: `ESC ] 11 ; ? BEL` returns the background colour as
`ESC ] 11 ; rgb:RRRR/GGGG/BBBB` (BEL- or ST-terminated). Compute relative luminance and, below
~0.5, set `nativeTheme.themeSource = 'dark'` so `prefers-color-scheme: dark` reaches the page.
Fall back to `$COLORFGBG`, then to dark.

This is a design recommendation, **`[UNVERIFIED]` on this machine** — OSC 11 needs a real tty and
the probes here ran without one. It fits the existing query/response capability pattern in
`caps.rs` and should be measured there when a tty harness exists. It also composes with the mode
2048 work in §6, since both are terminal-side queries.

---

## 8. The half-block backend deserves honesty, not a bigger zoom

At a 17×37 cell the half-block renderer produces `cols × rows*2` = 146×44 samples for the whole
page — each sample covering 17×18.5 device pixels. For a 16 CSS px glyph to occupy even 12 samples
of height, `S` would have to be about 13.9, leaving a CSS viewport 179 pixels wide. That is not a
browser.

No scale factor fixes this, and the spec should not pretend otherwise. `S` should be clamped
normally for this backend so that layout, colour blocks and page structure remain readable — which
is genuinely useful for "did the page load, where is the button" — and the product answer for
*reading* is a DOM-text reader mode rather than pixels. `doctor` already tells the user plainly
that body text will not be legible here, which is the right posture; this document just supplies
the arithmetic behind that sentence.

---

## 9. Changes required in files I do not own

Per the file-ownership rule, these are described rather than made. All are in the commander's
files.

**`apps/engine/src/main.js`** — accept a `scale` field on the existing `resize` command and on the
launch arguments, and apply it as `win.webContents.setZoomFactor(scale * userSiteZoom)`. Re-assert
the same call from `did-navigate` and `did-frame-navigate` (§5.4). Add
`app.commandLine.appendSwitch('force-color-profile', 'srgb')` before `app.whenReady()` (§7.1).
Report `screen.getPrimaryDisplay().scaleFactor` in the existing `ready` event — it is a useful
cross-check against the inferred `R`, and free.

**`apps/cli/src/main.rs`** — replace `page_w = vp_w` / `page_h = vp_h - cell_h` (around `:244-254`)
with the cell-aligned budget of §4.2, add the `R` and `S` computation of §4.1/§4.3, add a `--scale`
override, and drop frames whose header dimensions do not match the current geometry (§6). Show `R`,
`S` and the resulting CSS viewport in `doctor` output. **Do not** apply `S` to `PointerMap` (§5.5).

**`crates/bg-term/src/caps.rs`** — expose the derived `R`, and optionally the OSC 11 background
query of §7.6.

**No wire-protocol change is required** for any of this.

## 10. Suggested CI assertions

The valuable property of everything above is that it is all assertable without a screen, which
matters given that screenshot verification is unavailable on this machine.

Pure-function tests, no Electron needed: `S` for a 17×37 device-px cell snaps to 1.75; the iTerm2
invariant that ioctl-path and CSI-path inputs yield the same `S` (`14,R=1` and `7,R=2` both give
1.46); Apple Terminal's 7×15 clamps to 1.0; `page_w`/`page_h` for 146×23 cells of 17×37 reproduce
2482×814 exactly.

Electron probe tests, headless and log-based: `setZoomFactor(2)` leaves the bitmap size unchanged
while halving `innerWidth`; `#ff0000` arrives as exactly (255,0,0,255); alpha is 255 on
`about:blank`; a black-on-white text strip contains zero subpixel-fringed pixels; and
`sendInputEvent` at a device-pixel coordinate under zoom 2 hits the element that CSS places at half
that coordinate. Appendix A is written to be lifted into that harness.

---

## Appendix A — probe sources

Condensed from the four probes actually run. Each writes JSON and exits, so it drops into CI as-is.

```js
// scale + colour, the two decisive assertions
const { app, BrowserWindow, screen } = require('electron');
app.on('window-all-closed', () => {});   // else destroying a window quits before results are written
app.commandLine.appendSwitch('force-color-profile', 'srgb');

const PAGE = 'data:text/html,' + encodeURIComponent(`<!doctype html><meta charset=utf-8>
<style>html,body{margin:0;background:#fff}
 #a{position:absolute;left:0;top:0;width:100px;height:100px;background:#ff0000}
 #t{position:absolute;left:0;top:120px;width:380px;height:24px;
    font:16px/1.25 Helvetica,Arial,sans-serif;color:#000}</style>
<div id=a></div><div id=t>Hamburgefonstiv 0123456789</div>`);

app.whenReady().then(async () => {
  const win = new BrowserWindow({ show: false, width: 800, height: 600,
    webPreferences: { offscreen: true, contextIsolation: true, sandbox: true } });
  let last = null, resolve = null;
  win.webContents.on('paint', (_e, dirty, image) => {
    last = { size: image.getSize(), dirty, bitmap: image.toBitmap() };
    if (resolve) { const r = resolve; resolve = null; r(last); }
  });
  const nextPaint = () => new Promise((res) => {
    const t = setTimeout(() => { resolve = null; res(last); }, 3000);
    resolve = (v) => { clearTimeout(t); res(v); };
    win.webContents.invalidate();
  });
  const px = (f, x, y) => { const i = (y * f.size.width + x) * 4;
    return { b: f.bitmap[i], g: f.bitmap[i+1], r: f.bitmap[i+2], a: f.bitmap[i+3] }; };

  await win.loadURL(PAGE);
  await new Promise((r) => setTimeout(r, 600));

  const base = await nextPaint();                      // expect 800x600
  const red  = px(base, 50, 50);                       // expect (255,0,0,255)

  win.webContents.setZoomFactor(2);
  await new Promise((r) => setTimeout(r, 600));
  const zoomed = await nextPaint();                    // expect STILL 800x600
  const iw = await win.webContents.executeJavaScript('innerWidth');   // expect 400
  const dpr = await win.webContents.executeJavaScript('devicePixelRatio'); // expect 2
  const red2 = px(zoomed, 100, 100);                   // expect (255,0,0,255)
  console.log(JSON.stringify({ base: base.size, red, zoomed: zoomed.size, iw, dpr, red2 }));
  app.exit(0);
});
```

```js
// the units question -- sendInputEvent under zoom. Target is at CSS (300,200)-(340,240).
// Run against a page whose #target and #decoy push {id, clientX, clientY} into window.__hits.
win.webContents.setZoomFactor(2);                       // CSS viewport 400x300, bitmap 800x600
send(640, 440);  // device px  -> hits #target, page sees clientX 320 / clientY 220   CORRECT
send(320, 220);  // CSS px     -> hits #decoy                                          WRONG
```

Use a fresh `app.setPath('userData', mkdtemp(...))` in any zoom test — persisted per-origin zoom
leaks between processes (§5.4) and will silently contaminate results otherwise.
