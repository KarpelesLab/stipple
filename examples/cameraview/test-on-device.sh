#!/usr/bin/env bash
# Install cameraview on a device or emulator (the one adb talks to), grant it
# the camera, start it, and check frames arrive that aren't blank:
#
#   examples/cameraview/test-on-device.sh cameraview.apk [screenshot-dir]
#
# The app logs `cameraview: frame N WxH rotation R luma mean M spread S` every
# 30 frames (logcat, tag `stipple`); a spread above 8 is a picture with some
# contrast in it (the emulator's emulated camera shows a moving scene), not a
# black or flat frame. Before granting, the app must report the permission
# not determined, and the camera unopened.
set -euo pipefail

apk="${1:-cameraview.apk}"
out="${2:-visual_output}"
pkg=com.karpeleslab.stipple.camera
mkdir -p "$out"

adb wait-for-device
adb install -r -g "$apk" >/dev/null 2>&1 || adb install -r "$apk"
# Start clean, without the permission, then with it.
adb shell pm revoke "$pkg" android.permission.CAMERA 2>/dev/null || true
adb logcat -c
adb shell am start -W -n "$pkg/android.app.NativeActivity" >/dev/null
sleep 5
adb logcat -d -s stipple:I > "$out/cameraview-denied.log" || true
grep -E "cameraview: lenses .* permission (NotDetermined|Denied)" "$out/cameraview-denied.log"
adb shell am force-stop "$pkg"

adb shell pm grant "$pkg" android.permission.CAMERA
adb logcat -c
adb shell am start -W -n "$pkg/android.app.NativeActivity" >/dev/null
log="$out/cameraview.log"
for _ in $(seq 1 60); do
  adb logcat -d -s stipple:I > "$log" || true
  if grep -qE "cameraview: frame [0-9]+ .* spread (9|[1-9][0-9]+)$" "$log"; then
    break
  fi
  sleep 1
done
adb exec-out screencap -p > "$out/stipple-android-camera.png" || true
echo "=== cameraview log ==="
grep -E "cameraview|stipple: (asking|camera)" "$log" || true
grep -q "cameraview: lenses .* permission Granted" "$log"
grep -q "cameraview: opened" "$log"
# Frames with a picture: a spread of 9 or more.
grep -qE "cameraview: frame [0-9]+ .* spread (9|[1-9][0-9]+)$" "$log"
echo "cameraview: frames arrive, not blank"
