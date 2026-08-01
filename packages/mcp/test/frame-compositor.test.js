'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { compositeFrame, locateEngine, FRAME_HEADER_LEN } = require('../lib/engine');

function wireFrame(seq, width, height, dirty, pixels, format = 0) {
  const [x, y, w, h] = dirty;
  const out = Buffer.alloc(FRAME_HEADER_LEN + pixels.length);
  [seq, width, height, x, y, w, h, format].forEach((value, i) => {
    out.writeUInt32BE(value, i * 4);
  });
  pixels.copy(out, FRAME_HEADER_LEN);
  return out;
}

function solidPixels(count, bgra) {
  return Buffer.from(Array.from({ length: count }, () => bgra).flat());
}

function pixel(frame, x, y) {
  const i = (y * frame.width + x) * 4;
  return [...frame.pixels.subarray(i, i + 4)];
}

test('full frame establishes the retained BGRA canvas', () => {
  const frame = compositeFrame(wireFrame(
    1, 2, 2, [0, 0, 2, 2], solidPixels(4, [0, 0, 255, 255])
  ));
  assert.equal(frame.seq, 1);
  assert.equal(frame.pixels.length, 16);
  assert.deepEqual(pixel(frame, 1, 1), [0, 0, 255, 255]);
});

test('partial damage changes only its destination rectangle', () => {
  const base = compositeFrame(wireFrame(
    1, 3, 2, [0, 0, 3, 2], solidPixels(6, [0, 255, 0, 255])
  ));
  const updated = compositeFrame(wireFrame(
    2, 3, 2, [1, 0, 1, 2], solidPixels(2, [0, 0, 255, 255])
  ), base);

  assert.equal(updated.seq, 2);
  assert.strictEqual(updated.pixels, base.pixels, 'damage should reuse the retained canvas');
  assert.deepEqual(pixel(updated, 0, 0), [0, 255, 0, 255]);
  assert.deepEqual(pixel(updated, 1, 0), [0, 0, 255, 255]);
  assert.deepEqual(pixel(updated, 1, 1), [0, 0, 255, 255]);
  assert.deepEqual(pixel(updated, 2, 1), [0, 255, 0, 255]);
});

test('partial frame without a matching full base is rejected', () => {
  const partial = wireFrame(1, 4, 4, [1, 1, 1, 1], solidPixels(1, [1, 2, 3, 4]));
  assert.throws(() => compositeFrame(partial), /before a matching full base/);

  const oldGeometry = compositeFrame(wireFrame(
    1, 2, 2, [0, 0, 2, 2], solidPixels(4, [0, 0, 0, 255])
  ));
  assert.throws(() => compositeFrame(partial, oldGeometry), /before a matching full base/);
});

test('malformed dirty geometry and payload lengths are rejected', () => {
  const outside = wireFrame(1, 4, 4, [3, 3, 2, 2], solidPixels(4, [0, 0, 0, 0]));
  assert.throws(() => compositeFrame(outside), /outside/);

  const truncated = wireFrame(1, 4, 4, [0, 0, 4, 4], Buffer.alloc(8));
  assert.throws(() => compositeFrame(truncated), /expected 64/);
  assert.throws(() => compositeFrame(Buffer.alloc(12)), /Truncated frame header/);
});

test('a full frame at new geometry replaces the retained canvas', () => {
  const old = compositeFrame(wireFrame(
    1, 2, 2, [0, 0, 2, 2], solidPixels(4, [0, 0, 0, 255])
  ));
  const next = compositeFrame(wireFrame(
    2, 1, 1, [0, 0, 1, 1], solidPixels(1, [9, 8, 7, 6])
  ), old);
  assert.equal(next.width, 1);
  assert.equal(next.height, 1);
  assert.notStrictEqual(next.pixels, old.pixels);
  assert.deepEqual(pixel(next, 0, 0), [9, 8, 7, 6]);
});

test('explicit engine override rejects an npm half-install instead of falling back', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'terminal-fenster-mcp-engine-half-'));
  const previous = process.env.TERMINAL_FENSTER_ENGINE;
  try {
    fs.mkdirSync(path.join(root, 'src'), { recursive: true });
    fs.mkdirSync(path.join(root, 'node_modules/.bin'), { recursive: true });
    fs.writeFileSync(path.join(root, 'src/main.js'), '');
    fs.writeFileSync(path.join(root, 'node_modules/.bin/electron'), '');
    process.env.TERMINAL_FENSTER_ENGINE = root;
    assert.throws(() => locateEngine(), /runtime is not downloaded/);
  } finally {
    if (previous === undefined) delete process.env.TERMINAL_FENSTER_ENGINE;
    else process.env.TERMINAL_FENSTER_ENGINE = previous;
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test('engine discovery verifies the runtime executable named by path.txt', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'terminal-fenster-mcp-engine-ready-'));
  const previous = process.env.TERMINAL_FENSTER_ENGINE;
  try {
    fs.mkdirSync(path.join(root, 'src'), { recursive: true });
    fs.mkdirSync(path.join(root, 'node_modules/.bin'), { recursive: true });
    fs.mkdirSync(path.join(root, 'node_modules/electron/dist'), { recursive: true });
    fs.writeFileSync(path.join(root, 'src/main.js'), '');
    fs.writeFileSync(path.join(root, 'node_modules/.bin/electron'), '');
    fs.writeFileSync(path.join(root, 'node_modules/electron/path.txt'), 'runtime\n');
    fs.writeFileSync(path.join(root, 'node_modules/electron/dist/runtime'), 'binary');
    process.env.TERMINAL_FENSTER_ENGINE = root;
    const found = locateEngine();
    assert.equal(found.dir, root);
    assert.equal(found.runtime, path.join(root, 'node_modules/electron/dist/runtime'));
  } finally {
    if (previous === undefined) delete process.env.TERMINAL_FENSTER_ENGINE;
    else process.env.TERMINAL_FENSTER_ENGINE = previous;
    fs.rmSync(root, { recursive: true, force: true });
  }
});
