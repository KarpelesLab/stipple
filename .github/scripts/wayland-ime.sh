#!/usr/bin/env bash
# A real input method behind zwp_text_input_v3, under a running sway.
#
#   bash .github/scripts/wayland-ime.sh <textinput binary>
#
# Needs WAYLAND_DISPLAY (sway's socket) and XDG_RUNTIME_DIR set, and fcitx5
# with fcitx5-mozc, fcitx5-chinese-addons and fcitx5-hangul installed. fcitx5 connects to
# sway as an input method (zwp_input_method_v2 and a virtual keyboard);
# sway relays between it and the app's text-input-v3. wtype's keys reach
# fcitx5 through its keyboard grab while the app's field is enabled.
#
# 1. Mozc: "nihon" must arrive as a hiragana preedit, Space converts it
#    (the preedit becomes 日本) and Return commits it; then "ka" and Escape
#    must cancel the composition without committing anything.
# 2. Pinyin: "nihao" then Space must commit 你好 (fcitx5's pinyin shows the
#    syllables as the preedit until then).
# 3. Hangul: "gksrmf" (dubeolsik) composes 한 then 글 syllable by syllable:
#    한 is committed when the next syllable starts, 글 stays the preedit
#    until Space commits it.
#
# OUT_DIR (default visual_output) gets the logs and screenshots (grim).
# RUN_PREFIX, if set, is put in front of the app's command line.
set -uo pipefail

app="$1"
out=${OUT_DIR:-visual_output}
mkdir -p "$out"
out=$(cd "$out" && pwd)
fail=0

# An isolated configuration: an input method group with the keyboard layout
# and the engine under test, active as soon as a text field takes focus.
ime_home=$(mktemp -d)
export HOME="$ime_home" XDG_CONFIG_HOME="$ime_home/.config"
export LANG=C.UTF-8 LC_ALL=C.UTF-8
unset DISPLAY GTK_IM_MODULE QT_IM_MODULE XMODIFIERS
mkdir -p "$XDG_CONFIG_HOME/fcitx5/conf"
cat > "$XDG_CONFIG_HOME/fcitx5/config" <<'EOF'
[Hotkey/TriggerKeys]
0=Control+space

[Behavior]
ActiveByDefault=True
ShareInputState=No
ShowInputMethodInformation=False
EOF

profile() { # engine
  cat > "$XDG_CONFIG_HOME/fcitx5/profile" <<EOF
[Groups/0]
Name=Default
Default Layout=us
DefaultIM=$1

[Groups/0/Items/0]
Name=keyboard-us
Layout=

[Groups/0/Items/1]
Name=$1
Layout=

[GroupOrder]
0=Default
EOF
}

check() { # pattern file
  if grep -qF -- "$1" "$2"; then
    echo "ok: $1"
  else
    echo "::error::missing from $(basename "$2"): $1"
    fail=1
  fi
}

# Starts fcitx5 with the given engine and waits for it to bind sway's input
# method; sets FCITX.
start_fcitx() { # engine log
  profile "$1"
  fcitx5 --disable=xcb,xim,classicui,kimpanel,notificationitem,notifications,virtualkeyboard \
    --verbose='*=4,waylandim=5,wayland=5' > "$2" 2>&1 &
  FCITX=$!
  # The engine's addon loads at startup (Mozc's starts mozc_server first).
  for _ in $(seq 300); do
    grep -q "Loaded addon $1\$" "$2" && break
    sleep 0.1
  done
  grep -q "Loaded addon $1\$" "$2" || echo "::warning::fcitx5 hasn't loaded $1 after 30 s"
  # Let it bind sway's input method.
  sleep 2
}

stop() { # pid...
  for p in "$@"; do kill "$p" 2>/dev/null || true; done
  for p in "$@"; do wait "$p" 2>/dev/null || true; done
}

# Runs the app, focuses its field with Tab, types the given wtype arguments
# (a session each, with screenshots in between), and stops it.
run_app() { # log shot-prefix -- wtype-args... [-- wtype-args...]
  local log="$1" shot="$2"
  shift 3
  STIPPLE_WAYLAND_DEBUG=1 STIPPLE_TEXTINPUT_ECHO=1 ${RUN_PREFIX:-} "$app" > "$log" 2>&1 &
  local pid=$!
  sleep 3
  # One virtual keyboard for Tab; the field enables text-input on focus,
  # fcitx5 activates and grabs the keyboard.
  wtype -s 500 -k Tab
  sleep 1.5
  local n=0 args=()
  for a in "$@" --; do
    if [ "$a" = -- ]; then
      if [ ${#args[@]} -gt 0 ]; then
        wtype -s 500 -d 150 "${args[@]}"
        sleep 1.5
        n=$((n + 1))
        grim "$out/$shot-$n.png" 2>/dev/null || true
      fi
      args=()
    else
      args+=("$a")
    fi
  done
  stop "$pid"
}

# --- 1. Mozc -----------------------------------------------------------------
start_fcitx mozc "$out/fcitx5-mozc.log"
log="$out/wl-ime-mozc.log"
run_app "$log" wayland-ime-mozc -- nihon -- -k space -- -k Return -- ka -- -k Escape
stop "$FCITX"
echo "=== wl-ime-mozc.log ==="; grep -v ' key \|present ' "$log" || true
check 'text-input: enabled' "$log"
check 'textinput: text="" preedit=Some("にほ' "$log"
check 'textinput: text="" preedit=Some("日本")' "$log"
check 'textinput: text="日本" preedit=None' "$log"
# Escape cancels a composition: the preedit goes, nothing is committed.
check 'textinput: text="日本" preedit=Some("か")' "$log"
last=$(grep 'textinput: ' "$log" | tail -1)
if [ "$last" = 'textinput: text="日本" preedit=None' ]; then
  echo "ok: Escape cancelled the preedit"
else
  echo "::error::after Escape: $last"
  fail=1
fi

# --- 2. Pinyin ---------------------------------------------------------------
start_fcitx pinyin "$out/fcitx5-pinyin.log"
log="$out/wl-ime-pinyin.log"
run_app "$log" wayland-ime-pinyin -- nihao -- -k space
stop "$FCITX"
echo "=== wl-ime-pinyin.log ==="; grep -v ' key \|present ' "$log" || true
check 'textinput: text="" preedit=Some("ni' "$log"
check 'textinput: text="你好" preedit=None' "$log"
# The field's text and caret reach the input method (set_surrounding_text,
# relayed by sway); fcitx5 logs the protocol at this verbosity.
grep -qF 'surrounding_text("你好", 6, 6)' "$out/fcitx5-pinyin.log" \
  && echo 'ok: the input method saw the surrounding text' \
  || echo "::warning::fcitx5's log shows no surrounding text 你好"

# --- 3. Hangul ---------------------------------------------------------------
start_fcitx hangul "$out/fcitx5-hangul.log"
log="$out/wl-ime-hangul.log"
run_app "$log" wayland-ime-hangul -- gksrmf -- -k space
stop "$FCITX"
echo "=== wl-ime-hangul.log ==="; grep -v ' key \|present ' "$log" || true
check 'textinput: text="" preedit=Some("한")' "$log"
check 'textinput: text="한" preedit=Some("글")' "$log"
check 'textinput: text="한글 " preedit=None' "$log"

if [ "$fail" != 0 ]; then
  for f in "$out"/fcitx5-*.log; do echo "=== $(basename "$f") ==="; tail -60 "$f"; done
fi
rm -rf "$ime_home"
exit "$fail"
