'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { validateAgentNavigation, MAX_DATA_URL_BYTES } = require('../lib/navigation-policy');

test('agent navigation permits browser URLs and bounded inline documents', () => {
  for (const url of [
    'https://example.com/path?q=1',
    'http://127.0.0.1:8080/',
    'about:blank',
    'data:text/plain,hello',
  ]) assert.equal(validateAgentNavigation(url), url);
});

test('agent navigation rejects local, executable, external, and oversized URLs', () => {
  for (const url of [
    'file:///Users/alice/.ssh/id_ed25519',
    'javascript:alert(1)',
    'blob:https://example.com/id',
    'zoommtg://join',
    'relative/path',
    'data:text/plain,' + 'x'.repeat(MAX_DATA_URL_BYTES),
  ]) assert.throws(() => validateAgentNavigation(url), /not allowed|absolute|exceeds/);
});
