#!/bin/sh
set -eu

PROJECT_DIR=$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd -P)
BINARY="$PROJECT_DIR/target/release/terminal-fenster"
[ -x "$BINARY" ] || {
  printf 'package-layout test: build target/release/terminal-fenster first\n' >&2
  exit 1
}

for target_name in darwin-arm64 darwin-x64 linux-arm64 linux-x64; do
  node "$PROJECT_DIR/packaging/read-engine-lock.mjs" "$target_name" >/dev/null
done

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64)
    TARGET=darwin-arm64
    RUNTIME_RELATIVE='Electron.app/Contents/MacOS/Electron'
    FFMPEG_RELATIVE='Electron.app/Contents/Frameworks/Electron Framework.framework/Versions/A/Libraries/libffmpeg.dylib'
    ;;
  Darwin-x86_64)
    TARGET=darwin-x64
    RUNTIME_RELATIVE='Electron.app/Contents/MacOS/Electron'
    FFMPEG_RELATIVE='Electron.app/Contents/Frameworks/Electron Framework.framework/Versions/A/Libraries/libffmpeg.dylib'
    ;;
  Linux-aarch64|Linux-arm64)
    TARGET=linux-arm64
    RUNTIME_RELATIVE=electron
    FFMPEG_RELATIVE=libffmpeg.so
    ;;
  Linux-x86_64|Linux-amd64)
    TARGET=linux-x64
    RUNTIME_RELATIVE=electron
    FFMPEG_RELATIVE=libffmpeg.so
    ;;
  *) printf 'package-layout test: unsupported host\n' >&2; exit 1 ;;
esac

lock_line=$(node "$PROJECT_DIR/packaging/read-engine-lock.mjs" "$TARGET")
tab=$(printf '\t')
ELECTRON_VERSION=$(printf '%s\n' "$lock_line" | awk -F "$tab" '{print $1}')
CHROMIUM_VERSION=$(printf '%s\n' "$lock_line" | awk -F "$tab" '{print $2}')
VERSION=$(awk -F'"' '/^version = "/ { print $2; exit }' "$PROJECT_DIR/Cargo.toml")
[ -n "$VERSION" ] && [ -n "$ELECTRON_VERSION" ] && [ -n "$CHROMIUM_VERSION" ] || exit 1

TEST_DIR=$(mktemp -d "${TMPDIR:-/tmp}/terminal-fenster-package-layout.XXXXXX")
TEST_DIR=$(CDPATH='' cd -- "$TEST_DIR" && pwd -P)
trap 'rm -rf "$TEST_DIR"' EXIT INT TERM
RELEASE_ROOT="$TEST_DIR/release"
DIST="$RELEASE_ROOT/engine/node_modules/electron/dist"
mkdir -p "$RELEASE_ROOT/bin" "$RELEASE_ROOT/engine/node_modules/.bin" \
  "$DIST/$(dirname "$RUNTIME_RELATIVE")" "$DIST/$(dirname "$FFMPEG_RELATIVE")" \
  "$RELEASE_ROOT/engine/src" "$RELEASE_ROOT/licenses/rust"

cp "$BINARY" "$RELEASE_ROOT/bin/terminal-fenster"
cp "$PROJECT_DIR/apps/engine/src/main.js" "$PROJECT_DIR/apps/engine/src/frame-capture.js" \
  "$PROJECT_DIR/apps/engine/src/frame-pipeline.js" \
  "$PROJECT_DIR/apps/engine/src/security-policy.js" "$PROJECT_DIR/apps/engine/src/tabs.js" \
  "$RELEASE_ROOT/engine/src/"
cp "$PROJECT_DIR/packaging/electron-launcher.sh" \
  "$RELEASE_ROOT/engine/node_modules/.bin/electron"
chmod 0755 "$RELEASE_ROOT/engine/node_modules/.bin/electron"
printf '%s' "$RUNTIME_RELATIVE" > "$RELEASE_ROOT/engine/node_modules/electron/path.txt"
printf '#!/bin/sh\nprintf "v%s\\n"\n' "$ELECTRON_VERSION" > "$DIST/$RUNTIME_RELATIVE"
chmod 0755 "$DIST/$RUNTIME_RELATIVE"
printf 'free-codecs fixture\n' > "$DIST/$FFMPEG_RELATIVE"
: > "$DIST/LICENSE"
: > "$DIST/LICENSES.chromium.html"
: > "$RELEASE_ROOT/licenses/rust/INDEX.json"
cp "$PROJECT_DIR/LICENSE-MIT" "$RELEASE_ROOT/"
cp "$PROJECT_DIR/NOTICE.md" "$RELEASE_ROOT/THIRD-PARTY-NOTICES.md"
cp "$PROJECT_DIR/packaging/verify-release.sh" "$RELEASE_ROOT/verify.sh"
cp "$PROJECT_DIR/packaging/install-prebuilt.sh" "$RELEASE_ROOT/install.sh"
cp "$PROJECT_DIR/uninstall.sh" "$RELEASE_ROOT/uninstall.sh"
chmod 0755 "$RELEASE_ROOT/verify.sh" "$RELEASE_ROOT/install.sh" "$RELEASE_ROOT/uninstall.sh"

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

ffmpeg_sha=$(sha256_file "$DIST/$FFMPEG_RELATIVE")
cat > "$RELEASE_ROOT/.terminal-fenster-release" <<EOF
version=$VERSION
target=$TARGET
electron=$ELECTRON_VERSION
chromium=$CHROMIUM_VERSION
codec_policy=free-codecs-only
ffmpeg_archive_sha256=fixture
ffmpeg_library_sha256=$ffmpeg_sha
signing=fixture
EOF

generate_manifest() {
  manifest_tmp="$TEST_DIR/MANIFEST.sha256"
  (
    cd "$RELEASE_ROOT"
    find . -type f ! -name MANIFEST.sha256 -print | LC_ALL=C sort | while IFS= read -r file; do
      printf '%s  %s\n' "$(sha256_file "$file")" "$file"
    done
  ) > "$manifest_tmp"
  mv -- "$manifest_tmp" "$RELEASE_ROOT/MANIFEST.sha256"
}

generate_manifest
"$RELEASE_ROOT/verify.sh" >/dev/null

# A payload changed after manifest generation must be rejected before installation.
printf '\ncorruption fixture\n' >> "$RELEASE_ROOT/engine/src/main.js"
if "$RELEASE_ROOT/verify.sh" >/dev/null 2>&1; then
  printf 'package-layout test: corrupted payload passed verification\n' >&2
  exit 1
fi
cp "$PROJECT_DIR/apps/engine/src/main.js" "$RELEASE_ROOT/engine/src/main.js"
generate_manifest

PREFIX="$TEST_DIR/prefix"
BINDIR="$TEST_DIR/bin"
TEST_HOME="$TEST_DIR/home"
mkdir -p "$TEST_HOME"
HOME="$TEST_HOME" TERMINAL_FENSTER_PREFIX="$PREFIX" TERMINAL_FENSTER_BINDIR="$BINDIR" \
  "$RELEASE_ROOT/install.sh" >/dev/null 2>&1
[ -x "$BINDIR/terminal-fenster" ] || {
  printf 'package-layout test: prebuilt installer did not create the PATH entry\n' >&2
  exit 1
}
[ "$("$BINDIR/terminal-fenster" version)" = "terminal-fenster $VERSION" ] || exit 1

set +e
doctor_output=$("$BINDIR/terminal-fenster" doctor </dev/null 2>&1)
doctor_status=$?
set -e
[ "$doctor_status" -eq 0 ] || exit 1
case "$doctor_output" in
  *"HEADLESS OK"*) ;;
  *) printf 'package-layout test: doctor should report headless OK without a tty\n%s\n' \
       "$doctor_output" >&2; exit 1 ;;
esac
case "$doctor_output" in
  *"$PREFIX/engine/node_modules/.bin/electron"*) ;;
  *) printf 'package-layout test: installed archive could not find its sibling engine\n%s\n' \
       "$doctor_output" >&2; exit 1 ;;
esac

HOME="$TEST_HOME" TERMINAL_FENSTER_PREFIX="$PREFIX" TERMINAL_FENSTER_BINDIR="$BINDIR" \
  "$PREFIX/uninstall.sh" >/dev/null 2>&1
[ ! -e "$PREFIX" ] && [ ! -e "$BINDIR/terminal-fenster" ] || {
  printf 'package-layout test: installed archive did not uninstall cleanly\n' >&2
  exit 1
}

printf 'package-layout test: PASS\n'
