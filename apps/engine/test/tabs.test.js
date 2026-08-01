'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');

// Minimal stubs so TabManager can be unit-tested without Electron.
class FakeWebContents {
  constructor() {
    this.handlers = new Map();
    this.destroyed = false;
  }
  on(evt, fn) {
    if (!this.handlers.has(evt)) this.handlers.set(evt, []);
    this.handlers.get(evt).push(fn);
  }
  emit(evt, ...args) {
    for (const fn of this.handlers.get(evt) || []) fn({}, ...args);
  }
  invalidate() {}
  loadURL() {
    return Promise.resolve();
  }
  setFrameRate() {}
  setWindowOpenHandler() {
    return { action: 'deny' };
  }
}

class FakeWindow {
  constructor() {
    this.webContents = new FakeWebContents();
    this.destroyed = false;
  }
  isDestroyed() {
    return this.destroyed;
  }
  destroy() {
    this.destroyed = true;
  }
  setSize() {}
}

const { TabManager } = require('../src/tabs');

test('TabManager starts with one tab and emits tabs event', () => {
  const events = [];
  const mgr = new TabManager({
    createWindow: (w, h, wire) => {
      const win = new FakeWindow();
      wire(win);
      return win;
    },
    sendEvent: (o) => events.push(o),
    onActiveChange: () => {},
    agentMode: false,
  });
  mgr.start(800, 600, 'about:blank');
  assert.equal(mgr.tabs.length, 1);
  const tabsEvt = events.find((e) => e.t === 'tabs');
  assert.ok(tabsEvt);
  assert.equal(tabsEvt.n, 1);
  assert.equal(tabsEvt.active, 0);
});

test('TabManager newTab and switchTab update active index', () => {
  const events = [];
  const mgr = new TabManager({
    createWindow: (w, h, wire) => {
      const win = new FakeWindow();
      wire(win);
      return win;
    },
    sendEvent: (o) => events.push(o),
    onActiveChange: () => {},
    agentMode: false,
  });
  mgr.start(800, 600, 'about:blank');
  mgr.newTab();
  assert.equal(mgr.tabs.length, 2);
  assert.equal(mgr.active, 1);
  mgr.switchTab(0);
  assert.equal(mgr.active, 0);
  const last = events.filter((e) => e.t === 'tabs').pop();
  assert.equal(last.active, 0);
});

test('TabManager closeTab refuses to close the last tab', () => {
  const mgr = new TabManager({
    createWindow: (w, h, wire) => {
      const win = new FakeWindow();
      wire(win);
      return win;
    },
    sendEvent: () => {},
    onActiveChange: () => {},
    agentMode: false,
  });
  mgr.start(800, 600, 'about:blank');
  assert.equal(mgr.closeTab(), false);
  assert.equal(mgr.tabs.length, 1);
});

test('TabManager bounds renderer growth at sixteen tabs', () => {
  const events = [];
  const mgr = new TabManager({
    createWindow: (w, h, wire) => {
      const win = new FakeWindow();
      wire(win);
      return win;
    },
    sendEvent: (o) => events.push(o),
    onActiveChange: () => {},
    agentMode: false,
  });
  mgr.start(800, 600, 'about:blank');
  for (let i = 0; i < 20; i++) mgr.newTab();
  assert.equal(mgr.tabs.length, 16);
  assert.ok(events.some((event) => event.t === 'tabLimit' && event.max === 16));
});
