#!/usr/bin/env bash
# Run androiddemo built with a GPU backend on a device or emulator and check
# that it draws with it, straight to the window:
#
#   STIPPLE_APK_FEATURES=vk examples/androiddemo/build-apk.sh x86_64 stipple-vk.apk
#   examples/androiddemo/test-renderer.sh stipple-vk.apk Vulkan [screenshot-dir]
#
# The second argument is the API's name as Stipple says it ("Vulkan",
# "OpenGL ES"). Passes when the app logs that it draws with that API and
# presents with it to the window, keeps ticking, and doesn't fall back to the
# CPU; the screenshot lands in the directory.
set -euo pipefail

apk="${1:?usage: test-renderer.sh app.apk API [screenshot-dir]}"
api="${2:?usage: test-renderer.sh app.apk API [screenshot-dir]}"
shots="${3:-visual_output}"
pkg=com.karpeleslab.stipple
mkdir -p "$shots"
name="$(echo "$api" | tr 'A-Z ' 'a-z-')"

log() { adb logcat -d -s stipple:I 2>/dev/null || true; }

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
  log | grep -v "androiddemo: tick" | tail -40 >&2
  return 1
}

adb install -r "$apk" || { adb uninstall "$pkg" && adb install "$apk"; }
adb shell am force-stop "$pkg" || true
adb logcat -c
adb shell am start -W -n "$pkg/android.app.NativeActivity"
status=0
wait_for "stipple: drawing with $api" 60 || status=1
wait_for "stipple: presenting with $api straight to the window" 30 || status=1
wait_for "androiddemo: tick 3 " 30 || status=1
sleep 1
adb exec-out screencap -p > "$shots/android-$name.png" || true
log > "$shots/android-$name.log"
if log | grep -q "drawing on the CPU"; then
  echo "fell back to the CPU:" >&2
  log | grep "stipple:" >&2
  status=1
fi
adb shell am force-stop "$pkg" || true
exit "$status"
