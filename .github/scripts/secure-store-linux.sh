#!/usr/bin/env bash
# platform::secure_store on Linux, against a real Secret Service: GNOME
# Keyring in a private session bus, its keyrings in a throwaway home (never
# the runner's or a developer's own), unlocked with a password on stdin.
#
#   .github/scripts/secure-store-linux.sh target/release/examples/securestore
#
# Checks:
# - with no Secret Service on the bus, the example reports it is
#   unavailable and writes no file;
# - securestore's selftest (set, get, replace, remove) passes;
# - an item set by Stipple is what `secret-tool lookup` reads, and one set
#   by secret-tool with Stipple's attributes is what Stipple reads, each in
#   its own process;
# - with the keyring locked and no display for its unlock prompt, a read
#   fails as Locked instead of hanging;
# - with SECURE_STORE_CARGO_TEST=1, the crate's own round-trip test
#   (`cargo test`, run from the repository, with STIPPLE_SECRET_SERVICE_TEST=1).
set -euo pipefail

app=$(realpath "$1")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/home" "$work/run"
chmod 700 "$work/run"
# Cargo and rustup stay where they are.
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
export HOME="$work/home" XDG_DATA_HOME="$work/home/.local/share" XDG_RUNTIME_DIR="$work/run"
# No display: an unlock prompt can't show anywhere.
unset DISPLAY WAYLAND_DISPLAY XAUTHORITY GNOME_KEYRING_CONTROL
export STIPPLE_DEBUG_SECURE_STORE=1

echo "== no Secret Service on the bus"
# A bus with nothing on it (no activatable services either).
cat > "$work/empty-bus.conf" <<EOF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:tmpdir=/tmp</listen>
  <servicedir>$work/no-services</servicedir>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
EOF
mkdir -p "$work/no-services"
dbus-run-session --config-file="$work/empty-bus.conf" -- bash -c '
  out=$("$1" set token value 2>&1) && { echo "set succeeded without a Secret Service: $out"; exit 1; }
  echo "$out"
  echo "$out" | grep -q "secure storage unavailable: no Secret Service"
' bash "$app"
if [ -n "$(find "$HOME" -type f)" ]; then
  echo "files were written instead:" >&2
  find "$HOME" -type f >&2
  exit 1
fi

echo "== GNOME Keyring in a private session"
dbus-run-session -- bash -c '
  set -euo pipefail
  app="$1"
  printf stipple-test | gnome-keyring-daemon --unlock --components=secrets
  "$app" selftest
  "$app" set token "set by stipple"
  [ "$(secret-tool lookup service org.stipple.securestore account token)" = "set by stipple" ]
  printf "set by secret-tool" | secret-tool store --label=other \
    xdg:schema org.stipple.SecureStore service org.stipple.securestore account other
  [ "$("$app" get other)" = "set by secret-tool" ]
  "$app" remove token
  "$app" remove other
  [ "$("$app" get token)" = none ]
  if secret-tool lookup service org.stipple.securestore account token; then exit 1; fi
  echo "secret-tool interop passed"

  "$app" set locked "behind the lock"
  dbus-send --session --dest=org.freedesktop.secrets --print-reply /org/freedesktop/secrets \
    org.freedesktop.Secret.Service.Lock array:objpath:/org/freedesktop/secrets/collection/login
  out=$(timeout 60 "$app" get locked 2>&1) && { echo "read a locked item: $out"; exit 1; }
  echo "$out"
  echo "$out" | grep -q "the secure store is locked"
  echo "locked keyring passed"
' bash "$app"

if [ "${SECURE_STORE_CARGO_TEST:-}" = 1 ]; then
  echo "== the crate's round-trip test"
  dbus-run-session -- bash -c '
    set -euo pipefail
    printf stipple-test | gnome-keyring-daemon --unlock --components=secrets
    STIPPLE_SECRET_SERVICE_TEST=1 cargo test --lib platform::secure_store -- --nocapture 2>&1 | tee "$1/cargo-test.log"
    grep -q "round_trip_against_the_session_secret_service ... ok" "$1/cargo-test.log"
    grep -q "Secret Service session" "$1/cargo-test.log"
  ' bash "$work"
fi
echo "secure store checks passed"
