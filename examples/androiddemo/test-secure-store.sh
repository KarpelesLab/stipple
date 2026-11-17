#!/usr/bin/env bash
# platform::secure_store on a device or emulator (the one adb talks to; set
# ANDROID_SERIAL to pick one), with androiddemo's APK:
#
#   examples/androiddemo/test-secure-store.sh stipple.apk [log-dir]
#
# 1. A fresh install: nothing stored yet, then a round trip of every byte
#    value through the AndroidKeyStore key (set, get, remove) passes, and
#    the launch count is stored.
# 2. The stored value is a sealed file in the app's no-backup directory
#    (`SSv1`, the Keystore's IV, AES-GCM ciphertext): the plain text isn't
#    in it.
# 3. After the process is killed and started again, the value comes back:
#    the Keystore key outlives the process.
# 4. A file changed behind the app's back doesn't open: the value reads as
#    Corrupt (GCM's tag), and the app stores a new one over it.
set -uo pipefail

apk="${1:?usage: test-secure-store.sh stipple.apk [log-dir]}"
logs="${2:-visual_output}"
pkg=com.karpeleslab.stipple
mkdir -p "$logs"

log() { adb logcat -d -s stipple:I 2>/dev/null || true; }

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
  log | grep -E "secure store|AndroidRuntime" | tail -20 >&2
  return 1
}

failed=0
adb uninstall "$pkg" >/dev/null 2>&1 || true
adb install "$apk" >/dev/null
adb logcat -c
adb shell am start -W -n "$pkg/android.app.NativeActivity" >/dev/null

echo "== a fresh install: nothing stored, then a round trip"
wait_for "androiddemo: secure store previous=Ok\(None\)" 60 || failed=1
wait_for "androiddemo: secure store launch=1 set=Ok\(\(\)\) round_trip=Ok\(true\) removed=Ok\(true\)" || failed=1
log | grep -E "secure store" > "$logs/android-secure-store.log"

echo "== the value is sealed in the no-backup directory"
# The service's directory is the package name in hex.
svc="$(printf '%s' "$pkg" | od -An -tx1 | tr -d ' \n')"
key="$(printf '%s' androiddemo-launches | od -An -tx1 | tr -d ' \n')"
file="no_backup/stipple-secure-store/$svc/$key.sec"
adb shell run-as "$pkg" ls -l "$file" || failed=1
head="$(adb exec-out run-as "$pkg" cat "$file" | head -c 4)"
echo "magic: $head"
[ "$head" = SSv1 ] || { echo "not a sealed file" >&2; failed=1; }
if adb exec-out run-as "$pkg" cat "$file" | grep -qa "androiddemo secret"; then
  echo "the plain text is in the file" >&2
  failed=1
fi
# Nothing else of the store's is left behind (the round trip's file, a .tmp).
adb shell run-as "$pkg" ls "no_backup/stipple-secure-store/$svc"

echo "== killed and started again: the value comes back"
adb shell am force-stop "$pkg"
adb logcat -c
adb shell am start -W -n "$pkg/android.app.NativeActivity" >/dev/null
wait_for 'androiddemo: secure store previous=Ok\(Some\("androiddemo secret launch 1"\)\)' 60 || failed=1
wait_for "androiddemo: secure store launch=2 set=Ok\(\(\)\) round_trip=Ok\(true\)" || failed=1
log | grep -E "secure store" >> "$logs/android-secure-store.log"
adb shell am force-stop "$pkg"

echo "== a changed file reads as corrupt"
adb shell run-as "$pkg" sh -c "'printf x >> $file'"
adb logcat -c
adb shell am start -W -n "$pkg/android.app.NativeActivity" >/dev/null
wait_for "androiddemo: secure store previous=Err\(Corrupt\)" 60 || failed=1
wait_for "androiddemo: secure store launch=1 set=Ok\(\(\)\)" || failed=1
log | grep -E "secure store" >> "$logs/android-secure-store.log"
adb shell am force-stop "$pkg"

if [ "$failed" != 0 ]; then
  echo "secure store checks failed" >&2
  exit 1
fi
echo "secure store checks passed"
