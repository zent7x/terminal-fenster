#!/usr/bin/env bash
set -euo pipefail

PROJECT_DIR="$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd -P)"
OUTPUT_DIR="$PROJECT_DIR/dist"
CACHE_DIR="${TERMINAL_FENSTER_RELEASE_CACHE:-$PROJECT_DIR/.release-cache}"
FORCE=0
EXPECTED_TARGET=""
WORK_DIR=""

usage() {
  cat <<'EOF'
usage: tools/package-release.sh [--output DIR] [--expect-target TARGET] [--force]

Builds and verifies a prebuilt release for the current macOS/Linux host. Downloads the exact
Electron and free-codecs FFmpeg archives pinned in packaging/engine-lock.json, verifies their
SHA-256 hashes, and emits a .tar.gz plus matching .sha256 file. --expect-target makes CI fail
closed if the hosted runner architecture does not match its matrix entry.
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --output)
      [ "$#" -ge 2 ] || { printf 'package release: --output needs a value\n' >&2; exit 2; }
      OUTPUT_DIR="$2"
      shift 2
      ;;
    --expect-target)
      [ "$#" -ge 2 ] || { printf 'package release: --expect-target needs a value\n' >&2; exit 2; }
      EXPECTED_TARGET="$2"
      shift 2
      ;;
    --force) FORCE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) printf 'package release: unknown option: %s\n' "$1" >&2; usage >&2; exit 2 ;;
  esac
done

cleanup() {
  status=$?
  if [ -n "$WORK_DIR" ] && [ -d "$WORK_DIR" ]; then
    rm -rf -- "$WORK_DIR"
  fi
  exit "$status"
}
trap cleanup EXIT

need() { command -v "$1" >/dev/null 2>&1 || { printf 'package release: missing %s\n' "$1" >&2; exit 1; }; }
for command_name in cargo node curl unzip tar awk install; do need "$command_name"; done

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) TARGET=darwin-arm64 ;;
  Darwin-x86_64) TARGET=darwin-x64 ;;
  Linux-aarch64|Linux-arm64) TARGET=linux-arm64 ;;
  Linux-x86_64|Linux-amd64) TARGET=linux-x64 ;;
  *) printf 'package release: unsupported host %s/%s\n' "$(uname -s)" "$(uname -m)" >&2; exit 1 ;;
esac

if [ -n "$EXPECTED_TARGET" ] && [ "$TARGET" != "$EXPECTED_TARGET" ]; then
  printf 'package release: expected target %s but this host is %s\n' \
    "$EXPECTED_TARGET" "$TARGET" >&2
  exit 1
fi

IFS=$'\t' read -r ELECTRON_VERSION CHROMIUM_VERSION CODEC_POLICY RELEASE_BASE \
  ELECTRON_FILE ELECTRON_SHA FFMPEG_FILE FFMPEG_SHA FFMPEG_MEMBER FFMPEG_DEST RUNTIME_RELATIVE \
  < <(node "$PROJECT_DIR/packaging/read-engine-lock.mjs" "$TARGET")

[ "$CODEC_POLICY" = 'free-codecs-only' ] || {
  printf 'package release: refusing unapproved codec policy %s\n' "$CODEC_POLICY" >&2
  exit 1
}

VERSION=$(awk -F'"' '/^version = "/ { print $2; exit }' "$PROJECT_DIR/Cargo.toml")
[ -n "$VERSION" ] || { printf 'package release: workspace version is missing\n' >&2; exit 1; }
NAME="terminal-fenster-$VERSION-$TARGET"
mkdir -p "$OUTPUT_DIR" "$CACHE_DIR"
OUTPUT_DIR="$(CDPATH='' cd -- "$OUTPUT_DIR" && pwd -P)"
CACHE_DIR="$(CDPATH='' cd -- "$CACHE_DIR" && pwd -P)"
ARCHIVE="$OUTPUT_DIR/$NAME.tar.gz"
CHECKSUM="$ARCHIVE.sha256"
if [ -e "$ARCHIVE" ] || [ -e "$CHECKSUM" ]; then
  [ "$FORCE" -eq 1 ] || {
    printf 'package release: output already exists (pass --force): %s\n' "$ARCHIVE" >&2
    exit 1
  }
  rm -f -- "$ARCHIVE" "$CHECKSUM"
fi

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

download_verified() {
  file=$1
  expected=$2
  destination="$CACHE_DIR/$file"
  if [ -f "$destination" ] && [ "$(sha256_file "$destination")" = "$expected" ]; then
    printf '==> cached and verified: %s\n' "$file" >&2
  else
    temporary="$destination.part.$$"
    printf '==> downloading and verifying: %s\n' "$file" >&2
    curl --fail --location --proto '=https' --tlsv1.2 \
      --output "$temporary" "$RELEASE_BASE/$file"
    actual=$(sha256_file "$temporary")
    if [ "$actual" != "$expected" ]; then
      rm -f -- "$temporary"
      printf 'package release: checksum mismatch for %s\nexpected %s\nactual   %s\n' \
        "$file" "$expected" "$actual" >&2
      exit 1
    fi
    mv -f -- "$temporary" "$destination"
  fi
  printf '%s\n' "$destination"
}

cd "$PROJECT_DIR"
printf '==> building Terminal-Fenster %s for %s\n' "$VERSION" "$TARGET"
cargo build --release --locked
BINARY="$PROJECT_DIR/target/release/terminal-fenster"
[ -x "$BINARY" ] || { printf 'package release: release binary is missing\n' >&2; exit 1; }
[ "$("$BINARY" version)" = "terminal-fenster $VERSION" ] || {
  printf 'package release: binary version does not match workspace version\n' >&2
  exit 1
}

ELECTRON_ZIP=$(download_verified "$ELECTRON_FILE" "$ELECTRON_SHA")
FFMPEG_ZIP=$(download_verified "$FFMPEG_FILE" "$FFMPEG_SHA")

WORK_DIR=$(mktemp -d "$OUTPUT_DIR/.terminal-fenster-package.XXXXXX")
RELEASE_ROOT="$WORK_DIR/$NAME"
DIST="$RELEASE_ROOT/engine/node_modules/electron/dist"
mkdir -p "$RELEASE_ROOT/bin" "$RELEASE_ROOT/engine/node_modules/.bin" \
  "$RELEASE_ROOT/engine/node_modules/electron" "$DIST" "$RELEASE_ROOT/licenses/rust"

printf '==> extracting the pinned Electron runtime\n'
unzip -q "$ELECTRON_ZIP" -d "$DIST"
[ -x "$DIST/$RUNTIME_RELATIVE" ] || {
  printf 'package release: extracted Electron runtime is missing: %s\n' "$RUNTIME_RELATIVE" >&2
  exit 1
}

FFMPEG_TEMP="$WORK_DIR/free-ffmpeg"
mkdir -p "$FFMPEG_TEMP"
unzip -q "$FFMPEG_ZIP" -d "$FFMPEG_TEMP"
[ -f "$FFMPEG_TEMP/$FFMPEG_MEMBER" ] || {
  printf 'package release: free-codecs archive does not contain %s\n' "$FFMPEG_MEMBER" >&2
  exit 1
}
install -m 0755 "$FFMPEG_TEMP/$FFMPEG_MEMBER" "$DIST/$FFMPEG_DEST"
FFMPEG_LIBRARY_SHA=$(sha256_file "$DIST/$FFMPEG_DEST")

if [ "$(uname -s)" = Darwin ]; then
  need codesign
  # Replacing FFmpeg intentionally invalidates Electron's upstream ad-hoc seal. Restore a
  # self-consistent local signature; a public macOS release must still be Developer-ID signed
  # and notarized after this step.
  codesign --force --deep --sign - "$DIST/Electron.app" >/dev/null
  codesign --verify --deep --strict "$DIST/Electron.app"
  SIGNING=adhoc
else
  SIGNING=unsigned
fi

install -m 0755 "$BINARY" "$RELEASE_ROOT/bin/terminal-fenster"
cp -R "$PROJECT_DIR/apps/engine/src" "$RELEASE_ROOT/engine/src"
cp "$PROJECT_DIR/apps/engine/package.json" "$PROJECT_DIR/apps/engine/package-lock.json" \
  "$RELEASE_ROOT/engine/"
printf '%s' "$RUNTIME_RELATIVE" > "$RELEASE_ROOT/engine/node_modules/electron/path.txt"
install -m 0755 "$PROJECT_DIR/packaging/electron-launcher.sh" \
  "$RELEASE_ROOT/engine/node_modules/.bin/electron"

cp "$PROJECT_DIR/LICENSE-MIT" "$RELEASE_ROOT/"
cp "$PROJECT_DIR/NOTICE.md" "$RELEASE_ROOT/THIRD-PARTY-NOTICES.md"
cp "$PROJECT_DIR/README.md" "$PROJECT_DIR/RELEASE.md" "$RELEASE_ROOT/"
install -m 0755 "$PROJECT_DIR/packaging/install-prebuilt.sh" "$RELEASE_ROOT/install.sh"
install -m 0755 "$PROJECT_DIR/uninstall.sh" "$RELEASE_ROOT/uninstall.sh"
install -m 0755 "$PROJECT_DIR/packaging/verify-release.sh" "$RELEASE_ROOT/verify.sh"
node "$PROJECT_DIR/packaging/copy-cargo-licenses.mjs" "$RELEASE_ROOT/licenses/rust"

cat > "$RELEASE_ROOT/.terminal-fenster-release" <<EOF
version=$VERSION
target=$TARGET
electron=$ELECTRON_VERSION
chromium=$CHROMIUM_VERSION
codec_policy=$CODEC_POLICY
ffmpeg_archive_sha256=$FFMPEG_SHA
ffmpeg_library_sha256=$FFMPEG_LIBRARY_SHA
signing=$SIGNING
EOF

printf '==> generating the payload manifest\n'
MANIFEST_TEMP="$WORK_DIR/MANIFEST.sha256"
(
  cd "$RELEASE_ROOT"
  find . -type f ! -name MANIFEST.sha256 -print | LC_ALL=C sort | while IFS= read -r file; do
    printf '%s  %s\n' "$(sha256_file "$file")" "$file"
  done
) > "$MANIFEST_TEMP"
mv -- "$MANIFEST_TEMP" "$RELEASE_ROOT/MANIFEST.sha256"

"$RELEASE_ROOT/verify.sh"

printf '==> creating %s\n' "$(basename "$ARCHIVE")"
(
  cd "$WORK_DIR"
  COPYFILE_DISABLE=1 tar -czf "$ARCHIVE" "$NAME"
)
printf '%s  %s\n' "$(sha256_file "$ARCHIVE")" "$(basename "$ARCHIVE")" > "$CHECKSUM"

EXTRACTED="$WORK_DIR/extracted"
mkdir -p "$EXTRACTED"
tar -xzf "$ARCHIVE" -C "$EXTRACTED"
"$EXTRACTED/$NAME/verify.sh"

printf '==> exercising an isolated install and uninstall\n'
INSTALL_HOME="$WORK_DIR/install-home"
INSTALL_PREFIX="$WORK_DIR/install-prefix"
INSTALL_BINDIR="$WORK_DIR/install-bin"
mkdir -p "$INSTALL_HOME"
HOME="$INSTALL_HOME" TERMINAL_FENSTER_PREFIX="$INSTALL_PREFIX" TERMINAL_FENSTER_BINDIR="$INSTALL_BINDIR" \
  "$EXTRACTED/$NAME/install.sh" >/dev/null
[ "$($INSTALL_BINDIR/terminal-fenster version)" = "terminal-fenster $VERSION" ] || {
  printf 'package release: installed binary version smoke test failed\n' >&2
  exit 1
}
[ "$($INSTALL_PREFIX/engine/node_modules/.bin/electron --version)" = "v$ELECTRON_VERSION" ] || {
  printf 'package release: installed Electron runtime smoke test failed\n' >&2
  exit 1
}
HOME="$INSTALL_HOME" TERMINAL_FENSTER_PREFIX="$INSTALL_PREFIX" TERMINAL_FENSTER_BINDIR="$INSTALL_BINDIR" \
  "$INSTALL_PREFIX/uninstall.sh" >/dev/null 2>&1
[ ! -e "$INSTALL_PREFIX" ] && [ ! -e "$INSTALL_BINDIR/terminal-fenster" ] || {
  printf 'package release: isolated uninstall left installed paths behind\n' >&2
  exit 1
}

printf '\nrelease archive: %s\n' "$ARCHIVE"
printf 'checksum:        %s\n' "$CHECKSUM"
printf 'size:            %s bytes\n' "$(wc -c < "$ARCHIVE" | tr -d ' ')"
printf 'codec policy:    %s\n' "$CODEC_POLICY"
printf 'signing:         %s (Developer ID + notarization remain a release gate on macOS)\n' "$SIGNING"
