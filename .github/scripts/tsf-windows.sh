#!/usr/bin/env bash
# Input methods through the Text Services Framework on Windows, with the
# textinput example (see src/platform/backend/windows/tsf/selftest.rs).
#
#   bash .github/scripts/tsf-windows.sh <textinput binary>
#
# 1. TSF: the app activates a thread manager and gives the window a document
#    manager and a context over its text store. Tab focuses the field (the
#    window is tied to its document), then STIPPLE_TSF_SELFTEST plays an
#    input method through the real TSF manager: edit sessions start a
#    composition, write the reading "にほん" and the conversion "日本", and
#    end it. TSF locks the store, writes through ITextStoreACP and calls the
#    composition sink; the field must show each preedit, then the commit.
#    No real IME runs (none can be installed and scripted here), so this
#    checks our side of TSF, not an IME's behaviour.
# 2. IMM32: with STIPPLE_TSF=0 the app must not activate TSF.
set -euo pipefail

app="$1"
out=${OUT_DIR:-visual_output}
mkdir -p "$out"
title="Stipple Text Input"

wait_for() { # pattern file pid
  for _ in $(seq 300); do
    grep -qF "$1" "$2" && return 0
    kill -0 "$3" 2>/dev/null || break
    sleep 0.1
  done
  echo "::error::timed out waiting for '$1' in $2"
  cat "$2"
  return 1
}

check() { # pattern file
  if grep -qF "$1" "$2"; then
    echo "ok: $1"
  else
    echo "::error::missing from $2: $1"
    exit 1
  fi
}

post_tab() {
  powershell -NoProfile -ExecutionPolicy Bypass -File .github/scripts/post-key.ps1 -Title "$title" -Key 9
}

# --- 1. TSF ------------------------------------------------------------------
log="$out/tsf.log"
STIPPLE_TSF_SELFTEST=1 STIPPLE_TEXTINPUT_ECHO=1 "$app" > "$log" 2>&1 &
pid=$!
trap 'kill $pid 2>/dev/null || true' EXIT
post_tab
wait_for 'preedit=Some("日本")' "$log" "$pid"
powershell -NoProfile -ExecutionPolicy Bypass -File .github/scripts/post-key.ps1 -Title "$title" \
  -Screenshot "$out/stipple-windows-tsf-preedit.png" || true
wait_for 'tsf selftest: done' "$log" "$pid"
# The self-test closes the window a second later.
for _ in $(seq 100); do kill -0 "$pid" 2>/dev/null || break; sleep 0.1; done
cat "$log"
check 'stipple-tsf: active (client id' "$log"
check 'stipple-tsf: focus' "$log"
check 'stipple-tsf: composition started' "$log"
check 'textinput: text="" preedit=Some("にほん")' "$log"
check 'textinput: text="" preedit=Some("日本")' "$log"
check 'stipple-tsf: composition ended' "$log"
check 'textinput: text="日本" preedit=None' "$log"
for step in 1 2 3; do check "tsf selftest: step $step: hr=0x0 session=0x0" "$log"; done

# --- 2. IMM32 when TSF is switched off ----------------------------------------
log="$out/tsf-off.log"
STIPPLE_TSF=0 STIPPLE_DEBUG_TSF=1 "$app" > "$log" 2>&1 &
pid=$!
post_tab
wait_for 'stipple-tsf: off (STIPPLE_TSF=0)' "$log" "$pid"
kill "$pid" 2>/dev/null || true
if grep -q 'stipple-tsf: active' "$log"; then
  echo "::error::TSF activated with STIPPLE_TSF=0"
  exit 1
fi
echo "tsf-windows: ok"
