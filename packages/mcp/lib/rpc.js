// MCP stdio transport and JSON-RPC dispatch, dual-era.
//
// MCP changed shape in revision 2026-07-28: version negotiation moved from a one-time
// `initialize` handshake to per-request `_meta`, and `server/discover` became mandatory.
// Clients in the wild are a mix of both, so this server answers both and decides per
// request:
//
//   request carries _meta['io.modelcontextprotocol/protocolVersion']  -> modern, stateless
//   request is `initialize`                                           -> legacy, session
//
// Sources:
//   https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning
//   https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/stdio
//   https://modelcontextprotocol.io/specification/2026-07-28/server/discover
//   https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle
'use strict';

const MODERN_VERSIONS = ['2026-07-28'];
const LEGACY_VERSIONS = ['2025-11-25', '2025-06-18', '2025-03-26', '2024-11-05'];
const SUPPORTED_VERSIONS = [...MODERN_VERSIONS, ...LEGACY_VERSIONS];

const META_VERSION = 'io.modelcontextprotocol/protocolVersion';
const META_SERVER_INFO = 'io.modelcontextprotocol/serverInfo';

// JSON-RPC + MCP error codes.
const E_PARSE = -32700;
const E_INVALID_REQUEST = -32600;
const E_METHOD_NOT_FOUND = -32601;
const E_INVALID_PARAMS = -32602;
const E_INTERNAL = -32603;
const E_UNSUPPORTED_VERSION = -32022; // UnsupportedProtocolVersionError

class StdioServer {
  constructor(opts) {
    this.name = opts.name;
    this.version = opts.version;
    this.instructions = opts.instructions || '';
    this.listTools = opts.listTools;
    this.callTool = opts.callTool;
    this.log = opts.log || (() => {});
    this.onShutdown = opts.onShutdown || (() => {});
    this.negotiatedVersion = null;
  }

  capabilities() {
    return { tools: { listChanged: false } };
  }

  start() {
    let buf = '';
    process.stdin.setEncoding('utf8');
    process.stdin.on('data', (chunk) => {
      buf += chunk;
      // Messages are newline-delimited and MUST NOT contain embedded newlines, so a plain
      // split is the whole framing layer.
      let nl;
      while ((nl = buf.indexOf('\n')) !== -1) {
        const line = buf.slice(0, nl).trim();
        buf = buf.slice(nl + 1);
        if (line) this._handleLine(line);
      }
    });
    // "Servers SHOULD exit promptly when their standard input is closed" -- and we must
    // take the browser process down with us or we leak a Chromium tree.
    process.stdin.on('end', () => { this.onShutdown(); process.exit(0); });
    process.stdin.on('close', () => { this.onShutdown(); process.exit(0); });
  }

  _write(msg) {
    // JSON.stringify escapes newlines inside strings, so one message is always one line.
    process.stdout.write(JSON.stringify(msg) + '\n');
  }

  reply(id, result) { this._write({ jsonrpc: '2.0', id, result }); }

  replyError(id, code, message, data) {
    const error = { code, message };
    if (data !== undefined) error.data = data;
    this._write({ jsonrpc: '2.0', id, error });
  }

  async _handleLine(line) {
    let msg;
    try {
      msg = JSON.parse(line);
    } catch (e) {
      this.replyError(null, E_PARSE, 'Parse error: ' + e.message);
      return;
    }
    if (!msg || msg.jsonrpc !== '2.0' || typeof msg.method !== 'string') {
      if (msg && msg.id !== undefined) this.replyError(msg.id, E_INVALID_REQUEST, 'Invalid Request');
      return;
    }

    const isNotification = msg.id === undefined || msg.id === null;
    const params = msg.params || {};
    const meta = params._meta || {};
    const requested = meta[META_VERSION];
    const modern = typeof requested === 'string';

    try {
      // Per-request version gate for modern clients.
      if (modern && !SUPPORTED_VERSIONS.includes(requested)) {
        if (!isNotification) {
          this.replyError(msg.id, E_UNSUPPORTED_VERSION, 'Unsupported protocol version', {
            supported: SUPPORTED_VERSIONS,
            requested,
          });
        }
        return;
      }
      const result = await this._dispatch(msg.method, params, modern, isNotification);
      if (isNotification || result === undefined) return;
      this.reply(msg.id, modern ? { resultType: 'complete', ...result } : result);
    } catch (e) {
      if (isNotification) { this.log('notification handler threw: ' + e.stack); return; }
      if (e && e.rpcCode) this.replyError(msg.id, e.rpcCode, e.message, e.rpcData);
      else {
        this.log('internal error on ' + msg.method + ': ' + (e && e.stack));
        this.replyError(msg.id, E_INTERNAL, 'Internal error: ' + (e && e.message));
      }
    }
  }

  async _dispatch(method, params, modern, isNotification) {
    switch (method) {
      // ---- modern era -----------------------------------------------------------
      case 'server/discover':
        return {
          supportedVersions: SUPPORTED_VERSIONS,
          capabilities: this.capabilities(),
          instructions: this.instructions,
          _meta: { [META_SERVER_INFO]: { name: this.name, version: this.version } },
        };

      // ---- legacy era -----------------------------------------------------------
      case 'initialize': {
        const want = params.protocolVersion;
        // "If the server supports the requested version it MUST respond with the same
        // version. Otherwise it MUST respond with another version it supports."
        const agreed = LEGACY_VERSIONS.includes(want) ? want : LEGACY_VERSIONS[0];
        this.negotiatedVersion = agreed;
        this.log(`legacy initialize: client asked ${want}, agreed ${agreed}`);
        return {
          protocolVersion: agreed,
          capabilities: this.capabilities(),
          serverInfo: { name: this.name, version: this.version },
          instructions: this.instructions,
        };
      }

      case 'notifications/initialized':
      case 'notifications/cancelled':
      case 'notifications/roots/list_changed':
        return undefined; // notifications get no reply

      case 'ping':
        return {};

      // ---- tools ----------------------------------------------------------------
      case 'tools/list':
        return { tools: this.listTools() };

      case 'tools/call': {
        if (!params || typeof params.name !== 'string') {
          const err = new Error('tools/call requires a string "name"');
          err.rpcCode = E_INVALID_PARAMS;
          throw err;
        }
        return await this.callTool(params.name, params.arguments || {});
      }

      case 'resources/list':
        return { resources: [] };

      case 'prompts/list':
        return { prompts: [] };

      default: {
        if (isNotification) return undefined; // unknown notifications are ignored, per spec
        const err = new Error(`Method not found: ${method}`);
        err.rpcCode = E_METHOD_NOT_FOUND;
        throw err;
      }
    }
  }
}

module.exports = {
  StdioServer,
  SUPPORTED_VERSIONS,
  MODERN_VERSIONS,
  LEGACY_VERSIONS,
  E_METHOD_NOT_FOUND,
  E_INVALID_PARAMS,
  E_UNSUPPORTED_VERSION,
};
