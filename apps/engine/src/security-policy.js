'use strict';

// Top-level documents may stay on browser-native schemes only. In particular, a hostile page
// must not turn `location.href = "zoommtg:..."` (or another registered application protocol)
// into a silent host-application launch through Electron's external-protocol permission.
const ALLOWED_TOP_LEVEL_PROTOCOLS = new Set(['https:', 'http:', 'data:', 'file:', 'blob:']);

function isAllowedTopLevelUrl(raw, agentMode = false) {
  if (typeof raw !== 'string' || raw.length === 0 || raw.length > 2 * 1024 * 1024) return false;
  try {
    const parsed = new URL(raw);
    if (agentMode && (parsed.protocol === 'file:' || parsed.protocol === 'blob:')) return false;
    if (agentMode && parsed.protocol === 'data:' && Buffer.byteLength(raw, 'utf8') > 64 * 1024) {
      return false;
    }
    if (ALLOWED_TOP_LEVEL_PROTOCOLS.has(parsed.protocol)) return true;
    return parsed.protocol === 'about:' && parsed.pathname === 'blank';
  } catch (_) {
    return false;
  }
}

function requestingUrl(webContents, details) {
  if (details && typeof details.requestingUrl === 'string') return details.requestingUrl;
  if (details && typeof details.securityOrigin === 'string') return details.securityOrigin;
  try {
    return webContents && typeof webContents.getURL === 'function' ? webContents.getURL() : '';
  } catch (_) {
    return '';
  }
}

// Electron approves web permission requests unless the application installs a handler. A
// terminal cannot show Chromium's native prompts, so the only honest first-release policy is to
// deny every privileged capability and tell the core what was denied. Interactive grants can be
// added later behind terminal chrome that shows the origin and asks the user explicitly.
function installDenyAllPermissions(session, emit = () => {}) {
  session.setPermissionCheckHandler(() => false);
  session.setPermissionRequestHandler((webContents, permission, callback, details) => {
    emit({
      t: 'permissionDenied',
      permission: typeof permission === 'string' ? permission : 'unknown',
      url: requestingUrl(webContents, details),
    });
    callback(false);
  });
  session.setDevicePermissionHandler(() => false);
}

module.exports = { installDenyAllPermissions, isAllowedTopLevelUrl };
