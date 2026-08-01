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
  // invisible in Terminal-Fenster. Detected without screenshots by diffing the bitmap region
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
