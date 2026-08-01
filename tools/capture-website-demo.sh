#!/usr/bin/env bash
# Record a demo GIF for the landing page (website/assets/demo.gif).
#
# Requirements: Ghostty, terminal-fenster on PATH, ffmpeg, and Accessibility permission
# for screen recording if your OS prompts.
#
# Usage:
#   tools/capture-website-demo.sh
#   # then commit website/assets/demo.gif
set -euo pipefail

ROOT=$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd)
OUT="$ROOT/website/assets/demo.gif"
TMPDIR="${TMPDIR:-/tmp}/tf-demo-$$"
mkdir -p "$TMPDIR" "$(dirname "$OUT")"

command -v ffmpeg >/dev/null 2>&1 || {
  echo "capture-website-demo: install ffmpeg first" >&2
  exit 1
}

if ! command -v terminal-fenster >/dev/null 2>&1; then
  echo "capture-website-demo: build/install terminal-fenster first" >&2
  exit 1
fi

echo "==> Recording 12s window — focus Ghostty when it opens"
echo "    Output: $OUT"

# macOS: capture primary display. Adjust -i for Linux (x11grab) as needed.
if [ "$(uname -s)" = "Darwin" ]; then
  DEVICE=(-f avfoundation -framerate 15 -i "0:none")
else
  DEVICE=(-f x11grab -framerate 15 -video_size 1280x800 -i "${DISPLAY:-:0}.0")
fi

ffmpeg -y "${DEVICE[@]}" -t 12 "$TMPDIR/raw.mp4" &
REC=$!

sleep 2
terminal-fenster open news.ycombinator.com &
TF=$!
sleep 10
kill "$TF" 2>/dev/null || true
wait "$REC" 2>/dev/null || true

ffmpeg -y -i "$TMPDIR/raw.mp4" -vf "fps=12,scale=960:-1:flags=lanczos,split[s0][s1];[s0]palettegen[p];[s1][p]paletteuse" "$OUT"
rm -rf "$TMPDIR"

echo "==> Wrote $OUT ($(du -h "$OUT" | cut -f1))"
