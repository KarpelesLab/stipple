#!/usr/bin/env bash
# Run cameraview built with a GPU backend on a device or emulator (the one
# adb talks to) and check that the preview draws the camera's own buffers
# with it, with no copy:
#
#   STIPPLE_APK_FEATURES=gl STIPPLE_APK_EXAMPLE=cameraview_android \
#     STIPPLE_APK_MANIFEST=examples/cameraview/AndroidManifest.xml \
#     bash examples/androiddemo/build-apk.sh x86_64 cameraview-gl.apk
#   examples/cameraview/test-gpu.sh cameraview-gl.apk "OpenGL ES" [out-dir]
#
# The second argument is the API's name as Stipple says it ("OpenGL ES",
# "Vulkan"). Runs examples/cameraview/test-on-device.sh (frames with a
# picture in them), then wants the app to draw with that API and the preview
# to take the camera's Yuv420 hardware buffers (GLES: an external texture;
# Vulkan: VK_ANDROID_external_memory_android_hardware_buffer through a
# sampler YCbCr conversion), none refused. On the emulator, Vulkan imports
# them only when told to (debug.stipple.vk_ahb, set here: gfxstream reads
# the camera's three-plane buffers as two-plane NV12, so the picture's
# chroma is scrambled; see src/gpu/vk/exec/ahb.rs). The screenshot lands in
# the directory as stipple-android-camera.png.
set -euo pipefail

apk="${1:?usage: test-gpu.sh cameraview.apk API [out-dir]}"
api="${2:?usage: test-gpu.sh cameraview.apk API [out-dir]}"
out="${3:-visual_output}"
here="$(cd "$(dirname "$0")" && pwd)"
mkdir -p "$out"

# Each APK is signed with a key of its own build: start from no install.
adb uninstall com.karpeleslab.stipple.camera >/dev/null 2>&1 || true
if [ "$api" = Vulkan ]; then
  adb shell setprop debug.stipple.vk_ahb 1
fi
status=0
bash "$here/test-on-device.sh" "$apk" "$out" || status=1
sleep 2
adb exec-out screencap -p > "$out/stipple-android-camera.png" || true
adb logcat -d -s stipple:I > "$out/cameraview-gpu.log" || true
adb shell am force-stop com.karpeleslab.stipple.camera || true
adb shell setprop debug.stipple.vk_ahb 0 || true
grep "stipple: " "$out/cameraview-gpu.log" || true
grep -q "stipple: drawing with $api" "$out/cameraview-gpu.log" || {
  echo "cameraview didn't draw with $api" >&2
  status=1
}
grep -q "the camera preview draws the camera's own Yuv420 buffers on the GPU" "$out/cameraview-gpu.log" || {
  echo "the preview didn't take the camera's buffers" >&2
  status=1
}
if grep -q "didn't import the" "$out/cameraview-gpu.log"; then
  echo "$api refused a camera buffer" >&2
  status=1
fi
exit "$status"
