#!/usr/bin/env bash
# The reference app (examples/mediabrowser) read through AT-SPI from outside,
# as Orca does: started on its sample folder with a sound selected, its tree
# must hold the folder tree, the file table with its headers and rows, the
# player's Play button and its position slider, and the menu bar. Runs inside
# a private session bus (dbus-run-session); the display is the caller's.
#
# usage: atspi-mediabrowser.sh APP OUT_DIR
set -uo pipefail
APP=$1
OUT=$2
mkdir -p "$OUT"
# Its settings go to a fresh directory, not the runner's.
export XDG_CONFIG_HOME="$OUT/mediabrowser-config"
dbus-send --session --print-reply --dest=org.a11y.Bus /org/a11y/bus \
  org.a11y.Bus.GetAddress > /dev/null 2>&1 || true
STIPPLE_AUDIO=null "$APP" --sample --select Music/chime.wav > "$OUT/mediabrowser-atspi.log" 2>&1 &
PID=$!
sleep 4
python3 .github/scripts/atspi-roles.py "$(basename "$APP")" \
  "tree|Folders" "tree item|Music" "table|Files" "table column header|Name" \
  "table row|*" "table cell|chime.wav" "button|Play" "slider|Position" "menu bar|*" \
  | tee "$OUT/mediabrowser-atspi-tree.log"
STATUS=${PIPESTATUS[0]}
kill "$PID" 2>/dev/null
wait "$PID" 2>/dev/null
exit "$STATUS"
