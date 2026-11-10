# Input methods, dead keys and compose

Chinese, Japanese and Korean text is typed through an input method (IME): the
user types a reading, the IME shows candidates, and the chosen text is
committed. Dead keys (`´` then `e` gives `é`) and compose sequences (Compose,
`o`, `c` gives `©`) work the same way on a smaller scale. Stipple handles all
of them through one model.

## The model

While the user composes, the IME sends **preedit** text: the composition so
far, with a cursor or a highlighted range inside it. The focused text field
shows it at the caret, underlined, but it isn't part of the field's text yet:
`EditBuffer::text()` doesn't include it and it doesn't enter the undo history.
When the user picks the final text, the IME **commits** it, which replaces the
preedit and is inserted like typing (one undo step with the typing around it).
An empty preedit ends the composition without committing anything.

At the platform layer these are `Event::Ime(Ime::Preedit { text, cursor })`,
`Event::Ime(Ime::Commit(text))` and `Event::Ime(Ime::DeleteSurrounding { .. })`
(some Wayland IMEs delete text around the caret). The App delivers them to the
focused element as `KeyInput::Preedit`, `KeyInput::Text` and
`KeyInput::DeleteSurrounding`, which `EditBuffer::apply` handles; a custom key
handler can handle them too.

After each frame the App tells the window about the focused text field with
`Window::set_ime(Some(&ImeState))`: the caret rectangle (the start of the
composition while composing), so the IME can put its candidate window next to
it, and the text around the caret. When focus moves to something that doesn't
take text, it calls `set_ime(None)`, which switches the IME off, and if focus
moves away mid-composition the old field drops its preedit.

`EditBuffer` exposes the same pieces for custom widgets: `set_preedit`,
`preedit`, `clear_preedit` and `delete_surrounding`, and `display()`, which
returns the text to draw with the preedit spliced in and the caret, selection
and preedit ranges within it (`display_to_text` maps a click back). Mark the
preedit range on the text element with `Element::preedit` and the focus overlay
underlines it.

## Per platform

| | Input methods | Dead keys and compose |
|---|---|---|
| Wayland | `zwp_text_input_v3`, when the compositor offers it | Stipple, from the Compose file |
| X11 | XIM (IBus, Fcitx, uim… started with XIM support) | Stipple, from the Compose file (or the input method, while one is running) |
| Windows | IMM32 | `TranslateMessage` (`WM_CHAR`) |
| macOS | `NSTextInputClient` | `NSTextInputClient` |
| Web | composition events on a hidden `<textarea>` | the browser (composition or `input` events) |
| iOS, Android | Not yet | Not yet |

**Wayland.** Stipple binds `zwp_text_input_manager_v3` and creates a text input
for the seat. While a text field has focus (and the compositor has sent
`enter`), it is enabled and kept told the surrounding text and the caret
rectangle (`set_surrounding_text`, `set_cursor_rectangle`, `commit`). The
input method's `preedit_string`, `commit_string` and `delete_surrounding_text`
take effect together on `done`. This needs a compositor that relays
`text-input-v3` to an input method (sway, KWin, GNOME, Hyprland) and an input
method that speaks `input-method-v2` or the compositor's own (fcitx5, IBus).

**X11.** Stipple speaks XIM, the X input method protocol, over its own X
connection (no libX11 or IBus library): `src/platform/backend/x11/xim.rs`.

- *Finding the server.* At startup it reads the root window's `XIM_SERVERS`
  property and takes the server named by `@im=` in `XMODIFIERS` (for example
  `XMODIFIERS=@im=ibus`), or the first one listed when `XMODIFIERS` names
  none. `@im=none` or `STIPPLE_XIM=0` switches XIM off. The server must be
  running when the app starts; one started later isn't picked up.
- *Connecting.* It makes a small input-only window, and the `_XIM_XCONNECT`
  handshake gives the server's. Messages then go as `_XIM_PROTOCOL` client
  messages, or, above 20 bytes, through a property announced by a client
  message (transport 0.0, as libX11 does); the server's large messages arrive
  the same way. `XIM_CONNECT`, `XIM_OPEN` and `XIM_ENCODING_NEGOTIATION`
  (UTF-8 offered first, COMPOUND_TEXT otherwise) open the input method.
- *Input contexts.* The first time a text field in a window takes focus,
  Stipple creates an on-the-spot context for the window
  (`XIMPreeditCallbacks | XIMStatusNothing`, with the window as client and
  focus window) and focuses it (`XIM_SET_IC_FOCUS`). `Window::set_ime` keeps
  the caret's position in `spotLocation` (`XIM_SET_IC_VALUES`), where the
  input method puts its candidate window; `set_ime(None)` unfocuses it, and
  resets it (`XIM_RESET_IC`) if a composition was in progress.
- *Keys.* While a text field has focus, key presses and releases go to the
  server (`XIM_FORWARD_EVENT`, synchronous or not as `XIM_SET_EVENT_MASK`
  says). It answers with `XIM_COMMIT` (text, or a keysym), the preedit
  callbacks (`XIM_PREEDIT_START` / `DRAW` / `CARET` / `DONE`, which become
  `Ime::Preedit`; the segment being converted, `XIMReverse`, is the
  highlighted range), or the key itself, forwarded back when the input
  method didn't use it, which then takes the normal path (shortcuts,
  arrows, Stipple's compose). `XIM_SYNC` and synchronous commits are
  answered with `XIM_SYNC_REPLY`. Without a focused text field, keys don't go
  to the server at all.
- *Text.* Servers that don't take UTF-8 (IBus) send COMPOUND_TEXT. Stipple
  decodes it itself (`x11/ctext.rs`): ASCII and Latin-1, UTF-8 segments
  (`ESC % G`), the ISO 8859 parts, JIS X 0201, and the 94×94 sets a UTF-8
  libX11 locale uses for CJK (JIS X 0208, KS C 5601, GB 2312), from tables
  in `x11/ctext.bin`.
- *Never blocking.* The client is a state machine driven by the event loop;
  it never waits for a reply. The handshake, creating a context and each
  synchronous key have a deadline (15 s, 15 s, 1.5 s; keys take the direct
  path while setting up) the loop wakes up for. If
  one passes, the server answers the setup with an error, or its window is
  destroyed (the server quit), XIM switches off for the rest of the run: keys
  it held are handled directly, any preedit is cleared, and typing goes back
  to the direct path with Stipple's dead keys and compose. Keys forwarded
  asynchronously (what IBus asks for) aren't timed, as the server answers
  keys it uses with nothing; if the server stops without quitting, they wait
  for it, as with libX11.

`STIPPLE_X11_DEBUG=1` logs the XIM messages and what it decided
(`xim: ready`, `xim: commit "…"`, `xim: off (…)`).

**Windows.** IMM32: `WM_IME_COMPOSITION` gives the composition string, with its
caret, and the result string. The IME's own composition window is hidden
because the field draws the preedit; the candidate list is placed under the
caret with `ImmSetCandidateWindow`. While no text field has focus the window
has no input context, so the IME stays off. TSF (the Text Services Framework)
would add reconversion and richer IME features and is not used yet.

**macOS.** The view is an `NSTextInputClient`. While a text field has focus,
key-downs go through `interpretKeyEvents:`, and the input context answers with
`setMarkedText:` (preedit), `insertText:` (commit) or `doCommandBySelector:`
(not text: the key takes the normal path). `firstRectForCharacterRange:` gives
the caret for the candidate window. This also gives the keyboard layout's dead
keys (Option+E, then E).

## Dead keys and compose on Linux

On X11 and Wayland, turning dead keys and compose sequences into text is the
client's job. Stipple reads the same Compose files as libX11, with its own
parser:

1. `$XCOMPOSEFILE`, if set;
2. otherwise `~/.XCompose`, then `$XDG_CONFIG_HOME/XCompose`
   (`~/.config/XCompose`);
3. otherwise the system file for the locale (`LC_ALL`, `LC_CTYPE` or `LANG`),
   found through `/usr/share/X11/locale/compose.dir` (and `locale.alias`), or
   under `$XLOCALEDIR`. The C locale, and locales without a file, use
   `en_US.UTF-8`'s, since Stipple's text is always Unicode.

User files can `include "%L"` (the locale's file), `%H` (home) and `%S` (the
system locale directory), and later lines override earlier ones. Lines with
modifier prefixes (`~Ctrl <a>`) are skipped.

While a sequence is pending, it shows as preedit: the spacing form of a dead
key (`´`, `¨`, `^`), `·` for the Compose key, then the keys typed so far.
Backspace takes back the last key; a key that continues no sequence cancels it
and is dropped, as libX11 and xkbcommon do. Shift and the other modifiers don't
interrupt a sequence; with Ctrl or the logo key held, a key is a shortcut and
doesn't start one.

Set `STIPPLE_COMPOSE_DEBUG=1` to print the table size and locale when it loads.

## Testing

Unit tests cover the Compose parser and state machine
(`src/platform/compose.rs`, including the system's real `en_US.UTF-8` file when
present), the preedit in `EditBuffer` (`src/widgets/preedit.rs`), the IME
state the App reports and the stale-preedit cleanup, the `text-input-v3` wire
requests and `done` handling, the UTF-16 conversions Windows and macOS
need, and on X11 the XIM messages against byte fixtures (the handshake, the
`XIM_CREATE_IC` attributes, forwarded keys, preedit draws and carets,
commits, sync replies, the client-message and property transport, timeouts
and errors), a scripted fake server driving the client through the transport
encoding, and the COMPOUND_TEXT decoder.

In CI, X11 types `dead_acute e` and `Multi_key o c` into `examples/textinput`
with `xdotool` and checks the text that arrives (the example prints it with
`STIPPLE_TEXTINPUT_ECHO=1`), screenshotting the underlined preedit mid-sequence;
Wayland does the same with `wtype` under sway. On macOS the self-check drives
`setMarkedText:` and `insertText:` through Objective-C dispatch.

X11's XIM client runs against a real input method in CI: IBus started with
`ibus-daemon --xim` under Xvfb, `XMODIFIERS=@im=ibus`. With IBus's keyboard
engine, `dead_acute e` must arrive as an `XIM_COMMIT` of `é` (and the pending
dead key as IBus's preedit); then, with the Japanese Mozc engine, `nihon`,
Space and Return must give the preedit `にほん`, the conversion `日本` and its
commit, screenshotted mid-way (this part is informational: it depends on
Mozc's defaults, so it warns instead of failing); finally IBus quits
mid-session and the app must notice and fall back to its own compose. The
same runs were checked by hand with IBus 1.5.33 and Mozc, including a stopped
server. Fcitx and uim haven't been tried.

Elsewhere real IMEs can't be driven headlessly, so on Wayland, Windows and
macOS the input-method paths are built and unit-tested but not exercised end
to end.

## Not yet

- On X11: picking up an XIM server started after the app, XIM status
  callbacks, string conversion (`XIM_STR_CONVERSION`), and timing out
  asynchronous keys a stopped server never answers.
- TSF on Windows; reconversion and `attributedSubstringForProposedRange:` on
  macOS (Stipple doesn't hand the IME the field's text there).
- Per-segment styling of the preedit (IMEs mark the segment being converted;
  Stipple shows the highlighted range as the selection and underlines all of
  it).
- Soft keyboards on iOS and Android.
