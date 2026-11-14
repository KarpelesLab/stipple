#!/usr/bin/env bash
# The accessibility bus itself goes away and comes back (at-spi-bus-launcher
# restarted): a running Stipple app (touchdemo) must reconnect and register
# again, and a screen reader that starts afterwards must find it. Runs inside
# a private session bus (dbus-run-session); the display is the caller's.
#
# usage: atspi-bus-restart.sh APP OUT_DIR
set -uo pipefail
APP=$1
OUT=$2
mkdir -p "$OUT"
NAME=$(basename "$APP")
a11y_address() {
  dbus-send --session --print-reply=literal --dest=org.a11y.Bus /org/a11y/bus \
    org.a11y.Bus.GetAddress
}
echo "accessibility bus: $(a11y_address)"

STIPPLE_ATSPI_DEBUG=1 "$APP" > "$OUT/bus-restart-app.log" 2> "$OUT/bus-restart-app.err" &
PID=$!
sleep 3
python3 .github/scripts/atspi-walk.py --find-only "$NAME"
STATUS=$?

# The launcher's process, from the session bus (never another session's).
LAUNCHER=$(dbus-send --session --print-reply=literal --dest=org.freedesktop.DBus \
  /org/freedesktop/DBus org.freedesktop.DBus.GetConnectionUnixProcessID \
  string:org.a11y.Bus | awk '{print $NF}')
echo "stopping at-spi-bus-launcher (pid $LAUNCHER)"
kill "$LAUNCHER"
for _ in $(seq 20); do kill -0 "$LAUNCHER" 2>/dev/null || break; sleep 0.25; done
sleep 1
# A new one starts on demand, with a new bus.
echo "accessibility bus now: $(a11y_address)"
sleep 4
if [ $STATUS -eq 0 ]; then
  python3 .github/scripts/atspi-walk.py --find-only "$NAME"
  STATUS=$?
fi
kill "$PID" 2>/dev/null
wait "$PID" 2>/dev/null
echo "=== $NAME stderr (bridge debug) ==="
tail -n 30 "$OUT/bus-restart-app.err"
grep -q "reconnecting" "$OUT/bus-restart-app.err" || {
  echo "FAILED: the bridge never noticed the bus going away"
  STATUS=1
}
[ $STATUS -eq 0 ] && echo "ok: found again after the accessibility bus restarted"
exit $STATUS
