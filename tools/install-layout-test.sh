#!/bin/sh
set -eu

PROJECT_DIR=$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd)
BINARY="$PROJECT_DIR/target/release/terminal-fenster"
[ -x "$BINARY" ] || {
  printf 'install-layout test: build target/release/terminal-fenster first\n' >&2
  exit 1
}

TEST_DIR=$(mktemp -d "${TMPDIR:-/tmp}/terminal-fenster-layout.XXXXXX")
TEST_DIR=$(CDPATH='' cd -- "$TEST_DIR" && pwd -P)
trap 'rm -rf "$TEST_DIR"' EXIT INT TERM
ROOT="$TEST_DIR/root"
BINDIR="$TEST_DIR/path-bin"
ENGINE="$ROOT/engine"
RUNTIME_RELATIVE="fake-runtime/electron"

mkdir -p "$ROOT/bin" "$BINDIR" "$ENGINE/node_modules/.bin" \
  "$ENGINE/node_modules/electron/dist/fake-runtime"
cp "$BINARY" "$ROOT/bin/terminal-fenster"
printf '#!/bin/sh\nexit 0\n' > "$ENGINE/node_modules/.bin/electron"
chmod +x "$ENGINE/node_modules/.bin/electron"
printf '%s\n' "$RUNTIME_RELATIVE" > "$ENGINE/node_modules/electron/path.txt"
: > "$ENGINE/node_modules/electron/dist/$RUNTIME_RELATIVE"
ln -s "$ROOT/bin/terminal-fenster" "$BINDIR/terminal-fenster"

"$BINDIR/terminal-fenster" version >/dev/null
set +e
output=$("$BINDIR/terminal-fenster" doctor </dev/null 2>&1)
status=$?
set -e
[ "$status" -eq 0 ] || {
  printf 'install-layout test: non-TTY doctor with ready engine should exit 0, got %s\n' "$status" >&2
  exit 1
}
case "$output" in
  *"HEADLESS OK"*) ;;
  *) printf 'install-layout test: non-TTY doctor missing headless verdict\n%s\n' "$output" >&2; exit 1 ;;
esac
case "$output" in
  *"engine              $ENGINE/node_modules/.bin/electron"*) ;;
  *) printf 'install-layout test: symlinked command did not find sibling engine\n%s\n' "$output" >&2; exit 1 ;;
esac

rm "$ENGINE/node_modules/electron/dist/$RUNTIME_RELATIVE"
set +e
output=$("$BINDIR/terminal-fenster" doctor </dev/null 2>&1)
status=$?
set -e
[ "$status" -eq 1 ] || {
  printf 'install-layout test: broken engine should exit 1, got %s\n' "$status" >&2
  exit 1
}
case "$output" in
  *"engine              NOT READY"*) ;;
  *) printf 'install-layout test: half-install was not rejected\n%s\n' "$output" >&2; exit 1 ;;
esac

# Exercise the real uninstaller against an isolated HOME: default preserves profiles, the
# explicit purge removes only Terminal-Fenster's app-specific directory.
UNINSTALL_ROOT="$TEST_DIR/uninstall-root"
UNINSTALL_BIN="$TEST_DIR/uninstall-bin"
TEST_HOME="$TEST_DIR/home"
TEST_XDG_CONFIG_HOME="$TEST_HOME/.config"
case "$(uname -s)" in
  Darwin) TEST_PROFILE="$TEST_HOME/Library/Application Support/terminal-fenster" ;;
  Linux) TEST_PROFILE="$TEST_XDG_CONFIG_HOME/terminal-fenster" ;;
  *) printf 'install-layout test: unsupported test OS\n' >&2; exit 1 ;;
esac
mkdir -p "$UNINSTALL_ROOT/bin" "$UNINSTALL_ROOT/engine" "$UNINSTALL_BIN" "$TEST_PROFILE"
: > "$UNINSTALL_ROOT/.terminal-fenster-install"
ln -s "$UNINSTALL_ROOT/bin/terminal-fenster" "$UNINSTALL_BIN/terminal-fenster"
HOME="$TEST_HOME" XDG_CONFIG_HOME="$TEST_XDG_CONFIG_HOME" \
  TERMINAL_FENSTER_PREFIX="$UNINSTALL_ROOT" TERMINAL_FENSTER_BINDIR="$UNINSTALL_BIN" \
  "$PROJECT_DIR/uninstall.sh" >/dev/null 2>&1
[ ! -e "$UNINSTALL_ROOT" ] && [ ! -e "$UNINSTALL_BIN/terminal-fenster" ] && [ -d "$TEST_PROFILE" ] || {
  printf 'install-layout test: default uninstall did not preserve exactly the profile\n' >&2
  exit 1
}

mkdir -p "$UNINSTALL_ROOT/bin" "$UNINSTALL_ROOT/engine" "$TEST_PROFILE"
: > "$UNINSTALL_ROOT/.terminal-fenster-install"
ln -s "$UNINSTALL_ROOT/bin/terminal-fenster" "$UNINSTALL_BIN/terminal-fenster"
HOME="$TEST_HOME" XDG_CONFIG_HOME="$TEST_XDG_CONFIG_HOME" \
  TERMINAL_FENSTER_PREFIX="$UNINSTALL_ROOT" TERMINAL_FENSTER_BINDIR="$UNINSTALL_BIN" \
  "$PROJECT_DIR/uninstall.sh" --purge-profile >/dev/null 2>&1
[ ! -e "$UNINSTALL_ROOT" ] && [ ! -e "$UNINSTALL_BIN/terminal-fenster" ] && [ ! -e "$TEST_PROFILE" ] || {
  printf 'install-layout test: purge uninstall left scoped data behind\n' >&2
  exit 1
}

printf 'install-layout test: PASS\n'
