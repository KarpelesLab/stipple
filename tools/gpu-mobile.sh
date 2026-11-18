#!/usr/bin/env bash
# The GPU renderer's comparison corpus (src/gpu/compare.rs) and the benchmark
# (examples/bench) on a phone, an emulator or the iOS simulator. Both draw
# offscreen, so they run as plain executables: spawned in the simulator
# (`xcrun simctl spawn`), or pushed and run over `adb` on Android.
#
#   tools/gpu-mobile.sh ios-sim corpus             # Metal against the CPU
#   tools/gpu-mobile.sh ios-sim bench [ARGS...]    # bench --renderer metal ARGS
#   tools/gpu-mobile.sh android corpus [gl|vk]     # GLES or Vulkan (default gl)
#   tools/gpu-mobile.sh android bench [ARGS...]    # bench --renderer gpu ARGS
#
# ARGS go to the benchmark (`--renderer software`, `--only transition`,
# `--quick`, ...); its JSON (`--json FILE`, a path on this machine) comes
# back here. The corpus's frames go to STIPPLE_GPU_DUMP when it's set.
#
# iOS: the simulator STIPPLE_SIM names (a UDID), else the first booted
# iPhone, else the first available iPhone simulator, booted for the run. A
# physical iPhone has no shell to run an executable from (only signed apps
# run there), so this covers the simulator, whose Metal is the Mac's GPU.
#
# Android: the device `adb` sees (ANDROID_SERIAL picks one), an emulator or
# a phone with USB debugging; its ABI picks the target (arm64-v8a or
# x86_64). Needs ANDROID_HOME and an NDK (ANDROID_NDK_HOME, or the newest
# under $ANDROID_HOME/ndk) for the linker.
#
# Needs the Rust target (`rustup target add aarch64-apple-ios-sim`,
# `aarch64-linux-android`, `x86_64-linux-android`). CARGO overrides the
# `cargo` used (one whose toolchain has the target's standard library).
# Exits non-zero when a corpus test fails or the GPU isn't there
# (STIPPLE_REQUIRE_GPU is set for the corpus).
set -euo pipefail

platform="${1:-}"
what="${2:-}"
shift 2 2>/dev/null || true
root="$(cd "$(dirname "$0")/.." && pwd)"
cargo="${CARGO:-cargo}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

usage() {
  sed -n '7,10p' "$0" | sed 's/^# //' >&2
  exit 2
}

# Build with cargo's JSON messages and print the executable it made.
build() {
  (cd "$root" && "$cargo" "$@" --message-format=json-render-diagnostics) \
    | python3 -c '
import json, sys
exe = ""
for line in sys.stdin:
    try:
        m = json.loads(line)
    except ValueError:
        continue
    if m.get("executable"):
        exe = m["executable"]
print(exe)'
}

# The corpus's log must end with a passing result that ran the comparisons.
check_corpus() {
  local log="$1"
  if ! grep -q '^test result: ok\.' "$log"; then
    echo "gpu-mobile: the corpus failed" >&2
    return 1
  fi
  if ! grep -q 'gpu::compare::boxes_match_the_software_renderer \.\.\. .*Diff' "$log"; then
    echo "gpu-mobile: the comparisons didn't run (built without the GPU API?)" >&2
    return 1
  fi
  echo "gpu-mobile: corpus ok ($(grep -c '^test gpu::compare::' "$log") comparison tests)"
}

ios_sim() {
  local dev="${STIPPLE_SIM:-}"
  if [ -z "$dev" ]; then
    dev=$(xcrun simctl list devices -j | python3 -c '
import json, sys
d = json.load(sys.stdin)["devices"]
devs = [x for rt, ds in d.items() if "iOS" in rt for x in ds
        if x.get("isAvailable") and "iPhone" in x["name"]]
booted = [x for x in devs if x["state"] == "Booted"]
print((booted or devs or [{"udid": ""}])[0]["udid"])')
  fi
  [ -n "$dev" ] || { echo "gpu-mobile: no iPhone simulator" >&2; exit 1; }
  xcrun simctl boot "$dev" 2>/dev/null || true
  xcrun simctl bootstatus "$dev" -b >/dev/null
  echo "gpu-mobile: simulator $dev" >&2
  local target=aarch64-apple-ios-sim
  case "$what" in
    corpus)
      local exe
      exe=$(build test --no-run --lib --features mtl --target "$target")
      [ -n "$exe" ] || { echo "gpu-mobile: no test executable" >&2; exit 1; }
      # The simulator passes SIMCTL_CHILD_* on to the process, without the
      # prefix; it sees this Mac's file system.
      export SIMCTL_CHILD_STIPPLE_REQUIRE_GPU=1
      if [ -n "${STIPPLE_GPU_DUMP:-}" ]; then
        mkdir -p "$STIPPLE_GPU_DUMP"
        SIMCTL_CHILD_STIPPLE_GPU_DUMP="$(cd "$STIPPLE_GPU_DUMP" && pwd)"
        export SIMCTL_CHILD_STIPPLE_GPU_DUMP
      fi
      xcrun simctl spawn "$dev" "$exe" gpu:: app_gpu --nocapture --test-threads 1 2>&1 \
        | tee "$work/corpus.log"
      check_corpus "$work/corpus.log"
      ;;
    bench)
      local exe
      exe=$(build build --release --example bench --features mtl --target "$target")
      [ -n "$exe" ] || { echo "gpu-mobile: no bench executable" >&2; exit 1; }
      # The bench reads a flag's first value, so these come after the
      # caller's: `--renderer software` measures the CPU.
      local args=("$@" --renderer metal --runner ios-sim)
      # A relative --json path is relative to here, not the simulator's cwd.
      local i
      for i in "${!args[@]}"; do
        if [ "${args[$i]}" = --json ] && [ -n "${args[$((i + 1))]:-}" ]; then
          local j="${args[$((i + 1))]}"
          case "$j" in /*) ;; *) args[$((i + 1))]="$PWD/$j" ;; esac
        fi
      done
      xcrun simctl spawn "$dev" "$exe" "${args[@]}"
      ;;
    *) usage ;;
  esac
}

android() {
  local adb="adb"
  command -v adb >/dev/null || adb="${ANDROID_HOME:?set ANDROID_HOME}/platform-tools/adb"
  # A device that drops off (an emulator that died) would hang every adb
  # command after: give it two minutes to come back.
  if command -v timeout >/dev/null; then
    timeout 120 "$adb" wait-for-device || { echo "gpu-mobile: no device answers adb" >&2; exit 1; }
  else
    "$adb" wait-for-device
  fi
  local abi target
  abi=$("$adb" shell getprop ro.product.cpu.abi | tr -d '\r')
  case "$abi" in
    arm64-v8a) target=aarch64-linux-android ;;
    x86_64) target=x86_64-linux-android ;;
    *) echo "gpu-mobile: unsupported ABI $abi" >&2; exit 1 ;;
  esac
  local ndk="${ANDROID_NDK_HOME:-${ANDROID_NDK_LATEST_HOME:-}}"
  [ -n "$ndk" ] || ndk="$(ls -d "${ANDROID_HOME:?set ANDROID_HOME}"/ndk/* | sort -V | tail -1)"
  local host_tag=linux-x86_64
  [ "$(uname -s)" = Darwin ] && host_tag=darwin-x86_64
  local linker="$ndk/toolchains/llvm/prebuilt/$host_tag/bin/${target}21-clang"
  [ -x "$linker" ] || { echo "gpu-mobile: no NDK linker at $linker" >&2; exit 1; }
  export "CARGO_TARGET_$(echo "$target" | tr 'a-z-' 'A-Z_')_LINKER=$linker"
  echo "gpu-mobile: $("$adb" shell getprop ro.product.model | tr -d '\r') ($abi, API $("$adb" shell getprop ro.build.version.sdk | tr -d '\r'))" >&2
  local dir=/data/local/tmp/stipple-gpu
  "$adb" shell "rm -rf $dir && mkdir -p $dir"
  case "$what" in
    corpus)
      local api="${1:-gl}"
      case "$api" in gl | vk) ;; *) usage ;; esac
      local exe
      exe=$(build test --no-run --lib --features "$api" --target "$target")
      [ -n "$exe" ] || { echo "gpu-mobile: no test executable" >&2; exit 1; }
      "$adb" push "$exe" "$dir/stipple-tests" >/dev/null
      "$adb" shell "cd $dir && chmod 755 stipple-tests && mkdir -p dump && \
        STIPPLE_REQUIRE_GPU=1 STIPPLE_GPU_DUMP=$dir/dump TMPDIR=$dir \
        ./stipple-tests gpu:: app_gpu --nocapture --test-threads 1" 2>&1 \
        | tee "$work/corpus.log" || true
      if [ -n "${STIPPLE_GPU_DUMP:-}" ]; then
        mkdir -p "$STIPPLE_GPU_DUMP"
        "$adb" pull "$dir/dump/." "$STIPPLE_GPU_DUMP" >/dev/null || true
      fi
      "$adb" shell "rm -rf $dir" || true
      check_corpus "$work/corpus.log"
      ;;
    bench)
      local exe
      exe=$(build build --release --example bench --features gl,vk --target "$target")
      [ -n "$exe" ] || { echo "gpu-mobile: no bench executable" >&2; exit 1; }
      "$adb" push "$exe" "$dir/bench" >/dev/null
      local args=() json=""
      local a prev=""
      for a in "$@"; do
        if [ "$prev" = --json ]; then
          json="$a"
          a="$dir/out.json"
        fi
        args+=("$a")
        prev="$a"
      done
      # After the caller's: the bench reads a flag's first value.
      args+=(--renderer gpu --runner android)
      "$adb" shell "cd $dir && chmod 755 bench && ./bench ${args[*]}"
      [ -z "$json" ] || "$adb" pull "$dir/out.json" "$json" >/dev/null
      "$adb" shell "rm -rf $dir" || true
      ;;
    *) usage ;;
  esac
}

case "$platform" in
  ios-sim) ios_sim "$@" ;;
  android) android "$@" ;;
  *) usage ;;
esac
