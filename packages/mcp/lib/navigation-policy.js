'use strict';

const MAX_DATA_URL_BYTES = 64 * 1024;

// Agent browsing is intentionally narrower than an explicit human CLI action. A prompt-injected
// page must not convince the model to read a local file, mint a blob navigation, or feed an
// unbounded inline document into Chromium.
function validateAgentNavigation(raw) {
  if (typeof raw !== 'string' || raw.length === 0) throw new Error('url is required');
  let parsed;
  try { parsed = new URL(raw); } catch { throw new Error('url must be an absolute URL'); }
  if (parsed.protocol === 'https:' || parsed.protocol === 'http:') return raw;
  if (parsed.protocol === 'about:' && parsed.pathname === 'blank') return raw;
  if (parsed.protocol === 'data:') {
    if (Buffer.byteLength(raw, 'utf8') > MAX_DATA_URL_BYTES) {
      throw new Error(`data URL exceeds the ${MAX_DATA_URL_BYTES}-byte agent limit`);
    }
    return raw;
  }
  throw new Error(
    `scheme ${parsed.protocol} is not allowed for agent navigation; use http, https, about:blank, or a data URL up to 64 KiB`
  );
}

module.exports = { validateAgentNavigation, MAX_DATA_URL_BYTES };
