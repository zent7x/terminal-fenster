#!/bin/sh
set -eu

PROJECT_DIR=$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd)
cd "$PROJECT_DIR"

step() {
  printf '\n==> %s\n' "$1"
  shift
  "$@"
}

for command_name in cargo node npm; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    printf 'release check: required command not found: %s\n' "$command_name" >&2
    exit 1
  fi
done

ELECTRON="$PROJECT_DIR/apps/engine/node_modules/.bin/electron"
if [ ! -x "$ELECTRON" ]; then
  printf 'release check: Electron is not installed; run (cd apps/engine && npm ci)\n' >&2
  exit 1
fi

step "Rust formatting" cargo fmt --all -- --check
step "Rust tests" cargo test --workspace --locked
step "Rust lint" cargo clippy --workspace --all-targets -- -D warnings
step "Optimized Rust build" cargo build --workspace --release --locked
# Pre-rename orphan; cargo does not remove it when the binary was renamed.
rm -f "$PROJECT_DIR/target/release/blackglass" "$PROJECT_DIR/target/release/blackglass.d"
step "Installed-layout discovery smoke test" tools/install-layout-test.sh
step "Prebuilt archive layout/checksum/install smoke test" tools/package-layout-test.sh
step "Bash installer/packager syntax" bash -n install.sh uninstall.sh packaging/install-prebuilt.sh tools/package-release.sh
step "POSIX packaging/helper syntax" sh -n packaging/electron-launcher.sh packaging/verify-release.sh tools/ghostty-smoke.sh tools/install-layout-test.sh tools/package-layout-test.sh tools/release-check.sh
step "Packaging metadata syntax" node --check packaging/read-engine-lock.mjs
step "Cargo license collector syntax" node --check packaging/copy-cargo-licenses.mjs

(
  cd "$PROJECT_DIR/apps/engine"
  step "Engine scheduler/damage tests" npm test
)
step "Real-pixel engine integration" node tests/e2e/input-injection.js
step "Browser fixture matrix" "$ELECTRON" tests/fixtures/verify-fixtures.js
step "Benchmark parser self-test" node benchmarks/bench.mjs --self-test
step "Headless memory/cleanup smoke test" node benchmarks/engine-rss.js --duration 750 --json

(
  cd "$PROJECT_DIR/packages/mcp"
  step "MCP compositor and protocol tests" npm test
  step "MCP live Chromium tools" npm run test:live
)

(
  cd "$PROJECT_DIR/website"
  step "Website dependency install" npm ci
  step "Website production build" npm run build
)

step "Patch whitespace" git diff --check

printf '\nAutomated release gates passed.\n'
printf '%s\n' 'This is not permission to tag a release. Complete RELEASE.md terminal, performance,'
printf '%s\n' 'packaging, and signing gates from a real graphics terminal first.'
