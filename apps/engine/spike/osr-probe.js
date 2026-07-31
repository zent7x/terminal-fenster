// SPIKE: Prove Electron OSR produces real pixels + determine exact bitmap format.
// Decisive tests:
//   1. Does 'paint' fire at all, headless, on macOS arm64?
//   2. What is the byte order of getBitmap()? (render pure red, inspect bytes)
//   3. Is alpha premultiplied? (render 50% alpha over nothing)
//   4. Time-to-first-paint for a real remote page.
//   5. Achievable frame rate on an animating page.
//   6. Dirty-rect semantics: does dirtyRect shrink for small changes?
const { app, BrowserWindow } = require('electron');
const fs = require('fs');
const path = require('path');

const OUT = path.join(__dirname, 'out');
fs.mkdirSync(OUT, { recursive: true });

const report = { electron: process.versions.electron, chrome: process.versions.chrome, node: process.versions.node, tests: {} };
const t0 = Date.now();

function log(...a) { console.log('[probe]', ...a); }

function mkWin(w, h) {
  return new BrowserWindow({
    show: false,
    width: w,
    height: h,
    webPreferences: {
      offscreen: true,
      nodeIntegration: false,
      contextIsolation: true,
      sandbox: true,
    },
  });
}

// Test 1+2+3: byte order and alpha, using a known solid color.
function testByteOrder() {
  return new Promise((resolve) => {
    const win = mkWin(64, 64);
    let done = false;
    win.webContents.on('paint', (event, dirty, image) => {
      if (done) return;
      const size = image.getSize();
      const bmp = image.toBitmap();
      // Pure red page. BGRA => [0,0,255,255]. RGBA => [255,0,0,255].
      const px = [bmp[0], bmp[1], bmp[2], bmp[3]];
      const order = px[0] === 255 && px[2] === 0 ? 'RGBA' : px[2] === 255 && px[0] === 0 ? 'BGRA' : `UNKNOWN(${px.join(',')})`;
      report.tests.byteOrder = {
        size,
        bitmapLength: bmp.length,
        expectedLength: size.width * size.height * 4,
        lengthMatches: bmp.length === size.width * size.height * 4,
        firstPixel: px,
        order,
        dirtyRect: dirty,
      };
      log('byte order:', order, 'first px', px, 'len', bmp.length, 'expect', size.width * size.height * 4);
      done = true;
      win.destroy();
      resolve();
    });
    win.loadURL('data:text/html,<body style="margin:0;background:%23FF0000">');
    setTimeout(() => { if (!done) { report.tests.byteOrder = { error: 'TIMEOUT - no paint event in 10s' }; done = true; try { win.destroy(); } catch (e) {} resolve(); } }, 10000);
  });
}

// Test 4: real remote page, time to first paint, save PNG for human/visual verification.
function testRealPage(url, tag, w, h) {
  return new Promise((resolve) => {
    const win = mkWin(w, h);
    const start = Date.now();
    let paints = 0;
    let firstPaintMs = null;
    let lastImage = null;
    win.webContents.setFrameRate(60);
    win.webContents.on('paint', (event, dirty, image) => {
      paints++;
      if (firstPaintMs === null) firstPaintMs = Date.now() - start;
      lastImage = image;
    });
    win.webContents.on('did-finish-load', () => {
      // give it a moment to paint post-load, then snapshot
      setTimeout(() => {
        if (lastImage) {
          const png = lastImage.toPNG();
          fs.writeFileSync(path.join(OUT, `${tag}.png`), png);
          report.tests[tag] = {
            url, viewport: { w, h },
            firstPaintMs,
            paintCount: paints,
            pngBytes: png.length,
            imageSize: lastImage.getSize(),
            rawBytes: lastImage.toBitmap().length,
          };
          log(tag, 'firstPaint', firstPaintMs + 'ms', 'paints', paints, 'png', png.length, 'raw', lastImage.toBitmap().length);
        } else {
          report.tests[tag] = { url, error: 'loaded but no paint' };
        }
        win.destroy();
        resolve();
      }, 1500);
    });
    win.webContents.on('did-fail-load', (e, code, desc) => {
      report.tests[tag] = { url, error: `did-fail-load ${code} ${desc}` };
      log(tag, 'FAILED', code, desc);
      try { win.destroy(); } catch (err) {}
      resolve();
    });
    win.loadURL(url);
    setTimeout(() => { if (!report.tests[tag]) { report.tests[tag] = { url, error: 'TIMEOUT 25s' }; try { win.destroy(); } catch (e) {} resolve(); } }, 25000);
  });
}

// Test 5+6: frame rate + dirty rect behavior on an animating page.
function testFrameRateAndDamage() {
  return new Promise((resolve) => {
    const win = mkWin(1440, 900);
    const dirtyRects = [];
    let paints = 0;
    let start = null;
    win.webContents.setFrameRate(60);
    win.webContents.on('paint', (event, dirty, image) => {
      if (start === null) start = Date.now();
      paints++;
      if (dirtyRects.length < 40) dirtyRects.push({ x: dirty.x, y: dirty.y, w: dirty.width, h: dirty.height });
    });
    // Small animating box in the corner -> if damage tracking works, dirtyRect should be small, NOT full-screen.
    const html = `<body style="margin:0;background:%23222"><div id=b style="position:absolute;left:20px;top:20px;width:80px;height:80px;background:%230f0"></div><script>let i=0;function f(){i++;document.getElementById('b').style.transform='translateX('+(i%40)+'px)';requestAnimationFrame(f)}f()<\/script>`;
    win.loadURL('data:text/html,' + html);
    setTimeout(() => {
      const durMs = Date.now() - (start || Date.now());
      const full = dirtyRects.filter(r => r.w >= 1400 && r.h >= 850).length;
      const small = dirtyRects.filter(r => r.w < 800).length;
      report.tests.frameRateAndDamage = {
        paints,
        durationMs: durMs,
        fps: durMs > 0 ? +(paints / (durMs / 1000)).toFixed(1) : null,
        sampledDirtyRects: dirtyRects.slice(0, 12),
        fullScreenDamageCount: full,
        partialDamageCount: small,
        damageTrackingWorks: small > 0,
      };
      log('fps', report.tests.frameRateAndDamage.fps, 'partial-damage frames', small, 'full-damage frames', full);
      win.destroy();
      resolve();
    }, 4000);
  });
}

// CRITICAL: Electron's DEFAULT behavior when the last window closes is to QUIT the app.
// Destroying a window between tests killed the main process, so the next test's renderer
// failed Mach port rendezvous ("parent died?"). Subscribing here takes over that decision.
app.on('window-all-closed', () => { /* keep the app alive between probe stages */ });

app.whenReady().then(async () => {
  if (app.dock) app.dock.hide();
  log('electron', process.versions.electron, 'chrome', process.versions.chrome);
  await testByteOrder();
  await testRealPage('https://example.com', 'example-com', 1440, 900);
  await testRealPage('https://github.com', 'github-com', 1440, 900);
  await testFrameRateAndDamage();
  report.totalMs = Date.now() - t0;
  fs.writeFileSync(path.join(OUT, 'report.json'), JSON.stringify(report, null, 2));
  log('WROTE', path.join(OUT, 'report.json'), 'in', report.totalMs + 'ms');
  app.exit(0);
});
