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
# 4. a long press on a row (a swipe that stays put for 800 ms) opens its
#    context menu, whose item a tap picks, and a sideways swipe over the strip
#    of chips scrolls it;
# 5. the accessibility tree reaches the system: `uiautomator dump` lists the
#    button and the list's rows as virtual views (the node provider);
# 6. Back goes to the app first (it handles one by resetting the tap count),
#    Home and `am start` send it to the background and back, and a memory
#    trim reaches it (the lifecycle events);
# 7. Tab moves focus to the text field, the input method connects to it (an
#    InputConnection with a text inputType), and `input text` types into it
#    (keys); then the app drives the connection as a composing keyboard would
#    (compose, commit an emoji, delete around the caret, finish), and the
#    field must hold the result;
# 8. Notify asks for POST_NOTIFICATIONS (answered "Don't allow" in the system's
#    dialog, so the next one fails); once `pm grant`ed, the notification shows
#    in `dumpsys notification`;
# 9. Open brings up DocumentsUI's picker, where a file pushed to Downloads is
#    picked and the app reads it; Back cancels another; Save creates
#    stipple-saved.txt with the app's bytes; and the process killed under the
#    picker comes back cleanly (the results fragment isn't restored);
# 10. a Back the app doesn't handle leaves it to the system: with predictive
#    back (Android 13 and later, the manifest's enableOnBackInvokedCallback),
#    Back reaches the app through its OnBackInvokedCallback while it would go
#    back (the tap count isn't 0), and goes straight to the system, which
#    sends the app to the background, while it wouldn't;
# 11. process death: with two taps counted and the field typed in, Home has
#    the app save its state (onSaveInstanceState), `am kill` ends the
#    backgrounded process, and starting it again restores the count and the
#    text from the saved Bundle;
# 12. screenshots of each state land in the screenshot directory.
set -euo pipefail

apk="${1:?usage: test-on-device.sh app.apk [screenshot-dir]}"
shots="${2:-visual_output}"
pkg=com.karpeleslab.stipple
mkdir -p "$shots"

log() { adb logcat -d -s stipple:I 2>/dev/null || true; }
# How many logcat lines match $1 so far.
count() { log | grep -cE "$1" || true; }

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
# Start without the notification permission, as a fresh install does, so the
# app asks for it.
adb shell pm revoke "$pkg" android.permission.POST_NOTIFICATIONS 2>/dev/null || true
adb shell pm clear-permission-flags "$pkg" android.permission.POST_NOTIFICATIONS \
  user-set user-fixed 2>/dev/null || true
adb logcat -c
adb shell am start -W -n "$pkg/android.app.NativeActivity"
sdk="$(adb shell getprop ro.build.version.sdk | tr -d '\r')"
echo "android: API $sdk"

wait_for "Stipple Android: window [0-9]+x[0-9]+ presented=true" 90
wait_for "androiddemo: tick 3 " 60
adb exec-out screencap -p > "$shots/android.png"

# The button covers the middle of the screen.
size="$(adb shell wm size | sed -n 's/.*: \([0-9]*\)x\([0-9]*\).*/\1 \2/p' | tail -1)"
read -r width height <<< "$size"
adb shell input tap $((width / 2)) $((height / 2))
wait_for "androiddemo: tapped 1" 30
if ((sdk >= 33)); then
  # The app now has something to go back from: predictive back's callback
  # goes up.
  wait_for "stipple: back callback registered" 20
fi

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
if ((sdk >= 33)); then
  # It came through the callback, not as a key event; with nothing left to
  # go back from, the callback goes away again.
  wait_for "stipple: back from OnBackInvokedCallback" 5
  wait_for "stipple: back callback unregistered" 20
  # Android 14's OnBackAnimationCallback hears a key press as a gesture too.
  log | grep "androiddemo: back gesture" || echo "android: no back gesture progress for the key"
fi
if ((sdk >= 34)); then
  # The back gesture itself, with gesture navigation: a swipe from the left
  # edge reaches the app with its progress (OnBackAnimationCallback), and
  # goes back when it lifts...
  adb shell cmd overlay enable com.android.internal.systemui.navbar.gestural || true
  sleep 3
  registered="$(count "stipple: back callback registered")"
  handled="$(count "androiddemo: back handled")"
  adb shell input tap $((width / 2)) $((height / 2))
  for _ in $(seq 20); do
    (($(count "stipple: back callback registered") > registered)) && break
    sleep 1
  done
  adb shell input swipe 1 $((height / 2)) $((width * 2 / 3)) $((height / 2)) 800
  wait_for "androiddemo: back gesture Started \{ edge: Left \}" 20
  wait_for "androiddemo: back gesture progress" 10
  for _ in $(seq 20); do
    (($(count "androiddemo: back handled") > handled)) && break
    sleep 1
  done
  if (($(count "androiddemo: back handled") <= handled)); then
    echo "the back gesture didn't go back" >&2
    log | tail -20 >&2
    exit 1
  fi
  # ...while one the finger takes back to the edge goes nowhere.
  registered="$(count "stipple: back callback registered")"
  handled="$(count "androiddemo: back handled")"
  adb shell input tap $((width / 2)) $((height / 2))
  for _ in $(seq 20); do
    (($(count "stipple: back callback registered") > registered)) && break
    sleep 1
  done
  y=$((height / 2))
  adb shell "input motionevent DOWN 1 $y; for x in 40 80 120 160 200 240 280 320 360 400; do input motionevent MOVE \$x $y; done; sleep 0.5; for x in 360 300 240 180 120 60 20 3; do input motionevent MOVE \$x $y; done; sleep 0.5; input motionevent UP 3 $y"
  wait_for "androiddemo: back gesture Cancelled" 20
  sleep 1
  if (($(count "androiddemo: back handled") > handled)); then
    echo "a cancelled back gesture went back" >&2
    exit 1
  fi
  # The count is 1 again: the Back key resets it.
  adb shell input keyevent KEYCODE_BACK
  for _ in $(seq 20); do
    (($(count "androiddemo: back handled") > handled)) && break
    sleep 1
  done
  echo "android: the back gesture's progress reached the app"
fi
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

# A long press on a row (a swipe that stays put) opens its context menu and
# doesn't tap the row; the menu's item, found in the accessibility tree,
# picks it.
taps="$(log | grep -c "androiddemo: row [0-9]* tapped" || true)"
adb shell input swipe $((width / 2)) $((height * 78 / 100)) $((width / 2)) $((height * 78 / 100)) 800
sleep 2
adb exec-out screencap -p > "$shots/android-context-menu.png"
adb shell uiautomator dump /sdcard/stipple-menu.xml >/dev/null
adb shell cat /sdcard/stipple-menu.xml > "$shots/android-menu-ui.xml"
# The center of the first node whose text matches $1, from its bounds.
center_of() {
  sed 's/<node/\n<node/g' "$2" | grep -E "text=\"$1" | head -1 \
    | sed -n 's/.*bounds="\[\([0-9]*\),\([0-9]*\)\]\[\([0-9]*\),\([0-9]*\)\]".*/\1 \2 \3 \4/p' \
    | awk '{ print int(($1 + $3) / 2), int(($2 + $4) / 2) }'
}
item="$(center_of "Pin row [0-9]+" "$shots/android-menu-ui.xml")"
if [ -z "$item" ]; then
  echo "the long press opened no context menu:" >&2
  sed 's/<node/\n<node/g' "$shots/android-menu-ui.xml" | grep -o 'text="[^"]*"' >&2
  exit 1
fi
if [ "$(log | grep -c "androiddemo: row [0-9]* tapped" || true)" != "$taps" ]; then
  echo "the long press tapped the row" >&2
  exit 1
fi
read -r ix iy <<< "$item"
adb shell input tap "$ix" "$iy"
wait_for "androiddemo: row [0-9]+ pinned" 20

# A sideways swipe over the strip of chips scrolls it.
chip="$(center_of "Chip 1\"" "$shots/android-menu-ui.xml")"
if [ -z "$chip" ]; then
  echo "no chip in the accessibility tree" >&2
  exit 1
fi
read -r cx cy <<< "$chip"
adb shell input swipe $((cx + width / 4)) "$cy" $((cx - width / 4)) "$cy" 1000
wait_for "androiddemo: strip scrolled to [1-9][0-9]*" 20

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

### Notifications and the document picker, through the buttons' accessibility
### nodes (their bounds come from a uiautomator dump).

# Tap the first node in a fresh uiautomator dump matching the regex $1.
tap_node() {
  local b x1 y1 x2 y2
  adb shell uiautomator dump /sdcard/stipple-ui.xml >/dev/null 2>&1 || true
  b="$(adb shell cat /sdcard/stipple-ui.xml | sed 's/<node/\n<node/g' | grep -E "$1" | head -1 \
    | sed -n 's/.*bounds="\[\([0-9]*\),\([0-9]*\)\]\[\([0-9]*\),\([0-9]*\)\]".*/\1 \2 \3 \4/p')"
  [ -n "$b" ] || return 1
  read -r x1 y1 x2 y2 <<< "$b"
  adb shell input tap $(((x1 + x2) / 2)) $(((y1 + y2) / 2))
}
# Wait up to $2 seconds for node $1 and tap it.
tap_when() {
  local i
  for ((i = 0; i < ${2:-20}; i++)); do
    tap_node "$1" && return 0
    sleep 1
  done
  echo "no node matching $1 on screen:" >&2
  adb shell cat /sdcard/stipple-ui.xml | sed 's/<node/\n<node/g' | grep -oE '(text|resource-id|content-desc)="[^"]+"' >&2 || true
  adb exec-out screencap -p > "$shots/android-no-node.png" || true
  return 1
}
# Wait up to $2 seconds for the resumed activity to match $1.
wait_top() {
  local i
  for ((i = 0; i < ${2:-20}; i++)); do
    adb shell dumpsys activity activities | grep -E "topResumedActivity" | grep -qE "$1" && return 0
    sleep 1
  done
  echo "the top activity isn't $1:" >&2
  adb shell dumpsys activity activities | grep -E "topResumedActivity" >&2 || true
  return 1
}

# Notifications on Android 13+: the first asks for POST_NOTIFICATIONS (the
# permission was revoked before launch), through the results fragment. "Don't
# allow" drops it and makes the next one fail; once granted, one is posted.
tap_when 'text="Notify"'
wait_for "stipple: asking for the notification permission" 20
wait_top "GrantPermissionsActivity" 20
adb exec-out screencap -p > "$shots/android-permission.png"
tap_when 'permission_deny_button'
wait_for "stipple: notification permission Some\(false\)" 20
wait_for 'notification "Stipple test" not shown' 10
wait_top "$pkg" 20
tap_when 'text="Notify"'
wait_for "androiddemo: notify Err\(Os\(\"notifications aren't allowed\"\)\)" 20
adb shell pm grant "$pkg" android.permission.POST_NOTIFICATIONS
tap_when 'text="Notify"'
wait_for "stipple: notification [0-9]+ posted" 20
sleep 1
if ! adb shell dumpsys notification --noredact | grep -A40 "pkg=$pkg" \
  | grep -q "android.title=String (Stipple test)"; then
  echo "the notification isn't in dumpsys notification:" >&2
  adb shell dumpsys notification --noredact | grep -A20 "pkg=$pkg" >&2 || true
  exit 1
fi
adb shell dumpsys notification --noredact | grep "pkg=$pkg" | head -1

# The document picker (DocumentsUI, through ACTION_OPEN_DOCUMENT): pick a file
# pushed to Downloads, and the app reads its name and contents.
printf 'Hello from the picker\n' > "$shots/stipple-pick.txt"
adb push "$shots/stipple-pick.txt" /sdcard/Download/stipple-pick.txt >/dev/null
adb shell rm -f /sdcard/Download/stipple-saved.txt
tap_when 'text="Open"'
wait_for "stipple: document picker started" 20
wait_top "documentsui" 30
sleep 2
adb exec-out screencap -p > "$shots/android-picker.png"
# It opens on Recent files (or where it was last); go to Downloads if needed.
if ! tap_node 'text="stipple-pick.txt"'; then
  tap_when 'content-desc="Show roots"'
  tap_when 'text="Downloads"'
  tap_when 'text="stipple-pick.txt"'
fi
wait_for 'androiddemo: opened "stipple-pick.txt" \(22 bytes\): "Hello from the picker"' 30
# Back out of the picker: the callback hears it was cancelled.
wait_top "$pkg" 20
tap_when 'text="Open"'
wait_top "documentsui" 30
sleep 1
adb shell input keyevent KEYCODE_BACK
wait_for "androiddemo: open cancelled" 20
# Saving (ACTION_CREATE_DOCUMENT): the picker suggests the file's name, and
# Save writes the bytes there.
wait_top "$pkg" 20
tap_when 'text="Save"'
wait_top "documentsui" 30
tap_when 'text="stipple-saved.txt"'
tap_when 'resource-id="android:id/button1"'
wait_for "androiddemo: saved true" 30
saved="$(adb shell cat /sdcard/Download/stipple-saved.txt)"
echo "saved: $saved"
case "$saved" in
  "Saved by androiddemo"*) ;;
  *) echo "stipple-saved.txt holds: $saved" >&2; exit 1 ;;
esac

# The process dies while the picker is up, and Android brings the activity
# back with its saved state: the results fragment was dropped from it, so the
# restore doesn't trip over a class only Stipple's loader knows.
wait_top "$pkg" 20
tap_when 'text="Open"'
wait_top "documentsui" 30
sleep 1
old_pid="$(adb shell pidof "$pkg" | tr -d '\r')"
adb shell am kill "$pkg"
sleep 1
if adb shell pidof "$pkg" >/dev/null; then
  # am kill only ends processes it deems safe to; a debuggable app's own
  # user can end it too.
  adb shell run-as "$pkg" kill -9 "$old_pid" || true
  sleep 1
fi
adb logcat -c
adb shell input keyevent KEYCODE_BACK
wait_for "Stipple Android: window [0-9]+x[0-9]+ presented=true" 60
new_pid="$(adb shell pidof "$pkg" | tr -d '\r')"
echo "restored after process death: pid $old_pid -> $new_pid"
if [ -z "$new_pid" ] || [ "$new_pid" = "$old_pid" ] || adb logcat -d | grep -qE "FATAL EXCEPTION|Unable to instantiate fragment"; then
  echo "the activity didn't come back cleanly after process death:" >&2
  adb logcat -d | grep -E "AndroidRuntime|FATAL|Fragment" | tail -30 >&2 || true
  exit 1
fi
sleep 2
# The checks below look for their own lines.
adb logcat -c

# A Back the app doesn't handle (the tap count is 0) goes to the system. The
# first Back may only close the soft keyboard.
if ((sdk >= 33)); then
  # Predictive back: the app said it wouldn't go back, so the system goes
  # back without asking it (a root activity moves to the background).
  backgrounds="$(count "androiddemo: lifecycle Background")"
  for _ in 1 2 3; do
    adb shell input keyevent KEYCODE_BACK
    sleep 3
    (($(count "androiddemo: lifecycle Background") > backgrounds)) && break
  done
  if (($(count "androiddemo: lifecycle Background") <= backgrounds)); then
    echo "Back didn't leave the app" >&2
    log | tail -20 >&2
    exit 1
  fi
  if log | grep -q "androiddemo: back not handled"; then
    echo "the app was asked about a Back it said it wouldn't take" >&2
    exit 1
  fi
  echo "android: Back went to the system (predictive back)"
else
  for _ in 1 2 3; do
    adb shell input keyevent KEYCODE_BACK
    sleep 3
    log | grep -q "androiddemo: back not handled" && break
  done
  wait_for "androiddemo: back not handled" 10
fi

# Process death. Back in the foreground (the activity Back finished starts
# again, or one moved to the back returns), two taps (the count is 0 again).
frames="$(count "Stipple Android: window [0-9]+x[0-9]+ presented=true")"
adb shell am start -n "$pkg/android.app.NativeActivity" >/dev/null
for _ in $(seq 30); do
  (($(count "Stipple Android: window [0-9]+x[0-9]+ presented=true") > frames)) && break
  sleep 1
done
sleep 2
adb shell input tap $((width / 2)) $((height / 2))
wait_for "androiddemo: tapped 1" 30
sleep 1
adb shell input tap $((width / 2)) $((height / 2))
wait_for "androiddemo: tapped 2" 30
# Home: the app goes to the background and saves its state.
adb shell input keyevent KEYCODE_HOME
wait_for "androiddemo: saved taps=2" 30
wait_for "stipple: saved [0-9]+ bytes of state" 10
# End the backgrounded process, as the system would to free memory.
pid="$(adb shell pidof "$pkg" | tr -d '\r')"
adb shell am kill "$pkg"
sleep 2
if adb shell pidof "$pkg" >/dev/null; then
  # am kill only ends processes it deems safe to; a debuggable app's own
  # user can end it too.
  adb shell run-as "$pkg" kill -9 "$pid" || true
  sleep 2
fi
if adb shell pidof "$pkg" >/dev/null; then
  echo "the app's process didn't end" >&2
  exit 1
fi
echo "android: process $pid ended"
# Starting it again recreates the activity from the saved Bundle.
adb shell am start -W -n "$pkg/android.app.NativeActivity" >/dev/null
wait_for "stipple: restoring [0-9]+ bytes of saved state" 60
wait_for 'androiddemo: restored taps=2 ' 30
wait_for "Stipple Android: window [0-9]+x[0-9]+ presented=true" 60
sleep 3
adb exec-out screencap -p > "$shots/android-restored.png"
adb shell uiautomator dump /sdcard/stipple-ui.xml >/dev/null
adb shell cat /sdcard/stipple-ui.xml > "$shots/android-restored-ui.xml"
if ! grep -q "Tapped 2 times" "$shots/android-restored-ui.xml"; then
  echo "the restored app doesn't show the restored count:" >&2
  sed 's/<node/\n<node/g' "$shots/android-restored-ui.xml" | grep -o 'text="[^"]*"' >&2
  exit 1
fi
echo "android: state restored after process death"
echo "android: all checks passed"
