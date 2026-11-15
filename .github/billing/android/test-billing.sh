#!/usr/bin/env bash
# In-app purchases on Android, end to end, against a stand-in for the Play
# Billing Library (src/com/android/billingclient/api: the real library's
# signatures, answering from memory). The androiddemo APK (built by
# examples/androiddemo/build-apk.sh, without the library) gets the stand-in
# as classes2.dex, is signed again, installed and started; its log must show
# platform::store's whole path through the JNI driver
# (src/platform/backend/android/billing.rs):
#
# - the client built through the library's builders (pending purchases on)
#   and connected, its listener a java.lang.reflect.Proxy over Stipple's
#   native InvocationHandler;
# - at launch, the purchases left unfinished handed to App::on_transaction
#   (an unacknowledged one, a pending one), the finished ones not;
# - a consumable and a subscription's details (the subscription's base plan,
#   an emoji in a name), then a purchase: launchBillingFlow, the transaction
#   to on_transaction and to the purchase's callback;
# - finishing: the consumable consumed, the other acknowledged.
#
#   .github/billing/android/test-billing.sh stipple.apk [screenshot-dir]
#
# Needs ANDROID_HOME, javac, and a running emulator or device (adb).
set -euo pipefail

apk="${1:?usage: test-billing.sh stipple.apk [screenshot-dir]}"
apk="$(cd "$(dirname "$apk")" && pwd)/$(basename "$apk")"
shots="${2:-visual_output}"
pkg=com.karpeleslab.stipple
here="$(cd "$(dirname "$0")" && pwd)"
: "${ANDROID_HOME:?set ANDROID_HOME to the Android SDK}"
build_tools="$(ls -d "$ANDROID_HOME"/build-tools/* | sort -V | tail -1)"
platform_jar="$(ls "$ANDROID_HOME"/platforms/*/android.jar | sort -V | tail -1)"
mkdir -p "$shots"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# The stand-in library, as classes2.dex.
mkdir -p "$work/classes" "$work/dex"
javac -Xlint:-options -nowarn --release 8 -classpath "$platform_jar" -d "$work/classes" \
  "$here"/src/com/android/billingclient/api/*.java
"$build_tools/d8" --min-api 24 --lib "$platform_jar" --output "$work/dex" \
  $(find "$work/classes" -name '*.class')
cp "$apk" "$work/billing.apk"
cp "$work/dex/classes.dex" "$work/classes2.dex"
(cd "$work" && "$build_tools/aapt" remove billing.apk META-INF/MANIFEST.MF META-INF/CERT.SF \
  META-INF/CERT.RSA >/dev/null 2>&1 || true)
(cd "$work" && "$build_tools/aapt" add billing.apk classes2.dex >/dev/null)
"$build_tools/zipalign" -f -p 4 "$work/billing.apk" "$work/aligned.apk"
keytool -genkeypair -keystore "$work/debug.ks" -storepass android -keypass android \
  -alias debug -dname "CN=stipple" -keyalg RSA -keysize 2048 -validity 10000 >/dev/null 2>&1
"$build_tools/apksigner" sign --ks "$work/debug.ks" --ks-pass pass:android \
  --key-pass pass:android --out "$work/signed.apk" "$work/aligned.apk"

adb uninstall "$pkg" >/dev/null 2>&1 || true
adb install "$work/signed.apk"
adb logcat -c
adb shell am start -W -n "$pkg/android.app.NativeActivity"

log() { adb logcat -d -s stipple:I fakebilling:I 2>/dev/null || true; }
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
  log | grep -v "androiddemo: tick" | tail -60 >&2
  adb logcat -d | grep -E "AndroidRuntime|JNI DETECTED|F DEBUG" | tail -30 >&2 || true
  adb exec-out screencap -p > "$shots/android-billing-timeout.png" || true
  return 1
}

wait_for "fakebilling: build pending=true listener=true" 60
wait_for "stipple: billing: connected to Google Play"
# Unfinished at launch: the unacknowledged purchase and the pending one.
wait_for "androiddemo: transaction stipple_demo_interrupted Purchased GooglePlay id=GPA\.1111 token=token-interrupted kind=NonConsumable"
wait_for "androiddemo: transaction stipple_demo_pending Pending GooglePlay id=token-pending"
# Not knowing what it is, finishing acknowledges it (never consumes it).
wait_for "fakebilling: acknowledge token-interrupted"
# The products, in the user's currency, a subscription at its base plan.
wait_for "androiddemo: store products 2"
wait_for "androiddemo: product stipple_demo_gems Consumable \"Name of stipple_demo_gems .+\" ¥120 120000000 JPY \"\""
wait_for "androiddemo: product stipple_demo_vip Subscription \"Name of stipple_demo_vip .+\" \\\$4\.99 4990000 USD \"P1M\""
# The purchase, its transaction, consumed once granted.
wait_for "fakebilling: launch stipple_demo_gems offer=null activity=true"
wait_for "androiddemo: transaction stipple_demo_gems Purchased GooglePlay id=GPA\.0000-1 token=token-stipple_demo_gems-1 kind=Consumable"
wait_for "androiddemo: purchase Purchased\(Transaction \{ product_id: \"stipple_demo_gems\""
wait_for "fakebilling: consume token-stipple_demo_gems-1"
# Finished ones stay finished.
if log | grep -qE "androiddemo: transaction stipple_demo_(old|vip)"; then
  echo "an acknowledged purchase was handed back" >&2
  log | grep "androiddemo: transaction" >&2
  exit 1
fi
if adb logcat -d | grep -qE "JNI DETECTED|FATAL EXCEPTION"; then
  adb logcat -d | grep -E "JNI DETECTED|FATAL EXCEPTION|AndroidRuntime" | tail -30 >&2
  exit 1
fi
echo "android: in-app purchases ok"
