'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { installDenyAllPermissions, isAllowedTopLevelUrl } = require('../src/security-policy');

test('top-level navigation allows browser schemes and blocks external application schemes', () => {
  for (const url of [
    'https://example.com/',
    'http://127.0.0.1:8080/',
    'about:blank',
    'about:blank#ready',
    'data:text/plain,hello',
    'file:///tmp/page.html',
    'blob:https://example.com/1234',
  ]) {
    assert.equal(isAllowedTopLevelUrl(url), true, url);
  }
  for (const url of [
    'zoommtg://join?action=join',
    'mailto:attacker@example.com',
    'javascript:alert(1)',
    'about:crash',
    'not a url',
    '',
  ]) {
    assert.equal(isAllowedTopLevelUrl(url), false, url);
  }
  assert.equal(isAllowedTopLevelUrl('file:///tmp/secrets.txt', true), false);
  assert.equal(isAllowedTopLevelUrl('blob:https://example.com/1234', true), false);
  assert.equal(isAllowedTopLevelUrl('data:text/plain,hello', true), true);
  assert.equal(isAllowedTopLevelUrl('data:text/plain,' + 'x'.repeat(64 * 1024), true), false);
});

test('permission policy denies checks, requests, and devices while reporting requests', () => {
  const installed = {};
  const events = [];
  const fakeSession = {
    setPermissionCheckHandler(fn) { installed.check = fn; },
    setPermissionRequestHandler(fn) { installed.request = fn; },
    setDevicePermissionHandler(fn) { installed.device = fn; },
  };
  installDenyAllPermissions(fakeSession, (event) => events.push(event));

  assert.equal(installed.check(null, 'clipboard-read', 'https://evil.test'), false);
  assert.equal(installed.device({ deviceType: 'usb', origin: 'https://evil.test' }), false);
  let decision;
  installed.request(
    { getURL: () => 'https://fallback.test/' },
    'geolocation',
    (granted) => { decision = granted; },
    { requestingUrl: 'https://evil.test/map' }
  );
  assert.equal(decision, false);
  assert.deepEqual(events, [{
    t: 'permissionDenied',
    permission: 'geolocation',
    url: 'https://evil.test/map',
  }]);
});
