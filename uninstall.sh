#!/usr/bin/env bash
# Remove a Terminal-Fenster installation created by install.sh.
# Browser profiles are preserved unless --purge-profile is explicitly supplied.
set -euo pipefail

PREFIX="${TERMINAL_FENSTER_PREFIX:-$HOME/.local/share/terminal-fenster}"
BINDIR="${TERMINAL_FENSTER_BINDIR:-$HOME/.local/bin}"
PURGE_PROFILE=0

say()  { printf '\033[1;32m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

case "${1:-}" in
  "") ;;
  --purge-profile) PURGE_PROFILE=1 ;;
  -h|--help)
    printf '%s\n' 'usage: uninstall.sh [--purge-profile]'
    printf '%s\n' 'Removes the dedicated install prefix; browser profiles are preserved by default.'
    exit 0
    ;;
  *) die "unknown option: $1" ;;
esac

case "$PREFIX" in /*) ;; *) PREFIX="$PWD/$PREFIX" ;; esac
case "$BINDIR" in /*) ;; *) BINDIR="$PWD/$BINDIR" ;; esac
case "$PREFIX" in ""|/|"$HOME") die "refusing unsafe TERMINAL_FENSTER_PREFIX=$PREFIX" ;; esac
[ ! -L "$PREFIX" ] || die "refusing symlink install prefix $PREFIX"

if [ -e "$PREFIX" ]; then
  if [ ! -f "$PREFIX/.terminal-fenster-install" ]; then
    die "refusing to delete unmarked prefix $PREFIX; it was not created by the hardened installer"
  fi
  rm -rf -- "$PREFIX"
  say "removed install prefix $PREFIX (not recoverable)"
else
  warn "install prefix does not exist: $PREFIX"
fi

link="$BINDIR/terminal-fenster"
if [ -L "$link" ]; then
  target="$(readlink "$link")"
  if [ "$target" = "$PREFIX/bin/terminal-fenster" ]; then
    rm -- "$link"
    say "removed PATH symlink $link"
  else
    warn "left unrelated symlink in place: $link -> $target"
  fi
elif [ -e "$link" ]; then
  warn "left non-symlink path in place: $link"
fi

if [ "$PURGE_PROFILE" -eq 1 ]; then
  case "$(uname -s)" in
    Darwin) profile_root="$HOME/Library/Application Support/terminal-fenster" ;;
    Linux) profile_root="${XDG_CONFIG_HOME:-$HOME/.config}/terminal-fenster" ;;
    *) die "cannot determine Terminal-Fenster profile path on this OS" ;;
  esac
  case "$profile_root" in ""|/|"$HOME") die "refusing unsafe profile path $profile_root" ;; esac
  if [ -e "$profile_root" ]; then
    rm -rf -- "$profile_root"
    say "purged browser profiles $profile_root (not recoverable)"
  else
    say "no browser profile directory found at $profile_root"
  fi
else
  warn "browser profiles were preserved; rerun with --purge-profile to delete them"
fi

warn "the shared Electron download cache was not removed because other apps may use it"
