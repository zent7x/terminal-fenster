#!/usr/bin/env bash
# BlackGlass installer — build from source and lay the binary out the way the runtime expects.
#
#   curl -fsSL https://raw.githubusercontent.com/zent7x/blackglass/main/install.sh | bash
#
# or, from a checkout:
#
#   ./install.sh
#
# It produces the layout resolve_engine() looks for (apps/cli/src/main.rs):
#
#   $PREFIX/bin/blackglass        the release binary
#   $PREFIX/engine/               the Electron host (package.json + node_modules)
#   $BINDIR/blackglass -> $PREFIX/bin/blackglass    (a symlink on your PATH)
#
# Because the binary canonicalises its own path and then looks for a sibling `engine/`, the
# PATH symlink resolves to $PREFIX/bin/blackglass and finds $PREFIX/engine with no env var.
#
# Knobs (all optional):
#   BLACKGLASS_PREFIX   install root        (default: $HOME/.local/share/blackglass)
#   BLACKGLASS_BINDIR   PATH symlink dir     (default: $HOME/.local/bin)
#   BLACKGLASS_REPO     clone URL if not run inside a checkout
#                                            (default: https://github.com/zent7x/blackglass)
#   BLACKGLASS_REF      branch/tag to clone  (default: main)
#   BLACKGLASS_REUSE_ENGINE=1  copy an existing apps/engine/node_modules instead of running
#                              `npm ci` — faster for a same-machine reinstall / offline install.
set -euo pipefail

PREFIX="${BLACKGLASS_PREFIX:-$HOME/.local/share/blackglass}"
BINDIR="${BLACKGLASS_BINDIR:-$HOME/.local/bin}"
REPO="${BLACKGLASS_REPO:-https://github.com/zent7x/blackglass}"
REF="${BLACKGLASS_REF:-main}"
MIN_NODE_MAJOR=22

say()  { printf '\033[1;32m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

case "${1:-}" in
  -h|--help)
    sed -n '2,26p' "$0" | sed 's/^# \{0,1\}//'
    exit 0
    ;;
esac

# --- 1. platform ----------------------------------------------------------------------------
os="$(uname -s)"
case "$os" in
  Darwin|Linux) ;;
  *) die "unsupported OS '$os' — BlackGlass supports macOS and Linux" ;;
esac

# --- 2. prerequisites -----------------------------------------------------------------------
need() { command -v "$1" >/dev/null 2>&1 || die "missing prerequisite: $1 ($2)"; }
need cargo "install Rust from https://rustup.rs"
need node  "install Node.js >= ${MIN_NODE_MAJOR} from https://nodejs.org"
need npm   "ships with Node.js"

node_major="$(node -p 'process.versions.node.split(".")[0]')"
[ "$node_major" -ge "$MIN_NODE_MAJOR" ] \
  || die "Node.js >= ${MIN_NODE_MAJOR} required (the engine's in-process CDP needs a global WebSocket); found $(node -v)"

# --- 3. locate the source tree (a checkout we're inside, else clone) ------------------------
script_dir="$(cd "$(dirname "$0")" && pwd)"
if [ -f "$script_dir/Cargo.toml" ] && grep -q 'members.*apps/cli' "$script_dir/Cargo.toml" 2>/dev/null; then
  SRC="$script_dir"
  say "building from this checkout: $SRC"
else
  command -v git >/dev/null 2>&1 || die "missing prerequisite: git (needed to fetch the source)"
  SRC="$(mktemp -d "${TMPDIR:-/tmp}/blackglass-src.XXXXXX")"
  trap 'rm -rf "$SRC"' EXIT
  say "cloning $REPO@$REF"
  git clone --depth 1 --branch "$REF" "$REPO" "$SRC" \
    || die "clone failed — set BLACKGLASS_REPO/BLACKGLASS_REF or run install.sh from a checkout"
fi

[ -d "$SRC/apps/engine" ] || die "source tree at $SRC is missing apps/engine"

# --- 4. build the release binary ------------------------------------------------------------
say "building the release binary (cargo build --release)"
( cd "$SRC" && cargo build --release --locked ) || die "cargo build failed"
bin_src="$SRC/target/release/blackglass"
[ -x "$bin_src" ] || die "expected binary not found at $bin_src"

# --- 5. stage the engine --------------------------------------------------------------------
# The binary shells out to Electron at runtime, so the engine (package.json + node_modules)
# must live beside it. Install it fresh with `npm ci` for a platform-correct Electron, unless
# asked to reuse an existing one.
engine_dst="$PREFIX/engine"
say "installing the Electron engine into $engine_dst"
rm -rf "$engine_dst"
mkdir -p "$engine_dst"
cp -R "$SRC/apps/engine/src" "$engine_dst/src"
cp "$SRC/apps/engine/package.json" "$SRC/apps/engine/package-lock.json" "$engine_dst/"

if [ "${BLACKGLASS_REUSE_ENGINE:-0}" = "1" ] && [ -d "$SRC/apps/engine/node_modules" ]; then
  say "reusing existing node_modules (BLACKGLASS_REUSE_ENGINE=1)"
  cp -R "$SRC/apps/engine/node_modules" "$engine_dst/node_modules"
else
  ( cd "$engine_dst" && npm ci --omit=dev --no-audit --no-fund ) \
    || die "npm ci failed in $engine_dst"
fi
[ -x "$engine_dst/node_modules/.bin/electron" ] \
  || die "engine install incomplete: node_modules/.bin/electron is missing"

# --- 6. place the binary and a PATH symlink -------------------------------------------------
mkdir -p "$PREFIX/bin" "$BINDIR"
install -m 0755 "$bin_src" "$PREFIX/bin/blackglass"
ln -sf "$PREFIX/bin/blackglass" "$BINDIR/blackglass"
say "installed: $BINDIR/blackglass -> $PREFIX/bin/blackglass (engine at $engine_dst)"

# --- 7. PATH guidance + verification --------------------------------------------------------
case ":$PATH:" in
  *":$BINDIR:"*) ;;
  *) warn "$BINDIR is not on your PATH. Add this to your shell profile:"
     # shellcheck disable=SC2016  # the literal $PATH is deliberate — it's guidance to paste
     printf '\n    export PATH="%s:$PATH"\n\n' "$BINDIR" ;;
esac

say "done. Verify your terminal's capabilities with:"
printf '\n    %s/blackglass doctor\n\n' "$BINDIR"
printf 'Then browse:\n\n    %s/blackglass open example.com\n\n' "$BINDIR"
