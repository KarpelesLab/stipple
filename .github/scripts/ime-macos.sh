#!/usr/bin/env bash
# Type Japanese into examples/textinput through macOS's own Japanese input
# method (Kotoeri, romaji typing), via NSTextInputClient.
#
#   bash .github/scripts/ime-macos.sh <textinput binary>
#
# A small Swift helper (ime-macos.swift) enables and selects the input
# source (TISEnableInputSource / TISSelectInputSource) and posts the keys as
# CGEvents: Tab focuses the field, then n i h o n must arrive as a preedit
# (setMarkedText:), Space converts it and Return commits it (insertText:),
# leaving 日本 in the field. Live conversion is switched off first, so the
# preedit stays in hiragana until Space. Each stage logs what it found;
# exit code 0 only when the commit arrived. OUT_DIR as elsewhere.
set -uo pipefail

app="$1"
out=${OUT_DIR:-visual_output}
mkdir -p "$out" target
helper=target/ime-macos

if ! swiftc -O -o "$helper" .github/scripts/ime-macos.swift; then
  echo "ime-macos: FAILED: the helper doesn't compile"
  exit 1
fi
sw_vers
"$helper" trusted
"$helper" current
echo "=== Japanese input sources ==="
"$helper" list | grep -i 'kotoeri\|japanese' || echo "ime-macos: none listed"

# Hiragana while composing; Space converts.
defaults write com.apple.inputmethod.Kotoeri JIMPrefLiveConversionKey -bool false
killall -q JapaneseIM JapaneseIM-RomajiTyping 2>/dev/null || true

select_japanese() {
  for id in com.apple.inputmethod.Kotoeri.RomajiTyping.Japanese com.apple.inputmethod.Kotoeri.Japanese; do
    if "$helper" select "${id%.Japanese}" "$id"; then
      return 0
    fi
  done
  "$helper" current
  return 1
}

if ! select_japanese; then
  echo "ime-macos: FAILED: couldn't select the Japanese input source"
  exit 1
fi

log="$out/ime-macos.log"
STIPPLE_TEXTINPUT_ECHO=1 "$app" > "$log" 2>&1 &
pid=$!
trap 'kill $pid 2>/dev/null || true; "$helper" select com.apple.keylayout.ABC > /dev/null 2>&1 || true' EXIT
sleep 4
# Tab in direct mode would be typed by the input method too; it passes
# Tab through. Then select again with the app in front, and the Kana key
# for hiragana in case the source opened in another mode.
"$helper" keys tab +800
select_japanese > /dev/null || true
"$helper" current
"$helper" keys kana +500 n i h o n +1000
screencapture -x "$out/mac-ime-preedit.png" || true
"$helper" keys space +1000
screencapture -x "$out/mac-ime-converted.png" || true
"$helper" keys return +1000
kill "$pid" 2>/dev/null || true
wait "$pid" 2>/dev/null || true

echo "=== ime-macos.log ==="
cat "$log"
fail=0
check() { # pattern [warn]
  if grep -qF -- "$1" "$log"; then
    echo "ok: $1"
  elif [ "${2:-}" = warn ]; then
    echo "::warning::missing from ime-macos.log: $1"
  else
    echo "::error::missing from ime-macos.log: $1"
    fail=1
  fi
}
check 'textinput: text="" preedit=Some('
check 'textinput: text="" preedit=Some("にほ' warn
check 'textinput: text="日本" preedit=None'
if [ "$fail" != 0 ]; then
  if ! grep -q 'preedit=Some(' "$log"; then
    if grep -q 'textinput: text="[a-z]' "$log"; then
      echo "ime-macos: the keys arrived as plain letters: the input method wasn't active"
    else
      echo "ime-macos: no keys reached the field (posting events needs the Accessibility permission)"
    fi
  fi
  echo "ime-macos: FAILED"
fi
exit "$fail"
