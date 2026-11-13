#!/usr/bin/env bash
# Build the a11y probe (an accessibility service standing in for TalkBack) as
# a signed debug APK, with no Gradle: javac against the platform's android.jar,
# d8, then aapt, zipalign and apksigner, as examples/androiddemo/build-apk.sh.
#
#   .github/a11y/android/build-probe.sh [out.apk]
set -euo pipefail

out="$(realpath -m "${1:-a11yprobe.apk}")"
here="$(cd "$(dirname "$0")" && pwd)"
: "${ANDROID_HOME:?set ANDROID_HOME to the Android SDK}"
build_tools="$(ls -d "$ANDROID_HOME"/build-tools/* | sort -V | tail -1)"
platform_jar="$(ls "$ANDROID_HOME"/platforms/*/android.jar | sort -V | tail -1)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/classes" "$work/dex"
javac -Xlint:-options --release 8 -classpath "$platform_jar" -d "$work/classes" \
  "$here"/src/org/stipple/a11yprobe/*.java
"$build_tools/d8" --min-api 24 --lib "$platform_jar" --output "$work/dex" \
  $(find "$work/classes" -name '*.class')
"$build_tools/aapt" package -f -M "$here/AndroidManifest.xml" -S "$here/res" \
  -I "$platform_jar" -F "$work/base.apk"
(cd "$work/dex" && "$build_tools/aapt" add "$work/base.apk" classes.dex)
"$build_tools/zipalign" -f -p 4 "$work/base.apk" "$work/aligned.apk"
keytool -genkeypair -keystore "$work/debug.ks" -storepass android -keypass android \
  -alias debug -dname "CN=stipple" -keyalg RSA -keysize 2048 -validity 10000 >/dev/null 2>&1
"$build_tools/apksigner" sign --ks "$work/debug.ks" --ks-pass pass:android \
  --key-pass pass:android --out "$out" "$work/aligned.apk"
echo "built $out"
