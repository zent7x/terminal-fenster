# BlackGlass local test fixtures

Deterministic pages for exercising the Chromium engine, the input transport and the
terminal backends. Every page opens over `file://` with no server, reaches no network, and
loads no CDN. Open `index.html` to browse them.

Sixteen pages, one JSON corpus, one generator, one verifier, and four small assets. Nothing
here is a mock: each page runs the real API it is named after, reports what actually
happened, and refuses to decide questions it cannot see the answer to.

---

## 1. Why these exist

BlackGlass renders untrusted web content into a byte stream that a terminal emulator
interprets as commands. That inverts the usual browser threat model and makes the terminal
side, not the page side, the thing most likely to be wrong. Screenshot diffing is not
available on this host (the machine sits at a lock screen), so every fixture is built to be
verified two ways that both work headlessly:

- **from JavaScript** — `window.__bg.state()` returns a JSON snapshot of what the page
  observed, over CDP or `executeJavaScript`;
- **from raw pixels** — five fixed-position marker blocks and, per fixture, solid colour
  patches at documented CSS coordinates, so a BGRA frame can be checked with a byte scan
  and no DOM access at all.

Both paths are CI-able. Neither needs a human to look at a screen.

---

## 2. The contract every fixture honours

### 2.1 Readiness

```
document.documentElement[data-bg-ready] === "1"      # poll this, it needs no JS eval
window.__bg.ready === true
```

### 2.2 The `window.__bg` object

| member | meaning |
|---|---|
| `id` | fixture id, matches `manifest.json` |
| `proto` | contract version, currently `1` |
| `ready` | set once setup finished |
| `pass` | `true` / `false` / `null` when the page cannot judge itself |
| `query` | parsed query string |
| `expect` | **the fixture's own documented truth** — coordinates, colours, invariants |
| `state()` | JSON-serialisable snapshot |
| `reset()` | restore the initial state without a reload |

Read coordinates from `expect` rather than hard-coding them in a test. If a fixture ever
moves an element, the test moves with it instead of silently passing on the wrong pixel.

### 2.3 Marker pixels

Five blocks, 24x24 CSS px, `position: fixed`, on every page. Negative coordinates are
measured from the far edge of the frame.

| marker | sample point | colour |
|---|---|---|
| beacon | `(12, 12)` | `#ff00ff` loading → `#00ffff` ready |
| status | `(36, 12)` | `#333333` idle · `#00ff00` pass · `#ff0000` fail |
| top-right | `(-12, 12)` | `#ff8000` |
| bottom-left | `(12, -12)` | `#0080ff` |
| bottom-right | `(-12, -12)` | `#ffff00` |

The four corners let a scanner locate and scale the page inside a frame without any DOM
access. Colours are saturated primaries so they survive whatever quantisation a terminal
backend applies.

### 2.4 Determinism rules

- No `Math.random`, no `Date.now`-driven animation, anywhere.
- Animated fixtures load **paused at frame 0** and are scrubbed by an integer. `?play=1`
  releases them when the point is paint rate rather than pixel identity.
- Layout is absolute and integral; the design viewport is **1280x800**.
- `scroll-behavior: auto` is forced. Smooth scrolling would make frames depend on the clock.
- The blinking caret is the one thing that breaks frame identity. `contenteditable.html`
  takes `?caret=off` and mirrors the caret into a solid `#ff00ff` bar instead.

### 2.5 Self-containment

Every page inlines its own CSS and JS. There is no shared `.css` or `.js` file, so a
fixture still works when copied elsewhere, served over `http://`, or handed over as a
`data:` URL. The duplicated ~40-line harness block is the price, and it is worth it.

Only three things live outside the HTML: the video assets, the upload samples, and the
escape corpus — and even the video has its clip inlined as a `data:` URI by default.

---

## 3. What each fixture proves

| fixture | proves |
|---|---|
| `click-targets.html` | A click at a documented CSS coordinate lands on the element that occupies it. Reports `client/page/offset/screen` and the **signed error from the target centre**, which is the mouse-precision number. Includes right/middle/double-click targets, a calibration pair exactly 300 px apart, and a `transform: translate(20px,10px)` trap where the layout box and the painted box differ. |
| `text-input.html` | Synthetic keys insert text into a focused control; focus routing is visible as a colour. The password field is reported only as `{len, sum}` — never as text — so a test can prove the exact string arrived without the string ever entering a log. Carries a decoy card number for redaction tests (A09 T-AGT-2). Logs the full `key/code/keyCode/which/location` shape of every keydown, which is what the terminal key encoder has to reproduce. |
| `contenteditable.html` | Rich-text host, `plaintext-only` host, and a nested non-editable island. Reports selection offsets, the `beforeinput` `inputType` stream, and the **caret rectangle** the terminal cursor must be drawn at. |
| `hover.html` | CSS `:hover` under offscreen rendering. Tile A uses **no JavaScript at all**, isolating the compositor hit-test path from the event path. Also covers parent→child hover, a hover that triggers relayout, pointer events, and a tile deliberately occluded by a translucent overlay that must stay unhovered. |
| `drag-drop.html` | Native HTML5 drag and pointer-emulated drag, measured **separately**, because they fail separately. Plus a file-drop zone and a sortable list. |
| `scrolling.html` | 6000 px page, 24 colour bands 250 px tall in a strip at x=64, position markers every 250 px, a nested scroller, a horizontal scroller and a sticky header. The band colour at a viewport row tells a frame scanner where the viewport is. |
| `canvas2d.html` | 2D canvas as a pure function of an integer frame counter. The 40x40 patch at page (50,130) takes `PALETTE[frame % 6]`, so the frame index is readable from pixels. `?play=1` turns it into the repainting-page corpus and reports p50/p99 rAF gaps in-page. |
| `webgl-triangle.html` | A WebGL context exists and rasterises. `gl.readPixels` at the triangle centroid self-checks without a screenshot. Reports vendor/renderer/version/extension count, falling back WebGL2 → WebGL1 → experimental. |
| `css-animation.html` | Composited `transform`, layout-driven `left`, a `transition`, and `opacity` keyframes. Scrubbable to an exact progress with a negative `animation-delay` on a paused animation, so a frame at p=0.5 is reproducible anywhere. Lanes A and B must land on the same x through different code paths. |
| `video.html` | A real 3-second clip: red for 1 s, green for 1 s, blue for 1 s, at 320x180. Default source is an inline `data:` URI; `?src=webm|mp4|blob` switches. Mirrors the current frame into a canvas and reads the centre pixel, so decode is provable without a frame scan. Reports a ten-entry codec support matrix. |
| `form-submit.html` | Submission by button, by **Enter key** in a lone text field, and by `requestSubmit()`. Plus a POST expected to fail under `file://` and a `required`/`type=email` form that must not navigate. |
| `form-target.html` | The landing page. Re-derives what the query string should have been for each route and sets its own pass flag, so a test asserts one boolean instead of matching a URL string. |
| `file-upload.html` | File inputs (single, multiple, `accept`, `webkitdirectory`), a drop-to-upload zone, and a **programmatic** `DataTransfer` path that exercises `File` + `FileReader` end to end without any OS dialog. |
| `popup.html` / `popup-child.html` | Six `window.open` routes against the engine's `setWindowOpenHandler` deny policy, including a real user-gesture `target="_blank"` anchor. Records the outcome rather than prejudging which policy is right. The child is unmistakable three ways: lime viewport, distinctive title, `postMessage` back. |
| `escape-injection.html` | 29 terminal control-sequence payloads driven into title, URL, link text, download filename, console and page text. See section 5. |
| `index.html` | Human index; also a link-dense page for hint-mode testing. |

---

## 4. Running them

### 4.1 By hand

```bash
open tests/fixtures/index.html          # any browser, no server needed
```

### 4.2 Through BlackGlass

```bash
blackglass open "file://$PWD/tests/fixtures/click-targets.html"
```

### 4.3 The bundled verifier

`verify-fixtures.js` loads every fixture in an offscreen Electron window configured exactly
like `apps/engine/src/main.js` (`offscreen`, `sandbox: true`, `contextIsolation: true`,
`nodeIntegration: false`), waits for ready, pulls `state()`, scans the five marker pixels in
the raw BGRA frame, and then drives several fixtures with **real `sendInputEvent` calls** —
because a fixture that only works when poked from JavaScript proves nothing about the input
transport.

```bash
apps/engine/node_modules/electron/dist/Electron.app/Contents/MacOS/Electron \
  tests/fixtures/verify-fixtures.js
# --json            machine-readable output
# --only=hover,video
# --disable-gpu     force the SwiftShader path
```

Exit code 0 if every check passed. It uses a throwaway `userData` directory and opens no
port. Note that Chromium child processes need the agent Bash sandbox disabled to start.

---

## 5. `escape-injection.html` — read before running

This is the only fixture that is itself an attack. On load it writes a terminal control
sequence into `document.title` and `console.log`. That is the point: it reproduces exactly
what a hostile page does the instant it is opened, with no interaction.

**Every canary is an `echo` of a marker string.** Nothing here deletes, downloads or
exfiltrates anything, even in the worst case where the sanitiser is absent, the clipboard is
poisoned, and the user pastes into a shell and presses Return. Any future case must keep
that property.

Suggested procedure:

```bash
pbcopy </dev/null
blackglass open "file://$PWD/tests/fixtures/escape-injection.html"   # disposable terminal
pbpaste | xxd | head            # must be empty
```

`?case=<id>` selects a payload, `?case=none` disables the on-load application.
`__bg.applyTitle/applyHash/applyLink/applyDownload/applyConsole/applyText` drive individual
sinks; `applyAlert` is manual-only because a native dialog under OSR will stall a run;
`titleStorm()` is opt-in and stops itself after four seconds.

### 5.1 The corpus

`escape-corpus.json` holds all 29 cases and is **byte-identical** to the JSON inlined in the
page, so a Rust unit test and the browser test share one input set. Regenerate and re-inject
with:

```bash
node tests/fixtures/make-escape-corpus.js
```

Verify they still agree:

```bash
node -e 'const fs=require("fs"),h=fs.readFileSync("tests/fixtures/escape-injection.html","utf8"),
o="<script type=\"application/json\" id=\"bg-corpus\">",i=h.indexOf(o),
j=h.indexOf("</"+"script>",i);
process.exit(h.slice(i+o.length,j).trim()===fs.readFileSync("tests/fixtures/escape-corpus.json","utf8").trim()?0:1)'
```

Payloads are UTF-16 code units, because two cases (an unpaired surrogate and NUL) cannot
round-trip through a JSON string in every parser. Encoding:

```json
"segments": [ { "u": "001b" }, { "u": "0041", "n": 65536 } ]
```

`u` is hex, exactly four digits per code unit, big-endian, no separator. The payload is the
concatenation of each segment's decoded units repeated `n` times. The repeat form keeps a
64 KiB flood to one line.

Coverage: OSC 52 write (7-bit, C1 and ST-terminated), OSC 52 read, OSC 2 + CSI 21t title
report, bracketed-paste escape, DCS/sixel, **APC/kitty-graphics**, OSC 8 hyperlink spoof,
iTerm2 OSC 1337, CSI clear/home, SGR reverse, charset designation, SO/SI, ENQ, CR overwrite,
BS URL spoof, the full C0 zoo, the full C1 zoo, bidi override, bidi isolates, zero-width,
PUA, Zalgo, unpaired surrogate, U+2028/2029, a 16 KiB title, a 64 KiB unterminated OSC, and
one benign control case that must pass through unchanged.

The DCS and APC cases are BlackGlass-specific and are not in the usual terminal-injection
literature: our own sixel and kitty graphics data share one byte stream with page-derived
text, so a page that can emit either introducer can desynchronise our renderer.

---

## 6. Measured results

All numbers below were produced on **2026-08-01**, host macOS 26.1 / Apple M4, using the
vendored **Electron 43.2.0 / Chromium 150.0.7871.129** in an offscreen 1280x800 window. They
are measurements, not expectations — if the platform changes, these change.

```
14/14 fixtures passed
frame 1280x800 BGRA for every fixture; all five marker pixels correct on all 14
ready latency 1-3 ms after loadFile
```

### 6.1 Input transport

| what | result |
|---|---|
| Click at documented centre `(240,320)` of `t4` | hit, `dx=0 dy=0`, zero misses. `clientX/clientY` exactly as sent; `offsetX/offsetY = 40,40` |
| Typing `blackglass` then `hunter` via `keyDown`+`char`+`keyUp` | both fields exact; 16 keydowns; `password.len 6`, `sum 662`; `masked true` |
| Typing into `contenteditable` after a synthetic click | text `BLACKGLASS`, 10 `beforeinput` all `insertText`, caret rect `{x:146.33, y:132, w:0, h:19}` |
| Bare `mouseMove` with **no** preceding `mouseEnter` | **`:hover` activated anyway.** Tile A went `rgb(0,192,0)`; one `mouseover` fired |
| Occluded tile under a translucent overlay | stayed `rgb(192,0,0)`; `elementFromPoint` returned the occluder |
| `Element.matches(':hover')` | returned **false** for every tile while `getComputedStyle` showed the hover style applied. Assert on computed style, not on `matches(':hover')` |
| Form submit by clicking the button at `(635,160)` | navigated to exactly the predicted URL, `?q=blackglass&n=42&opt=b&hidden=h1&chk=on`; landing page self-checked `route: button` |

### 6.2 Scrolling — the most consequential finding

| what | result |
|---|---|
| wheel **event delivery** via `sendInputEvent({type:'mouseWheel', deltaY, canScroll:true})` | **works every time.** `counts.wheel` increments once per call, `deltaMode` is `0` (pixels), and `lastWheel.target` is the element under the cursor |
| **document** scroll from those events | **never moved.** `window.scrollY` stayed `0` for `deltaY` of `-600`, `-120`, `+120`, five stacked `-120`s, and three stacked `+80`s |
| **nested** `overflow:scroll` element | **inconsistent.** Three consecutive clean runs of `mouseEnter` + 3x `deltaY 80` over `#inner` left `scrollTop` at `0`. Two earlier sequences — both of which had already delivered a document-level wheel first — did move it (`scrollTop 120` after one `deltaY 120`, and ~`500` in another) |
| CDP `Input.dispatchMouseEvent {type:'mouseWheel', deltaY:240}` over the document | also does not scroll |
| CDP `Input.synthesizeScrollGesture` | **UNVERIFIED** — never settled and wedged the debugger session; not retried |
| `window.scrollTo` / `scrollBy` via `executeJavaScript` | works in every run; `__bg.scrollToY(1250)` lands on `1250`, band index 5, colour `#bfff00` |

The distinction the fixture's counters make is the useful part: this is **not** "the event
never arrived". The event arrives, correctly targeted, and nothing scrolls. The nested case
moving only in sequences that had a prior document-level wheel points at wheel-phase
latching rather than a flat lack of support — which is a better lead than "OSR can't
scroll", and also worse news, because intermittent input is harder to ship than absent
input.

Today's only deterministic scroll path is a scripted `scrollTo`/`scrollBy`.

### 6.3 Drag and drop — a finding

| what | result |
|---|---|
| Pointer lane via `sendInputEvent` (down, 5 moves, up) | drop succeeded |
| Native HTML5 lane via `sendInputEvent` | **`dragstart` never fired.** A synthetic mouse press never enters the OS drag loop |
| Native HTML5 lane via CDP `Input.dispatchDragEvent` | works: `dragenter 1`, `dragover 2`, `drop 1`, payload `BG-DND-PAYLOAD-1`, `types ["text/plain"]` |

### 6.4 Graphics and media

| what | result |
|---|---|
| Canvas 2D, paused | frame 0 patch `[255,0,0,255]`; `setFrame(3)` → `[0,255,255,255]`, matching `PALETTE[3]` |
| Canvas 2D, `play()` for 1.2 s | 72 rAF callbacks, **60.0 fps**, gap p50 **16.7 ms**, p99 **16.8 ms** — consistent with the project's earlier 16.65/19.94 ms measurement |
| WebGL | `webgl2` available, renderer `ANGLE (Apple, ANGLE Metal Renderer: Apple M4, Unspecified Version)`, `WebGL 2.0 (OpenGL ES 3.0 Chromium)`; centroid `readPixels` `[128,63,64,255]` — not the clear colour |
| CSS animation scrub | at p=0.5 both lanes at x=311 against an expected 310; at p=0.25 both at 176 against 175. The 1 px offset is the 2 px track border, and composited and layout-driven lanes agree with each other |
| Video decode, all four sources | `readyState 4`, duration 3.000, 320x180, no error |
| Decoded centre pixel at t=1.5 s | `data` `[0,255,0]`, `webm` `[0,255,0]`, `blob` `[0,255,0]`, `mp4` `[0,255,1]`. The YUV round trip is effectively exact; use ±4, not a wide tolerance |
| Canvas readback of a video frame | **not tainted** for any of `data:`, file-backed webm, file-backed mp4, or `blob:`, from a `file://` document. Pixel-level video assertions need no server |
| H.264 | `canPlayType` `probably`, and the mp4 decoded |

### 6.5 Popups

All six routes returned `null` and the engine's `setWindowOpenHandler` emitted a
`{t:'popup', url}` event for each — including for a real user-gesture `target="_blank"`
anchor click, which is the route most likely to bypass a deny handler. It did not. Zero
`postMessage` round trips, as expected when no child window is created.

### 6.6 Escape injection — the important table

All 29 payloads driven into `document.title`, measured as the engine's
`page-title-updated` payload. "wire" is UTF-16 code units the engine received.

| case | in | wire | C0/C1 surviving | bidi/ZW | PUA | lone surrogate |
|---|---|---|---|---|---|---|
| osc52-write-7bit | 57 | 56 | 0 | 0 | 0 | 0 |
| osc52-write-c1 | 52 | 52 | **2** (`0x9d`, `0x9c`) | 0 | 0 | 0 |
| osc2-title-set-and-report | 35 | 33 | 0 | 0 | 0 | 0 |
| dcs-sixel | 43 | 42 | 0 | 0 | 0 | 0 |
| apc-kitty-graphics | 35 | 34 | 0 | 0 | 0 | 0 |
| c0-zoo | 44 | 14 | **1** (`0x0b`) | 0 | 0 | 0 |
| c1-zoo | 43 | 43 | **32** | 0 | 0 | 0 |
| bidi-trojan-source | 37 | 37 | 0 | **1** | 0 | 0 |
| bidi-isolates | 20 | 20 | 0 | **7** | 0 | 0 |
| zero-width | 15 | 15 | 0 | **4** | 0 | 0 |
| private-use-area | 8 | 8 | 0 | 0 | **2** | 0 |
| lone-surrogate | 12 | 12 | 0 | 0 | 0 | **1** |
| overlong-title | 16393 | **4096** | 0 | 0 | 0 | 0 |
| unterminated-osc | 65538 | **4096** | 0 | 0 | 0 | 0 |
| benign-control | 19 | 19 | 0 | 0 | 0 | 0 |

What this says:

1. **Blink already canonicalises most C0 in `document.title`.** ESC, BEL, CR, BS, SO, SI,
   ENQ and DEL never reach the engine through that sink. `osc52-write-7bit` arrived with
   zero control units. That is a mitigating layer nobody wrote down, and it means the
   headline OSC 52 title attack is weaker than A09 assumed *for the title specifically*.
2. **C1 is untouched**, exactly as A09 T2 predicted. `0x9D … 0x9C` arrives intact.
3. **U+000B (vertical tab) survives.** In the full C0 sweep it was the single control that
   made it through. A C0 byte that moves the terminal cursor down a line reaches the engine
   unmodified. This is not in the A09 catalogue.
4. **Bidi, zero-width, PUA and unpaired surrogates all survive**, so the Unicode half of
   the sanitiser is doing all of its own work.
5. **Chromium caps titles at 4096 UTF-16 code units.** That can still be more than 8 KiB of
   UTF-8, so the A09 byte cap remains ours to enforce.
6. **The console sink is not canonicalised at all.** The same payload reached
   `console-message` with `0x1B` and `0x07` intact. E09's ranking was right: console is the
   more dangerous path, not the title.

None of this reduces the need for the sanitiser. It relocates the risk, and it means a
sanitiser test that only exercises `document.title` will pass while the real hole is open.

---

## 7. Assets

| file | bytes | sha256 | provenance |
|---|---|---|---|
| `assets/color-bars.webm` | 2543 | `f7d8110e4647f855c15b85517b2b7282ea8872169c37ab29715d4f045b496e4e` | generated locally, see below |
| `assets/color-bars.mp4` | 2629 | `65a4e7be14d38c94c3b5cdb41b661e6ea1da760ecd00b5eeeb9898021138758c` | generated locally |
| `assets/upload-sample.txt` | 63 | `f3bdcb4db23f7761ee149d1d0954bda842f9e4465c5c5a2c23e548d82a6a96c9` | byte sum 4668 |
| `assets/upload-sample.csv` | 49 | `9359355bafe7e0e9d79dabb33c62f688c3591b0ce9b2eec312bd08d44364a2f7` | byte sum 3588 |
| `assets/upload-dir/{a,b}.txt` | 9 each | — | for `webkitdirectory` |

The clips were produced on this host with ffmpeg from `lavfi` colour sources — no
third-party media, no licence question:

```bash
ffmpeg -f lavfi -i "color=c=0xFF0000:s=320x180:r=10:d=1" \
       -f lavfi -i "color=c=0x00FF00:s=320x180:r=10:d=1" \
       -f lavfi -i "color=c=0x0000FF:s=320x180:r=10:d=1" \
       -filter_complex "[0:v][1:v][2:v]concat=n=3:v=1:a=0[v]" -map "[v]" \
       -c:v libvpx -b:v 60k -pix_fmt yuv420p -color_range pc -deadline good -cpu-used 0 \
       assets/color-bars.webm
```

The mp4 twin uses `-c:v libx264 -preset veryslow -crf 20 -profile:v baseline -level 3.0
-movflags +faststart`. `video.html` inlines the webm as base64 by default, so the page is
self-contained; the files exist for the file-backed and H.264 paths.

---

## 8. Hazards

- **`escape-injection.html` is hostile by design.** Clear the clipboard first, use a
  disposable terminal. Canaries are `echo`-only.
- **`file-upload.html` has two buttons that open a real native file chooser.** Under
  offscreen rendering that `NSOpenPanel` does not paint into the frame and will stall an
  unattended run — the same class of stall D05 measured for downloads without
  `setSavePath()`. Automated tests must use `__bg.attachText()` or CDP
  `DOM.setFileInputFiles`, never `input.click()`.
- **`__bg.applyAlert()` blocks the renderer** behind a dialog that OSR may never show.
  Manual use only.
- **`__bg.titleStorm()`** is a deliberate DoS. It is opt-in and stops itself after at most
  ten seconds.
- **`popup.html` opening nothing is the expected result** under the current engine policy,
  not a failure.

---

## 9. Adding a fixture

1. Copy the shared chrome block (CSS) and harness block (JS) verbatim from any existing
   fixture and change only `id`.
2. Position everything absolutely, in integer CSS pixels, inside 1280x800.
3. Put every coordinate, colour and invariant in `expect`, and make `state()` report what
   was observed rather than what was hoped for.
4. Give the fixture at least one solid colour patch whose colour encodes state, so it can
   be checked from a raw frame.
5. Never call `Math.random`, never read the clock for animation.
6. Add it to `manifest.json` **and** the table in `index.html` — `fetch()` is blocked for
   `file://` documents, so the index cannot read the manifest at runtime.
7. Run `verify-fixtures.js`.

---

## 10. Known gaps

- **`Input.synthesizeScrollGesture` is UNVERIFIED.** The probe hung and wedged the debugger
  session. It is the most promising remaining candidate for main-document scroll and is
  worth one careful re-test with a hard timeout.
- **iTerm2 behaviour is untested** throughout, because macOS TCC blocks automating it on
  this host. The `osc1337-iterm2` corpus case exists but its exploitability there is
  unverified.
- **Nothing here has been run against a real terminal backend yet.** These fixtures verify
  the engine side. The clipboard oracle in section 5 is written but has not been executed
  end to end through `blackglass open`.
- **No `http://` variant.** The POST form and any origin-sensitive behaviour need a
  loopback server to test properly; the `file://` results are noted as such where they
  differ.
- **`index.html` and `manifest.json` are kept in step by hand.** A drift check is worth
  adding to CI.
