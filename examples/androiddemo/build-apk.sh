#!/usr/bin/env bash
# Build androiddemo and package it as a signed debug APK, with no Gradle: the
# NDK's clang links the cdylib, and the SDK's build tools (aapt, zipalign,
# apksigner) package and sign it.
#
#   examples/androiddemo/build-apk.sh [x86_64|arm64-v8a] [out.apk]
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
: "${ANDROID_HOME:?set ANDROID_HOME to the Android SDK}"
ndk="${ANDROID_NDK_HOME:-${ANDROID_NDK_LATEST_HOME:-}}"
if [ -z "$ndk" ]; then
  ndk="$(ls -d "$ANDROID_HOME"/ndk/* | sort -V | tail -1)"
fi
linker="$ndk/toolchains/llvm/prebuilt/linux-x86_64/bin/${target}21-clang"
if [ ! -x "$linker" ]; then
  echo "no NDK linker at $linker (set ANDROID_NDK_HOME to an NDK)" >&2
  exit 1
fi
export "CARGO_TARGET_$(echo "$target" | tr 'a-z-' 'A-Z_')_LINKER=$linker"

(cd "$root" && cargo build -p androiddemo --target "$target" --release)
target_dir="$(cd "$root" && cargo metadata --format-version 1 --no-deps \
  | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')"
lib="$target_dir/$target/release/libstipple_android.so"

build_tools="$(ls -d "$ANDROID_HOME"/build-tools/* | sort -V | tail -1)"
platform_jar="$(ls "$ANDROID_HOME"/platforms/*/android.jar | sort -V | tail -1)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/lib/$abi"
cp "$lib" "$work/lib/$abi/"
cp "$here/AndroidManifest.xml" "$work/"
(
  cd "$work"
  "$build_tools/aapt" package -f -M AndroidManifest.xml -I "$platform_jar" -F base.apk
  "$build_tools/aapt" add base.apk "lib/$abi/libstipple_android.so"
  "$build_tools/zipalign" -f -p 4 base.apk aligned.apk
  keytool -genkeypair -keystore debug.ks -storepass android -keypass android \
    -alias debug -dname "CN=stipple" -keyalg RSA -keysize 2048 -validity 10000 >/dev/null 2>&1
  "$build_tools/apksigner" sign --ks debug.ks --ks-pass pass:android --key-pass pass:android \
    --out signed.apk aligned.apk
)
cp "$work/signed.apk" "$out"
echo "built $out ($abi)"
