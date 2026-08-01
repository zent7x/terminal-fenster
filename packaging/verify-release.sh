#!/bin/sh
set -eu

release_root=$(CDPATH='' cd -- "$(dirname "$0")" && pwd -P)
cd "$release_root"

die() {
  printf 'release verification: %s\n' "$*" >&2
  exit 1
}

field() {
  awk -F= -v key="$1" '$1 == key { sub(/^[^=]*=/, ""); print; found=1; exit } END { if (!found) exit 1 }' \
    .terminal-fenster-release
}

[ -f .terminal-fenster-release ] || die '.terminal-fenster-release is missing'
[ -f MANIFEST.sha256 ] || die 'MANIFEST.sha256 is missing'

if command -v sha256sum >/dev/null 2>&1; then
  sha256sum -c MANIFEST.sha256 >/dev/null || die 'payload checksum verification failed'
elif command -v shasum >/dev/null 2>&1; then
  shasum -a 256 -c MANIFEST.sha256 >/dev/null || die 'payload checksum verification failed'
else
  die 'sha256sum or shasum is required to verify this release'
fi

version=$(field version) || die 'release version is missing'
target=$(field target) || die 'release target is missing'
electron=$(field electron) || die 'Electron version is missing'
codec_policy=$(field codec_policy) || die 'codec policy is missing'
ffmpeg_sha=$(field ffmpeg_library_sha256) || die 'FFmpeg library checksum is missing'
runtime_relative=$(tr -d '\r\n' < engine/node_modules/electron/path.txt)

[ -x bin/terminal-fenster ] || die 'bin/terminal-fenster is missing or not executable'
[ -x engine/node_modules/.bin/electron ] || die 'Electron launcher is missing'
[ -f engine/src/main.js ] || die 'engine/src/main.js is missing'
[ -f engine/src/frame-capture.js ] || die 'engine/src/frame-capture.js is missing'
[ -f engine/src/frame-pipeline.js ] || die 'engine/src/frame-pipeline.js is missing'
[ -f engine/src/security-policy.js ] || die 'engine/src/security-policy.js is missing'
[ -f engine/src/tabs.js ] || die 'engine/src/tabs.js is missing'
[ -f engine/node_modules/electron/dist/LICENSE ] || die 'Electron LICENSE is missing'
[ -f engine/node_modules/electron/dist/LICENSES.chromium.html ] \
  || die 'Chromium third-party notices are missing'
[ -f LICENSE-MIT ] || die 'Terminal-Fenster MIT license is missing'
[ -f THIRD-PARTY-NOTICES.md ] || die 'THIRD-PARTY-NOTICES.md is missing'
[ -f licenses/rust/INDEX.json ] || die 'Cargo dependency license index is missing'
[ "$codec_policy" = 'free-codecs-only' ] || die "unsupported codec policy: $codec_policy"
[ -n "$runtime_relative" ] || die 'Electron runtime path is empty'
[ -x "engine/node_modules/electron/dist/$runtime_relative" ] \
  || die 'Electron runtime is missing or not executable'

case "$target" in
  darwin-*) ffmpeg_path='engine/node_modules/electron/dist/Electron.app/Contents/Frameworks/Electron Framework.framework/Versions/A/Libraries/libffmpeg.dylib' ;;
  linux-*) ffmpeg_path='engine/node_modules/electron/dist/libffmpeg.so' ;;
  *) die "unsupported target marker: $target" ;;
esac
[ -f "$ffmpeg_path" ] || die "free-codecs FFmpeg library is missing: $ffmpeg_path"
if command -v sha256sum >/dev/null 2>&1; then
  actual_ffmpeg_sha=$(sha256sum "$ffmpeg_path" | awk '{print $1}')
else
  actual_ffmpeg_sha=$(shasum -a 256 "$ffmpeg_path" | awk '{print $1}')
fi
[ "$actual_ffmpeg_sha" = "$ffmpeg_sha" ] || die 'free-codecs FFmpeg checksum changed'

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) host_target=darwin-arm64 ;;
  Darwin-x86_64) host_target=darwin-x64 ;;
  Linux-aarch64|Linux-arm64) host_target=linux-arm64 ;;
  Linux-x86_64|Linux-amd64) host_target=linux-x64 ;;
  *) host_target=unsupported ;;
esac
[ "$target" = "$host_target" ] \
  || die "archive target $target does not match this host ($host_target)"

[ "$(bin/terminal-fenster version)" = "terminal-fenster $version" ] \
  || die 'Terminal-Fenster binary version does not match the release marker'
[ "$(engine/node_modules/.bin/electron --version)" = "v$electron" ] \
  || die 'Electron runtime version does not match the release marker'

printf 'release verification: PASS (%s, Terminal-Fenster %s, Electron %s, %s)\n' \
  "$target" "$version" "$electron" "$codec_policy"
