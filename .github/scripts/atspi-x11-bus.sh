#!/usr/bin/env bash
# The accessibility bus found through the X root window's AT_SPI_BUS
# property alone, as libatspi finds it: in a private session bus
# (dbus-run-session) on the caller's X display, the bus is started and its
# address put on the root window (at-spi-bus-launcher does that itself on
# X; set by hand when it didn't), then the app runs with no session bus to
# ask (no DBUS_SESSION_BUS_ADDRESS, an empty XDG_RUNTIME_DIR, no
# AT_SPI_BUS_ADDRESS), and a pyatspi client must find it registered.
#
# usage: atspi-x11-bus.sh APP OUT_DIR
set -uo pipefail
APP=$1
OUT=$2
mkdir -p "$OUT"
NAME=$(basename "$APP")
ADDR=$(dbus-send --session --print-reply=literal --dest=org.a11y.Bus /org/a11y/bus \
  org.a11y.Bus.GetAddress | tr -d ' ')
echo "accessibility bus: $ADDR"
sleep 1
# (Through python3-xlib: some xprop builds drop -set on the root. It stays
# connected until the end: when its last client goes, Xvfb resets and the
# root window's properties go with it.)
python3 - "$ADDR" > "$OUT/x11-bus-prop.log" 2>&1 <<'PY' &
import sys
from Xlib import X, Xatom, display
d = display.Display()
root = d.screen().root
atom = d.intern_atom("AT_SPI_BUS")
prop = root.get_full_property(atom, X.AnyPropertyType)
if prop is None or not prop.value:
    print("the launcher didn't set AT_SPI_BUS; setting it")
    root.change_property(atom, Xatom.STRING, 8, sys.argv[1].encode())
    d.sync()
prop = root.get_full_property(atom, X.AnyPropertyType)
print("AT_SPI_BUS on the root window: {!r}".format(prop.value if prop else None), flush=True)
import time
time.sleep(60)
PY
HOLDER=$!
sleep 2
cat "$OUT/x11-bus-prop.log"
EMPTY="$OUT/no-runtime-dir"
mkdir -p "$EMPTY" && chmod 700 "$EMPTY"
env -u DBUS_SESSION_BUS_ADDRESS -u AT_SPI_BUS_ADDRESS XDG_RUNTIME_DIR="$EMPTY" \
  STIPPLE_ATSPI_DEBUG=1 "$APP" > "$OUT/$NAME-x11-bus.log" 2>&1 &
PID=$!
sleep 4
python3 .github/scripts/atspi-roles.py "$NAME" "check box|Remember" | tail -n 2
STATUS=${PIPESTATUS[0]}
if ! grep -q "accessibility bus from the X root window's AT_SPI_BUS" "$OUT/$NAME-x11-bus.log"; then
  echo "the app didn't take the bus from the X property"
  STATUS=1
fi
kill "$PID" "$HOLDER" 2>/dev/null
wait "$PID" "$HOLDER" 2>/dev/null
grep "atspi" "$OUT/$NAME-x11-bus.log" | head -n 20
exit "$STATUS"
