#!/bin/sh
set -eu

PROJECT_DIR=$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd)
LABEL=${TERMINAL_FENSTER_SMOKE_LABEL:-default}
DURATION_MS=${TERMINAL_FENSTER_SMOKE_MS:-8000}
HOLD_SECONDS=${TERMINAL_FENSTER_SMOKE_HOLD_SECONDS:-8}
PAGE=${TERMINAL_FENSTER_SMOKE_PAGE:-repaint.html}
LOG_PATH="${TERMINAL_FENSTER_SMOKE_LOG:-$PROJECT_DIR/dist/ghostty-smoke-$LABEL.log}"
mkdir -p "$(dirname "$LOG_PATH")"

rm -f "$LOG_PATH" "$LOG_PATH.engine.stderr"
TERMINAL_FENSTER_LOG="$LOG_PATH" \
TERMINAL_FENSTER_EXIT_AFTER_MS="$DURATION_MS" \
  "$PROJECT_DIR/target/release/terminal-fenster" open \
  "$PROJECT_DIR/benchmarks/pages/$PAGE"
rc=$?

printf '\nTERMINAL_FENSTER_GHOSTTY_SMOKE_EXIT=%s\n' "$rc"
printf 'Log: %s\n' "$LOG_PATH"
sleep "$HOLD_SECONDS"
exit "$rc"
