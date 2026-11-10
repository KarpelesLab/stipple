#!/usr/bin/env bash
# The multiwindow example's self-test: titles, geometry, and a window opened
# and closed while the app runs (see examples/multiwindow/src/main.rs).
#
#   bash .github/scripts/runtime-windows.sh <multiwindow binary> <x11|wayland|log>
#
# Every platform checks the app's own report (`AppHandle::window_info`, the
# window count) and the backend's `STIPPLE_DEBUG_WINDOWS` lines. Then:
# - x11: reads the titles back with xprop, the size hints' resize increments,
#   the moved and resized window's geometry with xdotool, and sees the third
#   window come and go;
# - wayland (sway): finds the titles and the third window in `swaymsg -t
#   get_tree` (sway places windows itself, so positions aren't checked);
# - log (Windows, macOS): the debug lines carry what the OS read back.
set -euo pipefail

app="$1"
mode="$2"
out=${OUT_DIR:-visual_output}
mkdir -p "$out"
log="$out/runtime-windows.log"

STIPPLE_DEBUG_WINDOWS=1 "$app" --selftest > "$log" 2>&1 &
pid=$!
trap 'kill $pid 2>/dev/null || true' EXIT

wait_for() {
  for _ in $(seq 150); do
    grep -q "$1" "$log" && return 0
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
  done
  echo "::error::timed out waiting for '$1'"
  cat "$log"
  return 1
}

# Retry a check for up to ~3 s (window managers apply changes asynchronously).
eventually() {
  for _ in $(seq 30); do
    if eval "$1" >/dev/null 2>&1; then return 0; fi
    sleep 0.1
  done
  echo "::error::check failed: $1"
  eval "$1" || true
  return 1
}

x11_title() { xprop -id "$1" _NET_WM_NAME; }
sway_names() { swaymsg -t get_tree | grep -o '"name": "[^"]*"'; }

if [ "$mode" = x11 ]; then
  eventually 'xdotool search --name "Stipple — B"'
  a=$(xdotool search --name "Stipple — A" | head -1)
  b=$(xdotool search --name "Stipple — B" | head -1)
  echo "window A=$a B=$b"
  # The first title from the view, in UTF-8 (the em dash isn't Latin-1).
  eventually "x11_title $a | grep -qF 'Stipple — A step 0'"
fi
if [ "$mode" = wayland ]; then
  eventually "sway_names | grep -qF 'Stipple — B'"
  sway_names
fi

wait_for "multiwindow: step 1"
if [ "$mode" = x11 ]; then
  eventually "x11_title $a | grep -qF 'Stipple — A step 1'"
  eventually "x11_title $b | grep -qF 'Stipple — B ✓ retitled'"
  eventually "xprop -id $a WM_NORMAL_HINTS | grep -q 'resize increment: 8 by 16'"
  eventually "xdotool getwindowgeometry $b | grep -q 'Position: 360,260'"
  eventually "xdotool getwindowgeometry $b | grep -q 'Geometry: 300x200'"
  xprop -id "$a" _NET_WM_NAME WM_NAME WM_NORMAL_HINTS
  xdotool getwindowgeometry "$b"
fi
if [ "$mode" = wayland ]; then
  eventually "sway_names | grep -qF 'Stipple — A step 1'"
  eventually "sway_names | grep -qF 'Stipple — B ✓ retitled'"
fi

wait_for "multiwindow: step 2"
if [ "$mode" = x11 ]; then
  eventually 'xdotool search --name "Stipple — C"'
fi
if [ "$mode" = wayland ]; then
  eventually "sway_names | grep -qF 'Stipple — C'"
  sway_names
fi

wait_for "multiwindow: step 3"
# The app's own view of the windows: B moved and resized (where the
# platform lets apps place windows), C as it opened.
grep "multiwindow: info" "$log"
if [ "$mode" != wayland ]; then
  grep -q "multiwindow: info WindowId(1) pos=360,260 inner=300x200" "$log"
fi
grep -q "multiwindow: info WindowId(2) " "$log"

wait_for "multiwindow: step 4"
if [ "$mode" = x11 ]; then
  eventually '! xdotool search --name "Stipple — C"'
fi
if [ "$mode" = wayland ]; then
  eventually "! sway_names | grep -qF 'Stipple — C'"
fi

wait_for "multiwindow: step 5"
grep -q "multiwindow: windows=2" "$log"
wait_for "multiwindow: event loop exited"
wait "$pid"
trap - EXIT

# What the backend did, read back from the system where it can be.
grep "stipple-window:" "$log"
grep -q 'stipple-window: opened .* "Stipple — C"' "$log"
grep -q "stipple-window: closed " "$log"
if [ "$mode" = log ]; then
  grep -q 'stipple-window: title .*"Stipple — A step 1"' "$log"
  grep -q 'stipple-window: title .*"Stipple — B ✓ retitled"' "$log"
  grep -q "stipple-window: frame .* pos=360,260 inner=300x200" "$log"
  grep -q "stipple-window: increments .* 8x16" "$log"
fi
if [ "$mode" = wayland ]; then
  grep -q 'stipple-window: title .*"Stipple — B ✓ retitled"' "$log"
fi
echo "runtime windows: ok ($mode)"
