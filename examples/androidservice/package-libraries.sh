#!/usr/bin/env bash
# Package androidservice with Android libraries on its Java class path: the
# androidx.startup AAR (classes, a manifest with a provider, a string
# resource) and its dependencies, androidx.tracing (an AAR) and
# androidx.annotation (a jar), fetched from Google's Maven repository and
# checked against their SHA-256. libraries/java's PingInitializer is
# compiled against them, and androidx.startup runs it as the process starts
# (test-on-device.sh checks it with LIBRARIES=1).
#
#   examples/androidservice/package-libraries.sh [out-dir] [cargo-stipple args...]
#
# Writes out-dir/package.toml (the example's package.toml with absolute paths,
# libraries/java and libraries/manifest-fragment.xml, and `libraries`) and
# packages with it into out-dir (default target/androidservice-libraries).
# cargo-stipple itself fetches nothing: a library is a file the developer has.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
out="${1:-$root/target/androidservice-libraries}"
[ $# -gt 0 ] && shift
mkdir -p "$out/libs"
out="$(cd "$out" && pwd)"

maven=https://dl.google.com/android/maven2
fetch() {
  local path="$1" sum="$2" file
  file="$out/libs/$(basename "$path")"
  if [ ! -f "$file" ] || ! echo "$sum  $file" | sha256sum -c --status; then
    curl -fsSL --retry 3 -o "$file.part" "$maven/$path"
    echo "$sum  $file.part" | sha256sum -c --status || {
      echo "$path: not the expected file (SHA-256 mismatch)" >&2
      rm -f "$file.part"
      exit 1
    }
    mv "$file.part" "$file"
  fi
}
fetch androidx/startup/startup-runtime/1.1.1/startup-runtime-1.1.1.aar \
  e0a6329a371262fe4c450372b70fdaf33b769ef6917094723787cfce896b1dd3
fetch androidx/tracing/tracing/1.0.0/tracing-1.0.0.aar \
  07b8b6139665b884a162eccf97891ca50f7f56831233bf25168ae04f7b568612
fetch androidx/annotation/annotation/1.1.0/annotation-1.1.0.jar \
  d38d63edb30f1467818d50aaf05f8a692dea8b31392a049bfa991b159ad5b692

cat > "$out/package.toml" <<EOF
# Written by examples/androidservice/package-libraries.sh.
id = "org.stipple.servicedemo"
name = "Stipple service"
version = "1.0.0"
icon = "$root/tools/package/stipple-icon.svg"
icon-background = "#16181d"

[android]
abis = ["arm64-v8a", "x86_64"]
java = ["$here/java", "$here/libraries/java"]
res = "$here/res"
manifest-fragment = "$here/libraries/manifest-fragment.xml"
libraries = "$out/libs"
EOF

cd "$root"
exec tools/package/cargo-stipple android --example androidservice --config "$out/package.toml" \
  --out "$out" "$@"
