#!/usr/bin/env bash
# Mobile input depth on a device or emulator (the one adb talks to; set
# ANDROID_SERIAL to pick one), with androiddemo installed (test-on-device.sh
# installs it):
#
#   examples/androiddemo/test-input.sh [screenshot-dir]
#
# 1. Drag and drop: a finger long-presses the greeting under the "Tap me"
#    button, which is a drag source of a text: the long press lifts it out
#    of the app (`Window::lift_drag`: `View.startDragAndDrop`,
#    DRAG_FLAG_GLOBAL), the finger carries it over the button and lets go.
#    The window gets the system's DragEvents (STARTED, ENTERED, LOCATION,
#    DROP) through the host view's onDragEvent, the path a drop from
#    another app in split screen takes, and the app logs the drop
#    (`androiddemo: dropped text "Dropped by androiddemo"`). Then the
#    heading, a drag source of a file: it leaves as a content: URI of the
#    app's own FileProvider (org.stipple.android.Files), which the drop
#    reads back through the ContentResolver. Then a file another app's way
#    (a MediaStore content: URI, `android::drag_file_self_test`).
#    (examples/androiddemo/test-drag-apps.sh drops them in another app.)
# 2. Dead keys on a hardware keyboard: a virtual keyboard (made with the
#    shell's `uinput` command, Android 12 and later, so its keys go through
#    the kernel and InputReader as a real keyboard's, with the Generic key
#    character map) types Alt+E (a dead acute accent, U+0301 in Generic.kcm),
#    then E, then Alt+U (a dead diaeresis) and U into the focused text field,
#    which must end in "éü" (the accent shows as a preedit in between). An
#    input method may take hardware keys first; the emulator's passes them
#    on.
#
# Each part reports and the script fails at the end if one didn't pass
# (CI runs it without blocking until it has passed on its emulators).
set -uo pipefail

shots="${1:-visual_output}"
pkg=com.karpeleslab.stipple
mkdir -p "$shots"

log() { adb logcat -d -s stipple:I 2>/dev/null || true; }
count() { log | grep -cE "$1" || true; }

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

# The center of the first node in a fresh uiautomator dump matching $1.
center_of() {
  adb shell uiautomator dump /sdcard/stipple-input-ui.xml >/dev/null 2>&1 || true
  adb shell cat /sdcard/stipple-input-ui.xml | sed 's/<node/\n<node/g' | grep -E "$1" | head -1 \
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

failed=""
sdk="$(adb shell getprop ro.build.version.sdk | tr -d '\r')"
echo "android input: API $sdk"
adb logcat -c
adb shell am start -W -n "$pkg/android.app.NativeActivity" >/dev/null
wait_for "androiddemo: tick" 60 >/dev/null || failed="$failed start"
sleep 2

### 1. Drops, through the system's drag and drop.
# Drag from the node matching $1 to the "Tap me" button: down, held still
# ($2: "lift", a long press, which lifts a drag source out of the app), or a
# small move ($2: "move", the self-test starts its drag), a pause for the
# drag to take the finger, then over the button in steps, and up.
drag_from() {
  local from tap fx fy tx ty i path="" start
  from="$(find_node "$1")" || return 1
  tap="$(find_node 'text="Tap me"')" || return 1
  read -r fx fy <<< "$from"
  read -r tx ty <<< "$tap"
  echo "android input: dragging from $fx,$fy to \"Tap me\" at $tx,$ty ($2)"
  for i in 1 2 3 4 5 6 7 8; do
    path="$path $((fx + (tx - fx) * i / 8)),$((fy + (ty - fy) * i / 8))"
  done
  if [ "$2" = lift ]; then
    start="sleep 1.2"
  else
    start="sleep 0.3; input motionevent MOVE $fx $((fy + 12)); sleep 1.5"
  fi
  adb shell "input motionevent DOWN $fx $fy; $start; \
    for p in $path; do input motionevent MOVE \${p%,*} \${p#*,}; sleep 0.15; done; sleep 0.8; \
    input motionevent UP $tx $ty"
}
drop_part() {
  # A text, lifted from the greeting.
  drag_from 'text="(Hello, [^"]*! )?Tapped [0-9]+ times"' lift || return 1
  wait_for "stipple: lifted drag started: Some\(true\) \(text" 10 || return 1
  wait_for 'androiddemo: dropped text "Dropped by androiddemo" at [0-9]+,[0-9]+' 15 || return 1
  wait_for "androiddemo: drag over" 2 || return 1
  adb exec-out screencap -p > "$shots/android-dropped.png" || true
  echo "android input: a lifted text dropped through onDragEvent"
  sleep 3
  # A file, lifted from the heading: through the app's own FileProvider.
  drag_from 'text="Stipple on Android"' lift || return 1
  wait_for "stipple: lifted drag started: Some\(true\) \(1 file" 10 || return 1
  wait_for 'stipple: file provider opened "androiddemo-lifted.txt"' 15 || return 1
  wait_for 'androiddemo: dropped files \[".*/stipple-drops/[0-9]+/androiddemo-lifted\.txt"\] holding Some\("A file lifted out of androiddemo"\)' 10 || return 1
  echo "android input: a lifted file went through the app's FileProvider and came back"
  # A file (a MediaStore content: URI, Android 10 and later), from the
  # "Running for" label: copied through the ContentResolver into the app's
  # temporary directory, under its display name.
  if ((sdk < 29)); then
    echo "android input: no MediaStore Downloads before Android 10; skipping the file drop"
    return 0
  fi
  sleep 3
  drag_from 'text="Running for [0-9]+ s"' move || return 1
  wait_for 'stipple: dropped "stipple-dragged[^"]*\.txt" copied \([0-9]+ bytes\)' 20 || return 1
  wait_for 'androiddemo: dropped files \[".*/stipple-drops/[0-9]+/stipple-dragged[^"]*\.txt"\] holding Some\("A file dragged by androiddemo"\)' 10 || return 1
  echo "android input: a file drop (content: URI) was copied and arrived"
}
drop_part || failed="$failed drop"

### 2. Dead keys from a hardware keyboard.
dead_keys_part() {
  local field
  if ! adb shell command -v uinput >/dev/null 2>&1; then
    echo "android input: no uinput command (Android 11 and older); skipping the dead keys" >&2
    return 0
  fi
  field="$(find_node 'class="android.widget.EditText"')" || return 1
  adb shell input tap $field
  sleep 2
  # EV_KEY (1) codes: KEY_Q 16 (so Android sees an alphabetic keyboard),
  # KEY_E 18, KEY_U 22, KEY_LEFTALT 56, KEY_SPACE 57.
  local key='{"id":1,"command":"inject","events":[1,%d,%d,0,0,0]}\n'
  local delay='{"id":1,"command":"delay","duration":%d}\n'
  {
    printf '{"id":1,"command":"register","name":"stipple-keyboard","vid":6353,"pid":43982,"bus":"usb",'
    printf '"configuration":[{"type":100,"data":[0,1]},{"type":101,"data":[16,18,22,56,57]}]}\n'
    printf "$delay" 2000
    # Alt+E: the dead acute accent.
    printf "$key" 56 1; printf "$key" 18 1; printf "$key" 18 0; printf "$key" 56 0
    printf "$delay" 800
    printf "$key" 18 1; printf "$key" 18 0
    printf "$delay" 800
    # Alt+U: the dead diaeresis, then U.
    printf "$key" 56 1; printf "$key" 22 1; printf "$key" 22 0; printf "$key" 56 0
    printf "$delay" 800
    printf "$key" 22 1; printf "$key" 22 0
    printf "$delay" 1500
  } | adb shell uinput - >/dev/null
  # The accent alone, as the field's preedit (U+00B4 or U+02CA, as the
  # release's KeyCharacterMap has it).
  wait_for 'androiddemo: text ".*(´|ˊ)"' 10 || return 1
  wait_for 'androiddemo: text ".*éü"' 20 || return 1
  adb exec-out screencap -p > "$shots/android-dead-keys.png" || true
  echo "android input: dead keys composed é and ü"
}
dead_keys_part || failed="$failed dead-keys"

if ! adb shell pidof "$pkg" >/dev/null || log | grep -q "panicked"; then
  echo "the app crashed" >&2
  adb logcat -d | tail -40 >&2
  failed="$failed crash"
fi
if [ -n "$failed" ]; then
  echo "android input: FAILED:$failed" >&2
  exit 1
fi
echo "android input: all checks passed"
