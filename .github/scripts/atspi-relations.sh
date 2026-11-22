#!/usr/bin/env bash
# Run a Stipple app and follow its relations through AT-SPI from outside
# (.github/scripts/atspi-relations.py: controller-for / controlled-by,
# member-of, node-child-of / node-parent-of). Runs inside a private session
# bus (dbus-run-session); the display is the caller's.
#
# usage: atspi-relations.sh OUT_DIR CHECKS -- APP [ARGS...]
#   e.g. atspi-relations.sh out "controls radios" -- target/release/examples/widgetsdemo
set -uo pipefail
OUT=$1
CHECKS=$2
shift 3
APP=$1
mkdir -p "$OUT"
NAME=$(basename "$APP")
export XDG_CONFIG_HOME="$OUT/$NAME-config"
dbus-send --session --print-reply --dest=org.a11y.Bus /org/a11y/bus \
  org.a11y.Bus.GetAddress > /dev/null 2>&1 || true
STIPPLE_AUDIO=null STIPPLE_ATSPI_DEBUG=1 "$@" > "$OUT/$NAME-relations.log" 2>&1 &
PID=$!
sleep 4
# shellcheck disable=SC2086
python3 .github/scripts/atspi-relations.py "$NAME" $CHECKS | tee "$OUT/$NAME-relations-walk.log"
STATUS=${PIPESTATUS[0]}
kill "$PID" 2>/dev/null
wait "$PID" 2>/dev/null
if [ "$STATUS" != 0 ]; then
  echo "=== $NAME log ==="
  tail -n 40 "$OUT/$NAME-relations.log"
fi
exit "$STATUS"
