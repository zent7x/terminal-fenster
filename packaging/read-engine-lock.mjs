#!/usr/bin/env node

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.dirname(here);
const target = process.argv[2];
const lock = JSON.parse(fs.readFileSync(path.join(here, 'engine-lock.json'), 'utf8'));
const enginePackage = JSON.parse(
  fs.readFileSync(path.join(root, 'apps/engine/package.json'), 'utf8'),
);
const artifact = lock.artifacts[target];

if (!artifact) {
  console.error(`unsupported release target: ${target || '(missing)'}`);
  process.exit(2);
}
if (enginePackage.dependencies?.electron !== lock.electronVersion) {
  console.error(
    `engine-lock Electron ${lock.electronVersion} does not match package pin ` +
      `${enginePackage.dependencies?.electron || '(missing)'}`,
  );
  process.exit(2);
}

const values = [
  lock.electronVersion,
  lock.chromiumVersion,
  lock.codecPolicy,
  lock.releaseBaseUrl,
  artifact.electron.file,
  artifact.electron.sha256,
  artifact.ffmpeg.file,
  artifact.ffmpeg.sha256,
  artifact.ffmpeg.member,
  artifact.ffmpeg.destination,
  artifact.runtime,
];
if (values.some((value) => typeof value !== 'string' || value.includes('\t'))) {
  throw new Error('engine-lock contains a missing or tab-delimited field');
}
process.stdout.write(`${values.join('\t')}\n`);
