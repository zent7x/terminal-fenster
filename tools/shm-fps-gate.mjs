#!/usr/bin/env node
// Verify the Kitty `t=s` dense path meets RELEASE.md displayed-FPS gates against direct control.
//
// Run from a real Ghostty shell (not piped, not from an agent terminal):
//   node tools/shm-fps-gate.mjs
//
// Or use the dev wrapper if terminal-fenster is not on PATH:
//   PATH="$PWD/tools:$PATH" is unnecessary — this script finds the repo binary itself.
'use strict';

import { spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO = path.resolve(HERE, '..');
const BENCH = path.join(REPO, 'benchmarks/bench.mjs');
const BIN = path.join(REPO, 'target/release/terminal-fenster');

const MIN_SHM_FPS = 20;
const MIN_SHM_RATIO = 2.0;

function fail(msg, code = 1) {
  console.error(`shm-fps-gate: ${msg}`);
  process.exit(code);
}

function preflight() {
  if (!process.stdout.isTTY || !process.stdin.isTTY) {
    fail(
      'requires a real interactive terminal (stdin and stdout must be a TTY).\n' +
      '  Open Ghostty, cd to the repo, and run: node tools/shm-fps-gate.mjs',
      2
    );
  }
  if (!fs.existsSync(BIN)) {
    fail(`missing ${BIN}\n  Run: cargo build -p terminal-fenster --release`, 2);
  }
  const electron = path.join(REPO, 'apps/engine/node_modules/.bin/electron');
  if (!fs.existsSync(electron)) {
    fail(`missing Electron engine at ${electron}\n  Run: (cd apps/engine && npm ci)`, 2);
  }
}

function runBench(label, extraEnv = {}) {
  return new Promise((resolve, reject) => {
    const env = { ...process.env, ...extraEnv };
    const child = spawn(
      process.execPath,
      [
        BENCH,
        '--bin', BIN,
        '--page', 'repaint',
        '--runs', '3',
        '--duration-ms', '5000',
        '--label', label,
        '--json-only',
      ],
      { cwd: REPO, env, stdio: ['inherit', 'pipe', 'pipe'] }
    );
    let out = '';
    let err = '';
    child.stdout.on('data', (c) => { out += c; });
    child.stderr.on('data', (c) => { err += c; });
    child.on('error', reject);
    child.on('close', (code) => {
      const lines = out.trim().split('\n').filter(Boolean);
      const last = lines[lines.length - 1];
      let json = null;
      if (last) {
        try { json = JSON.parse(last); } catch { /* not json */ }
      }
      if (!json) {
        reject(new Error(
          `bench ${label} produced no JSON (exit ${code}).\n` +
          (err.trim() ? `stderr:\n${err.trim()}\n` : '') +
          (out.trim() ? `stdout tail:\n${out.trim().slice(-2000)}\n` : '')
        ));
        return;
      }
      if (code !== 0) {
        const blocked = json.preflight?.find?.((c) => c.fatal) ??
          json.preflight?.checks?.find?.((c) => c.fatal);
        const failedRuns = json.summary?.runs_failed ?? 0;
        let detail = `exit ${code}`;
        if (blocked) detail += ` — preflight: ${blocked.name}: ${blocked.detail}`;
        else if (failedRuns) detail += ` — ${failedRuns} run(s) failed`;
        if (err.trim()) detail += `\nstderr:\n${err.trim()}`;
        reject(new Error(`bench ${label} ${detail}`));
        return;
      }
      resolve(json);
    });
  });
}

function displayedFps(summary) {
  const gap = summary?.presentation?.gap_ms_p50;
  const fps = summary?.encode?.fps_mean;
  if (typeof gap === 'number' && gap > 0) return 1000 / gap;
  return fps ?? 0;
}

async function main() {
  preflight();

  console.error('shm-fps-gate: measuring direct control (TERMINAL_FENSTER_SHM=0)…');
  const direct = await runBench('gate-direct', {
    TERMINAL_FENSTER_SHM: '0',
    TERMINAL_FENSTER_TILE_CELLS: '1x1',
  });

  console.error('shm-fps-gate: measuring shared-memory dense path…');
  const shm = await runBench('gate-shm', {});

  const directFps = displayedFps(direct.summary);
  const shmFps = displayedFps(shm.summary);
  const ratio = directFps > 0 ? shmFps / directFps : 0;

  const report = {
    direct_fps: +directFps.toFixed(2),
    shm_fps: +shmFps.toFixed(2),
    ratio: +ratio.toFixed(2),
    min_shm_fps: MIN_SHM_FPS,
    min_ratio: MIN_SHM_RATIO,
    pass: shmFps >= MIN_SHM_FPS && ratio >= MIN_SHM_RATIO,
  };

  console.log(JSON.stringify(report, null, 2));

  if (!report.pass) {
    fail(`FAIL shm=${report.shm_fps}fps direct=${report.direct_fps}fps ratio=${report.ratio}`);
  }
  console.error(`shm-fps-gate: PASS shm=${report.shm_fps}fps (${report.ratio}× direct)`);
}

main().catch((e) => {
  console.error(`shm-fps-gate: ${e.message}`);
  process.exit(2);
});
