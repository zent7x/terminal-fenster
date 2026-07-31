// BGRA frame -> PNG, with no dependencies.
//
// The screenshot an agent gets is the *same buffer the terminal is drawing*, not a fresh
// Page.captureScreenshot. If the pixels in the terminal are wrong, the screenshot is wrong
// in exactly the same way, which is the only version of this tool that is useful for
// debugging the renderer.
//
// PNG here is deliberately minimal: 8-bit truecolour, no interlacing, filter type 0 on
// every scanline. zlib does the compression and Node has had it built in forever.
'use strict';

const zlib = require('zlib');

const SIGNATURE = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

// Node >= 20.15 exposes zlib.crc32. Keep a fallback so this file does not silently become
// the reason the package needs a newer runtime than the rest of the project.
let crc32 = zlib.crc32;
if (typeof crc32 !== 'function') {
  const table = new Int32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[n] = c;
  }
  crc32 = (buf) => {
    let c = -1;
    for (let i = 0; i < buf.length; i++) c = table[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
    return (c ^ -1) >>> 0;
  };
}

function chunk(type, data) {
  const len = Buffer.allocUnsafe(4);
  len.writeUInt32BE(data.length, 0);
  const typeBuf = Buffer.from(type, 'ascii');
  const crcBuf = Buffer.allocUnsafe(4);
  crcBuf.writeUInt32BE(crc32(Buffer.concat([typeBuf, data])) >>> 0, 0);
  return Buffer.concat([len, typeBuf, data, crcBuf]);
}

/// Nearest-neighbour downscale of a BGRA buffer.
///
/// Nearest neighbour, not a box filter, is the right choice here: agents look at
/// screenshots to read text and locate controls, and a box filter turns 9px UI type into
/// grey mush. Aliasing is the lesser evil.
function downscaleBGRA(pixels, width, height, targetW, targetH) {
  const out = Buffer.allocUnsafe(targetW * targetH * 4);
  const xRatio = width / targetW;
  const yRatio = height / targetH;
  for (let y = 0; y < targetH; y++) {
    const sy = Math.min(height - 1, Math.floor(y * yRatio));
    const srcRow = sy * width * 4;
    const dstRow = y * targetW * 4;
    for (let x = 0; x < targetW; x++) {
      const sx = Math.min(width - 1, Math.floor(x * xRatio));
      pixels.copy(out, dstRow + x * 4, srcRow + sx * 4, srcRow + sx * 4 + 4);
    }
  }
  return out;
}

/// Encode a BGRA buffer as a PNG Buffer.
///
/// `maxDimension` caps the longest side. This exists for token economy, not aesthetics: a
/// 2482x814 frame is ~500 KB of PNG and over a megabyte once base64'd into a model's
/// context, which is a large price for "is the button blue".
function encodeBGRA(pixels, width, height, opts = {}) {
  const maxDim = opts.maxDimension || 1024;
  let w = width;
  let h = height;
  let src = pixels;
  const longest = Math.max(width, height);
  if (maxDim > 0 && longest > maxDim) {
    const scale = maxDim / longest;
    w = Math.max(1, Math.round(width * scale));
    h = Math.max(1, Math.round(height * scale));
    src = downscaleBGRA(pixels, width, height, w, h);
  }

  // Raw scanlines: one filter byte (0 = None) then RGB. Alpha is dropped -- an offscreen
  // page composited on white has nothing useful in it and it is 25% of the bytes.
  const stride = w * 3;
  const raw = Buffer.allocUnsafe(h * (stride + 1));
  for (let y = 0; y < h; y++) {
    const o = y * (stride + 1);
    raw[o] = 0;
    const rowStart = y * w * 4;
    for (let x = 0; x < w; x++) {
      const s = rowStart + x * 4;
      const d = o + 1 + x * 3;
      raw[d] = src[s + 2];     // R  (source is BGRA)
      raw[d + 1] = src[s + 1]; // G
      raw[d + 2] = src[s];     // B
    }
  }

  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8;  // bit depth
  ihdr[9] = 2;  // colour type: truecolour
  ihdr[10] = 0; // compression: deflate
  ihdr[11] = 0; // filter method: adaptive
  ihdr[12] = 0; // interlace: none

  return {
    png: Buffer.concat([
      SIGNATURE,
      chunk('IHDR', ihdr),
      chunk('IDAT', zlib.deflateSync(raw, { level: 6 })),
      chunk('IEND', Buffer.alloc(0)),
    ]),
    width: w,
    height: h,
    scaled: w !== width || h !== height,
  };
}

module.exports = { encodeBGRA, downscaleBGRA };
