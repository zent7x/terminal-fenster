#!/usr/bin/env node
// End-to-end verification that input injection actually reaches the page.
//
// This deliberately does NOT need a terminal: it speaks the engine wire protocol directly
// over a unix socket, which is exactly what the Rust core does. That makes it runnable in
// CI, where no graphics-capable terminal exists.
//
// What it proves, by inspecting real pixels rather than trusting an event log:
//   1. click     -- a mouseDown/mouseUp at a known coordinate changes the pixel there
//   2. hover     -- a mouseMove triggers CSS :hover (a purely visual state)
//   3. typing    -- keyboard input inserts text into a focused field
//   4. scroll    -- a wheel event moves the document
//   5. coords    -- clicking target A does NOT activate target B (mapping is not degenerate)
//
// Run:  node tests/e2e/input-injection.js
'use strict';

const net = require('net');
const http = require('http');
const path = require('path');
const fs = require('fs');
const os = require('os');
const { spawn } = require('child_process');

const ENGINE_DIR = path.resolve(__dirname, '../../apps/engine');
const ELECTRON = path.join(ENGINE_DIR, 'node_modules/.bin/electron');
const MAIN = path.join(ENGINE_DIR, 'src/main.js');

const W = 800;
const H = 600;

const T_FRAME = 1;
const T_EVENT = 2;
const T_COMMAND = 10;

// Two 100x100 click targets at known positions, a hover box, a text field and a tall
// scroll region. Every element sits at a coordinate we assert against.
const PAGE = `<!doctype html><html><body style="margin:0;background:#ffffff">
<div id="a" style="position:absolute;left:0;top:0;width:100px;height:100px;background:#ff0000"></div>
<div id="b" style="position:absolute;left:200px;top:0;width:100px;height:100px;background:#0000ff"></div>
<div id="h"></div>
<!-- The base colour MUST live in the stylesheet, not an inline style attribute: an inline
     style has specificity 1000 and would outrank the #h:hover rule (110), so hover could
     never apply no matter how correct the input pipeline is. -->
<style>
  #h{position:absolute;left:400px;top:0;width:100px;height:100px;background:#888888}
  #h:hover{background:#00ff00}
</style>
<input id="t" style="position:absolute;left:0;top:200px;width:300px;height:40px;font-size:20px;background:#ffffff;border:1px solid #000">
<div style="position:absolute;top:300px;left:0;width:100%;height:40px;background:#ffff00" id="marker"></div>
<div style="height:4000px"></div>
<script>
  document.getElementById('a').onclick = () => { document.getElementById('a').style.background = '#00ff00'; };
  document.getElementById('b').onclick = () => { document.getElementById('b').style.background = '#00ff00'; };
  document.getElementById('t').addEventListener('input', e => {
    document.body.style.background = e.target.value === 'hello' ? '#00ff00' : '#ffffff';
  });
</script>
</body></html>`;

const SECOND_PAGE = `<!doctype html><html><head><title>Second Tab</title></head>
<body style="margin:0;background:#ff00ff"><div style="color:#000;font:30px sans-serif">TAB TWO</div></body></html>`;

function encodeMessage(type, payload) {
  const head = Buffer.allocUnsafe(5);
  head.writeUInt8(type, 0);
  head.writeUInt32BE(payload.length, 1);
  return Buffer.concat([head, payload]);
}

class Harness {
  constructor() {
    this.frames = [];
    this.events = [];
    this.latest = null;
    this.cdpPending = new Map();
    this.cdpId = 0;
  }

  async start() {
    this.dir = fs.mkdtempSync(path.join(os.tmpdir(), 'bg-e2e-'));
    this.sockPath = path.join(this.dir, 'e.sock');
    this.httpServer = http.createServer((req, res) => {
      res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
      res.end(req.url === '/tab-two' ? SECOND_PAGE : PAGE);
    });
    await new Promise((resolve, reject) => {
      this.httpServer.once('error', reject);
      this.httpServer.listen(0, '127.0.0.1', resolve);
    });
    const address = this.httpServer.address();
    const url = `http://127.0.0.1:${address.port}/`;
    this.secondUrl = `http://127.0.0.1:${address.port}/tab-two`;

    let connectTimer;
    const connected = new Promise((resolve, reject) => {
      this.server = net.createServer((sock) => {
        clearTimeout(connectTimer);
        this.sock = sock;
        this._attach(sock);
        // Deliberately race a resize against BrowserWindow creation. Real terminals do this
        // while settling a newly opened window; the engine must retain it before `ready`.
        sock.write(encodeMessage(
          T_COMMAND,
          Buffer.from(JSON.stringify({ t: 'resize', w: W, h: H }), 'utf8')
        ));
        resolve();
      });
      this.server.listen(this.sockPath);
      connectTimer = setTimeout(() => reject(new Error('engine did not connect in 30s')), 30000);
    });

    this.child = spawn(
      ELECTRON,
      [
        MAIN,
        `--tf-socket=${this.sockPath}`,
        '--tf-width=640',
        '--tf-height=480',
        `--tf-url=${url}`,
        `--user-data-dir=${path.join(this.dir, 'profile')}`,
      ],
      { stdio: ['ignore', 'ignore', 'pipe'] }
    );
    this.stderr = '';
    this.child.stderr.on('data', (chunk) => {
      this.stderr = (this.stderr + chunk.toString('utf8')).slice(-16384);
    });
    await connected;
  }

  _attach(sock) {
    let buf = Buffer.alloc(0);
    sock.on('data', (chunk) => {
      buf = buf.length ? Buffer.concat([buf, chunk]) : chunk;
      for (;;) {
        if (buf.length < 5) return;
        const type = buf.readUInt8(0);
        const len = buf.readUInt32BE(1);
        if (buf.length < 5 + len) return;
        const payload = buf.subarray(5, 5 + len);
        buf = buf.subarray(5 + len);
        if (type === T_FRAME) {
          this._compositeFrame(payload);
        } else if (type === T_EVENT) {
          const event = JSON.parse(payload.toString('utf8'));
          if (event.t === 'cdpResult' && this.cdpPending.has(event.id)) {
            const pending = this.cdpPending.get(event.id);
            clearTimeout(pending.timer);
            this.cdpPending.delete(event.id);
            if (event.error) pending.reject(new Error(event.error.message));
            else pending.resolve(event.result || {});
          } else {
            this.events.push(event);
          }
        }
      }
    });
  }

  // The engine now sends only the changed rectangle (damage tracking), so this harness must
  // do exactly what the Rust core does: keep a persistent full-frame BGRA buffer and blit
  // each dirty rect into it. Asserting on `this.fb` then still means "the composited page",
  // which is what every pixel check below intends.
  _compositeFrame(payload) {
    const width = payload.readUInt32BE(4);
    const height = payload.readUInt32BE(8);
    const dx = payload.readUInt32BE(12);
    const dy = payload.readUInt32BE(16);
    const dw = payload.readUInt32BE(20);
    const dh = payload.readUInt32BE(24);
    const pixels = payload.subarray(32);

    if (!this.fb || this.width !== width || this.height !== height) {
      this.fb = Buffer.alloc(width * height * 4);
      this.width = width;
      this.height = height;
    }
    const srcStride = dw * 4;
    const dstStride = width * 4;
    for (let row = 0; row < dh; row++) {
      const src = row * srcStride;
      const dst = (dy + row) * dstStride + dx * 4;
      pixels.copy(this.fb, dst, src, src + srcStride);
    }
    this.latest = { width, height, pixels: this.fb };
    this.frames.push({ dx, dy, dw, dh });
  }

  send(obj) {
    this.sock.write(encodeMessage(T_COMMAND, Buffer.from(JSON.stringify(obj), 'utf8')));
  }

  /// Read a pixel from the most recent frame. Chromium delivers BGRA.
  pixel(x, y) {
    if (!this.latest) throw new Error('no frame yet');
    const { width, pixels } = this.latest;
    const i = (y * width + x) * 4;
    return { b: pixels[i], g: pixels[i + 1], r: pixels[i + 2], a: pixels[i + 3] };
  }

  isGreen(x, y) {
    const p = this.pixel(x, y);
    return p.g > 200 && p.r < 100 && p.b < 100;
  }

  isYellow(x, y) {
    const p = this.pixel(x, y);
    return p.r > 200 && p.g > 200 && p.b < 100;
  }

  async settle(ms = 700) {
    await new Promise((r) => setTimeout(r, ms));
  }

  async waitFor(predicate, timeoutMs, label) {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      if (predicate()) return true;
      await this.settle(50);
    }
    throw new Error(`timed out waiting for ${label}${this.stderr ? `\nengine stderr:\n${this.stderr}` : ''}`);
  }

  async waitForStats(predicate, timeoutMs, label) {
    const deadline = Date.now() + timeoutMs;
    let latest = null;
    while (Date.now() < deadline) {
      latest = await this.engineStats();
      if (predicate(latest)) return latest;
      await this.settle(100);
    }
    throw new Error(`timed out waiting for ${label}; last stats=${JSON.stringify(latest)}`);
  }

  async engineStats() {
    const start = this.events.length;
    this.send({ t: 'stats' });
    for (let i = 0; i < 50; i++) {
      const found = this.events.slice(start).find((e) => e.t === 'stats');
      if (found) return found;
      await this.settle(20);
    }
    throw new Error('timed out waiting for engine stats');
  }

  cdp(method, params = {}) {
    const id = ++this.cdpId;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.cdpPending.delete(id);
        reject(new Error(`timed out waiting for CDP ${method}`));
      }, 5000);
      this.cdpPending.set(id, { resolve, reject, timer });
      this.send({ t: 'cdp', id, method, params });
    });
  }

  async stop() {
    try { this.send({ t: 'quit' }); } catch (e) {}
    if (this.child && this.child.exitCode === null && this.child.signalCode === null) {
      let exited = false;
      const exit = new Promise((resolve) => this.child.once('exit', () => { exited = true; resolve(); }));
      await Promise.race([exit, this.settle(1000)]);
      if (!exited) {
        try { this.child.kill(); } catch (e) {}
        await Promise.race([exit, this.settle(500)]);
      }
    }
    try { this.sock.end(); } catch (e) {}
    try { this.server.close(); } catch (e) {}
    try { this.httpServer.close(); } catch (e) {}
    try { fs.rmSync(this.dir, { recursive: true, force: true }); } catch (e) {}
  }
}

const results = [];
function check(name, pass, detail) {
  results.push({ name, pass, detail });
  console.log(`${pass ? 'PASS' : 'FAIL'}  ${name}${detail ? '  -- ' + detail : ''}`);
}

let activeHarness = null;

(async () => {
  const h = activeHarness = new Harness();
  await h.start();
  await h.waitFor(() => h.frames.length > 0, 10000, 'the first real frame');

  check('engine emits frames', h.frames.length > 0, `${h.frames.length} frames`);
  check(
    'startup resize race resolves to the requested viewport',
    h.latest && h.latest.width === W && h.latest.height === H,
    h.latest ? `${h.latest.width}x${h.latest.height}` : 'none'
  );

  const idleStats = await h.waitForStats((stats) => stats.rate === 1, 5000, 'idle paint pacing');
  check(
    'static pages throttle the offscreen compositor to idle',
    idleStats.rate === 1,
    `rate=${idleStats.rate}`
  );

  h.send({ t: 'input', kind: 'mouse', action: 'move', x: 50, y: 50 });
  const activeStats = await h.engineStats();
  check(
    'input wakes the compositor before injection',
    activeStats.rate > idleStats.rate,
    `rate=${activeStats.rate}`
  );

  // --- 1. click target A ---------------------------------------------------------
  const aBefore = h.pixel(50, 50);
  h.send({ t: 'input', kind: 'mouse', action: 'move', x: 50, y: 50 });
  h.send({ t: 'input', kind: 'mouse', action: 'down', x: 50, y: 50, button: 'left', clickCount: 1 });
  h.send({ t: 'input', kind: 'mouse', action: 'up', x: 50, y: 50, button: 'left', clickCount: 1 });
  await h.waitFor(() => h.isGreen(50, 50), 5000, 'target A click repaint');
  check(
    'click changes the pixel under the cursor',
    h.isGreen(50, 50),
    `before rgb(${aBefore.r},${aBefore.g},${aBefore.b}) after rgb(${h.pixel(50, 50).r},${h.pixel(50, 50).g},${h.pixel(50, 50).b})`
  );

  // --- 2. coordinate mapping is not degenerate -----------------------------------
  // Target B must still be blue: clicking A must not have activated it.
  const b = h.pixel(250, 50);
  check(
    'clicking target A did not activate target B',
    b.b > 200 && b.g < 100,
    `B is rgb(${b.r},${b.g},${b.b})`
  );

  // Now click B specifically and confirm it responds -- proving x maps correctly.
  h.send({ t: 'input', kind: 'mouse', action: 'move', x: 250, y: 50 });
  h.send({ t: 'input', kind: 'mouse', action: 'down', x: 250, y: 50, button: 'left', clickCount: 1 });
  h.send({ t: 'input', kind: 'mouse', action: 'up', x: 250, y: 50, button: 'left', clickCount: 1 });
  await h.waitFor(() => h.isGreen(250, 50), 5000, 'target B click repaint');
  check('click at a different x activates the correct target', h.isGreen(250, 50));

  // --- 3. hover ------------------------------------------------------------------
  h.send({ t: 'input', kind: 'mouse', action: 'move', x: 450, y: 50 });
  // Hosted offscreen rendering can deliver the CSS hover repaint later than the nominal
  // scheduler delay, and a lone synthetic move can be coalesced or land before the element is
  // hit-testable on a cold compositor -- so :hover never latches. Re-assert the move each poll
  // (with a 1 px jiggle for a real pointer delta) and synchronize on the visual contract itself.
  await h.waitFor(() => {
    h.send({ t: 'input', kind: 'mouse', action: 'move', x: 449, y: 50 });
    h.send({ t: 'input', kind: 'mouse', action: 'move', x: 450, y: 50 });
    return h.isGreen(450, 50);
  }, 5000, 'the CSS hover repaint');
  check('mouseMove triggers CSS :hover', h.isGreen(450, 50));

  // --- 4. typing -----------------------------------------------------------------
  h.send({ t: 'input', kind: 'mouse', action: 'down', x: 100, y: 220, button: 'left', clickCount: 1 });
  h.send({ t: 'input', kind: 'mouse', action: 'up', x: 100, y: 220, button: 'left', clickCount: 1 });
  await h.settle(300);
  for (const ch of 'hello') {
    h.send({ t: 'input', kind: 'key', action: 'press', keyCode: ch, text: ch });
  }
  await h.waitFor(() => h.isGreen(700, 500), 5000, 'the typed-value page repaint');
  // The page turns the body green only when the field contains exactly "hello",
  // so this asserts ordered character delivery, not just "something happened".
  check('typing inserts text in order into the focused field', h.isGreen(700, 500));

  // --- 5. scroll -----------------------------------------------------------------
  // The yellow marker sits at y=300. After scrolling down it must move up.
  const wasYellow = h.isYellow(600, 310);
  h.send({ t: 'input', kind: 'mouse', action: 'wheel', x: 400, y: 400, deltaX: 0, deltaY: -400 });
  await h.waitFor(() => !h.isYellow(600, 310), 5000, 'the document scroll repaint');
  const stillYellow = h.isYellow(600, 310);
  check(
    'wheel event scrolls the document',
    wasYellow && !stillYellow,
    `marker before yellow=${wasYellow} after yellow=${stillYellow}`
  );
  const scrollState = await h.cdp('Runtime.evaluate', {
    expression: 'scrollY',
    returnByValue: true,
  });
  const scrollY = scrollState.result && scrollState.result.value;
  check('wheel advances the page scroll offset', scrollY > 0, `scrollY=${scrollY}`);

  // --- 6. tabs -------------------------------------------------------------------
  const newTabStart = h.events.length;
  h.send({ t: 'tabNew', url: h.secondUrl });
  await h.waitFor(
    () => h.events.slice(newTabStart).some((e) => e.t === 'tabs' && e.n === 2 && e.active === 1),
    5000,
    'the second tab to become active'
  );
  await h.waitFor(() => {
    const pixel = h.pixel(700, 500);
    return pixel.r > 200 && pixel.b > 200 && pixel.g < 100;
  }, 5000, 'the second tab full repaint');
  check('new tab becomes active and paints its own full page', true);

  const secondLocation = await h.cdp('Runtime.evaluate', {
    expression: 'location.pathname',
    returnByValue: true,
  });
  check(
    'automation follows the active tab',
    secondLocation.result.value === '/tab-two',
    secondLocation.result.value
  );

  const switchStart = h.events.length;
  h.send({ t: 'tabSwitch', index: 0 });
  await h.waitFor(
    () => h.events.slice(switchStart).some((e) => e.t === 'tabs' && e.n === 2 && e.active === 0),
    5000,
    'the first tab to become active again'
  );
  await h.waitFor(() => h.isGreen(50, 50), 5000, 'the preserved first-tab pixels');
  check('switching tabs restores preserved page state without stale pixels', h.isGreen(50, 50));

  const closeStart = h.events.length;
  h.send({ t: 'tabClose', index: 1 });
  await h.waitFor(
    () => h.events.slice(closeStart).some((e) => e.t === 'tabs' && e.n === 1 && e.active === 0),
    5000,
    'the background tab to close'
  );
  check('background tabs close without terminating the browser', true);

  // --- 7. lifecycle events -------------------------------------------------------
  check(
    'engine reported ready with version info',
    h.events.some((e) => e.t === 'ready' && e.chrome),
    h.events.find((e) => e.t === 'ready')?.chrome
  );

  const permissionStart = h.events.length;
  const permissionResult = await h.cdp('Runtime.evaluate', {
    expression: `new Promise((resolve) => navigator.geolocation.getCurrentPosition(
      () => resolve('granted'),
      (error) => resolve('denied:' + error.code)
    ))`,
    awaitPromise: true,
    returnByValue: true,
  });
  const permissionValue = permissionResult.result && permissionResult.result.value;
  const permissionEvent = h.events.slice(permissionStart).find(
    (e) => e.t === 'permissionDenied' && e.permission === 'geolocation'
  );
  check(
    'privileged page permissions are denied and reported',
    typeof permissionValue === 'string' && permissionValue.startsWith('denied:') && !!permissionEvent,
    `${permissionValue || 'no result'}; event=${permissionEvent ? permissionEvent.permission : 'none'}`
  );

  const navigationStart = h.events.length;
  const beforeExternal = await h.cdp('Runtime.evaluate', {
    expression: 'location.href',
    returnByValue: true,
  });
  await h.cdp('Runtime.evaluate', {
    expression: `location.href = 'terminal-fenster-external-test://should-not-launch'`,
    returnByValue: true,
  });
  await h.settle(250);
  const afterExternal = await h.cdp('Runtime.evaluate', {
    expression: 'location.href',
    returnByValue: true,
  });
  const navigationEvent = h.events.slice(navigationStart).find(
    (e) => e.t === 'navigationBlocked' && e.url.startsWith('terminal-fenster-external-test:')
  );
  check(
    'external application protocols are blocked without leaving the page',
    beforeExternal.result.value === afterExternal.result.value && !!navigationEvent,
    navigationEvent ? navigationEvent.url : 'no blocked-navigation event'
  );

  const zoomEventStart = h.events.length;
  h.send({ t: 'zoom', factor: 1.25 });
  await h.settle(200);
  const zoomEvent = h.events.slice(zoomEventStart).find((e) => e.t === 'zoom');
  check(
    'page zoom command is applied and acknowledged',
    zoomEvent && Math.abs(zoomEvent.factor - 1.25) < 0.001,
    zoomEvent ? `factor=${zoomEvent.factor}` : 'no zoom event'
  );
  h.send({ t: 'zoom', factor: 1 });
  await h.settle(100);

  h.send({ t: 'visibility', visible: false });
  const hiddenStats = await h.engineStats();
  check(
    'terminal focus loss gates output and lowers paint rate',
    hiddenStats.visible === false && hiddenStats.rate <= 10,
    `visible=${hiddenStats.visible} rate=${hiddenStats.rate}`
  );
  h.send({ t: 'visibility', visible: true });

  await h.stop();
  activeHarness = null;

  const failed = results.filter((r) => !r.pass);
  console.log(`\n${results.length - failed.length}/${results.length} checks passed`);
  fs.writeFileSync(
    path.join(__dirname, 'input-injection-results.json'),
    JSON.stringify({ when: new Date().toISOString(), results }, null, 2)
  );
  process.exit(failed.length ? 1 : 0);
})().catch(async (e) => {
  console.error('harness error:', e);
  if (activeHarness) await activeHarness.stop();
  process.exit(2);
});
