'use strict';

// Pure frame-path helpers. Keeping these outside Electron makes the damage/coalescing rules
// executable in ordinary Node tests; a stale rectangle here becomes stale pixels on screen.

const FRAME_HEADER_LEN = 32;

function fullDamage(width, height) {
  return { x: 0, y: 0, width, height };
}

function clampDamage(dirty, width, height) {
  if (!dirty || width < 1 || height < 1) return fullDamage(width, height);
  const x = Math.max(0, Math.min(dirty.x | 0, width));
  const y = Math.max(0, Math.min(dirty.y | 0, height));
  const w = Math.max(0, Math.min(dirty.width | 0, width - x));
  const h = Math.max(0, Math.min(dirty.height | 0, height - y));
  return w === 0 || h === 0 ? fullDamage(width, height) : { x, y, width: w, height: h };
}

function unionDamage(a, b) {
  const x = Math.min(a.x, b.x);
  const y = Math.min(a.y, b.y);
  const right = Math.max(a.x + a.width, b.x + b.width);
  const bottom = Math.max(a.y + a.height, b.y + b.height);
  return { x, y, width: right - x, height: bottom - y };
}

function newFrame(seq, width, height, dirty, bitmap) {
  return { seq, width, height, dirty: clampDamage(dirty, width, height), bitmap };
}

// The latest bitmap contains the current pixels for the whole viewport, but damage from every
// paint that will not reach the wire must survive. Unioning damage is safe overdraw; replacing
// it is a visible stale-region bug.
function coalesceFrame(pending, next) {
  if (!pending) return { frame: next, coalesced: false, resized: false };
  if (pending.width !== next.width || pending.height !== next.height) {
    next.dirty = fullDamage(next.width, next.height);
    return { frame: next, coalesced: true, resized: true };
  }
  next.dirty = unionDamage(pending.dirty, next.dirty);
  return { frame: next, coalesced: true, resized: false };
}

// Crop the changed rectangle out of a full BGRA frame. Chromium's bitmap is non-strided
// (row stride == width*4), so each crop row is a contiguous source slice.
function cropBGRA(bitmap, imgW, dirty) {
  const srcStride = imgW * 4;
  const dstStride = dirty.width * 4;
  const out = Buffer.allocUnsafe(dirty.width * dirty.height * 4);
  for (let row = 0; row < dirty.height; row++) {
    const srcStart = (dirty.y + row) * srcStride + dirty.x * 4;
    bitmap.copy(out, row * dstStride, srcStart, srcStart + dstStride);
  }
  return out;
}

// Return separate header/pixel buffers so net.Socket can gather-write them. Concatenating here
// and again in the outer message framing used to copy two full viewports per paint.
function encodeFrameParts(frame) {
  const { dirty, width, height } = frame;
  const isFull = dirty.x === 0 && dirty.y === 0 && dirty.width === width && dirty.height === height;
  const pixels = isFull ? frame.bitmap : cropBGRA(frame.bitmap, width, dirty);
  const head = Buffer.allocUnsafe(FRAME_HEADER_LEN);
  head.writeUInt32BE(frame.seq, 0);
  head.writeUInt32BE(width, 4);
  head.writeUInt32BE(height, 8);
  head.writeUInt32BE(dirty.x, 12);
  head.writeUInt32BE(dirty.y, 16);
  head.writeUInt32BE(dirty.width, 20);
  head.writeUInt32BE(dirty.height, 24);
  head.writeUInt32BE(0, 28); // format 0 = BGRA8888
  return { head, pixels, isFull };
}

module.exports = {
  FRAME_HEADER_LEN,
  clampDamage,
  coalesceFrame,
  encodeFrameParts,
  fullDamage,
  newFrame,
  unionDamage,
};
