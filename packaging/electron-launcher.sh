#!/bin/sh
set -eu

engine_root=$(CDPATH='' cd -- "$(dirname "$0")/../.." && pwd -P)
path_file="$engine_root/node_modules/electron/path.txt"
[ -f "$path_file" ] || {
  printf 'terminal-fenster: Electron path marker is missing: %s\n' "$path_file" >&2
  exit 1
}
runtime_relative=$(tr -d '\r\n' < "$path_file")
[ -n "$runtime_relative" ] || {
  printf 'terminal-fenster: Electron path marker is empty: %s\n' "$path_file" >&2
  exit 1
}
runtime="$engine_root/node_modules/electron/dist/$runtime_relative"
[ -x "$runtime" ] || {
  printf 'terminal-fenster: Electron runtime is missing or not executable: %s\n' "$runtime" >&2
  exit 1
}
exec "$runtime" "$@"
