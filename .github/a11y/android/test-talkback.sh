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
# with the node's text) as it moves through the window (TalkBack's keyboard
# commands, Alt+Right; then swipes right, its gesture), and its click on
# "Tap me" (Alt+Enter, then a double tap) reaching the app. TalkBack's speech
# itself isn't read (the emulator may have no voice). Exit status 0 when
# TalkBack focused "Tap me" and the click reached the app, 1 when a check
# failed, 3 when there is no TalkBack. Both services are turned off at the
# end.
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
finish() {
  plog > "$out/talkback-probe.log"
  adb logcat -d 2>/dev/null | grep -iE 'talkback|accessibility' | tail -200 > "$out/talkback-logcat.log" || true
  adb exec-out screencap -p > "$out/android-talkback.png" 2>/dev/null || true
  disable
}
trap finish EXIT
# The focus events for node "$1" so far.
focused_on() { plog | grep -E "probe event TYPE_VIEW_ACCESSIBILITY_FOCUSED text=\[$1\]"; }
taps() { applog | grep -cE 'androiddemo: tapped [0-9]+'; }

if ! adb shell pm path "$talkback" | grep -q package:; then
  echo "::warning::no TalkBack ($talkback) on this emulator image"
  exit 3
fi
echo "talkback: $(adb shell pm path "$talkback" | tr -d '\r') $(adb shell dumpsys package "$talkback" | grep -m1 versionName | tr -d '\r ')"
adb install -r "$probe" >/dev/null || { adb uninstall org.stipple.a11yprobe && adb install "$probe"; }
adb logcat -c
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
# TalkBack's first run may open its tutorial over the app: back to the app.
echo "talkback: in front: $(adb shell dumpsys activity activities | grep -m1 -E 'mResumedActivity|topResumedActivity' | tr -d '\r')"
adb shell am start -W -n "$pkg/android.app.NativeActivity" >/dev/null
sleep 3

# Move TalkBack's focus through the window until it is on "Tap me".
size=$(adb shell wm size | sed -n 's/.*: \([0-9]*\)x\([0-9]*\).*/\1 \2/p' | tail -1)
read -r w h <<< "${size:-1080 1920}"
moves=(
  "keys:KEYCODE_ALT_LEFT KEYCODE_DPAD_RIGHT"
  "keys:KEYCODE_ALT_LEFT KEYCODE_SHIFT_LEFT KEYCODE_DPAD_RIGHT"
  "swipe"
)
reached=1
for how in "${moves[@]}"; do
  for i in $(seq 1 8); do
    if focused_on "Tap me" > /dev/null; then reached=0; break; fi
    case "$how" in
      keys:*) adb shell input keycombination ${how#keys:} ;;
      # TalkBack's swipe right (next item): a quick fling across the middle.
      swipe) adb shell input swipe $((w / 5)) $((h / 2)) $((w * 4 / 5)) $((h / 2)) 120 ;;
    esac
    sleep 1.5
  done
  echo "talkback: focus after moving by $how: $(plog | grep -E 'TYPE_VIEW_ACCESSIBILITY_FOCUSED' | tail -3 | sed 's/.*a11yprobe: //' | tr '\n' ';')"
  focused_on "Tap me" > /dev/null && { reached=0; break; }
done
echo "talkback: what TalkBack focused:"
plog | grep -E 'TYPE_VIEW_ACCESSIBILITY_FOCUSED' | sed 's/.*a11yprobe: /  /' | head -40
failed=0
if [ "$reached" = 0 ]; then
  echo "ok: TalkBack's accessibility focus reached \"Tap me\""
else
  echo "FAILED: TalkBack's accessibility focus never reached \"Tap me\""
  failed=1
fi

if [ "$reached" = 0 ]; then
  before=$(taps)
  clicked=1
  for how in "keys:KEYCODE_ALT_LEFT KEYCODE_ENTER" "doubletap"; do
    case "$how" in
      keys:*) adb shell input keycombination ${how#keys:} ;;
      # TalkBack's double tap activates the focused item wherever it lands.
      doubletap) adb shell "input tap $((w / 2)) $((h * 3 / 4)); input tap $((w / 2)) $((h * 3 / 4))" ;;
    esac
    sleep 2
    if [ "$(taps)" -gt "$before" ]; then clicked=0; echo "talkback: clicked by $how"; break; fi
  done
  if [ "$clicked" = 0 ]; then
    echo "ok: TalkBack's click on \"Tap me\" reached the app"
  else
    echo "FAILED: TalkBack's click on \"Tap me\" didn't reach the app"
    failed=1
  fi
  plog | grep -E 'TYPE_VIEW_CLICKED' | sed 's/.*a11yprobe: /  /' | head -5
fi
[ "$failed" = 0 ] && echo "talkback: all checks passed"
exit "$failed"
