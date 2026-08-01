#!/usr/bin/env node
// Headless memory probe for the Terminal-Fenster Electron engine.
//
// The engine (Chromium OSR) is the entire RAM story — the Rust core is a rounding error next
// to it — and unlike benchmarks/bench.mjs this needs no graphics terminal, because it drives
// the engine over the unix socket exactly like the e2e harness and never draws anything. That
// makes it runnable in CI / under the agent sandbox, which is the only place a "does it fit on
// a low-RAM box?" number can be produced without a human at a terminal.
//
// It reports peak and steady-state resident set of the whole Chromium process tree, so it also
// measures the effect of the low-RAM levers (--fps idle throttle, future --disable-features):
// run it twice and diff.
//
//   node benchmarks/engine-rss.js                          # about:blank, 1280x800, 8s
//   node benchmarks/engine-rss.js --url https://example.com --fps 10 --duration 12000
//   node benchmarks/engine-rss.js --json                   # machine-readable
//
// Must run with the agent sandbox disabled — Chromium children fail Mach rendezvous otherwise
// (same constraint as the B02 spike and the e2e test).
'use strict';

const net = require('net');
const path = require('path');
const fs = require('fs');
const os = require('os');
const { spawn, execSync } = require('child_process');

const T_COMMAND = 10;

// Accepts --name=value, --name value, and bare --name (-> true). The space form matters: a
// bare --name whose parseInt would otherwise become NaN silently disables a duration/interval.
function arg(name, dflt) {
  const i = process.argv.findIndex((a) => a === `--${name}` || a.startsWith(`--${name}=`));
  if (i === -1) return dflt;
  const a = process.argv[i];
  if (a.includes('=')) return a.slice(name.length + 3);
  const next = process.argv[i + 1];
  if (next !== undefined && !next.startsWith('--')) return next; // --name value
  return true; // bare flag, e.g. --json
}

const URL = arg('url', 'about:blank');
const W = parseInt(arg('width', '1280'), 10);
const H = parseInt(arg('height', '800'), 10);
const FPS = parseInt(arg('fps', '60'), 10);
const DURATION = parseInt(arg('duration', '8000'), 10);
const INTERVAL = 250;
const JSON_ONLY = !!arg('json', false);

const ENGINE_DIR = path.resolve(__dirname, '../apps/engine');
const ELECTRON = path.join(ENGINE_DIR, 'node_modules/.bin/electron');
const MAIN = path.join(ENGINE_DIR, 'src/main.js');

if (!fs.existsSync(ELECTRON)) {
  console.error(`electron not found at ${ELECTRON} — run 'npm ci' in apps/engine first`);
  process.exit(2);
}

// Resident set (KB) of a pid plus every descendant, from one ps snapshot. ucomm is used only
// to label; rss is what we sum. macOS rss is per-process and over-counts shared pages, so this
// is an upper bound on real memory — stated, not hidden.
function treeRssKb(rootPid) {
  let out;
  try {
    // Absolute path: a sandboxed/stripped PATH can leave a bare `ps` unresolvable, which would
    // silently zero every sample. /bin/ps is standard on macOS and Linux.
    out = execSync('/bin/ps -Ao pid=,ppid=,rss=', { encoding: 'utf8' });
  } catch {
    return null;
  }
  const children = new Map();
  const rss = new Map();
  for (const line of out.split('\n')) {
    const m = line.trim().match(/^(\d+)\s+(\d+)\s+(\d+)$/);
    if (!m) continue;
    const pid = +m[1];
    const ppid = +m[2];
    rss.set(pid, +m[3]);
    if (!children.has(ppid)) children.set(ppid, []);
    children.get(ppid).push(pid);
  }
  let total = 0;
  let count = 0;
  const stack = [rootPid];
  const seen = new Set();
  while (stack.length) {
    const pid = stack.pop();
    if (seen.has(pid)) continue;
    seen.add(pid);
    if (rss.has(pid)) {
      total += rss.get(pid);
      count++;
    }
    for (const c of children.get(pid) || []) stack.push(c);
  }
  return { kb: total, procs: count, pids: [...seen].filter((pid) => rss.has(pid)) };
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const mb = (kb) => +(kb / 1024).toFixed(1);

(async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'bg-rss-'));
  const sockPath = path.join(dir, 'e.sock');
  let child = null;
  let server = null;
  let engineSocket = null;
  let readyTimer = null;
  const observedPids = new Set();
  const events = [];

  async function shutdown() {
    if (readyTimer) clearTimeout(readyTimer);

    // `.bin/electron` is an npm launcher on macOS. Killing that PID can orphan the actual
    // Electron process while this server keeps its socket alive. Ask the engine to exit over
    // its real protocol first; closing the socket is a second, independently handled exit path.
    if (engineSocket && !engineSocket.destroyed) {
      const payload = Buffer.from('{"t":"quit"}', 'utf8');
      const message = Buffer.allocUnsafe(5 + payload.length);
      message.writeUInt8(T_COMMAND, 0);
      message.writeUInt32BE(payload.length, 1);
      payload.copy(message, 5);
      try { engineSocket.end(message); } catch {}
    }
    if (child && child.exitCode === null && child.signalCode === null) {
      const exited = new Promise((resolve) => child.once('exit', resolve));
      await Promise.race([exited, sleep(1500)]);
      if (child.exitCode === null && child.signalCode === null) {
        try { child.kill('SIGKILL'); } catch {}
      }
    }
    if (engineSocket && !engineSocket.destroyed) engineSocket.destroy();
    if (server) {
      await Promise.race([
        new Promise((resolve) => server.close(resolve)),
        sleep(1000),
      ]);
    }
    try { fs.rmSync(dir, { recursive: true, force: true }); } catch {}

    // The npm launcher can exit before Electron, so launcher status alone is not a cleanup
    // assertion. Every PID observed inside this probe's process tree must disappear. Kill only
    // those exact descendants if cleanup regresses, then fail the probe instead of leaking them.
    const alive = () => [...observedPids].filter((pid) => {
      try { process.kill(pid, 0); return true; } catch { return false; }
    });
    let leaked = alive();
    for (let attempt = 0; leaked.length && attempt < 20; attempt++) {
      await sleep(100);
      leaked = alive();
    }
    if (leaked.length) {
      for (const pid of leaked) {
        try { process.kill(pid, 'SIGKILL'); } catch {}
      }
      throw new Error(`engine cleanup leaked pid(s): ${leaked.join(', ')}`);
    }
  }

  try {
    const ready = new Promise((resolve, reject) => {
      server = net.createServer((sock) => {
        engineSocket = sock;
        let buf = Buffer.alloc(0);
        sock.on('data', (chunk) => {
          buf = buf.length ? Buffer.concat([buf, chunk]) : chunk;
          for (;;) {
            if (buf.length < 5) return;
            const len = buf.readUInt32BE(1);
            if (buf.length < 5 + len) return;
            const type = buf.readUInt8(0);
            const payload = buf.subarray(5, 5 + len);
            buf = buf.subarray(5 + len);
            if (type === 2) {
              const ev = JSON.parse(payload.toString('utf8'));
              events.push(ev);
              if (ev.t === 'ready') resolve();
            }
          }
        });
      });
      server.listen(sockPath);
      readyTimer = setTimeout(() => reject(new Error('engine did not connect in 30s')), 30000);
    });

    child = spawn(
      ELECTRON,
      [MAIN, `--tf-socket=${sockPath}`, `--tf-width=${W}`, `--tf-height=${H}`, `--tf-url=${URL}`, `--tf-fps=${FPS}`],
      { stdio: 'ignore' }
    );

    await ready;
    clearTimeout(readyTimer);
    readyTimer = null;

    const samples = [];
    const t0 = Date.now();
    while (Date.now() - t0 < DURATION) {
      const s = treeRssKb(child.pid);
      if (s) {
        samples.push(s);
        for (const pid of s.pids) observedPids.add(pid);
      }
      await sleep(INTERVAL);
    }

    if (!samples.length) {
      console.error('no RSS samples collected (ps unavailable?)');
      process.exitCode = 1;
      return;
    }
    const kbs = samples.map((s) => s.kb).sort((a, b) => a - b);
    const peak = kbs[kbs.length - 1];
    // Steady state = median of the second half, after Chromium finishes spinning up its helpers.
    const half = kbs.slice(Math.floor(kbs.length / 2));
    const steady = half[Math.floor(half.length / 2)];
    const procs = Math.max(...samples.map((s) => s.procs));

    const report = {
      url: URL,
      viewport: `${W}x${H}`,
      fps: FPS,
      samples: samples.length,
      peak_rss_mb: mb(peak),
      steady_rss_mb: mb(steady),
      max_processes: procs,
      electron: (events.find((e) => e.t === 'ready') || {}).electron || null,
      chrome: (events.find((e) => e.t === 'ready') || {}).chrome || null,
      note: 'macOS rss over-counts shared pages; read as an upper bound.',
    };

    if (JSON_ONLY) {
      console.log(JSON.stringify(report, null, 2));
    } else {
      console.log(`Terminal-Fenster engine RSS — ${URL} @ ${W}x${H}, ${FPS}fps`);
      console.log(`  Electron ${report.electron} / Chromium ${report.chrome}`);
      console.log(`  peak    ${report.peak_rss_mb} MB   (${procs} processes)`);
      console.log(`  steady  ${report.steady_rss_mb} MB   (median of ${half.length} samples)`);
      console.log(`  ${report.note}`);
    }
  } finally {
    await shutdown();
  }
})().catch((e) => {
  console.error('rss probe error:', e.message);
  process.exit(2);
});
