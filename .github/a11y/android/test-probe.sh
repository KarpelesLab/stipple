#!/usr/bin/env bash
# Drive androiddemo (already installed, as examples/androiddemo/test-on-device.sh
# leaves it) through an accessibility service, the way TalkBack does, from
# outside its process:
#
#   .github/a11y/android/test-probe.sh a11yprobe.apk [screenshot-dir]
#
# The probe (build-probe.sh) is installed and enabled with
# `settings put secure enabled_accessibility_services`; once connected it
# walks the app's window, moves accessibility focus through each node it
# would read, clicks "Tap me", sets the field's text and scrolls the list.
# This checks what it saw, and that the app got each action. The service is
# turned off again at the end.
set -euo pipefail

probe="${1:?usage: test-probe.sh a11yprobe.apk [screenshot-dir]}"
shots="${2:-visual_output}"
pkg=com.karpeleslab.stipple
service=org.stipple.a11yprobe/org.stipple.a11yprobe.Probe
mkdir -p "$shots"

plog() { adb logcat -d -s a11yprobe:I 2>/dev/null || true; }
applog() { adb logcat -d -s stipple:I 2>/dev/null || true; }
# Wait up to $3 seconds for a line of `$1` (plog or applog) matching $2.
wait_for() {
  local source="$1" pattern="$2" timeout="${3:-60}" i
  for ((i = 0; i < timeout; i++)); do
    if "$source" | grep -qE "$pattern"; then
      "$source" | grep -E "$pattern" | tail -1
      return 0
    fi
    sleep 1
  done
  echo "timed out waiting for: $pattern" >&2
  plog | tail -80 >&2
  return 1
}
fail() {
  echo "a11y probe: $*" >&2
  exit 1
}
disable() {
  adb shell settings put secure enabled_accessibility_services '""' >/dev/null 2>&1 || true
  adb shell settings put secure accessibility_enabled 0 >/dev/null 2>&1 || true
}
trap disable EXIT

adb install -r "$probe" || { adb uninstall org.stipple.a11yprobe && adb install "$probe"; }
adb logcat -c
adb shell am start -W -n "$pkg/android.app.NativeActivity" >/dev/null
sleep 3
adb shell settings put secure enabled_accessibility_services "$service"
adb shell settings put secure accessibility_enabled 1
wait_for plog "probe connected" 30
wait_for plog "probe done" 120
sleep 1
adb exec-out screencap -p > "$shots/android-a11y-probe.png" || true
plog > "$shots/a11yprobe.log"
applog | grep -v "androiddemo: tick" > "$shots/a11yprobe-app.log" || true
grep -E "probe (node|tree|focus|linear|click|set text|scroll|field|event)" "$shots/a11yprobe.log" \
  | sed 's/.*a11yprobe: //' | head -150

log="$shots/a11yprobe.log"
# The tree, read through the framework: the button and the field, with the
# widget classes TalkBack names them by, and their actions.
grep -qE 'probe node .*class=android.widget.Button text="Tap me" .*visible=true .*actions=\[[^]]*click' "$log" \
  || fail 'no visible "Tap me" Button node with a click action'
grep -qE 'probe node .*class=android.widget.EditText .*editable=true .*actions=\[[^]]*set_text' "$log" \
  || fail "no editable EditText node with a set-text action"
# Linear navigation: every stop took accessibility focus, and findFocus
# agreed, with an event for each.
nav="$(sed -n 's/.*probe linear navigation \([0-9]*\)\/\([0-9]*\).*/\1 \2/p' "$log" | tail -1)"
read -r moved stops <<< "$nav"
echo "a11y probe: accessibility focus moved to $moved of $stops stops"
[ -n "$stops" ] && [ "$stops" -ge 5 ] || fail "too few stops: $stops"
[ "$moved" = "$stops" ] || fail "focus reached only $moved of $stops stops"
grep -q "probe event TYPE_VIEW_ACCESSIBILITY_FOCUSED" "$log" \
  || fail "no TYPE_VIEW_ACCESSIBILITY_FOCUSED event"
# Activation: the click reached the app.
grep -q 'probe click "Tap me" -> true' "$log" || fail "ACTION_CLICK on \"Tap me\" failed"
wait_for applog "androiddemo: tapped [0-9]+" 10 > /dev/null || fail "the click didn't tap the button"
grep -q "probe event TYPE_VIEW_CLICKED" "$log" || fail "no TYPE_VIEW_CLICKED event"
# Text: set through the node, and read back from it.
grep -q "probe set text -> true" "$log" || fail "ACTION_SET_TEXT failed"
wait_for applog 'androiddemo: text "TalkBack"' 10 > /dev/null || fail "the field didn't take the text"
grep -q 'probe field now "TalkBack"' "$log" || fail "the field's node doesn't read the new text"
# Scrolling: androiddemo's list is a plain scrollable container, which
# offers ACTION_SCROLL_FORWARD as the sideways chip strip does; the probe
# picks the vertical one, and the list must scroll.
if grep -q 'probe scroll forward: no node offers it' "$log"; then
  echo "a11y probe: note: no node offers ACTION_SCROLL_FORWARD (a plain scroll container isn't marked scrollable)"
else
  grep -qE 'probe scroll forward .* -> true' "$log" || fail "ACTION_SCROLL_FORWARD failed"
  wait_for applog "androiddemo: scrolled to [1-9][0-9]*" 10 > /dev/null || fail "the list didn't scroll"
fi
# The slider's range (RangeInfo, ACTION_SET_PROGRESS, a step).
if grep -qE 'probe range class=android.widget.SeekBar .*range=0.0..1.0@0.5' "$log"; then
  echo "a11y probe: the SeekBar reads 0.5 of 0..1"
else
  fail "no SeekBar with a RangeInfo of 0..1 at 0.5"
fi
if grep -q 'probe set progress -> true' "$log" \
  && wait_for applog "androiddemo: volume 0.25" 10 > /dev/null; then
  echo "a11y probe: ACTION_SET_PROGRESS set the slider to 0.25"
else
  fail "ACTION_SET_PROGRESS didn't set the slider"
fi
if grep -q 'probe range step -> true' "$log" \
  && wait_for applog "androiddemo: volume 0.30" 10 > /dev/null; then
  echo "a11y probe: a scroll forward stepped the slider to 0.30"
else
  fail "a scroll forward didn't step the slider"
fi
grep -E "probe (range|set progress)" "$log" | sed 's/.*a11yprobe: //' || true
# A list's items and their place (CollectionInfo, CollectionItemInfo):
# androiddemo's list ends with three list items, "Note 1" to "Note 3".
# Noted, not yet required.
if grep -qE 'probe node .*class=android.widget.ListView .*collection=3x1' "$log" \
  && grep -qE 'probe node .*text="Note 2" .*item=1,0' "$log"; then
  echo "a11y probe: the list is a 3x1 collection, \"Note 2\" its item 1"
else
  echo "a11y probe: note: no list collection (3x1) with \"Note 2\" at item 1,0"
fi
# Text attribute runs: "Type your name:" is a Spanned with "Type" in en-GB
# (LocaleSpan), "your" bold (StyleSpan BOLD) and "name" a link (URLSpan,
# underlined), in UTF-16 ranges. Reported here, not failing the run yet
# (CI's "Android text spans" step checks the saved log).
spans="$(grep -E 'probe spans text="Type your name:"' "$log" | sed 's/.*a11yprobe: //' | tail -1 || true)"
if [ -n "$spans" ] \
  && grep -q 'locale(en-GB)@0\.\.4' <<< "$spans" \
  && grep -q 'style(1)@5\.\.9' <<< "$spans" \
  && grep -q 'url(https://stipple.rs)@10\.\.14' <<< "$spans" \
  && grep -q 'underline@10\.\.14' <<< "$spans"; then
  echo "a11y probe: text spans: $spans"
else
  echo "a11y probe: note: \"Type your name:\" doesn't read with its spans: ${spans:-no spans}"
fi
echo "a11y probe: all checks passed"
