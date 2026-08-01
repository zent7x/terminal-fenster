#!/usr/bin/env bash
# Install the already-built Terminal-Fenster archive containing this script. No Rust, Node, npm,
# source checkout, or network access is required.
set -euo pipefail

PREFIX="${TERMINAL_FENSTER_PREFIX:-$HOME/.local/share/terminal-fenster}"
BINDIR="${TERMINAL_FENSTER_BINDIR:-$HOME/.local/bin}"
ROOT="$(CDPATH='' cd -- "$(dirname "$0")" && pwd -P)"
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
  exit "$status"
}
trap cleanup EXIT

case "${1:-}" in
  '') ;;
  -h|--help)
    printf '%s\n' 'usage: ./install.sh'
    printf '%s\n' 'Installs this verified prebuilt release without build tools or network access.'
    exit 0
    ;;
  *) die "unknown option: $1" ;;
esac

case "$PREFIX" in /*) ;; *) PREFIX="$PWD/$PREFIX" ;; esac
case "$BINDIR" in /*) ;; *) BINDIR="$PWD/$BINDIR" ;; esac
case "$PREFIX" in ''|/|"$HOME") die "refusing unsafe TERMINAL_FENSTER_PREFIX=$PREFIX" ;; esac
case "$PREFIX/" in "$ROOT/"|"$ROOT/"*) die 'install prefix must be outside the extracted release directory' ;; esac
[ ! -L "$PREFIX" ] || die "refusing symlink install prefix $PREFIX"

say 'verifying release payload'
"$ROOT/verify.sh" || die 'release payload verification failed'

install_parent="$(dirname "$PREFIX")"
mkdir -p "$install_parent"
if [ -e "$PREFIX" ] && [ ! -f "$PREFIX/.terminal-fenster-install" ]; then
  die "refusing to replace unrelated prefix $PREFIX (missing Terminal-Fenster marker)"
fi

STAGE="$(mktemp -d "$install_parent/.terminal-fenster-stage.XXXXXX")"
say "staging the verified release at $STAGE"
cp -R "$ROOT/." "$STAGE/"
"$STAGE/bin/terminal-fenster" version > "$STAGE/.terminal-fenster-install"

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
"$BINDIR/terminal-fenster" version >/dev/null || die 'installed binary failed its version smoke test'
if [ -n "$BACKUP" ]; then
  rm -rf -- "$BACKUP"
  BACKUP=""
fi
ACTIVATED=0

say "installed: $BINDIR/terminal-fenster -> $PREFIX/bin/terminal-fenster"
case ":$PATH:" in
  *":$BINDIR:"*) ;;
  *) warn "$BINDIR is not on PATH; add: export PATH=\"$BINDIR:\$PATH\"" ;;
esac
say 'run terminal-fenster doctor in your graphics-capable terminal before browsing'
