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

## Elements that aren't text fields

A terminal or a custom editor can take input-method text too. Give it
`Element::ime_input(true)`, or `Element::ime_cursor_area(rect)` with its caret
relative to its top-left. While it has focus, the App calls `set_ime` with an
`ImeState` whose `cursor_area` is that rectangle in window coordinates (the
element's whole box without one) and with no surrounding text. The IME then
works as it does for a text field: the element's `on_key` handler gets
`KeyInput::Preedit` to draw at its caret and `KeyInput::Text` for commits, and
an empty preedit if focus leaves mid-composition. A viewport element with
`ime_input` gets the same while it holds input focus. Keys the IME doesn't use
still reach the element's `on_raw_key` handler. See [keyboard.md](keyboard.md).

## Per platform

| | Input methods | Dead keys and compose |
|---|---|---|
| Wayland | `zwp_text_input_v3`, when the compositor offers it | Stipple, from the Compose file |
| X11 | XIM (IBus, Fcitx, uim… started with XIM support) | Stipple, from the Compose file (or the input method, while one is running) |
| Windows | TSF (Text Services Framework), IMM32 as the fallback | `TranslateMessage` (`WM_CHAR`) |
| macOS | `NSTextInputClient` | `NSTextInputClient` |
| Web | composition events on a hidden `<textarea>` | the browser (composition or `input` events) |
| iOS | `UITextInput` on the view | the keyboard (on-screen ones compose; hardware dead keys not yet) |
| Android | an `InputConnection` from a host view (see [platforms/android.md](platforms/android.md#input-methods)) | the keyboard (on-screen ones compose; hardware dead keys not yet) |

**Wayland.** Stipple binds `zwp_text_input_manager_v3` and creates a text input
for the seat. While a text field has focus (and the compositor has sent
`enter`), it is enabled and kept told the surrounding text and the caret
rectangle (`set_surrounding_text`, `set_cursor_rectangle`, `commit`). The
input method's `preedit_string`, `commit_string` and `delete_surrounding_text`
take effect together on `done`. This needs a compositor that relays
`text-input-v3` to an input method (sway, KWin, GNOME, Hyprland) and an input
method that speaks `input-method-v2` or the compositor's own (fcitx5, IBus).
CI runs fcitx5 behind sway this way (see [Testing](#testing)).

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

**Windows.** Stipple is a Text Services Framework client, with IMM32 as the
fallback. Modern input methods are TSF text services (the Microsoft Japanese,
Chinese and Korean IMEs, handwriting, voice typing, the touch keyboard's
suggestions) and IMM32 reaches them only through a compatibility layer. All
of it is hand-written COM, like the UI Automation bridge
(`src/platform/backend/windows/tsf/`).

- *Activation.* The first time a window reports its IME state, the UI thread
  is made a single-threaded apartment, and a thread manager
  (`CLSID_TF_ThreadMgr`) is created and activated (`ITfThreadMgr::Activate`).
  If that fails, or `STIPPLE_TSF=0` is set, IMM32 is used as before.
- *Documents.* Each window gets a document manager with one context, over
  Stipple's text store. While a text field (or an `ime_input` element) has
  focus, `ITfThreadMgr::AssociateFocus` ties the window to it, so TSF puts
  its focus there whenever the window has the keyboard; otherwise the window
  is tied to an empty document manager, which leaves input methods nothing to
  compose into (they're off, as on the other platforms).
- *The text store* (`ITextStoreACP`) holds the field's text and selection
  from `Window::set_ime`, in UTF-16 offsets. `GetTextExt` answers with the
  caret rectangle (the composition's start while composing) in screen
  coordinates, which is where candidate windows go; `GetScreenExt` and
  `GetWnd` give the window. Embedded objects, attributes and
  `GetACPFromPoint` aren't offered.
- *Locks.* `RequestLock` grants a lock at once when none is held, whether
  asked synchronously or not. While one is held, a synchronous request is
  refused (`TS_E_SYNCHRONOUS`) and an asynchronous one is queued
  (`TS_S_ASYNC`) and granted as soon as the holder releases it, still inside
  the outer call. Nothing waits, so the message loop never blocks. Changes the
  App makes are told to TSF (`OnTextChange`, `OnSelectionChange`,
  `OnLayoutChange`) only while no lock is held; a report that comes during a
  lock waits for its release.
- *Compositions.* The context owner's composition sink
  (`ITfContextOwnerCompositionSink`) hears a composition start, change range
  and end; the text the input method writes goes through `SetText` and
  `InsertTextAtSelection`. After each lock Stipple compares the document with
  what the field holds: the composition becomes `Ime::Preedit`, and text
  changed outside it (a commit, the end of a composition, a voice-typing
  insertion, a correction of the word before the caret, reconversion taking a
  word back into a composition) becomes `Ime::DeleteSurrounding` and
  `Ime::Commit` around the caret. The App's next report is checked against the
  text it should then have: a report from before it handled the events is
  ignored, and anything else is a change the App made itself, which replaces
  the document (ending any composition with
  `ITfContextOwnerCompositionServices::TerminateComposition`, uncommitted).
- *Display attributes.* An `ITfTextEditSink` on the context reads, at the end
  of each edit session, the composition's `GUID_PROP_ATTRIBUTE` runs and
  resolves them through the category and display attribute managers. The
  clause being converted (`TF_ATTR_TARGET_CONVERTED` /
  `TF_ATTR_TARGET_NOTCONVERTED`) is the preedit's highlighted range;
  without one, the input method's selection inside the composition is, or a
  caret at its end. The field draws its own underline, so the attributes'
  colours and line styles aren't used.
- *Keys* reach text services through the hooks TSF installs on activation;
  the message loop is unchanged, and keys no input method takes arrive as
  `WM_KEYDOWN` / `WM_CHAR` as before.

If anything in the TSF path fails in a way that could lose typing (a
text store call panicking, `AssociateFocus` failing), TSF is switched off for
the rest of the run: every window is untied from its document, the thread
manager is deactivated, and the IMM32 path takes over from the next report.

`STIPPLE_DEBUG_TSF=1` logs activation, focus, lock requests, the store's
writes, compositions and the events sent (`stipple-tsf: …`).

The IMM32 fallback: `WM_IME_COMPOSITION` gives the composition string, with
its caret, and the result string. The IME's own composition window is hidden
because the field draws the preedit; the candidate list is placed under the
caret with `ImmSetCandidateWindow`. While no text field has focus the window
has no input context, so the IME stays off.

**macOS.** The view is an `NSTextInputClient`. While a text field has focus,
key-downs go through `interpretKeyEvents:`, and the input context answers with
`setMarkedText:` (preedit), `insertText:` (commit) or `doCommandBySelector:`
(not text: the key takes the normal path). `firstRectForCharacterRange:` gives
the caret for the candidate window. This also gives the keyboard layout's dead
keys (Option+E, then E).

**iOS.** The view is a `UITextInput`: while a text field has focus it is first
responder and the keyboard's document. The keyboard reads the field's text
around the caret and the selection (`textInRange:`, `selectedTextRange`, from
`Window::set_ime`, in UTF-16 offsets), composes with
`setMarkedText:selectedRange:` (the preedit, at the caret) and commits with
`insertText:` or `unmarkText`. Autocorrect, QuickType and dictation replace
the word before the caret with `replaceRange:withText:`, which Stipple turns
into `Ime::DeleteSurrounding` and a commit; a range that doesn't touch the
caret is ignored. Moving the caret from the keyboard (dragging on the space
bar) sends arrow keys. `firstRectForRange:` and `caretRectForPosition:` answer
with the caret rectangle, in the view's coordinates. The keyboard's own edits
show in its document at once; other changes to the field (a click moving the
caret, the app replacing the text) are announced to it through its
`inputDelegate` (`textDidChange:`, `selectionDidChange:`), but not while it
composes. Positions and ranges are subclasses of `UITextPosition` and
`UITextRange` built at run time, each offset an instance variable.

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

Unit tests cover the IME state an element with `ime_input` reports (its
caret rectangle, and the IME switching off when it loses focus), the Compose
parser and state machine
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
`setMarkedText:` and `insertText:` through Objective-C dispatch. On the iOS
simulator `touchdemo`'s self-test does the same with the view's `UITextInput`
methods: `setMarkedText:selectedRange:` must reach the field as a preedit, the
document read back through `textInRange:` / `selectedTextRange` /
`markedTextRange` must match, `firstRectForRange:` must lie in the field, and
a committing `insertText:` and a `replaceRange:withText:` must edit the
field's text. The document model is unit-tested on every host
(`src/platform/backend/ios/doc.rs`).

X11's XIM client runs against a real input method in CI: IBus started with
`ibus-daemon --xim` under Xvfb, `XMODIFIERS=@im=ibus`. With IBus's keyboard
engine, `dead_acute e` must arrive as an `XIM_COMMIT` of `é` (and the pending
dead key as IBus's preedit); then, with the Japanese Mozc engine, `nihon`,
Space and Return must give the preedit `にほん`, the conversion `日本` and its
commit, screenshotted mid-way (required: it ran as an informational check
first and passed on every run, over forty); finally IBus quits
mid-session and the app must notice and fall back to its own compose. The
same runs were checked by hand with IBus 1.5.33 and Mozc, including a stopped
server. Fcitx and uim haven't been tried.

On Windows, CI runs `examples/textinput` with `STIPPLE_TSF_SELFTEST=1`
(`.github/scripts/tsf-windows.sh`): TSF must activate, the window must be
tied to its document when Tab focuses the field, and the app then plays an
input method through the real TSF manager: edit sessions
(`ITfContext::RequestEditSession`) start a composition at the selection
(`ITfContextOwnerCompositionServices::StartComposition`), write `にほん` and
then `日本` into it (`ITfRange::SetText`) and end it. TSF locks Stipple's store, writes
through `ITextStoreACP` and calls the composition sink, and the field must
show each preedit and then the commit (screenshotted mid-way). Starting the
composition is tried through `ITfContextOwnerCompositionServices` and
`ITfContextComposition`, over the selection and over the insertion point, with
and without a composition sink, each attempt's `HRESULT` logged. On the
runner `ITfContextComposition` refused the app (`E_FAIL`) and the owner's
`ITfContextOwnerCompositionServices` worked; CI requires that path, the
composition lasting through all three steps, and the commit. (If every way
were refused the self-test would insert `日本` with `ITfInsertAtSelection`,
and CI fails.) With `STIPPLE_TSF=0` TSF must stay off. No real IME is
installed on the runner for this job, so what an input method does on its own (keys,
candidates, display attributes, reconversion) isn't exercised here (the
non-blocking attempt below tries the real one); the IMM32 fallback is build-checked only.

Wayland's `text-input-v3` runs against a real input method in CI:
fcitx5 under headless sway (`.github/scripts/wayland-ime.sh`). fcitx5 binds
sway's `zwp_input_method_v2` and a virtual keyboard; sway relays between it
and the app's text input, and `wtype`'s keys reach fcitx5 through its
keyboard grab while the field is enabled. With an isolated profile per
engine:

- *Mozc*: `nihon` must arrive as the preedit `にほｎ` and `にほん`, Space must
  turn it into `日本` (still a preedit), Return must commit it; then `ka` and
  Escape must cancel the preedit `か` without committing anything.
- *Pinyin* (fcitx5-chinese-addons): `nihao` shows the syllables `ni hao` as
  the preedit, and Space must commit `你好`. fcitx5's protocol log must show
  the field's text and caret reaching it (`surrounding_text("你好", 6, 6)`;
  a warning if not).
- *Hangul* (fcitx5-hangul, dubeolsik): `gksrmf` must compose `ㅎ`, `하`, `한`,
  commit `한` when the next syllable starts, compose `글`, and Space must
  commit it.

The same script passed locally against Ubuntu 26.04's packages (fcitx5
5.1.19, sway 1.11) in a rootless container. Mozc refuses to run as root, so
it must run as a normal user, as on the runner.

On Windows a separate, required job (`ime-windows`,
`.github/scripts/ime-windows.ps1`) tries the real Microsoft Japanese IME:
it installs the ja-JP typing feature if the IME's TSF profile isn't
registered (`Add-WindowsCapability`, then `Install-Language`, starting
Windows Update if the image stopped it), adds the IME to the user's
languages, switches the app's thread to Japanese
(`WM_INPUTLANGCHANGEREQUEST`), turns the IME on (`VK_IME_ON`, or `VK_KANJI`)
until `a` becomes a preedit that Escape cancels, and types `n i h o n`,
Space, Return with `SendInput`; the field must show the hiragana preedit,
`日本` and the commit. It has passed on the runner, but only about one run
in six until the script stopped tapping Alt to get past the foreground
lock: whenever that lone Alt release reached the app's window,
`DefWindowProc` put the window in system-menu mode (`SC_KEYMENU`, as any
Win32 window does) and the menu loop ate the keys after it — the Tab that
focuses the field, or `VK_IME_ON` and the probe's `a` (no run ever saw
that `a`). It now brings the window forward with `SetForegroundWindow`,
then `AttachThreadInput`, then Alt held with a masking key (VK 0xE8)
before its release, then a click on the title bar, checks
`GetGUIThreadInfo` for menu mode before each burst of keys, waits for the
app's own trace (`stipple-tsf: focus`) to confirm the field took focus
before switching the language, and repeats Escape when the IME's
prediction list takes the first one (a probe composition left behind is
how `兄本` was committed instead of `日本`). The commit after Return also
reached the app only with the next key: TSF delivers it through sent
messages that run inside `PeekMessageW`, and the event loop delivered its
queue only after a posted message (fixed in the loop). A failing run ends
with `FAILED at <stage>` and the window's input state. It has been
required since it passed three runs in a row after those fixes. On macOS a non-blocking step of the
`interaction-macos` job (`.github/scripts/ime-macos.sh`) compiles a small
Swift helper that enables and selects the built-in Japanese input source
(Kotoeri, romaji typing, with live conversion off) and posts the same keys
as CGEvents. It hasn't been seen passing yet: whether a CLI process may
post keys (Accessibility) and switch the input source there is what its
runs will show; it logs the stage it stopped at. Until the macOS one
passes, macOS input methods are also checked as protocol wiring (above) and
by hand (below), and iOS by its protocol self-test.

## Manual checklist

Where CI can't run a real input method, check these by hand before a
release, on each platform (X11 with IBus or Fcitx through XIM; Wayland with
fcitx5 or IBus under a compositor with `text-input-v3`; Windows with the
Microsoft IMEs; macOS with the built-in input sources; the web in Chrome,
Firefox and Safari; iOS and Android with their system keyboards). Use
`examples/textinput` (`STIPPLE_TEXTINPUT_ECHO=1` prints the field and
preedit) and `examples/textarea` for multi-line text.

| Check | What to do | Expected |
|---|---|---|
| Japanese | Turn on a Japanese IME (Mozc, Microsoft IME, Kotoeri), type `nihongo`, Space, Return | `にほんご` underlined while typing, the conversion highlighted after Space, `日本語` in the field after Return, one undo step |
| Chinese pinyin | Turn on pinyin (fcitx5 pinyin, Microsoft Pinyin, macOS Pinyin – Simplified), type `nihao`, pick a candidate with Space or a digit | the syllables as preedit, `你好` committed |
| Korean | Turn on a Hangul IME (dubeolsik), type `gksrmf`, then Space | syllables compose in place (`ㅎ` `하` `한`), `한` commits when `글` starts, `한글 ` in the field |
| Candidate window | During a composition, move the window, scroll the field so the caret moves, try a field near the bottom and right edges of the screen | the candidate list opens next to the caret (the composition's start), follows it, and stays on screen |
| Cancel | Compose, then press Escape; compose, then click elsewhere in the field; compose, then Tab to another field | Escape drops the preedit and commits nothing; clicking or moving focus ends the composition (committed or dropped, as the IME does) and never leaves a stale preedit |
| Reconvert | Select committed text (or put the caret in it) and press the IME's reconvert key (Windows: the 変換 key or Win+/; macOS: Control+Shift+R; Mozc: 変換) | the IME takes the word back as a composition with candidates, and the choice replaces it; where it isn't supported (macOS; XIM's string conversion) nothing is lost or duplicated |
| Mixed input | Type Latin, compose, type Latin again; use Backspace mid-composition | text stays in order; Backspace edits the preedit, not the field, until it is empty |
| Password fields | Focus a password field with an IME on | the IME is off (direct input); on macOS secure input is on |

Record the platform, input method and version with any failure. CI covers:
Japanese (Mozc) on X11 and Wayland, cancel on Wayland, pinyin and Korean on
Wayland; Japanese on Windows (required); the macOS row runs in CI as a
non-blocking attempt.

## Not yet

- On X11: picking up an XIM server started after the app, XIM status
  callbacks, string conversion (`XIM_STR_CONVERSION`), and timing out
  asynchronous keys a stopped server never answers.
- Reconversion and `attributedSubstringForProposedRange:` on macOS
  (Stipple doesn't hand the IME the field's text there).
- A real input method required in CI on macOS (attempted, see
  [Testing](#testing)), and on iOS, Android and the web.
- Per-segment styling of the preedit (IMEs mark the segment being converted;
  Stipple shows the highlighted range as the selection and underlines all of
  it).
- On Android: `getExtractedText` and cursor-anchor info, selection moves from
  a selection or mid-composition, hardware dead keys; and a real composing
  keyboard driven by a person (CI runs a self-test through the connection).
- On iOS: selection rectangles, hit-testing a point to a position, selections
  made from the keyboard, and dead keys on hardware keyboards.
