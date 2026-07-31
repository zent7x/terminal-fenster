// Minimal Chrome DevTools Protocol client. Zero dependencies.
//
// Node >= 22 ships a global WebSocket (undici), so a CDP client needs no npm package at
// all -- which matters here because this server must stay installable next to a browser
// that is already 200 MB of Chromium.
//
// We speak the *flattened* session protocol: one socket to the browser, and page sessions
// multiplexed over it by `sessionId`. That is simpler than juggling one socket per target
// and it is what every modern CDP client does.
'use strict';

const DEFAULT_TIMEOUT_MS = 15000;

class CdpError extends Error {
  constructor(method, err) {
    super(`CDP ${method} failed: ${err && err.message ? err.message : JSON.stringify(err)}`);
    this.name = 'CdpError';
    this.cdp = err;
  }
}

class CdpClient {
  constructor(url) {
    this.url = url;
    this.ws = null;
    this.nextId = 0;
    this.pending = new Map();
    this.listeners = new Map(); // "Domain.event" -> Set<fn>
    this.closed = false;
  }

  static async connect(url, timeoutMs = DEFAULT_TIMEOUT_MS) {
    if (typeof globalThis.WebSocket !== 'function') {
      throw new Error(
        'This Node build has no global WebSocket (needs Node >= 22). ' +
          'Upgrade Node or run BlackGlass MCP with BLACKGLASS_MCP_CDP=0 for coordinate-only mode.'
      );
    }
    const c = new CdpClient(url);
    await c._open(timeoutMs);
    return c;
  }

  _open(timeoutMs) {
    return new Promise((resolve, reject) => {
      const ws = new globalThis.WebSocket(this.url);
      this.ws = ws;
      const timer = setTimeout(() => reject(new Error(`CDP connect timed out after ${timeoutMs}ms`)), timeoutMs);
      ws.onopen = () => {
        clearTimeout(timer);
        resolve();
      };
      ws.onerror = (e) => {
        clearTimeout(timer);
        // An error after open is a transport failure, not a connect failure.
        if (this.ws && this.ws.readyState === 1) return;
        reject(new Error(`CDP connect failed: ${e && e.message ? e.message : 'socket error'}`));
      };
      ws.onclose = () => {
        this.closed = true;
        // Fail every in-flight call rather than letting callers hang forever.
        for (const [, p] of this.pending) p.reject(new Error('CDP connection closed'));
        this.pending.clear();
      };
      ws.onmessage = (ev) => this._onMessage(ev.data);
    });
  }

  _onMessage(raw) {
    let msg;
    try {
      msg = JSON.parse(typeof raw === 'string' ? raw : Buffer.from(raw).toString('utf8'));
    } catch {
      return;
    }
    if (msg.id !== undefined && this.pending.has(msg.id)) {
      const { resolve, reject, timer, method } = this.pending.get(msg.id);
      clearTimeout(timer);
      this.pending.delete(msg.id);
      if (msg.error) reject(new CdpError(method, msg.error));
      else resolve(msg.result || {});
      return;
    }
    if (msg.method) {
      const set = this.listeners.get(msg.method);
      if (set) for (const fn of set) { try { fn(msg.params || {}, msg.sessionId); } catch { /* listener must not break the pump */ } }
    }
  }

  on(event, fn) {
    if (!this.listeners.has(event)) this.listeners.set(event, new Set());
    this.listeners.get(event).add(fn);
    return () => this.listeners.get(event).delete(fn);
  }

  send(method, params = {}, sessionId = undefined, timeoutMs = DEFAULT_TIMEOUT_MS) {
    if (this.closed || !this.ws || this.ws.readyState !== 1) {
      return Promise.reject(new Error('CDP connection is not open'));
    }
    const id = ++this.nextId;
    const msg = { id, method, params };
    if (sessionId) msg.sessionId = sessionId;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`CDP ${method} timed out after ${timeoutMs}ms`));
      }, timeoutMs);
      this.pending.set(id, { resolve, reject, timer, method });
      this.ws.send(JSON.stringify(msg));
    });
  }

  /// Attach to the first page target and return its flattened sessionId.
  async attachToPage(timeoutMs = DEFAULT_TIMEOUT_MS) {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
      const { targetInfos = [] } = await this.send('Target.getTargets');
      const page = targetInfos.find((t) => t.type === 'page');
      if (page) {
        const { sessionId } = await this.send('Target.attachToTarget', {
          targetId: page.targetId,
          flatten: true,
        });
        return { sessionId, targetId: page.targetId };
      }
      if (Date.now() > deadline) throw new Error('no page target appeared');
      await new Promise((r) => setTimeout(r, 100));
    }
  }

  close() {
    this.closed = true;
    try { this.ws && this.ws.close(); } catch { /* already gone */ }
  }
}

module.exports = { CdpClient, CdpError };
