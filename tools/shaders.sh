#!/bin/sh
# Compile the Vulkan shaders (src/gpu/shaders/*.vert, *.frag) to the SPIR-V
# checked in next to them, or with --check, fail if what's checked in isn't
# what they compile to.
#
# The compiler is glslang's standalone `glslang`, at the release pinned below,
# built without the optimizer (ENABLE_OPT=OFF), so the bytes are the same on
# every machine. It's a tool for this script and CI only, never a dependency
# of the crate. GLSLANG (the path to the binary) overrides `glslang` on PATH.
#
#   git clone --depth 1 --branch 16.6.0 https://github.com/KhronosGroup/glslang
#   cmake -S glslang -B build -DCMAKE_BUILD_TYPE=Release -DENABLE_OPT=OFF \
#     -DGLSLANG_TESTS=OFF -DENABLE_HLSL=OFF
#   cmake --build build --target glslang-standalone
#   GLSLANG=build/StandAlone/glslang tools/shaders.sh
set -eu

PINNED="16.6.0"
GLSLANG="${GLSLANG:-glslang}"
DIR="$(cd "$(dirname "$0")/../src/gpu/shaders" && pwd)"

version=$("$GLSLANG" --version | sed -n 's/^Glslang Version: [0-9]*://p')
if [ "$version" != "$PINNED" ]; then
    echo "tools/shaders.sh: glslang $PINNED is pinned, $GLSLANG is ${version:-unknown}" >&2
    exit 1
fi

check=0
[ "${1:-}" = "--check" ] && check=1
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

drift=0
for src in "$DIR"/*.vert "$DIR"/*.frag; do
    name=$(basename "$src")
    out="$tmp/$name.spv"
    # Compile from the directory with a bare name: nothing of the path ends
    # up in the module.
    (cd "$DIR" && "$GLSLANG" -V --target-env vulkan1.0 --quiet -o "$out" "$name")
    if [ "$check" = 1 ]; then
        if ! cmp -s "$out" "$DIR/$name.spv"; then
            echo "src/gpu/shaders/$name.spv is stale: run tools/shaders.sh" >&2
            drift=1
        fi
    else
        cp "$out" "$DIR/$name.spv"
    fi
done
exit "$drift"
