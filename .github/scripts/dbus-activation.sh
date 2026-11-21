#!/usr/bin/env bash
# D-Bus activation end to end (App::dbus_activatable, docs/os-integration.md):
# a private session bus whose service directory holds the .service file
# `cargo stipple linux` would install for instancedemo. `gdbus call` to
# org.freedesktop.Application.Open makes the bus start the app, which takes
# the URI, the activation token and the startup id; ActivateAction reaches
# the running one; a launch from the command line forwards itself over the
# bus. Linux, with a display for the app's window.
#
#   bash .github/scripts/dbus-activation.sh target/release/examples/instancedemo [SERVICE]
#
# SERVICE: the .service file `cargo stipple linux` wrote (and its app
# installed where its Exec says): the bus runs that Exec. Without it, the
# app's path stands in.
set -euo pipefail

ex="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
out_dir="$(mkdir -p "${OUT_DIR:-visual_output}" && cd "${OUT_DIR:-visual_output}" && pwd)"
log="$out_dir/dbus-activation.log"
: >"$log"
work="$(mktemp -d)"
id=org.stipple.InstanceDemo
path=/org/stipple/InstanceDemo

# What the bus runs: the service's Exec (--gapplication-service), quitting
# once it heard three launches, its output in the log.
if [ -n "${2:-}" ]; then
  grep -qx "Name=$id" "$2"
  exec_line="$(sed -n 's/^Exec=//p' "$2")"
else
  exec_line="$ex --gapplication-service"
fi
echo "the service runs: $exec_line"
cat >"$work/run-demo.sh" <<EOF
#!/bin/sh
exec $exec_line --quit-after 3 >>"$log" 2>&1
EOF
chmod +x "$work/run-demo.sh"
mkdir -p "$work/services"
cat >"$work/services/$id.service" <<EOF
[D-BUS Service]
Name=$id
Exec=$work/run-demo.sh
EOF
cat >"$work/session.conf" <<EOF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:path=$work/bus</listen>
  <auth>EXTERNAL</auth>
  <servicedir>$work/services</servicedir>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
EOF
dbus-daemon --config-file="$work/session.conf" --nofork --print-address >"$work/address" &
bus=$!
trap 'kill "$bus" 2>/dev/null || true; rm -rf "$work"' EXIT
for _ in $(seq 50); do [ -S "$work/bus" ] && break; sleep 0.1; done
export DBUS_SESSION_BUS_ADDRESS="unix:path=$work/bus"
# The runner's own runtime directory is shared: a private one.
export XDG_RUNTIME_DIR="$work/run"
mkdir -m 700 "$XDG_RUNTIME_DIR"

echo "--- Open (the bus starts the app)"
timeout 60 gdbus call --session --dest "$id" --object-path "$path" \
  --method org.freedesktop.Application.Open \
  "['file:///tmp/dbus%20one.txt']" \
  "{'activation-token': <'tok-dbus'>, 'desktop-startup-id': <'ci_TIME77'>}"
echo "--- ActivateAction"
timeout 30 gdbus call --session --dest "$id" --object-path "$path" \
  --method org.freedesktop.Application.ActivateAction \
  "new-window" "[]" "{'activation-token': <'tok-2'>}"
echo "--- Introspect"
timeout 30 gdbus introspect --session --dest "$id" --object-path "$path" | tee "$out_dir/dbus-introspect.txt"
grep -q "org.freedesktop.Application" "$out_dir/dbus-introspect.txt"
echo "--- a launch from the command line, forwarded over the bus"
# --dbus-forward skips instancedemo's early forward_launch (the socket), so
# App::run's claim forwards it over the bus.
second="$(cd "$work" && timeout 30 "$ex" --dbus-forward hello 2>&1)"
echo "$second"

for _ in $(seq 100); do grep -q "dbus-forward" "$log" && break; sleep 0.2; done
sleep 1
cat "$log"
grep -q 'instancedemo: launch args=\["/tmp/dbus one.txt"\] cwd=? token=Some("tok-dbus") action=None started_app=true' "$log"
grep -q 'instancedemo: launch args=\[\] cwd=? token=Some("tok-2") action=Some("new-window") started_app=false' "$log"
grep -q "instancedemo: launch args=\[\"--dbus-forward\", \"hello\"\] cwd=$work token=None action=None started_app=false" "$log"
echo "D-Bus activation: the bus started the app, and its calls reached it"
