#!/usr/bin/env bash
# Install androiddemo on a device or emulator (the one adb talks to; set
# ANDROID_SERIAL to pick one), drive it, and check what it logs:
#
#   examples/androiddemo/test-on-device.sh stipple.apk [screenshot-dir]
#
# 1. a frame reaches the window, and the app's timer ticks (loop wakeups);
# 2. a tap on the button in the middle of the screen registers (touch);
# 3. a swipe over the list scrolls it without tapping a row, and a tap there
#    still taps one (touch scrolling);
# 4. the accessibility tree reaches the system: `uiautomator dump` lists the
#    button and the list's rows as virtual views (the node provider);
# 5. Back goes to the app first (it handles one by resetting the tap count),
#    Home and `am start` send it to the background and back, and a memory
#    trim reaches it (the lifecycle events);
# 6. Tab moves focus to the text field, the input method connects to it (an
#    InputConnection with a text inputType), and `input text` types into it
#    (keys); then the app drives the connection as a composing keyboard would
#    (compose, commit an emoji, delete around the caret, finish), and the
#    field must hold the result;
# 7. a Back the app doesn't handle leaves it to the system;
# 8. screenshots of each state land in the screenshot directory.
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
  # The timer ticks every second: leave its lines out, so the rest shows.
  log | grep -v "androiddemo: tick" | tail -60 >&2
  adb exec-out screencap -p > "$shots/android-timeout.png" || true
  adb logcat -d | grep -E "onStartInput|ImeTracker" | tail -10 >&2 || true
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

# Accessibility: the provider's virtual views show in the window's hierarchy.
wait_for "stipple: host view installed" 5
adb shell uiautomator dump /sdcard/stipple-ui.xml >/dev/null
adb shell cat /sdcard/stipple-ui.xml > "$shots/android-ui.xml"
for node in 'text="Tap me"[^>]*class="android.widget.Button"' \
  'text="Row 0"[^>]*class="android.widget.Button"' \
  'class="android.widget.EditText"' 'text="Stipple on Android"'; do
  if ! grep -qE "$node" "$shots/android-ui.xml"; then
    echo "no accessibility node matching $node:" >&2
    sed 's/<node/\n<node/g' "$shots/android-ui.xml" | grep -o 'text="[^"]*"[^>]*class="[^"]*"' >&2
    exit 1
  fi
done
echo "accessibility: $(grep -o '<node' "$shots/android-ui.xml" | wc -l) nodes"

# Back: the app handles the first one (the tap count goes back to 0).
adb shell input keyevent KEYCODE_BACK
wait_for "androiddemo: back handled" 20
# Home sends the app to the background, and starting it brings it back.
adb shell input keyevent KEYCODE_HOME
wait_for "androiddemo: lifecycle Background" 20
adb shell am start -n "$pkg/android.app.NativeActivity" >/dev/null
wait_for "androiddemo: lifecycle Foreground" 30
adb shell am send-trim-memory "$pkg" RUNNING_LOW
wait_for "androiddemo: lifecycle LowMemory" 20
sleep 2

# A finger drag over the list at the bottom of the screen scrolls it (and
# flings it on), and doesn't tap the row it started on. It runs before typing,
# while no soft keyboard covers the list. The swipe takes a second: an emulator
# renders slowly enough that a quicker one's moves get folded into its release.
adb exec-out screencap -p > "$shots/android-before-swipe.png"
adb shell input swipe $((width / 2)) $((height * 86 / 100)) $((width / 2)) $((height * 70 / 100)) 1000
if ! wait_for "androiddemo: scrolled to [1-9][0-9]*" 20; then
  echo "the swipe didn't scroll the list; the app's log:" >&2
  log | tail -20 >&2
  exit 1
fi
sleep 2
adb exec-out screencap -p > "$shots/android-swiped.png"
if log | grep -q "androiddemo: row [0-9]* tapped"; then
  echo "the swipe tapped a row:" >&2
  log | grep "androiddemo: row" >&2
  exit 1
fi
log | grep "androiddemo: scrolled to" | tail -3
# ...while a tap on the list still taps a row.
adb shell input tap $((width / 2)) $((height * 78 / 100))
wait_for "androiddemo: row [0-9]+ tapped" 20

adb shell input keyevent KEYCODE_TAB
sleep 1
adb shell input text "Stipple"
# Each key rebuilds the frame, which the emulator's software rendering makes slow.
wait_for 'androiddemo: text "Stipple"' 90
# The keyboard connected through the host view's InputConnection.
wait_for "stipple: input connection created \(inputType 0x4001" 10
# The app's self-test then composes "にほ", commits "日本🎉", deletes the
# emoji (two UTF-16 units) and composes and finishes "!".
wait_for "stipple: input connection self-test: inputType" 30
wait_for 'androiddemo: text "Stipple日本!"' 60
sleep 2
adb exec-out screencap -p > "$shots/android-typed.png"

if ! adb shell pidof "$pkg" >/dev/null || log | grep -q "panicked"; then
  echo "the app crashed" >&2
  adb logcat -d | tail -60 >&2
  exit 1
fi

# A Back the app doesn't handle (the tap count is 0) goes to the system. The
# first Back may only close the soft keyboard.
for _ in 1 2 3; do
  adb shell input keyevent KEYCODE_BACK
  sleep 3
  log | grep -q "androiddemo: back not handled" && break
done
wait_for "androiddemo: back not handled" 10
echo "android: all checks passed"
