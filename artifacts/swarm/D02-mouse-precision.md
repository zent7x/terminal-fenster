# D02 — Mouse Precision: End-to-End Audit

**Mission:** audit mouse handling from `input.rs` decode → `apps/cli` coordinate mapping → engine
`sendInputEvent`. Establish SGR-Pixels (mode 1016) semantics. Specify drag, double-click, context
menu, and cursor shape reporting.

**Date:** 2026-07-31 · **Host:** macOS 26.1, Apple M4 · **Engine:** Electron 43.2.0 / Chromium
150.0.7871.129 · **Baseline:** `cargo test` = 96 passing (70 `bg-term`, 14 `blackglass`, 12 `bg-proto`)

**Ownership:** this document is the only file D02 wrote. Every fix below is *described*, not applied.
`crates/`, `apps/cli/`, and `apps/engine/src/main.js` belong to the commander.

---

## 0. Evidence classes used

| Class | Meaning |
|---|---|
| **SRC** | Read from the upstream implementation's source, quoted. |
| **PROBE** | Executed against real Chromium 150 on this machine today. Raw output reproduced. |
| **REPRO** | Our own decode logic extracted verbatim and executed against a table of real `Cb` values. |
| **MEAS** | Numbers measured on this machine (this repo's prior missions or `TIOCGWINSZ`/DECRQM replies). |
| **UNVERIFIED** | Stated, not proven. Called out explicitly. |

Two probe harnesses were built in the scratchpad (never in the repo) and driven the same Electron
binary and the *same* `webPreferences` as `apps/engine/src/main.js`:
`/private/tmp/claude-501/-Users-adeebbashir-projects/a6555dd0-1471-4951-aa0d-5958b606ca83/scratchpad/probe-main.js`
and `.../probe2.js`. Chromium children require `dangerouslyDisableSandbox` under the agent Bash
sandbox, as the brief predicted.

---

## 1. The pipeline as it exists today

```
Ghostty  --CSI <Cb;Px;Py M/m-->  input.rs:step_csi:277
                                 input.rs:decode_sgr_mouse:344   -> Event::Mouse{kind,button,x,y,mods}
                                       |
                     apps/cli/src/main.rs:handle_event:608
                     apps/cli/src/main.rs:PointerMap::to_page:712  -> page-pixel (px,py)
                                       |
                     JSON over unix socket  {"t":"input","kind":"mouse",...}
                                       |
                     apps/engine/src/main.js:handleInput:152      -> wc.sendInputEvent({...})
```

Four coordinate spaces exist and only three are named anywhere in the code: terminal-report space,
text-area pixel space, page/CSS pixel space, and Chromium view space. The audit below is mostly about
the seams between them.

---

## 2. SGR-Pixels (mode 1016) semantics — answered

### 2.1 Are coordinates 1-based?

**No. Under 1016 they are 0-based. Under 1006 they are 1-based.** Both independent implementations
agree, and the asymmetry is deliberate, not a bug in either.

**SRC — kitty `kitty/mouse.c`:**

```c
int x = mpos->cell_x + 1, y = mpos->cell_y + 1;   /* 1-based, SGR_PROTOCOL */
...
case SGR_PIXEL_PROTOCOL:
    x = (int)round(mpos->global_x);               /* no +1 */
    y = (int)round(mpos->global_y);
```

**SRC — Ghostty `src/input/mouse_encode.zig`:**

```zig
.sgr => try writer.print("\x1B[<{d};{d};{d}{c}", .{ button_code, cell.x + 1, cell.y + 1, ... }),

.sgr_pixels => {
    const pixels = posToPixels(event.pos, opts.size);
    try writer.print("\x1B[<{d};{d};{d}{c}", .{ button_code, pixels.x, pixels.y, ... });
},
```

`PointerMap::to_page` (`apps/cli/src/main.rs:712`) already models this correctly: pass-through in
pixel mode, `saturating_sub(1)` in cell mode. The regression test at `main.rs:965` guards it. **This
part of the mapping is right and should not be changed.**

### 2.2 Are they physical or logical pixels?

**Terminal-dependent, and nothing in BlackGlass accounts for it.**

Ghostty reports **device pixels**. Derivation from MEAS on this machine: cell = 17×37, grid =
146×23, `CSI 14t` = 2482×851, and 146×17 = 2482, 23×37 = 851 exactly. A 17-*point* cell at 2×
backing scale would be a 34-device-px glyph advance — a ~24pt font — and a 2482-point-wide window
exceeds this display's 1512-point width. A 17-*device*-px cell (≈8.5pt advance) is an ordinary
11–12pt font, and 2482 device px = 1241 points fits. Therefore Ghostty's 1016 coordinates, its
`CSI 16t` cell size, and its `CSI 14t` window size are all in **device pixels**.

Apple Terminal reports **logical points**. MEAS: `TIOCGWINSZ` 840×450 at 120×30 → 7.0×15.0 per cell,
which A06 §3.6 already identified as points (device cell = 14×30 at 2×).

Consequence: the same page rendered under the two terminals gets viewports differing by a factor of
the backing scale, and `Session::start` hands whichever number it got straight to Chromium as the
window size with `deviceScaleFactor` left at its default of 1. See M-15.

### 2.3 What Ghostty and kitty actually send — full semantics table

| Property | 1006 (cells) | 1016 (pixels) | Evidence |
|---|---|---|---|
| Origin | 1-based | **0-based** | SRC, both impls |
| Reference frame | grid | text-area top-left, **padding excluded** | SRC (`mouse_x - g->left`; `.convert(.terminal, size)`) |
| Clamping | clamped to grid | **unclamped; may be negative or exceed the grid** | SRC — Ghostty `posToPixels` has no clamp |
| Rounding | n/a | `round()` to nearest int | SRC, both impls |
| Motion dedup | only on cell change | **none — every sample reported** | SRC: kitty `if (mouse_cell_changed \|\| protocol == SGR_PIXEL_PROTOCOL)`; Ghostty `if (action == .motion and opts.format != .sgr_pixels) { dedup }` |
| `Cb` range | ≤ 255 | **may exceed 255** (kitty `LEAVE_INDICATOR (1 << 8)` = 256) | SRC kitty |

`Cb` composition is identical in both implementations and matches `input.rs`'s modifier bits:
`+4` shift, `+8` alt/meta, `+16` ctrl, `+32` motion, base `0/1/2` = left/middle/right,
`64..67` = wheel up/down/left/right, `128..131` = buttons 8–11, `3` = no button.

**Support (MEAS, from A06 + `caps.rs` tests):** Ghostty 1.3.1 yes; iTerm2 3.6.9 replies
`CSI ?1016;4$y` = *permanently reset*, never available; Apple Terminal 465 has no DECRQM at all.
`caps.rs:202 parse_decrqm_supported` handles all five DECRQM values correctly.

---

## 3. The coordinate-frame defect: which viewport is authoritative

`Capabilities::viewport_px()` (`crates/bg-term/src/caps.rs:73`) prefers `CSI 14t` over `TIOCGWINSZ`,
and its doc comment at `caps.rs:70-72` asserts *"the ioctl excludes padding"*. The measurements say
the two terminals **swap** which query means what:

| | `CSI 14t` | `TIOCGWINSZ` | grid | `cols*cell_w × rows*cell_h` | which one is the text area |
|---|---|---|---|---|---|
| Ghostty 1.3.1 | 2482×851 | 2488×858 | 146×23 @ 17×37 | **2482×851** | `CSI 14t` (ioctl adds 6×7 padding) |
| Apple Terminal 465 | 860×467 | 840×450 | 120×30 @ 7×15 | **840×450** | `TIOCGWINSZ` (`CSI 14t` adds chrome) |

860/120 = 7.1667 and 467/30 = 15.5667 — not integers, so `CSI 14t` on Apple Terminal cannot be the
text area. `viewport_px()` therefore over-reports Apple Terminal's viewport by 20×17 px. The page is
rendered 860×467 into a 840×450 area, the right/bottom strip is unreachable (max cell-mode x is
119×7+3 = 836), and the doc comment's own warning — *"Using the wrong one puts every mouse coordinate
slightly off"* — describes exactly what the code does.

**The invariant that resolves it:** 1016 coordinates are relative to the text area, and the text area
is by definition `cols × cell_w` by `rows × cell_h`. That product is the only value guaranteed to be
the frame the mouse reports live in, and it is cheaply checkable. Prefer it; use `CSI 14t` /
`TIOCGWINSZ` only to *derive* `cell_w`/`cell_h`, and log loudly when a candidate viewport is not an
exact multiple of the cell size.

---

## 4. Findings, severity-ranked

| ID | Sev | Finding | Where |
|---|---|---|---|
| M-01 | High | Extended mouse buttons (back/forward) decode as **left/middle clicks on the page** | `input.rs:353` |
| M-02 | High | `MouseEvent.buttons` is **0 during a drag** — JS drag handlers see a hover | `main.rs:626`, `main.js:168` |
| M-03 | High | Double-click is **impossible**; Blink never synthesizes it | `main.rs:621,624` |
| M-04 | High | `viewport_px()` picks the wrong frame on Apple Terminal | `caps.rs:73` |
| M-05 | Med-High | `PointerMap` never re-syncs to the frame header or to a resize | `main.rs:701`, no SIGWINCH |
| M-06 | Med | Out-of-bounds policy is asymmetric; unclamped/negative 1016 coords are dropped silently | `main.rs:723`, `input.rs:467` |
| M-07 | Med | kitty's mouse-**leave** (`Cb` bit 256) decodes as a left-button move | `input.rs:353` |
| M-08 | Med | No context-menu plumbing at all | `main.js` |
| M-09 | Med | No cursor-shape reporting, and the obvious name mapping is **inverted** | `main.js` |
| M-10 | Med | Every 1016 motion sample is forwarded — no coalescing | `main.rs:626` |
| M-11 | Low-Med | `Cb = 3` without motion synthesizes a phantom **left** click | `input.rs:366`, `main.rs:616` |
| M-12 | Low | Shift+wheel is not translated to horizontal scroll | `main.rs:629` |
| M-13 | Low | `Decoder::pixel_mouse` is set but never read — two sources of truth | `input.rs:120` |
| M-14 | Low | `mouseEnter` only latches on a move, so click-without-move never enters | `main.js:163` |
| M-15 | Low | No DPR/zoom: CSS px == terminal device px, so pages render at half physical size on Retina | `main.rs:276` |

### Verified as **correct** — do not "fix" these

* **1006 vs 1016 origin handling** in `PointerMap::to_page` — matches both upstream implementations.
* **Wheel sign convention.** `WheelUp → deltaY:+120`, `WheelDown → −120` (`main.rs:630`). The repo's
  own e2e used `deltaY:-400` to scroll *down* and passed, so negative = scroll down is confirmed.
* **`sendInputEvent` works with `show:false` offscreen rendering.** Electron's docs say the window
  must be focused, and the web is full of reports to that effect, but `tests/e2e/input-injection-results.json`
  (2026-07-31, Chrome 150.0.7871.129) has 9/9 passing including click, `:hover`, ordered typing and
  scroll. The documented caveat does not apply to this OSR configuration. No action needed.
* **Modifier bit decoding** (`input.rs:345-350`) matches kitty's `SHIFT/ALT/CONTROL_INDICATOR`.
* **The `mouseEnter`-before-first-move latch** (`main.js:163`) is genuinely required for `:hover`.

---

## 5. Detailed findings

### M-01 (High) — extended buttons become clicks on the page

`decode_sgr_mouse` (`crates/bg-term/src/input.rs:353`) extracts the base button with `btn & 3` and
tests only `btn & 64` for wheel. Buttons 8–11 (`Cb` 128–131) have neither bit set, so they fall into
the ordinary-button path with a truncated base.

**REPRO** — `decode_sgr_mouse` extracted verbatim plus the `MouseButton → &str` map from
`main.rs:612-617`, compiled with `rustc --edition 2021` and executed:

```
Cb     meaning per xterm/kitty/ghostty              our decode        -> engine JSON
128    BUTTON 8 = mouse BACK                        Down/Left         -> Down button="left"
129    BUTTON 9 = mouse FORWARD                     Down/Middle       -> Down button="middle"
130    button 10                                    Down/Right        -> Down button="right"
131    button 11                                    Down/None         -> Down button="left"
160    motion while button 8 held                   Move/Left         -> Move button="left"
288    kitty LEAVE_INDICATOR|MOTION (256+32)        Move/Left         -> Move button="left"
3      no-button / legacy release                   Down/None         -> Down button="left"
```

Pressing the thumb "back" button on a mouse **left-clicks whatever is under the pointer**. Pressing
"forward" **middle-clicks it** — in a browser that opens links in new tabs and, on Linux, pastes the
primary selection into text fields. `input.rs`'s test module covers `Cb` 0, 20, 35, 64 and 65 only
(`input.rs:548-599`); nothing above 65 is tested.

**Fix (described):** strip modifier and motion bits with `btn & !0x3C` — which preserves bits 6 and 7
so 64–67 and 128–131 survive — then match the full base value. Map 128/129 to browser back/forward
commands rather than to page clicks, and drop 130/131 (xterm's encoding above button 11 is ambiguous).

### M-02 (High) — `MouseEvent.buttons` is 0 during a drag

`handle_event`'s `MouseKind::Move` arm (`apps/cli/src/main.rs:626`) emits
`{"action":"move","x":..,"y":..}` with no button information, and `handleInput` (`main.js:168`)
forwards a bare `mouseMove`. Blink therefore reports `buttons === 0` to the page for the entire drag.

**PROBE** (`probe-main.js`, tests A and B; the page pushes `e.buttons` from a `mousemove` listener):

```
PROBE A_drag_current_move_buttons  :: [0,0]     <- exactly what BlackGlass sends today
PROBE B_drag_with_leftbuttondown   :: [1]       <- modifiers:['leftbuttondown']
```

**Scope correction — this does *not* break native text selection.** I expected it to and tested it:

```
PROBE C1_select_WITHOUT_leftbuttondown :: "alpha beta gamma delta e"
PROBE C2_select_WITH_leftbuttondown    :: "alpha beta gamma delta e"
```

Blink's `EventHandler` keeps its own pressed-state from the `mouseDown`, so selection, link dragging
and scrollbar dragging work today. What breaks is **every page that reads `event.buttons` or
`event.which` on `mousemove`/`pointermove`**: canvas drawing tools, custom sliders and range inputs,
map panning, resizable panes, and most drag-and-drop libraries. They see a hover and do nothing.

**Fix (described):** `modifiers` accepts `leftbuttondown`, `middlebuttondown`, `rightbuttondown`
(Electron `InputEvent` docs, lowercase — *not* camelCase). Track held buttons in `apps/cli`, send them
on the wire, and append them to `modifierList()` in the engine.

### M-03 (High) — double-click is impossible

`main.rs:621` and `main.rs:624` hard-code `"clickCount":1`. Chromium does **not** derive click count
from timing when the embedder supplies events; it trusts the value verbatim.

**PROBE** (`probe-main.js` D/E/E2, `probe2.js` D/E/F):

```
PROBE D_two_clickCount1_pairs        :: {"dblclick":0,"click":2,"detail":[1,1]}
PROBE E_second_pair_clickCount2      :: {"dblclick":1,"click":2,"detail":[1,2]}
PROBE E2_clickCount2_after_1500ms_gap:: {"dblclick":1}
PROBE D_doubleclick_word_select      :: "beta"
PROBE E_tripleclick_paragraph_select :: "second paragraph here for triple click test"
PROBE F_clickCount4                  :: "second paragraph here for triple click test"
```

Two rapid `clickCount:1` pairs produce **zero** `dblclick` events and leave `event.detail` at 1.
Asserting `clickCount:2` on the second pair produces exactly one `dblclick` with `detail === 2` and
selects the word. E2 is the architecturally important one: the same assertion works after a 1500 ms
gap, proving there is **no server-side time window**. BlackGlass owns 100% of the double-click policy.
`clickCount:4` behaves as 3, so clamping is safe.

Today: no `dblclick` anywhere, no double-click-to-select-word, no triple-click-to-select-paragraph.
Spec in §6.2.

### M-04 (High) — wrong authoritative viewport

Covered in §3. `caps.rs:73` prefers `CSI 14t`; on Apple Terminal that is the window including chrome,
not the text area, and the doc comment at `caps.rs:70` states the inverse of the measurement.

### M-05 (Med-High) — `PointerMap` never re-syncs

`PointerMap` is constructed once at `main.rs:269` and its `page_w`/`page_h` are fixed for the life of
the session. Meanwhile `Renderer::on_frame` (`main.rs:838-839`) *does* update its own `page_w`/`page_h`
from every frame header. Two components hold divergent copies of the same truth with no reconciliation
and no assertion. Today they happen to agree (the probe's frame geometry check and the measured
8,081,424-byte frame = 32-byte header + 2482×814×4 both confirm Chromium honours the requested size),
so this is latent rather than active — but nothing keeps it that way.

`grep -rn SIGWINCH apps crates` returns nothing, and `grep -rn resize apps/cli/src/main.rs` returns
nothing: the engine implements a `resize` command (`main.js:231`) that **no one ever sends**. After
any terminal resize, the page keeps its old size, the image is drawn at the old geometry, and every
mouse coordinate is wrong by the delta.

### M-06 (Med) — asymmetric bounds policy, and dropped out-of-range reports

`PointerMap::to_page` (`main.rs:723-726`) *rejects* `py >= page_h` but *clamps* `px` into
`[0, page_w-1]`. A pointer that leaves the window to the right therefore synthesises a real
`mouseDown` at the page's right edge, while the same excursion downward is dropped. Since Ghostty's
`posToPixels` is unclamped (SRC), out-of-range reports genuinely occur.

Negative coordinates are worse. Tracing `CSI <0;-5;100M` through the decoder: `split_params`
(`input.rs:467`) is `filter_map(parse_u32)`, `parse_u32` rejects `-`, so the parameter is **silently
removed and the remaining ones re-index**. `parts.len()` becomes 2, the `>= 3` guard at `input.rs:279`
fails, control falls through to `decode_legacy_csi`, which returns `None` for final byte `M`, and the
event is emitted as `Event::Unknown` and discarded at `main.rs:653`. No panic, no corruption — but a
silent drop, and a class of bug where a malformed parameter shifts the meaning of its neighbours.

**Fix (described):** parse signed parameters; treat any coordinate outside the text area as
`mouseLeave` (clearing hover and held buttons) rather than clamping; make `split_params` preserve
positional structure so an unparseable parameter cannot re-index the rest.

### M-07 (Med) — kitty's mouse-leave decodes as a left-button move

**SRC:** `#define LEAVE_INDICATOR (1 << 8)` in `kitty/mouse.c`. kitty emits `Cb = 256 | 32 = 288`
under 1016 only. **REPRO:** 288 → `Move / Left`. We therefore inject a `mouseMove` at the exit
coordinate instead of a `mouseLeave`, leaving `:hover` latched on whatever the pointer was last over.
Ghostty does not emit this, so the fix is kitty-specific but free: test `btn & 256` before anything
else and emit a leave.

### M-08 (Med) — no context menu

`apps/engine/src/main.js` has no `context-menu` listener. The good news is that the input side already
works:

```
PROBE F_right_downup :: {"dom_contextmenu":1,
    "webContents_context_menu":[{"x":100,"y":100,"linkURL":"","menuSourceType":"mouse","isEditable":false}]}
PROBE G_type_contextMenu_over_link :: {"dom_contextmenu":0,"webContents_context_menu":[]}
```

A right `mouseDown`+`mouseUp` pair fires both the DOM `contextmenu` event and Electron's
`webContents` `context-menu` event with full params. **`sendInputEvent({type:'contextMenu'})` fires
nothing at all** — it is in Electron's accepted `type` list but is inert here. Do not use it.

So pages that render their own menu already work; what is missing is that BlackGlass never learns a
menu was requested, and in offscreen rendering there is no native menu to fall back on. Spec in §6.3.

### M-09 (Med) — no cursor reporting, and the naive mapping is inverted

No `cursor-changed` listener exists. Chromium exposes exactly what a browser needs:

```
PROBE H_cursor_changed :: {"overLink":["hand"],"overText":["pointer"],"overProgressDiv":["progress"]}
PROBE G_cursor_over_text_then_blank :: {"overText":[],"overBlank":["pointer"]}
```

Two traps, both proven above. First, **Electron reports Chromium's `CursorType` names, which are not
CSS names, and the two most common ones are inverted**: Chromium `pointer` is the ordinary arrow
(CSS `default`), while the link hand is Chromium `hand` (CSS `pointer`). Piping Electron's string
straight into kitty's OSC 22 — whose vocabulary *is* CSS — would show a hand over ordinary content and
an arrow over links. Second, `cursor-changed` is **edge-triggered**: the `overText:[]` result is a
move between two text regions where the cursor did not change, so the last value must be cached.
Spec in §6.4.

### M-10 (Med) — no motion coalescing

Both Ghostty and kitty deliberately disable motion dedup under 1016 (SRC, §2.3). Every pointer sample
therefore becomes one `format!`, one socket write, one JSON parse and one `sendInputEvent`. At a 60–125
Hz pointer that is a steady per-sample cost competing with the 60 fps frame path measured at p50
16.65 ms. Chromium coalesces `mousemove` internally — probe A sent 8 moves and the page observed 2 —
so nothing is gained by forwarding them all. Keep the newest move per frame, exactly like the engine's
existing frame back-pressure at `main.js:64`.

### M-11 (Low-Med) — phantom left click from `Cb = 3`

`Cb = 3` means "no button". Without the motion bit, `decode_sgr_mouse` falls to
`(Down, MouseButton::None)` (**REPRO** confirms), and `main.rs:616` maps `None → "left"`. A stray
non-motion `Cb=3` therefore becomes a real left click. `MouseButton::None` should never reach the
wire as a button; it should suppress the event.

### M-12 (Low) — shift+wheel does not scroll horizontally

The platform layer normally performs this translation; `sendInputEvent` bypasses it. `main.rs:629`
always fills `deltaY`. When `mods.shift` is set on a vertical wheel, swap the axes in `apps/cli`.

### M-13 (Low) — dead `pixel_mouse` flag

`Decoder::new(pixel_mouse)` stores the flag (`input.rs:120,124`) but `decode_sgr_mouse` never reads
it; the real decision lives in `PointerMap::pixel_mode`. Two fields named for the same fact, one
inert. Remove it or make `PointerMap` the only holder, so a future edit cannot set one and not the
other.

### M-14 (Low) — enter is only latched on move

`enteredOnce` (`main.js:140`) is set inside the `move` case only. A user who clicks without first
moving never generates `mouseEnter`, so `:hover` never activates for that element. Latch on any
in-bounds mouse event. It is also a module-global; it must become per-window before tabs land.

### M-15 (Low) — no device-pixel-ratio handling

`Session::start` passes the terminal's pixel viewport straight to `BrowserWindow` and never sets
`deviceScaleFactor` or `zoomFactor`, so 1 CSS px == 1 terminal pixel. On Ghostty those are *device*
pixels (§2.2), so a 2482-px viewport triggers a desktop layout breakpoint and then draws it at half
the physical size a browser would. This does not corrupt the mouse mapping — Electron's
`sendInputEvent` x/y stay in view coordinates regardless of zoom — so `zoomFactor` is a safe lever.
Flagged here because it is the reason pointer targets feel small, which reads as a precision problem.

---

## 6. Specifications

### 6.1 Drag

State lives in `apps/cli`. The terminal's `Cb` tells us at most one held button during motion, so
maintain our own set from Down/Up transitions and use `Cb` only to corroborate.

```
held: {Left, Middle, Right}          // set on Down, cleared on Up

on Down(b): held.insert(b)
on Up(b):   held.remove(b)
on Move:    emit {"action":"move","x":..,"y":..,"held":[...]}
on FocusLost / mouseLeave / Cb&256 / out-of-bounds: held.clear()
```

Engine: extend `modifierList()` so `held` contributes `leftbuttondown` / `middlebuttondown` /
`rightbuttondown` (lowercase — Electron `InputEvent` docs). Apply to `mouseMove`, and on `mouseUp`
include the buttons still held *after* the release.

Clearing on focus loss is not optional: a button released while the pointer is outside the terminal
generates no report, and a stuck `leftbuttondown` turns every subsequent hover into a phantom drag.

Cell-mode terminals (iTerm2, Apple Terminal) get motion only on cell crossings, so a drag is a coarse
polyline. Do not interpolate synthetic intermediate points into the page — a page that samples
`mousemove` to draw would render invented strokes. Send the real vertices and expose the precision
level to page-side shims, as A06 §3.6 recommends.

### 6.2 Double- and triple-click

Blink applies no timing rule of its own (M-03, probe E2), so the entire policy is ours.

```
last: { button, at: Instant, x: u32, y: u32, count: u8 }

on Down(button, x, y):
    same = button == last.button
        && now - last.at <= T
        && |x - last.x| <= Dx
        && |y - last.y| <= Dy
    count = if same { min(last.count + 1, 3) } else { 1 }
    last = { button, at: now, x, y, count }
    emit down with clickCount = count

on Up(button, x, y):
    emit up with clickCount = last.count      // must match the Down; probe E sends 2 on both
```

**T (time).** 500 ms default. Chromium's own repeated-click window uses `kDoubleClickTimeMs` with
`kDoubleClickWidth = kDoubleClickHeight = 4`, commented *"These values match the Windows defaults"*
(`ui/events/event.cc`, `MouseEvent::IsRepeatedClickEvent`). On macOS prefer the user's setting via
`defaults read -g com.apple.mouse.doubleClickThreshold` (seconds) when readable, else 500 ms.

**D (distance).** Chromium's threshold is `kDoubleClickWidth / 2` = 2 px, which is far too tight for a
terminal pointer. Use:

| Mode | `Dx` | `Dy` | Rationale |
|---|---|---|---|
| 1016 pixel | `max(4, cell_w)` | `max(4, cell_h)` | one cell is the finest distinction the user can aim at |
| 1006 cell | same cell only | same cell only | the coordinate is constant within a cell, so this is both the strictest achievable and the loosest meaningful test |

**Reset `count` to 1** on: a different button, `T` exceeded, `D` exceeded, any wheel event, any key
press, focus loss, and navigation. Never increment on `Move`. Cap at 3 (probe F: 4 behaves as 3).

Confirmed effects once implemented: `dblclick` fires; double-click selects the word; triple-click
selects the paragraph (probes D/E in `probe2.js`).

### 6.3 Context menu

Keep sending right `mouseDown` + `mouseUp`; it already produces both the DOM `contextmenu` event and
Electron's `context-menu` event (probe F). Do **not** use `sendInputEvent({type:'contextMenu'})`
(probe G: inert).

Engine (described): add
`wc.on('context-menu', (_e, p) => sendEvent({ t:'contextmenu', x:p.x, y:p.y, linkURL:p.linkURL, srcURL:p.srcURL, mediaType:p.mediaType, isEditable:p.isEditable, selectionText:p.selectionText, editFlags:p.editFlags, menuSourceType:p.menuSourceType }))`.
Offscreen rendering has no native menu, so the CLI must draw its own overlay and send back commands
(open link, copy link address, copy, paste, back, forward, reload, view source). `selectionText` and
`linkURL` are attacker-controlled and must go through `unicode::sanitize_for_terminal` before display,
exactly as the status bar already does at `main.rs:887`.

**macOS ctrl+click.** Ghostty reports it as `Cb = 16` (left + ctrl). Chromium's platform layer
normally converts ctrl+left into a right-click on macOS, but `sendInputEvent` bypasses that layer.
**UNVERIFIED** — not probed. If confirmed, translate ctrl+Left down/up into right down/up inside
`apps/cli` under `#[cfg(target_os = "macos")]` and drop the ctrl modifier. Also note the platform
split: macOS fires the context menu on mouse-**down**, other platforms on mouse-up.

**Cmd+click has no representation.** xterm's `Cb` has no bit for super/command, so "open link in new
tab" cannot arrive over the mouse protocol at all. It needs a keyboard-side chord or a menu entry.

### 6.4 Cursor shape reporting

Engine (described): `wc.on('cursor-changed', (_e, type) => { last = type; sendEvent({t:'cursor', v:type}); })`,
caching `last` and re-emitting on request, because the event is edge-triggered (probe G).

CLI: translate Chromium `CursorType` → CSS name → OSC 22. **The first two rows are the whole point of
this table.**

| Electron `cursor-changed` | CSS / OSC 22 name | note |
|---|---|---|
| `pointer` | `default` | Chromium `pointer` is the **arrow** |
| `hand` | `pointer` | the link hand |
| `text` | `text` | |
| `crosshair`, `wait`, `progress`, `help`, `move`, `cell`, `alias`, `copy`, `not-allowed`, `grab`, `grabbing`, `zoom-in`, `zoom-out`, `vertical-text` | identity | already CSS names |
| `*-resize` (`n`,`s`,`e`,`w`,`ne`,`nw`,`se`,`sw`,`ns`,`ew`,`nesw`,`nwse`) | identity | |
| `col-resize` | `ew-resize` | not in kitty's 30-name set |
| `row-resize` | `ns-resize` | not in kitty's 30-name set |
| `nodrop` | `no-drop` | note the hyphen |
| `context-menu` | `default` | not in kitty's set |
| `none`, `null`, `custom`, `*-panning`, `drag-drop-*`, `*-no-resize` | `default` | no terminal equivalent |

Emission: `ESC ] 22 ; <name> ESC \`, only when the name changes.

**Support.** kitty ≥ 0.31.0 defines OSC 22 with CSS cursor names plus a stack (`>` push, `<` pop) and
a `?` query. Ghostty documents OSC 22 as supported since 1.0.0 using "CSS's list of standardized
cursor shapes", and Ghostty's own docs caution that there is no cross-terminal consensus — so gate
emission on a positive capability signal, not on `$TERM`. Ghostty's push/pop/query support is
**UNVERIFIED**; kitty's `?` query form is the portable probe: `ESC ] 22 ; ? ESC \`.

**Teardown.** A pointer shape left set outlives the process and follows the user into their shell —
the same class of bug `RESTORE_SEQ` already guards against for images and mouse modes. Because
push/pop is not universally available, the safe universal teardown is an explicit
`\x1b]22;default\x1b\\` appended to `RESTORE_SEQ` (`crates/bg-term/src/tty.rs:27`). **That file is
not mine to edit; the change is described here and the existing test at `tty.rs:223` should gain a
matching assertion.**

---

## 7. Runtime invariants and tests to add

Assertions worth making cheap and permanent (described, not written):

1. `cols * cell_w == viewport_w && rows * cell_h == viewport_h`. If a candidate viewport is not an
   exact multiple, it is not the text area — log it and prefer the one that factors (§3).
2. Frame-header `width/height` must equal `PointerMap.page_w/page_h`. On mismatch, re-sync the
   pointer map from the frame rather than silently diverging (M-05).
3. On `SIGWINCH`: re-detect geometry, send `{"t":"resize"}`, rebuild `PointerMap`, clear `held`.

Unit tests (`bg-term`): a table-driven decode test over `Cb ∈ {0,1,2,3,16,20,32,33,35,64,65,66,67,128,
129,130,131,160,288}` asserting kind, button and modifiers for each — this is the test that would have
caught M-01, M-07 and M-11, and its absence is why they are all still present at 96 green tests.
Also a negative-parameter test for `CSI <0;-5;100M`.

E2E (`tests/e2e/`, extending the existing harness): drag reports `buttons === 1`; two rapid clicks fire
exactly one `dblclick` with `detail === 2`; a slow pair fires none; triple-click selects a paragraph;
right-click emits a `contextmenu` engine event; hovering a link emits `cursor` = `hand`.

---

## 8. What I could not verify

* **Live mouse input on Ghostty.** The machine is at a lock screen and there is no way to generate
  real pointer events, so no byte-level capture of Ghostty's 1016 output exists. Every 1016 claim
  above is source-verified against both Ghostty and kitty rather than captured on the wire. The
  cheapest closing evidence is a one-shot capture tool that enables 1003+1006+1016, dumps raw stdin
  for ten seconds, and asserts the first report's coordinates against a known click position.
* **iTerm2 3.6.9** remains blocked by macOS TCC, as the brief states. It is moot for 1016 (DECRQM says
  permanently reset) but not for the cell-mode fallback path.
* **macOS ctrl+left → right-click** inside `sendInputEvent` (§6.3).
* **Ghostty's OSC 22 push/pop/query** support (§6.4).
* **Whether Ghostty clamps 1016 coordinates at the window edge in practice.** The source has no clamp;
  the observed behaviour at the boundary is untested.

---

## 9. Single most actionable recommendation

Introduce one `PointerState` in `apps/cli/src/main.rs` that owns held buttons, the click counter and
per-frame move coalescing, and fix `decode_sgr_mouse`'s base-button mask (`btn & !0x3C`) as its
prerequisite. M-01, M-02, M-03, M-10 and M-11 all live in the same ~40 lines of `handle_event` and the
same six lines of `decode_sgr_mouse`; one struct plus one mask closes every High-severity mouse defect,
and each part is provable by the probes already run.
