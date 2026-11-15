#!/usr/bin/env bash
# Build androiddemo and package it as a signed debug APK, with no Gradle: the
# NDK's clang links the cdylib, and the SDK's build tools (aapt, zipalign,
# apksigner) package and sign it. The APK's classes.dex holds Stipple's
# org.stipple.android.Alarm receiver and org.stipple.android.Tapped activity
# (stipple::platform::android_receiver_dex, written here by a host test), for
# scheduled notifications and their taps. With STIPPLE_BILLING=1 it also
# carries the Play Billing Library, for in-app purchases (see
# docs/purchases.md).
#
#   examples/androiddemo/build-apk.sh [x86_64|arm64-v8a] [out.apk]
#
# The app's packaged files, examples/androiddemo/assets/ (or the directory
# STIPPLE_APK_ASSETS names), go into the APK's assets/, where
# stipple::platform::assets reads them through the AAssetManager. Music and
# other already-compressed formats are stored uncompressed (aapt's -0), so
# reading them doesn't inflate them first.
#
# Another example builds the same way: STIPPLE_APK_EXAMPLE names its cdylib
# example (`cameraview_android`), and STIPPLE_APK_MANIFEST its manifest
# (`examples/cameraview/AndroidManifest.xml`); both default to androiddemo's.
# STIPPLE_APK_FEATURES builds it with those Cargo features (`vk`, `gl`: then
# it draws with that GPU API, see examples/androiddemo/test-renderer.sh).
#
# Needs ANDROID_HOME (the SDK, with build-tools and a platform) and an NDK in
# ANDROID_NDK_HOME (or the newest one under $ANDROID_HOME/ndk), plus the Rust
# target (`rustup target add x86_64-linux-android aarch64-linux-android`).
set -euo pipefail

abi="${1:-x86_64}"
out="${2:-stipple.apk}"
case "$abi" in
  x86_64) target=x86_64-linux-android ;;
  arm64-v8a) target=aarch64-linux-android ;;
  *) echo "unknown ABI: $abi (x86_64 or arm64-v8a)" >&2; exit 2 ;;
esac

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
asset_dir="$(cd "${STIPPLE_APK_ASSETS:-$here/assets}" 2>/dev/null && pwd || true)"
example="${STIPPLE_APK_EXAMPLE:-androiddemo}"
manifest="$(cd "$(dirname "${STIPPLE_APK_MANIFEST:-$here/AndroidManifest.xml}")" && pwd)/$(basename "${STIPPLE_APK_MANIFEST:-$here/AndroidManifest.xml}")"
: "${ANDROID_HOME:?set ANDROID_HOME to the Android SDK}"
ndk="${ANDROID_NDK_HOME:-${ANDROID_NDK_LATEST_HOME:-}}"
if [ -z "$ndk" ]; then
  ndk="$(ls -d "$ANDROID_HOME"/ndk/* | sort -V | tail -1)"
fi
host_tag=linux-x86_64
[ "$(uname -s)" = Darwin ] && host_tag=darwin-x86_64
linker="$ndk/toolchains/llvm/prebuilt/$host_tag/bin/${target}21-clang"
if [ ! -x "$linker" ]; then
  echo "no NDK linker at $linker (set ANDROID_NDK_HOME to an NDK)" >&2
  exit 1
fi
export "CARGO_TARGET_$(echo "$target" | tr 'a-z-' 'A-Z_')_LINKER=$linker"

features=()
[ -n "${STIPPLE_APK_FEATURES:-}" ] && features=(--features "$STIPPLE_APK_FEATURES")
(cd "$root" && cargo build --example "$example" --target "$target" --release ${features[@]+"${features[@]}"})
target_dir="$(cd "$root" && cargo metadata --format-version 1 --no-deps \
  | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')"
# The example `androiddemo` is libandroiddemo.so; the manifest names the
# library the NativeActivity loads `stipple_android`.
lib="$target_dir/$target/release/examples/lib$example.so"

build_tools="$(ls -d "$ANDROID_HOME"/build-tools/* | sort -V | tail -1)"
platform_jar="$(ls "$ANDROID_HOME"/platforms/*/android.jar | sort -V | tail -1)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/lib/$abi"
cp "$lib" "$work/lib/$abi/libstipple_android.so"
# The Alarm receiver and Tapped activity, loading libstipple_android.so (the dexdump test writes it
# next to the backend's run-time classes).
(cd "$root" && STIPPLE_DEX_OUT="$work/stipple-classes.dex" \
  cargo test --lib platform::dex::tests::writes_the_classes_for_dexdump >/dev/null)
cp "$work/stipple-classes.receiver.dex" "$work/classes.dex"
cp "$manifest" "$work/AndroidManifest.xml"
# STIPPLE_BILLING=1: the Play Billing Library (and its dependencies) as
# classes2.dex..., and the entries it adds to the manifest, for
# platform::store (tools/android/billing.py fetches it from Google's Maven
# repository; STIPPLE_BILLING_VERSION picks the version).
extra_dex=()
if [ "${STIPPLE_BILLING:-0}" = 1 ]; then
  python3 "$root/tools/android/billing.py" "$work/billing" \
    --version "${STIPPLE_BILLING_VERSION:-8.3.0}" --manifest "$manifest"
  cp "$work/billing/AndroidManifest.xml" "$work/AndroidManifest.xml"
  for dex in "$work"/billing/classes*.dex; do
    cp "$dex" "$work/"
    extra_dex+=("$(basename "$dex")")
  done
fi
(
  cd "$work"
  assets=()
  if [ -d "$asset_dir" ]; then
    assets=(-A "$asset_dir")
    for ext in png jpg jpeg webp ogg mp3 opus flac; do assets+=(-0 "$ext"); done
  fi
  "$build_tools/aapt" package -f -M AndroidManifest.xml -I "$platform_jar" "${assets[@]}" -F base.apk
  "$build_tools/aapt" add base.apk "lib/$abi/libstipple_android.so"
  "$build_tools/aapt" add base.apk classes.dex ${extra_dex[@]+"${extra_dex[@]}"}
  "$build_tools/zipalign" -f -p 4 base.apk aligned.apk
  keytool -genkeypair -keystore debug.ks -storepass android -keypass android \
    -alias debug -dname "CN=stipple" -keyalg RSA -keysize 2048 -validity 10000 >/dev/null 2>&1
  "$build_tools/apksigner" sign --ks debug.ks --ks-pass pass:android --key-pass pass:android \
    --out signed.apk aligned.apk
)
cp "$work/signed.apk" "$out"
echo "built $out ($abi)"
