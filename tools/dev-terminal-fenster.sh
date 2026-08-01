#!/usr/bin/env bash
# Run the local release build without installing to ~/.local/bin.
set -euo pipefail
ROOT=$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd)
BIN="$ROOT/target/release/terminal-fenster"
STALE="$ROOT/target/release/blackglass"
if [[ -e "$STALE" ]]; then
  echo "dev-terminal-fenster: removing stale $STALE (pre-rename orphan — use terminal-fenster)" >&2
  rm -f "$STALE" "$ROOT/target/release/blackglass.d"
fi
if [[ ! -x "$BIN" ]]; then
  echo "dev-terminal-fenster: missing $BIN — run: cargo build -p terminal-fenster --release" >&2
  exit 1
fi
export TERMINAL_FENSTER_ENGINE="${TERMINAL_FENSTER_ENGINE:-$ROOT/apps/engine}"
exec "$BIN" "$@"
