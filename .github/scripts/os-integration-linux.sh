#!/usr/bin/env bash
# traydemo's selftest against a mock desktop on a private session bus. Run
# inside `dbus-run-session`, with DISPLAY pointing at an X server (Xvfb).
#
#   dbus-run-session -- bash .github/scripts/os-integration-linux.sh target/release/traydemo
#
# Checks, from the mock's log and the app's output:
# - the notification reached org.freedesktop.Notifications.Notify;
# - an https URL went to the portal's OpenURI, and a file: URL to xdg-open
#   (a stand-in script on PATH that logs its argument);
# - the StatusNotifierItem registered with the watcher, served its properties
#   and dbusmenu layout, and a host's menu click and activation came back to
#   the app as its Ping command and click action.
set -euo pipefail

app=$(realpath "$1")
out=${OUT_DIR:-.}
mkdir -p "$out"
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)

# xdg-open stand-in.
mkdir -p "$work/bin"
cat > "$work/bin/xdg-open" <<EOF
#!/bin/sh
echo "XDG-OPEN \$1" >> "$work/xdg-open.log"
EOF
chmod +x "$work/bin/xdg-open"
echo "hello" > "$work/note.txt"

python3 "$here/mock-desktop.py" > "$out/mock-desktop.log" 2>&1 &
MOCK=$!
cleanup() { kill "$MOCK" 2>/dev/null || true; }
trap cleanup EXIT
show() {
  echo "----- mock-desktop.log"; cat "$out/mock-desktop.log" || true
  echo "----- traydemo.log"; cat "$out/traydemo.log" || true
  echo "----- xdg-open.log"; cat "$work/xdg-open.log" 2>/dev/null || true
}

for _ in $(seq 50); do
  grep -q "mock desktop ready" "$out/mock-desktop.log" 2>/dev/null && break
  sleep 0.1
done
grep -q "mock desktop ready" "$out/mock-desktop.log"

PATH="$work/bin:$PATH" \
STIPPLE_DEBUG_SHELL=1 \
STIPPLE_SELFTEST_EXPECT_TRAY=1 \
STIPPLE_SELFTEST_URL="https://example.com/stipple-selftest" \
STIPPLE_SELFTEST_FILE="$work/note.txt" \
  timeout 60 "$app" --selftest > "$out/traydemo.log" 2>&1
sleep 1
show

# The app's side.
grep -q "traydemo: tray shown=true" "$out/traydemo.log"
grep -q "traydemo: tray command Ping" "$out/traydemo.log"
grep -q "traydemo: tray ping ok" "$out/traydemo.log"
grep -q "traydemo: tray activate ok" "$out/traydemo.log"
grep -q "traydemo: event loop exited" "$out/traydemo.log"
# The desktop's side.
grep -qE "^NOTIFY app=traydemo summary=Stipple traydemo body=[0-9]+ pings so far" "$out/mock-desktop.log"
grep -q "^OPENURI https://example.com/stipple-selftest" "$out/mock-desktop.log"
grep -q "^REGISTER org.kde.StatusNotifierItem-" "$out/mock-desktop.log"
grep -q "^ITEM id=traydemo title=Stipple tray demo status=Active menu=/MenuBar pixmaps=22x22:1936,32x32:4096,64x64:16384" "$out/mock-desktop.log"
grep -q "^MENU .* _Muted" "$out/mock-desktop.log"
grep -q "^MENU .* Show a _notification" "$out/mock-desktop.log"
grep -q "^CLICKED Ping" "$out/mock-desktop.log"
grep -q "^ACTIVATED" "$out/mock-desktop.log"
grep -qF "XDG-OPEN file://$work/note.txt" "$work/xdg-open.log"
echo "os integration: all checks passed"
