// Browser session: owns the Electron engine process and both channels to it.
//
//   channel 1  unix socket, 0600  -- the SAME wire protocol the Rust terminal core speaks
//                                    (apps/engine/src/main.js). Frames in, commands out.
//   channel 2  CDP over loopback  -- read-only page semantics (accessibility tree, box
//                                    model). Optional; see the security note below.
//
// A deliberate design choice: every *action* (click, type, scroll) is sent over channel 1
// as an engine `input` command, NOT over CDP. That means an agent's click travels the
// exact same code path as a human's click from the terminal -- one input pipeline, one set
// of bugs, and a page cannot distinguish the two. CDP is used only to *observe* (what is on
// the page, where is it), never to act.
//
// SECURITY NOTE. Enabling CDP starts a DevTools listener on 127.0.0.1. It is loopback-only
// (verified with lsof) so it is not reachable off-host, but it is UNAUTHENTICATED: any
// process running as this user can attach and drive the browser. That is a real widening of
// the engine's "opens no listening port of its own" posture. It is therefore:
//   * opt-out via BLACKGLASS_MCP_CDP=0 (semantic tools degrade to coordinate-only),
//   * bound to an ephemeral port discovered race-free from DevToolsActivePort,
//   * scoped to a throwaway --user-data-dir unless BLACKGLASS_MCP_PROFILE is set.
// The durable fix is an `eval` command on the existing 0600 socket; see README "Removing
// the DevTools port".
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

const DEFAULT_WIDTH = 1280;
const DEFAULT_HEIGHT = 800;

function locateEngine() {
  const candidates = [];
  if (process.env.BLACKGLASS_ENGINE) candidates.push(process.env.BLACKGLASS_ENGINE);
  candidates.push(path.resolve(__dirname, '../../../apps/engine'));
  for (const dir of candidates) {
    const electron = path.join(dir, 'node_modules/.bin/electron');
    const main = path.join(dir, 'src/main.js');
    if (fs.existsSync(electron) && fs.existsSync(main)) return { dir, electron, main };
  }
  throw new Error(
    `Could not find the BlackGlass engine. Looked in: ${candidates.join(', ')}. ` +
      'Set BLACKGLASS_ENGINE to the directory containing node_modules/.bin/electron and src/main.js.'
  );
}

function encodeMessage(type, payload) {
  const head = Buffer.allocUnsafe(5);
  head.writeUInt8(type, 0);
  head.writeUInt32BE(payload.length, 1);
  return Buffer.concat([head, payload]);
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
    this.cdpSessionId = null;
    // Bumped on every navigation. Snapshot refs carry the epoch they were minted in, so a
    // ref that survived a navigation can be rejected instead of silently clicking the
    // wrong thing (A03 failure mode c-F2).
    this.navEpoch = 0;
    this._loadWaiters = [];
    this._closed = false;
  }

  async start(initialUrl = 'about:blank') {
    const { electron, main } = locateEngine();

    // Private directory for the socket, mirroring the Rust core's posture: 0700 dir so no
    // other local user can even reach the 0600 socket inside it.
    this.dir = fs.mkdtempSync(path.join(os.tmpdir(), 'blackglass-mcp-'));
    fs.chmodSync(this.dir, 0o700);
    this.sockPath = path.join(this.dir, 'engine.sock');
    this.profileDir = process.env.BLACKGLASS_MCP_PROFILE || path.join(this.dir, 'profile');
    fs.mkdirSync(this.profileDir, { recursive: true });

    const connected = new Promise((resolve, reject) => {
      this.server = net.createServer((sock) => {
        this.sock = sock;
        try { fs.chmodSync(this.sockPath, 0o600); } catch { /* best effort */ }
        this._attach(sock);
        resolve();
      });
      this.server.on('error', reject);
      this.server.listen(this.sockPath, () => {
        try { fs.chmodSync(this.sockPath, 0o600); } catch { /* best effort */ }
      });
      setTimeout(() => reject(new Error('engine did not connect within 30s')), 30000);
    });

    const args = [
      main,
      `--bg-socket=${this.sockPath}`,
      `--bg-width=${this.width}`,
      `--bg-height=${this.height}`,
      `--bg-url=${initialUrl}`,
    ];
    if (this.useCdp) {
      // Port 0 => the OS picks a free port and Chromium writes it to DevToolsActivePort.
      // Asking the OS beats picking a port ourselves, which would race.
      args.push('--remote-debugging-port=0', `--user-data-dir=${this.profileDir}`);
    }

    // stdout of the child must never reach OUR stdout: stdout is the MCP channel and any
    // stray byte would corrupt the JSON-RPC stream.
    this.child = spawn(electron, args, { stdio: ['ignore', 'pipe', 'pipe'] });
    this.child.stdout.on('data', (d) => this.log('engine stdout: ' + d.toString().trim()));
    this.child.stderr.on('data', (d) => this.log('engine stderr: ' + d.toString().trim()));
    this.child.on('exit', (code, sig) => {
      this.log(`engine exited code=${code} signal=${sig}`);
      this._closed = true;
    });

    await connected;
    await this._waitForEvent('ready', 30000);
    if (initialUrl && initialUrl !== 'about:blank') this.navEpoch++;

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
    const { CdpClient } = require('./cdp');
    const portFile = path.join(this.profileDir, 'DevToolsActivePort');
    const deadline = Date.now() + 20000;
    let wsUrl = null;
    while (Date.now() < deadline) {
      if (fs.existsSync(portFile)) {
        const [port, browserPath] = fs.readFileSync(portFile, 'utf8').split('\n');
        if (port && browserPath) {
          wsUrl = `ws://127.0.0.1:${port.trim()}${browserPath.trim()}`;
          break;
        }
      }
      await new Promise((r) => setTimeout(r, 100));
    }
    if (!wsUrl) throw new Error('DevToolsActivePort never appeared');

    this.cdp = await CdpClient.connect(wsUrl);
    const { sessionId } = await this.cdp.attachToPage();
    this.cdpSessionId = sessionId;
    await this.cdp.send('Accessibility.enable', {}, sessionId);
    await this.cdp.send('DOM.enable', {}, sessionId);
    await this.cdp.send('Page.enable', {}, sessionId);
    this.log(`CDP attached at ${wsUrl.replace(/\/devtools\/browser\/.*/, '/devtools/browser/<redacted>')}`);
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
  /// CDP box models and engine input events use, and after a resize it can disagree with
  /// the dimensions of the most recent frame (see README, "Resize lag").
  viewportSize() {
    return this.state.viewport || { width: this.width, height: this.height };
  }

  cdpSend(method, params) {
    if (!this.cdp) {
      throw new Error(
        'Page semantics are unavailable: CDP is not connected' +
          (this.cdpError ? ` (${this.cdpError})` : ' (disabled via BLACKGLASS_MCP_CDP=0)') +
          '. Coordinate tools (browser_click_xy, browser_screenshot) still work.'
      );
    }
    return this.cdp.send(method, params, this.cdpSessionId);
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
        if (type === T_FRAME) this._onFrame(payload);
        else if (type === T_EVENT) this._onEvent(payload);
      }
    });
    sock.on('error', (e) => this.log('socket error: ' + e.message));
  }

  _onFrame(payload) {
    this.latestFrame = {
      seq: payload.readUInt32BE(0),
      width: payload.readUInt32BE(4),
      height: payload.readUInt32BE(8),
      format: payload.readUInt32BE(28),
      pixels: payload.subarray(FRAME_HEADER_LEN),
      at: Date.now(),
    };
  }

  _onEvent(payload) {
    let e;
    try { e = JSON.parse(payload.toString('utf8')); } catch { return; }
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

  close() {
    if (this._closed) return;
    this._closed = true;
    try { this.cdp && this.cdp.close(); } catch { /* ignore */ }
    try { this.send({ t: 'quit' }); } catch { /* ignore */ }
    try { this.child && this.child.kill(); } catch { /* ignore */ }
    try { this.server && this.server.close(); } catch { /* ignore */ }
    try { fs.rmSync(this.dir, { recursive: true, force: true }); } catch { /* ignore */ }
  }
}

module.exports = { Session, locateEngine, DEFAULT_WIDTH, DEFAULT_HEIGHT };
