#!/usr/bin/env bash
# The default network's changes on a device or emulator (the one adb talks
# to; set ANDROID_SERIAL to pick one), with androiddemo installed
# (test-on-device.sh installs it):
#
#   examples/androiddemo/test-network.sh
#
# androiddemo logs platform::network() at start and each change its
# App::on_network hook hears. With Wi-Fi and mobile data on (an emulator's
# default, Wi-Fi the default network):
#
# 1. Wi-Fi off (`svc wifi disable`): still online, over cellular, metered,
#    and a new generation: the change of default network that breaks open
#    sockets.
# 2. Data Saver on and off (`cmd netpolicy set restrict-background`):
#    constrained, then not.
# 3. Mobile data off too (`svc data disable`): offline.
# 4. Wi-Fi on again: online over Wi-Fi, a newer generation.
#
# Mobile data and Wi-Fi are left on. The script fails if a step's line
# doesn't come.
set -uo pipefail

pkg=com.karpeleslab.stipple
log() { adb logcat -d -s stipple:I 2>/dev/null || true; }
# The app's lines since the last `mark`.
mark=0
mark() { mark=$(log | wc -l); }
since() { log | tail -n "+$((mark + 1))"; }

# Wait up to $2 seconds for a line since the mark matching $1; print the
# first.
wait_for() {
  local pattern="$1" timeout="${2:-30}" i
  for ((i = 0; i < timeout; i++)); do
    if since | grep -qE "$pattern"; then
      since | grep -E "$pattern" | head -1
      return 0
    fi
    sleep 1
  done
  echo "test-network: no line matching: $pattern" >&2
  log | grep -E "androiddemo: network|stipple: .*network" | tail -10 >&2
  return 1
}

generation() { sed -n 's/.*generation=\([0-9]*\).*/\1/p' <<<"$1"; }

restore() {
  adb shell svc wifi enable >/dev/null 2>&1 || true
  adb shell svc data enable >/dev/null 2>&1 || true
  adb shell cmd netpolicy set restrict-background false >/dev/null 2>&1 || true
}
trap restore EXIT

restore
# Let the radios settle on Wi-Fi as the default network.
for _ in $(seq 1 30); do
  adb shell dumpsys connectivity 2>/dev/null | grep -q "Active default network: [0-9]" && break
  sleep 1
done
adb shell am force-stop "$pkg"
adb logcat -c
adb shell am start -W -n "$pkg/android.app.NativeActivity" >/dev/null

fail=0
start=$(wait_for "androiddemo: network online=true transport=Wifi" 30) || fail=1
echo "start: $start"
wait_for "stipple: network callback registered" 10 >/dev/null || fail=1

mark
adb shell svc wifi disable
cell=$(wait_for "network changed online=true transport=Cellular metered=true" 40) || fail=1
echo "wifi off: $cell"
# Android reports Wi-Fi lost a moment before cellular takes over; the
# backend waits a second for the replacement, so no offline comes between.
if since | grep -q "network changed online=false"; then
  echo "test-network: warning: offline between Wi-Fi and cellular" >&2
fi

# Data Saver on, on the metered network: constrained. (API 30 reports the
# network blocked and back first: an offline between.)
mark
adb shell cmd netpolicy set restrict-background true
saver=$(wait_for "network changed online=true transport=Cellular metered=true constrained=true" 30) || fail=1
echo "data saver on: $saver"
mark
adb shell cmd netpolicy set restrict-background false
wait_for "network changed online=true transport=Cellular metered=true constrained=false" 30 >/dev/null || fail=1

mark
adb shell svc data disable
off=$(wait_for "network changed online=false" 40) || fail=1
echo "data off: $off"

mark
adb shell svc wifi enable
wifi=$(wait_for "network changed online=true transport=Wifi" 60) || fail=1
echo "wifi on: $wifi"

if [ "$fail" = 0 ]; then
  g0=$(generation "$start") g1=$(generation "$cell") g2=$(generation "$off") g3=$(generation "$wifi")
  if ! { [ "$g1" -gt "$g0" ] && [ "$g2" -gt "$g1" ] && [ "$g3" -gt "$g2" ]; }; then
    echo "test-network: generations didn't grow: $g0 $g1 $g2 $g3" >&2
    fail=1
  fi
fi
if [ "$fail" != 0 ]; then
  echo "test-network: FAILED" >&2
  exit 1
fi
echo "test-network: ok"
