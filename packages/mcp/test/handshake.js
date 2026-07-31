#!/usr/bin/env node
// Protocol conformance test. Starts NO browser -- it exercises only the stdio transport
// and JSON-RPC dispatch, so it runs anywhere, including CI with no display and no Chromium.
//
// Covers both protocol eras, because a server that only speaks one of them silently fails
// against half the clients in the wild:
//   legacy  (2025-06-18) initialize -> notifications/initialized -> tools/list
//   modern  (2026-07-28) server/discover -> tools/list, versions in per-request _meta
//   errors  unsupported protocol version -> -32022, unknown method -> -32601
'use strict';

const path = require('path');
const { spawn } = require('child_process');

const SERVER = path.resolve(__dirname, '../index.js');
const META = 'io.modelcontextprotocol/protocolVersion';

function runSession(requests, { timeoutMs = 10000 } = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [SERVER], { stdio: ['pipe', 'pipe', 'pipe'] });
    let out = '';
    let err = '';
    child.stdout.on('data', (d) => { out += d.toString(); });
    child.stderr.on('data', (d) => { err += d.toString(); });
    const timer = setTimeout(() => { child.kill(); reject(new Error('server did not exit in time')); }, timeoutMs);
    child.on('exit', () => {
      clearTimeout(timer);
      const lines = out.split('\n').filter((l) => l.trim());
      resolve({ raw: out, stderr: err, messages: lines.map((l) => JSON.parse(l)) });
    });
    for (const r of requests) child.stdin.write(JSON.stringify(r) + '\n');
    child.stdin.end(); // EOF is the graceful-shutdown signal
  });
}

const results = [];
function check(name, pass, detail) {
  results.push({ name, pass, detail });
  console.log(`${pass ? 'PASS' : 'FAIL'}  ${name}${detail ? '  -- ' + detail : ''}`);
}

(async () => {
  // ---- legacy era ------------------------------------------------------------------
  const legacy = await runSession([
    { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'conformance-test', version: '1.0.0' } } },
    { jsonrpc: '2.0', method: 'notifications/initialized' },
    { jsonrpc: '2.0', id: 2, method: 'tools/list', params: {} },
  ]);

  console.log('\n--- legacy stdout, verbatim ---');
  console.log(legacy.raw.trim().split('\n').map((l) => l.slice(0, 300) + (l.length > 300 ? ' ...[truncated for display]' : '')).join('\n'));
  console.log('--- end ---\n');

  const init = legacy.messages.find((m) => m.id === 1);
  check('initialize returns a result', !!(init && init.result), init && init.error ? JSON.stringify(init.error) : '');
  check('initialize echoes the requested protocol version', init && init.result && init.result.protocolVersion === '2025-06-18', init && init.result && init.result.protocolVersion);
  check('initialize declares the tools capability', !!(init && init.result && init.result.capabilities && init.result.capabilities.tools));
  check('initialize reports serverInfo', !!(init && init.result && init.result.serverInfo && init.result.serverInfo.name === 'blackglass'), init && init.result && JSON.stringify(init.result.serverInfo));
  check('notifications/initialized produces no response', legacy.messages.filter((m) => m.id === undefined || m.id === null).length === 0);

  const list = legacy.messages.find((m) => m.id === 2);
  const tools = list && list.result && list.result.tools;
  check('tools/list returns tools', Array.isArray(tools) && tools.length > 0, tools ? `${tools.length} tools` : 'none');
  check('every tool has name + description + object inputSchema',
    Array.isArray(tools) && tools.every((t) => typeof t.name === 'string' && typeof t.description === 'string' && t.inputSchema && t.inputSchema.type === 'object'));
  check('tool names are unique', Array.isArray(tools) && new Set(tools.map((t) => t.name)).size === tools.length);
  check('legacy results carry no modern resultType field', list && list.result && list.result.resultType === undefined);
  check('stdout is pure JSON-RPC (every line parses)', legacy.messages.length === legacy.raw.trim().split('\n').filter((l) => l.trim()).length);
  check('no message contains an embedded newline', legacy.raw.trim().split('\n').every((l) => { try { JSON.parse(l); return true; } catch { return false; } }));
  check('diagnostics went to stderr, not stdout', legacy.stderr.includes('blackglass-mcp'));

  if (Array.isArray(tools)) console.log('\ntools: ' + tools.map((t) => t.name).join(', ') + '\n');

  // ---- modern era ------------------------------------------------------------------
  const modern = await runSession([
    { jsonrpc: '2.0', id: 'd1', method: 'server/discover', params: { _meta: { [META]: '2026-07-28', 'io.modelcontextprotocol/clientInfo': { name: 'conformance-test', version: '1.0.0' } } } },
    { jsonrpc: '2.0', id: 'd2', method: 'tools/list', params: { _meta: { [META]: '2026-07-28' } } },
    { jsonrpc: '2.0', id: 'd3', method: 'tools/list', params: { _meta: { [META]: '1900-01-01' } } },
    { jsonrpc: '2.0', id: 'd4', method: 'nonsense/method', params: {} },
  ]);

  console.log('--- modern stdout, verbatim ---');
  console.log(modern.raw.trim().split('\n').map((l) => l.slice(0, 300) + (l.length > 300 ? ' ...[truncated for display]' : '')).join('\n'));
  console.log('--- end ---\n');

  const disc = modern.messages.find((m) => m.id === 'd1');
  check('server/discover returns a DiscoverResult', !!(disc && disc.result), disc && disc.error ? JSON.stringify(disc.error) : '');
  check('DiscoverResult advertises supportedVersions incl. 2026-07-28',
    !!(disc && disc.result && Array.isArray(disc.result.supportedVersions) && disc.result.supportedVersions.includes('2026-07-28')),
    disc && disc.result && JSON.stringify(disc.result.supportedVersions));
  check('DiscoverResult sets resultType=complete', disc && disc.result && disc.result.resultType === 'complete');
  check('DiscoverResult carries serverInfo in _meta',
    !!(disc && disc.result && disc.result._meta && disc.result._meta['io.modelcontextprotocol/serverInfo']),
    disc && disc.result && disc.result._meta && JSON.stringify(disc.result._meta['io.modelcontextprotocol/serverInfo']));

  const mlist = modern.messages.find((m) => m.id === 'd2');
  check('tools/list works without an initialize handshake', !!(mlist && mlist.result && mlist.result.tools.length > 0));
  check('modern results carry resultType=complete', mlist && mlist.result && mlist.result.resultType === 'complete');
  check('modern and legacy expose the same tool set',
    !!(mlist && mlist.result && tools && mlist.result.tools.length === tools.length));

  const bad = modern.messages.find((m) => m.id === 'd3');
  check('unsupported protocol version -> -32022', !!(bad && bad.error && bad.error.code === -32022), bad && bad.error && JSON.stringify(bad.error));
  check('the error lists supported versions', !!(bad && bad.error && bad.error.data && Array.isArray(bad.error.data.supported)));

  const unknown = modern.messages.find((m) => m.id === 'd4');
  check('unknown method -> -32601', !!(unknown && unknown.error && unknown.error.code === -32601), unknown && unknown.error && unknown.error.message);

  // ---- tool-level error handling ---------------------------------------------------
  const toolErr = await runSession([
    { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 't', version: '1' } } },
    { jsonrpc: '2.0', id: 2, method: 'tools/call', params: { name: 'browser_snapshot', arguments: {} } },
    { jsonrpc: '2.0', id: 3, method: 'tools/call', params: { name: 'no_such_tool', arguments: {} } },
  ]);
  const noSession = toolErr.messages.find((m) => m.id === 2);
  check('calling a page tool with no session is a tool error, not a crash',
    !!(noSession && noSession.result && noSession.result.isError === true),
    noSession && noSession.result && noSession.result.content[0].text);
  const badTool = toolErr.messages.find((m) => m.id === 3);
  check('unknown tool name -> JSON-RPC error', !!(badTool && badTool.error), badTool && badTool.error && badTool.error.message);

  const failed = results.filter((r) => !r.pass);
  console.log(`\n${results.length - failed.length}/${results.length} checks passed`);
  process.exit(failed.length ? 1 : 0);
})().catch((e) => {
  console.error('harness error:', e);
  process.exit(2);
});
