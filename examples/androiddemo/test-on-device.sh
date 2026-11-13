#!/usr/bin/env bash
# Install androiddemo on a device or emulator (the one adb talks to; set
# ANDROID_SERIAL to pick one), drive it, and check what it logs:
#
#   examples/androiddemo/test-on-device.sh stipple.apk [screenshot-dir]
#
# 1. a frame reaches the window, and the app's timer ticks (loop wakeups);
# 2. a tap on the button in the middle of the screen registers (touch), and
#    two fingers pinch it, spreading then closing, without tapping it (a
#    virtual touch screen made with the shell's `uinput` command, which
#    Android 11 doesn't have yet: `adb shell input` has one finger);
# 3. a swipe over the list scrolls it without tapping a row, and a tap there
#    still taps one (touch scrolling);
# 4. a long press on a row (a swipe that stays put for 800 ms) opens its
#    context menu, whose item a tap picks, and a sideways swipe over the strip
#    of chips scrolls it;
# 5. the accessibility tree reaches the system: `uiautomator dump` lists the
#    button and the list's rows as virtual views (the node provider);
# 6. Back goes to the app first (it handles one by resetting the tap count),
#    on Android 13 and later through its OnBackInvokedCallback; with gesture
#    navigation (Android 10 and later), a swipe from the left edge goes back
#    too (on Android 14 the app hears its progress), and one the finger takes
#    back to the edge doesn't; Home and `am start` send it to the background
#    and back, and a memory trim reaches it (the lifecycle events);
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
#    sends the app to the background, while it wouldn't (before Android 13,
#    the app hears it and declines, and the activity's onBackPressed sends
#    it away);
# 11. process death: with two taps counted and the field typed in, Home has
#    the app save its state (onSaveInstanceState), `am kill` ends the
#    backgrounded process, and starting it again restores the count and the
#    text from the saved Bundle;
# 12. screenshots of each state land in the screenshot directory.
#
# STIPPLE_ANDROID_QUICK=1 runs only what differs between Android releases
# (1, 2, 5, 6 and 10), as CI does on its older emulators.
#
# The edge swipe needs a navigation bar: an emulator made without a device
# profile has hardware keys instead (`hw.mainKeys=yes` in its config.ini),
# so no back gesture, and the swipe is skipped. STIPPLE_ANDROID_STRICT=1 (CI,
# which sets `hw.mainKeys=no`) fails instead.
set -euo pipefail

apk="${1:?usage: test-on-device.sh app.apk [screenshot-dir]}"
shots="${2:-visual_output}"
pkg=com.karpeleslab.stipple
quick="${STIPPLE_ANDROID_QUICK:-0}"
strict="${STIPPLE_ANDROID_STRICT:-0}"
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

size="$(adb shell wm size | sed -n 's/.*: \([0-9]*\)x\([0-9]*\).*/\1 \2/p' | tail -1)"
read -r width height <<< "$size"
# Where the "Tap me" button is, from its accessibility node: the layout
# depends on the screen (CI's emulator is 320x640, a Pixel 6 much larger).
wait_for "stipple: host view installed" 30
tap_me=""
for _ in $(seq 10); do
  adb shell uiautomator dump /sdcard/stipple-ui.xml >/dev/null 2>&1 || true
  tap_me="$(adb shell cat /sdcard/stipple-ui.xml | sed 's/<node/\n<node/g' \
    | grep -E 'text="Tap me"' | head -1 \
    | sed -n 's/.*bounds="\[\([0-9]*\),\([0-9]*\)\]\[\([0-9]*\),\([0-9]*\)\]".*/\1 \2 \3 \4/p' \
    | awk '{ print int(($1 + $3) / 2), int(($2 + $4) / 2) }')"
  [ -n "$tap_me" ] && break
  sleep 1
done
if [ -z "$tap_me" ]; then
  echo "no \"Tap me\" node; tapping the middle of the screen" >&2
  tap_me="$((width / 2)) $((height / 2))"
fi
read -r tap_x tap_y <<< "$tap_me"
echo "android: \"Tap me\" at $tap_x,$tap_y"
adb shell input tap "$tap_x" "$tap_y"
wait_for "androiddemo: tapped 1" 30
if ((sdk >= 33)); then
  # The app now has something to go back from: predictive back's callback
  # goes up.
  wait_for "stipple: back callback registered" 20
fi

# Pinch: `adb shell input` has one finger, so a virtual touch screen (made
# with the shell's `uinput` command, which reads its device and events as
# JSON) puts two on the button, 60 px apart, spreads them to 180 px, lifts
# them, and then closes them from 180 px to 60 px. Its events go through the
# kernel and the system's input pipeline as a real screen's do.
# The uinput script for two fingers going from $1 to $2 px apart along a row
# through the button's center, by $3 px a step, 50 ms apart.
pinch_events() {
  local from="$1" to="$2" by="$3" d
  local move='{"id":1,"command":"inject","events":[3,47,0,3,53,%d,3,47,1,3,53,%d,0,0,0]}\n'
  local delay='{"id":1,"command":"delay","duration":%d}\n'
  printf '{"id":1,"command":"inject","events":[3,47,0,3,57,1,3,53,%d,3,54,%d,1,330,1,0,0,0]}\n' \
    $((tap_x - from / 2)) "$tap_y"
  printf "$delay" 100
  printf '{"id":1,"command":"inject","events":[3,47,1,3,57,2,3,53,%d,3,54,%d,0,0,0]}\n' \
    $((tap_x + from / 2)) "$tap_y"
  for ((d = from + by; (by > 0 && d <= to) || (by < 0 && d >= to); d += by)); do
    printf "$delay" 50
    printf "$move" $((tap_x - d / 2)) $((tap_x + d / 2))
  done
  printf "$delay" 100
  printf '{"id":1,"command":"inject","events":[3,47,1,3,57,-1,0,0,0]}\n'
  printf '{"id":1,"command":"inject","events":[3,47,0,3,57,-1,1,330,0,0,0,0]}\n'
  printf "$delay" 500
}
if adb shell command -v uinput >/dev/null 2>&1; then
  taps="$(count "androiddemo: tapped")"
  begun="$(count "androiddemo: pinch Began")"
  abs='"info":{"value":0,"minimum":0,"maximum":%d,"fuzz":0,"flat":0,"resolution":0}'
  {
    # A direct touch screen (INPUT_PROP_DIRECT) with BTN_TOUCH and the
    # multi-touch slots, mapped onto the display's pixels.
    printf '{"id":1,"command":"register","name":"stipple-pinch","vid":6353,"pid":43981,"bus":"usb",'
    printf '"configuration":[{"type":100,"data":[0,1,3]},{"type":101,"data":[330]},'
    printf '{"type":103,"data":[47,53,54,57]},{"type":110,"data":[1]}],"abs_info":['
    printf "{\"code\":47,$abs},{\"code\":53,$abs},{\"code\":54,$abs},{\"code\":57,$abs}]}\n" \
      9 $((width - 1)) $((height - 1)) 65535
    # The system picks up the new device.
    printf '{"id":1,"command":"delay","duration":1500}\n'
    pinch_events 60 180 12
    pinch_events 180 60 -12
  } | adb shell uinput - >/dev/null
  for _ in $(seq 20); do
    (($(count "androiddemo: pinch Ended") >= 2)) && break
    sleep 1
  done
  log | grep -E "androiddemo: pinch (Began|Ended)" | tail -4 || true
  ends="$(log | { grep -oE "androiddemo: pinch Ended scale [0-9.]+" || true; } | tail -2 \
    | awk '{ print $NF }' | tr '\n' ' ')"
  read -r spread closed <<< "$ends"
  if (($(count "androiddemo: pinch Began") < begun + 2)) \
    || ! awk -v s="${spread:-0}" -v c="${closed:-9}" 'BEGIN { exit !(s > 2.5 && c < 0.4) }'; then
    echo "the pinch didn't reach the app (ended at scales: $ends):" >&2
    log | grep -v "androiddemo: tick" | tail -20 >&2
    exit 1
  fi
  if (($(count "androiddemo: tapped") != taps)); then
    echo "the pinch tapped the button" >&2
    exit 1
  fi
  echo "android: pinched to $spread and back to $closed"
else
  echo "android: no uinput command (Android 11 and older); skipping the pinch" >&2
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

# The back gesture, with gesture navigation (Android 10 and later): a swipe
# from the left edge goes back (through the callback on Android 13, with its
# progress on Android 14, as the Back key before), and one the finger takes
# back to the edge goes nowhere. SystemUI's edge handler only runs with a
# navigation bar: a device with hardware keys has none.
gestures=no
if ((sdk >= 29)); then
  if [ "$(adb shell settings get secure navigation_mode | tr -d '\r')" != 2 ]; then
    # Switching the navigation mode recreates the activity: wait for it to
    # settle before going on.
    adb shell cmd overlay enable com.android.internal.systemui.navbar.gestural || true
    for _ in $(seq 20); do
      [ "$(adb shell settings get secure navigation_mode | tr -d '\r')" = 2 ] && break
      sleep 1
    done
    sleep 5
  fi
  for _ in $(seq 10); do
    if adb shell dumpsys activity service com.android.systemui 2>/dev/null \
      | grep -A1 "EdgeBackGestureHandler:" | grep -q "mIsEnabled=true"; then
      gestures=yes
      break
    fi
    sleep 1
  done
  if [ "$gestures" = no ]; then
    why="no back gesture: SystemUI's edge handler isn't on (navigation_mode $(adb shell settings get secure navigation_mode | tr -d '\r'), qemu.hw.mainkeys '$(adb shell getprop qemu.hw.mainkeys | tr -d '\r')': hardware keys leave no navigation bar)"
    if [ "$strict" = 1 ]; then
      echo "$why" >&2
      exit 1
    fi
    echo "android: $why; skipping the edge swipe" >&2
  fi
fi
# Tap the button (the app then has something to go back from), and wait for
# predictive back's callback where there is one.
tap_for_back() {
  local registered
  registered="$(count "stipple: back callback registered")"
  adb shell input tap "$tap_x" "$tap_y"
  if ((sdk >= 33)); then
    for _ in $(seq 20); do
      (($(count "stipple: back callback registered") > registered)) && break
      sleep 1
    done
  else
    sleep 2
  fi
}
# Wait up to 20 s for more than $2 lines matching $1.
wait_more() {
  local i
  for ((i = 0; i < 20; i++)); do
    (($(count "$1") > $2)) && return 0
    sleep 1
  done
  return 1
}
if [ "$gestures" = yes ]; then
  handled="$(count "androiddemo: back handled")"
  started="$(count "androiddemo: back gesture Started")"
  callback="$(count "stipple: back from OnBackInvokedCallback")"
  tap_for_back
  # Step by step, with a pause before lifting: seen in CI, a slow emulator
  # delivered `input swipe`'s moves all at once, the gesture starting and
  # going back in the same few milliseconds, with no progress in between.
  y=$((height / 2))
  out="$(for i in 1 2 3 4 5 6 7 8 9 10; do echo $((width * i * 2 / 30)); done | tr '\n' ' ')"
  adb shell "input motionevent DOWN 1 $y; for x in $out; do input motionevent MOVE \$x $y; done; sleep 0.5; input motionevent UP $((width * 2 / 3)) $y"
  if ! wait_more "androiddemo: back handled" "$handled"; then
    echo "the edge swipe didn't go back:" >&2
    log | grep -v "androiddemo: tick" | tail -20 >&2
    adb exec-out screencap -p > "$shots/android-edge-swipe.png" || true
    exit 1
  fi
  if ((sdk >= 33)) && (($(count "stipple: back from OnBackInvokedCallback") <= callback)); then
    echo "the edge swipe went back without the OnBackInvokedCallback" >&2
    exit 1
  fi
  if ((sdk >= 34)); then
    # Its start and progress reached the app first.
    if (($(count "androiddemo: back gesture Started") <= started)); then
      echo "the edge swipe's start didn't reach the app" >&2
      exit 1
    fi
    wait_for "androiddemo: back gesture progress" 10
  fi
  echo "android: the edge swipe went back"
  # ...while one the finger takes back to the edge goes nowhere.
  handled="$(count "androiddemo: back handled")"
  tap_for_back
  y=$((height / 2))
  out="$(for i in 1 2 3 4 5 6 7 8; do echo $((width * i * 7 / 64)); done | tr '\n' ' ')"
  back="$(for i in 6 4 2 1; do echo $((width * i * 7 / 64)); done | tr '\n' ' ')"
  adb shell "input motionevent DOWN 1 $y; for x in $out; do input motionevent MOVE \$x $y; done; sleep 0.5; for x in $back 3; do input motionevent MOVE \$x $y; done; sleep 0.5; input motionevent UP 3 $y"
  if ((sdk >= 34)); then
    wait_for "androiddemo: back gesture Cancelled" 20
  fi
  sleep 3
  if (($(count "androiddemo: back handled") > handled)); then
    echo "a cancelled back gesture went back" >&2
    exit 1
  fi
  # The count is 1 again: the Back key resets it.
  adb shell input keyevent KEYCODE_BACK
  wait_more "androiddemo: back handled" "$handled"
  echo "android: the cancelled edge swipe didn't go back"
fi
# Home sends the app to the background, and starting it brings it back.
adb shell input keyevent KEYCODE_HOME
wait_for "androiddemo: lifecycle Background" 20
adb shell am start -n "$pkg/android.app.NativeActivity" >/dev/null
wait_for "androiddemo: lifecycle Foreground" 30
adb shell am send-trim-memory "$pkg" RUNNING_LOW
wait_for "androiddemo: lifecycle LowMemory" 20
sleep 2

# The checks from here to the declined Back are the same on every release:
# not in quick mode. (The block isn't indented: it's most of the script.)
if [ "$quick" != 1 ]; then

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
fi
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
  # The activity's onBackPressed then finishes it (or, on Android 12, moves
  # it to the back).
  wait_for "androiddemo: lifecycle Background" 20
  echo "android: Back went to the system (onBackPressed)"
fi

# Process death (not in quick mode; not indented, like the block above).
# Back in the foreground (the activity Back finished starts again, or one
# moved to the back returns), two taps (the count is 0 again).
if [ "$quick" != 1 ]; then
frames="$(count "Stipple Android: window [0-9]+x[0-9]+ presented=true")"
adb shell am start -n "$pkg/android.app.NativeActivity" >/dev/null
for _ in $(seq 30); do
  (($(count "Stipple Android: window [0-9]+x[0-9]+ presented=true") > frames)) && break
  sleep 1
done
sleep 2
adb shell input tap "$tap_x" "$tap_y"
wait_for "androiddemo: tapped 1" 30
sleep 1
adb shell input tap "$tap_x" "$tap_y"
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
fi
echo "android: all checks passed"
