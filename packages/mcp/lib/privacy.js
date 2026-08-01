'use strict';

const fs = require('fs');
const os = require('os');
const path = require('path');

function redactUrl(raw) {
  const text = String(raw || '');
  if (text === 'about:blank') return text;
  if (text.startsWith('data:')) {
    const comma = text.indexOf(',');
    const meta = (comma < 0 ? text.slice(5) : text.slice(5, comma))
      .replace(/[^a-zA-Z0-9/;=+.-]/g, '').slice(0, 80);
    const payloadLength = comma < 0 ? 0 : Array.from(text.slice(comma + 1)).length;
    return `data:${meta},<redacted:${payloadLength}>`;
  }
  if (text.startsWith('file:')) {
    let segments = 0;
    try { segments = new URL(text).pathname.split('/').filter(Boolean).length; } catch {}
    return `file:///<path-redacted:${segments}>`;
  }
  let url;
  try { url = new URL(text); } catch { return '<url-redacted>'; }
  if (!['https:', 'http:'].includes(url.protocol)) return `<${url.protocol || 'url'}-redacted>`;
  const pathSegments = url.pathname.split('/').filter(Boolean).length;
  const keys = [...new Set([...url.searchParams.keys()])]
    .map((key) => key.replace(/[^a-zA-Z0-9_.-]/g, '').slice(0, 32))
    .filter(Boolean)
    .slice(0, 12);
  const fragmentLength = url.hash ? Array.from(url.hash.slice(1)).length : 0;
  return `${url.protocol}//${url.host}/<path:${pathSegments}>` +
    `?<params:${url.searchParams.size}${keys.length ? ':' + keys.join(',') : ''}>` +
    `#<fragment:${fragmentLength}>`;
}

function classifyKey(key) {
  const text = String(key || '');
  if (Array.from(text).length === 1) return '<printable>';
  return text.replace(/[^a-zA-Z0-9_-]/g, '').slice(0, 32) || '<other>';
}

function defaultMcpStateDir() {
  const base = process.platform === 'darwin'
    ? path.join(os.homedir(), 'Library', 'Application Support')
    : (process.env.XDG_STATE_HOME || path.join(os.homedir(), '.local', 'state'));
  const dir = path.join(base, 'terminal-fenster', 'mcp');
  fs.mkdirSync(dir, { recursive: true, mode: 0o700 });
  fs.chmodSync(dir, 0o700);
  return dir;
}

function appendPrivate(file, line) {
  const flags = fs.constants.O_WRONLY | fs.constants.O_CREAT | fs.constants.O_APPEND |
    (fs.constants.O_NOFOLLOW || 0);
  const fd = fs.openSync(file, flags, 0o600);
  try {
    fs.fchmodSync(fd, 0o600);
    fs.writeSync(fd, line);
  } finally {
    fs.closeSync(fd);
  }
}

module.exports = { appendPrivate, classifyKey, defaultMcpStateDir, redactUrl };
