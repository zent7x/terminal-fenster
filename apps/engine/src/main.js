// Terminal-Fenster Chromium engine host.
//
// Runs an offscreen BrowserWindow and speaks a length-prefixed binary protocol to the Rust
// terminal core over a Unix domain socket.
//
//   [u8 type][u32 BE length][payload]
//
//   type 1  engine -> core : FRAME   (binary header + BGRA pixels)
//   type 2  engine -> core : EVENT   (JSON: title/url/loading/crash/cdpResult)
//   type 10 core  -> engine : COMMAND (JSON: navigate/resize/input/cdp/quit)
//
// Security posture: the Chromium sandbox stays ON. Web content never gets Node integration,
// context isolation is enforced, and the engine only ever connects to a local socket path
// handed to it by its parent -- it opens no listening port of its own.
'use strict';

const { app, BrowserWindow } = require('electron');
const net = require('net');
const { coalesceFrame, encodeFrameParts, fullDamage, newFrame } = require('./frame-pipeline');
const {
  TextureCaptureQueue,
  extractDamage,
  frameSize,
  releaseTexture,
  sharedTextureEnabled,
} = require('./frame-capture');
const { installDenyAllPermissions } = require('./security-policy');
const { TabManager } = require('./tabs');

// A stable, app-specific data root: with this, persistent profiles live under
// ".../terminal-fenster" instead of the generic shared "Electron" directory. This is Terminal-Fenster's
// own storage and never reads or writes the user's real Chrome profile.
app.setName('terminal-fenster');

const SOCKET_PATH = process.argv.find((a) => a.startsWith('--tf-socket='))?.slice(12);
const INITIAL_W = parseInt(process.argv.find((a) => a.startsWith('--tf-width='))?.slice(11) || '1280', 10);
const INITIAL_H = parseInt(process.argv.find((a) => a.startsWith('--tf-height='))?.slice(12) || '800', 10);
const INITIAL_URL = process.argv.find((a) => a.startsWith('--tf-url='))?.slice(9) || 'about:blank';
// A named, persistent profile. `persist:` makes cookies, localStorage and logins survive
// across runs, so signing in to a site once keeps you signed in. Different names are fully
// separate cookie jars (e.g. --profile work vs --profile personal).
const PROFILE = process.argv.find((a) => a.startsWith('--tf-profile='))?.slice(13) || 'default';
// MCP sessions are exposed to prompt-injected page content. They get a narrower navigation
// policy than an explicit human CLI action: no local files/blobs and capped data URLs.
const AGENT_MODE = process.argv.includes('--tf-agent-mode');
// Max offscreen paint rate. Lower it to spend less GPU/CPU (fewer Chromium readbacks and
// terminal redraws); raise it for smoother motion. Clamped to Electron's accepted 1..240.
const FRAME_RATE = (() => {
  const n = parseInt(process.argv.find((a) => a.startsWith('--tf-fps='))?.slice(9) || '120', 10);
  return Number.isFinite(n) ? Math.max(1, Math.min(240, n)) : 120;
})();
const SHARED_TEXTURE = sharedTextureEnabled(process.argv);
const LOW_RAM = process.argv.includes('--tf-low-ram') || process.env.TERMINAL_FENSTER_LOW_RAM === '1';
const textureCapture = SHARED_TEXTURE ? new TextureCaptureQueue() : null;

if (LOW_RAM) {
  app.commandLine.appendSwitch('disable-dev-shm-usage');
  app.commandLine.appendSwitch('disable-features', 'SpareRendererForSitePerProcess,Translate');
}

if (!SOCKET_PATH) {
  console.error('[engine] fatal: --tf-socket=<path> is required');
  process.exit(2);
}

const T_FRAME = 1;
const T_EVENT = 2;
const T_COMMAND = 10;
const MAX_COMMAND_LEN = 64 * 1024;

// Electron quits the app when the last window closes. We manage lifetime ourselves;
// without this, closing a tab would kill the engine and orphan the terminal core.
app.on('window-all-closed', () => {});

let sock = null;
let tabs = null;
let seq = 0;
// requestId of the most recent findInPage() call, so stale found-in-page results from a
// superseded query (fast typing fires a new find before the old one's async result lands)
// don't overwrite the count shown for the current query.
let currentFindRequestId = 0;
let requestedSize = { width: INITIAL_W, height: INITIAL_H };

// --- Frame scheduler (B07) --------------------------------------------------------
// Layer 1: at most ONE in-flight + ONE pending. Newest bitmap wins; dirty rects are
// unioned across coalesced paints (see frame-pipeline.js) so no change is withheld.
// Layer 2: visibility + idle pacing via setFrameRate (Electron floors at 1 fps).
let writeInFlight = false;
let pendingFrame = null;
let forceFullPaint = true;
let visible = true;
// Give startup a full active window before idle throttling is eligible. Zero would mean
// "quiet since system boot" and could drop a slow first navigation to 1 fps after 250 ms.
const schedulerStartedNs = nowNs();
let lastPaintNs = schedulerStartedNs;
let lastInputNs = schedulerStartedNs;
let curRate = FRAME_RATE;
// Cap from the CLI adaptive transport ladder (C09). Never raises above FRAME_RATE.
let transportFpsCap = FRAME_RATE;
const RATE_FG = FRAME_RATE;
const RATE_SCROLL = process.platform === 'darwin' ? Math.max(FRAME_RATE, 120) : FRAME_RATE;
const RATE_BG = Math.min(10, FRAME_RATE);
const RATE_IDLE = 1;
const IDLE_NS = 500_000_000; // 500 ms quiet ⇒ idle
const SCROLL_BOOST_NS = 1_500_000_000; // keep full fps briefly after wheel
let scrollBoostUntilNs = 0;
const stats = {
  produced: 0,
  sent: 0,
  coalesced: 0,
  resized: 0,
  full: 0,
  partial: 0,
  suppressed: 0,
};

function nowNs() {
  return Number(process.hrtime.bigint());
}

function applyRate() {
  const win = tabs?.activeWin;
  if (!win || win.isDestroyed()) return;
  const now = nowNs();
  let target;
  if (!visible) target = RATE_BG;
  else if (now < scrollBoostUntilNs) target = RATE_SCROLL;
  else if (now - lastPaintNs > IDLE_NS && now - lastInputNs > IDLE_NS) target = RATE_IDLE;
  else target = RATE_FG;
  target = Math.min(target, transportFpsCap);
  if (target !== curRate) {
    win.webContents.setFrameRate(target);
    curRate = target;
  }
}

// Write framing and payload as separate buffers under cork/uncork. net.Socket turns these into
// one writev without materialising another viewport-sized Buffer.
function sendMessageParts(type, parts) {
  if (!sock || sock.destroyed) return null;
  const payloadLength = parts.reduce((n, part) => n + part.length, 0);
  const header = Buffer.allocUnsafe(5);
  header.writeUInt8(type, 0);
  header.writeUInt32BE(payloadLength, 1);
  let accepted = true;
  sock.cork();
  try {
    if (!sock.write(header)) accepted = false;
    for (const part of parts) if (!sock.write(part)) accepted = false;
  } finally {
    sock.uncork();
  }
  return accepted;
}

function sendMessage(type, payload) {
  return sendMessageParts(type, [payload]);
}

function sendEvent(obj) {
  sendMessage(T_EVENT, Buffer.from(JSON.stringify(obj), 'utf8'));
}

function scheduleFlush() {
  if (writeInFlight || !pendingFrame) return;
  if (!visible) {
    // Keep coalescing into pending; do not write while the terminal is unfocused.
    stats.suppressed++;
    return;
  }
  flushFrame();
}

function flushFrame() {
  if (writeInFlight || !pendingFrame || !visible) return;
  const frame = pendingFrame;
  pendingFrame = null;
  writeInFlight = true;
  stats.sent++;
  const { head, pixels, isFull } = encodeFrameParts(frame);
  if (isFull) stats.full++;
  else stats.partial++;
  const ok = sendMessageParts(T_FRAME, [head, pixels]);
  if (ok === null) {
    writeInFlight = false;
    return;
  }
  if (ok) {
    writeInFlight = false;
    if (pendingFrame) setImmediate(scheduleFlush);
  } else {
    sock.once('drain', () => {
      writeInFlight = false;
      scheduleFlush();
    });
  }
}

function enqueueFrame(width, height, dirty, bitmap) {
  const next = newFrame(seq++, width, height, dirty, bitmap);
  const merged = coalesceFrame(pendingFrame, next);
  if (merged.coalesced) stats.coalesced++;
  if (merged.resized) stats.resized++;
  pendingFrame = merged.frame;
  scheduleFlush();
}

function onPaint(sourceWindow, event, dirty, image) {
  if (!sock || sock.destroyed) return;
  const win = tabs?.activeWin;
  if (!win || win.isDestroyed() || sourceWindow !== win) {
    // Shared textures are explicitly owned by the consumer. Inactive tabs still receive
    // occasional paints at 1 fps, and dropping those events without release leaks GPU memory.
    releaseTexture(event);
    return;
  }
  stats.produced++;
  lastPaintNs = nowNs();
  applyRate();

  if (SHARED_TEXTURE && event?.texture) {
    const [width, height] = win.getContentSize();
    const size = { width, height };
    const damage = forceFullPaint
      ? fullDamage(width, height)
      : extractDamage(event, dirty, width, height);
    releaseTexture(event);
    if (size.width < 1 || size.height < 1) {
      stats.suppressed++;
      return;
    }
    forceFullPaint = false;
    textureCapture.schedule(win, size, damage, enqueueFrame);
    return;
  }

  const size = image.getSize();
  const bitmap = image.toBitmap(); // BGRA, 4 bytes/px, verified non-strided
  // Electron can emit a transient 0x0 paint while a hidden OSR view is resizing. It
  // contains no pixels and cannot advance any consumer's retained framebuffer.
  if (size.width < 1 || size.height < 1 || bitmap.length !== size.width * size.height * 4) {
    stats.suppressed++;
    return;
  }

  // Delay cropping until flush. If backpressure folds multiple paints together, the latest
  // full bitmap is retained while all skipped damage is unioned into one complete update.
  const damage = forceFullPaint ? fullDamage(size.width, size.height) : dirty;
  forceFullPaint = false;
  enqueueFrame(size.width, size.height, damage, bitmap);
}

function createWindow(w, h, wireContents) {
  const b = new BrowserWindow({
    show: false,
    width: w,
    height: h,
    webPreferences: {
      offscreen: SHARED_TEXTURE ? { useSharedTexture: true } : true,
      // Named persistent profile: cookies/logins survive restarts and are isolated per name.
      partition: 'persist:' + PROFILE,
      // Hardening: web content must never reach Node or our privileged context.
      nodeIntegration: false,
      contextIsolation: true,
      sandbox: true,
      webSecurity: true,
    },
  });
  // The offscreen window has no native permission prompt. Deny every privileged web API until
  // terminal chrome can show the requesting origin and collect an explicit user decision.
  installDenyAllPermissions(b.webContents.session, sendEvent);
  b.webContents.setFrameRate(FRAME_RATE);
  b.webContents.on('paint', (event, dirty, image) => onPaint(b, event, dirty, image));
  b.webContents.on('found-in-page', (_e, result) => {
    if (result.requestId !== currentFindRequestId) return;
    sendEvent({
      t: 'find',
      active: result.activeMatchOrdinal || 0,
      matches: result.matches || 0,
      final: !!result.finalUpdate,
    });
  });
  wireContents(b);
  return b;
}

function onActiveTabChange(prev, next) {
  enteredOnce = false;
  pendingWheel = null;
  pendingFrame = null;
  forceFullPaint = true;
  if (prev && !prev.isDestroyed()) prev.webContents.setFrameRate(1);
  if (next && !next.isDestroyed()) next.webContents.setFrameRate(curRate);
}

function navigate(url) {
  if (!tabs) return;
  tabs.navigate(url);
}

// --- Input injection --------------------------------------------------------------

// Tracks whether Chromium has been told the pointer is inside the view. Required for
// CSS :hover to activate in offscreen rendering.
let enteredOnce = false;
let pendingWheel = null;

function modifierList(m) {
  const out = [];
  if (!m) return out;
  if (m.shift) out.push('shift');
  if (m.ctrl) out.push('control');
  if (m.alt) out.push('alt');
  if (m.meta) out.push('meta');
  return out;
}

function wakeInput() {
  lastInputNs = nowNs();
  const win = tabs?.activeWin;
  if (!win || win.isDestroyed()) return;
  if (!visible) {
    visible = true;
    win.webContents.invalidate();
  }
  applyRate();
}

function flushWheel() {
  const win = tabs?.activeWin;
  if (!pendingWheel || !win || win.isDestroyed()) return;
  const cmd = pendingWheel;
  pendingWheel = null;
  const wc = win.webContents;
  const mods = modifierList(cmd.mods);
  wc.sendInputEvent({
    type: 'mouseWheel',
    x: Math.round(cmd.x),
    y: Math.round(cmd.y),
    deltaX: cmd.deltaX || 0,
    deltaY: cmd.deltaY || 0,
    canScroll: true,
    modifiers: mods,
  });
}

function handleInput(cmd) {
  const win = tabs?.activeWin;
  if (!win || win.isDestroyed()) return;
  if (cmd.kind === 'mouse' && cmd.action === 'wheel') {
    wakeInput();
    scrollBoostUntilNs = nowNs() + SCROLL_BOOST_NS;
    if (!pendingWheel) {
      pendingWheel = { x: cmd.x, y: cmd.y, mods: cmd.mods, deltaX: cmd.deltaX || 0, deltaY: cmd.deltaY || 0 };
    } else {
      pendingWheel.deltaX += cmd.deltaX || 0;
      pendingWheel.deltaY += cmd.deltaY || 0;
    }
    flushWheel();
    return;
  }
  wakeInput();
  const wc = win.webContents;
  const mods = modifierList(cmd.mods);

  if (cmd.kind === 'mouse') {
    const base = { x: Math.round(cmd.x), y: Math.round(cmd.y), modifiers: mods };
    switch (cmd.action) {
      case 'move':
        wakeInput();
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

// MCP semantic inspection uses Chromium's DevTools protocol, but exposing a
// --remote-debugging-port creates an unauthenticated localhost control plane. Electron's
// in-process debugger API provides the same domains over the already-private Unix socket.
async function handleCdpCommand(cmd) {
  const id = cmd.id;
  const win = tabs?.activeWin;
  if (!win || win.isDestroyed()) {
    sendEvent({ t: 'cdpResult', id, error: { message: 'browser window is not ready' } });
    return;
  }
  if (typeof cmd.method !== 'string' || !cmd.method) {
    sendEvent({ t: 'cdpResult', id, error: { message: 'CDP method is required' } });
    return;
  }
  try {
    const debug = win.webContents.debugger;
    if (!debug.isAttached()) debug.attach('1.3');
    const result = await debug.sendCommand(cmd.method, cmd.params || {});
    sendEvent({ t: 'cdpResult', id, result: result || {} });
  } catch (e) {
    sendEvent({
      t: 'cdpResult',
      id,
      error: {
        message: e && e.message ? e.message : String(e),
        code: Number.isFinite(e?.code) ? e.code : undefined,
      },
    });
  }
}

function handleCommand(cmd) {
  switch (cmd.t) {
    case 'navigate':
      navigate(cmd.url);
      break;
    case 'resize':
      {
        const width = Number.isFinite(cmd.w) ? Math.max(1, Math.round(cmd.w)) : requestedSize.width;
        const height = Number.isFinite(cmd.h) ? Math.max(1, Math.round(cmd.h)) : requestedSize.height;
        requestedSize = { width, height };
      }
      if (tabs) {
        tabs.resize(requestedSize.width, requestedSize.height);
        const win = tabs.activeWin;
        if (win && !win.isDestroyed()) {
          win.webContents.invalidate();
        }
      }
      break;
    case 'input':
      handleInput(cmd);
      break;
    case 'visibility':
      // Terminal focus: gate output while unfocused, keep coalescing damage into pending.
      visible = !!cmd.visible;
      applyRate();
      if (visible) scheduleFlush();
      break;
    case 'fps':
      {
        const rate = Number(cmd.rate);
        if (Number.isFinite(rate)) {
          transportFpsCap = Math.max(1, Math.min(FRAME_RATE, Math.round(rate)));
          applyRate();
        }
      }
      break;
    case 'reload':
      {
        const win = tabs?.activeWin;
        if (win && !win.isDestroyed()) win.webContents.reload();
      }
      break;
    case 'copy':
      {
        const win = tabs?.activeWin;
        if (win && !win.isDestroyed()) win.webContents.copy();
      }
      break;
    case 'back':
      {
        const win = tabs?.activeWin;
        if (win && !win.isDestroyed() && win.webContents.navigationHistory.canGoBack()) {
          win.webContents.navigationHistory.goBack();
        }
      }
      break;
    case 'forward':
      {
        const win = tabs?.activeWin;
        if (win && !win.isDestroyed() && win.webContents.navigationHistory.canGoForward()) {
          win.webContents.navigationHistory.goForward();
        }
      }
      break;
    case 'find':
      {
        const win = tabs?.activeWin;
        if (win && !win.isDestroyed()) {
          const q = typeof cmd.query === 'string' ? cmd.query : '';
          if (!q) {
            win.webContents.stopFindInPage('clearSelection');
          } else {
            currentFindRequestId = win.webContents.findInPage(q, {
              findNext: !!cmd.findNext,
              forward: cmd.forward !== false,
            });
          }
        }
      }
      break;
    case 'stopFind':
      {
        const win = tabs?.activeWin;
        if (win && !win.isDestroyed()) {
          win.webContents.stopFindInPage('clearSelection');
        }
      }
      break;
    case 'zoom':
      {
        const win = tabs?.activeWin;
        if (win && !win.isDestroyed()) {
          const factor = Number(cmd.factor);
          if (Number.isFinite(factor)) {
            const clamped = Math.max(0.5, Math.min(3.0, factor));
            win.webContents.setZoomFactor(clamped);
            sendEvent({ t: 'zoom', factor: win.webContents.getZoomFactor() });
          }
        }
      }
      break;
    case 'tabNew':
      if (tabs) tabs.newTab(typeof cmd.url === 'string' ? cmd.url : undefined);
      break;
    case 'tabClose':
      if (tabs) tabs.closeTab(Number.isFinite(cmd.index) ? cmd.index : undefined);
      break;
    case 'tabSwitch':
      if (tabs && Number.isFinite(cmd.index)) tabs.switchTab(Math.max(0, Math.round(cmd.index)));
      break;
    case 'tabNext':
      if (tabs) tabs.switchRelative(1);
      break;
    case 'tabPrev':
      if (tabs) tabs.switchRelative(-1);
      break;
    case 'stats':
      sendEvent({ t: 'stats', ...stats, rate: curRate, visible });
      break;
    case 'cdp':
      void handleCdpCommand(cmd);
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
      if (len > MAX_COMMAND_LEN) {
        console.error(`[engine] command exceeded ${MAX_COMMAND_LEN} bytes`);
        socket.destroy();
        return;
      }
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
    // Give data already queued by the core one event-loop turn to land. A terminal can emit
    // SIGWINCH while Electron is connecting; attachReader is live already, so the retained
    // resize becomes the BrowserWindow's initial geometry instead of a stale first frame.
    setImmediate(() => {
      tabs = new TabManager({
        createWindow,
        sendEvent,
        onActiveChange: onActiveTabChange,
        agentMode: AGENT_MODE,
      });
      tabs.start(requestedSize.width, requestedSize.height, INITIAL_URL);
      sendEvent({
        t: 'ready',
        electron: process.versions.electron,
        chrome: process.versions.chrome,
        width: requestedSize.width,
        height: requestedSize.height,
        profile: PROFILE,
        frameRate: FRAME_RATE,
        sharedTexture: SHARED_TEXTURE,
        userData: app.getPath('userData'),
      });
    });
  });
  sock.on('error', (e) => {
    console.error('[engine] socket error:', e.message);
    app.exit(3);
  });
  // If the terminal core dies, the engine must not linger as an orphan holding a
  // Chromium process tree.
  sock.on('close', () => app.exit(0));
  attachReader(sock);
  // Single low-frequency timer whose only job is noticing the transition into idle after
  // activity stops. No work when already idle with nothing pending.
  setInterval(() => {
    if (curRate === RATE_IDLE && !pendingFrame) return;
    applyRate();
  }, 250);
});
