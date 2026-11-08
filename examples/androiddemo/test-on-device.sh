#!/usr/bin/env bash
# Install androiddemo on a device or emulator (the one adb talks to; set
# ANDROID_SERIAL to pick one), drive it, and check what it logs:
#
#   examples/androiddemo/test-on-device.sh stipple.apk [screenshot-dir]
#
# 1. a frame reaches the window, and the app's timer ticks (loop wakeups);
# 2. a tap on the button in the middle of the screen registers (touch);
# 3. Tab moves focus to the text field and `input text` types into it (keys);
# 4. screenshots of each state land in the screenshot directory.
set -euo pipefail

apk="${1:?usage: test-on-device.sh app.apk [screenshot-dir]}"
shots="${2:-visual_output}"
pkg=com.karpeleslab.stipple
mkdir -p "$shots"

log() { adb logcat -d -s stipple:I 2>/dev/null || true; }

# Wait up to $2 seconds for a logcat line matching $1.
wait_for() {
  local pattern="$1" timeout="${2:-60}" i
  for ((i = 0; i < timeout; i++)); do
    if log | grep -qE "$pattern"; then
      log | grep -E "$pattern" | tail -1
      return 0
    fi
    sleep 1
  done
  echo "timed out waiting for: $pattern" >&2
  log | tail -40 >&2
  return 1
}

# Each build signs with a fresh debug key, so an older install has to go first.
adb install -r "$apk" || { adb uninstall "$pkg" && adb install "$apk"; }
adb shell am force-stop "$pkg" || true
adb logcat -c
adb shell am start -W -n "$pkg/android.app.NativeActivity"

wait_for "Stipple Android: window [0-9]+x[0-9]+ presented=true" 90
wait_for "androiddemo: tick 3 " 60
adb exec-out screencap -p > "$shots/android.png"

# The button covers the middle of the screen.
size="$(adb shell wm size | sed -n 's/.*: \([0-9]*\)x\([0-9]*\).*/\1 \2/p' | tail -1)"
read -r width height <<< "$size"
adb shell input tap $((width / 2)) $((height / 2))
wait_for "androiddemo: tapped 1" 30

adb shell input keyevent KEYCODE_TAB
sleep 1
adb shell input text "Stipple"
wait_for 'androiddemo: text "Stipple"' 30
sleep 2
adb exec-out screencap -p > "$shots/android-typed.png"

if ! adb shell pidof "$pkg" >/dev/null || log | grep -q "panicked"; then
  echo "the app crashed" >&2
  adb logcat -d | tail -60 >&2
  exit 1
fi
echo "android: all checks passed"
