#!/usr/bin/env bash
# Drive androiddemo (installed, as examples/androiddemo/test-on-device.sh
# leaves it) with TalkBack itself, which the Google APIs emulator images
# from API 33 on ship (/product/app/talkback; the API 30 one doesn't):
#
#   .github/a11y/android/test-talkback.sh a11yprobe.apk [log-dir]
#
# TalkBack and the probe service (.github/a11y/android/Probe.java) are both
# turned on; with TalkBack on, the probe only watches and logs the app's
# accessibility events, which is how this sees what TalkBack does: its
# accessibility focus arriving on a node (TYPE_VIEW_ACCESSIBILITY_FOCUSED,
# with the node's text) as it moves through the window, and its click on
# "Tap me" reaching the app. TalkBack's speech itself isn't read (the
# emulator may have no voice).
#
# TalkBack only hears input that comes through the system's input pipeline
# (its keyboard interceptor and touch explorer are input filters there):
# `adb shell input` injects its events after those filters, so TalkBack
# never sees them (an `input tap` with touch exploration on reaches the app
# as a plain tap). So the keys and touches come from virtual devices made
# with the shell's `uinput` command (API 31 on), whose events go through the
# kernel like a real keyboard's and touch screen's: TalkBack's keyboard
# commands (Alt+Right: next item; Alt+Enter: click), then, if those don't
# reach the app, its gestures (a swipe right, a double tap).
#
# Exit status 0 when TalkBack's focus stopped on a plain label (the window's
# text gets stops of its own, not folded into one item) and reached "Tap me",
# and its click reached the app; 1 when a check failed, 3 when there is no
# TalkBack (or no uinput). Both services are turned off at the end.
set -uo pipefail

probe="${1:?usage: test-talkback.sh a11yprobe.apk [log-dir]}"
out="${2:-visual_output}"
pkg=com.karpeleslab.stipple
talkback=com.google.android.marvin.talkback
services="$talkback/$talkback.TalkBackService:org.stipple.a11yprobe/org.stipple.a11yprobe.Probe"
mkdir -p "$out"

plog() { adb logcat -d -s a11yprobe:I 2>/dev/null || true; }
applog() { adb logcat -d -s stipple:I 2>/dev/null || true; }
disable() {
  adb shell settings put secure enabled_accessibility_services '""' >/dev/null 2>&1 || true
  adb shell settings put secure accessibility_enabled 0 >/dev/null 2>&1 || true
  adb shell settings put secure touch_exploration_enabled 0 >/dev/null 2>&1 || true
}
top() { adb shell dumpsys activity activities | grep -m1 -E 'mResumedActivity|topResumedActivity' | tr -d '\r'; }
# What is on screen and where TalkBack's focus is, for the log: the windows
# (`uiautomator dump` doesn't work with TalkBack on), the resumed activity,
# and the accessibility manager's windows and focus.
diag() {
  echo "talkback: --- $1"
  echo "talkback: resumed: $(top)"
  adb shell dumpsys window windows | grep -E 'mCurrentFocus|Window #' | tr -d '\r' | sed 's/^/talkback:   /'
  adb shell dumpsys accessibility | tr -d '\r' \
    | grep -E 'Focused Window Id|A11yWindow|Enabled features|touchExplorationEnabled' \
    | sed -E 's/, region=SkRegion\([^)]*\)\)//; s/, (layer|transitionTime|hasParent|isAnchored|pictureInPicture)=[^,]*//g; s/^/talkback:   /' \
    | cut -c1-300
}
finish() {
  plog > "$out/talkback-probe.log"
  adb logcat -d 2>/dev/null | grep -iE 'talkback|accessibility|a11yprobe' | tail -300 > "$out/talkback-logcat.log" || true
  adb shell dumpsys accessibility > "$out/talkback-dumpsys-accessibility.log" 2>/dev/null || true
  adb exec-out screencap -p > "$out/android-talkback.png" 2>/dev/null || true
  disable
}
trap finish EXIT
# The focus events so far, as "text;text;...".
focus_trail() {
  plog | sed -n 's/.*probe event TYPE_VIEW_ACCESSIBILITY_FOCUSED text=\[\(.*\)\].*/\1/p' | tr '\n' ';'
}
focused_on() { plog | grep -qE "probe event TYPE_VIEW_ACCESSIBILITY_FOCUSED text=\[$1\]"; }
taps() { applog | grep -cE 'androiddemo: tapped [0-9]+'; }

if ! adb shell pm path "$talkback" | grep -q package:; then
  echo "::warning::no TalkBack ($talkback) on this emulator image"
  exit 3
fi
if ! adb shell command -v uinput >/dev/null 2>&1; then
  echo "::warning::no uinput command on this emulator image: can't reach TalkBack"
  exit 3
fi
echo "talkback: $(adb shell pm path "$talkback" | tr -d '\r') $(adb shell dumpsys package "$talkback" | grep -m1 versionName | tr -d '\r ')"
size=$(adb shell wm size | sed -n 's/.*: \([0-9]*\)x\([0-9]*\).*/\1 \2/p' | tail -1)
read -r w h <<< "${size:-1080 1920}"

# Keys through a virtual keyboard: each argument is a chord of Linux key
# codes (KEY_LEFTALT 56, KEY_RIGHT 106, KEY_ENTER 28), pressed in order and
# released in reverse. The device is made anew each time (the system takes
# it in within the first delay).
keys() {
  local chord k
  {
    printf '{"id":2,"command":"register","name":"stipple-talkback-keys","vid":6353,"pid":43982,"bus":"usb",'
    printf '"configuration":[{"type":100,"data":[1]},{"type":101,"data":[28,42,56,103,105,106,108]}]}\n'
    printf '{"id":2,"command":"delay","duration":1500}\n'
    for chord in "$@"; do
      for k in $chord; do
        printf '{"id":2,"command":"inject","events":[1,%d,1,0,0,0]}\n{"id":2,"command":"delay","duration":30}\n' "$k"
      done
      for k in $(printf '%s\n' $chord | tac); do
        printf '{"id":2,"command":"inject","events":[1,%d,0,0,0,0]}\n{"id":2,"command":"delay","duration":30}\n' "$k"
      done
      printf '{"id":2,"command":"delay","duration":800}\n'
    done
  } | adb shell uinput - >/dev/null
}
# Gestures through a virtual touch screen (a direct one, with multi-touch
# slots, mapped onto the display's pixels, as test-on-device.sh's pinch):
# "swipe" (right, across the middle in 100 ms: TalkBack's next item) and
# "doubletap" (TalkBack activates the focused item wherever it lands).
touches() {
  local g i x0 x1 y
  local abs='"info":{"value":0,"minimum":0,"maximum":%d,"fuzz":0,"flat":0,"resolution":0}'
  inject() { printf '{"id":3,"command":"inject","events":[%s]}\n' "$1"; }
  pause() { printf '{"id":3,"command":"delay","duration":%d}\n' "$1"; }
  down() { inject "3,47,0,3,57,1,3,53,$1,3,54,$2,1,330,1,0,0,0"; }
  move() { inject "3,53,$1,3,54,$2,0,0,0"; }
  up() { inject "3,57,-1,1,330,0,0,0,0"; }
  {
    printf '{"id":3,"command":"register","name":"stipple-talkback-touch","vid":6353,"pid":43983,"bus":"usb",'
    printf '"configuration":[{"type":100,"data":[0,1,3]},{"type":101,"data":[330]},'
    printf '{"type":103,"data":[47,53,54,57]},{"type":110,"data":[1]}],"abs_info":['
    printf "{\"code\":47,$abs},{\"code\":53,$abs},{\"code\":54,$abs},{\"code\":57,$abs}]}\n" \
      9 $((w - 1)) $((h - 1)) 65535
    pause 1500
    for g in "$@"; do
      case "$g" in
        swipe)
          x0=$((w / 5)) x1=$((w * 4 / 5)) y=$((h / 2))
          down "$x0" "$y"
          for i in 1 2 3 4 5 6 7 8 9 10; do
            pause 10
            move $((x0 + (x1 - x0) * i / 10)) "$y"
          done
          pause 10
          up
          ;;
        doubletap)
          down $((w / 2)) $((h * 3 / 4)); pause 40; up; pause 100
          down $((w / 2)) $((h * 3 / 4)); pause 40; up
          ;;
      esac
      pause 800
    done
  } | adb shell uinput - >/dev/null
}

adb install -r "$probe" >/dev/null || { adb uninstall org.stipple.a11yprobe && adb install "$probe"; }
adb logcat -c
# Notification permission dialogs held TalkBack's focus in CI: the demo's
# (it asks on Android 13+), and TalkBack's own (its PermissionRequestActivity
# asks as it starts, in front of the app). Both granted up front.
adb shell pm grant "$pkg" android.permission.POST_NOTIFICATIONS 2>/dev/null || true
adb shell pm grant "$talkback" android.permission.POST_NOTIFICATIONS 2>/dev/null || true
adb shell am start -W -n "$pkg/android.app.NativeActivity" >/dev/null
sleep 3
adb shell settings put secure enabled_accessibility_services "$services"
adb shell settings put secure accessibility_enabled 1
for i in $(seq 1 30); do
  plog | grep -q "probe watching" && break
  sleep 1
done
plog | grep "probe watching" || echo "talkback: the probe didn't report watching (it may have run its own steps)"
echo "talkback: enabled services: $(adb shell settings get secure enabled_accessibility_services | tr -d '\r')"
echo "talkback: touch exploration: $(adb shell settings get secure touch_exploration_enabled | tr -d '\r')"
sleep 5
diag "TalkBack on"
# Something else may still be in front (a dialog, TalkBack's tutorial): Back
# to the app.
if ! top | grep -q "$pkg/"; then
  echo "talkback: not the app in front: Back"
  adb shell input keyevent KEYCODE_BACK
  sleep 1
  adb shell am start -W -n "$pkg/android.app.NativeActivity" >/dev/null
  sleep 3
  diag "after Back"
fi
echo "talkback: focus so far: $(focus_trail)"

# Move TalkBack's focus through the window, an item at a time, until it is
# on "Tap me".
reached=1
for how in keys touch; do
  for i in $(seq 1 16); do
    if focused_on "Tap me"; then reached=0; break; fi
    case "$how" in
      keys) keys "56 106" ;;
      touch) touches swipe ;;
    esac
    sleep 0.5
  done
  echo "talkback: focus after moving by $how: $(focus_trail)"
  focused_on "Tap me" && { reached=0; break; }
done
failed=0
if [ "$reached" = 0 ]; then
  echo "ok: TalkBack's accessibility focus reached \"Tap me\""
else
  echo "FAILED: TalkBack's accessibility focus never reached \"Tap me\""
  diag "focus never reached \"Tap me\""
  failed=1
fi
# The window's labels are items of their own (TalkBack folds the text of a
# focusable node's children into it: a focusable window would hide them).
if focused_on "(Stipple on Android|Type your name:|Running for [0-9]+ s)"; then
  echo "ok: TalkBack stopped on a label"
else
  echo "FAILED: TalkBack never stopped on a label (\"Stipple on Android\", \"Type your name:\", \"Running for\")"
  failed=1
fi

if [ "$reached" = 0 ]; then
  before=$(taps)
  clicked=1
  for how in keys doubletap; do
    case "$how" in
      keys) keys "56 28" ;;
      doubletap) touches doubletap ;;
    esac
    sleep 2
    if [ "$(taps)" -gt "$before" ]; then clicked=0; echo "talkback: clicked by $how"; break; fi
  done
  if [ "$clicked" = 0 ]; then
    echo "ok: TalkBack's click on \"Tap me\" reached the app"
  else
    echo "FAILED: TalkBack's click on \"Tap me\" didn't reach the app"
    diag "the click didn't reach the app"
    failed=1
  fi
  plog | grep -E 'TYPE_VIEW_CLICKED' | sed 's/.*a11yprobe: /  /' | head -5
fi
[ "$failed" = 0 ] && echo "talkback: all checks passed"
exit "$failed"
