#!/usr/bin/env bash
# Run Orca, the GNOME screen reader, against a running Stipple app (touchdemo)
# and check what it says: Orca logs every utterance ("SPEECH OUTPUT: '...'")
# to its debug file whether or not a speech synthesizer can be heard. Tab
# moves the focus into the app's text field and typing echoes into it; Orca
# must speak the window and the focused entry. Runs inside a private session
# bus (dbus-run-session); the display is the caller's.
#
# usage: orca-linux.sh APP OUT_DIR TAB_COMMAND TYPE_COMMAND
set -uo pipefail
APP=$1
OUT=$2
TAB=$3
TYPE=$4
mkdir -p "$OUT"
dbus-send --session --print-reply --dest=org.a11y.Bus /org/a11y/bus \
  org.a11y.Bus.GetAddress > /dev/null 2>&1 || true

"$APP" > "$OUT/orca-app.log" 2>&1 &
APP_PID=$!
sleep 3
orca --version || true
orca --replace --debug --debug-file="$OUT/orca-debug.out" > "$OUT/orca.log" 2>&1 &
ORCA_PID=$!
# Orca starts, finds the desktop's applications and the active window.
sleep 12
$TAB
sleep 4
$TYPE
sleep 4
kill "$ORCA_PID" "$APP_PID" 2>/dev/null
wait 2>/dev/null

grep -a "SPEECH OUTPUT" "$OUT/orca-debug.out" > "$OUT/orca-speech.txt" 2>/dev/null
echo "=== what Orca said ==="
cat "$OUT/orca-speech.txt"
echo "=== orca stdout/stderr ==="
tail -n 30 "$OUT/orca.log"
# Orca read the app: the focused field (an entry, "text" in some Orca
# versions), named once the typed text is in it.
grep -qiE "entry|text" "$OUT/orca-speech.txt" || {
  echo "FAILED: Orca never spoke the focused entry"
  echo "=== the end of Orca's debug log ==="
  tail -n 80 "$OUT/orca-debug.out"
  exit 1
}
echo "ok: Orca spoke the focused entry"
