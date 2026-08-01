#!/usr/bin/env bash
# Compile the macOS native scroll helper (faster than `swift scroll_helper.swift`).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/native/macos/scroll_helper.swift"
OUT="$ROOT/native/macos/scroll-helper"
swiftc -O -o "$OUT" "$SRC"
echo "built $OUT"
