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
  }

  async start() {
    this.dir = fs.mkdtempSync(path.join(os.tmpdir(), 'bg-e2e-'));
    this.sockPath = path.join(this.dir, 'e.sock');
    const url = 'data:text/html;charset=utf-8,' + encodeURIComponent(PAGE);

    const connected = new Promise((resolve, reject) => {
      this.server = net.createServer((sock) => {
        this.sock = sock;
        this._attach(sock);
        resolve();
      });
      this.server.listen(this.sockPath);
      setTimeout(() => reject(new Error('engine did not connect in 30s')), 30000);
    });

    this.child = spawn(
      ELECTRON,
      [MAIN, `--bg-socket=${this.sockPath}`, `--bg-width=${W}`, `--bg-height=${H}`, `--bg-url=${url}`],
      { stdio: 'ignore' }
    );
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
          const width = payload.readUInt32BE(4);
          const height = payload.readUInt32BE(8);
          this.latest = { width, height, pixels: payload.subarray(32) };
          this.frames.push(this.latest);
        } else if (type === T_EVENT) {
          this.events.push(JSON.parse(payload.toString('utf8')));
        }
      }
    });
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

  async settle(ms = 700) {
    await new Promise((r) => setTimeout(r, ms));
  }

  stop() {
    try { this.send({ t: 'quit' }); } catch (e) {}
    try { this.child.kill(); } catch (e) {}
    try { this.server.close(); } catch (e) {}
    try { fs.rmSync(this.dir, { recursive: true, force: true }); } catch (e) {}
  }
}

const results = [];
function check(name, pass, detail) {
  results.push({ name, pass, detail });
  console.log(`${pass ? 'PASS' : 'FAIL'}  ${name}${detail ? '  -- ' + detail : ''}`);
}

(async () => {
  const h = new Harness();
  await h.start();
  await h.settle(2500);

  check('engine emits frames', h.frames.length > 0, `${h.frames.length} frames`);
  check(
    'frame geometry matches requested viewport',
    h.latest && h.latest.width === W && h.latest.height === H,
    h.latest ? `${h.latest.width}x${h.latest.height}` : 'none'
  );

  // --- 1. click target A ---------------------------------------------------------
  const aBefore = h.pixel(50, 50);
  h.send({ t: 'input', kind: 'mouse', action: 'move', x: 50, y: 50 });
  h.send({ t: 'input', kind: 'mouse', action: 'down', x: 50, y: 50, button: 'left', clickCount: 1 });
  h.send({ t: 'input', kind: 'mouse', action: 'up', x: 50, y: 50, button: 'left', clickCount: 1 });
  await h.settle();
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
  await h.settle();
  check('click at a different x activates the correct target', h.isGreen(250, 50));

  // --- 3. hover ------------------------------------------------------------------
  h.send({ t: 'input', kind: 'mouse', action: 'move', x: 450, y: 50 });
  await h.settle();
  check('mouseMove triggers CSS :hover', h.isGreen(450, 50));

  // --- 4. typing -----------------------------------------------------------------
  h.send({ t: 'input', kind: 'mouse', action: 'down', x: 100, y: 220, button: 'left', clickCount: 1 });
  h.send({ t: 'input', kind: 'mouse', action: 'up', x: 100, y: 220, button: 'left', clickCount: 1 });
  await h.settle(300);
  for (const ch of 'hello') {
    h.send({ t: 'input', kind: 'key', action: 'press', keyCode: ch, text: ch });
  }
  await h.settle();
  // The page turns the body green only when the field contains exactly "hello",
  // so this asserts ordered character delivery, not just "something happened".
  check('typing inserts text in order into the focused field', h.isGreen(700, 500));

  // --- 5. scroll -----------------------------------------------------------------
  // The yellow marker sits at y=300. After scrolling down it must move up.
  const markerBefore = h.pixel(600, 310);
  const wasYellow = markerBefore.r > 200 && markerBefore.g > 200 && markerBefore.b < 100;
  h.send({ t: 'input', kind: 'mouse', action: 'wheel', x: 400, y: 400, deltaX: 0, deltaY: -400 });
  await h.settle();
  const markerAfter = h.pixel(600, 310);
  const stillYellow = markerAfter.r > 200 && markerAfter.g > 200 && markerAfter.b < 100;
  check(
    'wheel event scrolls the document',
    wasYellow && !stillYellow,
    `marker before yellow=${wasYellow} after yellow=${stillYellow}`
  );

  // --- 6. lifecycle events -------------------------------------------------------
  check(
    'engine reported ready with version info',
    h.events.some((e) => e.t === 'ready' && e.chrome),
    h.events.find((e) => e.t === 'ready')?.chrome
  );

  h.stop();

  const failed = results.filter((r) => !r.pass);
  console.log(`\n${results.length - failed.length}/${results.length} checks passed`);
  fs.writeFileSync(
    path.join(__dirname, 'input-injection-results.json'),
    JSON.stringify({ when: new Date().toISOString(), results }, null, 2)
  );
  process.exit(failed.length ? 1 : 0);
})().catch((e) => {
  console.error('harness error:', e);
  process.exit(2);
});
