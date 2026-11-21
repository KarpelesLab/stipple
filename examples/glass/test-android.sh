#!/usr/bin/env bash
# Run glass (a translucent window over the wallpaper) on a device or
# emulator, drawn on the CPU and with each GPU renderer, and check that what
# is behind shows through the same way whichever draws it:
#
#   STIPPLE_APK_EXAMPLE=glass_android STIPPLE_APK_MANIFEST=examples/glass/AndroidManifest.xml \
#     bash examples/androiddemo/build-apk.sh x86_64 glass.apk          # and with
#   STIPPLE_APK_FEATURES=vk ... glass-vk.apk; STIPPLE_APK_FEATURES=gl ... glass-gl.apk
#   examples/glass/test-android.sh out-dir glass.apk [glass-vk.apk Vulkan] [glass-gl.apk "OpenGL ES"]
#
# Each run's screenshot lands in out-dir. The background's pixel (at the left
# edge, in the padding around the card, halfway down) of each GPU run must match the CPU run's
# within a few levels, and the GPU runs must present straight to the window,
# keeping its alpha (a swapchain with INHERIT, an EGL config with alpha),
# not fall back to the window's bitmap.
set -euo pipefail

out="${1:?usage: test-android.sh out-dir cpu.apk [gpu.apk API]...}"
cpu="${2:?usage: test-android.sh out-dir cpu.apk [gpu.apk API]...}"
shift 2
pkg=com.karpeleslab.stipple.glass
mkdir -p "$out"

log() { adb logcat -d -s stipple:I 2>/dev/null || true; }

# Install and start `$1`, wait for its first frame (and with an API `$2`,
# for it presenting with it), screenshot it into `$3`.png and its log.
run() {
  local apk="$1" api="$2" name="$3" i
  adb uninstall "$pkg" >/dev/null 2>&1 || true
  adb install "$apk"
  adb logcat -c
  adb shell am start -W -n "$pkg/android.app.NativeActivity"
  local want="presented=true"
  [ -n "$api" ] && want="stipple: presenting with $api straight to the window"
  for ((i = 0; i < 60; i++)); do
    log | grep -q "$want" && break
    sleep 1
  done
  sleep 3
  adb exec-out screencap -p > "$out/$name.png"
  log > "$out/$name.log"
  adb shell am force-stop "$pkg" || true
  log | grep -q "$want"
}

# The background's pixel as "R G B".
pixel() {
  $(command -v magick || echo convert) "$1" -format '%[fx:round(255*p{8,h/2}.r)] %[fx:round(255*p{8,h/2}.g)] %[fx:round(255*p{8,h/2}.b)]' info:
}

status=0
run "$cpu" "" glass-android-cpu || { echo "glass didn't present on the CPU" >&2; status=1; }
ref=$(pixel "$out/glass-android-cpu.png")
echo "background on the CPU: $ref"
while [ $# -ge 2 ]; do
  apk="$1" api="$2"
  shift 2
  name="glass-android-$(echo "$api" | tr 'A-Z ' 'a-z-')"
  if ! run "$apk" "$api" "$name"; then
    echo "glass didn't present with $api straight to the window" >&2
    grep "stipple:" "$out/$name.log" >&2 || true
    status=1
    continue
  fi
  if grep -q "can't keep a transparent window's alpha\|drawing on the CPU" "$out/$name.log"; then
    echo "$api didn't keep the window's alpha:" >&2
    grep "stipple:" "$out/$name.log" >&2 || true
    status=1
  fi
  got=$(pixel "$out/$name.png")
  echo "background with $api: $got"
  echo "$got $ref" | awk '{ for (i = 1; i <= 3; i++) { d = $i - $(i + 3); if (d < -4 || d > 4) exit 1 } }' || {
    echo "$api's background differs from the CPU's" >&2
    status=1
  }
done
exit "$status"
