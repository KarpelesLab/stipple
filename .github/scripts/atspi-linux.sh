#!/usr/bin/env bash
# Run a Stipple app (touchdemo) and check it through AT-SPI from outside, as a
# screen reader does (.github/scripts/atspi-walk.py). Runs inside a private
# session bus (dbus-run-session), whose org.a11y.Bus starts at-spi-bus-launcher
# and the registry on demand; the display (Xvfb's DISPLAY, or sway's
# WAYLAND_DISPLAY) is the caller's.
#
# usage: atspi-linux.sh APP OUT_DIR TAB_COMMAND...
set -uo pipefail
APP=$1
OUT=$2
shift 2
mkdir -p "$OUT"
LOG="$OUT/atspi-app.log"
NAME=$(basename "$APP")

# Start the accessibility bus first, as a desktop session does.
dbus-send --session --print-reply --dest=org.a11y.Bus /org/a11y/bus \
  org.a11y.Bus.GetAddress > "$OUT/atspi-bus.txt" 2>&1 || true
cat "$OUT/atspi-bus.txt"

STIPPLE_ATSPI_DEBUG=1 "$APP" > "$LOG" 2> "$OUT/atspi-app.err" &
PID=$!
sleep 3
python3 .github/scripts/atspi-walk.py "$NAME" "$LOG" "$@"
STATUS=$?
kill "$PID" 2>/dev/null
wait "$PID" 2>/dev/null
echo "=== $NAME log ==="
tail -n 30 "$LOG"
echo "=== $NAME stderr (bridge debug) ==="
tail -n 60 "$OUT/atspi-app.err"
exit $STATUS
