#!/usr/bin/env node
//
// Terminal-Fenster benchmark harness.
//
// Measures the real binary at target/release/terminal-fenster end to end:
//   * cold and warm start to first frame
//   * frames delivered and frames per second
//   * encode milliseconds and wire bytes per frame
//   * resident set size of the whole process tree (CLI + Electron + Chromium)
//
// Emits machine-readable JSON plus a readable summary.
//
// ---------------------------------------------------------------------------
// WHY IT IS SHAPED THIS WAY
//
// 1. It must run in a real terminal. `terminal-fenster open` calls isatty(stdin) and
//    refuses to run otherwise (apps/cli/src/main.rs:228). Capability detection
//    then *asks the terminal questions*: the query bytes are written to STDOUT
//    and the replies are read from STDIN (crates/tf-term/src/caps.rs:117-120).
//
// 2. Therefore stdout must NOT be redirected. If stdout goes to a pipe or a
//    file, the terminal never sees the queries, never replies, cell size stays
//    unknown, and cmd_open bails with "could not determine terminal pixel size"
//    (main.rs:244-251) *before* it writes a single log line. So capturing
//    stdout to count bytes is not an option, and this harness never tries.
//
// 3. That leaves $TERMINAL_FENSTER_LOG as the only safe measurement channel, which is
//    exactly why it exists: "Logging must never go to stdout while browsing:
//    stdout is the graphics channel" (main.rs:29-31). Every number below is
//    parsed from that file or sampled from the OS. Nothing is inferred from
//    screen contents, so this is CI-able and does not need a visible display.
//
// 4. stderr IS piped, because nothing reads it during capability detection and
//    capturing it makes failures diagnosable instead of mysterious.
//
// ---------------------------------------------------------------------------
// LOG LINES THIS PARSER DEPENDS ON  (emitted by apps/cli/src/main.rs)
//
//   <unix_ms> start url=… term=Some("Ghostty") backend=kitty kitty_gfx=true \
//             kitty_kbd=true pixel_mouse=true viewport=2482x851 \
//             cell=Some((17, 37)) page=2482x814                     (main.rs:257)
//   <unix_ms> first-frame after 366ms geometry=Some((2482, 814)) \
//             payload_bytes=8081424                                 (main.rs:539)
//   <unix_ms> event {"t":"title","v":"…"}                           (main.rs:549)
//   <unix_ms> bounded-run complete frames=459 fps=60 \
//             last_wire_bytes=53999 encode_ms=0.74 convert_ms=0.21
//   <unix_ms> frame-stats samples=459 encode_ms_p50=0.67 \
//             encode_ms_p99=0.91 wire_bytes_p50=53120 \
//             wire_bytes_p99=54788 gap_samples=458 gap_ms_p50=16.67 \
//             gap_ms_p99=19.94 convert_ms_p50=0.19 convert_ms_p99=0.28
//
// The timestamp prefix is Unix epoch milliseconds (main.rs:34-37), the same
// clock Node's Date.now() reads, so harness and binary timestamps are directly
// comparable and spawn-to-first-frame can be measured across the process
// boundary.
//
// Run `node bench.mjs --self-test` to verify the parser against fixtures.
// See README.md for how to run a real measurement.

import { spawn, execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { fileURLToPath } from 'node:url';

const execFileAsync = promisify(execFile);

const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO = path.resolve(HERE, '..');

const SCHEMA_VERSION = 2;

const DEFAULTS = {
  bin: path.join(REPO, 'target/release/terminal-fenster'),
  url: 'file://' + path.join(HERE, 'pages/repaint.html'),
  runs: 5,
  durationMs: 8000,
  // One `ps -Ao …ucomm=` call measured 36 ms idle and up to ~223 ms under load
  // on this machine, so 250 ms is honourable in the common case. Preflight
  // warns when ps is too slow for the requested interval, and the sampler
  // always reports the interval it achieved rather than the one requested.
  sampleMs: 250,
  settleMs: 2000,
  out: path.join(HERE, 'results'),
  backend: null, // null = let capability detection choose
  label: null,
  dryRun: false,
  selfTest: false,
  jsonOnly: false,
  keepLogs: true,
};

// ------------------------------------------------------------------ arguments

function parseArgs(argv) {
  const o = { ...DEFAULTS };
  const need = (i, name) => {
    if (i + 1 >= argv.length) fatal(`${name} requires a value`);
    return argv[i + 1];
  };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    switch (a) {
      case '--bin':         o.bin = path.resolve(need(i, a)); i++; break;
      case '--url':         o.url = need(i, a); i++; break;
      case '--runs':        o.runs = int(need(i, a), a); i++; break;
      case '--duration-ms': o.durationMs = int(need(i, a), a); i++; break;
      case '--sample-ms':   o.sampleMs = int(need(i, a), a); i++; break;
      case '--settle-ms':   o.settleMs = int(need(i, a), a); i++; break;
      case '--out':         o.out = path.resolve(need(i, a)); i++; break;
      case '--label':       o.label = need(i, a); i++; break;
      case '--backend':     o.backend = need(i, a); i++; break;
      case '--page': {
        const p = need(i, a); i++;
        o.url = 'file://' + path.join(HERE, 'pages', p.endsWith('.html') ? p : p + '.html');
        break;
      }
      case '--dry-run':     o.dryRun = true; break;
      case '--self-test':   o.selfTest = true; break;
      case '--json-only':   o.jsonOnly = true; break;
      case '--no-keep-logs': o.keepLogs = false; break;
      case '-h': case '--help': usage(); process.exit(0); break;
      default: fatal(`unknown option ${a}  (try --help)`);
    }
  }
  if (o.runs < 1) fatal('--runs must be >= 1');
  if (o.durationMs < 1000) fatal('--duration-ms must be >= 1000 (shorter runs are dominated by load time)');
  if (o.backend && !['kitty', 'unicode', 'sixel'].includes(o.backend)) {
    fatal(`--backend must be kitty | unicode | sixel, got ${o.backend}`);
  }
  return o;
}

function int(v, name) {
  const n = Number(v);
  if (!Number.isInteger(n)) fatal(`${name} expects an integer, got ${JSON.stringify(v)}`);
  return n;
}

function fatal(msg) {
  process.stderr.write(`bench: ${msg}\n`);
  process.exit(2);
}

function usage() {
  process.stdout.write(`
Terminal-Fenster benchmark harness

  node benchmarks/bench.mjs [options]

REQUIRES an interactive, graphics-capable terminal (Ghostty / kitty / WezTerm).
It cannot run under a pipe, under CI without a PTY, or inside an agent sandbox.

OPTIONS
  --runs N            number of runs; run 0 is the cold run   (default ${DEFAULTS.runs})
  --duration-ms MS    bounded run length per run              (default ${DEFAULTS.durationMs})
  --page NAME         page from benchmarks/pages             (default repaint)
  --url URL           explicit URL, overrides --page
  --backend B         force kitty | unicode | sixel           (default: auto-detect)
  --sample-ms MS      RSS sampling interval                   (default ${DEFAULTS.sampleMs})
                      one ps call costs 170-700 ms on a busy machine, so lower
                      values are requests the sampler cannot honour
  --settle-ms MS      pause between runs                      (default ${DEFAULTS.settleMs})
  --bin PATH          binary under test                       (default target/release/terminal-fenster)
  --out DIR           results directory                       (default benchmarks/results)
  --label NAME        tag added to the result filename
  --json-only         print JSON only, no human summary
  --no-keep-logs      delete per-run logs after parsing
  --dry-run           run preflight checks and print the plan, execute nothing
  --self-test         verify the log parser against fixtures, execute nothing
  -h, --help          this text

EXIT CODES
  0  all runs valid
  1  a run failed, or preflight failed
  2  bad usage
`);
}

// ------------------------------------------------------------------ log parser
//
// Kept as a pure function of (log text, spawn/exit wall clock) so it is
// testable without launching a browser. --self-test exercises it directly.

const RE = {
  start: /^(\d+)\s+start\s+(.*)$/,
  // geometry is Debug-printed from Option<(u32,u32)>, so it renders with DOUBLE
  // parens: `Some((2482, 814))`. The inner pair is optional-parenthesised here
  // only to survive a future switch to a flat tuple.
  firstFrame: /^(\d+)\s+first-frame after (\d+)ms geometry=(?:Some\(\(?(\d+),\s*(\d+)\)?\)|None) payload_bytes=(\d+)/,
  frameStats: /^(\d+)\s+frame-stats samples=(\d+) encode_ms_p50=([\d.]+) encode_ms_p99=([\d.]+) wire_bytes_p50=(\d+) wire_bytes_p99=(\d+) gap_samples=(\d+) gap_ms_p50=([\d.]+) gap_ms_p99=([\d.]+)(?: convert_ms_p50=([\d.]+) convert_ms_p99=([\d.]+))?/,
  complete: /^(\d+)\s+bounded-run complete frames=(\d+) fps=([\d.]+) last_wire_bytes=(\d+) encode_ms=([\d.]+)(?: convert_ms=([\d.]+))?/,
  event: /^(\d+)\s+event\s+(.*)$/,
};

function parseStartFields(rest) {
  const f = {};
  const grab = (re, fn) => { const m = rest.match(re); if (m) fn(m); };
  grab(/\burl=(\S+)/,            (m) => { f.url = m[1]; });
  grab(/\bterm=Some\("([^"]*)"\)/, (m) => { f.term_program = m[1]; });
  if (f.term_program === undefined && /\bterm=None\b/.test(rest)) f.term_program = null;
  grab(/\bbackend=(\w+)/,        (m) => { f.backend = m[1]; });
  grab(/\bkitty_gfx=(true|false)/,  (m) => { f.kitty_graphics = m[1] === 'true'; });
  grab(/\bkitty_shm=(true|false)/,  (m) => { f.kitty_shared_memory = m[1] === 'true'; });
  grab(/\bkitty_kbd=(true|false)/,  (m) => { f.kitty_keyboard = m[1] === 'true'; });
  grab(/\bpixel_mouse=(true|false)/,(m) => { f.pixel_mouse = m[1] === 'true'; });
  grab(/\bviewport=(\d+)x(\d+)/, (m) => { f.viewport_px = [+m[1], +m[2]]; });
  // Debug of Option<(u16,u16)> -> `Some((17, 37))`, double parens.
  grab(/\bcell=Some\(\(?(\d+),\s*(\d+)\)?\)/, (m) => { f.cell_px = [+m[1], +m[2]]; });
  grab(/\bpage=(\d+)x(\d+)/,     (m) => { f.page_px = [+m[1], +m[2]]; });
  return f;
}

/**
 * Turn one run's log into metrics. Returns { ok, errors, ... }.
 * Never invents a value: a metric that cannot be derived is null, and the run
 * is marked not-ok with a reason. Silent zeros would be worse than a failure.
 */
export function parseRunLog(text, { spawnedAtMs, exitedAtMs, exitCode, stderr }) {
  const errors = [];
  const lines = text.split('\n').filter((l) => l.trim().length > 0);

  let start = null, firstFrame = null, frameStats = null, complete = null;
  const events = [];
  for (const line of lines) {
    let m;
    if ((m = line.match(RE.start))) {
      start = { ts: +m[1], ...parseStartFields(m[2]) };
    } else if ((m = line.match(RE.firstFrame))) {
      firstFrame = {
        ts: +m[1],
        after_ms: +m[2],
        geometry: m[3] !== undefined ? [+m[3], +m[4]] : null,
        payload_bytes: +m[5],
      };
    } else if ((m = line.match(RE.frameStats))) {
      frameStats = {
        ts: +m[1],
        samples: +m[2],
        encode_ms_p50: +m[3],
        encode_ms_p99: +m[4],
        wire_bytes_p50: +m[5],
        wire_bytes_p99: +m[6],
        gap_samples: +m[7],
        gap_ms_p50: +m[8],
        gap_ms_p99: +m[9],
        convert_ms_p50: m[10] === undefined ? null : +m[10],
        convert_ms_p99: m[11] === undefined ? null : +m[11],
      };
    } else if ((m = line.match(RE.complete))) {
      complete = {
        ts: +m[1],
        frames: +m[2],
        fps_logged: +m[3],
        last_wire_bytes: +m[4],
        last_encode_ms: +m[5],
        last_convert_ms: m[6] === undefined ? null : +m[6],
      };
    } else if ((m = line.match(RE.event))) {
      events.push({ ts: +m[1], json: m[2] });
    }
  }

  if (lines.length === 0) {
    errors.push(
      'log is empty: terminal-fenster exited before writing any line. The usual cause is ' +
      'that the terminal did not answer the capability queries, so cmd_open bailed at ' +
      '"could not determine terminal pixel size" (main.rs:244-251). Run inside Ghostty.'
    );
  }
  if (!start) errors.push('missing "start" line');
  if (!firstFrame) errors.push('missing "first-frame" line: no frame ever reached the terminal');
  if (!complete) errors.push('missing "bounded-run complete" line: run did not finish its bounded window');
  if (exitCode !== 0) errors.push(`terminal-fenster exited ${exitCode}`);

  const m = {
    ok: errors.length === 0,
    errors,
    exit_code: exitCode,
    stderr: (stderr || '').trim() || null,

    terminal: start ? {
      term_program: start.term_program ?? null,
      backend: start.backend ?? null,
      kitty_graphics: start.kitty_graphics ?? null,
      kitty_shared_memory: start.kitty_shared_memory ?? null,
      kitty_keyboard: start.kitty_keyboard ?? null,
      pixel_mouse: start.pixel_mouse ?? null,
      viewport_px: start.viewport_px ?? null,
      cell_px: start.cell_px ?? null,
      page_px: start.page_px ?? null,
    } : null,
    url: start?.url ?? null,

    startup: {
      // Wall clock from spawn() to the "start" line: TtyGuard acquisition plus
      // the capability handshake (300 ms deadline, main.rs:241).
      tty_and_detect_ms: null,
      // "start" line to the run loop beginning: Electron spawn plus the Unix
      // socket accept. This is the "engine ready" number.
      engine_spawn_connect_ms: null,
      // Run loop start to first frame drawn, as the binary measured it.
      first_frame_after_run_start_ms: firstFrame?.after_ms ?? null,
      // The headline: process spawn to a frame on screen.
      spawn_to_first_frame_ms: null,
      spawn_to_exit_ms: exitedAtMs - spawnedAtMs,
    },

    frames: {
      // Compatibility alias: engine frames accepted by the core.
      count: complete?.frames ?? null,
      received_count: complete?.frames ?? null,
      presented_count: frameStats?.samples ?? complete?.frames ?? null,
      // Instantaneous value the binary reports: frames seen in the trailing
      // second (main.rs:842-844). A sample, not an average.
      fps_logged_instantaneous: complete?.fps_logged ?? null,
      // Completed terminal presentations over the bounded window. A legacy log
      // without frame-stats falls back to received frames and says so in bytes.
      fps_over_window: null,
      // Completed presentations after the first, from first frame to exit.
      fps_steady_state: null,
      // Diagnostic producer/consumer split: these count socket frames before
      // the core coalesces multiple messages into one terminal presentation.
      fps_received_over_window: null,
      fps_received_steady_state: null,
      run_window_ms: null,
      steady_window_ms: null,
    },

    bytes: {
      // The final-frame values remain for compatibility and quick diagnosis.
      // Bounded runs additionally report a distribution over every completed
      // terminal presentation; normal interactive runs retain no sample history.
      last_encode_ms: complete?.last_encode_ms ?? null,
      last_convert_ms: complete?.last_convert_ms ?? null,
      last_wire_bytes: complete?.last_wire_bytes ?? null,
      frame_samples: frameStats?.samples ?? null,
      encode_ms_p50: frameStats?.encode_ms_p50 ?? null,
      encode_ms_p99: frameStats?.encode_ms_p99 ?? null,
      convert_ms_p50: frameStats?.convert_ms_p50 ?? null,
      convert_ms_p99: frameStats?.convert_ms_p99 ?? null,
      wire_bytes_p50: frameStats?.wire_bytes_p50 ?? null,
      wire_bytes_p99: frameStats?.wire_bytes_p99 ?? null,
      present_gap_samples: frameStats?.gap_samples ?? null,
      present_gap_ms_p50: frameStats?.gap_ms_p50 ?? null,
      present_gap_ms_p99: frameStats?.gap_ms_p99 ?? null,
      first_frame_payload_bytes: firstFrame?.payload_bytes ?? null,
      raw_pixel_bytes: null,
      compression_ratio_approx: null,
      samples_are_last_frame_only: frameStats === null,
    },

    events: events.length,
    event_lines: events.slice(0, 50),
  };

  if (start) m.startup.tty_and_detect_ms = start.ts - spawnedAtMs;

  if (start && firstFrame) {
    const runStartTs = firstFrame.ts - firstFrame.after_ms;
    m.startup.engine_spawn_connect_ms = runStartTs - start.ts;
    m.startup.spawn_to_first_frame_ms = firstFrame.ts - spawnedAtMs;
    m._runStartTs = runStartTs;
  }

  if (firstFrame && complete && m._runStartTs != null) {
    const win = complete.ts - m._runStartTs;
    const steady = win - firstFrame.after_ms;
    const received = complete.frames;
    const presented = frameStats?.samples ?? received;
    m.frames.run_window_ms = win;
    m.frames.steady_window_ms = steady;
    if (win > 0) {
      m.frames.fps_over_window = round2((presented * 1000) / win);
      m.frames.fps_received_over_window = round2((received * 1000) / win);
    }
    if (steady > 0) {
      if (presented > 1) {
        m.frames.fps_steady_state = round2(((presented - 1) * 1000) / steady);
      }
      if (received > 1) {
        m.frames.fps_received_steady_state = round2(((received - 1) * 1000) / steady);
      }
    }
  }

  if (firstFrame?.geometry) {
    const [w, h] = firstFrame.geometry;
    const raw = w * h * 4;
    m.bytes.raw_pixel_bytes = raw;
    // The payload carries a 32-byte header (tf-proto FRAME_HEADER_LEN). If this
    // does not hold, the wire format changed and the ratio below is nonsense.
    if (firstFrame.payload_bytes !== raw + 32) {
      errors.push(
        `frame payload ${firstFrame.payload_bytes} != w*h*4+32 (${raw + 32}); ` +
        'the frame wire format changed and this parser is out of date'
      );
      m.ok = false;
    }
    const representativeWireBytes = frameStats?.wire_bytes_p50 || complete?.last_wire_bytes;
    if (representativeWireBytes > 0) {
      // Approximate on purpose: the numerator is the FIRST frame's raw size and
      // the denominator is the median presented frame's encoded size (or the
      // legacy final-frame sample). Geometry/damage can differ across the run.
      m.bytes.compression_ratio_approx = round2(raw / representativeWireBytes);
    }
  }

  delete m._runStartTs;
  return m;
}

const round2 = (n) => Math.round(n * 100) / 100;

// ------------------------------------------------------------- RSS sampling
//
// RSS is read from ps because that is the only figure available for a process
// tree we do not control. Caveats, all recorded in the JSON rather than only
// in prose:
//   * Chromium's helpers share a lot of pages with each other, so the SUM of
//     per-process RSS double counts. Treat tree_total as an upper bound.
//   * ps is sampled, so a spike shorter than the achieved interval is invisible.
//   * ps is SLOW. Measured on this machine (~700 processes, macOS 26.1):
//       ps -Ao pid=,ppid=,rss=,comm=   450-1150 ms   (resolves full paths)
//       ps -Ao pid=,ppid=,rss=,ucomm=  167-223  ms
//       ps -o  pid=,rss= -p <40 pids>  317-657  ms
//     Targeting specific PIDs is no cheaper, so the cost is ps startup, not the
//     number of processes inspected. The fast loop therefore uses `ucomm`, and
//     the sampler reports the interval it ACHIEVED, never merely the one it was
//     asked for. `ucomm` truncates to 16 characters, so one full-path snapshot
//     is taken per run purely to label the breakdown.

async function psSnapshot(useFullComm = false) {
  const field = useFullComm ? 'comm=' : 'ucomm=';
  const { stdout } = await execFileAsync('ps', ['-Ao', `pid=,ppid=,rss=,${field}`], {
    maxBuffer: 8 * 1024 * 1024,
  });
  const procs = new Map();
  const kids = new Map();
  for (const line of stdout.split('\n')) {
    const m = line.match(/^\s*(\d+)\s+(\d+)\s+(\d+)\s+(.*)$/);
    if (!m) continue;
    const pid = +m[1], ppid = +m[2], rss_kb = +m[3], comm = m[4].trim();
    procs.set(pid, { pid, ppid, rss_kb, comm });
    if (!kids.has(ppid)) kids.set(ppid, []);
    kids.get(ppid).push(pid);
  }
  return { procs, kids };
}

function descendantsOf(rootPid, kids) {
  const out = [];
  const stack = [rootPid];
  const seen = new Set();
  while (stack.length) {
    const p = stack.pop();
    if (seen.has(p)) continue;
    seen.add(p);
    out.push(p);
    for (const c of kids.get(p) || []) stack.push(c);
  }
  return out;
}

const shortComm = (c) => {
  const base = c.split('/').pop() || c;
  return base.length > 40 ? base.slice(0, 39) + '…' : base;
};

class RssSampler {
  constructor(rootPid, intervalMs) {
    this.rootPid = rootPid;
    this.intervalMs = intervalMs;
    this.samples = [];
    this.pidsSeen = new Set([rootPid]);
    this.psCosts = [];
    this.labels = new Map(); // pid -> full executable path, filled once per run
    this.stopped = false;
    this.failures = 0;
  }
  start() { this.loop = this._tick(); }

  async _tick() {
    while (!this.stopped) {
      const t0 = Date.now();
      try {
        const { procs, kids } = await psSnapshot(false);
        const pids = descendantsOf(this.rootPid, kids).filter((p) => procs.has(p));
        if (pids.length > 0) {
          let total = 0;
          const detail = [];
          for (const p of pids) {
            const rec = procs.get(p);
            total += rec.rss_kb;
            detail.push({ pid: rec.pid, rss_kb: rec.rss_kb, comm: rec.comm });
            this.pidsSeen.add(p);
          }
          this.samples.push({
            t_ms: t0,
            n_procs: pids.length,
            tree_total_kb: total,
            main_kb: procs.get(this.rootPid)?.rss_kb ?? null,
            procs: detail,
          });
          // One full-path snapshot per run, once the tree has actually forked,
          // so the breakdown is not stuck with 16-character ucomm stubs.
          if (pids.length > 1 && this.labels.size === 0) this._label();
        }
      } catch {
        this.failures++;
      }
      const spent = Date.now() - t0;
      this.psCosts.push(spent);
      await sleep(Math.max(0, this.intervalMs - spent));
    }
  }

  // Fire-and-forget: a slow labelling call must not stall the RSS time series.
  _label() {
    this.labels.set(0, 'pending');
    psSnapshot(true)
      .then(({ procs }) => {
        this.labels.clear();
        for (const [pid, rec] of procs) this.labels.set(pid, rec.comm);
      })
      .catch(() => { this.labels.clear(); });
  }

  async stop() { this.stopped = true; try { await this.loop; } catch { /* ignore */ } }

  summary() {
    const achieved = [];
    for (let i = 1; i < this.samples.length; i++) {
      achieved.push(this.samples[i].t_ms - this.samples[i - 1].t_ms);
    }
    const base = {
      samples: this.samples.length,
      ps_failures: this.failures,
      requested_interval_ms: this.intervalMs,
      // What the sampler actually managed. ps is slow enough on a busy machine
      // that this routinely exceeds the requested interval; reporting only the
      // request would overstate the resolution of the peak below.
      achieved_interval_ms_p50: achieved.length ? Math.round(percentile(achieved, 50)) : null,
      achieved_interval_ms_max: achieved.length ? Math.max(...achieved) : null,
      ps_cost_ms_p50: this.psCosts.length ? Math.round(percentile(this.psCosts, 50)) : null,
    };
    if (this.samples.length === 0) {
      return {
        ...base,
        note: 'no sample landed while the process tree was alive. The run was shorter than ' +
              'one ps call (see ps_cost_ms_p50); peak RSS is unknown, not zero.',
        peak_tree_total_kb: null, peak_tree_total_mb: null,
        peak_n_procs: null, peak_main_process_kb: null, peak_breakdown: null,
      };
    }
    let peak = this.samples[0];
    for (const s of this.samples) if (s.tree_total_kb > peak.tree_total_kb) peak = s;
    const totals = this.samples.map((s) => s.tree_total_kb);
    return {
      ...base,
      peak_tree_total_kb: peak.tree_total_kb,
      peak_tree_total_mb: round2(peak.tree_total_kb / 1024),
      peak_n_procs: peak.n_procs,
      peak_main_process_kb: peak.main_kb,
      median_tree_total_kb: Math.round(percentile(totals, 50)),
      peak_breakdown: peak.procs
        .map((p) => ({
          ...p,
          comm: this.labels.get(p.pid) ? shortComm(this.labels.get(p.pid)) : p.comm,
        }))
        .sort((a, b) => b.rss_kb - a.rss_kb),
      caveat:
        'tree_total is the SUM of per-process RSS. Chromium helpers share pages, ' +
        'so this over-counts and should be read as an upper bound. Peak resolution ' +
        'is limited by achieved_interval_ms_p50.',
    };
  }
}

// ----------------------------------------------------------------- utilities

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function percentile(values, p) {
  if (values.length === 0) return null;
  const s = [...values].sort((a, b) => a - b);
  if (s.length === 1) return s[0];
  const rank = Math.ceil((p / 100) * s.length);        // nearest-rank
  return s[Math.min(Math.max(rank, 1), s.length) - 1];
}

function stats(values) {
  const v = values.filter((x) => typeof x === 'number' && Number.isFinite(x));
  if (v.length === 0) return null;
  const mean = v.reduce((a, b) => a + b, 0) / v.length;
  const sd = v.length > 1
    ? Math.sqrt(v.reduce((a, b) => a + (b - mean) ** 2, 0) / (v.length - 1))
    : 0;
  return {
    n: v.length,
    min: round2(Math.min(...v)),
    p50: round2(percentile(v, 50)),
    max: round2(Math.max(...v)),
    mean: round2(mean),
    stddev: round2(sd),
  };
}

async function sha256(file) {
  return new Promise((resolve, reject) => {
    const h = createHash('sha256');
    createReadStream(file).on('error', reject).on('data', (d) => h.update(d)).on('end', () => resolve(h.digest('hex')));
  });
}

async function tryExec(cmd, args) {
  try { return (await execFileAsync(cmd, args)).stdout.trim(); } catch { return null; }
}

async function hostInfo() {
  const info = {
    platform: process.platform,
    arch: process.arch,
    release: os.release(),
    cpus: os.cpus().length,
    total_mem_bytes: os.totalmem(),
    node: process.version,
    hostname_hashed: createHash('sha256').update(os.hostname()).digest('hex').slice(0, 12),
  };
  if (process.platform === 'darwin') {
    info.os_version = await tryExec('sw_vers', ['-productVersion']);
    info.os_build = await tryExec('sw_vers', ['-buildVersion']);
    info.cpu_brand = await tryExec('sysctl', ['-n', 'machdep.cpu.brand_string']);
    info.model = await tryExec('sysctl', ['-n', 'hw.model']);
  }
  return info;
}

// ---------------------------------------------------------------- preflight

async function preflight(opt) {
  const checks = [];
  const add = (name, ok, detail, fatalIfFalse = true) =>
    checks.push({ name, ok, detail, fatal: fatalIfFalse && !ok });

  // Binary present and executable.
  let binStat = null;
  try {
    binStat = await fs.stat(opt.bin);
    await fs.access(opt.bin, fs.constants.X_OK);
    add('binary executable', true, opt.bin);
  } catch (e) {
    add('binary executable', false, `${opt.bin}: ${e.code || e.message}. Build it with: cargo build --release`);
  }

  // Version, via a path that does not need a tty.
  let version = null;
  if (binStat) {
    try {
      const { stdout } = await execFileAsync(opt.bin, ['version']);
      version = stdout.trim();
      add('binary runs', true, version);
    } catch (e) {
      add('binary runs', false, `${opt.bin} version failed: ${e.message}`);
    }
  }

  // The hard requirement.
  const isTty = Boolean(process.stdin.isTTY && process.stdout.isTTY);
  add('stdin and stdout are a tty', isTty,
    isTty ? 'ok'
          : 'terminal-fenster open calls isatty(stdin) and refuses otherwise (main.rs:228), and ' +
            'capability queries are written to stdout and answered on stdin (caps.rs:117-120). ' +
            'Run this from an interactive terminal window; do not pipe or redirect it.');

  // Graphics capability. Not fatal (the unicode backend is a real code path),
  // but it changes what the numbers mean, so it is recorded either way.
  const termProgram = process.env.TERM_PROGRAM || '';
  const graphicsLikely = /ghostty|kitty|wezterm/i.test(termProgram) ||
                         Boolean(process.env.KITTY_WINDOW_ID) ||
                         opt.backend === 'kitty';
  add('graphics-capable terminal', graphicsLikely,
    graphicsLikely
      ? `TERM_PROGRAM=${termProgram || '(unset)'}`
      : `TERM_PROGRAM=${termProgram || '(unset)'} — no kitty graphics expected. The run will ` +
        'use the Unicode half-block fallback, so encode ms and wire bytes measure a ' +
        'DIFFERENT encoder and are not comparable to kitty results.',
    false);

  // Multiplexers rewrite or swallow graphics escapes.
  const inMux = Boolean(process.env.TMUX) || /^screen/.test(process.env.TERM || '');
  add('not inside tmux/screen', !inMux,
    inMux ? 'graphics passthrough through a multiplexer changes what is measured; run in a bare terminal'
          : 'ok', false);

  // Engine present, mirroring locate_engine()'s dev layout (main.rs:326-334).
  const enginePath = path.join(REPO, 'apps/engine/node_modules/.bin/electron');
  let engineOk = false;
  try { await fs.access(enginePath); engineOk = true; } catch { /* not there */ }
  add('electron engine present', engineOk,
    engineOk ? enginePath
             : `${enginePath} not found. Run: (cd apps/engine && npm install), or set TERMINAL_FENSTER_ENGINE.`);

  // The URL, when it is a local file.
  if (opt.url.startsWith('file://')) {
    const p = opt.url.slice('file://'.length);
    let ok = false;
    try { await fs.access(p); ok = true; } catch { /* missing */ }
    add('page exists', ok, ok ? p : `${p} not found`);
  } else {
    add('page', true, `${opt.url} (remote URL: results depend on the network)`, false);
  }

  // ps must work, or there is no RSS. Its cost is measured, not assumed: it
  // sets the ceiling on RSS sampling resolution and varies hugely with load.
  let psCostMs = null;
  try {
    const t0 = Date.now();
    const { procs } = await psSnapshot();
    psCostMs = Date.now() - t0;
    add('ps available', procs.size > 0, `${procs.size} processes visible, ${psCostMs} ms per call`);
    add('ps fast enough for --sample-ms', psCostMs <= opt.sampleMs,
      psCostMs <= opt.sampleMs
        ? `${psCostMs} ms <= ${opt.sampleMs} ms`
        : `one ps call costs ${psCostMs} ms but --sample-ms is ${opt.sampleMs}. RSS will be ` +
          `sampled slower than requested; the JSON reports the achieved interval. Raise ` +
          `--sample-ms to at least ${Math.ceil(psCostMs / 100) * 100} to stop over-requesting.`,
      false);
  } catch (e) {
    add('ps available', false, `ps failed: ${e.message}`);
  }

  return { checks, version, binStat, enginePath, graphicsLikely, psCostMs };
}

// --------------------------------------------------------------- single run

export async function runOnce(opt, index, logPath) {
  const env = {
    ...process.env,
    TERMINAL_FENSTER_LOG: logPath,
    TERMINAL_FENSTER_EXIT_AFTER_MS: String(opt.durationMs),
  };
  if (opt.backend) env.TERMINAL_FENSTER_BACKEND = opt.backend;

  // Start from a clean log so parsing is unambiguous: log_line appends
  // (main.rs:38), it does not truncate.
  await fs.rm(logPath, { force: true });

  const spawnedAtMs = Date.now();
  // stdin+stdout inherited: the capability handshake needs the real terminal.
  // stderr piped: nothing reads it, and capturing it makes failures legible.
  const child = spawn(opt.bin, ['open', opt.url], { stdio: ['inherit', 'inherit', 'pipe'], env });

  let stderr = '';
  child.stderr.setEncoding('utf8');
  child.stderr.on('data', (d) => { stderr += d; });

  const sampler = new RssSampler(child.pid, opt.sampleMs);
  sampler.start();

  // Generous ceiling: bounded window + Electron cold start + the 1.5 s
  // shutdown grace (main.rs:662), plus slack.
  const hardTimeoutMs = opt.durationMs + 60_000;
  let timedOut = false;

  const exitCode = await new Promise((resolve) => {
    const t = setTimeout(() => {
      timedOut = true;
      child.kill('SIGKILL');
    }, hardTimeoutMs);
    child.on('error', (e) => { clearTimeout(t); stderr += `\nspawn error: ${e.message}`; resolve(-1); });
    child.on('exit', (code, signal) => {
      clearTimeout(t);
      resolve(code === null ? (signal ? -2 : -1) : code);
    });
  });
  const exitedAtMs = Date.now();

  await sampler.stop();

  // Leaked-process check. After exit the children are reparented, so ppid no
  // longer identifies them; instead test whether any PID we saw is still alive.
  //
  // This MUST be a drain-with-retry, not a single probe. shutdown() gives the
  // engine a 1.5 s grace period before SIGKILL (main.rs:662-675) and Chromium's
  // helpers then take their own time to unwind, so a single check a few hundred
  // milliseconds after exit reports phantom leaks on every healthy run.
  const drainDeadline = Date.now() + 6000;
  let leaked = [];
  let leakDrainMs = 0;
  while (true) {
    await sleep(400);
    try {
      const { procs } = await psSnapshot();
      leaked = [...sampler.pidsSeen]
        .filter((p) => p !== child.pid && procs.has(p))
        .map((p) => ({ pid: p, comm: shortComm(procs.get(p).comm), rss_kb: procs.get(p).rss_kb }));
    } catch {
      leaked = [];
    }
    leakDrainMs = Date.now() - exitedAtMs;
    if (leaked.length === 0 || Date.now() > drainDeadline) break;
  }

  let logText = '';
  try { logText = await fs.readFile(logPath, 'utf8'); } catch { /* stays empty */ }

  const parsed = parseRunLog(logText, { spawnedAtMs, exitedAtMs, exitCode, stderr });
  if (timedOut) {
    parsed.ok = false;
    parsed.errors.push(`run exceeded ${hardTimeoutMs} ms and was SIGKILLed`);
  }
  // A surviving helper is a real defect worth surfacing, but it does not make
  // the timing or memory numbers wrong. Recording it as a failure would throw
  // away a perfectly good measurement, so it is a warning.
  const warnings = [];
  if (leaked.length > 0) {
    warnings.push(
      `${leaked.length} process(es) still alive ${leakDrainMs} ms after exit: ` +
      leaked.map((l) => `${l.comm}(${l.pid})`).join(', ')
    );
  }

  return {
    index,
    kind: index === 0 ? 'cold' : 'warm',
    pid: child.pid,
    spawned_at_ms: spawnedAtMs,
    exited_at_ms: exitedAtMs,
    timed_out: timedOut,
    warnings,
    leaked_processes: leaked,
    leak_drain_ms: leakDrainMs,
    log_path: opt.keepLogs ? logPath : null,
    rss: sampler.summary(),
    ...parsed,
  };
}

// ------------------------------------------------------------------ summary

export function summarize(runs) {
  const ok = runs.filter((r) => r.ok);
  const cold = ok.find((r) => r.kind === 'cold') || null;
  const warm = ok.filter((r) => r.kind === 'warm');
  const pick = (rs, fn) => rs.map(fn);

  return {
    runs_total: runs.length,
    runs_ok: ok.length,
    runs_failed: runs.length - ok.length,
    cold_start: cold ? {
      spawn_to_first_frame_ms: cold.startup.spawn_to_first_frame_ms,
      tty_and_detect_ms: cold.startup.tty_and_detect_ms,
      engine_spawn_connect_ms: cold.startup.engine_spawn_connect_ms,
      first_frame_after_run_start_ms: cold.startup.first_frame_after_run_start_ms,
    } : null,
    warm_start: warm.length ? {
      spawn_to_first_frame_ms: stats(pick(warm, (r) => r.startup.spawn_to_first_frame_ms)),
      tty_and_detect_ms: stats(pick(warm, (r) => r.startup.tty_and_detect_ms)),
      engine_spawn_connect_ms: stats(pick(warm, (r) => r.startup.engine_spawn_connect_ms)),
      first_frame_after_run_start_ms: stats(pick(warm, (r) => r.startup.first_frame_after_run_start_ms)),
    } : null,
    throughput: ok.length ? {
      fps_steady_state: stats(pick(ok, (r) => r.frames.fps_steady_state)),
      fps_over_window: stats(pick(ok, (r) => r.frames.fps_over_window)),
      fps_received_steady_state: stats(pick(ok, (r) => r.frames.fps_received_steady_state)),
      received_frames: stats(pick(ok, (r) => r.frames.received_count)),
      presented_frames: stats(pick(ok, (r) => r.frames.presented_count)),
    } : null,
    encode: ok.length ? {
      frame_samples: stats(pick(ok, (r) => r.bytes.frame_samples)),
      encode_ms_p50: stats(pick(ok, (r) => r.bytes.encode_ms_p50)),
      encode_ms_p99: stats(pick(ok, (r) => r.bytes.encode_ms_p99)),
      convert_ms_p50: stats(pick(ok, (r) => r.bytes.convert_ms_p50)),
      convert_ms_p99: stats(pick(ok, (r) => r.bytes.convert_ms_p99)),
      wire_bytes_p50: stats(pick(ok, (r) => r.bytes.wire_bytes_p50)),
      wire_bytes_p99: stats(pick(ok, (r) => r.bytes.wire_bytes_p99)),
      present_gap_ms_p50: stats(pick(ok, (r) => r.bytes.present_gap_ms_p50)),
      present_gap_ms_p99: stats(pick(ok, (r) => r.bytes.present_gap_ms_p99)),
      last_encode_ms: stats(pick(ok, (r) => r.bytes.last_encode_ms)),
      last_convert_ms: stats(pick(ok, (r) => r.bytes.last_convert_ms)),
      last_wire_bytes: stats(pick(ok, (r) => r.bytes.last_wire_bytes)),
      compression_ratio_approx: stats(pick(ok, (r) => r.bytes.compression_ratio_approx)),
      note:
        'p50/p99 values are computed by the CLI over completed terminal presentations ' +
        'during each bounded run, then summarized across runs here.',
    } : null,
    memory: ok.length ? {
      peak_tree_total_mb: stats(pick(ok, (r) => r.rss.peak_tree_total_mb)),
      peak_n_procs: stats(pick(ok, (r) => r.rss.peak_n_procs)),
      peak_main_process_kb: stats(pick(ok, (r) => r.rss.peak_main_process_kb)),
    } : null,
  };
}

function renderText(report) {
  const L = [];
  const s = report.summary;
  const pad = (v, w) => String(v ?? '-').padStart(w);
  const fmt = (st, unit = 'ms') =>
    st ? `${pad(st.p50, 8)} ${unit}   (min ${st.min}, max ${st.max}, sd ${st.stddev}, n=${st.n})`
       : '        -';

  L.push('');
  L.push('Terminal-Fenster benchmark');
  L.push('='.repeat(72));
  L.push(`binary      ${report.binary.path}`);
  L.push(`version     ${report.binary.version ?? '?'}   sha256 ${(report.binary.sha256 || '').slice(0, 16)}`);
  L.push(`host        ${report.host.cpu_brand || report.host.arch} / ${report.host.platform} ${report.host.os_version || report.host.release}`);
  const t = report.runs.find((r) => r.terminal)?.terminal;
  L.push(`terminal    ${t?.term_program ?? '?'}   backend=${t?.backend ?? '?'}   ` +
         `page=${t?.page_px ? t.page_px.join('x') : '?'}  cell=${t?.cell_px ? t.cell_px.join('x') : '?'}`);
  L.push(`page        ${report.config.url}`);
  L.push(`config      runs=${report.config.runs}  duration=${report.config.duration_ms}ms  ` +
         `rss_sample=${report.config.sample_ms}ms`);
  L.push(`runs        ${s.runs_ok} ok, ${s.runs_failed} failed`);
  L.push('');

  L.push('START TO FIRST FRAME');
  L.push('-'.repeat(72));
  if (s.cold_start) {
    const c = s.cold_start;
    L.push(`  cold (run 0)                 ${pad(c.spawn_to_first_frame_ms, 8)} ms  spawn -> first frame`);
    L.push(`    tty + capability detect    ${pad(c.tty_and_detect_ms, 8)} ms`);
    L.push(`    engine spawn + connect     ${pad(c.engine_spawn_connect_ms, 8)} ms`);
    L.push(`    engine ready -> 1st frame  ${pad(c.first_frame_after_run_start_ms, 8)} ms`);
  } else L.push('  cold                         - (no valid cold run)');
  if (s.warm_start) {
    L.push(`  warm  spawn -> first frame   ${fmt(s.warm_start.spawn_to_first_frame_ms)}`);
    L.push(`        tty + capability       ${fmt(s.warm_start.tty_and_detect_ms)}`);
    L.push(`        engine spawn + connect ${fmt(s.warm_start.engine_spawn_connect_ms)}`);
    L.push(`        ready -> 1st frame     ${fmt(s.warm_start.first_frame_after_run_start_ms)}`);
  } else L.push('  warm                         - (no valid warm run)');
  L.push('');

  L.push('THROUGHPUT');
  L.push('-'.repeat(72));
  if (s.throughput) {
    L.push(`  presented fps, steady state  ${fmt(s.throughput.fps_steady_state, 'fps')}`);
    L.push(`  presented fps, whole window  ${fmt(s.throughput.fps_over_window, 'fps')}`);
    L.push(`  received fps, steady state   ${fmt(s.throughput.fps_received_steady_state, 'fps')}`);
    L.push(`  presentations per run        ${fmt(s.throughput.presented_frames, '   ')}`);
    L.push(`  engine frames per run        ${fmt(s.throughput.received_frames, '   ')}`);
  } else L.push('  - (no valid run)');
  L.push('');

  L.push('PRESENTATION DISTRIBUTION  (per-frame percentiles, summarized across runs)');
  L.push('-'.repeat(72));
  if (s.encode) {
    if (s.encode.encode_ms_p50) {
      L.push(`  BGRA conversion p50 / frame  ${fmt(s.encode.convert_ms_p50)}`);
      L.push(`  BGRA conversion p99 / frame  ${fmt(s.encode.convert_ms_p99)}`);
      L.push(`  encode p50 / frame           ${fmt(s.encode.encode_ms_p50)}`);
      L.push(`  encode p99 / frame           ${fmt(s.encode.encode_ms_p99)}`);
      L.push(`  wire bytes p50 / frame       ${fmt(s.encode.wire_bytes_p50, 'B ')}`);
      L.push(`  wire bytes p99 / frame       ${fmt(s.encode.wire_bytes_p99, 'B ')}`);
      L.push(`  presentation gap p50         ${fmt(s.encode.present_gap_ms_p50)}`);
      L.push(`  presentation gap p99         ${fmt(s.encode.present_gap_ms_p99)}`);
      L.push(`  presented frames sampled     ${fmt(s.encode.frame_samples, '   ')}`);
    } else {
      L.push(`  conversion (legacy final)    ${fmt(s.encode.last_convert_ms)}`);
      L.push(`  encode (legacy final frame)  ${fmt(s.encode.last_encode_ms)}`);
      L.push(`  wire (legacy final frame)    ${fmt(s.encode.last_wire_bytes, 'B ')}`);
    }
    L.push(`  raw -> wire reduction        ${fmt(s.encode.compression_ratio_approx, 'x ')}`);
  } else L.push('  - (no valid run)');
  L.push('');

  L.push('MEMORY  (sum of RSS across the process tree; shared pages double counted)');
  L.push('-'.repeat(72));
  if (s.memory) {
    L.push(`  peak tree RSS                ${fmt(s.memory.peak_tree_total_mb, 'MB')}`);
    L.push(`  processes at peak            ${fmt(s.memory.peak_n_procs, '   ')}`);
    L.push(`  peak RSS, terminal-fenster itself  ${fmt(s.memory.peak_main_process_kb, 'KB')}`);
    const bd = report.runs.find((r) => r.ok && r.rss.peak_breakdown)?.rss.peak_breakdown;
    if (bd) {
      L.push('  breakdown at peak (first ok run):');
      for (const p of bd.slice(0, 8)) {
        L.push(`    ${pad(p.rss_kb, 9)} KB  ${p.comm}`);
      }
    }
  } else L.push('  - (no valid run)');
  L.push('');

  const warned = report.runs.filter((r) => r.warnings?.length);
  if (warned.length) {
    L.push('WARNINGS');
    L.push('-'.repeat(72));
    for (const w of warned) for (const msg of w.warnings) L.push(`  run ${w.index}: ${msg}`);
    L.push('');
  }

  const failed = report.runs.filter((r) => !r.ok);
  if (failed.length) {
    L.push('FAILURES');
    L.push('-'.repeat(72));
    for (const f of failed) {
      L.push(`  run ${f.index} (${f.kind}) exit=${f.exit_code}`);
      for (const e of f.errors) L.push(`    - ${e}`);
      if (f.stderr) L.push(`    stderr: ${f.stderr.split('\n')[0]}`);
    }
    L.push('');
  }

  L.push(`JSON  ${report._json_path}`);
  L.push('');
  return L.join('\n');
}

// ---------------------------------------------------------------- self-test
//
// Verifies the parser and every derived metric against synthetic fixtures.
// This is NOT a benchmark result: no browser runs, no timing is measured.

const FIXTURE_OK = [
  '1000000000320 start url=file:///x/repaint.html term=Some("ghostty") backend=kitty kitty_gfx=true kitty_shm=true kitty_kbd=true pixel_mouse=true viewport=2482x851 cell=Some((17, 37)) page=2482x814',
  '1000000000897 event {"t":"loading","v":true}',
  '1000000000898 first-frame after 366ms geometry=Some((2482, 814)) payload_bytes=8081424',
  '1000000000901 event {"t":"title","v":"Terminal-Fenster repaint bench"}',
  '1000000008539 frame-stats samples=459 encode_ms_p50=0.67 encode_ms_p99=0.91 wire_bytes_p50=53120 wire_bytes_p99=54788 gap_samples=458 gap_ms_p50=16.67 gap_ms_p99=19.94 convert_ms_p50=0.19 convert_ms_p99=0.28',
  '1000000008540 bounded-run complete frames=459 fps=60 last_wire_bytes=53999 encode_ms=0.74 convert_ms=0.21',
].join('\n');

async function selfTest() {
  const out = [];
  let failures = 0;
  const eq = (name, got, want) => {
    const pass = got === want;
    if (!pass) failures++;
    out.push(`  ${pass ? 'PASS' : 'FAIL'}  ${name}: got ${got}${pass ? '' : `, want ${want}`}`);
  };
  const near = (name, got, want, tol) => {
    const pass = typeof got === 'number' && Math.abs(got - want) <= tol;
    if (!pass) failures++;
    out.push(`  ${pass ? 'PASS' : 'FAIL'}  ${name}: got ${got}${pass ? '' : `, want ~${want}`}`);
  };

  out.push('[self-test] PARSER FIXTURE — synthetic log input, NOT a measurement.');
  out.push('');
  out.push('fixture: happy path');
  const r = parseRunLog(FIXTURE_OK, {
    spawnedAtMs: 1000000000000, exitedAtMs: 1000000010100, exitCode: 0, stderr: '',
  });
  eq('ok', r.ok, true);
  eq('backend', r.terminal.backend, 'kitty');
  eq('kitty_graphics', r.terminal.kitty_graphics, true);
  eq('kitty_shared_memory', r.terminal.kitty_shared_memory, true);
  eq('page width', r.terminal.page_px[0], 2482);
  eq('cell height', r.terminal.cell_px[1], 37);
  eq('tty_and_detect_ms', r.startup.tty_and_detect_ms, 320);
  eq('engine_spawn_connect_ms', r.startup.engine_spawn_connect_ms, 212);
  eq('first_frame_after_run_start_ms', r.startup.first_frame_after_run_start_ms, 366);
  eq('spawn_to_first_frame_ms', r.startup.spawn_to_first_frame_ms, 898);
  eq('spawn_to_exit_ms', r.startup.spawn_to_exit_ms, 10100);
  eq('frames', r.frames.count, 459);
  eq('received frames', r.frames.received_count, 459);
  eq('presented frames', r.frames.presented_count, 459);
  eq('run_window_ms', r.frames.run_window_ms, 8008);
  eq('steady_window_ms', r.frames.steady_window_ms, 7642);
  near('fps_steady_state', r.frames.fps_steady_state, 59.93, 0.02);
  near('fps_over_window', r.frames.fps_over_window, 57.32, 0.02);
  near('received fps steady state', r.frames.fps_received_steady_state, 59.93, 0.02);
  eq('last_wire_bytes', r.bytes.last_wire_bytes, 53999);
  eq('last_encode_ms', r.bytes.last_encode_ms, 0.74);
  eq('last_convert_ms', r.bytes.last_convert_ms, 0.21);
  eq('per-frame samples', r.bytes.frame_samples, 459);
  eq('encode p50', r.bytes.encode_ms_p50, 0.67);
  eq('encode p99', r.bytes.encode_ms_p99, 0.91);
  eq('conversion p50', r.bytes.convert_ms_p50, 0.19);
  eq('conversion p99', r.bytes.convert_ms_p99, 0.28);
  eq('wire bytes p50', r.bytes.wire_bytes_p50, 53120);
  eq('wire bytes p99', r.bytes.wire_bytes_p99, 54788);
  eq('presentation gap samples', r.bytes.present_gap_samples, 458);
  eq('presentation gap p50', r.bytes.present_gap_ms_p50, 16.67);
  eq('presentation gap p99', r.bytes.present_gap_ms_p99, 19.94);
  eq('uses per-frame distribution', r.bytes.samples_are_last_frame_only, false);
  eq('raw_pixel_bytes', r.bytes.raw_pixel_bytes, 8081392);
  eq('payload = raw + 32 header', r.bytes.first_frame_payload_bytes, 8081392 + 32);
  near('compression_ratio_approx', r.bytes.compression_ratio_approx, 152.14, 0.02);
  eq('events counted', r.events, 2);

  out.push('');
  out.push('fixture: received frames coalesce before terminal presentation');
  const c = parseRunLog(FIXTURE_OK.replace(
    'frame-stats samples=459',
    'frame-stats samples=230'
  ).replace('gap_samples=458', 'gap_samples=229'), {
    spawnedAtMs: 1000000000000, exitedAtMs: 1000000010100, exitCode: 0, stderr: '',
  });
  eq('coalesced run remains valid', c.ok, true);
  eq('received count preserved', c.frames.received_count, 459);
  eq('presentation count used', c.frames.presented_count, 230);
  near('headline fps is terminal presentations', c.frames.fps_steady_state, 29.97, 0.02);
  near('received fps remains diagnostic', c.frames.fps_received_steady_state, 59.93, 0.02);

  out.push('');
  out.push('fixture: empty log (terminal never answered the capability queries)');
  const e = parseRunLog('', { spawnedAtMs: 1, exitedAtMs: 2, exitCode: 1, stderr: 'terminal-fenster: could not determine terminal pixel size.' });
  eq('not ok', e.ok, false);
  eq('start metric is null, not zero', e.startup.spawn_to_first_frame_ms, null);
  eq('frame count is null, not zero', e.frames.count, null);
  eq('stderr captured', e.stderr, 'terminal-fenster: could not determine terminal pixel size.');
  const hasHint = e.errors.some((x) => x.includes('terminal pixel size'));
  eq('diagnoses the tty cause', hasHint, true);

  out.push('');
  out.push('fixture: engine started but never painted');
  const n = parseRunLog(
    '1000000000320 start url=about:blank term=None backend=unicode kitty_gfx=false kitty_kbd=false pixel_mouse=false viewport=800x600 cell=Some((8, 16)) page=800x584\n' +
    '1000000008500 bounded-run complete frames=0 fps=0 last_wire_bytes=0 encode_ms=0.00',
    { spawnedAtMs: 1000000000000, exitedAtMs: 1000000009000, exitCode: 0, stderr: '' });
  eq('not ok', n.ok, false);
  eq('term=None parsed', n.terminal.term_program, null);
  eq('backend', n.terminal.backend, 'unicode');
  eq('no first frame -> null, not zero', n.startup.spawn_to_first_frame_ms, null);
  eq('fps not fabricated', n.frames.fps_steady_state, null);

  out.push('');
  out.push('fixture: wire format drift is caught, not silently ratioed');
  const d = parseRunLog(
    '1000000000320 start url=about:blank term=Some("ghostty") backend=kitty kitty_gfx=true kitty_kbd=true pixel_mouse=true viewport=100x100 cell=Some((10, 10)) page=100x90\n' +
    '1000000000500 first-frame after 180ms geometry=Some((100, 90)) payload_bytes=12345\n' +
    '1000000008500 bounded-run complete frames=10 fps=1 last_wire_bytes=100 encode_ms=0.10',
    { spawnedAtMs: 1000000000000, exitedAtMs: 1000000009000, exitCode: 0, stderr: '' });
  eq('not ok', d.ok, false);
  eq('flags the format change', d.errors.some((x) => x.includes('wire format changed')), true);

  out.push('');
  out.push('percentile: nearest-rank');
  eq('p50 of 1..5', percentile([1, 2, 3, 4, 5], 50), 3);
  eq('p100 of 1..5', percentile([1, 2, 3, 4, 5], 100), 5);
  eq('p50 of single', percentile([7], 50), 7);

  // The RSS path is the only measurement channel that does not need a tty, so
  // it can be exercised for real here against a throwaway process tree:
  //   sh
  //    +- sleep      (background)
  //    +- sleep      (foreground)
  // This proves psSnapshot parsing, descendant walking and peak tracking work
  // on a genuine multi-process tree, which is the shape Chromium presents.
  out.push('');
  out.push('rss sampler: live process tree (sh + 2 sleeps)');
  // The child must outlive several ps calls, and ps costs 170-700 ms here, so
  // a short-lived tree would make this test measure scheduling luck.
  const kid = spawn('/bin/sh', ['-c', 'sleep 3 & sleep 3 & wait'], { stdio: 'ignore' });
  const sampler = new RssSampler(kid.pid, 150);
  sampler.start();
  await new Promise((r) => kid.on('exit', r));
  await sampler.stop();
  const s = sampler.summary();
  const gt = (name, got, min) => {
    const pass = typeof got === 'number' && got > min;
    if (!pass) failures++;
    out.push(`  ${pass ? 'PASS' : 'FAIL'}  ${name}: got ${got}${pass ? '' : `, want > ${min}`}`);
  };
  gt('samples taken', s.samples, 0);
  gt('peak tree RSS kb', s.peak_tree_total_kb, 0);
  gt('processes at peak (tree walk found children)', s.peak_n_procs, 1);
  gt('ps cost measured', s.ps_cost_ms_p50, 0);
  eq('ps failures', s.ps_failures, 0);
  eq('breakdown sorted desc', s.peak_breakdown
    ? s.peak_breakdown.every((p, i, a) => i === 0 || a[i - 1].rss_kb >= p.rss_kb) : false, true);

  // Guard the very last step of a real benchmark. Without this, a formatting
  // bug would surface only after all runs had completed, destroying the
  // measurement it was supposed to print.
  out.push('');
  out.push('summary + renderer: does not throw on fixture data');
  try {
    const fixtureRun = {
      index: 0, kind: 'cold', ok: r.ok, errors: r.errors, exit_code: 0, stderr: null,
      terminal: r.terminal, startup: r.startup, frames: r.frames, bytes: r.bytes,
      rss: s,
    };
    const rep = {
      binary: { path: '/x/terminal-fenster', version: 'terminal-fenster 0.1.0', sha256: 'deadbeefdeadbeef' },
      host: { cpu_brand: 'Apple M4', arch: 'arm64', platform: 'darwin', os_version: '26.1', release: '25.1.0' },
      config: { url: 'file:///x/repaint.html', runs: 1, duration_ms: 8000, sample_ms: 250 },
      runs: [fixtureRun],
      summary: summarize([fixtureRun]),
      _json_path: '/x/out.json',
    };
    const text = renderText(rep);
    eq('renderer returns text', typeof text === 'string' && text.length > 400, true);
    eq('renders start section', text.includes('START TO FIRST FRAME'), true);
    eq('renders throughput section', text.includes('THROUGHPUT'), true);
    eq('renders memory section', text.includes('MEMORY'), true);
    eq('summary counts the ok run', rep.summary.runs_ok, 1);
    eq('summary reports cold start', rep.summary.cold_start.spawn_to_first_frame_ms, 898);
  } catch (e) {
    failures++;
    out.push(`  FAIL  renderer threw: ${e.message}`);
  }

  out.push('');
  out.push(failures === 0 ? `[self-test] all checks passed` : `[self-test] ${failures} CHECK(S) FAILED`);
  process.stdout.write(out.join('\n') + '\n');
  return failures === 0 ? 0 : 1;
}

// --------------------------------------------------------------------- main

async function main() {
  const opt = parseArgs(process.argv.slice(2));

  if (opt.selfTest) process.exit(await selfTest());

  const pre = await preflight(opt);
  const fatalChecks = pre.checks.filter((c) => c.fatal);

  if (opt.dryRun || fatalChecks.length > 0) {
    const L = ['', 'Terminal-Fenster benchmark — preflight', '='.repeat(72)];
    for (const c of pre.checks) {
      L.push(`  [${c.ok ? ' ok ' : c.fatal ? 'FAIL' : 'warn'}] ${c.name}`);
      L.push(`         ${c.detail}`);
    }
    L.push('');
    L.push('PLAN');
    L.push('-'.repeat(72));
    L.push(`  ${opt.runs} run(s) of ${opt.durationMs} ms each  (run 0 = cold, rest = warm)`);
    L.push(`  command per run:`);
    L.push(`    TERMINAL_FENSTER_LOG=<out>/logs/run-N.log \\`);
    L.push(`    TERMINAL_FENSTER_EXIT_AFTER_MS=${opt.durationMs} \\`);
    if (opt.backend) L.push(`    TERMINAL_FENSTER_BACKEND=${opt.backend} \\`);
    L.push(`    ${opt.bin} open ${opt.url}`);
    L.push(`  RSS sampled every ${opt.sampleMs} ms over the process tree`);
    L.push(`  results -> ${opt.out}`);
    L.push('');
    if (fatalChecks.length > 0) {
      L.push(`BLOCKED: ${fatalChecks.length} preflight check(s) failed. Nothing was executed.`);
      L.push('');
    }
    process.stdout.write(L.join('\n') + '\n');
    process.exit(fatalChecks.length > 0 ? 1 : 0);
  }

  const logsDir = path.join(opt.out, 'logs');
  await fs.mkdir(logsDir, { recursive: true });

  // Nothing may be written to stdout or stderr while a run is in flight:
  // stdout is the graphics channel and a stray byte corrupts an image
  // mid-transmission. All output is buffered and printed at the end.
  const runs = [];
  for (let i = 0; i < opt.runs; i++) {
    const logPath = path.join(logsDir, `run-${i}.log`);
    runs.push(await runOnce(opt, i, logPath));
    if (i < opt.runs - 1) await sleep(opt.settleMs);
  }
  if (!opt.keepLogs) await fs.rm(logsDir, { recursive: true, force: true });

  const stamp = new Date().toISOString().replace(/[:.]/g, '-');
  const name = `terminal-fenster-bench-${stamp}${opt.label ? '-' + opt.label : ''}.json`;
  const jsonPath = path.join(opt.out, name);

  const report = {
    schema_version: SCHEMA_VERSION,
    generated_at: new Date().toISOString(),
    tool: 'benchmarks/bench.mjs',
    host: await hostInfo(),
    binary: {
      path: opt.bin,
      version: pre.version,
      size_bytes: pre.binStat?.size ?? null,
      mtime: pre.binStat?.mtime?.toISOString() ?? null,
      sha256: pre.binStat ? await sha256(opt.bin) : null,
      engine: pre.enginePath,
    },
    config: {
      url: opt.url,
      runs: opt.runs,
      duration_ms: opt.durationMs,
      sample_ms: opt.sampleMs,
      settle_ms: opt.settleMs,
      forced_backend: opt.backend,
      label: opt.label,
    },
    preflight: pre.checks,
    definitions: {
      cold: 'run index 0 — the first launch of this series. The OS page cache is NOT ' +
            'purged, so this is "first launch", not "cold disk". For a true cold-cache ' +
            'number run `sudo purge` (macOS) by hand immediately before the harness.',
      warm: 'runs 1..n-1 — launched after a previous run of the same binary, so the ' +
            'Electron framework and Chromium are already in the page cache.',
      spawn_to_first_frame_ms:
        'wall clock from spawn() of the terminal-fenster process to the timestamp of its ' +
        '"first-frame" log line. Crosses the process boundary; both sides read the same ' +
        'Unix millisecond clock.',
      fps_steady_state:
        '(completed terminal presentations - 1) / (time from first frame to end of the ' +
        'bounded window). Excludes load time and does not overcount socket frames that ' +
        'the core coalesces before presenting.',
      presentation_distribution:
        'nearest-rank p50/p99 over every completed terminal presentation in a bounded ' +
        'run. Encode time excludes the terminal write; presentation gaps are measured ' +
        'after each stdout flush and therefore include output backpressure.',
      rss: 'sum of per-process RSS across the terminal-fenster process tree, sampled by ps. ' +
           'Chromium helpers share pages, so the sum is an upper bound.',
    },
    summary: summarize(runs),
    runs,
    _json_path: jsonPath,
  };

  await fs.writeFile(jsonPath, JSON.stringify(report, null, 2));
  await fs.writeFile(path.join(opt.out, 'latest.json'), JSON.stringify(report, null, 2));

  if (opt.jsonOnly) {
    process.stdout.write(JSON.stringify(report, null, 2) + '\n');
  } else {
    // Reset attributes and clear: the last run left an image on screen.
    process.stdout.write('\x1b[0m\x1b[2J\x1b[H');
    process.stdout.write(renderText(report));
  }
  process.exit(report.summary.runs_failed > 0 ? 1 : 0);
}

// Only run when invoked directly, so the parser can be imported by tests.
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((e) => {
    process.stderr.write(`bench: unexpected failure: ${e?.stack || e}\n`);
    process.exit(1);
  });
}
