#!/usr/bin/env node

import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

const destination = process.argv[2];
if (!destination) {
  console.error('usage: copy-cargo-licenses.mjs DESTINATION');
  process.exit(2);
}

const metadata = JSON.parse(
  execFileSync('cargo', ['metadata', '--locked', '--format-version', '1'], {
    encoding: 'utf8',
  }),
);
const packages = metadata.packages
  .filter((pkg) => pkg.source !== null)
  .sort((a, b) => `${a.name}@${a.version}`.localeCompare(`${b.name}@${b.version}`));

fs.mkdirSync(destination, { recursive: true });
const index = [];
for (const pkg of packages) {
  const sourceDirectory = path.dirname(pkg.manifest_path);
  const files = fs
    .readdirSync(sourceDirectory, { withFileTypes: true })
    .filter(
      (entry) =>
        entry.isFile() && /^(license|copying|notice|unlicense)([._-].*)?$/i.test(entry.name),
    )
    .map((entry) => entry.name)
    .sort();
  if (files.length === 0) {
    throw new Error(`${pkg.name}@${pkg.version} ships no top-level license/notice file`);
  }
  const packageDirectory = path.join(destination, `${pkg.name}-${pkg.version}`);
  fs.mkdirSync(packageDirectory, { recursive: true });
  for (const file of files) {
    fs.copyFileSync(path.join(sourceDirectory, file), path.join(packageDirectory, file));
  }
  index.push({ name: pkg.name, version: pkg.version, license: pkg.license, files });
}

fs.writeFileSync(path.join(destination, 'INDEX.json'), `${JSON.stringify(index, null, 2)}\n`);
console.log(`copied licenses for ${index.length} Cargo dependencies`);
