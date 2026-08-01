'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const {
  clampDamage,
  coalesceFrame,
  encodeFrameParts,
  newFrame,
  unionDamage,
} = require('../src/frame-pipeline');

function bitmap(width, height) {
  const out = Buffer.alloc(width * height * 4);
  for (let i = 0; i < width * height; i++) {
    out.set([i, i + 1, i + 2, 255], i * 4);
  }
  return out;
}

test('damage is clamped and degenerate damage safely becomes full-frame', () => {
  assert.deepEqual(clampDamage({ x: 3, y: 1, width: 9, height: 8 }, 5, 4), {
    x: 3, y: 1, width: 2, height: 3,
  });
  assert.deepEqual(clampDamage({ x: 9, y: 9, width: 1, height: 1 }, 5, 4), {
    x: 0, y: 0, width: 5, height: 4,
  });
});

test('coalescing preserves all skipped damage while retaining the newest bitmap', () => {
  const first = newFrame(1, 8, 6, { x: 1, y: 1, width: 2, height: 2 }, Buffer.alloc(192, 1));
  const latestPixels = Buffer.alloc(192, 2);
  const next = newFrame(2, 8, 6, { x: 6, y: 4, width: 2, height: 2 }, latestPixels);
  const { frame, coalesced, resized } = coalesceFrame(first, next);
  assert.equal(coalesced, true);
  assert.equal(resized, false);
  assert.deepEqual(frame.dirty, { x: 1, y: 1, width: 7, height: 5 });
  assert.equal(frame.bitmap, latestPixels);
});

test('a geometry change promotes coalesced damage to a full update', () => {
  const first = newFrame(1, 8, 6, { x: 1, y: 1, width: 1, height: 1 }, Buffer.alloc(192));
  const next = newFrame(2, 4, 3, { x: 2, y: 1, width: 1, height: 1 }, Buffer.alloc(48));
  const { frame, resized } = coalesceFrame(first, next);
  assert.equal(resized, true);
  assert.deepEqual(frame.dirty, { x: 0, y: 0, width: 4, height: 3 });
});

test('frame encoding returns a zero-copy full bitmap and an exact partial crop', () => {
  const source = bitmap(4, 3);
  const full = encodeFrameParts(newFrame(7, 4, 3, { x: 0, y: 0, width: 4, height: 3 }, source));
  assert.equal(full.isFull, true);
  assert.equal(full.pixels, source);
  assert.equal(full.head.readUInt32BE(0), 7);
  assert.equal(full.head.readUInt32BE(20), 4);
  assert.equal(full.head.readUInt32BE(24), 3);

  const partial = encodeFrameParts(newFrame(8, 4, 3, { x: 1, y: 1, width: 2, height: 2 }, source));
  assert.equal(partial.isFull, false);
  assert.equal(partial.pixels.length, 2 * 2 * 4);
  assert.deepEqual([...partial.pixels], [
    ...source.subarray((1 * 4 + 1) * 4, (1 * 4 + 3) * 4),
    ...source.subarray((2 * 4 + 1) * 4, (2 * 4 + 3) * 4),
  ]);
});

test('rectangle union is the smallest box covering both damages', () => {
  assert.deepEqual(
    unionDamage(
      { x: 10, y: 5, width: 4, height: 10 },
      { x: 2, y: 12, width: 9, height: 3 },
    ),
    { x: 2, y: 5, width: 12, height: 10 },
  );
});
