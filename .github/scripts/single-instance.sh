#!/usr/bin/env bash
# App::single_instance end to end (docs/os-integration.md): two launches of
# instancedemo. The first owns the id and quits once it has heard one
# launch; the second, from another directory, with arguments and a startup
# id, hands them over and exits at once. Linux, macOS and Windows (Git Bash).
#
#   bash .github/scripts/single-instance.sh target/release/examples/instancedemo
set -euo pipefail

ex="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
# macOS has no `timeout`; perl's alarm stands in for it.
run() {
  if command -v timeout >/dev/null; then timeout "$@"; else perl -e 'alarm shift; exec @ARGV' "$@"; fi
}
out_dir="${OUT_DIR:-visual_output}"
mkdir -p "$out_dir"
log="$out_dir/single-instance.log"

run 60 "$ex" --quit-after 1 >"$log" 2>&1 &
first=$!
sleep 3
dir="$(mktemp -d)"
second="$(cd "$dir" && DESKTOP_STARTUP_ID=ci_TIME42 run 30 "$ex" hello "two words" 2>&1)"
echo "second launch: $second"
grep -q "forwarded to the running instance" <<<"$second"
# The first heard it, and quit.
wait "$first"
cat "$log"
grep -q 'instancedemo: launch args=\["hello", "two words"\]' "$log"
grep -q "cwd=.*$(basename "$dir")" "$log"
grep -q 'token=Some("ci_TIME42")' "$log"
echo "single instance: the second launch reached the first"
