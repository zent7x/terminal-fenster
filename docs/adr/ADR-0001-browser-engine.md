# ADR-0001: Browser engine and frame acquisition path

- **Status:** Accepted
- **Date:** 2026-07-31
- **Decision owner:** Commander (B01)
- **Supersedes:** none

## Context

BlackGlass must render Chromium-class web content as pixels inside a terminal. The engine
choice determines web compatibility, frame latency, install size, packaging and security
posture, and it is expensive to reverse. It must be settled by measurement, not preference.

Constraints verified on the target machine (macOS 26.1, Apple M4, 10 cores, 24 GB, arm64):

- Disk headroom is only ~9 GiB, so a from-source Chromium/CEF build is not feasible here.
- Node 24.11.1, Rust 1.93 available. Electron 43.2.0 (Chromium 150.0.7871.129) installs in ~4 s.

## Options considered

1. **Electron offscreen rendering (OSR), bitmap path** — `webPreferences.offscreen: true`,
   frames via the `paint` event as a `NativeImage`.
2. **Electron OSR, shared-texture path** — `offscreen: { useSharedTexture: true }`, frames
   arrive as `event.texture` (an IOSurface handle on macOS).
3. **Raw Chromium / CEF embedding** — maximum control, but requires a multi-GB toolchain and
   a from-source build that this machine cannot host; also a much larger packaging burden.
4. **Headless Chrome + CDP `Page.startScreencast`** — no Electron dependency; delivers
   base64 JPEG/PNG frames through the DevTools JSON transport. See the correction below:
   the frame-rate objection originally recorded here was wrong.

## Measurement

Spike `apps/engine/spike/fps-matrix.js`, 5 s run, 1440x900 viewport, on a page that forces
genuine per-frame repaints (canvas fill + text mutation). Each mode ran in its own process
because `disableHardwareAcceleration()` is process-global and must precede app-ready.

| Mode | Paints | FPS | Gap p50 | Gap p95 | Gap p99 | Delivery |
|---|---|---|---|---|---|---|
| gpu (default) | 279 | 60.2 | 16.65 ms | 18.62 ms | 19.94 ms | NativeImage bitmap |
| software (`disableHardwareAcceleration`) | 298 | 60.1 | 16.63 ms | 18.61 ms | 19.45 ms | NativeImage bitmap |
| shared (`useSharedTexture: true`) | 296 | 59.8 | 16.70 ms | 17.40 ms | 18.65 ms | `event.texture`, zero bitmaps |

Supporting measurements from `apps/engine/spike/osr-probe.js`:

- Pixel format is **BGRA**, 4 bytes/pixel, non-strided: a 1440x900 frame is exactly
  5,184,000 bytes (`w*h*4`), verified against a known pure-red page (`[0,0,255,255]`).
- `https://example.com` first paint at 2104 ms cold, rendering correct fonts, layout and
  link styling (`spike/out/example-com.png`).

### Invalidated first result

The initial probe reported **0.3 fps** and was wrong. It animated with CSS
`transform: translateX`, which Chromium promotes to a compositor-only animation that
produces **no software paint events at all**. The corrected page (canvas + text mutation)
shows the paint pipeline runs at full 60 fps. Recorded here because the failure mode is
non-obvious and would otherwise be rediscovered: *a terminal browser must never benchmark
its frame path with composited-only CSS properties.*

## Decision

**Adopt Electron OSR (option 1), default hardware-accelerated, bitmap `paint` path.**

Shared-texture mode (option 2) is deferred, not rejected. It is genuinely faster on tail
pacing (p99 18.65 ms vs 19.94 ms) but delivers a GPU handle rather than pixels: `bitmapFrames`
was 0 and `textureFrames` was 296. Consuming it requires a native addon to map the IOSurface
and a disciplined `texture.release()` on every frame or the GPU process stalls on exhausted
buffers. That is real complexity to buy ~1.3 ms of p99 when the bitmap path already saturates
the 60 Hz cadence. Revisit only if profiling shows the BGRA copy is the binding constraint.

Option 3 is rejected for this milestone: CEF cannot be built in the available disk.

Option 4 is **deferred as a documented, unbuilt Tier-2 fallback**, on corrected grounds.

### Correction: the original CDP rejection was measurably wrong

This ADR first claimed CDP screencast "caps frame rate well below the compositor's native
cadence." Mission B03 measured it against the identical fps-matrix workload on Chrome
150.0.7871.187 and found **59.9 fps at p50 frame gap 16.66 ms** — statistically tied with
OSR's 16.65 ms. That premise is false and is struck.

The decision to prefer OSR still holds, for three structural reasons that survive
measurement:

1. **Lossy text.** JPEG q80 measured PSNR 34.51 dB with a max channel error of 99/255,
   producing visible ringing on glyph edges. A browser whose text is mush is not a browser.
2. **No dirty-rect field exists in the screencast protocol.** A blinking caret re-sends the
   entire viewport. That is disqualifying for the SSH story specifically.
3. **A 7.50 ms/frame decode tax** — 37–45% of a core — spent undoing an encode we never
   wanted in the first place.

Recording the correction rather than quietly editing the conclusion: a decision defended by
a false premise is one good measurement away from being reopened, and the next person
deserves to know which argument actually carries the weight.

B03 also corrected two environment facts: free disk is **5.5 GiB**, not ~9 GiB, and Chrome
136+ permanently forbids remote debugging on the default profile, so a CDP fallback could
never reuse the user's logged-in session.

## Consequences

- We inherit Chromium 150 web compatibility for free, including WebGL, media and workers.
- Frames are CPU-side BGRA buffers; the renderer must handle BGRA→RGB(A) conversion, and
  that conversion sits on the hot path and must be measured.
- Electron is a ~309 MB dependency. Packaging and integrity verification are required work
  (owned by B10/F03), and the engine version must be pinned.
- The Chromium sandbox stays **enabled** (`sandbox: true`, `contextIsolation: true`,
  `nodeIntegration: false`) as set in the spikes. Only the *harness* sandbox was relaxed to
  run the spikes; see the environment note below.
- Damage tracking is **not proven, and the existing evidence is confounded** (per B02). Both
  spikes forced full-viewport damage by construction: `osr-probe.js` animated a CSS
  `transform` (compositor-only) and `fps-matrix.js` fills the entire canvas every frame.
  Neither could have observed a small dirty rect even if Chromium reports them. A localized
  change (caret blink, hover) has never been tested. Nothing in this repo may assume partial
  damage works until that spike runs.
- Related: the engine currently **writes the dirty rect into the frame header and then sends
  the full bitmap anyway**. Until damage is both proven and consumed, every frame costs full
  viewport bytes. This is the single largest known performance gap.
- Shared-texture mode was re-measured by B05 across 10 paired runs and was **0.52 ms
  *slower*** (t=0.41) — the p99 advantage recorded in the table above came from a single
  sample and does not reproduce. B05 additionally found IOSurface pads rows to a 64-byte
  boundary (`bytesPerRow` 9984 vs the 9928 a naive `w*4` stride assumes at 2482x814), so an
  addon reusing the bitmap path's stride math would shear the image on real window sizes.
  Shared texture is now **rejected**, not merely deferred.

## Environment note (not a product decision)

Chromium child processes fail to start under Claude Code's Bash sandbox with
`bootstrap_look_up ... Permission denied (1100)` followed by `No rendezvous client,
terminating process` — the harness blocks the Mach port rendezvous that Chromium's
multi-process model needs. Spikes therefore run with the harness sandbox disabled. This does
**not** relax Chromium's own sandbox, which remains on.

Separately: Electron's default behavior when the last window closes is to **quit the app**.
Destroying a window between spike stages killed the main process and made the next stage's
renderer fail rendezvous with `parent died?`. Subscribing to `window-all-closed` takes over
that decision and is required in any multi-window host.

## Reversal cost

Moderate. The engine sits behind a frame-source interface; swapping to CDP screencast or CEF
means reimplementing frame acquisition and input injection but leaves the terminal renderer,
compositor, protocol encoders and input decoders untouched.
