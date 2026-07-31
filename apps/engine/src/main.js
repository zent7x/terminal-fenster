// BlackGlass Chromium engine host.
//
// Runs an offscreen BrowserWindow and speaks a length-prefixed binary protocol to the Rust
// terminal core over a Unix domain socket.
//
//   [u8 type][u32 BE length][payload]
//
//   type 1  engine -> core : FRAME   (binary header + BGRA pixels)
//   type 2  engine -> core : EVENT   (JSON: title/url/loading/crash)
//   type 10 core  -> engine : COMMAND (JSON: navigate/resize/input/quit)
//
// Security posture: the Chromium sandbox stays ON. Web content never gets Node integration,
// context isolation is enforced, and the engine only ever connects to a local socket path
// handed to it by its parent -- it opens no listening port of its own.
'use strict';

const { app, BrowserWindow } = require('electron');
const net = require('net');

const SOCKET_PATH = process.argv.find((a) => a.startsWith('--bg-socket='))?.slice(12);
const INITIAL_W = parseInt(process.argv.find((a) => a.startsWith('--bg-width='))?.slice(11) || '1280', 10);
const INITIAL_H = parseInt(process.argv.find((a) => a.startsWith('--bg-height='))?.slice(12) || '800', 10);
const INITIAL_URL = process.argv.find((a) => a.startsWith('--bg-url='))?.slice(9) || 'about:blank';

if (!SOCKET_PATH) {
  console.error('[engine] fatal: --bg-socket=<path> is required');
  process.exit(2);
}

const T_FRAME = 1;
const T_EVENT = 2;
const T_COMMAND = 10;

// Electron quits the app when the last window closes. We manage lifetime ourselves;
// without this, closing a tab would kill the engine and orphan the terminal core.
app.on('window-all-closed', () => {});

let sock = null;
let win = null;
let seq = 0;

// --- Backpressure -----------------------------------------------------------------
// A terminal (especially over SSH) can be far slower than a 60 fps compositor. If we
// queued every frame, memory would grow without bound and the user would watch an
// ever-more-stale page. Instead we keep at most ONE frame in flight and coalesce:
// while a write is draining, later frames overwrite the pending one. The newest frame
// always wins, which is the correct trade for interactivity.
let writeInFlight = false;
let pendingFrame = null;
const stats = { produced: 0, sent: 0, coalesced: 0 };

function sendMessage(type, payload) {
  if (!sock || sock.destroyed) return false;
  const header = Buffer.allocUnsafe(5);
  header.writeUInt8(type, 0);
  header.writeUInt32BE(payload.length, 1);
  return sock.write(Buffer.concat([header, payload]));
}

function sendEvent(obj) {
  sendMessage(T_EVENT, Buffer.from(JSON.stringify(obj), 'utf8'));
}

function flushFrame() {
  if (writeInFlight || !pendingFrame) return;
  const frame = pendingFrame;
  pendingFrame = null;
  writeInFlight = true;
  stats.sent++;
  const ok = sendMessage(T_FRAME, frame);
  if (ok) {
    writeInFlight = false;
    if (pendingFrame) setImmediate(flushFrame);
  } else {
    sock.once('drain', () => {
      writeInFlight = false;
      flushFrame();
    });
  }
}

function onPaint(_event, dirty, image) {
  if (!sock || sock.destroyed) return;
  stats.produced++;
  const size = image.getSize();
  const bitmap = image.toBitmap(); // BGRA, 4 bytes/px, verified non-strided
  const head = Buffer.allocUnsafe(32);
  head.writeUInt32BE(seq++, 0);
  head.writeUInt32BE(size.width, 4);
  head.writeUInt32BE(size.height, 8);
  head.writeUInt32BE(dirty.x, 12);
  head.writeUInt32BE(dirty.y, 16);
  head.writeUInt32BE(dirty.width, 20);
  head.writeUInt32BE(dirty.height, 24);
  head.writeUInt32BE(0, 28); // format 0 = BGRA8888
  if (pendingFrame) stats.coalesced++;
  pendingFrame = Buffer.concat([head, bitmap]);
  flushFrame();
}

function createWindow(w, h) {
  const b = new BrowserWindow({
    show: false,
    width: w,
    height: h,
    webPreferences: {
      offscreen: true,
      // Hardening: web content must never reach Node or our privileged context.
      nodeIntegration: false,
      contextIsolation: true,
      sandbox: true,
      webSecurity: true,
    },
  });
  b.webContents.setFrameRate(60);
  b.webContents.on('paint', onPaint);
  b.webContents.on('page-title-updated', (_e, title) => sendEvent({ t: 'title', v: title }));
  b.webContents.on('did-navigate', (_e, url) => sendEvent({ t: 'url', v: url }));
  b.webContents.on('did-navigate-in-page', (_e, url) => sendEvent({ t: 'url', v: url }));
  b.webContents.on('did-start-loading', () => sendEvent({ t: 'loading', v: true }));
  b.webContents.on('did-stop-loading', () => sendEvent({ t: 'loading', v: false }));
  b.webContents.on('did-fail-load', (_e, code, desc, url) =>
    sendEvent({ t: 'loadError', code, desc, url })
  );
  b.webContents.on('render-process-gone', (_e, details) =>
    sendEvent({ t: 'crash', reason: details.reason, exitCode: details.exitCode })
  );
  // Popups: report them rather than silently opening an invisible window.
  b.webContents.setWindowOpenHandler(({ url }) => {
    sendEvent({ t: 'popup', url });
    return { action: 'deny' };
  });
  return b;
}

// --- Input injection --------------------------------------------------------------

// Tracks whether Chromium has been told the pointer is inside the view. Required for
// CSS :hover to activate in offscreen rendering.
let enteredOnce = false;

function modifierList(m) {
  const out = [];
  if (!m) return out;
  if (m.shift) out.push('shift');
  if (m.ctrl) out.push('control');
  if (m.alt) out.push('alt');
  if (m.meta) out.push('meta');
  return out;
}

function handleInput(cmd) {
  if (!win || win.isDestroyed()) return;
  const wc = win.webContents;
  const mods = modifierList(cmd.mods);

  if (cmd.kind === 'mouse') {
    const base = { x: Math.round(cmd.x), y: Math.round(cmd.y), modifiers: mods };
    switch (cmd.action) {
      case 'move':
        // CSS :hover does not activate from mouseMove alone in offscreen rendering: the
        // widget must first be told the pointer entered it. We latch that once and send
        // mouseEnter ahead of the first move, then plain moves thereafter.
        if (!enteredOnce) {
          enteredOnce = true;
          wc.sendInputEvent({ ...base, type: 'mouseEnter' });
        }
        wc.sendInputEvent({ ...base, type: 'mouseMove' });
        break;
      case 'enter':
        enteredOnce = true;
        wc.sendInputEvent({ ...base, type: 'mouseEnter' });
        break;
      case 'leave':
        enteredOnce = false;
        wc.sendInputEvent({ ...base, type: 'mouseLeave' });
        break;
      case 'down':
        wc.sendInputEvent({
          ...base,
          type: 'mouseDown',
          button: cmd.button || 'left',
          clickCount: cmd.clickCount || 1,
        });
        break;
      case 'up':
        wc.sendInputEvent({
          ...base,
          type: 'mouseUp',
          button: cmd.button || 'left',
          clickCount: cmd.clickCount || 1,
        });
        break;
      case 'wheel':
        wc.sendInputEvent({
          ...base,
          type: 'mouseWheel',
          deltaX: cmd.deltaX || 0,
          deltaY: cmd.deltaY || 0,
          canScroll: true,
        });
        break;
    }
    return;
  }

  if (cmd.kind === 'key') {
    // Electron wants keyDown/char/keyUp. `char` is what actually inserts text; sending
    // only keyDown types nothing for printable characters.
    if (cmd.action === 'press' || cmd.action === 'down') {
      wc.sendInputEvent({ type: 'keyDown', keyCode: cmd.keyCode, modifiers: mods });
      if (cmd.text) {
        for (const ch of cmd.text) {
          wc.sendInputEvent({ type: 'char', keyCode: ch, modifiers: mods });
        }
      }
      if (cmd.action === 'press') {
        wc.sendInputEvent({ type: 'keyUp', keyCode: cmd.keyCode, modifiers: mods });
      }
    } else if (cmd.action === 'up') {
      wc.sendInputEvent({ type: 'keyUp', keyCode: cmd.keyCode, modifiers: mods });
    }
  }
}

function handleCommand(cmd) {
  switch (cmd.t) {
    case 'navigate':
      if (win && !win.isDestroyed()) win.loadURL(cmd.url);
      break;
    case 'resize':
      if (win && !win.isDestroyed()) {
        win.setSize(Math.max(1, cmd.w), Math.max(1, cmd.h));
        // Force a repaint so the new geometry reaches the terminal immediately rather
        // than waiting for the page to happen to change.
        win.webContents.invalidate();
      }
      break;
    case 'input':
      handleInput(cmd);
      break;
    case 'reload':
      if (win && !win.isDestroyed()) win.webContents.reload();
      break;
    case 'back':
      if (win && !win.isDestroyed() && win.webContents.navigationHistory.canGoBack()) {
        win.webContents.navigationHistory.goBack();
      }
      break;
    case 'forward':
      if (win && !win.isDestroyed() && win.webContents.navigationHistory.canGoForward()) {
        win.webContents.navigationHistory.goForward();
      }
      break;
    case 'stats':
      sendEvent({ t: 'stats', ...stats });
      break;
    case 'quit':
      app.exit(0);
      break;
  }
}

// --- Socket framing ---------------------------------------------------------------

function attachReader(socket) {
  let buf = Buffer.alloc(0);
  socket.on('data', (chunk) => {
    buf = buf.length ? Buffer.concat([buf, chunk]) : chunk;
    for (;;) {
      if (buf.length < 5) return;
      const type = buf.readUInt8(0);
      const len = buf.readUInt32BE(1);
      if (buf.length < 5 + len) return;
      const payload = buf.subarray(5, 5 + len);
      buf = buf.subarray(5 + len);
      if (type === T_COMMAND) {
        try {
          handleCommand(JSON.parse(payload.toString('utf8')));
        } catch (e) {
          console.error('[engine] bad command:', e.message);
        }
      }
    }
  });
}

app.whenReady().then(() => {
  if (app.dock) app.dock.hide();
  sock = net.createConnection(SOCKET_PATH, () => {
    win = createWindow(INITIAL_W, INITIAL_H);
    sendEvent({
      t: 'ready',
      electron: process.versions.electron,
      chrome: process.versions.chrome,
      width: INITIAL_W,
      height: INITIAL_H,
    });
    if (INITIAL_URL && INITIAL_URL !== 'about:blank') win.loadURL(INITIAL_URL);
  });
  sock.on('error', (e) => {
    console.error('[engine] socket error:', e.message);
    app.exit(3);
  });
  // If the terminal core dies, the engine must not linger as an orphan holding a
  // Chromium process tree.
  sock.on('close', () => app.exit(0));
  attachReader(sock);
});
