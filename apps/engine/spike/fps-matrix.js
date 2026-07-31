// SPIKE: THE decisive experiment. Electron OSR paints only once per page in the default
// config, which would make this a screenshot tool, not a browser. Test the three candidate
// configurations against a page that genuinely repaints, and measure real paint throughput.
//
// Usage: electron fps-matrix.js <mode>
//   mode = gpu        -> default hardware-accelerated OSR
//   mode = software   -> app.disableHardwareAcceleration()
//   mode = shared     -> webPreferences.offscreen = { useSharedTexture: true }
//
// disableHardwareAcceleration is process-global and must run before app-ready, so each
// mode must be a separate process. Emits one JSON line for the harness to collect.
const { app, BrowserWindow } = require('electron');
const fs = require('fs');
const path = require('path');

const MODE = process.argv[2] || 'gpu';
const DURATION_MS = 5000;
const OUT = path.join(__dirname, 'out');
fs.mkdirSync(OUT, { recursive: true });

if (MODE === 'software') app.disableHardwareAcceleration();

app.on('window-all-closed', () => {});

// A page that forces genuine repaints every frame: canvas drawing + text mutation.
// Deliberately NOT a CSS transform, which is composited on the GPU thread and can
// produce zero software paints -- that flaw invalidated the first probe.
const PAGE = `<!doctype html><html><body style="margin:0;background:#111">
<canvas id=c width=1440 height=900></canvas>
<div id=t style="position:absolute;top:0;left:0;color:#0f0;font:24px monospace"></div>
<script>
const c=document.getElementById('c'),x=c.getContext('2d'),t=document.getElementById('t');
let n=0;
function f(){
  n++;
  x.fillStyle='hsl('+(n%360)+',80%,50%)';
  x.fillRect(0,0,1440,900);
  x.fillStyle='#000';
  x.fillRect((n*7)%1400,100,120,120);
  t.textContent='frame '+n+' '+Date.now();
  requestAnimationFrame(f);
}
f();
</script></body></html>`;

function makeWindow() {
  const offscreen = MODE === 'shared' ? { useSharedTexture: true } : true;
  return new BrowserWindow({
    show: false,
    width: 1440,
    height: 900,
    webPreferences: {
      offscreen,
      nodeIntegration: false,
      contextIsolation: true,
      sandbox: true,
    },
  });
}

app.whenReady().then(() => {
  if (app.dock) app.dock.hide();
  const result = { mode: MODE, electron: process.versions.electron, chrome: process.versions.chrome };
  let win;
  try {
    win = makeWindow();
  } catch (e) {
    result.error = 'window construction failed: ' + e.message;
    console.log('__RESULT__' + JSON.stringify(result));
    return app.exit(0);
  }

  let paints = 0;
  let textureFrames = 0;
  let bitmapFrames = 0;
  let first = null;
  let last = null;
  const gaps = [];
  const dirty = [];

  win.webContents.setFrameRate(60);
  win.webContents.on('paint', (event, dirtyRect, image) => {
    const now = process.hrtime.bigint();
    if (first === null) first = now; else gaps.push(Number(now - last) / 1e6);
    last = now;
    paints++;
    // In shared-texture mode the frame arrives as event.texture, not a NativeImage.
    if (event && event.texture) {
      textureFrames++;
      // Must release or the GPU process will stall on exhausted buffers.
      try { event.texture.release(); } catch (e) {}
    } else if (image) {
      bitmapFrames++;
    }
    if (dirtyRect && dirty.length < 20) dirty.push({ w: dirtyRect.width, h: dirtyRect.height });
  });

  win.webContents.on('did-finish-load', () => { result.loaded = true; });
  win.webContents.on('render-process-gone', (e, d) => { result.renderProcessGone = d; });

  win.loadURL('data:text/html;charset=utf-8,' + encodeURIComponent(PAGE));

  setTimeout(() => {
    const spanMs = first && last ? Number(last - first) / 1e6 : 0;
    gaps.sort((a, b) => a - b);
    const pct = (p) => (gaps.length ? +gaps[Math.min(gaps.length - 1, Math.floor(gaps.length * p))].toFixed(2) : null);
    result.paints = paints;
    result.textureFrames = textureFrames;
    result.bitmapFrames = bitmapFrames;
    result.spanMs = +spanMs.toFixed(1);
    result.fps = spanMs > 0 ? +((paints - 1) / (spanMs / 1000)).toFixed(1) : 0;
    result.frameGapMs = { p50: pct(0.5), p95: pct(0.95), p99: pct(0.99) };
    result.dirtySamples = dirty.slice(0, 6);
    result.partialDamage = dirty.filter((d) => d.w < 1440 || d.h < 900).length;
    console.log('__RESULT__' + JSON.stringify(result));
    app.exit(0);
  }, DURATION_MS);
});
