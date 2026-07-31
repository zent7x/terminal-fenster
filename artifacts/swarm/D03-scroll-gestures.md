# D03 — Scrolling, High-Resolution Deltas, Momentum and Pinch-Zoom

**Mission:** replace the crude fixed `±120` wheel delta. Design smooth scrolling, high-resolution
deltas, momentum/inertia, and pinch-zoom where the terminal can express it. Be honest about what
terminals can and cannot report. Give the exact `deltaY` values and the accumulation algorithm.

**Owned output:** this file only. Every change to `crates/`, `apps/cli/`, `apps/engine/` below is
written as an instruction for the commander, not applied. Baseline left untouched.

**Date:** 2026-07-31 · macOS 26.1 · Apple M4 · Electron 43.2.0 / Chromium 150.0.7871.129

---

## 0. TL;DR

1. **The current `±120` is not wrong — it is un-normalized.** I measured Chromium's wheel response
   directly: `deltaY` maps **1 CSS pixel per unit, exactly**, and `120` is precisely Chromium's own
   native per-notch distance. For an isolated mouse-wheel notch the existing constant is *correct*
   and must not regress. The bug is that the same 120 is emitted for every event when a trackpad or
   a terminal-side multiplier produces 3–7 events per physical gesture. (§2.1, §4.1)

2. **The sign is also already correct, and the opposite of what the DOM says.** Measured from a
   mid-scroll position so clamping could not mask direction: `deltaY:+120` moved `scrollY` 500 → 380
   (**up**), `deltaY:-120` moved 500 → 620 (**down**). The page's own `WheelEvent.deltaY` has the
   *inverted* sign (we send `-120`, the page sees `+120`). CDP's `Input.dispatchMouseEvent` is
   inverted again relative to `sendInputEvent`. Three conventions, all live in this codebase. (§2.2)

3. **Terminals cannot report magnitude, only direction — so we must infer rate, not distance.**
   SGR mouse encodes wheel as buttons 64–67 with no magnitude field and no release event. Kitty's
   own documentation is explicit that its per-pixel scrolling "does not affect applications running
   inside the terminal". Terminals express magnitude by **sending more events**, which is why
   Ghostty ships `mouse-scroll-multiplier = precision:1,discrete:3` and why users report ~7 events
   per physical notch. Our delta must therefore be a function of inter-event *timing*. (§1, §3)

4. **Two whole design directions are closed, and one of them crashes the browser.**
   `Input.synthesizeScrollGesture` and `Input.synthesizePinchGesture` — the CDP APIs that would give
   real fling physics and real pinch — **SIGSEGV the entire engine**, reproduced 5/5 across every
   variant (default, `gestureSourceType: mouse`, `gestureSourceType: touch`, `preventFling: true`).
   All `gesture*` event types throw `Invalid event object` from `sendInputEvent`. Do not spend a day
   on either. (§3)

5. **Do not build a momentum engine — build a momentum *interpreter*.** macOS already generates the
   inertial tail and the terminal already forwards it as a decaying stream of wheel events. The
   naive rate-adaptive design has a subtle failure here: as momentum decays the gap *grows*, so a
   rate-adaptive step *grows*, producing a jerk at the exact moment the scroll should be settling.
   §5 specifies the monotonic-decay clamp that fixes it. (§5.3)

6. **Pinch-zoom is not expressible; ctrl+wheel is.** No terminal reports pinch gestures to
   applications. Worse, page-scale zoom is unreachable anyway: `Emulation.setPageScaleFactor` is a
   silent no-op in OSR and `visualViewport.scale` never leaves 1.0. The achievable feature is
   **reflow zoom** via `setZoomFactor`, which I verified works (2.0 → `innerWidth` 800→400). (§6)

7. **The entire scroll fix lands in one file.** Because the correct choice is to *keep* Chromium's
   built-in ease-out animation (§4.3), no engine change is needed for scrolling at all —
   `apps/cli/src/main.rs:629-640` is the whole blast radius. Zoom needs one new engine command. (§8)

---

## 1. What terminals can and cannot report

This is the constraint that shapes everything else, so it goes first and it is stated honestly.

### 1.1 The wire has no magnitude field

Under SGR mouse reporting (mode 1006), a wheel event is `CSI < Cb ; Cx ; Cy M` where `Cb` carries
the `SCROLL_BUTTON_INDICATOR` bit (64):

| `Cb` | meaning |
|-----:|---------|
| 64 | wheel up |
| 65 | wheel down |
| 66 | wheel left / tilt |
| 67 | wheel right / tilt |

Plus modifier bits (shift 4, alt 8, ctrl 16). That is the entire vocabulary. There is:

* **no delta magnitude** — every notch looks identical on the wire;
* **no release event** — xterm's specification states release events are not reported for wheel
  buttons, confirmed in sibling artifact `A06-input-research.md:524`;
* **no phase field** — nothing distinguishes "user is actively dragging" from "OS inertia tail"
  from "gesture ended";
* **no device class** — nothing distinguishes a $10 notched mouse wheel from a Magic Trackpad.

Sibling `A06-input-research.md:490-493` and `crates/bg-term/src/input.rs:352-361` already decode
exactly this and nothing more. The decoder is correct; there is simply no more information present.

### 1.2 Terminals *have* high-resolution data and deliberately keep it

This is the part that is easy to get wrong. Modern terminals do receive sub-line, high-precision
scroll deltas from the OS — they just consume them locally for their own scrollback and hand
applications whole notches.

Kitty's configuration reference is explicit about `pixel_scroll`:

> When enabled, kitty's own scrollback will move by sub-line increments instead of only whole
> lines. This does not affect applications running inside the terminal (for example full-screen
> TUIs) that handle scrolling themselves.

Kitty 0.46's release coverage describes smooth pixel scrolling and Linux momentum scrolling as
scrollback-buffer features for the same reason. `touch_scroll_multiplier` is documented as applying
only to "high precision scrolling devices" — again, kitty-side.

**Local primary-source evidence** from the installed Ghostty binary on this machine:

```
$ /Applications/Ghostty.app/Contents/MacOS/ghostty +show-config --default --docs
# Multiplier for scrolling distance with the mouse wheel.
#
# A prefix of `precision:` or `discrete:` can be used to set the multiplier
# only for scrolling with the specific type of devices. ...
# The default value is "3" for discrete devices and "1" for precision devices.
mouse-scroll-multiplier = precision:1,discrete:3
```

Ghostty therefore **knows** whether the device is a precision trackpad or a discrete wheel, and
applies different multipliers — but the mouse protocol gives it no way to tell us. The
device-class bit exists inside the terminal and dies there.

### 1.3 Magnitude is expressed as event *count* — this is the real source of the bug

Because the wire cannot carry magnitude, a terminal that wants "one notch = 3 lines" emits **three
wheel events**. Kitty's issue tracker documents this directly (kitty#262, kitty#5502): the
multiplier causes kitty to send multiple scroll ticks to the application, which then multiplies
again with the application's own step, so scrolling is correct in the shell or in a full-screen app
but not both. Ghostty has the same class of report — users measuring with `wev` observe roughly
seven scroll events for a single physical wheel movement (ghostty#4259, ghostty#9966), and
ghostty discussion #3955 is titled, verbatim in substance, that mouse-based programs should
probably receive only a single scroll signal.

**Consequence for BlackGlass:** a fixed 120 px per received event is multiplied by an unknown,
per-terminal, per-device, user-configurable factor of roughly 1–7 before it reaches us. That is the
actual defect — not the constant's value.

> **UNVERIFIED:** I could not confirm the exact event-expansion count Ghostty 1.3.1 produces for one
> physical notch *on this machine*. The machine is at a lock screen, so no human can generate a real
> trackpad or wheel gesture, and synthetic events would not exercise the terminal's device
> classification. §9.1 gives the exact 3-minute measurement procedure to run when a human is present.
> The algorithm in §5 is designed to be **immune to this constant** (§5.2), so the number is a
> calibration nicety, not a blocker.

### 1.4 Trackpad gestures: what is genuinely unavailable

| Gesture | Reported to applications? | Evidence |
|---|---|---|
| Two-finger scroll | **Yes**, as discrete wheel buttons 64/65 | SGR spec; `input.rs:352` |
| Horizontal two-finger scroll | **Usually**, as buttons 66/67 | SGR spec; needs per-terminal check (§7) |
| Momentum / inertia tail | **Indirectly** — arrives as a decaying stream of ordinary wheel events, with no flag marking it as inertial | §5.3 |
| **Pinch to zoom** | **No.** No terminal protocol carries it | Searched; no protocol found. Terminals bind pinch to local font-size change |
| Rotate, swipe, force-touch | **No** | as above |
| Sub-line / fractional delta | **No** — explicitly withheld (§1.2) | kitty `pixel_scroll` docs |

**Bottom line:** we receive a direction and a timestamp. Everything else must be inferred.

---

## 2. Measured Chromium wheel semantics

All numbers below are from live probes against the real engine stack — Electron 43.2.0 /
Chromium 150.0.7871.129, offscreen `BrowserWindow`, `webPreferences.offscreen = true`,
`setFrameRate(60)`, `devicePixelRatio = 1`, 800×600 viewport, on a 20000 px-tall test page.
Probe sources and exact commands are in Appendix A so this is reproducible in CI.

### 2.1 `deltaY` is 1:1 with CSS pixels

Single `sendInputEvent({type:'mouseWheel', deltaY: d})` from `scrollY = 0`, settled 600 ms:

| sent `deltaY` | resulting `scrollY` (px) |
|---:|---:|
| −1 | 1 |
| −10 | 10 |
| −40 | 40 |
| −53 | 53 |
| −100 | 100 |
| −120 | 120 |
| −240 | 240 |
| −360 | 360 |

**One unit of `deltaY` is exactly one CSS pixel.** There is no scaling, no line-height conversion,
no acceleration curve applied by Chromium. `120` is not an arbitrary legacy number here: it is
Chromium's own native per-notch scroll distance (3 lines × 40 px), so an isolated notch of 120 px
is exactly what a real browser does.

Identical behaviour with `hasPreciseScrollingDeltas: true` (−1→1, −10→10, −40→40, −53→53, −120→120,
−360→360). The flag changes *animation*, not distance (§4.3).

**Fractional deltas accumulate and are not truncated.** Ten events of `deltaY: -0.4` produced
`scrollY = 4` — exactly 10 × 0.4. Sub-pixel precision is preserved end-to-end, so the CLI may emit
fractional deltas without keeping its own residual accumulator.

### 2.2 Sign convention — three different conventions in one stack

Measured **from `scrollY = 500`** specifically so that clamping at the top could not be mistaken for
a direction result (my first probe measured from 0 and was ambiguous; this supersedes it):

| action | before | after |
|---|---:|---:|
| `sendInputEvent deltaY: +120` | 500 | **380** (scrolled **up**) |
| `sendInputEvent deltaY: -120` | 500 | **620** (scrolled **down**) |

So for `webContents.sendInputEvent`: **positive `deltaY` scrolls up.**

`apps/cli/src/main.rs:630` currently reads:

```rust
let dy = if kind == MouseKind::WheelUp { 120 } else { -120 };
```

`WheelUp → +120 → scrolls up`. **This is correct. Do not "fix" it.**

Two traps sit next to it:

* **The page sees the opposite sign.** Sending `deltaY: -120` (scroll down) delivers
  `WheelEvent.deltaY = +120` to the page. Confirmed for all three probe events; `deltaMode` is
  always `0` (`DOM_DELTA_PIXEL`), including for non-precise events.
* **CDP is inverted relative to `sendInputEvent`.** `Input.dispatchMouseEvent` with
  `type:'mouseWheel', deltaY:+100` scrolled **down** to `scrollY = 100`. Anyone who reaches for CDP
  as a fallback will silently invert scrolling.

### 2.3 A single event is clamped to one viewport height

| viewport `innerHeight` | sent `deltaY` | resulting `scrollY` |
|---:|---:|---:|
| 600 | −600 | 600 |
| 600 | −601 | 600 |
| 600 | −900 | 600 |
| 600 | −6000 | 600 |
| 600 | −1200 | 600 |
| **200** | −100000 | **200** |

The cap tracks `innerHeight`, not a constant (600-px viewport caps at 600; 200-px viewport caps at
200). **Excess is silently discarded** — no error, no partial application. Any design that batches a
large accumulated delta into one event will quietly lose scroll distance. §5.5 splits oversized
deltas.

### 2.4 Chromium applies no acceleration of its own

Five events of `deltaY: -120`:

| spacing | total `scrollY` |
|---|---:|
| 16 ms apart | 600 |
| 300 ms apart | 600 |

5 × 120 = 600 in both cases. Chromium neither compounds nor accelerates rapid wheel input, and it
loses nothing when events overlap an in-flight animation. **All acceleration is ours to implement**
— and equally, accumulation correctness is free.

### 2.5 `wheelTicksX` / `wheelTicksY` are inert

| sent | `scrollY` |
|---|---:|
| `deltaY:-120, wheelTicksY:-1` | 120 |
| `deltaY:-120, wheelTicksY:-3` | 120 |
| `deltaY:0, wheelTicksY:-3` | **0** |

Scroll distance is driven **solely** by `deltaX`/`deltaY`. The tick fields do not scroll on their
own and do not scale the delta. Ignore them.

### 2.6 Pages can cancel our scroll, and receive our modifiers

A non-passive `wheel` listener calling `preventDefault()`, installed inline in the document so there
is no listener-registration race, **blocks the scroll completely**: `scrollY` stayed `0` across three
`deltaY: -120` events while the listener counter reached 3.

> A caveat worth recording because it cost me a wrong conclusion: when the same listener is
> registered *after* load via `executeJavaScript` and a wheel event is sent ~80 ms later, the scroll
> goes through anyway (observed 293 → 413 px). That is the compositor fast-path acting before
> updated listener properties reach it. The inline-listener result is the correct one. Any CI test
> asserting `preventDefault` behaviour must install the listener in the document, not after load.

Implications: we are **not** the source of truth for scroll position (never predict it locally), and
`ctrlKey` does reach the page (`ctrl: true` observed in the DOM event), so pages implementing their
own ctrl+wheel zoom — Maps, Figma, canvas apps — will work if we forward the modifier (§6.3).

Also confirmed: **one `sendInputEvent` produces exactly one DOM `wheel` event** (3 sent, 3 received),
while a single 120 px scroll emits many `scroll` events (24 across 3 wheels) because of the
animation in §4.3.

---

## 3. Closed doors — do not spend time here

### 3.1 `gesture*` events are unreachable from `sendInputEvent`

Electron's type definitions list `gestureScrollBegin`, `gestureScrollUpdate`, `gestureFlingStart`,
`gesturePinchBegin`, `gesturePinchUpdate` and more on the base `InputEvent` interface
(`node_modules/electron/electron.d.ts:8917`). That list is a red herring: `sendInputEvent` accepts
only `MouseInputEvent | MouseWheelInputEvent | KeyboardInputEvent`
(`electron.d.ts:18346`), and `MouseInputEvent` narrows `type` to
`mouseDown|mouseUp|mouseEnter|mouseLeave|contextMenu|mouseWheel|mouseMove` (`electron.d.ts:9790`).

Verified at runtime rather than inferred from typings — every one throws:

```
gesturePinchBegin    THREW: Invalid event object
gesturePinchUpdate   THREW: Invalid event object
gestureScrollBegin   THREW: Invalid event object
gestureScrollUpdate  THREW: Invalid event object
gestureFlingStart    THREW: Invalid event object
```

Scroll position and zoom factor were unchanged afterwards. **There is no gesture path through
`sendInputEvent`.**

### 3.2 CDP synthetic gestures crash the engine (SIGSEGV)

This is the most important negative result in the document. `webContents.debugger.attach('1.3')`
succeeds, and then:

| CDP command | result |
|---|---|
| `Input.dispatchMouseEvent` (`mouseWheel`) | **works** — `deltaY:+100` → `scrollY 100` |
| `Emulation.setPageScaleFactor` | **silent no-op** — `visualViewport.scale` stayed 1.0 |
| `Input.synthesizeScrollGesture` | **SIGSEGV — whole process dies** |
| `Input.synthesizePinchGesture` | **SIGSEGV — whole process dies** |

The crash is deterministic and not variant-specific. Reproduced **5/5**:

| variant | result |
|---|---|
| `synthesizeScrollGesture` default | SIGSEGV |
| `synthesizeScrollGesture` `gestureSourceType: 'mouse'` | SIGSEGV |
| `synthesizeScrollGesture` `gestureSourceType: 'touch'` | SIGSEGV |
| `synthesizeScrollGesture` `preventFling: true` | SIGSEGV |
| `synthesizePinchGesture` `scaleFactor: 2.0` | SIGSEGV |

These are exactly the APIs one would reach for to get "real" Chromium fling physics and real pinch
zoom. They take the browser process down with no JS-catchable error — in production this is a tab
crash, or worse, an engine crash that `B08-crash-recovery.md` would have to absorb on every scroll.

**Instruction for the commander:** treat `Input.synthesize*Gesture` as forbidden API in this
codebase. It is worth one line in the engine's review checklist, because it looks like the obvious
right answer and fails catastrophically.

### 3.3 Page-scale (true pinch) zoom is unreachable

`visualViewport.scale` never moved from 1.0 under any path tried: `setZoomFactor` (which is reflow
zoom, §6.1), `Emulation.setPageScaleFactor` (no-op), ctrl+wheel (§6.3), and
`synthesizePinchGesture` (crash). **Pinch-to-zoom in the visual-viewport sense is not implementable
on this stack today.** Say so in user-facing docs rather than shipping a half-working gesture.

---

## 4. Design decisions that fall out of the measurements

### 4.1 Keep 120 px as the isolated-notch step

`D_MAX = 120` px. It equals Chromium's native per-notch distance (§2.1), matches every desktop
browser, and preserves today's behaviour for a plain notched mouse wheel — so the change in §5 is a
strict improvement with **no regression on the discrete-mouse path**.

### 4.2 Normalize by *rate*, not by count

Since the wire carries no magnitude (§1.1) and terminals inflate event counts by an unknown factor
(§1.3), the only usable signal is the **inter-event interval**. The design target is therefore not
"pixels per event" but **bounded scroll velocity**: choose per-event delta `d` such that
`velocity = d / dt` saturates at a comfortable ceiling. This makes the algorithm immune to the
terminal's multiplier (§5.2) — the property that matters most, because we cannot measure it.

### 4.3 Send **non-precise** deltas and let Chromium do the smoothing

This is the smooth-scrolling decision, and the measurement makes it easy.

Sampling `scrollY` every ~16 ms after one `deltaY: -120` event:

| mode | timeline (px) |
|---|---|
| default (non-precise) | 0, 7, 16, 28, 43, 60, 77, 92, 105, 113, 118, **120** |
| `hasPreciseScrollingDeltas: true` | **120** at the first sample |

Non-precise wheel input is **animated by Chromium with an ease-out curve**; precise input is applied
**instantly**. Curves at other magnitudes (non-precise): `−40` → 0,1,2,5,9,14,20,31,35,38,39,40;
`−400` → 0,62,140,232,317,374,399,400.

> Sampling caveat: each sample includes an `executeJavaScript` round-trip, so the elapsed-time
> labels are an upper bound. Read these as "roughly 110–180 ms, ease-out" rather than as exact
> durations. The *shape* and the precise-vs-non-precise distinction are unambiguous.

**Decision: do not set `hasPreciseScrollingDeltas`.** Rationale:

* Our input is inherently discrete (§1.1). Applying a 120 px step instantly makes scrolling
  *teleport*; the ease-out is exactly the smoothing the mission asks for, for free.
* Chromium retargets rather than stacks overlapping animations — proven by §2.4, where five events
  16 ms apart summed to exactly 600 px with no loss.
* It keeps the whole fix inside `apps/cli/src/main.rs`; no engine change, no new protocol field, no
  frame-loop coupling.

Writing our own interpolator would mean re-implementing this ease-out, driving it from our frame
scheduler (`B07-frame-scheduler.md`), and fighting Chromium's animator for ownership of scroll
position — strictly worse for strictly more code. Revisit only if §9 measurement shows the ~150 ms
latency is objectionable on a real trackpad.

---

## 5. The accumulation algorithm

### 5.1 Pipeline

```
SGR wheel bytes
   └─> burst-group      (§5.2)  collapse terminal-side event expansion -> logical notches
        └─> rate model  (§5.3)  notch interval -> delta in CSS px
             └─> clamp  (§5.5)  viewport-height ceiling, axis mapping
                  └─> IPC batch (§5.6) one sendInputEvent per read cycle
```

### 5.2 Stage 1 — burst grouping (defeats the terminal multiplier)

The insight that makes this robust: terminal-side expansion (§1.3) writes N events into the pty in a
**tight loop**, so they arrive back-to-back within the same `read()` with sub-millisecond spacing.
A genuine high-rate device is paced by hardware — macOS trackpad scroll events arrive at display
cadence, roughly 8–16 ms apart. The two populations separate cleanly.

> Group consecutive same-direction wheel events into one **logical notch** when they arrive within
> `BURST_MS = 5` ms of each other **and** in the same read batch.

* Ghostty `discrete:3` emitting 3 events ~0.1 ms apart → **1 notch** → 120 px. Correct, and the
  user's `mouse-scroll-multiplier` value becomes irrelevant to us.
* A trackpad at 8–16 ms spacing → **not grouped** → handled by the rate model as genuine high-rate
  input. Correct.

`BURST_MS = 5` is the one constant that needs empirical confirmation on a real device (§9.1). It is
deliberately placed well below the 8 ms hardware floor and well above the sub-millisecond pty loop,
so it has roughly an order of magnitude of margin on each side.

### 5.3 Stage 2 — the rate model

State per axis:

```rust
struct ScrollAxis {
    dt_ema:   f64,           // ms, smoothed inter-notch interval
    last_ts:  Option<Instant>,
    last_dir: i8,            // +1 / -1 / 0
    d_prev:   f64,           // px, previously emitted step
}
```

Constants:

| name | value | meaning |
|---|---:|---|
| `D_MAX` | 120 px | isolated-notch step; Chromium-native, today's value (§4.1) |
| `D_MIN` | 16 px | floor of the shaping curve (~one text line) |
| `D_FLOOR` | 4 px | absolute floor; keeps motion visible under the velocity cap |
| `T_FULL` | 160 ms | interval at/above which a notch is "isolated" and earns `D_MAX` |
| `GAMMA` | 0.5 | curve shaping (square-root) |
| `V_MAX` | 3000 px/s | scroll-velocity ceiling |
| `T_MOMENTUM` | 100 ms | below this, treat the stream as a gesture/inertia tail |
| `RECOVER` | 1.15 | max per-event growth of the step during a tail |
| `IDLE_RESET` | 400 ms | gap after which state resets to fresh |
| `ALPHA` | 0.35 | EMA weight on the newest interval |

```rust
fn on_notch(&mut self, dir: i8, now: Instant) -> f64 {
    let dt_raw = self.last_ts.map(|t| ms(now - t)).unwrap_or(IDLE_RESET);

    // Direction reversal or a long idle gap starts a fresh gesture.
    if dir != self.last_dir || dt_raw > IDLE_RESET {
        self.dt_ema = T_FULL;
        self.d_prev = D_MAX;
    } else {
        // Smooth the interval. Essential over SSH, where arrival is bursty (§5.4).
        self.dt_ema = ALPHA * dt_raw + (1.0 - ALPHA) * self.dt_ema;
    }

    // Shaping curve: sub-linear, so faster gestures travel further but not proportionally.
    let shape = D_MIN + (D_MAX - D_MIN) * (self.dt_ema / T_FULL).min(1.0).powf(GAMMA);

    // Velocity ceiling: d/dt must not exceed V_MAX.
    let cap = (V_MAX * self.dt_ema / 1000.0).max(D_FLOOR);
    let mut d = shape.min(cap);

    // Momentum tail: while events are dense, the step may not jump upward.
    // Without this, a decaying inertia tail (dt rising) would make steps GROW and
    // produce a jerk exactly as the scroll should be settling.
    if self.dt_ema < T_MOMENTUM {
        d = d.min(self.d_prev * RECOVER);
    }

    self.d_prev  = d;
    self.last_ts = Some(now);
    self.last_dir = dir;
    d
}
```

Resulting delta and steady-state velocity:

| `dt_ema` | notches/s | **emitted delta (px)** | velocity (px/s) | typical source |
|---:|---:|---:|---:|---|
| 4 ms | 250 | **12** | 3000 | pathological burst |
| 8 ms | 125 | **24** | 3000 | hard trackpad flick |
| 16 ms | 62 | **48** | 3000 | trackpad flick |
| 25 ms | 40 | **57** | 2284 | fast trackpad drag |
| 40 ms | 25 | **68** | 1700 | trackpad drag |
| 80 ms | 12 | **90** | 1119 | slow drag / fast wheel |
| 120 ms | 8 | **106** | 884 | brisk wheel |
| ≥160 ms | ≤6 | **120** | ≤750 | **isolated wheel notch** |

The two ends are the acceptance criteria: an isolated notch is unchanged at **120 px**, and no input
rate can exceed **3000 px/s** (~3.7 viewports/second at 800 px). The middle is monotone and smooth.

**Why the momentum clamp matters.** A macOS inertia tail decays from ~8 ms spacing to ~100 ms
spacing over roughly a second. Without the clamp, `dt_ema` rising from 8→80 ms would raise the step
from 24→90 px — the scroll would *speed up per event* while the user perceives it slowing, landing
as a visible lurch at the end. With the clamp the step can only grow 15 % per event, so the emitted
distance decays smoothly along with the event rate. Above `T_MOMENTUM` the clamp lifts, so genuinely
slow deliberate scrolling immediately earns full 120 px steps rather than being throttled.

### 5.4 Why the EMA is load-bearing over SSH

Sibling artifacts `A07-ssh-remote.md` and `C09-ssh-adaptive.md` establish that BlackGlass runs over
SSH. Input arrival there is bursty: Nagle coalescing and RTT jitter deliver several notches in one
packet after a pause, so raw `dt` alternates between ~0 ms and ~200 ms for a physically *uniform*
gesture. Feeding raw `dt` into the curve would make the step oscillate between 16 px and 120 px —
visibly stuttering scroll caused entirely by the network. The EMA (`ALPHA = 0.35`, ~3-event memory)
plus burst grouping (§5.2) absorbs this. Note that burst grouping alone is *not* sufficient, because
SSH bursts can exceed `BURST_MS`; the two mechanisms cover different failure modes.

### 5.5 Stage 3 — clamping and axis mapping

```rust
// Never exceed the per-event viewport ceiling measured in §2.3; excess is silently discarded.
let ceiling = 0.9 * viewport_height_css;      // 0.9 for margin against rounding/DPR
if delta.abs() > ceiling { /* split into ceil(delta/ceiling) events */ }
```

Sign, per §2.2 (`sendInputEvent` convention, positive = up):

| input | emitted |
|---|---|
| `WheelUp` | `deltaY = +d` |
| `WheelDown` | `deltaY = -d` |
| `WheelRight` | `deltaX = +d` |
| `WheelLeft` | `deltaX = -d` |
| `Shift + WheelUp/Down` | swap to `deltaX` (§7) |

### 5.6 Stage 4 — IPC batching

Run the model **per notch** (so rates stay correct), then sum the resulting deltas produced within
one input read cycle into a **single** `sendInputEvent`. A 7-event burst becomes one IPC message
instead of seven, and §2.4 proves the summed result is identical to the individual events. Apply
§5.5 splitting *after* summing.

---

## 6. Zoom

### 6.1 What actually works: `setZoomFactor` (reflow zoom)

Measured, 800 px viewport:

| `setZoomFactor` | `getZoomFactor()` | `innerWidth` | `visualViewport.scale` |
|---:|---:|---:|---:|
| 1.25 | 1.25 | 640 | 1 |
| 2.0 | 2.0 | 400 | 1 |
| 0.5 | 0.5 | 1600 | 1 |

This is **reflow zoom** — CSS pixels change size and the page re-lays-out, exactly like Ctrl+`+` in
a desktop browser. It is not pinch zoom (`visualViewport.scale` stays 1), and per §3.3 pinch zoom is
unreachable. Reflow zoom is also the *better* fit for a terminal: text reflows to the cell grid
instead of being magnified into a blurry crop.

### 6.2 Zoom ladder

Use Chromium's own ladder so behaviour matches user expectation:

```
25, 33, 50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200, 250, 300, 400, 500   (%)
```

Bindings — keyboard first, because Ghostty's kitty-keyboard support makes modifiers reliable
(verified per project brief) whereas mouse modifiers are subject to terminal capture:

| binding | action |
|---|---|
| `Ctrl/Cmd` + `=` / `+` | next ladder step up |
| `Ctrl/Cmd` + `-` | next ladder step down |
| `Ctrl/Cmd` + `0` | reset to 100 % |
| `Ctrl` + wheel | ±1 ladder step per **notch** (post-§5.2 grouping, so the terminal multiplier cannot make one gesture jump 7 steps) |

### 6.3 The ctrl+wheel conflict, stated honestly

Measured: ctrl+wheel through `sendInputEvent` does **not** trigger Chromium's built-in zoom
(`getZoomFactor()` stayed 1.0, `visualViewport.scale` stayed 1.0) — it just scrolled. But the
modifier **does** reach the page (`ctrlKey: true` in the DOM event, §2.6).

So there is a genuine conflict with no free answer:

* If we intercept ctrl+wheel and call `setZoomFactor`, browser zoom works — but Google Maps and
  Figma stop zooming, because they never see the event.
* If we forward it, those pages zoom — but browser zoom does nothing, since Chromium will not do it
  for us.

**Recommendation:** intercept by default (browser-zoom is what the overwhelming majority of pages
and users expect from ctrl+scroll), and expose an escape hatch — a `scroll.ctrl_wheel = zoom | page`
setting plus a modifier override (e.g. `Ctrl+Alt+wheel` always forwards). Document it. Do not try to
auto-detect by watching for a scroll that did not happen; that is a race against the compositor and
§2.6 shows the compositor wins unpredictably.

---

## 7. Horizontal scrolling

Wheel-left/right (buttons 66/67) is already decoded (`input.rs:359-360`) and the engine already
accepts `deltaX` (`main.js:198`); §2 confirms horizontal behaves identically (`deltaX: -120` →
`scrollX = 120`). Run the same §5 model on an independent `ScrollAxis` instance.

Add `Shift + vertical wheel → horizontal` as the portable fallback, since not every terminal emits
66/67. Ghostty's default `mouse-shift-capture = false` (verified locally in §1.2's config dump)
means shift is forwarded to the application rather than captured for selection, so this is safe on
our primary target — but it is a per-terminal behaviour worth listing in the capability matrix.

---

## 8. Instructions for the commander (no core files touched by me)

**`apps/cli/src/main.rs:629-640`** — the only change needed for scrolling.

Replace the two fixed `±120` literals with the §5 accumulator. Keep the existing sign mapping
(`WheelUp → positive`), which §2.2 proves correct. Emit `f64` deltas — §2.1 shows fractional values
accumulate exactly, so no residual accumulator is required, but note the current `format!` string
interpolates an integer and will need a `{:.2}`-style format to avoid emitting `deltaY:0` for
sub-pixel steps. Suggested shape:

```rust
MouseKind::WheelUp | MouseKind::WheelDown => {
    let dir = if kind == MouseKind::WheelUp { 1 } else { -1 };
    let d = self.scroll_y.on_notch(dir, Instant::now());   // §5.3
    let dy = dir as f64 * d;                                // §5.5 sign
    format!(r#"{{"t":"input","kind":"mouse","action":"wheel","x":{px},"y":{py},"deltaX":0,"deltaY":{dy:.2}{m}}}"#)
}
```

Burst grouping (§5.2) belongs in the input read loop where batch boundaries are visible, not in the
per-event match arm.

**No engine change is required for scrolling.** Do *not* add `hasPreciseScrollingDeltas` (§4.3).

**`apps/engine/src/main.js`** — one new command for zoom only:

```js
if (cmd.t === 'zoom') {                 // { "t":"zoom", "factor": 1.25 }
  wc.setZoomFactor(cmd.factor);
}
```

**Engine review checklist** — add: `Input.synthesizeScrollGesture` and `Input.synthesizePinchGesture`
are forbidden; they SIGSEGV the browser process (§3.2).

---

## 9. Verification plan

### 9.1 The one measurement that needs a human (3 minutes)

Blocked here by the lock screen; everything else in this document is measured. Run in each terminal:

```sh
# Enable SGR mouse reporting, then timestamp every wheel byte that arrives.
printf '\033[?1003h\033[?1006h'
# scroll: (a) one slow deliberate wheel notch  (b) a slow trackpad drag  (c) a hard flick, let it coast
#   ... capture with:  script -q /dev/null cat -v   or a small read() loop that stamps monotonic ms
printf '\033[?1006l\033[?1003l'
```

Record, per gesture: number of `CSI <64` / `<65` events, and the millisecond gaps between them.
This yields (1) the true terminal expansion factor per notch (§1.3), and (2) the real inter-event
spacing floor, which confirms or retunes `BURST_MS` (§5.2). Nothing else in the design depends on it.

### 9.2 CI-able assertions (no display, no human)

These run headless against the engine and would have caught every bug in this document:

1. `deltaY: -N` → `scrollY == N` for N ∈ {1, 10, 53, 120, 360} — pins the 1:1 contract (§2.1).
2. From `scrollY = 500`, `deltaY: +120` → `380` — pins the sign, from mid-scroll so clamping cannot
   mask a regression (§2.2).
3. `deltaY: -100000` → `scrollY == innerHeight` — pins the clamp (§2.3).
4. 5 × `deltaY: -120` at 16 ms → `scrollY == 600` — pins accumulation (§2.4).
5. Non-precise `-120` is **not** complete at the first frame; precise `-120` **is** — pins the
   animation contract that §4.3 depends on (§4.3).
6. Inline non-passive `preventDefault` listener → `scrollY` stays 0 (§2.6). Listener must be inline.
7. Unit-test the §5.3 model directly against the §5.3 table — pure function, no browser needed.
8. Assert `Input.synthesize*Gesture` is absent from the source tree (grep guard, §3.2).

### 9.3 Subjective acceptance

Isolated notch moves exactly 120 px; a trackpad drag never exceeds 3000 px/s; an inertia tail decays
without a terminal lurch; ctrl+wheel steps the zoom ladder once per physical notch regardless of
`mouse-scroll-multiplier`.

---

## Appendix A — reproducing the measurements

Probes were run from `apps/engine` against the vendored Electron, with the agent sandbox disabled
(Chromium children cannot spawn under it — `bootstrap_look_up ... Permission denied`):

```sh
cd /Users/adeebbashir/projects/blackglass/apps/engine
./node_modules/.bin/electron <probe>.js <mode>
```

Probe skeleton (offscreen window + 20000 px page, matching the production OSR path):

```js
const win = new BrowserWindow({ width: 800, height: 600, show: false,
  webPreferences: { offscreen: true, nodeIntegration: false, contextIsolation: true } });
const wc = win.webContents; wc.setFrameRate(60);
await wc.loadFile(tallPage);                       // body { height: 20000px }
const js = (c) => wc.executeJavaScript(c, true);
wc.sendInputEvent({ type: 'mouseWheel', x: 400, y: 300, deltaX: 0, deltaY: -120, canScroll: true });
await sleep(600);
console.log(await js('window.scrollY'));
```

Run each risky CDP call in its **own process** — `Input.synthesize*Gesture` takes the process down,
so batching them into one probe loses every result after the crash (this is how the first probe
run died mid-way).

## Appendix B — constants, single table

```rust
const D_MAX:       f64 = 120.0;  // px, isolated notch  (Chromium-native, unchanged from today)
const D_MIN:       f64 = 16.0;   // px, shaping-curve floor
const D_FLOOR:     f64 = 4.0;    // px, absolute floor under the velocity cap
const T_FULL:      f64 = 160.0;  // ms, interval earning a full notch
const GAMMA:       f64 = 0.5;    // curve shaping
const V_MAX:       f64 = 3000.0; // px/s, velocity ceiling
const T_MOMENTUM:  f64 = 100.0;  // ms, below this the momentum clamp applies
const RECOVER:     f64 = 1.15;   // max step growth per event during a tail
const IDLE_RESET:  f64 = 400.0;  // ms, gap that resets gesture state
const ALPHA:       f64 = 0.35;   // EMA weight on newest interval
const BURST_MS:    f64 = 5.0;    // ms, terminal-expansion grouping window  [tune via §9.1]
const CEIL_FRAC:   f64 = 0.9;    // fraction of viewport height per event
```

---

## Sources

Local primary evidence: `/Applications/Ghostty.app/Contents/MacOS/ghostty +show-config --default
--docs`; `apps/engine/node_modules/electron/electron.d.ts:8917,9790,9795,18346`;
`crates/bg-term/src/input.rs:344-378`; `apps/cli/src/main.rs:629-640`;
`apps/engine/src/main.js:194-202`; sibling artifact `artifacts/swarm/A06-input-research.md`.
Live measurement: Electron 43.2.0 / Chromium 150.0.7871.129 OSR probes (Appendix A).

- [kitty.conf configuration reference](https://sw.kovidgoyal.net/kitty/conf/) — `pixel_scroll`, `touch_scroll_multiplier`, `wheel_scroll_multiplier`
- [Kitty 0.46 release coverage](https://linuxiac.com/kitty-0-46-terminal-emulator-released-with-smooth-scrolling-and-tab-dragging/) — pixel/momentum scrolling scope
- [kitty#262 Fullscreen scroll multiplier](https://github.com/kovidgoyal/kitty/issues/262)
- [kitty#5502 Different scrolling in shell vs full-screen application](https://github.com/kovidgoyal/kitty/issues/5502)
- [ghostty#3955 Scrolling in mouse-based programs should send a single scroll signal](https://github.com/ghostty-org/ghostty/discussions/3955)
- [ghostty#4259 Mouse-mode programs scrolling too fast](https://github.com/ghostty-org/ghostty/discussions/4259)
- [ghostty#8670 mouse-scroll-multiplier precision multiplier](https://github.com/ghostty-org/ghostty/issues/8670)
- [xterm control sequences (ctlseqs)](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html) — mouse tracking, mode 1007
