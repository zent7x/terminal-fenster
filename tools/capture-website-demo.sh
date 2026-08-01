#!/usr/bin/env bash
# Capture a short, real Terminal-Fenster session for the README and landing page.
#
# The macOS path records only the temporary Ghostty window—never the whole desktop—then
# types into a local fixture, navigates through the real omnibox, and closes cleanly.
#
# Usage:
#   cargo build --release
#   tools/capture-website-demo.sh
#
# Outputs:
#   website/public/assets/demo.gif
#   website/public/assets/demo.png
set -euo pipefail

ROOT=$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd)
OUT_GIF="$ROOT/website/public/assets/demo.gif"
OUT_PNG="$ROOT/website/public/assets/demo.png"
BIN="${TERMINAL_FENSTER_BIN:-$ROOT/target/release/terminal-fenster}"
CAPTURE_DIR=$(mktemp -d "${TMPDIR:-/tmp}/terminal-fenster-demo.XXXXXX")
SESSION_LOG="$CAPTURE_DIR/session.log"
PROFILE="website-demo-$$"

cleanup() {
  if [[ -n "${GHOSTTY_PID:-}" ]] && ps -p "$GHOSTTY_PID" >/dev/null 2>&1; then
    osascript \
      -e 'on run argv' \
      -e 'tell application "System Events"' \
      -e 'set frontmost of first process whose unix id is (item 1 of argv as integer) to true' \
      -e 'key code 36' \
      -e 'end tell' \
      -e 'end run' "$GHOSTTY_PID" >/dev/null 2>&1 || true
  fi
  trash "$CAPTURE_DIR" >/dev/null 2>&1 || true
}
trap cleanup EXIT

[[ "$(uname -s)" == "Darwin" ]] || {
  echo "capture-website-demo: the window-only recorder currently requires macOS" >&2
  exit 2
}
[[ -x "$BIN" ]] || {
  echo "capture-website-demo: missing $BIN — run cargo build --release" >&2
  exit 2
}
[[ -d /Applications/Ghostty.app ]] || {
  echo "capture-website-demo: Ghostty.app is not installed in /Applications" >&2
  exit 2
}
for command_name in ffmpeg screencapture swift osascript trash; do
  command -v "$command_name" >/dev/null 2>&1 || {
    echo "capture-website-demo: required command not found: $command_name" >&2
    exit 2
  }
done

mkdir -p "$(dirname "$OUT_GIF")"
START_URL="file://$ROOT/tests/fixtures/text-input.html"

echo "==> Launching a disposable Ghostty capture window"
open -na Ghostty.app --args \
  --window-save-state=never \
  -e env \
  "TERMINAL_FENSTER_LOG=$SESSION_LOG" \
  "$BIN" open "$START_URL" --profile "$PROFILE"

for _ in {1..60}; do
  if [[ -f "$SESSION_LOG" ]] && rg -q 'first-frame' "$SESSION_LOG"; then
    break
  fi
  sleep 0.25
done
[[ -f "$SESSION_LOG" ]] && rg -q 'first-frame' "$SESSION_LOG" || {
  echo "capture-website-demo: browser did not paint within 15 seconds" >&2
  exit 1
}

GHOSTTY_PID=$(ps -axo pid=,command= | awk -v marker="$PROFILE" \
  'index($0, "Ghostty.app/Contents/MacOS/ghostty") && index($0, marker) { print $1 }' | tail -n 1)
[[ -n "$GHOSTTY_PID" ]] || {
  echo "capture-website-demo: could not locate the disposable Ghostty process" >&2
  exit 1
}

WINDOW_ID=$(swift -e '
import CoreGraphics
let target = Int(CommandLine.arguments[1])!
let windows = (CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]]) ?? []
var best = (id: 0, area: 0)
for item in windows {
  let pid = item[kCGWindowOwnerPID as String] as? Int ?? -1
  let layer = item[kCGWindowLayer as String] as? Int ?? -1
  guard pid == target && layer == 0 else { continue }
  let id = item[kCGWindowNumber as String] as? Int ?? 0
  let bounds = item[kCGWindowBounds as String] as? [String: Any] ?? [:]
  let width = bounds["Width"] as? Int ?? 0
  let height = bounds["Height"] as? Int ?? 0
  if width * height > best.area { best = (id, width * height) }
}
print(best.id)
' "$GHOSTTY_PID")
[[ "$WINDOW_ID" =~ ^[1-9][0-9]*$ ]] || {
  echo "capture-website-demo: could not locate the Ghostty window" >&2
  exit 1
}

echo "==> Recording an 8-second real terminal interaction"
(
  for frame in $(seq 0 63); do
    printf -v number '%03d' "$frame"
    screencapture -x -l "$WINDOW_ID" "$CAPTURE_DIR/frame-$number.png"
    sleep 0.125
  done
) &
CAPTURE_PID=$!

osascript \
  -e 'on run argv' \
  -e 'tell application "System Events"' \
  -e 'set frontmost of first process whose unix id is (item 1 of argv as integer) to true' \
  -e 'delay 0.8' \
  -e 'key code 48' \
  -e 'delay 0.2' \
  -e 'keystroke "terminal-fenster"' \
  -e 'delay 1.2' \
  -e 'keystroke "l" using control down' \
  -e 'delay 0.25' \
  -e 'keystroke "u" using control down' \
  -e 'delay 0.15' \
  -e 'keystroke "https://news.ycombinator.com"' \
  -e 'key code 36' \
  -e 'delay 3.0' \
  -e 'key code 121' \
  -e 'end tell' \
  -e 'end run' "$GHOSTTY_PID"

wait "$CAPTURE_PID"

if ! rg -Fq 'event type=url value=https://news.ycombinator.com/' "$SESSION_LOG" ||
  rg -Fq 'event type=loadError url=https://news.ycombinator.com/' "$SESSION_LOG"; then
  echo "capture-website-demo: the recorded interaction did not load Hacker News cleanly" >&2
  exit 1
fi

ffmpeg -hide_banner -loglevel error -y \
  -framerate 8 -i "$CAPTURE_DIR/frame-%03d.png" \
  -filter_complex \
  "fps=8,scale=960:-2:flags=lanczos,split[s0][s1];[s0]palettegen=stats_mode=diff:max_colors=128[p];[s1][p]paletteuse=dither=bayer:bayer_scale=3:diff_mode=rectangle" \
  -loop 0 "$OUT_GIF"

STILL="$CAPTURE_DIR/frame-052.png"
[[ -f "$STILL" ]] || STILL="$CAPTURE_DIR/frame-063.png"
ffmpeg -hide_banner -loglevel error -y -i "$STILL" \
  -vf "scale=1024:-2:flags=lanczos" "$OUT_PNG"

osascript \
  -e 'on run argv' \
  -e 'tell application "System Events"' \
  -e 'set frontmost of first process whose unix id is (item 1 of argv as integer) to true' \
  -e 'keystroke "q" using control down' \
  -e 'end tell' \
  -e 'end run' "$GHOSTTY_PID" >/dev/null 2>&1 || true

echo "==> Wrote ${OUT_GIF#$ROOT/} ($(du -h "$OUT_GIF" | cut -f1))"
echo "==> Wrote ${OUT_PNG#$ROOT/} ($(du -h "$OUT_PNG" | cut -f1))"
