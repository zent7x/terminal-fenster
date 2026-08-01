'use strict';

const { test } = require('node:test');
const assert = require('node:assert/strict');
const {
  extractDamage,
  sharedTextureEnabled,
  blitBGRA,
  ensureRetained,
} = require('../src/frame-capture');
const { fullDamage } = require('../src/frame-pipeline');

test('shared texture is opt-in', () => {
  assert.equal(sharedTextureEnabled([]), false);
  assert.equal(sharedTextureEnabled(['--tf-shared-texture']), true);
  assert.equal(sharedTextureEnabled([], { TERMINAL_FENSTER_SHARED_TEXTURE: '1' }), true);
  assert.equal(sharedTextureEnabled(['--tf-no-shared-texture']), false);
  assert.equal(sharedTextureEnabled([], { TERMINAL_FENSTER_SHARED_TEXTURE: '0' }), false);
});

test('captureUpdateRect overrides full-frame dirty rect', () => {
  const event = {
    texture: {
      textureInfo: {
        codedSize: { width: 800, height: 600 },
        metadata: { captureUpdateRect: { x: 10, y: 20, width: 40, height: 50 } },
      },
    },
  };
  const dirty = fullDamage(800, 600);
  const d = extractDamage(event, dirty, 800, 600);
  assert.deepEqual(d, { x: 10, y: 20, width: 40, height: 50 });
});

test('retained framebuffer blit preserves untouched pixels', () => {
  const state = { bitmap: null, width: 0, height: 0 };
  const full = ensureRetained(state, 4, 4);
  full.fill(0xff);
  const patch = Buffer.from([0, 255, 0, 255, 0, 255, 0, 255]);
  blitBGRA(full, 4, 1, 1, 2, 1, patch);
  assert.equal(full[0], 0xff);
  assert.equal(full[(1 * 4 + 1) * 4 + 1], 255);
});
