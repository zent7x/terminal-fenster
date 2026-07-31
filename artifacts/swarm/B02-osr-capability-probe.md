# B02 — Electron OSR Capability Probe (design)

**Status:** design only. Nothing in this document has been executed. Every number quoted from
prior work is cited to a file and line; every number this probe would produce is marked as an
expectation, not a result.

**Deliverable:** drop the JS in §6 at `apps/engine/spike/b02-capability-probe.js` and run it per §7.

---

## 1. Bottom line

Damage tracking is the load-bearing unknown, and **the existing evidence does not answer it —
it is confounded.** Both prior probes used stimuli that genuinely damage the entire viewport,
so "every dirty rect was full-screen" is the expected result whether or not Chromium tracks
damage. We have never actually tested a localized change.

Worse, the repo currently contradicts itself on this point:

| Source | Claim |
|---|---|
| `docs/adr/ADR-0001-browser-engine.md:84-88` | "Damage tracking is **not yet proven** … an open question that materially affects SSH bandwidth." |
| `artifacts/swarm/A10-performance-plan.md:499` | "ADR-0001 **established** that Electron OSR does report partial damage." |

ADR-0001 is correct and A10 misattributes it. `A07-ssh-remote.md:208` then builds on the
optimistic reading — "Damage tracking is worth **81–99%** on typing. A keystroke costs **568
bytes**" — which is a *conditional* result that silently assumes the unproven premise. **A10:499
should be corrected regardless of what this probe finds** (§10).

This probe is built to settle it with a stimulus that makes a partial rect unmistakable, and to
distinguish three materially different outcomes (§8) rather than emit another boolean.

---

## 2. Why the existing evidence is confounded

**Probe 1 — `apps/engine/spike/osr-probe.js:114-147` (`testFrameRateAndDamage`).**
The stimulus is `style.transform='translateX(...)'` (`osr-probe.js:127`). `transform` is a
compositor-only property: Chromium promotes the element to its own layer and animates it on the
GPU thread without ever re-running paint. The measured result was **1 paint in 3741 ms, fps 0.3,
`damageTrackingWorks: false`** (`apps/engine/spike/out/report.json:47-62`). That is a measurement
of the *layer model*, not of damage tracking. `fps-matrix.js:26-28` already documents this exact
flaw: *"Deliberately NOT a CSS transform, which is composited on the GPU thread and can produce
zero software paints — that flaw invalidated the first probe."*

**Probe 2 — `apps/engine/spike/fps-matrix.js:28-44`.**
This corrected the fps problem and reached 60 fps, but its page calls
`x.fillRect(0,0,1440,900)` **every frame** (`fps-matrix.js:38`). It genuinely damages the whole
viewport. A full-screen dirty rect is the *correct* answer for that page. `fps-matrix.js:114`
computes `partialDamage` from it, but the stimulus can never produce a partial rect, so the
metric is vacuous.

**Neither probe has ever driven a localized change.** The question is untouched, not answered.

### 2.1 A second, independent confound: the full-frame image

`electron.d.ts:17217-17220` documents the third `paint` argument as *"The image data of the
**whole frame**."* So the `NativeImage` is always full-size regardless of damage. Confirmed by
`out/report.json:36-41` (`imageSize 1440x900`, `rawBytes 5184000`).

Consequence: **partial damage can only ever save wire bytes, never the CPU-side copy.** And today
it does not even save wire bytes — `apps/engine/src/main.js:86` calls `image.toBitmap()` and
`main.js:97` does `Buffer.concat([head, bitmap])`, sending the entire 5,184,000-byte frame. The
dirty rect is written into the header (`main.js:91-94`) and then ignored. Whatever this probe
finds, **exploiting it requires a crop in `onPaint` that does not exist yet** (§10).

---

## 3. Hypotheses the probe discriminates between

| # | Hypothesis | Predicted signature |
|---|---|---|
| **H1** | Chromium reports true per-region damage. | Rects at `x≈600, y≈400, w≈40, h≈40`; ratio ≈ 0.0012. |
| **H2** | Damage is reported as full-width row strips only. | `x=0, w=frameW`, `h` small. Counted separately as `fullWidthPartialHeight`. |
| **H3** | Damage exists in Viz but is dropped in Electron's bitmap path. | `dirtyRect` always full **but** `textureInfo.contentRect` / `metadata.captureUpdateRect` partial in `--mode=shared`. |
| **H4** | Frame-pool recycling forces full damage on most frames. | Damage is *intermittent*: full for the first N frames, then partial, in a repeating cycle. Detected via `firstPartialIndex` + raw per-frame rect samples. |
| **H5** | Capture-cadence throttling unions damage. | Damage ratio grows as `setFrameRate` drops 60 → 30 → 10. |
| **H6** | No damage signal at all in any mode. | Every ratio is 1.0 everywhere, including the 40×40 stimulus. |

**H4 and H5 are the reason this probe reports distributions and raw per-frame samples rather than
a boolean.** A 4-second boolean is exactly what produced the misleading
`damageTrackingWorks: false`. H5 matters disproportionately: A07 and A10 both plan to *throttle
fps over SSH*, and if throttling unions damage, we would be destroying the very locality we
throttle in order to exploit. That is a self-defeating design and it is cheap to test now.

### 3.1 Experimental hygiene rule

No stimulus may use `transform`, `opacity`, `filter`, `will-change`, `position: fixed`, or any 3D
property **on the changing element**. Those promote it to its own compositor layer, whose changes
never enter the paint path — the mistake that invalidated probe 1. The stimuli use
`background-color`, canvas `fillRect`, text mutation, and real `:hover`.

The stimulus box sits at **(600, 400), 40×40** — deliberately *not* at the origin. A genuine
partial rect must therefore have `x > 0` **and** `y > 0`, which no full-frame rect can imitate.
`rectsOnStimulusBox` counts exactly these. Two controls bracket the result: a full-viewport
canvas (must read 1.0) and a compositor-only `opacity` animation (must read ~0 paints). If the
tiny canvas cannot be distinguished from the full canvas, the signal carries no information and
every conclusion is void.

---

## 4. Stage inventory

Twenty stages. Estimated wall time ≈ 2 min for the default mode.

| Stage | Answers |
|---|---|
| `gpu` | `getGPUFeatureStatus()` / `getGPUInfo('basic')` — explains every other result. |
| `damage-tiny-fps60` | **The decisive one.** 40×40 canvas at (600,400). |
| `damage-full-fps60` | Positive control: must be ratio 1.0. |
| `damage-composited-control` | Negative control: `opacity` keyframes must produce ~0 paints. |
| `damage-caret` | Caret blink in a focused `<input>` (~1 Hz, ~1×20 px). 12 s window. |
| `damage-hover` | Real `:hover` driven by injected `mouseMove`, not a timer. |
| `damage-text-tick` | Small monospace text mutation, fixed-width container (no reflow). |
| `damage-tiny-fps30` / `-fps10` | **H5**: does throttling union damage? |
| `damage-tiny-720x450` | Does behaviour survive a smaller frame? |
| `idle-static` | Do paints stop at zero on an idle page? (Free SSH win if yes.) |
| `invalidate-forces-full` | `main.js:236` calls `invalidate()` on resize — cost and safety. |
| `paint-control-stop-start` | `stopPainting()`/`startPainting()` — zero-cost when unfocused? |
| `dpr-1x` / `dpr-2x` | **The unit question** (§4.1). |
| `zoom-2x` | `setZoomFactor` vs `deviceScaleFactor`; frame size stability. |
| `webgl` | Context, `UNMASKED_RENDERER`, WebGL2, and whether GL frames reach paint. |
| `video` | Codec support offline + whether `<video>` frames reach paint. |
| `cursor-changed` | Does it fire in OSR; what type strings; latency. |
| `popup-window-open` | `setWindowOpenHandler` + does the child inherit OSR and paint? |
| `popup-select-dropdown` | **Is every `<select>` on the web invisible?** (§4.2) |
| `dialog-alert-hang` | Opt-in: does `alert()` wedge the engine? |

### 4.1 The DPR unit question

With `offscreen.deviceScaleFactor = 2` (`electron.d.ts:22577-22582`, experimental, default 1) the
bitmap is 2×. **Is `dirtyRect` in CSS pixels or device pixels?** The stimulus is at CSS (600,400):

- `rect.x ≈ 600` → CSS px → every crop we write must be scaled by DPR or it is wrong by 2×.
- `rect.x ≈ 1200` → device px → crop directly against the bitmap.

Getting this wrong produces a crop that is silently offset — the worst failure mode, because it
looks like a rendering bug rather than a units bug. `dpr-2x` uses a 720×450 window so the box
stays on-screen and the frame is 1440×900.

### 4.2 The `<select>` dropdown test

`TextureInfo.widgetType` is `'popup' | 'frame'` (`electron.d.ts:23723-23725`), which means
Chromium renders select dropdowns as a **separate widget**. If that widget is never composited
into our frame, every dropdown on the web is invisible in BlackGlass — a P0-class usability
failure hiding behind a working-looking browser.

Tested without screenshots (the machine is at a lock screen): keep the last full BGRA frame,
click the select, then `regionDiff` the 200×150 region directly below it against a control region
far away. Frames are non-strided (`out/report.json:47-51`: `bitmapLength === w*h*4`), so the row
offset is `(y*W + x)*4`. Non-zero diff below + zero diff in the control ⇒ the dropdown composites.

---

## 5. Verified API facts used by the probe

All from the **bundled** `apps/engine/node_modules/electron/electron.d.ts`, version 43.2.0
(`node_modules/electron/dist/version` → `43.2.0`). These are primary source and local, not recalled.

| Fact | Citation |
|---|---|
| `paint` signature is `(details: Event<WebContentsPaintEventParams>, dirtyRect: Rectangle, image: NativeImage)` | `electron.d.ts:17215-17220` |
| `image` is "the image data of the **whole frame**" | `electron.d.ts:17217-17219` |
| `details.texture?: OffscreenSharedTexture` | `electron.d.ts:24193-24201` |
| `TextureInfo.contentRect` — "In OSR case, it is the same with `dirtyRect` that needs to be painted" | `electron.d.ts:23743-23747` |
| `Metadata.captureUpdateRect` — "Updated area of frame, can be considered as the `dirty` area" | `electron.d.ts:24684-24688` |
| `Offscreen.deviceScaleFactor` (experimental, default 1) | `electron.d.ts:22577-22582` |
| `Offscreen.sharedTexturePixelFormat`: `argb｜rgbaf16｜nv12` | `electron.d.ts:22567-22575` |
| `setFrameRate` accepts 1–240 when `useSharedTexture` is false | `electron.d.ts:18397-18401` |
| `WindowOpenHandlerResponse.overrideBrowserWindowOptions` | `electron.d.ts:20295-20298` |
| `cursor-changed` → `(event, type, image, scale, size, hotspot)` | `electron.d.ts:16228-16242` |
| `isOffscreen()` / `isPainting()` / `startPainting()` / `stopPainting()` / `invalidate()` | `electron.d.ts:18176, 18180, 18483, 18495, 18130` |

**The high-value discovery here is `captureUpdateRect`.** In shared-texture mode we can read *two*
damage signals that are independent of `dirtyRect`, **by reading metadata only** — no IOSurface
mapping and therefore **no native addon**. ADR-0001 deferred shared-texture mode because consuming
the pixels needs native code; reading its *damage metadata* does not. That makes H3 cheap to test,
and it is the difference between "damage is impossible" and "damage is being dropped by Electron's
bitmap path, and here is where."

---

## 6. The probe

Save as `apps/engine/spike/b02-capability-probe.js`.

Verification performed on this file: `node --check` passes; all 20 stage configs were walked
end-to-end against a stubbed `electron` module; all **16 inline page `<script>` blocks** were
individually parsed with `new Function` (0 invalid); the generated `data:` URLs were decoded and
inspected; and `regionDiff` + `summarize` were unit-tested against synthetic frames (a 10×10 box
diff returned exactly 400 bytes; a 40×40 rect on a 1440×900 frame summarized to ratio 0.00123 with
`rectsOnStimulusBox = 3`, while a full-damage set summarized to `partialFrames = 0`). It has **not**
been run under real Electron.

```js
// SPIKE B02: Electron OSR capability + damage-tracking probe.
//
// THE question this exists to answer: does Chromium ever report a dirtyRect smaller than
// the full viewport? ADR-0001 left this open because every stimulus we had ever measured
// (fps-matrix.js's full-viewport canvas fill) genuinely damages the whole screen. This probe
// drives changes that damage a KNOWN 40x40 box at a KNOWN offset (600,400) so that a real
// partial rect is unmistakable: x~600,y~400,w~40,h~40 versus x0,y0,1440x900.
//
// Usage:
//   electron b02-capability-probe.js                      # bitmap path (the shipping path)
//   electron b02-capability-probe.js --mode=software      # disableHardwareAcceleration()
//   electron b02-capability-probe.js --mode=shared        # shared-texture damage cross-check
//   electron b02-capability-probe.js --only=damage-tiny-fps60,dpr-2x
//   electron b02-capability-probe.js --dialogs=1          # opt-in: may HANG (see notes)
//
// disableHardwareAcceleration() is process-global and must run before app-ready, so each
// mode is a separate process. Emits one __RESULT__<json> line plus out/b02-<mode>.json.
'use strict';

const { app, BrowserWindow } = require('electron');
const fs = require('fs');
const path = require('path');

// ---------------------------------------------------------------- args / setup

function arg(name, dflt) {
  const pre = `--${name}=`;
  const hit = process.argv.find((a) => a.startsWith(pre));
  return hit === undefined ? dflt : hit.slice(pre.length);
}

const MODE = arg('mode', 'bitmap'); // bitmap | software | shared
const ONLY = arg('only', '').split(',').filter(Boolean);
const DIALOGS = arg('dialogs', '0') === '1';
const SHARED = MODE === 'shared';
const OUT = path.join(__dirname, 'out');
fs.mkdirSync(OUT, { recursive: true });

if (MODE === 'software') app.disableHardwareAcceleration();

// Electron quits the app when the last window closes; destroying a window between stages
// would kill the main process and make the next stage's renderer fail Mach rendezvous.
app.on('window-all-closed', () => {});

const R = {
  probe: 'B02',
  mode: MODE,
  electron: process.versions.electron,
  chrome: process.versions.chrome,
  node: process.versions.node,
  platform: process.platform,
  arch: process.arch,
  startedAt: new Date().toISOString(),
  stages: {},
};

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const log = (...a) => console.log('[b02]', ...a);

// Box geometry shared by every damage stimulus. Deliberately NOT at the origin: a real
// partial rect must have x>0 and y>0, which no "full frame" rect can fake.
const BX = 600;
const BY = 400;
const BW = 40;
const BH = 40;

// ---------------------------------------------------------------- window helper

function mkWin(opts) {
  const o = opts || {};
  const w = o.w || 1440;
  const h = o.h || 900;
  const dsf = o.dsf || null;
  const fps = o.fps || 60;

  let offscreen;
  if (SHARED || dsf) {
    offscreen = {};
    if (SHARED) offscreen.useSharedTexture = true;
    if (dsf) offscreen.deviceScaleFactor = dsf;
  } else {
    offscreen = true;
  }

  const win = new BrowserWindow({
    show: false,
    width: w,
    height: h,
    webPreferences: {
      offscreen,
      // Same hardening as the shipping engine; a probe that relaxes the sandbox measures
      // a configuration we will never ship.
      nodeIntegration: false,
      contextIsolation: true,
      sandbox: true,
      webSecurity: true,
    },
  });
  win.webContents.setFrameRate(fps);
  return win;
}

function page(body) {
  return 'data:text/html;charset=utf-8,' + encodeURIComponent('<!doctype html><meta charset="utf-8">' + body);
}

function js(win, code, ms) {
  const limit = ms || 4000;
  return Promise.race([
    win.webContents.executeJavaScript(code, true),
    new Promise((_, rej) => setTimeout(() => rej(new Error('executeJavaScript timeout')), limit)),
  ]).catch((e) => ({ __error: String((e && e.message) || e) }));
}

// ---------------------------------------------------------------- input helpers

function move(wc, x, y) {
  wc.sendInputEvent({ type: 'mouseMove', x: x, y: y });
}

function click(wc, x, y) {
  wc.sendInputEvent({ type: 'mouseMove', x: x, y: y });
  wc.sendInputEvent({ type: 'mouseDown', x: x, y: y, button: 'left', clickCount: 1 });
  wc.sendInputEvent({ type: 'mouseUp', x: x, y: y, button: 'left', clickCount: 1 });
}

// ---------------------------------------------------------------- damage collector

function collectDamage(win, opts) {
  const keepBitmap = !!(opts && opts.keepBitmap);
  const cap = (opts && opts.cap) || 300;

  const s = {
    paints: 0,
    frameW: null,
    frameH: null,
    ratios: [],
    rects: [],
    fullWidthPartialHeight: 0,
    imageSizes: {},
    textureInfoSample: null,
    textureContentRects: [],
    textureUpdateRects: [],
    lastBitmap: null,
    errors: [],
  };

  const onPaint = (details, dirty, image) => {
    s.paints++;
    let fw = null;
    let fh = null;

    const tex = details && details.texture;
    if (tex) {
      // Shared-texture mode: we only READ metadata (no IOSurface mapping, so no native
      // addon needed), then release immediately or the GPU process stalls on exhausted
      // buffers. textureInfo carries TWO damage signals independent of `dirtyRect`.
      try {
        const ti = tex.textureInfo;
        fw = ti.codedSize.width;
        fh = ti.codedSize.height;
        if (!s.textureInfoSample) {
          s.textureInfoSample = {
            widgetType: ti.widgetType,
            pixelFormat: ti.pixelFormat,
            codedSize: ti.codedSize,
            visibleRect: ti.visibleRect,
            colorSpace: ti.colorSpace,
          };
        }
        if (s.textureContentRects.length < cap) {
          s.textureContentRects.push(ti.contentRect);
          s.textureUpdateRects.push((ti.metadata && ti.metadata.captureUpdateRect) || null);
        }
      } catch (e) {
        if (s.errors.length < 5) s.errors.push('texture: ' + String((e && e.message) || e));
      } finally {
        try { tex.release(); } catch (e) { /* already released */ }
      }
    } else if (image && !image.isEmpty()) {
      const sz = image.getSize();
      fw = sz.width;
      fh = sz.height;
      const key = sz.width + 'x' + sz.height;
      s.imageSizes[key] = (s.imageSizes[key] || 0) + 1;
      if (keepBitmap) s.lastBitmap = image.toBitmap();
    }

    if (s.frameW === null && fw) {
      s.frameW = fw;
      s.frameH = fh;
    }

    if (s.frameW) {
      const ratio = (dirty.width * dirty.height) / (s.frameW * s.frameH);
      s.ratios.push(ratio);
      if (dirty.width >= s.frameW && dirty.height < s.frameH) s.fullWidthPartialHeight++;
      if (s.rects.length < cap) {
        s.rects.push({
          i: s.paints,
          x: dirty.x,
          y: dirty.y,
          w: dirty.width,
          h: dirty.height,
          r: +ratio.toFixed(5),
        });
      }
    }
  };

  win.webContents.on('paint', onPaint);
  return { s: s, stop: () => win.webContents.off('paint', onPaint) };
}

// A frame is "partial" if its dirty rect covers < 98% of the frame. The 2% slack absorbs
// off-by-one rounding; it is NOT a threshold that can turn a full frame into a partial one.
const PARTIAL = 0.98;

function summarize(s) {
  const sorted = s.ratios.slice().sort((a, b) => a - b);
  const q = (p) => (sorted.length ? +sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * p))].toFixed(5) : null);
  const partial = s.ratios.filter((v) => v < PARTIAL).length;

  let firstPartialIndex = null;
  for (let i = 0; i < s.rects.length; i++) {
    if (s.rects[i].r < PARTIAL) { firstPartialIndex = s.rects[i].i; break; }
  }

  // Did any rect land on the stimulus box rather than the origin? This is the decisive
  // signal: a rect at (600,400) cannot be a coincidence.
  const onBox = s.rects.filter((r) => r.x > 0 && r.y > 0 && r.w < (s.frameW || 1e9) && r.h < (s.frameH || 1e9)).length;

  const texPartial = s.textureUpdateRects.filter(
    (r) => r && s.frameW && r.width * r.height < s.frameW * s.frameH * PARTIAL
  ).length;
  const texContentPartial = s.textureContentRects.filter(
    (r) => r && s.frameW && r.width * r.height < s.frameW * s.frameH * PARTIAL
  ).length;

  return {
    paints: s.paints,
    frameSize: s.frameW ? { w: s.frameW, h: s.frameH } : null,
    imageSizes: s.imageSizes,
    damageRatio: {
      min: sorted.length ? +sorted[0].toFixed(5) : null,
      p50: q(0.5),
      p95: q(0.95),
      max: sorted.length ? +sorted[sorted.length - 1].toFixed(5) : null,
    },
    partialFrames: partial,
    fullFrames: s.ratios.length - partial,
    partialFraction: s.ratios.length ? +(partial / s.ratios.length).toFixed(3) : null,
    firstPartialIndex: firstPartialIndex,
    rectsOnStimulusBox: onBox,
    fullWidthPartialHeight: s.fullWidthPartialHeight,
    rectSamples: s.rects.slice(0, 60),
    texture: s.textureInfoSample
      ? {
          info: s.textureInfoSample,
          samples: s.textureContentRects.length,
          contentRectPartialFrames: texContentPartial,
          captureUpdateRectPresent: s.textureUpdateRects.filter(Boolean).length,
          captureUpdateRectPartialFrames: texPartial,
          contentRectSamples: s.textureContentRects.slice(0, 20),
          captureUpdateRectSamples: s.textureUpdateRects.slice(0, 20),
        }
      : null,
    collectorErrors: s.errors,
  };
}

// Count differing bytes in a rectangle of two BGRA frames. Frames are non-strided
// (ADR-0001: bitmapLength === w*h*4), so row offset is (y*W + x)*4.
function regionDiff(a, b, W, x0, y0, w, h) {
  if (!a || !b || a.length !== b.length || !W) return null;
  let diff = 0;
  for (let y = y0; y < y0 + h; y++) {
    const off = (y * W + x0) * 4;
    const end = off + w * 4;
    if (end > a.length) break;
    for (let i = off; i < end; i++) if (a[i] !== b[i]) diff++;
  }
  return diff;
}

// Held outside `out` so a 5 MB frame is never serialized into the JSON report.
let selectBefore = null;

// ---------------------------------------------------------------- stage runner

async function stage(name, cfg) {
  if (ONLY.length && ONLY.indexOf(name) === -1) return;
  const t0 = Date.now();
  const w = cfg.w || 1440;
  const h = cfg.h || 900;
  const out = { window: { w: w, h: h }, dsf: cfg.dsf || 1, fps: cfg.fps || 60, durationMs: cfg.ms || 6000 };

  let win = null;
  let rec = null;
  try {
    win = mkWin({ w: w, h: h, dsf: cfg.dsf, fps: cfg.fps });
    if (cfg.before) await cfg.before(win, out);
    rec = collectDamage(win, { keepBitmap: cfg.keepBitmap });

    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('load timeout 15s')), 15000);
      win.webContents.once('did-finish-load', () => { clearTimeout(timer); resolve(); });
      win.webContents.once('did-fail-load', (_e, code, desc) => {
        clearTimeout(timer);
        reject(new Error('did-fail-load ' + code + ' ' + desc));
      });
      win.loadURL(cfg.url || page(cfg.html));
    });

    let cleanup = null;
    if (cfg.drive) cleanup = await cfg.drive(win, out, rec ? rec.s : null);
    await sleep(cfg.ms || 6000);
    if (cleanup) cleanup();
    if (cfg.after) Object.assign(out, (await cfg.after(win, rec ? rec.s : null, out)) || {});
  } catch (e) {
    out.error = String((e && e.message) || e);
  }

  if (rec) {
    rec.stop();
    Object.assign(out, summarize(rec.s));
  }
  out.wallMs = Date.now() - t0;
  try { if (win && !win.isDestroyed()) win.destroy(); } catch (e) { /* already gone */ }

  R.stages[name] = out;
  log(name, JSON.stringify({
    paints: out.paints,
    dmgMin: out.damageRatio && out.damageRatio.min,
    dmgP50: out.damageRatio && out.damageRatio.p50,
    partialFrac: out.partialFraction,
    onBox: out.rectsOnStimulusBox,
    err: out.error,
  }));
}

// ---------------------------------------------------------------- stimuli
// RULE: none of these may use `transform`, `opacity`, `filter`, `will-change`, `position:
// fixed` or 3D on the changing element. Those promote it to its own compositor layer whose
// changes are composited on the GPU thread and never enter the software paint path. That
// exact mistake produced osr-probe.js's misleading `damageTrackingWorks: false`
// (out/report.json: 1 paint in 3741 ms).

const EVLOG =
  '<script>window.__log=[];["mousedown","mouseup","click","focus","change","keydown"].forEach(' +
  'function(t){document.addEventListener(t,function(e){if(window.__log.length<40)window.__log.push(' +
  't+":"+((e.target&&e.target.id)||(e.target&&e.target.tagName)))},true)});</script>';

// Positive control for PARTIAL damage: a 40x40 canvas at (600,400) repainting every frame.
const HTML_TINY_CANVAS =
  '<body style="margin:0;background:#101010">' +
  '<canvas id=c width=' + BW + ' height=' + BH + ' style="position:absolute;left:' + BX + 'px;top:' + BY + 'px"></canvas>' +
  '<script>var x=document.getElementById("c").getContext("2d"),n=0;' +
  '(function f(){n++;x.fillStyle="hsl("+(n%360)+",90%,50%)";x.fillRect(0,0,' + BW + ',' + BH + ');' +
  'window.__frames=n;requestAnimationFrame(f)})();</script>';

// Positive control for FULL damage. If we cannot distinguish this from the tiny canvas,
// the dirtyRect signal carries no information and every conclusion below is void.
const HTML_FULL_CANVAS =
  '<body style="margin:0;background:#111"><canvas id=c width=1440 height=900></canvas>' +
  '<script>var x=document.getElementById("c").getContext("2d"),n=0;' +
  '(function f(){n++;x.fillStyle="hsl("+(n%360)+",80%,50%)";x.fillRect(0,0,1440,900);' +
  'window.__frames=n;requestAnimationFrame(f)})();</script>';

// Caret blink: Chromium's default is ~500 ms on / 500 ms off, and it only blinks when the
// page has focus. Damage should be ~1x20 px. This is the single most important stimulus for
// the SSH story, because typing is the motion where damage tracking is worth 81-99% (A07).
const HTML_CARET =
  '<body style="margin:0;background:#ffffff">' +
  '<input id=i value="" style="position:absolute;left:' + BX + 'px;top:' + BY + 'px;width:200px;height:24px;font:16px monospace">' +
  '<script>document.getElementById("i").focus();</script>' + EVLOG;

// Real :hover driven by real injected mouse moves, not a JS timer, so it exercises the
// hit-test -> style-recalc -> paint path a user would.
const HTML_HOVER =
  '<style>#b{position:absolute;left:' + BX + 'px;top:' + BY + 'px;width:120px;height:40px;background:#333}' +
  '#b:hover{background:#e11}</style>' +
  '<body style="margin:0;background:#101010"><div id=b></div>';

// Small text mutation with a fixed-width container and a monospace font so the change
// cannot reflow anything outside the box.
const HTML_TEXT_TICK =
  '<body style="margin:0;background:#101010">' +
  '<div id=t style="position:absolute;left:' + BX + 'px;top:' + BY + 'px;width:180px;height:20px;' +
  'overflow:hidden;color:#0f0;font:16px/20px monospace"></div>' +
  '<script>var n=0;setInterval(function(){n++;document.getElementById("t").textContent=' +
  '("00000000"+n).slice(-8)},100);</script>';

// NEGATIVE control: opacity is compositor-only. Expect ~0 paints (or full-frame paints that
// are unrelated to this element). Confirms our layer-model reasoning is right, which is what
// makes the positive results trustworthy.
const HTML_COMPOSITED =
  '<style>@keyframes fade{from{opacity:1}to{opacity:.2}}' +
  '#o{position:absolute;left:' + BX + 'px;top:' + BY + 'px;width:120px;height:40px;background:#0af;' +
  'animation:fade .5s infinite alternate}</style>' +
  '<body style="margin:0;background:#101010"><div id=o></div>';

// Idle baseline: static text. Expect paints to drop to 0 after load. Non-zero idle paints
// would mean we burn SSH bandwidth on a page nobody is touching.
const HTML_IDLE =
  '<body style="margin:0;background:#fff;font:16px/1.5 system-ui;padding:40px">' +
  '<h1>static</h1><p>' + 'lorem ipsum dolor sit amet. '.repeat(40) + '</p>';

// ---------------------------------------------------------------- main

app.whenReady().then(async () => {
  if (app.dock) app.dock.hide();
  log('electron', process.versions.electron, 'chrome', process.versions.chrome, 'mode', MODE);

  // ---- GPU posture (cheap, and it explains every other result) ----
  if (!ONLY.length || ONLY.indexOf('gpu') !== -1) {
    const g = { featureStatus: null, basic: null };
    try { g.featureStatus = app.getGPUFeatureStatus(); } catch (e) { g.featureStatus = { error: String(e.message) }; }
    try { g.basic = await app.getGPUInfo('basic'); } catch (e) { g.basic = { error: String(e.message) }; }
    R.stages.gpu = g;
    log('gpu', JSON.stringify(g.featureStatus));
  }

  // ================= DAMAGE: the decisive block =================

  await stage('damage-tiny-fps60', { html: HTML_TINY_CANVAS, ms: 6000, fps: 60 });
  await stage('damage-full-fps60', { html: HTML_FULL_CANVAS, ms: 6000, fps: 60 });
  await stage('damage-composited-control', { html: HTML_COMPOSITED, ms: 6000, fps: 60 });

  await stage('damage-caret', {
    html: HTML_CARET,
    ms: 12000, // caret blinks at ~1 Hz; a 4 s window is too short to be conclusive
    drive: async (win) => {
      win.webContents.focus();
      click(win.webContents, BX + 50, BY + 12);
      return null;
    },
    after: async (win) => ({
      pageFocus: await js(win, 'JSON.stringify({hasFocus:document.hasFocus(),active:document.activeElement&&document.activeElement.id,log:window.__log})'),
    }),
  });

  await stage('damage-hover', {
    html: HTML_HOVER,
    ms: 8000,
    drive: async (win) => {
      let inside = false;
      const id = setInterval(() => {
        inside = !inside;
        if (win.isDestroyed()) return;
        move(win.webContents, inside ? BX + 60 : 60, inside ? BY + 20 : 60);
      }, 300);
      return () => clearInterval(id);
    },
  });

  await stage('damage-text-tick', { html: HTML_TEXT_TICK, ms: 8000 });

  // Frame-rate sweep. HYPOTHESIS: throttling fps makes Chromium union several compositor
  // frames into one capture, inflating damage. If true, the throttling we plan for SSH
  // destroys the very locality we throttle in order to exploit -- a self-defeating design.
  await stage('damage-tiny-fps30', { html: HTML_TINY_CANVAS, ms: 6000, fps: 30 });
  await stage('damage-tiny-fps10', { html: HTML_TINY_CANVAS, ms: 6000, fps: 10 });

  // Viewport sweep: does damage behaviour survive a smaller frame?
  await stage('damage-tiny-720x450', { html: HTML_TINY_CANVAS, ms: 6000, w: 720, h: 450 });

  // Idle: the cheapest possible win if paints really do stop.
  await stage('idle-static', { html: HTML_IDLE, ms: 6000 });

  // invalidate() forces a full-frame regeneration. main.js:236 calls it on every resize;
  // confirm it costs exactly one full-damage frame and does not wedge the damage pipeline.
  await stage('invalidate-forces-full', {
    html: HTML_IDLE,
    ms: 4000,
    drive: async (win) => {
      let n = 0;
      const id = setInterval(() => {
        if (win.isDestroyed() || n++ >= 5) return;
        win.webContents.invalidate();
      }, 500);
      return () => clearInterval(id);
    },
  });

  // startPainting/stopPainting: can we go to zero cost when the terminal loses focus?
  await stage('paint-control-stop-start', {
    html: HTML_TINY_CANVAS,
    ms: 6000,
    drive: async (win, out) => {
      out.control = {};
      setTimeout(() => { if (!win.isDestroyed()) { win.webContents.stopPainting(); out.control.stoppedAtMs = 2000; } }, 2000);
      setTimeout(() => {
        if (!win.isDestroyed()) {
          out.control.isPaintingWhileStopped = win.webContents.isPainting();
          win.webContents.startPainting();
          out.control.restartedAtMs = 4000;
        }
      }, 4000);
      return null;
    },
    after: async (win, s, out) => ({
      frameRate: win.webContents.getFrameRate(),
      isOffscreen: win.webContents.isOffscreen(),
      isPaintingAtEnd: win.webContents.isPainting(),
      control: out.control,
    }),
  });

  // ================= DPR / zoom =================
  // THE UNIT QUESTION: with deviceScaleFactor=2 the bitmap is 2x, but is dirtyRect in CSS
  // px or device px? The stimulus box is at CSS (600,400). rect.x ~600 => CSS px and every
  // crop we write must be scaled. rect.x ~1200 => device px and we can crop directly.
  await stage('dpr-1x', { html: HTML_TINY_CANVAS, ms: 5000, w: 720, h: 450, dsf: 1,
    after: async (win) => ({ pageDpr: await js(win, 'JSON.stringify({dpr:window.devicePixelRatio,iw:window.innerWidth,ih:window.innerHeight})') }) });

  await stage('dpr-2x', { html: HTML_TINY_CANVAS, ms: 5000, w: 720, h: 450, dsf: 2,
    after: async (win) => ({ pageDpr: await js(win, 'JSON.stringify({dpr:window.devicePixelRatio,iw:window.innerWidth,ih:window.innerHeight})') }) });

  // NOTE the viewport: at zoomFactor 2 the CSS layout viewport halves, so a 720-wide window
  // would put the box at left:600px off-screen and we would measure nothing. 1440 wide keeps
  // it visible (innerWidth becomes 720; the box spans CSS 600..640).
  await stage('zoom-2x', {
    html: HTML_TINY_CANVAS,
    ms: 5000,
    w: 1440,
    h: 900,
    drive: async (win, out) => {
      win.webContents.setZoomFactor(2);
      out.zoomFactor = win.webContents.getZoomFactor();
      return null;
    },
    after: async (win) => ({ pageDpr: await js(win, 'JSON.stringify({dpr:window.devicePixelRatio,iw:window.innerWidth,ih:window.innerHeight})') }),
  });

  // ================= WebGL =================
  await stage('webgl', {
    ms: 6000,
    html:
      '<body style="margin:0;background:#000">' +
      '<canvas id=g width=300 height=300 style="position:absolute;left:' + BX + 'px;top:' + BY + 'px"></canvas>' +
      '<script>var c=document.getElementById("g");var gl=c.getContext("webgl2")||c.getContext("webgl");' +
      'if(!gl){window.__info={ok:false,reason:"no context"};}else{' +
      'var d=gl.getExtension("WEBGL_debug_renderer_info");' +
      'window.__info={ok:true,version:gl.getParameter(gl.VERSION),' +
      'glsl:gl.getParameter(gl.SHADING_LANGUAGE_VERSION),' +
      'vendor:d?gl.getParameter(d.UNMASKED_VENDOR_WEBGL):null,' +
      'renderer:d?gl.getParameter(d.UNMASKED_RENDERER_WEBGL):null,' +
      'maxTexture:gl.getParameter(gl.MAX_TEXTURE_SIZE),' +
      'isWebGL2:!!(window.WebGL2RenderingContext&&gl instanceof WebGL2RenderingContext),' +
      'extCount:gl.getSupportedExtensions().length,' +
      'contextLost:false};' +
      'c.addEventListener("webglcontextlost",function(){window.__info.contextLost=true});' +
      'var n=0;(function f(){n++;gl.clearColor((n%60)/60,0.2,0.6,1);gl.clear(gl.COLOR_BUFFER_BIT);' +
      'window.__frames=n;requestAnimationFrame(f)})();}</script>',
    after: async (win) => ({ webgl: await js(win, 'JSON.stringify({info:window.__info,frames:window.__frames||0})') }),
  });

  // ================= video =================
  // Codec *availability* is answered offline by canPlayType + MediaSource.isTypeSupported +
  // mediaCapabilities. The *render path* is answered by canvas.captureStream() -> <video>,
  // which needs no media file and no network. Actual h264 decode of a real file is NOT
  // proven here -- see the deliverable's limitations section.
  await stage('video', {
    ms: 7000,
    html:
      '<body style="margin:0;background:#000">' +
      '<video id=v muted playsinline style="position:absolute;left:' + BX + 'px;top:300px;width:320px;height:240px;background:#222"></video>' +
      '<canvas id=src width=320 height=240 style="display:none"></canvas>' +
      '<script>' +
      'var types=[\'video/mp4; codecs="avc1.42E01E"\',\'video/mp4; codecs="avc1.640028"\',' +
      '\'audio/mp4; codecs="mp4a.40.2"\',\'video/webm; codecs="vp8"\',\'video/webm; codecs="vp9"\',' +
      '\'video/mp4; codecs="av01.0.04M.08"\',\'video/webm; codecs="opus"\',\'application/vnd.apple.mpegurl\'];' +
      'var probe=document.createElement("video");window.__codecs={};' +
      'types.forEach(function(m){window.__codecs[m]={canPlayType:probe.canPlayType(m),' +
      'mse:(window.MediaSource&&MediaSource.isTypeSupported)?MediaSource.isTypeSupported(m):null};});' +
      'var c=document.getElementById("src"),x=c.getContext("2d"),n=0;' +
      '(function f(){n++;x.fillStyle="hsl("+((n*3)%360)+",90%,50%)";x.fillRect(0,0,320,240);' +
      'requestAnimationFrame(f)})();' +
      'window.__play="pending";' +
      'try{var st=c.captureStream(30);var v=document.getElementById("v");v.srcObject=st;' +
      'v.play().then(function(){window.__play="ok"}).catch(function(e){window.__play="ERR:"+e.name+":"+e.message});}' +
      'catch(e){window.__play="CAPTURE_ERR:"+e.message}' +
      'if(navigator.mediaCapabilities){navigator.mediaCapabilities.decodingInfo({type:"file",' +
      'video:{contentType:\'video/mp4; codecs="avc1.640028"\',width:1920,height:1080,bitrate:4000000,framerate:30}})' +
      '.then(function(r){window.__mc=r}).catch(function(e){window.__mc={error:String(e)}});}' +
      '</script>',
    after: async (win) => ({
      video: await js(win, 'JSON.stringify({codecs:window.__codecs,play:window.__play,mc:window.__mc||null,' +
        'el:(function(){var v=document.getElementById("v");return{readyState:v.readyState,videoWidth:v.videoWidth,' +
        'videoHeight:v.videoHeight,currentTime:v.currentTime,paused:v.paused}})()})'),
    }),
  });

  // ================= cursor-changed =================
  await stage('cursor-changed', {
    ms: 5000,
    html:
      '<body style="margin:0;background:#fff;font:16px monospace">' +
      '<a href="#x" style="position:absolute;left:100px;top:100px">link</a>' +
      '<p style="position:absolute;left:100px;top:200px">selectable text</p>' +
      '<div style="position:absolute;left:100px;top:300px;width:120px;height:40px;cursor:crosshair;background:#ccc"></div>' +
      '<div style="position:absolute;left:100px;top:400px;width:120px;height:40px;cursor:wait;background:#ddd"></div>' +
      '<input style="position:absolute;left:100px;top:500px">',
    before: async (win, out) => {
      out.cursors = [];
      const t0 = Date.now();
      win.webContents.on('cursor-changed', (_e, type, image, scale, size, hotspot) => {
        if (out.cursors.length < 40) {
          out.cursors.push({
            t: Date.now() - t0,
            type: type,
            custom: type === 'custom' ? { scale: scale, size: size, hotspot: hotspot, empty: !image || image.isEmpty() } : null,
          });
        }
      });
    },
    drive: async (win) => {
      const pts = [[700, 700], [110, 105], [110, 205], [150, 315], [150, 415], [150, 505], [700, 700]];
      let i = 0;
      const id = setInterval(() => {
        if (win.isDestroyed() || i >= pts.length) return;
        move(win.webContents, pts[i][0], pts[i][1]);
        i++;
      }, 400);
      return () => clearInterval(id);
    },
  });

  // ================= popups =================
  await stage('popup-window-open', {
    ms: 6000,
    html:
      '<body style="margin:0;background:#101010">' +
      '<a id=a href="#" style="position:absolute;left:' + BX + 'px;top:' + BY + 'px;color:#fff">open</a>' +
      '<script>window.__opened=null;document.getElementById("a").onclick=function(e){e.preventDefault();' +
      'try{var w=window.open("about:blank","_blank","width=400,height=300");window.__opened=!!w;' +
      'if(w){w.document.write("<body style=\\"margin:0;background:#e11\\"><h1>popup</h1>");w.document.close();}}' +
      'catch(err){window.__opened="ERR:"+err.message}};</script>',
    before: async (win, out) => {
      out.popup = { requests: [], created: false, childPaints: 0, childType: null, childOffscreen: null, childFrameSize: null };
      win.webContents.setWindowOpenHandler((d) => {
        out.popup.requests.push({
          url: String(d.url).slice(0, 120),
          frameName: d.frameName,
          disposition: d.disposition,
          features: String(d.features || '').slice(0, 120),
        });
        return {
          action: 'allow',
          overrideBrowserWindowOptions: {
            show: false,
            width: 400,
            height: 300,
            webPreferences: { offscreen: true, sandbox: true, contextIsolation: true, nodeIntegration: false },
          },
        };
      });
      win.webContents.on('did-create-window', (child) => {
        out.popup.created = true;
        try {
          out.popup.childType = child.webContents.getType();
          out.popup.childOffscreen = child.webContents.isOffscreen();
          child.webContents.setFrameRate(30);
          child.webContents.on('paint', (_d, _dirty, img) => {
            out.popup.childPaints++;
            if (!out.popup.childFrameSize && img && !img.isEmpty()) out.popup.childFrameSize = img.getSize();
          });
        } catch (e) {
          out.popup.childError = String(e.message);
        }
        setTimeout(() => { try { if (!child.isDestroyed()) child.destroy(); } catch (e) {} }, 3500);
      });
    },
    drive: async (win) => {
      setTimeout(() => { if (!win.isDestroyed()) click(win.webContents, BX + 10, BY + 8); }, 500);
      return null;
    },
    after: async (win, s, out) => ({ openedInPage: await js(win, 'String(window.__opened)'), popup: out.popup }),
  });

  // <select> renders as a SEPARATE Chromium widget (TextureInfo.widgetType can be 'popup').
  // If that widget is never composited into our frame, every dropdown on the web is
  // invisible in BlackGlass. Detected without screenshots by diffing the bitmap region
  // directly below the select, before and after the click.
  await stage('popup-select-dropdown', {
    ms: 6000,
    keepBitmap: true,
    html:
      '<body style="margin:0;background:#101010">' +
      '<select id=s style="position:absolute;left:' + BX + 'px;top:' + BY + 'px;width:200px;font:16px monospace">' +
      '<option>alpha</option><option>bravo</option><option>charlie</option><option>delta</option></select>' +
      EVLOG,
    drive: async (win, out, s) => {
      out.select = { clickedAtMs: 1500 };
      selectBefore = null;
      setTimeout(() => {
        if (win.isDestroyed()) return;
        // Freeze the last frame BEFORE the dropdown opens, then click to open it.
        selectBefore = s && s.lastBitmap ? Buffer.from(s.lastBitmap) : null;
        out.select.haveBeforeFrame = !!selectBefore;
        out.select.paintsBeforeClick = s ? s.paints : null;
        click(win.webContents, BX + 100, BY + 10);
      }, 1500);
      return null;
    },
    after: async (win, s, out) => {
      const res = { select: out.select || {} };
      res.select.eventLog = await js(win, 'JSON.stringify(window.__log||[])');
      res.select.paintsAtEnd = s ? s.paints : null;
      res.select.paintsCausedByClick =
        s && res.select.paintsBeforeClick != null ? s.paints - res.select.paintsBeforeClick : null;
      // Pixel evidence: a dropdown composited into our frame MUST change bytes in the
      // 200x150 region directly below the select.
      res.select.diffBelowSelect = regionDiff(
        selectBefore, s && s.lastBitmap, s && s.frameW, BX, BY + 30, 200, 150
      );
      // Control region far from the dropdown; should stay 0 on a static page.
      res.select.diffControlRegion = regionDiff(
        selectBefore, s && s.lastBitmap, s && s.frameW, 50, 50, 200, 150
      );
      res.select.partialRectsNearDropdown = s
        ? s.rects.filter((r) => r.y >= BY && r.y <= BY + 200 && r.r < PARTIAL).length
        : 0;
      res.select.verdict =
        res.select.diffBelowSelect === null
          ? 'INCONCLUSIVE (no bitmap pair; shared-texture mode has no NativeImage)'
          : res.select.diffBelowSelect > 0
          ? 'dropdown IS composited into the OSR frame'
          : 'dropdown is NOT composited into the OSR frame -- every <select> on the web would be invisible';
      selectBefore = null; // release the 5 MB copy
      return res;
    },
  });

  // ================= dialogs (opt-in; can HANG) =================
  // alert()/confirm() are native modals. On a hidden OSR window they may block the renderer
  // -- and a page that calls alert() would then wedge the whole engine. Off by default.
  if (DIALOGS) {
    await stage('dialog-alert-hang', {
      html: '<body style="margin:0;background:#fff"><p>dialog test</p>',
      ms: 1000,
      after: async (win) => {
        const t0 = Date.now();
        const r = await js(win, 'alert("bg"); "returned"', 5000);
        return { alert: { result: r, elapsedMs: Date.now() - t0, blocked: r && r.__error ? true : false } };
      },
    });
  }

  // ---- write out ----
  R.finishedAt = new Date().toISOString();
  const file = path.join(OUT, 'b02-' + MODE + '.json');
  fs.writeFileSync(file, JSON.stringify(R, null, 2));
  log('WROTE', file);
  console.log('__RESULT__' + JSON.stringify({
    mode: MODE,
    stages: Object.keys(R.stages).length,
    verdict: verdict(),
  }));
  app.exit(0);
});

// One-line machine-readable answer to the question the probe exists to settle.
function verdict() {
  const tiny = R.stages['damage-tiny-fps60'];
  const full = R.stages['damage-full-fps60'];
  if (!tiny) return 'damage stage not run';
  const tinyPartial = tiny.partialFraction;
  const onBox = tiny.rectsOnStimulusBox;
  if (tinyPartial > 0 && onBox > 0) {
    return 'PARTIAL DAMAGE CONFIRMED (bitmap path): min ratio ' + tiny.damageRatio.min +
      ', ' + onBox + ' rects on the stimulus box; full-canvas control p50 ' +
      (full ? full.damageRatio.p50 : 'n/a');
  }
  const tex = tiny.texture;
  if (tex && (tex.captureUpdateRectPartialFrames > 0 || tex.contentRectPartialFrames > 0)) {
    return 'BITMAP dirtyRect ALWAYS FULL, but shared-texture metadata reports partial damage ' +
      '(captureUpdateRect partial ' + tex.captureUpdateRectPartialFrames + ', contentRect partial ' +
      tex.contentRectPartialFrames + ') -- the information exists and is dropped in the bitmap path.';
  }
  return 'NO PARTIAL DAMAGE OBSERVED in mode=' + MODE + ' (paints ' + tiny.paints +
    ', min ratio ' + (tiny.damageRatio && tiny.damageRatio.min) + '). Self-computed damage required.';
}
```

---

## 7. How to run

Chromium child processes fail under the agent Bash sandbox with
`bootstrap_look_up ... Permission denied` (ADR-0001, "Environment note"), so this needs the
**harness** sandbox disabled. Chromium's own sandbox stays on (`sandbox: true` in `mkWin`).

```bash
cd /Users/adeebbashir/projects/blackglass/apps/engine

# 1) the shipping path -- this is the one that matters
./node_modules/.bin/electron spike/b02-capability-probe.js | tee spike/out/b02-bitmap.log

# 2) H3 cross-check: is damage present in Viz but dropped in the bitmap path?
./node_modules/.bin/electron spike/b02-capability-probe.js --mode=shared | tee spike/out/b02-shared.log

# 3) does software rendering change the damage story?
./node_modules/.bin/electron spike/b02-capability-probe.js --mode=software | tee spike/out/b02-software.log

# just the decisive stages (~25 s)
./node_modules/.bin/electron spike/b02-capability-probe.js \
  --only=damage-tiny-fps60,damage-full-fps60,damage-caret

# opt-in, may hang the process:
./node_modules/.bin/electron spike/b02-capability-probe.js --only=dialog-alert-hang --dialogs=1
```

Writes `spike/out/b02-<mode>.json` and prints one `__RESULT__{...}` line carrying the verdict.
**Runs 1 and 2 are the ones to do first;** together they separate H1/H2 from H3 from H6.

**Disk:** this probe downloads nothing and builds nothing — Electron 43.2.0 is already installed
at `apps/engine/node_modules/electron` (`dist/version` → `43.2.0`). Output is a few hundred KB of
JSON. Measured free space at design time was **5.5 GiB / 99% used** (`df -h`), which is *tighter
than the 9 GiB in the brief*; the constraint is worth re-checking before any other work, but it
does not block this probe.

---

## 8. Decision matrix — what each outcome costs us

Read `verdict` in the `__RESULT__` line, then confirm against `damage-tiny-fps60.rectSamples`.

### Outcome A — H1/H2 confirmed (partial rects in the bitmap path)

`damage-tiny-fps60` shows rects at ≈(600,400,40,40); `rectsOnStimulusBox > 0`; `damage-full-fps60`
still reads 1.0.

- A07's typing economics hold: 40×40 = 1,600 px = **6,400 B** raw versus **5,184,000 B** for a full
  frame — an **810× reduction** before any encoding. A 1×20 px caret is **80 B**.
- Required work: crop in `main.js` `onPaint` (§10). Until that lands the saving is **0**, because
  `main.js:97` sends the whole bitmap today.
- Check `fullWidthPartialHeight`: if damage is always full-width row strips (H2), that is *better*
  for us, not worse — contiguous rows are a single `subarray`, no per-row copy.
- Then check `damage-tiny-fps30` / `-fps10`. If the ratio climbs as fps drops (H5), **the SSH
  throttling plan in A07/A10 partially cancels its own benefit** and the adaptive controller must
  be redesigned to throttle *transmission*, not *capture*.

### Outcome B — H3 (bitmap always full, texture metadata partial)

`verdict` reports `captureUpdateRect partial N`. The information exists in Viz and is discarded on
the way to `NativeImage`.

- Options, cheapest first: (1) read damage from `--mode=shared` metadata while still taking pixels
  from the bitmap path — needs testing whether both are available on one window; (2) patch
  Electron's OSR consumer; (3) shared-texture mode plus a native addon, which ADR-0001 explicitly
  deferred.
- This is the outcome where the *reason* matters most, so keep the raw `captureUpdateRectSamples`.

### Outcome C — H6 (no partial damage anywhere)

- A07:208's "81–99% on typing, 568 B per keystroke" is **unreachable via Chromium's damage signal**
  and must be restated as conditional.
- Fallback: compute damage ourselves by tile-hashing the BGRA buffer. A07 already settled on 64×68
  tiles (`A07:221`), which is 396 tiles for 1440×900.
- Rough cost, **UNVERIFIED — this is arithmetic, not a measurement**: hashing 5,184,000 B at
  1–5 GB/s ≈ **1–5 ms/frame**, against A10 §0.1's measured full-frame `write()` of **10.8 ms p50 /
  20.1 ms p99**. Plausibly a net win, but it must be benchmarked before it is believed. It also
  costs a second 5 MB frame buffer for the previous frame.
- Caution on the obvious optimization: sampling every Nth row to cut hash cost will miss thin
  *horizontal* changes (a 1 px underline or border) while still catching a caret, which spans ~20
  rows. That is a correctness/latency trade, not a free win.

---

## 9. What this probe does **not** prove

Stated plainly so nobody over-reads the output.

1. **Real video decode.** The `video` stage proves codec *claims* (`canPlayType`,
   `MediaSource.isTypeSupported`, `mediaCapabilities`) and that `<video>` frames from a
   `captureStream` reach paint. It does **not** prove that a real H.264 file decodes, because
   that needs a media asset and the environment is disk- and network-constrained. Cover it with
   A10's `video-h264` corpus case (`A10:524`) once a small local fixture is committed.
2. **Real-world pages.** Every stimulus is synthetic and deliberately minimal, to isolate damage
   behaviour. Damage on a real page (compositing layers, iframes, scrolling) will differ, and A07
   already measured that a 1-px scroll dirties 78% of the tile grid (`A07:11`). Synthetic partial
   damage does **not** imply real-world partial damage.
3. **Other terminals or machines.** Results are specific to this Apple M4 / macOS 26.1 host and
   the GPU stack it reports. The `gpu` stage exists precisely so results can be compared later.
4. **iTerm2.** Out of scope here and still UNVERIFIED per the brief (macOS TCC blocks automation).
5. **Chromium's internal reason** for whatever we observe. H4 (frame-pool recycling) is a
   *hypothesis* about `FrameSinkVideoCapturerImpl` behaviour that I have **not** read the source
   for — Chromium source is not available in this environment and would violate the disk
   constraint to fetch. The probe detects the *pattern* (intermittent damage via
   `firstPartialIndex` and raw per-frame rects) without asserting the mechanism. Do not quote a
   mechanism we have not read.
6. **The `<select>` result is directional, not diagnostic.** A zero diff below the dropdown proves
   the pixels are not in our frame; it does not distinguish "native macOS menu rendered elsewhere"
   from "popup widget never composited". `eventLog` confirms the click landed, which rules out the
   third explanation (the click missed).

---

## 10. Requests to the commander (core files — I did not touch them)

Per the file-ownership rule these are described, not made.

1. **Correct `artifacts/swarm/A10-performance-plan.md:499.`** It asserts "ADR-0001 established that
   Electron OSR does report partial damage." ADR-0001:84-88 says the opposite. Suggested wording:
   *"ADR-0001 left partial damage unproven (see B02). This metric measures whether the signal
   exists at all, and whether we exploit it."* This matters beyond tidiness: A07's remote-viability
   conclusions inherit the optimistic reading.
2. **`apps/engine/src/main.js` — the crop that makes damage pay.** Today `main.js:86` takes the
   full bitmap and `main.js:97` concatenates all of it; the dirty rect is written into the header
   at `main.js:91-94` and then ignored, so partial damage currently saves **zero** bytes. If
   Outcome A holds, `onPaint` needs to emit only the dirty sub-rectangle. Two notes for whoever
   writes it: the wire header already carries the rect, so the format may not need to change; and
   because `image` is always the whole frame (`electron.d.ts:17217-17219`), the crop saves wire
   bytes only, never the `toBitmap()` copy.
3. **Decide the rect's coordinate space before writing that crop.** `dpr-2x` answers it. Guessing
   here produces a silently offset crop that will read as a rendering bug.

---

## 11. Provenance

- Electron/Chromium API facts: `apps/engine/node_modules/electron/electron.d.ts` @ 43.2.0, cited
  line-by-line in §5. Local primary source.
- Prior measurements: `apps/engine/spike/out/report.json`, `docs/adr/ADR-0001-browser-engine.md`,
  `artifacts/swarm/A07-ssh-remote.md`, `artifacts/swarm/A10-performance-plan.md`.
- Probe verification: `node --check`; 20/20 stage configs walked against a stubbed `electron`;
  16/16 inline page scripts parsed via `new Function`; `data:` URLs decoded and inspected;
  `regionDiff` and `summarize` unit-tested against synthetic frames.
- **Not executed under real Electron.** Every expectation in §8 is a prediction.
