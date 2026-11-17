# Drag and drop

An element can be a drop target for files and text dragged from other
applications (Finder, Explorer, a file manager, a browser), and for items
dragged inside the app. Files and text dragged from an element can leave the
window and drop in other applications. While a drag hovers a target, the view can read what
the drag is doing to it and animate. The `dropdemo` example shows all of it:

```sh
cargo run --example dropdemo
```

## Drop targets

`Element::on_drop` makes an element take drops. The handler runs against the
state with the `DropData` and the drop position in logical pixels:

```rust
label(cx, "Drop files here").on_drop(cx, |s, data, at| {
    match data {
        DropData::Files(paths) => s.files.extend(paths.iter().cloned()),
        DropData::Text(text) => s.notes.push(text.clone()),
        DropData::Internal(id) => s.move_item(id, at),
    }
})
```

`on_drop` takes every kind of data. `on_drop_of(cx, DragOffer::FILES, ...)`
takes only the kinds you name; a drag carrying anything else is refused (the
cursor shows "no", and a drop does nothing). `DragOffer` has `FILES`, `TEXT`,
`INTERNAL` and `ANY`, or build one from its three flags.

Drops are hit-tested like taps: the top-most drop target under the pointer is
the one the drag is over. A target the app doesn't style itself gets a
highlight (the theme's hover overlay and a focus-colored border) while an
acceptable drag is over it. It fades in when the drag enters and out when it
leaves or drops, with the target's drag-over `level` (below), and switches
at once under reduced motion.

## Animating a drop zone

The simplest way to style a target for the drag is per-state overrides with a
transition (see [animation.md](animation.md)):

```rust
column(vec![label(cx, "Drop files")])
    .padding(Insets::uniform(16.0))
    .fill(theme.palette.surface)
    .border(theme.palette.border, 1.0)
    // Grows and glows while a drag it takes is over it, tints red for one it
    // doesn't, and springs back when the drag leaves or drops.
    .drag_over(|e| e.padding(Insets::uniform(24.0))
        .fill(theme.palette.surface.mix(theme.palette.primary, 0.25))
        .border(theme.palette.primary, 4.0))
    .drag_refused(|e| e.fill(theme.palette.surface.mix(theme.palette.danger, 0.25))
        .border(theme.palette.danger, 4.0))
    .transition(Transition::spring(SpringParams::new(180.0, 0.45)))
    .on_drop_of(cx, DragOffer::FILES, |s, data, _| s.add(data))
```

`.drag_over` holds while a drag the target accepts is over it (or over a drop
target inside it), `.drag_refused` while one it refuses is. A target with
either draws its own look, so the app doesn't add its highlight, and the app
rebuilds only when a drag enters, leaves or changes verdict. `.dragged(f)`
styles a `drag_source` while it is being dragged.

To build the target from the drag instead (different words, the pointer
position), register it first, read its `DropState`, build the element from it,
and stamp the id on it:

```rust
let zone = cx.register_drop(DragOffer::FILES, |s, data, _| s.add(data));
let drag = cx.drop_state(zone);
let glow = if drag.accepted { theme.palette.primary } else { theme.palette.danger };
column(vec![label(cx, if drag.over { "Release to drop" } else { "Drop files" })])
    .padding(Insets::uniform(16.0 + 8.0 * drag.level))   // grows
    .fill(theme.palette.surface.mix(glow, 0.25 * drag.level)) // glows, or tints red
    .border(theme.palette.border.mix(glow, drag.level), 1.0 + 3.0 * drag.level)
    .drop_target(zone)
```

`DropState` has:

- `over`: a drag is over the target now.
- `accepted`: the target takes what the drag carries. It keeps its value while
  the level eases out, so a fading tint keeps its color.
- `position`: the pointer within the target (from its top-left corner) while
  the drag is over it.
- `level`: eases from 0 to 1 over `DROP_FADE` (180 ms) when a drag enters, and
  back to 0 when it leaves or drops. Drive a grow, a glow or a tint with it.

The app rebuilds the view when a drag enters, leaves or drops, on each move
over a target whose view reads its state (so `position` stays current), and on
every frame while a level eases; then it goes idle again. A target whose state
the view reads doesn't get the app's highlight: it draws its own.

Drop target ids are handed out in registration order, like scroll ids, so the
state follows the target as long as the view registers its drop targets in the
same order each time. `dropdemo` uses both: its zones' words come from
`DropState`, their look from `.drag_over` / `.drag_refused` with a transition
(a spring with a little overshoot on the first zone, an ease on the second).

`Element::on_drop_hover(cx, |s, hovering| ...)` also tells the state when an
acceptable drag starts and stops hovering a target.

## Dragging inside the app

`Element::drag_source` lets an element be dragged: pressing on it and moving
4 px starts a drag whose payload the handler returns, usually
`DropData::Internal` naming the item (`None` declines). The drag hovers and
drops on drop targets exactly like one from another app, so reordering a list
is a drop target per row that accepts `DragOffer::INTERNAL`. A press that
becomes a drag doesn't tap; Escape cancels it.

```rust
row_label.drag_source(cx, move |_| Some(DropData::Internal(item.id.clone())))
```

## Across windows, and out of them

An in-app drag of a `DropData::Internal` payload dragged out of its window
goes on over the app's other windows: their drop targets hover and take it as
they would in the window it started in (a tab dropped on another window's tab
bar, say). Let go outside every window of the app, it tells its source, if
the source asked with `Element::on_drop_outside`:

```rust
tab.drag_source(cx, move |_| Some(DropData::Internal(id.to_string())))
    .on_drop_outside(cx, move |s, _data, at| s.open_in_new_window(id, at))
```

`at` is where it was let go on the desktop, in logical pixels, or `None` where
the system doesn't say where windows are (Wayland: there, any release outside
the window it started in counts as outside, as the drag can't tell another of
the app's windows from the desktop). While the drag is outside every window,
a picture of the source element follows the pointer, in a borderless window
of its own, on X11 and Windows; macOS (whose windows keep a title bar) and
Wayland show none, only the cursor. The app finds the window under the
pointer from where each window says it is (`WindowInfo::position` and its
frame), and gets the moves outside the window because a press captures the
pointer (an implicit grab on X11 and Wayland, `SetCapture` on Windows, AppKit
on macOS). The [tab bar](widgets.md#tab-bar) is built on it.

## Dragging out of the app

The same `drag_source` drags files and text into other applications. There is
nothing else to call: return `DropData::Files` or `DropData::Text` as the
payload, and the drag works in the app as above while the pointer stays in
the window; once the pointer leaves the window with the button still down,
the drag becomes the platform's, and any application that takes files or
text (a file manager, an editor, a browser, a terminal) can drop it.

```rust
attachment.drag_source(cx, move |_| Some(DropData::Files(vec![path.clone()])))
quote.drag_source(cx, move |s| Some(DropData::Text(s.quote.clone())))
```

- `DropData::Internal` never leaves the app (`DropData::can_leave_app`): it
  means nothing anywhere else. A drag of one past the window's edge stays an
  in-app drag, and dropping it outside does nothing.
- Once out, the platform owns the drag: its cursor, Escape to cancel, the
  target's verdict. In-app targets stop hovering. If the pointer comes back
  over the window, the drag arrives like one from another application:
  targets get `DropData::Files` or `DropData::Text`, not your own payload.
- Leaving offers a copy. The app isn't told whether, or where, it dropped.
- The platform gets the drag through `Window::start_drag`, which the App calls
  for you; a backend that can't take it returns `false`, and the drag stays
  in the app.

On a touch screen (iOS, Android) dragging out starts with a long press, as
the platform's own drags do: holding a finger still on a `drag_source`
whose payload is files or text lifts it out of the app at once, a system
drag with a picture of the element under the finger, which other apps (and
this one's drop targets) can drop. A finger that moves right away drags in
the app as above. A long press on such a source lifts it instead of opening
its context menu; a source of an `Internal` payload keeps its long press.
The App hands the lift to the platform through `Window::lift_drag`, on its
own long press and when the platform's lift gesture asks
(`Event::DragLift`, iOS); nothing else changes for the app.

| Platform | How | Files | Text |
|---|---|---|---|
| X11 | XDND v5 source, by hand on the wire: owns `XdndSelection`, grabs the pointer and keyboard, finds the `XdndAware` window under the pointer, `XdndEnter` / `Position` / `Leave` / `Drop`, serves `SelectionRequest`s, waits for `XdndFinished` | `text/uri-list` | `UTF8_STRING`, `text/plain;charset=utf-8`, `text/plain`, `STRING` |
| Wayland | a `wl_data_source` handed to `wl_data_device.start_drag` with the press's serial (copy action from v3); `send` is served from memory; `cancelled` / `dnd_finished` end it | `text/uri-list` | `text/plain;charset=utf-8` and friends |
| Windows | OLE `DoDragDrop` with hand-written `IDataObject` and `IDropSource` vtables, run by the message loop after the handler returns | `CF_HDROP` | `CF_UNICODETEXT` |
| macOS | `beginDraggingSessionWithItems:event:source:` on the view, which is the `NSDraggingSource` | an `NSURL` per file, with its Finder icon | `NSString` |
| iOS | a long press: `UIDragInteraction` on the view (enabled on iPhone too) recognizes the lift and asks for the items under the finger (`Event::DragLift`); the element's picture is the lift's preview | an `NSItemProvider` per file with a file representation of its type (`registerFileRepresentationForTypeIdentifier:`, the type from `UTType` by extension), named after the file | an `NSItemProvider` of the `NSString` |
| Android | a long press: `View.startDragAndDrop` from the host view, `DRAG_FLAG_GLOBAL \| DRAG_FLAG_GLOBAL_URI_READ`, the element's picture as the shadow | `content:` URIs of the app's `FileProvider` (`org.stipple.android.Files`, which the manifest declares; see [android.md](platforms/android.md#packaging)), typed by extension | `ClipData` text |
| Web | not yet: the drag stays in the app | | |

Each platform shows its own drag feedback (X11: the core "fleur" cursor over
a target that accepts, a "no" circle elsewhere; macOS: the file's icon), not
an image of the element.

## Platforms

The platform layer reports drags from other applications as
`Event::DragEnter { position, offer }`, `DragOver { position }`, `DragLeave`
and `Drop { position, data }`. The app answers each enter and move through
`Window::set_drop_accepted`, which the backend passes on to the source.

| Platform | Protocol | Files | Text |
|---|---|---|---|
| X11 | XDND v5, by hand on the wire | `text/uri-list` | `UTF8_STRING`, `text/plain` |
| Wayland | `wl_data_device` / `wl_data_offer` (copy action from v3) | `text/uri-list` | `text/plain;charset=utf-8` and friends |
| Windows | OLE `IDropTarget` (hand-written COM vtable); `WM_DROPFILES` too | `CF_HDROP` | `CF_UNICODETEXT` |
| macOS | `NSDraggingDestination` on the view | `public.file-url` (resolved through `NSURL`) | `public.utf8-plain-text` |
| iOS | `UIDropInteraction` on the view | images and other files (`NSItemProvider` file representations, copied) | `NSString`, and web links (`NSURL`) as their URL |
| Android | `View.onDragEvent` on the host view | `file:` URIs as paths, `content:` URIs copied through the `ContentResolver` | `ClipData` text (and `coerceToText` for the rest) |
| Web | not yet | | |

On Windows and macOS the OS asks for the answer inside its own callback, where
the app can't run, so the backend answers with the app's verdict from the
previous move (a drag first entering is answered "copy" if it carries files or
text). The cursor's badge can trail by one move; the drop itself always goes
where the app says. Android does the same for the drop's result (its
`DragEvent`s come on the main thread, not the app's); iOS asks on its main
thread, where the app's handler runs, so its `UIDropProposal` is the app's
answer to that very move.

### iOS and Android

On phones and tablets a drop comes from another app beside this one (iPad
split view and Slide Over; Android's split screen and freeform windows; an
iPhone app since iOS 15, with the drag held while switching apps). The
events and `DropData` are the desktop's. What the other app drags comes as
typed items, which the backend reads after the drop and before delivering
it (`Event::Drop` arrives a moment after the finger lifts):

- Text is `DropData::Text`; on iOS a web link (a `public.url` that isn't a
  file) is its URL as text.
- Anything else (a photo, a PDF, a file from the Files app) only lasts while
  the drop is handled, so it is copied, under its own name (the provider's
  `suggestedName` on iOS, `OpenableColumns.DISPLAY_NAME` on Android, with an
  extension from its type if it has none), into a directory of the drop's
  own: `<tmp>/stipple-drops/<n>/` (iOS: the app's `tmp`; Android: its cache's
  `tmp`). The drop is `DropData::Files` with the copies' paths; delete them
  once read. An Android `file:` URI is passed as its path.
- A drop of several items carries every file if any; otherwise their texts,
  one per line.

Android asks for the drag's URI permissions (`requestDragAndDropPermissions`)
when it drops, and copies `content:` URIs on a thread of its own (a cloud
provider may fetch the file first). iOS loads each item through
`NSItemProvider` (`loadObjectOfClass:` for text and links,
`loadFileRepresentationForTypeIdentifier:` for files), off the main thread.
The target's verdict changes iOS's drag badge (copy or forbidden); Android
shows none.

`file:` URIs are percent-decoded to paths (`stipple::platform::parse_uri_list`);
other schemes, and files on other hosts, are skipped.

Dragging out works the other way (see [above](#dragging-out-of-the-app)): a
long press on a drag source of text or files lifts it. On iOS the drag
interaction's own long press asks the app for the items there, and each
file goes as a provider that hands the file over when the receiver loads
it (the receiver gets a copy). On Android each file goes as a `content:`
URI naming it (`content://<package>.stipple.files/root/<path>`), which the
app's provider opens read-only for the app the drag granted it to; the
provider has no state of its own, so a target may read it after the drag
(while its grant lasts) even if the app's process was restarted.

## Testing

- `App::drag_enter`, `drag_over`, `drag_leave` and `drop_data` feed drags to an
  app in tests, and `pointer_press` / `pointer_drag_to` / `pointer_release`
  drive in-app drags; `App::render_frame` builds the frame.
- `dropdemo --expect N` exits once N drops have landed, printing a line per
  drop. CI runs it on X11 with `.github/scripts/xdnd-drop.py` (an XDND source
  in python3-xlib: files, text, a refused drag, screenshots of the zone's
  accepted and refused looks) plus an in-app chip drag by `xdotool`. For
  dragging out, `xdotool` drags dropdemo's "File" and "Text" chips out of the
  window onto `.github/scripts/xdnd-target.py` (an XDND target in
  python3-xlib, beside the app), which must receive the file's URI list and
  the text; another drag out is cancelled with Escape (the target sees a
  leave, no drop), and one goes out and back onto the app's own zone, which
  takes it as a drop of files. On
  Windows CI tries `.github/scripts/ole-drop.ps1` (real OLE `DoDragDrop`
  from another process, for a file and for text), but the drag doesn't
  complete on the runner yet, so that step only reports; on macOS with
  `STIPPLE_COCOA_DROP=<path>`, which drives the view's own dragging methods
  with a stand-in `NSDraggingInfo` whose pasteboard holds the file's URL.
- On the Android emulator, `examples/androiddemo/test-input.sh` long-presses
  two of `androiddemo`'s drag sources (`adb shell input motionevent`), a
  text and a file, which lift out of the app as system drags, and carries
  them over the app's own window: the text must drop, and the file (a
  `content:` URI of the app's `FileProvider`) must arrive copied, with its
  bytes; then a self-test drag of a `MediaStore` `content:` URI (another
  app's kind of file). `examples/androiddemo/test-drag-apps.sh` puts
  `androiddemo` and a probe app (`.github/clip/android`, a plain Java drop
  target) in freeform windows (in split screen on API 33, where `am task
  resize` doesn't move a freeform window's surface) and lifts the same two
  into the probe, which must log the text, and the file's name, type and
  bytes as its `ContentResolver` reads them. `test-input.sh` passed on local
  API 34 and 30 emulators and in CI on API 34, 33 and 30;
  `test-drag-apps.sh` locally on API 34, 33 and 30 and in CI on API 34, 33
  and 30 (three runs in a row on each in a trial run). CI runs them without
  blocking.
- On the iOS simulator `touchdemo`'s self-test calls the drop interaction's
  delegate with a stand-in session over real `NSItemProvider`s (a text, a
  file); and the drag interaction's, asking for the items under its two drag
  sources as UIKit does after the long press, then drops those items on its
  pinch box through the drop interaction, which must read the text and a
  copy of the file. An XCUITest (`StippleRealTouchTests`) long-presses a
  drag source with a real touch and drags it onto the pinch box: a real
  lift and drop within the app. Both passed in CI (one run). A drag between
  two apps can't be scripted on the simulator (`simctl` and XCUITest can't
  open a second app beside it). Not blocking yet. The data conversion (types, file names, the drop's
  directory, merging items; Android's provider URIs) is unit-tested on every
  host (`src/platform/backend/mobile_drop.rs`, `mobile_drag.rs`), and the
  App's lift (`Window::lift_drag`) against a stand-in window.
- Wayland's protocol handling is unit-tested against a scripted compositor
  (drops, and the drag-out source: `start_drag`, `send`, `dnd_finished`); CI
  doesn't drag on Wayland. The X11 source's state machine and messages are
  unit-tested too, and the App's hand-off (`Window::start_drag`) against a
  stand-in window.
- Dragging out on Wayland, Windows and macOS is build-checked only: CI can't
  drive a compositor's drag, OLE's modal drag loop or an `NSDraggingSession`
  with a real pointer.

## Not yet

- A picture of the element following an in-app drag inside the windows, and
  outside them on macOS and Wayland.

- Telling the app how a drag out ended (dropped, where, or cancelled), and a
  move action (delete the source's copy).
- `XdndProxy` and large (`INCR`) transfers on X11, and other formats than
  files and plain text (images, rich text, a file's contents) anywhere.
- On Windows the app's handler doesn't run while `DoDragDrop`'s modal loop
  does, so a drag out that comes back over the window shows no drop-target
  feedback; its events (and a drop, which lands) are delivered when the drag
  ends.
- A drag image following the pointer during in-app drags.
- Drops on the web.
- Dragging out of the app on iOS (`UIDragInteraction`, which lifts an item
  on a long press rather than when a drag leaves the window) and Android
  (`View.startDragAndDrop`, which would need a `FileProvider` for files).
