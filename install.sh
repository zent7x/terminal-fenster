#!/usr/bin/env bash
# Terminal-Fenster installer — build from source and lay the binary out the way the runtime expects.
#
# Once a signed/tagged public release exists, the script can be piped from that immutable tag.
# Do not use the mutable `main` URL as a reproducible release channel.
#
# or, from a checkout:
#
#   ./install.sh
#
# It produces the layout resolve_engine() looks for (apps/cli/src/main.rs):
#
#   $PREFIX/bin/terminal-fenster        the release binary
#   $PREFIX/engine/               the Electron host (package.json + node_modules)
#   $BINDIR/terminal-fenster -> $PREFIX/bin/terminal-fenster    (a symlink on your PATH)
#
# Because the binary canonicalises its own path and then looks for a sibling `engine/`, the
# PATH symlink resolves to $PREFIX/bin/terminal-fenster and finds $PREFIX/engine with no env var.
#
# Knobs (all optional):
#   TERMINAL_FENSTER_PREFIX   install root        (default: $HOME/.local/share/terminal-fenster)
#   TERMINAL_FENSTER_BINDIR   PATH symlink dir     (default: $HOME/.local/bin)
#   TERMINAL_FENSTER_REPO     clone URL if not run inside a checkout
#                                            (default: https://github.com/zent7x/terminal-fenster)
#   TERMINAL_FENSTER_REF      branch/tag to clone  (default: main; use a release tag for reproducibility)
#   TERMINAL_FENSTER_REUSE_ENGINE=1  copy an existing apps/engine/node_modules instead of running
#                              `npm ci` — faster for a same-machine reinstall / offline install.
set -euo pipefail

PREFIX="${TERMINAL_FENSTER_PREFIX:-$HOME/.local/share/terminal-fenster}"
BINDIR="${TERMINAL_FENSTER_BINDIR:-$HOME/.local/bin}"
REPO="${TERMINAL_FENSTER_REPO:-https://github.com/zent7x/terminal-fenster}"
REF="${TERMINAL_FENSTER_REF:-main}"
MIN_NODE_MAJOR=22
MIN_NODE_MINOR=12
SOURCE_TMP=""
STAGE=""
BACKUP=""
ACTIVATED=0

say()  { printf '\033[1;32m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

cleanup() {
  status=$?
  if [ -n "$STAGE" ] && [ -d "$STAGE" ]; then
    rm -rf -- "$STAGE"
  fi
  if [ "$status" -ne 0 ] && [ "$ACTIVATED" -eq 1 ]; then
    [ ! -e "$PREFIX" ] || rm -rf -- "$PREFIX"
    if [ -n "$BACKUP" ] && [ -d "$BACKUP" ]; then
      mv -- "$BACKUP" "$PREFIX" || true
    fi
  elif [ "$status" -ne 0 ] && [ -n "$BACKUP" ] && [ -d "$BACKUP" ] && [ ! -e "$PREFIX" ]; then
    mv -- "$BACKUP" "$PREFIX" || true
  fi
  if [ -n "$SOURCE_TMP" ] && [ -d "$SOURCE_TMP" ]; then
    rm -rf -- "$SOURCE_TMP"
  fi
  exit "$status"
}
trap cleanup EXIT

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
  *) die "unsupported OS '$os' — Terminal-Fenster supports macOS and Linux" ;;
esac

# --- 2. prerequisites -----------------------------------------------------------------------
need() { command -v "$1" >/dev/null 2>&1 || die "missing prerequisite: $1 ($2)"; }
need cargo "install Rust from https://rustup.rs"
need node  "install Node.js >= ${MIN_NODE_MAJOR} from https://nodejs.org"
need npm   "ships with Node.js"

node_ok="$(node -p "const [a,b]=process.versions.node.split('.').map(Number); +(a>${MIN_NODE_MAJOR} || (a===${MIN_NODE_MAJOR} && b>=${MIN_NODE_MINOR}))")"
[ "$node_ok" = "1" ] \
  || die "Node.js >= ${MIN_NODE_MAJOR}.${MIN_NODE_MINOR}.0 required by Electron; found $(node -v)"

case "$PREFIX" in
  /*) ;;
  *) PREFIX="$PWD/$PREFIX" ;;
esac
case "$BINDIR" in
  /*) ;;
  *) BINDIR="$PWD/$BINDIR" ;;
esac
case "$PREFIX" in
  ""|/|"$HOME") die "refusing unsafe TERMINAL_FENSTER_PREFIX=$PREFIX" ;;
esac

# --- 3. locate the source tree (a checkout we're inside, else clone) ------------------------
script_dir="$(cd "$(dirname "$0")" && pwd)"
if [ -f "$script_dir/Cargo.toml" ] && grep -q 'members.*apps/cli' "$script_dir/Cargo.toml" 2>/dev/null; then
  SRC="$script_dir"
  say "building from this checkout: $SRC"
else
  command -v git >/dev/null 2>&1 || die "missing prerequisite: git (needed to fetch the source)"
  SOURCE_TMP="$(mktemp -d "${TMPDIR:-/tmp}/terminal-fenster-src.XXXXXX")"
  SRC="$SOURCE_TMP"
  say "cloning $REPO@$REF"
  git clone --depth 1 --branch "$REF" "$REPO" "$SRC" \
    || die "clone failed — set TERMINAL_FENSTER_REPO/TERMINAL_FENSTER_REF or run install.sh from a checkout"
fi

[ -d "$SRC/apps/engine" ] || die "source tree at $SRC is missing apps/engine"
if [ "$REF" = "main" ] && [ "$SRC" = "$SOURCE_TMP" ]; then
  warn "installing mutable ref 'main'; set TERMINAL_FENSTER_REF to a release tag for a reproducible install"
fi

# --- 4. build the release binary ------------------------------------------------------------
say "building the release binary (cargo build --release)"
( cd "$SRC" && cargo build --release --locked ) || die "cargo build failed"
bin_src="$SRC/target/release/terminal-fenster"
[ -x "$bin_src" ] || die "expected binary not found at $bin_src"

# --- 5. stage the complete install beside the destination -----------------------------------
# The binary shells out to Electron at runtime, so the engine (package.json + node_modules)
# must live beside it. Install it fresh with `npm ci` for a platform-correct Electron, unless
# asked to reuse an existing one. Nothing below touches the working prefix until the staged
# engine has materialized and passed validation.
install_parent="$(dirname "$PREFIX")"
mkdir -p "$install_parent"
[ ! -L "$PREFIX" ] || die "refusing symlink install prefix $PREFIX"
if [ -e "$PREFIX" ] \
   && [ ! -f "$PREFIX/.terminal-fenster-install" ] \
   && { [ ! -x "$PREFIX/bin/terminal-fenster" ] || [ ! -f "$PREFIX/engine/package.json" ]; }; then
  die "refusing to replace unrelated prefix $PREFIX (missing Terminal-Fenster marker/layout)"
fi
STAGE="$(mktemp -d "$install_parent/.terminal-fenster-stage.XXXXXX")"
engine_stage="$STAGE/engine"
mkdir -p "$STAGE/bin" "$engine_stage"
say "staging the Electron engine at $engine_stage"
cp -R "$SRC/apps/engine/src" "$engine_stage/src"
cp "$SRC/apps/engine/package.json" "$SRC/apps/engine/package-lock.json" "$engine_stage/"

if [ "${TERMINAL_FENSTER_REUSE_ENGINE:-0}" = "1" ] && [ -d "$SRC/apps/engine/node_modules" ]; then
  say "reusing existing node_modules (TERMINAL_FENSTER_REUSE_ENGINE=1)"
  cp -R "$SRC/apps/engine/node_modules" "$engine_stage/node_modules"
else
  ( cd "$engine_stage" && npm ci --omit=dev --no-audit --no-fund ) \
    || die "npm ci failed in $engine_stage"
fi
[ -x "$engine_stage/node_modules/.bin/electron" ] \
  || die "engine install incomplete: node_modules/.bin/electron is missing"

# Electron 43's npm package is lazy: npm ci can leave a launcher but no Chromium runtime.
# Invoke it while output is visible, then validate the exact file named by path.txt.
say "materializing and validating the pinned Electron runtime"
"$engine_stage/node_modules/.bin/electron" --version \
  || die "Electron runtime download/materialization failed"
electron_version="$("$engine_stage/node_modules/.bin/electron" --version)"
expected_electron="$(node -e 'const p=require(process.argv[1]); process.stdout.write(p.dependencies.electron)' "$SRC/apps/engine/package.json")"
[ "$electron_version" = "v$expected_electron" ] \
  || die "Electron version mismatch: expected v$expected_electron, got $electron_version"
path_file="$engine_stage/node_modules/electron/path.txt"
[ -f "$path_file" ] || die "Electron runtime is incomplete: $path_file is missing"
runtime_relative="$(tr -d '\r\n' < "$path_file")"
[ -n "$runtime_relative" ] || die "Electron runtime is incomplete: path.txt is empty"
[ -f "$engine_stage/node_modules/electron/dist/$runtime_relative" ] \
  || die "Electron runtime is incomplete: dist/$runtime_relative is missing"

install -m 0755 "$bin_src" "$STAGE/bin/terminal-fenster"
cp -R "$SRC/packages/mcp" "$STAGE/mcp"
cp "$SRC/LICENSE-MIT" "$SRC/NOTICE.md" "$STAGE/"
mkdir -p "$STAGE/licenses/rust"
node "$SRC/packaging/copy-cargo-licenses.mjs" "$STAGE/licenses/rust" \
  || die "could not collect Cargo dependency licenses"
"$STAGE/bin/terminal-fenster" version > "$STAGE/.terminal-fenster-install"

# --- 6. atomically replace the dedicated prefix, with rollback ------------------------------
if [ -e "$PREFIX" ]; then
  BACKUP="$(mktemp -d "$install_parent/.terminal-fenster-backup.XXXXXX")"
  rmdir "$BACKUP"
  mv -- "$PREFIX" "$BACKUP"
fi
ACTIVATED=1
if ! mv -- "$STAGE" "$PREFIX"; then
  STAGE=""
  [ -z "$BACKUP" ] || mv -- "$BACKUP" "$PREFIX"
  BACKUP=""
  ACTIVATED=0
  die "could not activate staged install at $PREFIX"
fi
STAGE=""

mkdir -p "$BINDIR"
ln -sfn "$PREFIX/bin/terminal-fenster" "$BINDIR/terminal-fenster"
"$BINDIR/terminal-fenster" version >/dev/null \
  || die "installed binary failed its non-interactive version smoke test"
if [ -n "$BACKUP" ]; then
  rm -rf -- "$BACKUP"
  BACKUP=""
fi
ACTIVATED=0
say "installed: $BINDIR/terminal-fenster -> $PREFIX/bin/terminal-fenster (engine at $PREFIX/engine)"

# --- 7. PATH guidance + verification --------------------------------------------------------
case ":$PATH:" in
  *":$BINDIR:"*) ;;
  *) warn "$BINDIR is not on your PATH. Add this to your shell profile:"
     # shellcheck disable=SC2016  # the literal $PATH is deliberate — it's guidance to paste
     printf '\n    export PATH="%s:$PATH"\n\n' "$BINDIR" ;;
esac

say "done. Run setup to verify everything:"
printf '\n    %s/terminal-fenster setup\n\n' "$BINDIR"
printf 'Interactive browsing (Ghostty / kitty / WezTerm / iTerm2):\n\n    %s/terminal-fenster open example.com\n\n' "$BINDIR"
printf 'Headless / scripts (any terminal, including macOS Terminal):\n\n    %s/terminal-fenster open example.com --headless\n\n' "$BINDIR"
