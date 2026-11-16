#!/usr/bin/env bash
# A drag from androiddemo into another app, on a device or emulator (the one
# adb talks to; set ANDROID_SERIAL to pick one), with androiddemo installed
# (test-on-device.sh installs it) and the probe app built
# (.github/clip/android/build-clip.sh):
#
#   examples/androiddemo/test-drag-apps.sh clipprobe.apk [screenshot-dir]
#
# Both apps go into freeform windows, androiddemo above the probe's drop
# target (`org.stipple.clip.Drop`, an ordinary Java view with an
# OnDragListener): freeform support switched on with `settings put global
# enable_freeform_support 1`, each started with `am start --windowingMode 5`
# and placed with `am task resize`. Then a finger long-presses androiddemo's
# greeting (a drag source of a text), which lifts it out of the app
# (`View.startDragAndDrop`, DRAG_FLAG_GLOBAL), and carries it into the probe's
# window: the probe logs `drop text "Dropped by androiddemo"`. Then the
# heading, a drag source of a file: it leaves as a content: URI of
# androiddemo's FileProvider (org.stipple.android.Files, granted with
# DRAG_FLAG_GLOBAL_URI_READ), which the probe queries for its display name
# and reads through its ContentResolver: `drop file "androiddemo-lifted.txt"
# ... "A file lifted out of androiddemo"`.
#
# Where the windows don't come up in freeform mode it says so and exits 0
# without testing.
set -uo pipefail

probe_apk="${1:?usage: test-drag-apps.sh clipprobe.apk [screenshot-dir]}"
shots="${2:-visual_output}"
pkg=com.karpeleslab.stipple
probe=org.stipple.clip
mkdir -p "$shots"

log() { adb logcat -d -s stipple:I stipple-clip:I 2>/dev/null || true; }

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

# The center of the first node matching $1 in the focused window's dump.
center_of() {
  adb shell uiautomator dump /sdcard/stipple-apps-ui.xml >/dev/null 2>&1 || true
  adb shell cat /sdcard/stipple-apps-ui.xml | sed 's/<node/\n<node/g' | grep -E "$1" | head -1 \
    | sed -n 's/.*bounds="\[\([0-9]*\),\([0-9]*\)\]\[\([0-9]*\),\([0-9]*\)\]".*/\1 \2 \3 \4/p' \
    | awk '{ print int(($1 + $3) / 2), int(($2 + $4) / 2) }' || true
}

# Wait up to 20 s for a node matching $1; print its center.
find_node() {
  local at="" i
  for ((i = 0; i < 20; i++)); do
    at="$(center_of "$1")"
    [ -n "$at" ] && { echo "$at"; return 0; }
    sleep 1
  done
  echo "no node matching $1 on screen" >&2
  return 1
}

# The id of the task running package $1 (the line's order differs between
# releases).
task_of() {
  adb shell dumpsys activity activities | grep -E "^ *\* Task\{.* A=[0-9]+:$1 " | head -1 \
    | sed -E 's/.*Task\{[0-9a-f]+ #([0-9]+).*/\1/'
}

# Whether package $1's task is in a freeform window.
freeform() {
  adb shell dumpsys activity activities | grep -E "^ *\* Task\{.* A=[0-9]+:$1 " | grep -q "mode=freeform"
}

# Put task $1 at left $2, top $3, right $4, bottom $5 (again, if the
# window was still opening and kept its first place: Android 13 does).
place() {
  local i
  for i in 1 2 3; do
    adb shell am task resize "$@"
    sleep 2
    adb shell dumpsys activity activities | grep -A3 -E "^ *\* Task\{[0-9a-f]+ #$1 " \
      | grep -q "mBounds=Rect($2, $3 - $4, $5)" && return 0
  done
  echo "android apps: task $1 didn't move to ($2, $3 - $4, $5)" >&2
}

# Long-press at $1 ("x y"), carry it to $2 in steps, let go.
lift_to() {
  local fx fy tx ty path="" i
  read -r fx fy <<< "$1"
  read -r tx ty <<< "$2"
  echo "android apps: lifting at $fx,$fy, dropping at $tx,$ty"
  for i in 1 2 3 4 5 6 7 8; do
    path="$path $((fx + (tx - fx) * i / 8)),$((fy + (ty - fy) * i / 8))"
  done
  adb shell "input motionevent DOWN $fx $fy; sleep 1.2; \
    for p in $path; do input motionevent MOVE \${p%,*} \${p#*,}; sleep 0.15; done; sleep 0.8; \
    input motionevent UP $tx $ty"
}

sdk="$(adb shell getprop ro.build.version.sdk | tr -d '\r')"
echo "android apps: API $sdk"
adb install -r -g "$probe_apk" >/dev/null || { echo "can't install $probe_apk" >&2; exit 1; }
adb shell settings put global enable_freeform_support 1
adb shell settings put global force_resizable_activities 1
adb shell am force-stop "$pkg"
adb shell am force-stop "$probe"
read -r w h < <(adb shell wm size | sed -n 's/.*: \([0-9]*\)x\([0-9]*\).*/\1 \2/p' | tail -1)

restore() {
  adb shell am force-stop "$probe" || true
  adb shell am force-stop "$pkg" || true
  adb shell settings put global enable_freeform_support 0 || true
  adb shell settings put global force_resizable_activities 0 || true
}
trap restore EXIT

# androiddemo on top: its nodes are read while it has focus.
adb shell am start --windowingMode 5 -n "$pkg/android.app.NativeActivity" >/dev/null
sleep 3
app_task="$(task_of "$pkg")"
# Side by side, not overlapping: a touch on a freeform window raises it.
place "$app_task" 0 0 "$w" $((h * 72 / 100))
sleep 3
if ! freeform "$pkg"; then
  echo "android apps: androiddemo didn't open in a freeform window; skipping"
  exit 0
fi
greeting="$(find_node 'text="(Hello, [^"]*! )?Tapped [0-9]+ times"')" || exit 1
title="$(find_node 'text="Stipple on Android"')" || exit 1
# The probe below it.
adb shell am start --windowingMode 5 -n "$probe/.Drop" >/dev/null
sleep 3
probe_task="$(task_of "$probe")"
place "$probe_task" 0 $((h * 74 / 100)) "$w" $((h * 95 / 100))
sleep 3
target="$(find_node 'text="Drop here"')" || exit 1
adb exec-out screencap -p > "$shots/android-apps-side-by-side.png" || true

failed=""
adb logcat -c
lift_to "$greeting" "$target"
if wait_for 'stipple-clip: drop text "Dropped by androiddemo"' 15; then
  echo "android apps: a text dragged out of androiddemo dropped in another app"
else
  failed="$failed text"
fi
sleep 3
lift_to "$title" "$target"
if wait_for 'stipple-clip: drop file "androiddemo-lifted.txt" type text/plain \(32 bytes\): "A file lifted out of androiddemo"' 20; then
  echo "android apps: a file dragged out of androiddemo was read by another app through its FileProvider"
else
  failed="$failed file"
fi
adb exec-out screencap -p > "$shots/android-apps-dropped.png" || true
log | grep -v "androiddemo: tick" > "$shots/android-apps.log"
if [ -n "$failed" ]; then
  echo "android apps: FAILED:$failed" >&2
  exit 1
fi
echo "android apps: passed"
