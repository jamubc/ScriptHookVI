#!/usr/bin/env bash
set -e
here=$(cd "$(dirname "$0")" && pwd)
out=$(realpath -m "${1:-/tmp/ScriptHookVI-preview.png}")
code=${2:-menu.open = true}
size=${SIZE:-1280x720}
back=${BACK:-xc:#3a2540}
scripts=${SCRIPTS:-scripts}
cd "$here"
log=$(mktemp)
if ! cargo xwin build --release --example preview --target x86_64-pc-windows-msvc >"$log" 2>&1; then
  cat "$log" >&2
  rm -f "$log"
  exit 1
fi
rm -f "$log"
raw=$(mktemp --suffix=.rgba)
WINEDEBUG=-all wine target/x86_64-pc-windows-msvc/release/examples/preview.exe "$scripts" "$raw" "${size%x*}" "${size#*x}" "$code" 2>&1 | grep -a ScriptHookVI || true
magick \( "$back" -resize "$size!" \) \( -size "$size" -depth 8 "rgba:$raw" \) -composite "$out"
rm -f "$raw"
echo "$out"
