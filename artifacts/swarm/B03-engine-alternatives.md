# B03 — Engine alternatives to Electron OSR

**Mission:** assess CDP `Page.startScreencast`, WKWebView offscreen, and Servo as alternatives
to Electron OSR under the disk constraint. Give exact CDP params, measured fps/latency, and a
keep/drop call on CDP as a no-Electron fallback.

**Verdict up front:**

| Option | Call | One-line reason |
|---|---|---|
| CDP `Page.startScreencast` | **KEEP as documented Tier-2 fallback; do not build now** | Technically viable (frames *and* input verified), but lossy on text, no damage rects, and a mandatory decode tax. Its only real edge is zero marginal disk. |
| WKWebView offscreen | **DROP** | No continuous frame API, requires a live view hierarchy, and macOS/iOS-only — which kills the Linux/SSH use case outright. |
| Servo | **DROP for this milestone; revisit ≥2027** | 62% Web Platform Tests vs Chromium's 89%, v0.0.5, and a from-source build this disk cannot host. |

---

## 0. Environment and honesty notes

- Host: macOS 26.1, Apple M4. **Chrome 150.0.7871.187** (system install, already present).
- Electron baseline for comparison is Chromium **150.0.7871.129** — same Chromium major, so
  this is a genuine apples-to-apples comparison rather than a cross-version one.
- All benchmarks used Node 24.11.1's **global `WebSocket`**, so no packages were installed.
  Nothing was downloaded; peak extra disk was the throwaway Chrome profiles, all removed.
- **Disk is tighter than the mission brief states: 5.5 GiB free, not ~9 GiB** (`df -h`,
  `/System/Volumes/Data`, 99% capacity). This makes any from-source engine even less viable.
- Chromium child processes need the harness sandbox disabled, exactly as ADR-0001 records.
  Chromium's own sandbox was not touched.

### Two false results I generated and corrected — both matter to the commander

**(1) A stray Electron poisoned the first run.** My initial benchmark used a fixed CDP port
(9333) and reported *60.4 fps at 800x568 from "Chrome/150.0.7871.129"*. That was not Chrome.
`lsof -nP -iTCP:9333` showed **PID 37164 = `apps/engine/node_modules/electron/.../Electron`**
already listening. I had measured a leaked Electron from another spike and would have reported
it as CDP. Rewrote to launch with `--remote-debugging-port=0` and read the real port back from
`<user-data-dir>/DevToolsActivePort`, then assert the endpoint is not Electron.

> **Action for the commander:** PID 37164 was still alive at time of writing and is not mine —
> I left it running per the file-ownership rule. It holds port 9333 and will silently corrupt
> any other agent's CDP or OSR benchmark the same way it corrupted mine. Worth reaping, and
> worth banning fixed debug ports in any future spike.

**(2) I misattributed a bottleneck to Chrome that was my own client.** A dense-text page at a
constant 60 Hz change rate delivered only **9.0 fps** — I nearly reported that as CDP's encode
ceiling. It was my message handler doing base64 decode + `Buffer` allocation *before* sending
`Page.screencastFrameAck`. Acking first, parsing after, took the identical config to
**49.0 fps**. See §2.3 — this is a 5.4x effect and is the single most important implementation
detail in this whole document.

---

## 1. CDP `Page.startScreencast` — exact parameters

From the DevTools Protocol (tot) definition. **The API is officially marked `Experimental`.**

### `Page.startScreencast` — all five parameters

| Param | Type | Values | Behaviour measured here |
|---|---|---|---|
| `format` | string | `jpeg` \| `png` | `jpeg` default-ish; `png` is lossless but ~2.3x the bytes and ~2.4x the capture→arrival latency. |
| `quality` | integer | 0–100 | JPEG only; ignored for PNG. Diminishing returns — see §2.2. |
| `maxWidth` | integer | pixels | Honored exactly. Chrome downscales **before** encode — the single most effective lever. |
| `maxHeight` | integer | pixels | Honored exactly; aspect preserved by fitting inside the box. |
| `everyNthFrame` | integer | ≥1 | Clean linear divider: 1→59.9 fps, 2→30.1 fps, 4→15.0 fps. |

### `Page.screencastFrame` event

- `data` — base64 of the encoded image (**+33% wire inflation, unavoidable**)
- `sessionId` — integer frame number, must be echoed to the ack
- `metadata` — `offsetTop`, `pageScaleFactor`, `deviceWidth`, `deviceHeight`,
  `scrollOffsetX`, `scrollOffsetY`, `timestamp`

> **There is no dirty-rect / damage field in the metadata.** This is confirmed against the
> protocol definition and by observation: a caret blink re-sends the entire viewport. This is
> the most consequential structural difference from Electron OSR, which supplies a `dirtyRect`
> on every `paint`.

### `Page.screencastFrameAck`

Takes `sessionId`. Chrome will not emit the next frame until the current one is acked — this is
the backpressure valve, and it is why ack-ordering dominates throughput (§2.3).

### Recommended parameter set, if this path is ever built

```json
{ "format": "png", "maxWidth": <terminal_px_w>, "maxHeight": <terminal_px_h>, "everyNthFrame": 1 }
```

`png` rather than `jpeg` is deliberate and may look surprising — justification in §2.5. Set
`maxWidth`/`maxHeight` to the *terminal's* pixel box (e.g. 2482x814 in Ghostty), never the
page viewport, so Chrome does the downscale before paying encode and transport costs.

---

## 2. CDP measurements

Workload is the **exact page from `apps/engine/spike/fps-matrix.js`** (canvas fill + text
mutation via `requestAnimationFrame`; deliberately not a CSS transform), at 1440x900 pinned
with `Emulation.setDeviceMetricsOverride`, 5 s runs.

### 2.1 Parameter matrix

Client acks *after* parsing (the naive ordering), synthetic canvas page:

| Params | fps | gap p50 | gap p95 | gap p99 | capture→arrival p50/p95 | payload p50 | actual size |
|---|---|---|---|---|---|---|---|
| jpeg q80, nth=1 | **59.9** | 16.66 ms | 29.74 | 45.75 | 5.04 / 10.86 ms | 11,793 B | 1440x900 |
| jpeg q50 | 59.6 | 16.50 | 30.99 | 53.93 | 7.08 / 14.56 | 10,283 B | 1440x900 |
| jpeg q30 | 55.3 | 16.64 | 33.56 | 93.58 | 6.54 / 35.47 | 9,754 B | 1440x900 |
| png | 57.5 | 16.06 | 33.45 | 76.09 | 12.29 / 27.86 | 30,005 B | 1440x900 |
| jpeg q80, maxW 720 / maxH 450 | 59.8 | 16.29 | 30.28 | 53.17 | **2.14 / 7.09** | 3,840 B | 720x450 |
| jpeg q80, nth=2 | 30.1 | 33.54 | 50.93 | 64.58 | 7.09 / 10.24 | 11,815 B | 1440x900 |
| jpeg q80, nth=4 | 15.0 | 67.60 | 101.64 | 118.32 | 10.31 / 41.00 | 11,936 B | 1440x900 |

**Correcting ADR-0001 on one point:** the ADR rejects CDP partly because it "caps frame rate
well below the compositor's native cadence." That specific claim is **not supported** —
measured 59.9 fps with a p50 frame gap of 16.66 ms, statistically indistinguishable from the
OSR path's 16.65 ms. The rejection is still correct, but for other reasons (§2.5, §3).

### 2.2 Quality knob is nearly useless on real content

On the flat-fill synthetic page, q80→q30 saves 17% of bytes. On a **dense text page** the knob
barely moves the needle either, because text is high-frequency and resists DCT compression:

| Config (text page) | payload p50 (binary) |
|---|---|
| jpeg q80, 1440x900 | 236,382 B |
| jpeg q40, 1440x900 | 139,863 B |
| jpeg q80, maxW 720 | 74,418 B |

`maxWidth` beats `quality` decisively: **downscaling 2x saves 3.2x the bytes** while dropping
quality in half saves only 1.7x — and downscaling costs no fidelity the terminal could have
shown anyway, since the terminal grid is the real resolution limit.

### 2.3 Screencast is change-driven, and ack ordering dominates throughput

Frame delivery tracks the page's actual mutation rate, not a clock:

- Static idle page → **1 frame in 5 seconds**. Genuinely free when nothing moves.
- Page mutating on a 100 ms `setInterval` → **10.2 fps**.
- Page mutating per `requestAnimationFrame` → ~60 fps.

Holding change rate constant at 60 Hz and varying only client ack discipline and content cost:

| Variant | page change fps | delivered fps | delivered/changed | payload p50 (b64) |
|---|---|---|---|---|
| cheap content, ack-first | 60.0 | 57.6 | **0.957** | 11,340 B |
| dense text, ack-first | 60.2 | **49.0** | **0.817** | 315,176 B |
| dense text, ack-first, maxW 720 | 60.0 | **59.0** | **0.980** | 99,224 B |
| dense text, q40, ack-first | 60.0 | 45.1 | 0.750 | 186,484 B |
| dense text, **ack-after-parse** | 60.2 | **9.0** | **0.150** | 315,176 B |

The last row versus the second is the same Chrome, same page, same params — **5.4x throughput
purely from acking before parsing.** Any async frame transport we build, CDP or not, must ack
or release on the receiving thread before doing work. This is the same class of bug as the
`texture.release()` discipline ADR-0001 already flags for shared-texture mode.

### 2.4 The decode tax (this is the real cost)

`crates/tf-term/src/kitty.rs` sends `f=24` (raw RGB) + zlib and takes **BGRA** input — the
file's own comment explains PNG (`f=100`) was rejected because it moves a full encode onto our
hot path. So a CDP frame *must* be decoded back to raw pixels before it can enter the existing
encoder. Measured with Pillow/libjpeg-turbo:

| Frame | JPEG bytes | Decode p50 | p95 | Raw RGB out | Cost of one core @60 fps |
|---|---|---|---|---|---|
| 1440x900 q80 | 231,819 | **7.50 ms** | 8.48 | 3,888,000 B | ~45% |
| 1440x900 q40 | 142,838 | 6.12 ms | 7.70 | 3,888,000 B | ~37% |
| 720x450 | 74,231 | **2.25 ms** | 3.20 | 972,000 B | ~13% |

Electron OSR pays **zero** here — it hands over BGRA directly. At full viewport, CDP spends
roughly 37–45% of a core doing nothing but undoing an encode Chrome should never have done.

It also forces a **new Rust dependency**. The workspace today is `libc` + `flate2` only
(`Cargo.toml`); CDP would add a JPEG/PNG decoder and its supply-chain surface.

### 2.5 JPEG is lossy, and this is a text renderer

Using a Chrome-rendered PNG of the text page as ground truth and re-encoding at each quality:

| Quality | Bytes | PSNR | Max channel error |
|---|---|---|---|
| q80 | 246,980 | **34.51 dB** | 99 / 255 |
| q60 | 183,852 | 30.88 dB | 118 / 255 |
| q40 | 147,562 | 28.66 dB | 136 / 255 |

~40 dB is the usual "visually lossless" threshold. **34.5 dB on rendered text is visible
ringing on glyph edges** — mosquito noise around exactly the thing a terminal browser exists to
display. A max channel error of 99/255 on a text page is not a rounding difference.

This is why the recommended params in §1 say `png` despite the cost: for a product whose
payload is mostly text, lossy transport is a correctness problem, not a tuning parameter. And
note PNG screencast at 268,870 B is roughly what our own raw-RGB+zlib kitty payload would be
anyway — so `format: "png"` makes CDP's transport cost honest rather than cheap-looking.

### 2.6 Input works — CDP is a complete engine substitute, not just a frame source

A fallback is worthless if it can only render. Verified end-to-end: dispatched ten
`Input.dispatchKeyEvent` pairs spelling `terminal-fenster` into a focused `<input>`; the DOM observed
all ten keys and `input.value === "terminal-fenster"`.

| Metric | p50 | p95 |
|---|---|---|
| key → DOM acknowledges | 4.86 ms | 39.52 ms |
| key → next screencast frame | 21.52 ms | 44.81 ms |

Cold start: 412–575 ms to `DevToolsActivePort`; 16–73 ms from `startScreencast` to first frame.
Comparable to the 212 ms engine-ready / 366 ms first-frame of the current Electron path.

### 2.7 Two hard operational constraints

1. **Chrome 136+ refuses `--remote-debugging-port` on the default profile.** Verified
   empirically (ProcessSingleton abort) and confirmed as deliberate security hardening in
   Chrome's own developer blog — a non-default `--user-data-dir` is mandatory, because the
   default profile's encryption key must stay unreachable to debug-port attackers.
   **Consequence: a CDP fallback can never reuse the user's logged-in Chrome session.** Every
   launch is a cold, cookie-less profile. That is good for our threat model and bad for UX.
2. **The debug port is an unauthenticated full-control channel.** Any local process that can
   reach `127.0.0.1:<port>` can drive the browser, read every page and exfiltrate cookies. Ties
   directly to A09. If this path is ever built: bind loopback only, use an ephemeral port,
   treat the WebSocket URL as a secret capability token, and keep the throwaway profile.

---

## 3. CDP vs Electron OSR — the decision table

| Axis | Electron OSR (current) | CDP screencast |
|---|---|---|
| Throughput | 60.2 fps | 49–59.9 fps (ack-first) |
| Frame gap p50 | 16.65 ms | 16.66 ms — **tie** |
| Frame gap p99 | **19.94 ms** | 45.75 ms — **2.3x worse tail** |
| Pixel fidelity | Lossless BGRA | Lossy JPEG (34.5 dB) or 2.3x-cost PNG |
| Damage / dirty rect | `dirtyRect` per paint | **None — always full viewport** |
| Decode cost | 0 ms | 2.25–7.50 ms/frame (13–45% of a core) |
| Encodes on the path | 1 (ours) | 2 (Chrome's + ours) + 1 decode |
| Marginal disk | 309 MB shipped | **0 MB if Chrome present** |
| Session reuse | Ours to control | Impossible (Chrome 136+) |
| API stability | Public, supported | **Marked `Experimental`** |

The tail-latency and damage rows are the ones that matter for this product. A terminal browser
over SSH (A07) lives or dies on sending *small deltas*; CDP structurally cannot, because the
protocol has nowhere to put a dirty rect.

**Where CDP genuinely wins:** it needs no shipped engine. If Chrome/Edge/Brave/Chromium is
already installed, Terminal-Fenster costs 0 MB extra and inherits Chromium 150 for free. That is a
real strategic asset for a `curl | sh` install story or a locked-down machine where a 309 MB
download is unacceptable — and it is the *only* reason to keep this path documented.

---

## 4. WKWebView offscreen — DROP

- **No continuous frame API.** `takeSnapshot(with:completionHandler:)` is a one-shot async
  snapshot, not a stream. There is no public equivalent of Chromium's `paint` callback, so
  driving it at 60 fps means polling snapshots — a per-frame full re-render with a completion
  handler on every one.
- **Renders only when visible.** WKWebView will not reliably paint when detached from the view
  hierarchy; the standard workaround is to attach a zero-size or hidden view to a real window,
  and even then JavaScript and rendering behave inconsistently. There is no true headless mode.
- **macOS/iOS only.** This is disqualifying on its own. A terminal browser's most valuable
  scenario is a Linux box over SSH; an engine that cannot run on Linux cannot be the engine.
- **WebKit, not Chromium.** Different compatibility surface from everything already measured,
  discarding the Chromium-150 parity we currently get for free.

No measurement was attempted because the API shape rules it out before performance is relevant.

---

## 5. Servo — DROP for this milestone

- **Web compatibility is not close.** ~62% weighted Web Platform Tests versus Chromium's ~89%.
  For a product positioned as "a real Chromium-class browser," rendering roughly two-thirds of
  the platform is a product failure, not a performance trade.
- **Maturity.** Current line is 0.0.x (0.0.5 as of early 2026); reports of broken rendering and
  crashes on ordinary sites are common, and the project describes itself as an embeddable
  engine rather than a daily driver.
- **Disk.** A from-source Rust build of a browser engine plus dependencies cannot be hosted in
  5.5 GiB. This is the same wall that eliminated CEF in ADR-0001. No prebuilt embedding
  artifact was found that avoids it.
- **The embedding API is actively churning** (`libservo` renamed to `servo` on crates.io,
  `UserContentManager` reshaped, accessibility hooks landing) — appropriate for a young
  project, wrong for a load-bearing dependency this quarter.

Genuinely worth revisiting: Servo is Rust, which would collapse our process model into a single
binary and delete the entire IPC and packaging burden. That is a real prize, just not in 2026.
Recommend a calendar reminder, not a spike.

---

## 6. Recommendation

**Keep Electron OSR. Keep CDP as a documented, unbuilt Tier-2 fallback. Drop WKWebView and
Servo.**

CDP is not the strawman ADR-0001 implies — it hits 59.9 fps, injects input correctly, and costs
zero marginal disk. It fails on three things that happen to be exactly this product's core:
lossy text, no damage rectangles, and a 37–45%-of-a-core decode tax to undo an encode we never
wanted. Those are structural, not tunable.

**Single most actionable item:** amend ADR-0001's stated reason for rejecting CDP. It currently
rejects it for capping "frame rate well below the compositor's native cadence," which is
measurably false (59.9 fps, 16.66 ms p50) and is the kind of wrong premise that gets a settled
decision reopened by the next person who benchmarks it. Replace it with the three real reasons
above. I have not edited the ADR — core files are the commander's.

Two supporting items worth folding in cheaply:

1. Define the frame-source interface as `(pixels, pixel_format, damage: Option<Rect>, timestamp)`.
   `Option` on damage is the honest shape: OSR supplies it, CDP structurally cannot, and encoding
   that now keeps the fallback a drop-in instead of a rewrite.
2. Record the ack-first rule (§2.3) wherever the shared-texture `release()` discipline is
   recorded. Same failure mode, 5.4x penalty, and it will bite any async transport we write.

---

## Appendix — reproduction

Scripts live in the session scratchpad (not committed; they write nothing into the repo):

- `cdp-bench2.js` — param matrix; port auto-discovery via `DevToolsActivePort`, asserts the
  endpoint is not Electron, parses real JPEG/PNG dimensions from payload bytes.
- `cdp-encodebound.js` / `cdp-minclient.js` — holds page change rate at 60 Hz via rAF while
  varying content cost and ack ordering; counts page-side rAF ticks to compute delivered/changed.
- `cdp-realpage.js` — dense-text and idle payload measurement.
- `cdp-input.js` — `Input.dispatchKeyEvent` round-trip and key→frame latency.

Run with the harness sandbox disabled (Chromium child processes need it, per ADR-0001).
`cdp-screencast-bench.js` is the **invalid** first version, retained only as the record of the
fixed-port failure described in §0.

Sources: [Page.startScreencast (DevTools Protocol)](https://chromedevtools.github.io/devtools-protocol/tot/Page/#method-startScreencast),
[Changes to remote debugging switches](https://developer.chrome.com/blog/remote-debugging-port),
[WKWebView offscreen rendering (Apple Developer Forums)](https://developer.apple.com/forums/thread/710015),
[Using WKWebView in headless mode](https://nemecek.be/blog/19/using-wkwebview-in-headless-mode),
[Servo January 2026 (Phoronix)](https://www.phoronix.com/news/Servo-January-2026),
[I tried Servo (OSnews)](https://www.osnews.com/story/142940/i-tried-servo-the-undercover-web-browser-engine-made-with-rust/).
