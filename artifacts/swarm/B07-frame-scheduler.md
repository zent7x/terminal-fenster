# B07 — Frame Scheduler Design

**Mission:** Specify the exact frame-scheduling algorithm for the BlackGlass engine→core path:
visibility-aware pacing, frame coalescing, dirty-region accumulation across dropped frames,
idle throttling, and dropped-frame accounting — with the invariants that make it correct.

**Owned output:** this file only. All changes to core files (`apps/engine/src/main.js`,
`apps/cli/src/main.rs`, `crates/bg-proto/src/lib.rs`) are **specified here as instructions for
the commander**, not applied. See §10.

**Status of the current code:** coalescing is implemented and correct as far as it goes, but it
**drops the dirty rect of every coalesced frame** (`main.js:96-97`). That is a latent
stale-region bug that is masked today only because the consumer ignores the dirty rect. Fixing
it is the single highest-value change in this document (§5, §10.1).

---

## 1. Evidence base (what is real)

All line references are to the repo at the time of writing.

| Fact | Source | Value |
|---|---|---|
| Coalescing model: 1 in-flight + 1 pending, newest wins | `apps/engine/src/main.js:42-99` | `writeInFlight`, `pendingFrame` |
| Coalesce counter incremented on overwrite | `main.js:96` | `stats.coalesced++` |
| Dirty rect **discarded** on coalesce | `main.js:96-97` | old header thrown away |
| 32-byte frame header carries `seq,w,h,dirtyX,dirtyY,dirtyW,dirtyH,format` | `main.js:87-95`, `bg-proto/src/lib.rs:19-30` | u32 BE ×8 |
| `image.toBitmap()` returns the **full** framebuffer (BGRA, non-strided) | `main.js:86`, ADR-0001 | w·h·4 bytes |
| Consumer currently **ignores** dirty rect, re-encodes full frame | `apps/cli/src/main.rs:770-822` | `encode_rgb_frame(... page_w, page_h ...)` |
| `setFrameRate(60)` set once at window creation | `main.js:115` | fixed 60 |
| `setFrameRate` accepts **only 1–240** (CPU/`toBitmap` path) | `electron/electron.d.ts:18399-18402` | source floor = 1 fps, not 0 |
| Focus reporting `?1004h` enabled | `crates/bg-term/src/tty.rs:152` | visibility signal present |
| `FocusGained`/`FocusLost` decoded | `crates/bg-term/src/input.rs:106-107,271-274` | events exist |
| Focus events **dropped**, never forwarded to engine | `apps/cli/src/main.rs:639` | `Event::FocusGained \| FocusLost ... => false` |
| Consumer poll wakes every 16 ms even when idle | `apps/cli/src/main.rs:461` | `poll(..., 16)` |
| PTY write is the dominant cost; damage-tracking is "the primary optimization lever" | A10 §0.1, §4.1 `damage_area_ratio` | — |
| Accounting: `coalesce_ratio = 1 − present_fps/paint_fps`; unexplained drops are bugs | A10 §4.1-4.2 | metric definitions |
| Idle targets: `write()` syscalls during idle = **0**; wakeups < 20/s | A10 §6.3 | thresholds |

Electron 43.2.0, Chromium 150, `webPreferences.offscreen: true`, `useSharedTexture` effectively
false (we call `toBitmap()`), so the 1–240 `setFrameRate` range applies.

---

## 2. Design in one paragraph

The scheduler is two independent layers. **Layer 1 (output pacing)** lives entirely on the
engine side and is fully under our control: a single-slot "newest-wins" coalescer that, on every
coalesce, **unions the dirty rectangles** of the frame it is dropping into the frame it keeps, so
no changed pixel is ever silently withheld from the consumer. Layer 1 reaches a *true* 0 fps
whenever Chromium stops painting: no pending frame ⇒ no encode, no write. **Layer 2 (source
pacing)** caps Chromium's paint production with `setFrameRate` and gates output on visibility,
driven by terminal focus events. Layer 2 is an optimization; Layer 1 is the correctness spine.
Correctness never depends on Layer 2.

---

## 3. State model

One scheduler instance per engine process (single view). All state is engine-side.

```
// The one pending frame (Layer 1). null ⇒ nothing to send.
pending = null            // { seq, bitmap, width, height, dirty:{x,y,w,h} }
writeInFlight = false      // a T_FRAME write has not yet drained

// Visibility / pacing (Layer 2)
visible   = true           // driven by terminal focus events
lastPaintNs = 0            // monotonic ns of the last 'paint'
lastInputNs = 0            // monotonic ns of the last injected input
curRate   = 60             // last value passed to setFrameRate()

// Accounting (§6). Monotonic counters, never reset in-session.
stats = {
  produced:   0,   // 'paint' events received
  sent:       0,   // T_FRAME writes issued (WriteBegin)
  coalesced:  0,   // paints folded into a pending frame that was replaced before send
  suppressed: 0,   // paints accepted while !visible and intentionally not written
  resized:    0,   // paints whose geometry differed from pending (forced full-damage)
}
```

**Constants** (tunable; starting points, to be swept per A10 §4/§6):

```
RATE_FG     = 60   // fps, focused
RATE_BG     = 10   // fps, unfocused but session live
RATE_IDLE   = 1    // fps floor (Chromium minimum); static pages self-quiesce anyway
IDLE_NS     = 500e6  // 500 ms with no paint AND no input ⇒ idle
```

`dirty:{x,y,w,h}` is stored as a **bounding box** in framebuffer pixel coordinates,
origin top-left, half-open on the far edge (`x .. x+w`). One box, not a list — see §5.4 for why
a box (over-approximation) is always correct and why a rect-list is a later optimization.

---

## 4. The algorithm

### 4.1 Rectangle union (the load-bearing primitive)

```
function unionRect(a, b):            // a may be null; b is a valid rect
    if a is null: return { x:b.x, y:b.y, w:b.w, h:b.h }
    x0 = min(a.x,      b.x)
    y0 = min(a.y,      b.y)
    x1 = max(a.x+a.w,  b.x+b.w)
    y1 = max(a.y+a.h,  b.y+b.h)
    return { x:x0, y:y0, w:x1-x0, h:y1-y0 }
```

Pure, O(1), allocates nothing meaningful. This is the only new logic Layer 1 needs.

### 4.2 On paint (producer side)

```
function onPaint(dirty, image):                 // Chromium 'paint' event
    if socket dead: return
    stats.produced += 1
    lastPaintNs = now()
    size   = image.getSize()
    bitmap = image.toBitmap()                    // full BGRA framebuffer, current pixels

    if pending is not null:
        // We are about to discard the PREVIOUS pending frame's *transmission*.
        // Its pixels are superseded by `bitmap`, but its DAMAGE must survive.
        stats.coalesced += 1
        pending.dirty = unionRect(pending.dirty, dirty)    // <-- INVARIANT I2
        pending.bitmap = bitmap
        pending.seq    = nextSeq()                          // newest paint owns the frame
        if size.width != pending.width or size.height != pending.height:
            // geometry changed: old union coords are meaningless; force full damage.
            stats.resized += 1
            pending.width  = size.width
            pending.height = size.height
            pending.dirty  = { x:0, y:0, w:size.width, h:size.height }
    else:
        pending = { seq:nextSeq(), bitmap, width:size.width, height:size.height, dirty }

    // clamp defensively (union across a resize, or a bad rect, must not exceed bounds)
    pending.dirty = clampToFrame(pending.dirty, pending.width, pending.height)

    scheduleFlush()
```

`nextSeq()` is the existing per-paint `seq++` (`main.js:88`). Assigning it per paint (not per
send) is deliberate: it lets the accounting decoder tell which paints reached the wire and which
were coalesced (A10 §4.2 keys traces by `seq`).

### 4.3 Flush (consumer-facing side, respects visibility + backpressure)

```
function scheduleFlush():
    if writeInFlight: return           // one frame in flight already; coalesce continues
    if pending is null: return         // nothing to send  -> true 0 fps  (INVARIANT I5)
    if not visible:                    // Layer 2 output gate
        stats.suppressed += 1          // count it; the frame stays in `pending`, fresh
        return
    flushFrame()

function flushFrame():
    f = pending
    pending = null
    writeInFlight = true
    stats.sent += 1
    header = pack32(f.seq, f.width, f.height, f.dirty.x, f.dirty.y, f.dirty.w, f.dirty.h, FMT)
    ok = socket.write(frame(T_FRAME, header ++ payloadFor(f)))   // §9 defines payloadFor
    if ok:
        writeInFlight = false
        if pending is not null: setImmediate(scheduleFlush)      // drain the newest
    else:
        socket.once('drain', () => { writeInFlight = false; scheduleFlush() })
```

This is the existing `flushFrame` (`main.js:64-80`) with two changes: it consumes
`pending.dirty` into the header, and it honors the `visible` gate. `suppressed` accounting is new.

### 4.4 Visibility + pacing (Layer 2)

```
function onVisibility(v):              // from core: {"t":"visibility","visible":v}
    visible = v
    applyRate()
    if visible: scheduleFlush()        // a frame may have accrued while hidden -> send newest

function onInput(cmd):                  // hook in existing input handler
    lastInputNs = now()
    applyRate()

function applyRate():                   // idempotent; only calls setFrameRate on change
    target =
        (not visible)                                   ? RATE_BG
      : (now()-lastPaintNs > IDLE_NS
           and now()-lastInputNs > IDLE_NS)             ? RATE_IDLE
      : RATE_FG
    if target != curRate:
        webContents.setFrameRate(target)                // clamps 1..240
        curRate = target
```

`applyRate()` is also invoked opportunistically from `onPaint` (a paint means "active") and from
a **single** low-frequency timer (e.g. one 250 ms `setInterval`) whose only job is to notice the
transition *into* idle after activity stops. That timer is the one permitted periodic wakeup on
the engine side; it does no work when already at `RATE_IDLE` and `pending is null`.

On `onVisibility(false)` we deliberately **keep coalescing** (paints still fold into `pending`
with a growing union) but **stop writing**. That saves the entire encode+PTY cost while the user
is not looking, and because `pending` always holds the newest bitmap + the full accumulated
union, resuming is a single correct flush. No `invalidate()` is required on resume: if the page
was static while hidden, `pending` is null and the terminal already shows the correct last image;
if it changed, `pending` carries it. `invalidate()` on resume is available as a belt-and-braces
option but costs a forced full-damage frame (worst-case PTY write) and is not the default.

---

## 5. Invariants

These are the correctness contract. A test or review that wants to break the scheduler should
target these.

**I1 — Freshness.** The pixels of a transmitted frame are always from the most recent `paint`.
*Why:* `pending.bitmap` is unconditionally overwritten on every paint (§4.2). We never transmit
an old bitmap.

**I2 — Damage completeness (no stale region).** The dirty rect transmitted with a frame covers
**every** pixel that changed since the last frame the consumer applied:

> `transmitted.dirty ⊇ ⋃ { paint.dirtyRect : last_sent_seq < paint.seq ≤ transmitted.seq }`

*Why:* `pending.dirty` starts as the first paint's rect and is `unionRect`-accumulated on every
coalesce (§4.2). It is cleared only by `flushFrame`, which starts the next accumulation fresh
from the following paint — and that paint's damage is, by Chromium's contract, relative to the
frame we just sent. Induction closes. **Over-approximation is allowed; under-approximation is
forbidden** (§5.4).

**I3 — Bounded queue / memory.** At most one frame is pending and at most one is in flight, so
`queue_depth ≤ 1` always — comfortably under A10's `max(queue_depth) > 3` fail threshold. Memory
is ≤ 2 × framebuffer (~10.4 MiB at 1440×900) regardless of paint rate or PTY speed.

**I4 — Consumer compositing soundness.** When the consumer applies only the transmitted dirty
sub-rectangle onto its retained canvas, the result equals a full-frame replace. *Why:* by I1 the
frame's bitmap is the full current framebuffer, so any sub-rect copied from it is current; by I2
the sub-rect covers all real damage. (Holds trivially in today's full-frame mode where the
consumer replaces everything; becomes load-bearing under §9's damage-only mode.)

**I5 — Idle silence / true 0 fps.** If no paint arrives, `pending` stays null and `scheduleFlush`
is a no-op ⇒ zero encodes and zero `write()`s. This satisfies A10 §6.3 ("write() syscalls during
idle = 0") **at the output layer**, independent of `setFrameRate` (which cannot reach 0).

**I6 — Accounting closure.** With `pending == null` and nothing in flight,
`produced == sent + coalesced + suppressed`. Any positive residual is a lost frame — the bug
class A10 §4.1 calls out. (Derivation in §6.)

**I7 — Visibility never loses data.** Suppressing output while hidden cannot drop a change:
suppressed paints remain folded into `pending` (fresh bitmap + accumulated union), so the first
flush after `visible = true` reproduces every change. `suppressed` is bookkeeping, not loss.

**I8 — Monotonic seq.** `seq` increments exactly once per paint and is non-decreasing on the
wire; the consumer may use `seq` gaps purely as a coalescing signal, never as an error.

### 5.4 Why the union — worked counterexample

This is the failure the mission names. Timeline, damage-only transmit (§9), **without** union:

```
t0  full frame sent. Terminal canvas = C0. pending = null.
t1  paint A dirties R_A (spinner, top-left).      pending = { bitmap:B1, dirty:R_A }
t2  paint B dirties R_B (clock, bottom-right) —   PTY still draining B1, so we coalesce.
      WITHOUT union: pending = { bitmap:B2, dirty:R_B }   // R_A thrown away  (this is main.js:96-97 today)
      B2 is the FULL current framebuffer, so its pixels for R_A ARE correct.
t3  flush sends header.dirty = R_B only.
      Consumer composites R_B from B2  -> clock updates.
      Consumer never touches R_A       -> spinner shows C0's STALE pixels
                                          until some later paint happens to cover R_A.
```

The pixels were never wrong (B2 is always full and current); the **metadata** under-reported the
damage. That is why the bug is invisible today (the consumer ignores dirty and redraws
everything, `main.rs:790-822`) and why it becomes a visible artifact the instant damage-only
transmit is enabled. **With** union, `t2` yields `pending.dirty = R_A ∪ R_B`; the flush ships a
box containing both, the consumer composites both from B2, and both regions are current. The
union box may also cover untouched pixels between R_A and R_B — that is safe over-damage (extra
bytes, never wrong). A single bounding box is the right v1: O(1), tiny, always correct. Its one
weakness — two small far-apart damages produce a near-full box — is a *bandwidth* regression, not
a correctness one, and is the motivation for a future multi-rect / tile-grid accumulator (§9.3),
not a reason to withhold the box now.

---

## 6. Dropped-frame accounting

Extend `stats` (§3) and derive the reportable metrics. Every paint has exactly one fate: it
becomes the `seq` that rides a transmitted frame (`sent`), or it is folded-and-replaced
(`coalesced`), or it is held back because hidden (`suppressed`), or it is still sitting in
`pending`. Therefore:

```
dropped_bug = produced − sent − coalesced − suppressed − (pending ? 1 : 0)
            ≡ 0    // INVARIANT I6; any nonzero value is a lost frame and must fail CI
```

Because a transmit that folds *k* paints yields `1 sent + (k−1) coalesced`, summing over all
transmits gives `produced = sent + coalesced (+ suppressed + pending)`. This is the identity a
harness asserts at end-of-run with `pending == null`.

Derived metrics (map onto A10 §4.1 verbatim):

| Metric | Formula | A10 name |
|---|---|---|
| present rate | `sent / wall_seconds` | `present_fps` |
| paint rate | `produced / wall_seconds` | `paint_fps` |
| coalesce ratio | `coalesced / produced` = `1 − sent/produced` | `coalesce_ratio` |
| damage area ratio | `dirty.w·dirty.h / (w·h)` per sent frame | `damage_area_ratio` |
| queue depth | `≤ 1` by construction (I3) | `queue_depth` |

**Drop attribution** (A10 §4.2) requires the engine to emit trace records — `PaintEvent(seq)`,
`FrameCoalesced(seq)`, `WriteBegin(seq)`, `WriteEnd(seq, bytes)`, `QueueDepth(depth)` — so the
offline decoder can classify each coalesce by the phase that overran (encoder-bound vs
PTY-bound vs engine-bound vs IPC-bound). The scheduler already knows these moments; wiring the
trace is a small addition (§10.1, gated behind `BG_TRACE` so it is zero-cost in production).

Distinguish clearly, in any report: **coalesced** = deliberate, correct, expected under a slow
PTY (A10 §0.1); **dropped_bug** = a defect. Conflating them hides regressions.

---

## 7. Visibility-aware pacing — state machine

Three pacing states, driven by focus and recent activity:

```
             focus lost                     no paint & no input for IDLE_NS
  FOREGROUND ───────────► BACKGROUND        FOREGROUND ───────────────────► IDLE
   rate 60                 rate 10 (RATE_BG)  rate 60                          rate 1
   output ON              output GATED OFF   output ON                        output ON
      ▲                       │  (paints still coalesce+union into pending)       │
      └──────focus gained─────┘                 paint or input ──────────────────┘  (back to FOREGROUND)
                                              flush newest on the way out
```

- **FOREGROUND:** normal 60 fps, output flows, coalescing absorbs any PTY backpressure.
- **BACKGROUND (unfocused):** cap source at `RATE_BG` (fewer paints, less `toBitmap` copy cost)
  and gate output off (I7 guarantees no data loss). This is the biggest battery/PTY win for the
  common "browser open in another pane" case.
- **IDLE (focused but static):** cap source at `RATE_IDLE`. For a *truly* static page Chromium
  stops requesting BeginFrames on its own, so wakeups are already near zero and `RATE_IDLE`
  mostly bounds pathological rAF/animation loops rather than helping static pages — see §8.

The **signal source** is the terminal, not the browser: BlackGlass is a single view, so
"visibility" means "is the terminal that shows us focused." `?1004h` is already enabled
(`tty.rs:152`) and focus events already decode (`input.rs:106-107`); they are simply dropped
today (`main.rs:639`). §10.2 forwards them.

---

## 8. Idle throttling to 0 fps — honest layering

Two mechanisms, different guarantees:

**Output layer (Layer 1) — true 0 fps, deterministic, ours.** No paint ⇒ `pending` null ⇒
`scheduleFlush` no-op ⇒ zero encode, zero `write()` (I5). This is the mechanism that actually
delivers A10 §6.3's "0 write() syscalls during idle." It needs no timer and no polling on the
frame path; `paint` is push-based.

**Source layer (Layer 2) — floors at 1 fps, advisory.** `setFrameRate` accepts only 1–240
(`electron.d.ts:18401`), so it can *never* command 0. Its value is capping *animation-bearing*
pages when unfocused or idle, not static pages. **UNVERIFIED:** whether `setFrameRate(1)`
measurably reduces idle wakeups on a *static* page is doubtful — Chromium's compositor already
goes BeginFrame-idle with no damage — and should be measured with A10 §6's `ri_pkg_idle_wkups`
harness before we claim any benefit. Do not assert a static-page win from Layer 2 without that
data.

**Consumer-side idle wakeups are a separate, real problem — and not on the frame path.** The core
poll loop uses a fixed 16 ms timeout (`main.rs:461`) and thus wakes ~62×/s even on a fully static
page, doing nothing but re-arming. That collides with A10 §6.3's "< 20 wakeups/s" target. The
scheduler cannot fix this from the engine side; the fix is in the core loop (§10.2): when no
frame is pending and not loading, compute the poll timeout from the only armed deadlines
(escape-disambiguation 40 ms, or the test exit deadline) and otherwise **block indefinitely**
(`timeout = -1`), waking only on real stdin/socket `POLLIN`. This drops idle wakeups toward zero
and is the second-highest-value change after the union fix.

---

## 9. Transmission modes (what the scheduler enables)

The scheduler maintains a correct union regardless of how much of the frame we actually put on
the wire. Two modes, selected by `payloadFor(f)` in §4.3:

**Mode A — full-frame (today).** `payloadFor(f) = f.bitmap` (whole w·h·4). The consumer replaces
its canvas; `dirty` is informational. Simple; pays the full PTY cost every frame, which A10 §0.1
identifies as the throughput ceiling. Union is *maintained* but not *exploited*.

**Mode B — damage-only (the payoff).** `payloadFor(f)` copies only the `f.dirty` sub-rectangle out
of `f.bitmap` (a strided copy: `dirty.h` rows of `dirty.w·4` bytes at stride `f.width·4`). The
consumer composites that patch onto a retained canvas at `(dirty.x, dirty.y)` and re-encodes.
This is where I2/I4 become load-bearing and where an un-unioned dirty rect would produce the §5.4
stale region. Expected win tracks `1 − damage_area_ratio`; A10 calls damage tracking "the primary
optimization lever."

Mode B requires coordinated changes I do **not** own: the payload semantics (header stays the
32-byte `bg-proto` layout; only the payload length/meaning changes to `dirty.w·dirty.h·4`), the
consumer's retained-canvas composite (`Renderer` in `main.rs`), and the kitty encoder's ability
to update a sub-region. I specify the contract in §10.3 and recommend landing the **union fix
(§10.1) first and independently**, so the scheduler is already correct when Mode B arrives.
Shipping Mode B on top of the current un-unioned coalescer would ship the §5.4 artifact.

**§9.3 Future: multi-rect accumulation.** Replace the single box with a small fixed-capacity list
(e.g. up to 4 rects) or a coarse tile grid (e.g. 64×64 tiles, one dirty bit each). Accumulate by
inserting/merging on coalesce; on overflow, collapse to the bounding box (degrades gracefully to
Mode-B-with-over-damage, still correct by I2). Only worth it once Mode B is measured and the
bounding-box over-damage on multi-region pages (scroll + fixed header, video + controls) is shown
to matter. Not v1.

---

## 10. Required changes to core files (for the commander — I did not edit these)

### 10.1 `apps/engine/src/main.js` — union, visibility gate, trace (HIGHEST PRIORITY)

1. **Union on coalesce.** Replace the pending model (`main.js:48-99`) with §3's `pending` object
   and §4.1-4.2's `unionRect`. Concretely, at the current coalesce point (`main.js:96-97`),
   instead of discarding the old header, compute `pending.dirty = unionRect(pending.dirty,
   dirty)` and keep the newest bitmap. Reset `pending.dirty` to null only in `flushFrame` after
   the frame leaves. This is the fix for the §5.4 stale-region bug and is **correct to land on
   its own**, before any Mode B work.
2. **Resize guard.** When a paint's `getSize()` differs from `pending.{width,height}`, set
   `pending.dirty` to the full new frame and `stats.resized++` (§4.2). Prevents a stale union in
   old coordinates from being applied to a resized canvas.
3. **Visibility gate + pacing.** Add a `{"t":"visibility","visible":bool}` command handler
   (alongside `main.js:207-243`) that sets `visible`, calls `applyRate()`, and `scheduleFlush()`
   on resume. Gate `flushFrame` on `visible` (§4.3). Replace the fixed `setFrameRate(60)`
   (`main.js:115`) with `applyRate()` driven by focus + `IDLE_NS` (§4.4). Add the single 250 ms
   idle-transition timer.
4. **Trace hooks (behind `BG_TRACE`).** Emit `PaintEvent/FrameCoalesced/WriteBegin/WriteEnd/
   QueueDepth` per A10 §4.2 so drop attribution works. Zero-cost when unset. Extend the `stats`
   event (`main.js:236-238`) with `coalesced` (exists), `suppressed`, `resized`, and derived
   `dropped_bug` for the §6 identity.

### 10.2 `apps/cli/src/main.rs` — forward focus, fix idle wakeups

1. **Forward focus as visibility.** At `main.rs:639`, replace the drop of
   `Event::FocusGained | Event::FocusLost` with a command send:
   `self.send(r#"{"t":"visibility","visible":true}"#)` / `false`. This is what powers §7.
2. **Idle poll timeout (A10 §6.3).** At `main.rs:461`, do not hard-code `16`. When no frame is
   pending and `!status.loading`, set the poll timeout to the nearest armed deadline
   (escape-disambiguation at 40 ms if `escape_pending_since.is_some()`, or the test
   `exit_after_ms` deadline) and otherwise `-1` (block until stdin/socket readable). Keeps
   interactivity identical while collapsing idle wakeups from ~62/s toward ~0/s.
3. **(Mode B only, later) retained-canvas composite.** Teach `Renderer` (`main.rs:747-845`) to
   keep a persistent RGB canvas and, when the frame is a damage patch, blit `dirty` at
   `(dirty.x,dirty.y)` before encoding. Guard with `dirty ⊆ frame bounds`, else treat as full
   frame (defense-in-depth, ties to A09 threat model — never index out of bounds on
   attacker-influenced geometry).

### 10.3 `crates/bg-proto/src/lib.rs` — Mode B payload semantics (later)

Header layout is unchanged (§1). For Mode B, define the payload length as
`dirty_w · dirty_h · 4` (a patch) rather than `width · height · 4` (full frame), selected by a new
`format` value (e.g. `format = 1` = "BGRA damage patch", keeping `0` = full BGRA). Add a
`FrameHeader::patch_payload()` alongside `expected_payload()` (`lib.rs:51-53`) and let the
consumer branch on `format`. This keeps full-frame and damage-patch frames self-describing on one
wire and lets Mode B roll out without a flag-day.

---

## 11. Verification hooks

Tie each invariant to a check the swarm's bench/CI can run (A10 §4, §6, §7):

| Invariant | Check |
|---|---|
| I2 damage completeness | `canvas-corner` + a second corner repaint under induced PTY backpressure; assert every changed tile is present in some transmitted `dirty` union. A property test over random rect sequences: `⋃ inputs ⊆ ⋃ transmitted.dirty`. |
| I3 queue bound | `pacing` run at 60 fps: assert `max(queue_depth) ≤ 1` (A10 fails at >3). |
| I5 idle silence | `idle` run, static page: assert `write()` count after quiescence == 0 (A10 §6.3). **`BG_STAMP` must be unset** or the pixel-stamp injects 60 Hz and destroys the measurement (A10 §4.3, §6.3). |
| I6 accounting closure | end-of-run assert `produced == sent + coalesced + suppressed` (pending drained); `dropped_bug == 0`. |
| I7 visibility no-loss | focus-lost during an animation, then focus-gained: assert the first post-resume frame's union covers all damage accrued while hidden. |
| Layer-2 idle benefit | A10 §6 `ri_pkg_idle_wkups` with/without `setFrameRate(1)` on a static page — **measure before claiming** (§8). |

Unit-testable purely in Rust/JS without the engine: `unionRect` (associativity, over-approximation
never under), the §6 accounting identity, and `clampToFrame` on adversarial rects.

---

## 12. Open questions / UNVERIFIED

1. **`setBackgroundThrottling` under OSR `show:false`.** Whether Electron's default background
   throttling does anything for an already-offscreen window, and whether it helps or fights our
   explicit `setFrameRate`, is UNVERIFIED (`electron.d.ts:18374,18523`). Characterize before
   relying on it; our design does not depend on it.
2. **`setFrameRate(1)` static-page wakeup effect** — §8, UNVERIFIED, likely negligible.
3. **`invalidate()` on resume** — currently specified as *not* needed (I7). If a future
   partial-damage consumer can desync (e.g. dropped a patch on a full socket), a forced
   full-damage frame on focus-gain is the cheap resync; costs one worst-case PTY write.
4. **Multi-rect vs bounding box** — deferred to §9.3, decision gated on Mode B damage-area data.

---

## 13. Single most actionable recommendation

**Land the union-on-coalesce fix in `main.js` now, as an isolated change (§10.1 item 1), before
any damage-only transmit work.** The coalescer already drops the dirty rect of every frame it
discards (`main.js:96-97`); this is a dormant stale-region bug (§5.4) that is invisible only
because the consumer still redraws whole frames. Fixing it is O(1) code, adds no dependency, and
makes the scheduler correct *ahead of* the Mode B optimization that would otherwise ship the
artifact. Everything else here (visibility pacing, idle-wakeup poll fix, trace-based accounting)
is valuable but sequenceable; the union fix is the one that prevents a correctness regression the
moment the primary performance lever is pulled.
