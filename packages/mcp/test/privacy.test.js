'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { appendPrivate, classifyKey, redactUrl } = require('../lib/privacy');

test('URL redaction drops credentials, values, fragments, data, and local paths', () => {
  const cases = [
    ['https://user:pass@example.com/reset?token=SECRET#access_token=ALSO_SECRET', ['pass', 'SECRET', 'ALSO_SECRET']],
    ['file:///Users/alice/.ssh/id_ed25519', ['alice', '.ssh', 'id_ed25519']],
    ['data:text/plain,PRIVATE_BODY', ['PRIVATE_BODY']],
  ];
  for (const [url, forbidden] of cases) {
    const redacted = redactUrl(url);
    for (const secret of forbidden) assert.doesNotMatch(redacted, new RegExp(secret));
  }
});

test('printable audit keys cannot reconstruct typed secrets', () => {
  assert.equal(classifyKey('s'), '<printable>');
  assert.equal(classifyKey('Enter'), 'Enter');
});

test('private append creates mode 0600 and refuses symlinks', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'bg-mcp-log-'));
  const log = path.join(dir, 'audit.jsonl');
  appendPrivate(log, 'one\n');
  assert.equal(fs.statSync(log).mode & 0o777, 0o600);
  const target = path.join(dir, 'target');
  const link = path.join(dir, 'link');
  fs.writeFileSync(target, 'untouched');
  fs.symlinkSync(target, link);
  assert.throws(() => appendPrivate(link, 'bad\n'));
  assert.equal(fs.readFileSync(target, 'utf8'), 'untouched');
  fs.rmSync(dir, { recursive: true, force: true });
});
