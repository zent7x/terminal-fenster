'use strict';

const { isAllowedTopLevelUrl } = require('./security-policy');

let nextTabId = 1;
const MAX_TABS = 16;

function boundedString(value, max) {
  const text = typeof value === 'string' ? value : '';
  return text.length <= max ? text : text.slice(0, max);
}

/**
 * Multi-tab offscreen browser manager. Only the active tab paints to the terminal;
 * inactive tabs keep their session state in hidden BrowserWindows.
 */
class TabManager {
  /**
   * @param {{
   *   createWindow: (w: number, h: number, wire: (b: import('electron').BrowserWindow) => void) => import('electron').BrowserWindow,
   *   sendEvent: (obj: object) => void,
   *   onActiveChange: (prev: import('electron').BrowserWindow | null, next: import('electron').BrowserWindow | null) => void,
   *   agentMode: boolean,
   * }} opts
   */
  constructor(opts) {
    this.createWindow = opts.createWindow;
    this.sendEvent = opts.sendEvent;
    this.onActiveChange = opts.onActiveChange;
    this.agentMode = opts.agentMode;
    /** @type {{ id: number, win: import('electron').BrowserWindow, title: string, url: string, loading: boolean }[]} */
    this.tabs = [];
    this.active = 0;
    this.w = 1;
    this.h = 1;
  }

  get activeTab() {
    return this.tabs[this.active];
  }

  get activeWin() {
    return this.activeTab?.win ?? null;
  }

  start(width, height, initialUrl) {
    this.w = Math.max(1, width);
    this.h = Math.max(1, height);
    const tab = this._spawnTab();
    this.tabs = [tab];
    this.active = 0;
    if (initialUrl && initialUrl !== 'about:blank') {
      this._navigate(tab, initialUrl);
    }
    this._syncActiveMeta();
    this._emitTabs();
    return tab.win;
  }

  newTab(url) {
    if (this.tabs.length >= MAX_TABS) {
      this.sendEvent({ t: 'tabLimit', max: MAX_TABS });
      return null;
    }
    const tab = this._spawnTab();
    this.tabs.push(tab);
    this.switchTab(this.tabs.length - 1);
    if (url && url !== 'about:blank') {
      this._navigate(tab, url);
    }
    return tab;
  }

  switchTab(index) {
    if (index < 0 || index >= this.tabs.length) return;
    const prev = this.activeWin;
    this.active = index;
    const next = this.activeWin;
    this.onActiveChange(prev, next);
    this._syncActiveMeta();
    if (next && !next.isDestroyed()) {
      next.webContents.invalidate();
    }
    this._emitTabs();
  }

  switchRelative(delta) {
    if (this.tabs.length < 2) return;
    const n = this.tabs.length;
    this.switchTab((this.active + delta + n) % n);
  }

  closeTab(index) {
    if (this.tabs.length <= 1) return false;
    const idx = typeof index === 'number' ? index : this.active;
    if (idx < 0 || idx >= this.tabs.length) return false;
    const [removed] = this.tabs.splice(idx, 1);
    if (!removed.win.isDestroyed()) removed.win.destroy();
    if (this.active >= this.tabs.length) {
      this.active = this.tabs.length - 1;
    } else if (idx < this.active) {
      this.active -= 1;
    }
    const prev = removed.win;
    const next = this.activeWin;
    this.onActiveChange(prev, next);
    this._syncActiveMeta();
    if (next && !next.isDestroyed()) next.webContents.invalidate();
    this._emitTabs();
    return true;
  }

  resize(w, h) {
    this.w = Math.max(1, w);
    this.h = Math.max(1, h);
    for (const tab of this.tabs) {
      if (!tab.win.isDestroyed()) tab.win.setSize(this.w, this.h);
    }
  }

  navigate(url) {
    const tab = this.activeTab;
    if (!tab || tab.win.isDestroyed()) return;
    this._navigate(tab, url);
  }

  _spawnTab() {
    const tab = {
      id: nextTabId++,
      win: null,
      title: 'New Tab',
      url: 'about:blank',
      loading: false,
    };
    tab.win = this.createWindow(this.w, this.h, (b) => this._wireTab(tab, b));
    return tab;
  }

  _wireTab(tab, b) {
    const wc = b.webContents;
    wc.on('page-title-updated', (_e, title) => {
      tab.title = boundedString(title, 512) || 'New Tab';
      if (this.activeTab === tab) this.sendEvent({ t: 'title', v: tab.title });
      this._emitTabs();
    });
    wc.on('did-navigate', (_e, url) => {
      tab.url = boundedString(url, 8192);
      if (this.activeTab === tab) this.sendEvent({ t: 'url', v: tab.url });
      this._emitTabs();
    });
    wc.on('did-navigate-in-page', (_e, url) => {
      tab.url = boundedString(url, 8192);
      if (this.activeTab === tab) this.sendEvent({ t: 'url', v: tab.url });
      this._emitTabs();
    });
    wc.on('did-start-loading', () => {
      tab.loading = true;
      if (this.activeTab === tab) this.sendEvent({ t: 'loading', v: true });
      this._emitTabs();
    });
    wc.on('did-stop-loading', () => {
      tab.loading = false;
      if (this.activeTab === tab) this.sendEvent({ t: 'loading', v: false });
      this._emitTabs();
    });
    wc.on('did-fail-load', (_e, code, desc, url, isMainFrame) => {
      if (!isMainFrame) return;
      if (this.activeTab !== tab) return;
      this.sendEvent({ t: 'loadError', code, desc, url });
    });
    const guardNavigation = (event, legacyUrl) => {
      const url = typeof event?.url === 'string' ? event.url : legacyUrl;
      if (isAllowedTopLevelUrl(url, this.agentMode)) return;
      event.preventDefault();
      this.sendEvent({ t: 'navigationBlocked', url: typeof url === 'string' ? url : '' });
    };
    wc.on('will-navigate', guardNavigation);
    wc.on('will-redirect', guardNavigation);
    wc.on('render-process-gone', (_e, details) => {
      if (this.activeTab !== tab) return;
      this.sendEvent({ t: 'crash', reason: details.reason, exitCode: details.exitCode });
    });
    wc.setWindowOpenHandler(({ url }) => {
      if (url && isAllowedTopLevelUrl(url, this.agentMode)) {
        this.newTab(url);
      } else {
        this.sendEvent({ t: 'popup', url: url || '' });
      }
      return { action: 'deny' };
    });
  }

  _navigate(tab, url) {
    if (!isAllowedTopLevelUrl(url, this.agentMode)) {
      this.sendEvent({ t: 'navigationBlocked', url: typeof url === 'string' ? url : '' });
      return;
    }
    tab.url = boundedString(url, 8192);
    tab.win.loadURL(url).catch(() => {});
    if (this.activeTab === tab) this.sendEvent({ t: 'url', v: tab.url });
    this._emitTabs();
  }

  _syncActiveMeta() {
    const tab = this.activeTab;
    if (!tab) return;
    this.sendEvent({ t: 'title', v: tab.title });
    this.sendEvent({ t: 'url', v: tab.url });
    this.sendEvent({ t: 'loading', v: tab.loading });
  }

  _emitTabs() {
    const payload = { t: 'tabs', active: this.active, n: this.tabs.length };
    this.tabs.forEach((tab, i) => {
      payload[`title${i}`] = tab.title;
      payload[`url${i}`] = tab.url;
      payload[`loading${i}`] = tab.loading;
    });
    this.sendEvent(payload);
  }
}

module.exports = { TabManager };
