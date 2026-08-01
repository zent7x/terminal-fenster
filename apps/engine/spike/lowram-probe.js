// SPIKE: which Chromium switches actually reduce the offscreen engine's memory footprint?
//
// engine-rss.js measured the shipping engine at ~281 MB (about:blank). This spike answers the
// follow-on question the P1.2 low-RAM task needs: of the switches people reach for, which ones
// really move RSS on OUR offscreen configuration, and — the part that is easy to get wrong —
// which of them do so without breaking rendering. A flag that saves 40 MB and paints nothing is
// a regression, not a win, so every config is required to produce a real paint before its
// number counts.
//
// Design mirrors the B02 capability probe: command-line switches are process-global and must be
// applied before app-ready, so each config is a SEPARATE electron process. The process measures
// its own tree RSS via /bin/ps (absolute path — a stripped PATH would zero it) and prints one
// __RESULT__<json> line. Drive it with the shell loop in the trailer, which diffs against the
// baseline config.
//
// Usage:
//   electron lowram-probe.js --config=baseline
//   electron lowram-probe.js --config=v8cap
//   electron lowram-probe.js --config=fewproc
//   electron lowram-probe.js --config=combined --url=https://example.com
'use strict';

const { app, BrowserWindow } = require('electron');
const { execSync } = require('child_process');

function arg(name, dflt) {
  const hit = process.argv.find((a) => a.startsWith(`--${name}=`));
  return hit ? hit.slice(name.length + 3) : dflt;
}

const CONFIG = arg('config', 'baseline');
const URL = arg('url', 'about:blank');
const W = parseInt(arg('width', '1280'), 10);
const H = parseInt(arg('height', '800'), 10);
const SETTLE_MS = parseInt(arg('settle', '3500'), 10);

// Each config is a list of [switch, value?] applied before ready. Rationale per switch is in
// the trailer. `hwaccel:false` is handled specially (disableHardwareAcceleration, not a switch).
const CONFIGS = {
  baseline: { switches: [], hwaccel: true },
  // Cap V8's old-space. A browser chrome that never runs heavy app JS does not need the default
  // multi-hundred-MB heap ceiling.
  v8cap: { switches: [['js-flags', '--max-old-space-size=128']], hwaccel: true },
  // One renderer, no site-isolation process explosion. A single-tab browser gains nothing from
  // per-site processes and pays a fixed per-process cost for each.
  fewproc: {
    switches: [['renderer-process-limit', '1'], ['disable-site-isolation-trials', '']],
    hwaccel: true,
  },
  // Drop the GPU process. B02 showed this machine already composites OSR in software, so the GPU
  // process is largely dead weight here.
  nogpu: { switches: [], hwaccel: false },
  // Everything that held its number, together.
  combined: {
    switches: [
      ['js-flags', '--max-old-space-size=128'],
      ['renderer-process-limit', '1'],
      ['disable-site-isolation-trials', ''],
      ['disable-features', 'Translate,MediaRouter,OptimizationHints'],
    ],
    hwaccel: false,
  },
};

const cfg = CONFIGS[CONFIG];
if (!cfg) {
  console.log('__RESULT__' + JSON.stringify({ config: CONFIG, error: 'unknown config' }));
  process.exit(2);
}

for (const [k, v] of cfg.switches) {
  if (v === '') app.commandLine.appendSwitch(k);
  else app.commandLine.appendSwitch(k, v);
}
if (!cfg.hwaccel) app.disableHardwareAcceleration();

app.on('window-all-closed', () => {});

function treeRssKb(rootPid) {
  const out = execSync('/bin/ps -Ao pid=,ppid=,rss=', { encoding: 'utf8' });
  const kids = new Map();
  const rss = new Map();
  for (const line of out.split('\n')) {
    const m = line.trim().match(/^(\d+)\s+(\d+)\s+(\d+)$/);
    if (!m) continue;
    rss.set(+m[1], +m[3]);
    if (!kids.has(+m[2])) kids.set(+m[2], []);
    kids.get(+m[2]).push(+m[1]);
  }
  let total = 0;
  let procs = 0;
  const stack = [rootPid];
  const seen = new Set();
  while (stack.length) {
    const pid = stack.pop();
    if (seen.has(pid)) continue;
    seen.add(pid);
    if (rss.has(pid)) {
      total += rss.get(pid);
      procs++;
    }
    for (const c of kids.get(pid) || []) stack.push(c);
  }
  return { kb: total, procs };
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

app.whenReady().then(async () => {
  if (app.dock) app.dock.hide();
  let paints = 0;
  const win = new BrowserWindow({
    show: false,
    width: W,
    height: H,
    webPreferences: {
      offscreen: true,
      nodeIntegration: false,
      contextIsolation: true,
      sandbox: true,
      webSecurity: true,
    },
  });
  win.webContents.setFrameRate(60);
  win.webContents.on('paint', () => {
    paints++;
  });

  try {
    await new Promise((resolve, reject) => {
      const t = setTimeout(() => reject(new Error('load timeout 15s')), 15000);
      win.webContents.once('did-finish-load', () => {
        clearTimeout(t);
        resolve();
      });
      win.webContents.once('did-fail-load', (_e, code, desc) => {
        clearTimeout(t);
        reject(new Error(`did-fail-load ${code} ${desc}`));
      });
      win.loadURL(URL);
    });
    // Nudge a repaint so even about:blank yields at least one paint to prove rendering works
    // under this config.
    await win.webContents.executeJavaScript('document.body && (document.body.style.background="#123");', true).catch(() => {});
    await sleep(SETTLE_MS);

    const s = treeRssKb(process.pid);
    console.log(
      '__RESULT__' +
        JSON.stringify({
          config: CONFIG,
          url: URL,
          viewport: `${W}x${H}`,
          rss_mb: +(s.kb / 1024).toFixed(1),
          procs: s.procs,
          paints,
          renders: paints > 0,
          electron: process.versions.electron,
          chrome: process.versions.chrome,
        })
    );
  } catch (e) {
    console.log('__RESULT__' + JSON.stringify({ config: CONFIG, error: String(e.message || e) }));
  }
  app.exit(0);
});

// ---------------------------------------------------------------------------------------------
// Driver (run with the agent sandbox disabled, from apps/engine):
//
//   for c in baseline v8cap fewproc nogpu combined; do
//     ./node_modules/.bin/electron spike/lowram-probe.js --config=$c 2>/dev/null | grep __RESULT__
//   done
//
// Read rss_mb per config and diff against baseline. A config with renders:false is disqualified
// no matter how low its RSS.
