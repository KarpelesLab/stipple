#!/usr/bin/env bash
# Drive a running Stipple window (touchdemo) with VoiceOver, through
# VoiceOver's own AppleScript interface, and check what it says.
#
#   bash .github/scripts/voiceover-macos.sh <touchdemo binary>
#
# 1. Lets AppleScript control VoiceOver, as the "Allow VoiceOver to be
#    controlled with AppleScript" checkbox does: SCREnableAppleScript in
#    VoiceOver's defaults (both where older and newer macOS keep them) and
#    /private/var/db/Accessibility/.VoiceOverAppleScriptEnabled (writable
#    as root because SIP is off on GitHub's macOS runners), and grants the
#    processes that run osascript here Apple Events to VoiceOver in TCC (the
#    same rows as clicking "OK" on the consent prompt would add). The
#    welcome dialog is turned off.
# 2. Starts touchdemo, brings it to the front, and starts VoiceOver.
# 3. Moves the VoiceOver cursor through the window (move right / move into
#    item / move out of item, depth first) and logs every phrase VoiceOver
#    speaks ("content of last phrase"), until it reaches "Row 0".
# 4. Presses it (perform action, VoiceOver's VO-Space, which reaches the
#    element's accessibilityPerformPress), and checks the app counted the
#    tap.
# 5. The keyboard alone (keys typed through System Events): Tab twice
#    focuses the field, then "Row 0" (the app posts
#    AXFocusedUIElementChanged, which VoiceOver follows: what it says then
#    is reported, not required), and Space and Return press the button (the
#    app logs each tap).
#
# Exit status: 0 when VoiceOver spoke "Row 0" as a button and the press
# reached the app; 1 when a check failed; 3 when AppleScript couldn't
# control VoiceOver at all (the runner refused). Every stage logs what it
# found. OUT_DIR as elsewhere.
set -uo pipefail

app="$1"
out=${OUT_DIR:-visual_output}
mkdir -p "$out"
applog="$out/voiceover-touchdemo.log"
phrases="$out/voiceover-phrases.log"
: > "$phrases"
failures=()
check() { # check <ok: 0/1> <what>
  if [ "$1" = 0 ]; then echo "ok: $2"; else echo "FAILED: $2"; failures+=("$2"); fi
}
# osascript with a deadline (macOS has no `timeout`; perl's alarm stands in).
osa() { perl -e 'alarm 20; exec @ARGV' osascript "$@"; }
vo() { osa -e "tell application \"VoiceOver\" to $1" 2>&1; }

echo "=== environment ==="
sw_vers
csrutil status 2>&1 || true
id
echo "parent processes of this shell:"
pid=$$
for _ in 1 2 3 4 5 6 7 8; do
  line=$(ps -o pid=,ppid=,comm= -p "$pid" 2>/dev/null) || break
  echo "  $line"
  pid=$(echo "$line" | awk '{print $2}')
  [ "$pid" -le 1 ] 2>/dev/null && break
done

echo "=== letting AppleScript control VoiceOver ==="
defaults write com.apple.VoiceOver4/default SCREnableAppleScript -bool true
group="$HOME/Library/Group Containers/group.com.apple.VoiceOver/Library/Preferences"
mkdir -p "$group" 2>/dev/null || true
defaults write "$group/com.apple.VoiceOver4/default" SCREnableAppleScript -bool true 2>&1 \
  || echo "note: couldn't write VoiceOver's group container defaults"
# No welcome dialog ("Use VoiceOver?" / the quick start).
defaults write com.apple.VoiceOverTraining doNotShowSplashScreen -bool true
defaults write com.apple.VoiceOver4/default SCRCategories_SCRCategorySystemWide_SCRSpeechComponentSettings_SCRDisableSpeech -bool false 2>/dev/null || true
sudo mkdir -p /private/var/db/Accessibility
if printf a | sudo tee /private/var/db/Accessibility/.VoiceOverAppleScriptEnabled > /dev/null; then
  echo "wrote /private/var/db/Accessibility/.VoiceOverAppleScriptEnabled"
else
  echo "note: couldn't write /private/var/db/Accessibility/.VoiceOverAppleScriptEnabled"
fi
defaults read com.apple.VoiceOver4/default SCREnableAppleScript 2>&1 || true

# Apple Events to VoiceOver (and System Events, to bring the app forward)
# for whichever of these TCC holds responsible for osascript here: the
# runner's agent, the shells, osascript itself, and every client the
# databases already grant Accessibility or Apple Events to.
sysdb="/Library/Application Support/com.apple.TCC/TCC.db"
userdb="$HOME/Library/Application Support/com.apple.TCC/TCC.db"
echo "TCC access table: $(sudo sqlite3 "$sysdb" "PRAGMA table_info(access);" 2>&1 | cut -d'|' -f2 | tr '\n' ' ')"
echo "TCC clients granted Accessibility / Apple Events / PostEvent:"
for db in "$sysdb" "$userdb"; do
  sudo sqlite3 "$db" "SELECT service, client, client_type, auth_value, indirect_object_identifier FROM access WHERE service IN ('kTCCServiceAccessibility','kTCCServiceAppleEvents','kTCCServicePostEvent');" 2>&1 \
    | sed "s|^|  $(basename "$(dirname "$(dirname "$db")")"): |"
done
clients=$( { for db in "$sysdb" "$userdb"; do
    sudo sqlite3 "$db" "SELECT client || '|' || client_type FROM access WHERE service IN ('kTCCServiceAccessibility','kTCCServiceAppleEvents');" 2>/dev/null
  done
  for c in /bin/bash /bin/sh /bin/zsh /usr/bin/osascript /usr/bin/perl /usr/libexec/sshd-keygen-wrapper \
    /opt/hca/hosted-compute-agent /usr/local/opt/runner/provisioner/provisioner; do
    echo "$c|1"
  done
  echo "com.apple.Terminal|0"
} | sort -u)
grant() { # grant <db> <client> <client_type> <bundle id>
  sudo sqlite3 "$1" "INSERT OR REPLACE INTO access (service, client, client_type, auth_value, auth_reason, auth_version, indirect_object_identifier_type, indirect_object_identifier, flags) VALUES ('kTCCServiceAppleEvents', '$2', $3, 2, 3, 1, 0, '$4', 0);" 2>&1
}
while IFS= read -r entry; do
  [ -n "$entry" ] || continue
  client=${entry%|*}; ctype=${entry##*|}
  for db in "$sysdb" "$userdb"; do
    for target in com.apple.VoiceOver com.apple.systemevents; do
      err=$(grant "$db" "$client" "$ctype" "$target") || echo "note: TCC $db: $client -> $target: $err"
    done
  done
  echo "granted Apple Events to VoiceOver and System Events for: $client ($ctype)"
done <<< "$clients"
# tccd caches decisions; it's started again on demand.
sudo killall tccd 2>/dev/null || true
killall tccd 2>/dev/null || true
sleep 2

echo "=== touchdemo ==="
"$app" > "$applog" 2>&1 &
app_pid=$!
sleep 5
front() {
  osa -e "tell application \"System Events\" to set frontmost of (first process whose unix id is $app_pid) to true" 2>&1 \
    || echo "note: System Events couldn't bring touchdemo forward"
}
front
osa -e 'tell application "System Events" to get name of first process whose frontmost is true' 2>&1 \
  | sed 's/^/frontmost: /'

echo "=== VoiceOver ==="
stop_vo() {
  vo "quit" > /dev/null 2>&1 || true
  sleep 2
  if pgrep -x VoiceOver > /dev/null; then
    killall VoiceOver 2>/dev/null || true
  fi
}
finish() {
  stop_vo
  kill "$app_pid" 2>/dev/null || true
  echo "=== what VoiceOver said ==="
  cat "$phrases"
  echo "=== touchdemo log (tail) ==="
  tail -n 20 "$applog" || true
}
trap finish EXIT
/System/Library/CoreServices/VoiceOver.app/Contents/MacOS/VoiceOverStarter 2>&1 &
ok=1
for i in $(seq 1 30); do
  sleep 1
  if pgrep -x VoiceOver > /dev/null; then
    r=$(vo "return content of last phrase")
    st=$?
    if [ "$st" = 0 ]; then ok=0; echo "VoiceOver answers AppleScript after ${i}s: \"$r\""; break; fi
    [ $((i % 5)) = 0 ] && echo "VoiceOver running; AppleScript says: $r"
  elif [ $((i % 5)) = 0 ]; then
    echo "VoiceOver isn't running yet (${i}s)"
  fi
done
if [ "$ok" != 0 ]; then
  echo "VoiceOver processes: $(pgrep -lx VoiceOver || echo none)"
  echo "last AppleScript answer: $r"
  echo "::warning::VoiceOver couldn't be controlled through AppleScript on this runner"
  exit 3
fi

# What VoiceOver says, logged; the phrase is the reply to the command.
say_now() { # say_now <label>
  sleep 1.2
  local p
  p=$(vo "return content of last phrase")
  echo "$1: $p" | tee -a "$phrases" >&2
  printf '%s' "$p"
}
front
sleep 2
say_now "front" > /dev/null
under=$(vo "return text under vo cursor" 2>&1) || under=$(vo "tell vo cursor to return text under cursor" 2>&1)
echo "text under the VoiceOver cursor: $under" | tee -a "$phrases"

# A VoiceOver cursor command through AppleScript, else its keys (VO is
# Control+Option) through System Events.
vo_move() { # vo_move right|into|out
  local cmd key mods r
  case "$1" in
    right) cmd="move right"; key=124; mods="{control down, option down}" ;;
    into) cmd="move into item"; key=125; mods="{control down, option down, shift down}" ;;
    out) cmd="move out of item"; key=126; mods="{control down, option down, shift down}" ;;
    press) cmd="perform action"; key=49; mods="{control down, option down}" ;;
  esac
  r=$(vo "tell vo cursor to $cmd")
  if [ $? != 0 ] || printf '%s' "$r" | grep -qi 'error'; then
    echo "  ($cmd through AppleScript: $r; with keys instead)" >&2
    osa -e "tell application \"System Events\" to key code $key using $mods" >&2
  fi
}

# Depth first through the window: into groups, right along siblings, out
# when a level ends (the phrase stops changing).
found=1
saw_count=1
last=""
for step in $(seq 1 80); do
  p=$(say_now "step $step")
  case "$p" in
    *"Row 0"*) found=0; break ;;
  esac
  case "$p" in
    *"Rows tapped"*) saw_count=0 ;;
  esac
  if [ "$p" = "$last" ]; then
    vo_move out
    sleep 0.5
    vo_move right
  elif printf '%s' "$p" | grep -qi 'group\|window\|scroll area\|content'; then
    vo_move into
    q=$(say_now "  into")
    if [ "$q" = "$p" ]; then vo_move right; fi
  else
    vo_move right
  fi
  last="$p"
done
check $found "VoiceOver's cursor reached \"Row 0\""
if [ "$found" = 0 ]; then
  echo "$p" | grep -qi 'button'
  check $? "VoiceOver said \"Row 0\" is a button (\"$p\")"
  [ "$saw_count" = 0 ] && echo "ok: VoiceOver read \"Rows tapped\" on the way" \
    || echo "note: VoiceOver didn't read \"Rows tapped\" on the way"
  before=$(grep -c '^touchdemo: row 0 tapped' "$applog")
  vo_move press
  sleep 2
  say_now "after the press" > /dev/null
  after=$(grep -c '^touchdemo: row 0 tapped' "$applog")
  [ "$after" -gt "$before" ]
  check $? "VO-Space (perform action) on \"Row 0\" pressed it: the app logged the tap"
fi

# The keyboard alone. Nothing in the app has focus now (the press above was
# a click, which doesn't focus a button), so Tab goes to the field, then to
# "Row 0".
front
sleep 1
key() { # key <key code> <what>
  osa -e "tell application \"System Events\" to key code $1" 2>&1 \
    || echo "note: System Events couldn't type $2"
}
key 48 Tab
sleep 1
key 48 Tab
p=$(say_now "after Tab, Tab")
if printf '%s' "$p" | grep -q "Row 0"; then
  echo "ok: VoiceOver followed the focus to \"Row 0\" (\"$p\")"
else
  echo "note (not required): VoiceOver didn't say \"Row 0\" as Tab focused it (\"$p\")"
fi
for k in "49 Space" "36 Return"; do
  before=$(grep -c '^touchdemo: row 0 tapped' "$applog")
  key ${k% *} "${k#* }"
  sleep 1.5
  after=$(grep -c '^touchdemo: row 0 tapped' "$applog")
  [ "$after" -gt "$before" ]
  check $? "${k#* } on the focused \"Row 0\" pressed it: the app logged the tap"
done

if [ ${#failures[@]} -gt 0 ]; then
  printf 'VoiceOver checks failed: %s\n' "${failures[*]}"
  exit 1
fi
echo "VoiceOver checks passed"
