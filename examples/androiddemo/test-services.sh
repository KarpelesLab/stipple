#!/usr/bin/env bash
# The system's services from androiddemo, driven with `adb shell input`
# touches, on a device or emulator (the one adb talks to; set ANDROID_SERIAL
# to pick one), with androiddemo installed (test-on-device.sh installs it)
# and the clipboard probe (.github/clip/android/build-clip.sh) given:
#
#   examples/androiddemo/test-services.sh clipprobe.apk [screenshot-dir]
#
# 1. Timers: the app's one-second AppHandle::every timer changes visible text
#    ("Running for N s"), read twice from the accessibility tree
#    (`uiautomator dump`), and logs its ticks.
# 2. The clipboard out of the app: swipes bring the strip's last chips into
#    view and a tap on *Set clip* writes the clipboard (ClipboardManager,
#    through platform::clipboard); another app's activity (the probe's
#    Read, brought to the front so it may read) reads it back.
# 3. The clipboard into the app: the probe's receiver puts a text on the
#    clipboard, as another app copying (`am broadcast`), and a tap on *Get
#    clip* reads it in the app.
# 4. Opening a URL: a tap on *Open link* (AppHandle::open_url, an
#    ACTION_VIEW intent): the activity manager logs the VIEW intent with the
#    URL started from the app's uid and resolved to another app (logcat's
#    `START u0 {act=android.intent.action.VIEW dat=https://example.com/...
#    cmp=...} from uid N`), and that app comes to the front; the app is
#    brought back with `am start`. Chrome's first-run screen is switched off
#    for it (its command-line file, Chrome as the debug app), though the
#    check doesn't depend on what the browser shows.
#
# Each part reports and the script fails at the end if one didn't pass
# (CI runs it without blocking until it has passed on its emulators).
set -uo pipefail

probe="${1:?usage: test-services.sh clipprobe.apk [screenshot-dir]}"
shots="${2:-visual_output}"
pkg=com.karpeleslab.stipple
copied="Copied by androiddemo"
from_probe="Copied by another app"
url_host="example.com"
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

dump() {
  adb shell uiautomator dump /sdcard/stipple-services-ui.xml >/dev/null 2>&1 || true
  adb shell cat /sdcard/stipple-services-ui.xml 2>/dev/null | sed 's/<node/\n<node/g'
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

# Bring the strip's chip $1 into view (swiping the strip leftwards from
# Chip 1) and print its center.
chip() {
  local at strip sx sy width i
  width="$(adb shell wm size | sed -n 's/.*: \([0-9]*\)x[0-9]*.*/\1/p' | tail -1)"
  for ((i = 0; i < 8; i++)); do
    at="$(center_of "text=\"$1\"")"
    if [ -n "$at" ]; then
      read -r sx sy <<< "$at"
      # Wholly on screen: not cut by the screen's right edge.
      if ((sx < width * 85 / 100)); then
        echo "$at"
        return 0
      fi
    fi
    strip="$(center_of 'text="Chip [0-9]+"')"
    [ -n "$strip" ] || strip="$at"
    [ -n "$strip" ] || { echo "no chip on screen" >&2; return 1; }
    read -r sx sy <<< "$strip"
    adb shell input swipe $((width * 3 / 4)) "$sy" $((width / 4)) "$sy" 600
    sleep 1
  done
  echo "the chip \"$1\" never came into view" >&2
  return 1
}

failed=""
sdk="$(adb shell getprop ro.build.version.sdk | tr -d '\r')"
echo "android services: API $sdk"
adb install -r "$probe" >/dev/null || { adb uninstall org.stipple.clip >/dev/null; adb install "$probe" >/dev/null; }
adb shell am force-stop "$pkg"
adb logcat -c
adb shell am start -W -n "$pkg/android.app.NativeActivity" >/dev/null
wait_for "androiddemo: tick" 60 >/dev/null || failed="$failed start"
sleep 2

### 1. A timer changing visible text.
timer_part() {
  local first second
  first="$(dump | grep -oE 'text="Running for [0-9]+ s"' | head -1)"
  sleep 3
  second="$(dump | grep -oE 'text="Running for [0-9]+ s"' | head -1)"
  echo "android services: the label read $first, then $second"
  [ -n "$first" ] && [ -n "$second" ] && [ "$first" != "$second" ] || return 1
  wait_for "androiddemo: tick [3-9]" 10 >/dev/null || return 1
  echo "android services: the timer updates the screen"
}
timer_part || failed="$failed timer"

### 2. The app writes the clipboard, another app reads it.
copy_part() {
  local at
  at="$(chip "Set clip")" || return 1
  adb shell input tap $at
  wait_for "androiddemo: clipboard set true" 10 || return 1
  # Android 13's clipboard overlay (bottom left) goes after a few seconds.
  sleep 2
  adb shell am start -W -n org.stipple.clip/.Read >/dev/null
  wait_for "stipple-clip: read" 15 || return 1
  log | grep -qF "stipple-clip: read \"$copied\"" || return 1
  echo "android services: another app read what the app copied"
  adb shell am start -W -n "$pkg/android.app.NativeActivity" >/dev/null
  sleep 2
}
copy_part || failed="$failed copy"

### 3. Another app writes the clipboard, the app reads it.
paste_part() {
  local at
  # 0x20: FLAG_INCLUDE_STOPPED_PACKAGES.
  adb shell am broadcast -f 0x20 -n org.stipple.clip/.Set -a org.stipple.clip.SET \
    --es text "'$from_probe'" >/dev/null
  wait_for "stipple-clip: set \"$from_probe\"" 15 || return 1
  sleep 6
  at="$(chip "Get clip")" || return 1
  adb shell input tap $at
  wait_for "androiddemo: clipboard got " 10 || return 1
  log | grep -qF "androiddemo: clipboard got Some(\"$from_probe\")" || return 1
  echo "android services: the app read what another app copied"
}
paste_part || failed="$failed paste"

### 4. Opening a URL in another app.
# Chrome's first run (its welcome screen, which takes the VIEW intent's place
# in the activity stack) off while the part runs: Chrome reads its command
# line from this file when it's the debug app (or on a debuggable build).
chrome=com.android.chrome
chrome_flags=/data/local/tmp/chrome-command-line
no_first_run() {
  adb shell pm path "$chrome" >/dev/null 2>&1 || return 0
  adb shell "echo '_ --disable-fre --no-default-browser-check --no-first-run' > $chrome_flags"
  adb shell am set-debug-app --persistent "$chrome" >/dev/null
  adb shell am force-stop "$chrome"
}
first_run_back() {
  adb shell pm path "$chrome" >/dev/null 2>&1 || return 0
  adb shell am clear-debug-app >/dev/null || true
  adb shell rm -f "$chrome_flags" || true
}

# The activity manager's line for an activity started with a VIEW intent
# for the URL's host, from uid $1 (`START u0 {act=... dat=https://host/...
# cmp=<what it resolved to>} from uid N`; Android 13 and later shorten the
# URL's path to `...`).
view_started() {
  adb logcat -d -s ActivityTaskManager:I ActivityManager:I 2>/dev/null \
    | grep -E "START u0 \{.*act=android.intent.action.VIEW .*dat=https://$url_host/.*\} .*from uid $1( |$)" \
    | tail -1
}

url_part() {
  local at top uid started
  uid="$(adb shell cmd package list packages -U "$pkg" | grep -E "package:$pkg " \
    | sed -n 's/.*uid:\([0-9]*\).*/\1/p' | head -1)"
  [ -n "$uid" ] || { echo "no uid for $pkg" >&2; return 1; }
  no_first_run
  at="$(chip "Open link")" || return 1
  adb shell input tap $at
  wait_for "androiddemo: opened https://$url_host/[^ ]* Ok\(\(\)\)" 10 || return 1
  # The app's open_url fired a VIEW intent for the URL, which the system
  # resolved to another app and started.
  for _ in $(seq 10); do
    started="$(view_started "$uid")"
    [ -n "$started" ] && break
    sleep 1
  done
  if [ -z "$started" ]; then
    echo "no VIEW intent for $url_host started from uid $uid" >&2
    adb logcat -d -s ActivityTaskManager:I ActivityManager:I | grep "START u0" | tail -5 >&2
    return 1
  fi
  echo "android services: started:$(sed 's/.*START u0/ START u0/' <<< "$started")"
  if grep -qE "cmp=$pkg/" <<< "$started"; then
    echo "the URL's intent resolved to the app itself" >&2
    return 1
  fi
  # ...which came to the front.
  for _ in $(seq 20); do
    top="$(resumed)"
    [ -n "$top" ] && ! grep -q "$pkg" <<< "$top" && break
    sleep 1
  done
  echo "android services: on top: $top"
  sleep 2
  adb exec-out screencap -p > "$shots/android-opened-url.png" || true
  if grep -q "$pkg" <<< "$top"; then
    echo "the app is still on top after opening the URL" >&2
    return 1
  fi
  echo "android services: the URL opened in another app"
  adb shell am start -W -n "$pkg/android.app.NativeActivity" >/dev/null
}
url_part || failed="$failed url"
first_run_back

if ! adb shell pidof "$pkg" >/dev/null || log | grep -q "panicked"; then
  echo "the app crashed" >&2
  adb logcat -d | tail -40 >&2
  failed="$failed crash"
fi
if [ -n "$failed" ]; then
  echo "android services: FAILED:$failed" >&2
  exit 1
fi
echo "android services: all checks passed"
