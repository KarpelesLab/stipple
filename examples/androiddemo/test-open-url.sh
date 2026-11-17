#!/usr/bin/env bash
# URLs opening androiddemo (App::on_open_url), on a device or emulator (the
# one adb talks to; set ANDROID_SERIAL to pick one), with androiddemo
# installed (test-on-device.sh installs it):
#
#   examples/androiddemo/test-open-url.sh [screenshot-dir]
#
# The manifest claims the stipple-demo scheme on Stipple's Link activity (a
# windowless trampoline: NativeActivity doesn't pass onNewIntent on). Each
# part starts a VIEW intent as a browser following a link does (`am start -a
# android.intent.action.VIEW -d URL`) and reads the app's hook back from
# logcat (`androiddemo: opened url "..."`):
#
# 1. Cold: the app force-stopped, the URL starts its process; the hook hears
#    it once the app runs, and the app's NativeActivity is the one resumed.
# 2. Warm: the app in front, a second URL reaches the same process, and no
#    second NativeActivity starts.
# 3. From the background: Home, then a third URL brings the app back.
#
# Each part reports and the script fails at the end if one didn't pass
# (CI runs it without blocking until it has passed on its emulators).
set -uo pipefail

shots="${1:-visual_output}"
pkg=com.karpeleslab.stipple
mkdir -p "$shots"

log() { adb logcat -d -s stipple:I 2>/dev/null || true; }

# Wait up to $2 seconds for a logcat line matching $1.
wait_for() {
  local pattern="$1" timeout="${2:-30}" i
  for ((i = 0; i < timeout; i++)); do
    if log | grep -qE "$pattern"; then
      log | grep -E "$pattern" | tail -1
      return 0
    fi
    sleep 1
  done
  echo "timed out waiting for: $pattern" >&2
  log | grep -v "androiddemo: tick" | tail -30 >&2
  return 1
}

resumed() { adb shell dumpsys activity activities | grep -E "topResumedActivity|mResumedActivity" | head -1; }
# How many of the app's NativeActivity records the activity manager holds.
activities() {
  adb shell dumpsys activity activities \
    | grep -oE "ActivityRecord\{[0-9a-f]+ u0 $pkg/android.app.NativeActivity" | sort -u | wc -l
}
view() { adb shell am start -W -a android.intent.action.VIEW -d "'$1'" 2>&1 | grep -E "Activity:|Error" || true; }

failed=()

echo "== 1. cold start by URL"
adb shell am force-stop "$pkg"
adb logcat -c
view "stipple-demo://callback?code=1"
if wait_for 'androiddemo: opened url "stipple-demo://callback\?code=1"' 40; then
  sleep 2
  if resumed | grep -q "$pkg/android.app.NativeActivity"; then
    echo "cold: ok ($(resumed))"
  else
    echo "cold: the app's activity isn't in front: $(resumed)" >&2
    failed+=(cold)
  fi
else
  failed+=(cold)
fi
pid="$(adb shell pidof "$pkg" | tr -d '\r')"
adb exec-out screencap -p > "$shots/android-open-url-cold.png" 2>/dev/null || true

echo "== 2. a URL while the app runs"
adb logcat -c
view "stipple-demo://callback?code=2"
if wait_for 'androiddemo: opened url "stipple-demo://callback\?code=2"' 20; then
  now="$(adb shell pidof "$pkg" | tr -d '\r')"
  count="$(activities | tr -d ' \r')"
  if [ "$now" = "$pid" ] && [ "$count" = 1 ] && resumed | grep -q "$pkg/android.app.NativeActivity"; then
    echo "warm: ok (pid $pid, one NativeActivity)"
  else
    echo "warm: pid $pid -> $now, $count NativeActivity record(s), $(resumed)" >&2
    failed+=(warm)
  fi
else
  failed+=(warm)
fi

echo "== 3. a URL while the app is in the background"
adb shell input keyevent KEYCODE_HOME
sleep 2
adb logcat -c
view "stipple-demo://callback?code=3&name=caf%C3%A9"
if wait_for 'androiddemo: opened url "stipple-demo://callback\?code=3&name=caf%C3%A9"' 20; then
  sleep 2
  if resumed | grep -q "$pkg/android.app.NativeActivity"; then
    echo "background: ok"
  else
    echo "background: the app didn't come back: $(resumed)" >&2
    failed+=(background)
  fi
else
  failed+=(background)
fi
adb exec-out screencap -p > "$shots/android-open-url-warm.png" 2>/dev/null || true

if [ ${#failed[@]} -gt 0 ]; then
  echo "test-open-url.sh: failed: ${failed[*]}" >&2
  exit 1
fi
echo "test-open-url.sh: all passed"
