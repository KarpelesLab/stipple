#!/usr/bin/env bash
# androidservice on a device or emulator (the one adb talks to; set
# ANDROID_SERIAL to pick one), from the universal APK cargo-stipple writes:
#
#   tools/package/cargo-stipple android --example androidservice \
#       --config examples/androidservice/package.toml
#   examples/androidservice/test-on-device.sh \
#       "target/stipple-package/android/Stipple-service-1.0.0-universal.apk" [log-dir]
#
# 1. The manifest has the fragment's components next to
#    Stipple's activity (`cmd package query-services`, `dumpsys package`).
# 2. A service with no activity: the process isn't running, `am startservice`
#    starts it for the app's Java PingService alone, whose static initializer
#    loads the library; the library's JNI_OnLoad binds the service's native
#    method with RegisterNatives, and the Rust side logs the call, with
#    ANativeActivity_onCreate never run.
# 3. A receiver: `am broadcast` to PingReceiver, whose native method is found
#    by name; the Rust side logs it and its reply comes back as the
#    broadcast's result data.
# 4. The activity still starts in the same process (a Stipple app), and the
#    service then sees it.
#
# Fails if one of them didn't pass.
set -uo pipefail

apk="${1:?usage: test-on-device.sh app-universal.apk [log-dir]}"
out="${2:-visual_output}"
pkg=org.stipple.servicedemo
mkdir -p "$out"
fail=0

log() { adb logcat -d -s androidservice:I stipple:I 2>/dev/null || true; }

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
  log | tail -30 >&2
  return 1
}

check() {
  local what="$1"
  shift
  if "$@"; then
    echo "ok: $what"
  else
    echo "FAILED: $what" >&2
    fail=1
  fi
}

adb install -r -t "$apk" >/dev/null || { echo "install failed" >&2; exit 1; }
adb shell am force-stop "$pkg"
adb logcat -c

# 1. The merged manifest, as the device parsed it.
adb shell dumpsys package "$pkg" > "$out/androidservice-package.txt"
check "the fragment's service is declared" \
  bash -c "adb shell cmd package query-services --components -n $pkg/.PingService | grep -q $pkg/.PingService"
check "the fragment's receiver is declared" \
  bash -c "adb shell cmd package query-receivers --components -n $pkg/.PingReceiver | grep -q $pkg/.PingReceiver"
check "Stipple's NativeActivity is still declared" grep -q "$pkg/android.app.NativeActivity" "$out/androidservice-package.txt"

# 2. The service, in a process started for it alone. Android 8 and later
# don't start a background app's services: the temporary allowlist (what a
# high-priority push message gives an app) lets it.
adb shell cmd deviceidle tempwhitelist "$pkg" >/dev/null 2>&1 || true
adb shell am startservice -n "$pkg/.PingService" --es text boot | tee "$out/androidservice-startservice.txt"
check "JNI_OnLoad ran without the activity" \
  wait_for 'androidservice: JNI_OnLoad \(activity started: false\)' 30
check "the service's native method (RegisterNatives) ran without the activity" \
  wait_for 'androidservice: service started "boot" id=[0-9]+ \(activity started: false\)' 30
check "no activity was created" bash -c "! adb logcat -d -s androidservice:I | grep -q ANativeActivity_onCreate"

# 3. The receiver (an explicit broadcast; -f 0x20, FLAG_INCLUDE_STOPPED_PACKAGES,
# in case the app is in its stopped state).
adb shell am broadcast -f 0x20 -n "$pkg/.PingReceiver" --es text "hello" | tee "$out/androidservice-broadcast.txt"
check "the receiver's native method (by name) ran" \
  wait_for 'androidservice: receiver ping "hello" \(activity started: false\)' 30
check "the receiver's reply came back" grep -q 'data="pong hello"' "$out/androidservice-broadcast.txt"

# 4. The activity, in the same process.
pid_before="$(adb shell pidof "$pkg" | tr -d '\r')"
adb shell am start -W -n "$pkg/android.app.NativeActivity" >/dev/null
check "the activity started" wait_for 'androidservice: activity running' 30
pid_after="$(adb shell pidof "$pkg" | tr -d '\r')"
echo "process before $pid_before, after $pid_after"
check "the activity runs in the service's process" test -n "$pid_before" -a "$pid_before" = "$pid_after"
adb shell am startservice -n "$pkg/.PingService" --es text "again" >/dev/null
check "the service sees the activity" \
  wait_for 'androidservice: service started "again" id=[0-9]+ \(activity started: true\)' 30
adb shell screencap -p "/sdcard/androidservice.png" && adb pull "/sdcard/androidservice.png" "$out/" >/dev/null

log > "$out/androidservice-logcat.log"
adb shell am force-stop "$pkg"
if [ "$fail" != 0 ]; then
  echo "androidservice: some checks failed" >&2
  exit 1
fi
echo "androidservice: all checks passed"
