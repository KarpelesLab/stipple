#!/usr/bin/env bash
# stipple::platform::android on a device or emulator (the one adb talks to;
# set ANDROID_SERIAL to pick one): JNI access from the app, and an activity
# started for a result, with intentresult's APK (see lib.rs for building it):
#
#   examples/intentresult/test-on-device.sh intentresult.apk [screenshot-dir]
#
# 1. At start: java_vm, application_context and activity answer, and
#    run_on_ui_thread runs on the main thread; an intent nothing handles,
#    started for a result, is answered RESULT_CANCELED (0).
# 2. A tap on the big button asks VpnService.prepare for the consent dialog
#    and starts it for a result; its Cancel closes it, and the callback
#    hears 0.
# 3. Again, and while the dialog is up the font scale changes, which the
#    manifest doesn't declare in configChanges: Android recreates the
#    activity under the dialog. Then *OK* (found with uiautomator): the
#    answer, RESULT_OK (-1), is delivered to the new activity's fragment and
#    still reaches the callback.
# 4. Again: prepare returns null now (consent given), and the app says so.
#
# Each part reports, and the script fails at the end if one didn't pass.
set -uo pipefail

apk="${1:?usage: test-on-device.sh intentresult.apk [screenshot-dir]}"
shots="${2:-visual_output}"
pkg=com.karpeleslab.stipple.intentresult
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
  log | tail -30 >&2
  return 1
}

dump() {
  adb shell uiautomator dump /sdcard/stipple-result-ui.xml >/dev/null 2>&1 || true
  adb shell cat /sdcard/stipple-result-ui.xml 2>/dev/null | sed 's/<node/\n<node/g'
}
# The center of the first node in a fresh dump matching $1.
center_of() {
  dump | grep -E "$1" | head -1 \
    | sed -n 's/.*bounds="\[\([0-9]*\),\([0-9]*\)\]\[\([0-9]*\),\([0-9]*\)\]".*/\1 \2 \3 \4/p' \
    | awk '{ print int(($1 + $3) / 2), int(($2 + $4) / 2) }' || true
}
# The resumed activity's line in dumpsys (Android 11 and older name it
# mResumedActivity).
resumed() { adb shell dumpsys activity activities | grep -E "topResumedActivity|mResumedActivity" | head -1; }

# Wait up to 20 seconds for the VPN consent dialog to be in front.
wait_dialog() {
  local i
  for ((i = 0; i < 20; i++)); do
    if resumed | grep -q "vpndialogs"; then return 0; fi
    sleep 1
  done
  echo "the consent dialog didn't come up: $(resumed)" >&2
  return 1
}

# Tap the big button (it fills the middle of the screen).
ask() {
  local at x y
  at="$(center_of 'text="Ask for VPN consent"')"
  if [ -z "$at" ]; then
    read -r x y <<< "$(adb shell wm size | sed -n 's/.*: \([0-9]*\)x\([0-9]*\).*/\1 \2/p' | tail -1)"
    at="$((x / 2)) $((y / 2))"
  fi
  read -r x y <<< "$at"
  adb shell input tap "$x" "$y"
}

failed=""
sdk="$(adb shell getprop ro.build.version.sdk | tr -d '\r')"
echo "intentresult: API $sdk"
adb install -r "$apk" >/dev/null || { adb uninstall "$pkg" >/dev/null; adb install "$apk" >/dev/null; }
# No consent yet (a run before may have given it).
adb shell appops set "$pkg" ACTIVATE_VPN default 2>/dev/null || true
adb shell settings put system font_scale 1.0
adb shell am force-stop "$pkg"
adb logcat -c
adb shell am start -W -n "$pkg/android.app.NativeActivity" >/dev/null

### 1. JNI access, the main thread, an intent nothing handles.
start_part() {
  wait_for "intentresult: started 1 vm=true context=true activity=true" 60 || return 1
  wait_for "intentresult: ui thread main=true" 10 || return 1
  wait_for "intentresult: unhandled result 0 data=false" 10 || return 1
}
start_part || failed="$failed start"
sleep 3

### 2. The consent dialog, backed out of.
cancel_part() {
  ask
  wait_for "intentresult: asking" 10 >/dev/null || return 1
  wait_dialog || return 1
  # Drawn and focused before Back goes to it.
  sleep 3
  adb shell screencap -p /sdcard/stipple-result.png && adb pull /sdcard/stipple-result.png "$shots/intentresult-dialog.png" >/dev/null
  # The dialog isn't cancelable (Back does nothing): its Cancel button.
  local at x y
  at="$(center_of 'resource-id="android:id/button2"')"
  if [ -z "$at" ]; then
    echo "no Cancel button in the dialog" >&2
    return 1
  fi
  read -r x y <<< "$at"
  adb shell input tap "$x" "$y"
  wait_for "intentresult: vpn result 0 data=(true|false) main=true \(activity 1\)" 20 || return 1
}
cancel_part || failed="$failed cancel"
sleep 2

### 3. Agreed to, with the activity recreated under the dialog.
recreate_part() {
  local ok x y
  ask
  wait_dialog || return 1
  adb shell settings put system font_scale 1.15
  # The activity under the dialog is visible, so it's recreated now.
  wait_for "intentresult: started 2 vm=true context=true activity=true" 30 || return 1
  sleep 2
  # The new activity's fragment, added at once since a request waits (the
  # first activity added its own).
  if [ "$(log | grep -c 'results fragment added')" -lt 2 ]; then
    echo "the new activity has no results fragment" >&2
    return 1
  fi
  ok="$(center_of 'resource-id="android:id/button1"')"
  if [ -z "$ok" ]; then
    echo "no OK button in the dialog" >&2
    dump | grep -E 'text="[^"]+"' | head -20 >&2
    return 1
  fi
  read -r x y <<< "$ok"
  adb shell input tap "$x" "$y"
  wait_for "intentresult: vpn result -1 data=(true|false) main=true \(activity 2\)" 20 || return 1
}
recreate_part || failed="$failed recreate"
adb shell settings put system font_scale 1.0
sleep 4

### 4. Consent given: prepare returns null.
prepared_part() {
  # Restoring the font scale recreated the activity once more.
  wait_for "intentresult: started 3" 30 >/dev/null || return 1
  sleep 2
  ask
  wait_for "intentresult: vpn already prepared" 20 || return 1
}
prepared_part || failed="$failed prepared"

log > "$shots/intentresult-logcat.log"
adb shell appops set "$pkg" ACTIVATE_VPN default 2>/dev/null || true
if [ -n "$failed" ]; then
  echo "intentresult: FAILED:$failed" >&2
  exit 1
fi
echo "intentresult: all parts passed"
