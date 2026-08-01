// Browser session: owns the Electron engine process and both channels to it.
//
//   channel 1  unix socket, 0600  -- the SAME wire protocol the Rust terminal core speaks
//                                    (apps/engine/src/main.js). Frames in, commands out.
//   channel 2  CDP over the same socket -- the engine proxies Electron's in-process
//                                         debugger API. No TCP listener is opened.
//
// A deliberate design choice: every *action* (click, type, scroll) is sent over channel 1
// as an engine `input` command, NOT over CDP. That means an agent's click travels the
// exact same code path as a human's click from the terminal -- one input pipeline, one set
// of bugs, and a page cannot distinguish the two. CDP is used only to *observe* (what is on
// the page, where is it), never to act.
//
// Page semantics are opt-out via TERMINAL_FENSTER_MCP_CDP=0. When enabled, CDP requests and
// responses remain inside the existing 0600 Unix socket; web content still has no access
// to Electron, Node, or the debugger object.
'use strict';

const net = require('net');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { spawn } = require('child_process');

const T_FRAME = 1;
const T_EVENT = 2;
const T_COMMAND = 10;
const FRAME_HEADER_LEN = 32;
const MAX_MESSAGE_LEN = 64 * 1024 * 1024;

const DEFAULT_WIDTH = 1280;
const DEFAULT_HEIGHT = 800;

function locateEngine() {
  const probe = (candidate) => {
    const dir = path.resolve(candidate);
    const electron = path.join(dir, 'node_modules/.bin/electron');
    const main = path.join(dir, 'src/main.js');
    const electronPackage = path.join(dir, 'node_modules/electron');
    const pathFile = path.join(electronPackage, 'path.txt');
    if (!fs.existsSync(main)) return { error: `missing ${main}` };
    if (!fs.existsSync(electron)) return { error: `missing ${electron}` };
    if (!fs.existsSync(pathFile)) {
      return { error: `Electron runtime is not downloaded (${pathFile} is missing)` };
    }
    const runtimeName = fs.readFileSync(pathFile, 'utf8').trim();
    const runtime = path.resolve(electronPackage, 'dist', runtimeName);
    if (!runtimeName || !fs.existsSync(runtime) || !fs.statSync(runtime).isFile()) {
      return { error: `Electron runtime executable is missing (${runtime})` };
    }
    return { value: { dir, electron, main, runtime } };
  };

  if (process.env.TERMINAL_FENSTER_ENGINE) {
    const result = probe(process.env.TERMINAL_FENSTER_ENGINE);
    if (result.value) return result.value;
    throw new Error(`TERMINAL_FENSTER_ENGINE=${process.env.TERMINAL_FENSTER_ENGINE} is not ready: ${result.error}`);
  }

  // Installed layout: <prefix>/bin/terminal-fenster with <prefix>/engine beside it.
  const bin = process.env.TERMINAL_FENSTER_BIN;
  if (bin) {
    let base = path.resolve(bin);
    try {
      if (fs.existsSync(base)) base = fs.realpathSync(base);
    } catch { /* best effort */ }
    for (let i = 0; i < 6; i++) {
      base = path.dirname(base);
      const result = probe(path.join(base, 'engine'));
      if (result.value) return result.value;
    }
  }

  const source = path.resolve(__dirname, '../../../apps/engine');
  const result = probe(source);
  if (result.value) return result.value;
  throw new Error(`Could not find a ready Terminal-Fenster engine at ${source}: ${result.error}`);
}

function encodeMessage(type, payload) {
  const head = Buffer.allocUnsafe(5);
  head.writeUInt8(type, 0);
  head.writeUInt32BE(payload.length, 1);
  return Buffer.concat([head, payload]);
}

/**
 * Composite one damage-only BGRA frame into the retained full viewport.
 *
 * The engine wire payload is `header + dirty_w*dirty_h*4`, not necessarily a full
 * bitmap. Keeping this pure also pins the MCP consumer to the same protocol invariants
 * as the Rust terminal core instead of letting screenshot correctness drift separately.
 */
function compositeFrame(payload, previous = null) {
  if (!Buffer.isBuffer(payload) || payload.length < FRAME_HEADER_LEN) {
    throw new Error(`Truncated frame header: ${payload?.length || 0} bytes`);
  }
  const frame = {
    seq: payload.readUInt32BE(0),
    width: payload.readUInt32BE(4),
    height: payload.readUInt32BE(8),
    dirtyX: payload.readUInt32BE(12),
    dirtyY: payload.readUInt32BE(16),
    dirtyW: payload.readUInt32BE(20),
    dirtyH: payload.readUInt32BE(24),
    format: payload.readUInt32BE(28),
  };
  if (frame.format !== 0) throw new Error(`Unexpected frame format ${frame.format}`);
  if (frame.width === 0 || frame.height === 0 || frame.dirtyW === 0 || frame.dirtyH === 0) {
    throw new Error('Frame and dirty dimensions must be non-zero');
  }
  if (
    frame.dirtyX >= frame.width || frame.dirtyY >= frame.height ||
    frame.dirtyW > frame.width - frame.dirtyX ||
    frame.dirtyH > frame.height - frame.dirtyY
  ) {
    throw new Error(
      `Dirty rect ${frame.dirtyX},${frame.dirtyY} ${frame.dirtyW}x${frame.dirtyH} ` +
      `is outside ${frame.width}x${frame.height}`
    );
  }

  const fullBytes = frame.width * frame.height * 4;
  const dirtyBytes = frame.dirtyW * frame.dirtyH * 4;
  if (!Number.isSafeInteger(fullBytes) || fullBytes > MAX_MESSAGE_LEN - FRAME_HEADER_LEN) {
    throw new Error(`Frame geometry ${frame.width}x${frame.height} exceeds the 64 MiB limit`);
  }
  if (!Number.isSafeInteger(dirtyBytes) || payload.length !== FRAME_HEADER_LEN + dirtyBytes) {
    throw new Error(
      `Dirty payload has ${payload.length - FRAME_HEADER_LEN} bytes, expected ${dirtyBytes}`
    );
  }

  const isFull = frame.dirtyX === 0 && frame.dirtyY === 0 &&
    frame.dirtyW === frame.width && frame.dirtyH === frame.height;
  const dirty = payload.subarray(FRAME_HEADER_LEN);
  if (isFull) {
    frame.pixels = Buffer.from(dirty);
  } else {
    if (
      !previous || previous.width !== frame.width || previous.height !== frame.height ||
      previous.format !== frame.format || previous.pixels.length !== fullBytes
    ) {
      throw new Error('Partial frame arrived before a matching full base frame');
    }
    // Mutate the retained canvas in place: copying a multi-megabyte viewport for a caret-
    // sized update would erase the bandwidth win that damage transport was built for.
    frame.pixels = previous.pixels;
    const srcStride = frame.dirtyW * 4;
    const dstStride = frame.width * 4;
    for (let row = 0; row < frame.dirtyH; row++) {
      const src = row * srcStride;
      const dst = (frame.dirtyY + row) * dstStride + frame.dirtyX * 4;
      dirty.copy(frame.pixels, dst, src, src + srcStride);
    }
  }
  frame.at = Date.now();
  return frame;
}

class Session {
  constructor(opts = {}) {
    this.width = opts.width || DEFAULT_WIDTH;
    this.height = opts.height || DEFAULT_HEIGHT;
    this.useCdp = opts.useCdp !== false;
    this.log = opts.log || (() => {});
    this.latestFrame = null;
    this.events = [];
    this.state = { url: 'about:blank', title: '', loading: false, chrome: null, electron: null };
    this.cdp = null;
    this._cdpNextId = 0;
    this._cdpPending = new Map();
    // Bumped on every navigation. Snapshot refs carry the epoch they were minted in, so a
    // ref that survived a navigation can be rejected instead of silently clicking the
    // wrong thing (A03 failure mode c-F2).
    this.navEpoch = 0;
    this._loadWaiters = [];
    this._closed = false;
    this._closePromise = null;
  }

  async start(initialUrl = 'about:blank') {
    const { electron, main } = locateEngine();

    // Private directory for the socket, mirroring the Rust core's posture: 0700 dir so no
    // other local user can even reach the 0600 socket inside it.
    this.dir = fs.mkdtempSync(path.join(os.tmpdir(), 'terminal-fenster-mcp-'));
    fs.chmodSync(this.dir, 0o700);
    this.sockPath = path.join(this.dir, 'engine.sock');
    this.profileDir = process.env.TERMINAL_FENSTER_MCP_PROFILE || path.join(this.dir, 'profile');
    fs.mkdirSync(this.profileDir, { recursive: true });

    let connectTimer;
    const connected = new Promise((resolve, reject) => {
      this.server = net.createServer((sock) => {
        clearTimeout(connectTimer);
        this.sock = sock;
        try { fs.chmodSync(this.sockPath, 0o600); } catch { /* best effort */ }
        this._attach(sock);
        resolve();
      });
      this.server.on('error', (e) => {
        clearTimeout(connectTimer);
        reject(e);
      });
      this.server.listen(this.sockPath, () => {
        try { fs.chmodSync(this.sockPath, 0o600); } catch { /* best effort */ }
      });
      connectTimer = setTimeout(
        () => reject(new Error('engine did not connect within 30s')),
        30000
      );
    });

    const args = [
      main,
      `--tf-socket=${this.sockPath}`,
      `--tf-width=${this.width}`,
      `--tf-height=${this.height}`,
      '--tf-agent-mode',
      `--user-data-dir=${this.profileDir}`,
    ];

    // stdout of the child must never reach OUR stdout: stdout is the MCP channel and any
    // stray byte would corrupt the JSON-RPC stream.
    this.child = spawn(electron, args, { stdio: ['ignore', 'pipe', 'pipe'] });
    this.child.stdout.on('data', (d) => this.log('engine stdout: ' + d.toString().trim()));
    this.child.stderr.on('data', (d) => this.log('engine stderr: ' + d.toString().trim()));
    this.child.on('exit', (code, sig) => {
      this.log(`engine exited code=${code} signal=${sig}`);
      this._closed = true;
      this._failCdpPending('engine exited');
      this._cleanupDir();
    });

    await connected;
    await this._waitForEvent('ready', 30000);
    if (initialUrl && initialUrl !== 'about:blank') await this.navigate(initialUrl);

    if (this.useCdp) {
      try {
        await this._connectCdp();
      } catch (e) {
        this.log('CDP unavailable, falling back to coordinate-only mode: ' + e.message);
        this.cdp = null;
        this.cdpError = e.message;
      }
    }
    return this;
  }

  async _connectCdp() {
    this.cdp = { transport: 'engine-debugger' };
    try {
      await this.cdpSend('Accessibility.enable', {});
      await this.cdpSend('DOM.enable', {});
      await this.cdpSend('Page.enable', {});
      this.log('CDP attached through the private engine socket (no TCP listener)');
    } catch (e) {
      this.cdp = null;
      throw e;
    }
  }

  /// Re-read title, URL and viewport straight from the page.
  ///
  /// The socket event stream is authoritative but asynchronous: an action's effect on the
  /// title can still be in flight when the tool returns, and an agent that reads a stale
  /// status concludes its click did nothing and retries. One CDP round trip removes the
  /// race entirely.
  async syncState() {
    if (!this.cdp) return;
    try {
      const r = await this.cdpSend('Runtime.evaluate', {
        expression: 'JSON.stringify({t:document.title,u:location.href,w:innerWidth,h:innerHeight})',
        returnByValue: true,
      });
      const v = JSON.parse(r.result.value);
      // Fragment-only changes leave the DOM (and therefore every ref) intact, so they must
      // not invalidate the snapshot; a real document change must.
      const strip = (u) => String(u).split('#')[0];
      if (strip(v.u) !== strip(this.state.url)) this.navEpoch++;
      this.state.url = v.u;
      this.state.title = v.t;
      this.state.viewport = { width: v.w, height: v.h };
    } catch {
      // A navigating or crashed page cannot answer; the event stream remains the fallback.
    }
  }

  /// The CSS-pixel viewport the page believes it has. This is the coordinate space both
  /// CDP box models and engine input events use. Screenshot tools still compare it with
  /// the retained frame defensively so a future resize regression cannot cause mis-clicks.
  viewportSize() {
    return this.state.viewport || { width: this.width, height: this.height };
  }

  cdpSend(method, params = {}, timeoutMs = 15000) {
    if (!this.cdp) {
      throw new Error(
        'Page semantics are unavailable: CDP is not connected' +
          (this.cdpError ? ` (${this.cdpError})` : ' (disabled via TERMINAL_FENSTER_MCP_CDP=0)') +
          '. Coordinate tools (browser_click_xy, browser_screenshot) still work.'
      );
    }
    const id = ++this._cdpNextId;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this._cdpPending.delete(id);
        reject(new Error(`CDP ${method} timed out after ${timeoutMs}ms`));
      }, timeoutMs);
      this._cdpPending.set(id, { resolve, reject, timer, method });
      try {
        this.send({ t: 'cdp', id, method, params });
      } catch (e) {
        clearTimeout(timer);
        this._cdpPending.delete(id);
        reject(e);
      }
    });
  }

  _failCdpPending(reason) {
    for (const { reject, timer } of this._cdpPending.values()) {
      clearTimeout(timer);
      reject(new Error(reason));
    }
    this._cdpPending.clear();
  }

  _attach(sock) {
    let buf = Buffer.alloc(0);
    sock.on('data', (chunk) => {
      buf = buf.length ? Buffer.concat([buf, chunk]) : chunk;
      for (;;) {
        if (buf.length < 5) return;
        const type = buf.readUInt8(0);
        const len = buf.readUInt32BE(1);
        if (len > MAX_MESSAGE_LEN) {
          this.log(`engine declared an oversized ${len}-byte message; closing the socket`);
          sock.destroy();
          return;
        }
        if (buf.length < 5 + len) return;
        const payload = buf.subarray(5, 5 + len);
        buf = buf.subarray(5 + len);
        if (type === T_FRAME) this._onFrame(payload);
        else if (type === T_EVENT) this._onEvent(payload);
      }
    });
    sock.on('error', (e) => this.log('socket error: ' + e.message));
    sock.on('close', () => this._failCdpPending('engine socket closed'));
  }

  _onFrame(payload) {
    try {
      this.latestFrame = compositeFrame(payload, this.latestFrame);
    } catch (e) {
      this.log('dropped invalid frame: ' + e.message);
    }
  }

  _onEvent(payload) {
    let e;
    try { e = JSON.parse(payload.toString('utf8')); } catch { return; }
    if (e.t === 'cdpResult') {
      const pending = this._cdpPending.get(e.id);
      if (!pending) return;
      clearTimeout(pending.timer);
      this._cdpPending.delete(e.id);
      if (e.error) {
        pending.reject(new Error(
          `CDP ${pending.method} failed: ${e.error.message || JSON.stringify(e.error)}`
        ));
      } else {
        pending.resolve(e.result || {});
      }
      return;
    }
    this.events.push(e);
    if (this.events.length > 500) this.events.splice(0, this.events.length - 500);
    switch (e.t) {
      case 'ready':
        this.state.chrome = e.chrome;
        this.state.electron = e.electron;
        break;
      case 'title': this.state.title = e.v; break;
      case 'url':
        if (e.v !== this.state.url) this.navEpoch++;
        this.state.url = e.v;
        break;
      case 'loading':
        this.state.loading = e.v;
        if (e.v === false) {
          const w = this._loadWaiters.splice(0);
          for (const fn of w) fn();
        }
        break;
      case 'loadError':
        this.state.lastError = `${e.desc} (${e.code}) for ${e.url}`;
        break;
      case 'crash':
        this.state.lastError = `renderer gone: ${e.reason} exit=${e.exitCode}`;
        break;
      default: break;
    }
  }

  _waitForEvent(name, timeoutMs) {
    const found = this.events.find((e) => e.t === name);
    if (found) return Promise.resolve(found);
    return new Promise((resolve, reject) => {
      const started = Date.now();
      const iv = setInterval(() => {
        const hit = this.events.find((e) => e.t === name);
        if (hit) { clearInterval(iv); resolve(hit); }
        else if (Date.now() - started > timeoutMs) { clearInterval(iv); reject(new Error(`timed out waiting for engine '${name}' event`)); }
      }, 25);
    });
  }

  send(cmd) {
    if (!this.sock || this.sock.destroyed) throw new Error('engine socket is closed');
    this.sock.write(encodeMessage(T_COMMAND, Buffer.from(JSON.stringify(cmd), 'utf8')));
  }

  /// Resolve when the page stops loading, or after timeoutMs. Never rejects: a page that
  /// keeps a connection open forever (analytics beacons, websockets) is normal, and an
  /// agent would rather get a slightly-early snapshot than an error.
  waitForLoad(timeoutMs = 10000) {
    if (!this.state.loading) return Promise.resolve(false);
    return new Promise((resolve) => {
      const timer = setTimeout(() => resolve(true), timeoutMs);
      this._loadWaiters.push(() => { clearTimeout(timer); resolve(false); });
    });
  }

  async navigate(url) {
    this.state.lastError = null;
    this.send({ t: 'navigate', url });
    // did-start-loading may not have arrived yet, so give it a moment to latch before we
    // decide whether to wait for the load to finish.
    await new Promise((r) => setTimeout(r, 150));
    const timedOut = await this.waitForLoad();
    return { timedOut, error: this.state.lastError || null };
  }

  _cleanupDir() {
    if (!this.dir) return;
    try { fs.rmSync(this.dir, { recursive: true, force: true }); } catch { return; }
    this.dir = null;
  }

  close() {
    if (this._closePromise) return this._closePromise;
    if (this._closed) {
      this._cleanupDir();
      return Promise.resolve();
    }
    this._closed = true;
    this._failCdpPending('browser session closed');
    try { this.send({ t: 'quit' }); } catch { /* ignore */ }
    try { this.server && this.server.close(); } catch { /* ignore */ }
    // Electron can write Local State during shutdown. Removing the profile first lets that late
    // write recreate a small orphan directory, so wait for process exit and clean afterwards.
    this._closePromise = new Promise((resolve) => {
      if (!this.child || this.child.exitCode !== null || this.child.signalCode !== null) {
        this._cleanupDir();
        resolve();
        return;
      }
      let finished = false;
      let forceTimer;
      let giveUpTimer;
      const finish = () => {
        if (finished) return;
        finished = true;
        clearTimeout(forceTimer);
        clearTimeout(giveUpTimer);
        this._cleanupDir();
        resolve();
      };
      this.child.once('exit', finish);
      forceTimer = setTimeout(() => {
        try { this.child.kill(); } catch { /* already gone */ }
      }, 1000);
      giveUpTimer = setTimeout(finish, 2000);
    });
    return this._closePromise;
  }
}

module.exports = {
  Session,
  locateEngine,
  compositeFrame,
  DEFAULT_WIDTH,
  DEFAULT_HEIGHT,
  FRAME_HEADER_LEN,
};
