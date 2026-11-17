#!/usr/bin/env bash
# A drag from androiddemo into another app, on a device or emulator (the one
# adb talks to; set ANDROID_SERIAL to pick one), with androiddemo installed
# (test-on-device.sh installs it) and the probe app built
# (.github/clip/android/build-clip.sh):
#
#   examples/androiddemo/test-drag-apps.sh clipprobe.apk [screenshot-dir]
#
# Both apps go on screen at once, androiddemo above the probe's drop target
# (`org.stipple.clip.Drop`, an ordinary Java view with an OnDragListener).
# On most releases in freeform windows: freeform support switched on with
# `settings put global enable_freeform_support 1`, each started with
# `am start --windowingMode 5` and placed with `am task resize`. Android 13
# (API 33) moves a freeform task's bounds in the window manager but leaves
# its surface where it opened, so the windows overlap on screen and the drop
# lands back in androiddemo: there the two go into split screen instead,
# androiddemo moved to the top stage with the shell's own command
# (`dumpsys activity service SystemUIService WMShell splitscreen
# moveToSideStage`), on a display made twice as tall for the test
# (`wm size`, reset afterwards) so androiddemo's half shows all of it.
# STIPPLE_DRAG_LAYOUT=freeform|split picks one on any release. Then a finger long-presses androiddemo's
# greeting (a drag source of a text), which lifts it out of the app
# (`View.startDragAndDrop`, DRAG_FLAG_GLOBAL), and carries it into the probe's
# window: the probe logs `drop text "Dropped by androiddemo"`. Then the
# heading, a drag source of a file: it leaves as a content: URI of
# androiddemo's FileProvider (org.stipple.android.Files, granted with
# DRAG_FLAG_GLOBAL_URI_READ), which the probe queries for its display name
# and reads through its ContentResolver: `drop file "androiddemo-lifted.txt"
# ... "A file lifted out of androiddemo"`.
#
# Where the windows don't come up in freeform mode (or split screen) it says
# so and exits 0 without testing.
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
layout="${STIPPLE_DRAG_LAYOUT:-}"
if [ -z "$layout" ]; then
  if [ "$sdk" = 33 ]; then layout=split; else layout=freeform; fi
fi
echo "android apps: API $sdk, $layout windows"
adb install -r -g "$probe_apk" >/dev/null || { echo "can't install $probe_apk" >&2; exit 1; }
if [ "$layout" = freeform ]; then
  adb shell settings put global enable_freeform_support 1
  adb shell settings put global force_resizable_activities 1
fi

# End any split screen: each app task in a stage taken out of the side stage
# (which ends the split). A split left over (an earlier run's, with a stopped
# app's stage empty) would keep its other stage's task, whichever app that
# is, as androiddemo's partner.
unsplit() {
  local task
  for task in $(adb shell dumpsys activity activities \
    | grep -E "^ *\* Task\{[0-9a-f]+ #[0-9]+ type=standard .*mode=multi-window" \
    | sed -E 's/.*Task\{[0-9a-f]+ #([0-9]+).*/\1/' | sort -u); do
    adb shell dumpsys activity service SystemUIService WMShell splitscreen \
      removeFromSideStage "$task" >/dev/null 2>&1 || true
  done
}

[ "$layout" = split ] && unsplit
adb shell am force-stop "$pkg"
adb shell am force-stop "$probe"
[ "$layout" = split ] && { adb shell wm size reset; sleep 2; }
read -r w h < <(adb shell wm size | sed -n 's/.*: \([0-9]*\)x\([0-9]*\).*/\1 \2/p' | tail -1)

restore() {
  [ "$layout" = split ] && unsplit
  adb shell am force-stop "$probe" || true
  adb shell am force-stop "$pkg" || true
  adb shell settings put global enable_freeform_support 0 || true
  adb shell settings put global force_resizable_activities 0 || true
  adb shell wm size reset || true
}
trap restore EXIT

# The center of window $1's frame (dumpsys window), as "x y".
window_center() {
  adb shell dumpsys window windows | grep -E -A30 "Window #[0-9]+ Window\{[0-9a-f]+ u0 $1\}:" \
    | grep -oE ' frame=\[[0-9]+,[0-9]+\]\[[0-9]+,[0-9]+\]' | head -1 \
    | sed -E 's/.*\[([0-9]+),([0-9]+)\]\[([0-9]+),([0-9]+)\]/\1 \2 \3 \4/' \
    | awk '{ print int(($1 + $3) / 2), int(($2 + $4) / 2) }'
}

# The packages of the visible apps' tasks in a split stage, one per line.
split_visible() {
  adb shell dumpsys activity activities \
    | grep -E "^ *\* Task\{[0-9a-f]+ #[0-9]+ type=standard .* visible=true .*mode=multi-window" \
    | sed -E 's/.* A=[0-9]+:([^ ]+) .*/\1/' | sort -u
}

# The probe full screen, then androiddemo, moved to the top stage: the probe
# fills the other half. Whether the two (and only they) show split.
split_up() {
  adb shell am start -W -n "$probe/.Drop" >/dev/null
  sleep 2
  adb shell am start -W -n "$pkg/android.app.NativeActivity" >/dev/null
  sleep 3
  adb shell dumpsys activity service SystemUIService WMShell splitscreen \
    moveToSideStage "$(task_of "$pkg")" 0 >/dev/null
  sleep 4
  # The probe may have the focus now (uiautomator dumps the focused window):
  # starting androiddemo again focuses its stage, still split.
  adb shell am start -W -n "$pkg/android.app.NativeActivity" >/dev/null
  sleep 2
  [ "$(split_visible | tr '\n' ' ')" = "$(printf '%s\n' "$pkg" "$probe" | sort | tr '\n' ' ')" ]
}

if [ "$layout" = split ]; then
  # Twice as tall, so each half is about the screen androiddemo has alone.
  adb shell wm size "${w}x$((h * 2))"
  sleep 2
  # The shell sometimes pairs androiddemo with another app it had shown
  # (seen with Chrome): end that split and try again.
  ok=""
  for attempt in 1 2 3; do
    split_up && { ok=1; break; }
    echo "android apps: split screen showed: $(split_visible | tr '\n' ' ')(attempt $attempt)"
    unsplit
    adb shell am force-stop "$pkg"
    adb shell am force-stop "$probe"
    sleep 3
  done
  if [ -z "$ok" ]; then
    echo "android apps: the two apps didn't go into split screen; skipping"
    adb shell dumpsys activity activities | grep -E "^ *\* Task\{" >&2
    exit 0
  fi
  greeting="$(find_node 'text="(Hello, [^"]*! )?Tapped [0-9]+ times"')" || exit 1
  title="$(find_node 'text="Stipple on Android"')" || exit 1
  # uiautomator dumps androiddemo's window, not the probe's: its middle.
  target="$(window_center "$probe/$probe.Drop")"
  [ -n "$target" ] || { echo "no window for $probe/.Drop" >&2; exit 1; }
else
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
fi
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
