'use strict';

// Shared-texture partial damage readback (B02/B04). Electron's bitmap `dirtyRect` is always
// full-frame; `textureInfo.metadata.captureUpdateRect` reports the real damage. We still read
// pixels through `capturePage` on that rect and composite into a retained full framebuffer.

const { clampDamage, unionDamage, fullDamage } = require('./frame-pipeline');

function sharedTextureEnabled(argv, env = process.env) {
  if (argv.includes('--tf-no-shared-texture')) return false;
  if (argv.includes('--tf-shared-texture')) return true;
  const v = env.TERMINAL_FENSTER_SHARED_TEXTURE;
  if (v === '1' || v === 'true' || v === 'on') return true;
  if (v === '0' || v === 'false' || v === 'off') return false;
  // Off by default: partial readback is still experimental and can leave grey bands
  // when the retained framebuffer has not been fully primed yet.
  return false;
}

function extractDamage(event, dirty, width, height) {
  if (event?.texture) {
    try {
      const ti = event.texture.textureInfo;
      const codedW = ti?.codedSize?.width ?? width;
      const codedH = ti?.codedSize?.height ?? height;
      const rect = ti?.metadata?.captureUpdateRect;
      if (rect && rect.width > 0 && rect.height > 0) {
        return clampDamage(rect, codedW, codedH);
      }
      return fullDamage(codedW, codedH);
    } catch (_) {
      // Fall through to the paint dirty rect.
    }
  }
  return clampDamage(dirty, width, height);
}

function releaseTexture(event) {
  if (!event?.texture) return;
  try {
    event.texture.release();
  } catch (_) {
    /* already released */
  }
}

function frameSize(event, dirty, image) {
  if (event?.texture) {
    const ti = event.texture.textureInfo;
    return {
      width: ti?.codedSize?.width ?? dirty?.width ?? 0,
      height: ti?.codedSize?.height ?? dirty?.height ?? 0,
    };
  }
  if (image && !image.isEmpty()) {
    return image.getSize();
  }
  return { width: dirty?.width ?? 0, height: dirty?.height ?? 0 };
}

function ensureRetained(state, width, height) {
  const need = width * height * 4;
  if (!state.bitmap || state.width !== width || state.height !== height || state.bitmap.length !== need) {
    state.bitmap = Buffer.alloc(need);
    state.width = width;
    state.height = height;
  }
  return state.bitmap;
}

function blitBGRA(dst, dstW, x, y, w, h, src) {
  const srcStride = w * 4;
  const dstStride = dstW * 4;
  for (let row = 0; row < h; row++) {
    src.copy(dst, (y + row) * dstStride + x * 4, row * srcStride, row * srcStride + srcStride);
  }
}

class TextureCaptureQueue {
  constructor() {
    this.state = { bitmap: null, width: 0, height: 0 };
    this.pendingDamage = null;
    this.inFlight = false;
    this.stats = { captures: 0, coalesced: 0 };
  }

  schedule(win, size, damage, onFrame) {
    if (this.pendingDamage && size.width === this.state.width && size.height === this.state.height) {
      this.pendingDamage = unionDamage(this.pendingDamage, damage);
      this.stats.coalesced++;
    } else {
      this.pendingDamage = damage;
    }
    if (this.inFlight) return;
    this.inFlight = true;
    setImmediate(() => this.flush(win, size, onFrame));
  }

  async flush(win, size, onFrame) {
    let damage = this.pendingDamage || fullDamage(size.width, size.height);
    this.pendingDamage = null;
    const fresh =
      !this.state.bitmap ||
      this.state.width !== size.width ||
      this.state.height !== size.height;
    if (fresh) {
      damage = fullDamage(size.width, size.height);
    }
    try {
      const img = await win.webContents.capturePage({
        x: damage.x,
        y: damage.y,
        width: damage.width,
        height: damage.height,
      });
      const partial = img.toBitmap();
      const bitmap = ensureRetained(this.state, size.width, size.height);
      blitBGRA(bitmap, size.width, damage.x, damage.y, damage.width, damage.height, partial);
      this.stats.captures++;
      onFrame(size.width, size.height, damage, bitmap);
    } catch (_) {
      /* skip frame on capture failure */
    } finally {
      this.inFlight = false;
      if (this.pendingDamage) {
        setImmediate(() => this.flush(win, size, onFrame));
      }
    }
  }
}

module.exports = {
  TextureCaptureQueue,
  blitBGRA,
  ensureRetained,
  extractDamage,
  frameSize,
  releaseTexture,
  sharedTextureEnabled,
};
