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

# Under Wayland the keys come from wtype, whose every run makes a virtual
# keyboard and destroys it on exit. With no other keyboard in the seat (a
# headless compositor has none), each run turns the seat's keyboard
# capability on and off again: the app (any client) is told the keyboard
# came back and asks for a new wl_keyboard, but wtype presses its key the
# moment the compositor has its keymap, so the press often reaches the seat
# before the app's new wl_keyboard does, and only the release arrives. A
# keyboard held for the whole walk (a wtype that only sleeps; sleep in ms,
# under the 32-bit microseconds wtype's usleep takes) keeps the capability,
# and the app's one wl_keyboard, so every run's keys land.
HOLD=
if [ "$1" = wtype ]; then
  wtype -s 1800000 &
  HOLD=$!
  sleep 1
fi

STIPPLE_ATSPI_DEBUG=1 "$APP" > "$LOG" 2> "$OUT/atspi-app.err" &
PID=$!
sleep 3
python3 .github/scripts/atspi-walk.py "$NAME" "$LOG" "$@"
STATUS=$?
kill "$PID" 2>/dev/null
wait "$PID" 2>/dev/null
if [ -n "$HOLD" ]; then
  kill "$HOLD" 2>/dev/null
  wait "$HOLD" 2>/dev/null
fi
echo "=== $NAME log ==="
tail -n 30 "$LOG"
echo "=== $NAME stderr (bridge debug) ==="
tail -n 60 "$OUT/atspi-app.err"
exit $STATUS
