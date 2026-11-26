# Accessibility

Stipple draws every control itself, so the operating system can't see them on
its own. Each frame, the `App` builds an accessibility tree from the layout
(`stipple::core::AccessNode`: roles, names, bounds, focus, selected / checked /
expanded state, a text field's caret and selection, which element labels
a field, where a scroll container is scrolled, a slider's or a progress
bar's value along its range, a table's size and each cell's place in it,
and how text is drawn), maps it to the platform
layer's neutral (crate-internal) `A11yNode` tree, and gives it to the window,
whose backend exposes it to the platform's accessibility API.
`App::accessibility_tree()` returns the tree for tests.

| Platform | What the tree reaches | Checked in CI from another process |
| --- | --- | --- |
| Windows | UI Automation: a hand-built provider per node | yes, `System.Windows.Automation` |
| macOS | NSAccessibility: an `NSAccessibilityElement` per node | yes if the runner allows the AX API (see below) |
| iOS | UIAccessibility: a `UIAccessibilityElement` per node | yes, an XCUITest runner |
| Android | an `AccessibilityNodeProvider`, a virtual view per node | yes, an accessibility service, and `uiautomator` |
| Web | an ARIA overlay | in-page, from headless Chrome |
| Linux (X11, Wayland) | AT-SPI: an object per node, registered with the registry | yes, a pyatspi client (X11); sway and Orca required too |

None of these has been tried with a screen reader by a person. What CI
does, on each platform where it can, is read and drive the app from
**another process**, through the same channel the screen reader uses, and on
four of them it also runs **the screen reader itself** and checks what it
says or does:

| Screen reader | CI job / step | Status |
| --- | --- | --- |
| NVDA (Windows) | `screen-reader-windows` | passing in CI; required |
| VoiceOver (macOS) | `visual-macos`, *Screen reader (VoiceOver)* | passing in CI; required |
| TalkBack (Android) | `visual-android` (API 34) | passing in CI; required |
| Orca (Linux) | `a11y-dbus` (see [Linux](#linux)) | passing in CI; required |
| Narrator, JAWS, VoiceOver on iOS | none | not tried |

The rest of this page says what each check proves, and what it doesn't.

## Scrolling

A scroll container (`Element::scrollable`, which `widgets::scroll` and
`scroll_horizontal` make) carries where it is scrolled in its node
(`AccessNode::scroll`, an `AccessScroll`: the offset from its start, the
largest offset, the viewport's length, whether it scrolls sideways and
whether it runs right to left). A list, tree or table takes the scroll of
the container that is its body, so the one container isn't reported twice.
Every platform exposes it, and every one scrolls it the same way: a wheel at
the container's middle, which the app gives the innermost container under it
that scrolls that way (`platform::access`, shared by the bridges):

| Platform | What a scroll container is | How a screen reader scrolls it |
| --- | --- | --- |
| Windows | a pane (or the list) with the Scroll pattern; its items have ScrollItem | `SetScrollPercent`, `Scroll` (a page or a line), `ScrollIntoView` |
| macOS | an `AXScrollArea` (a list keeps its role) with an `AXScrollBar` | the bar's `AXValue` (0 to 1), `AXIncrement` / `AXDecrement` |
| iOS | elements inside it | `accessibilityScroll:` (VoiceOver's three-finger swipe), a page, with "Page N of M" |
| Android | a scrollable node | `ACTION_SCROLL_FORWARD` / `BACKWARD`, a page, offered only where it can go; `ACTION_SHOW_ON_SCREEN` on what's inside; `TYPE_VIEW_SCROLLED` when it moved |
| Linux | a scroll pane (a list keeps its role) with `Value` (the offset) | `Action` "scroll forward" / "scroll backward", setting `Value.CurrentValue`, `Component.ScrollTo` on what's inside |
| Web | the overlay element, with the scroll in its data | focus moving to a control scrolled out of view brings it back |

A page is 80% of the viewport. Bringing a node into view
(`platform::access::reveal`) scrolls every container on the way down to it,
outermost first, by the least that shows it whole, each with a wheel over a
part of that container that shows and isn't an inner container scrolling
the same way; a container scrolling both ways (a wide table's body, read
from its horizontal bar) scrolls down then sideways, and a cell of a table
with frozen columns comes out beside them, not under them. **An activation or a focus on a node scrolled out of view
brings it into view first**, then clicks it where it then is (its middle,
within what the containers show): AT-SPI's "click" action and `GrabFocus`,
UIA's Invoke, Toggle, SelectionItem and ExpandCollapse, macOS's `AXPress`
on Stipple's own elements, Android's click, focus and select, and the web
overlay's click. Before, the click landed where the item was laid out,
outside the viewport, on whatever was drawn there (or outside the window).

App tests (`src/a11y_scroll_tests.rs`, `src/a11y_relations_tests.rs`)
scroll a plain container a page, to its end and back to an item through the
events the bridges make, check the tree's offsets follow, and activate an
item laid out below its list in one delivery (the scroll, then the click):
the item's handler runs (a bare click at its middle misses it) and it is in
view after. Not covered: iOS's `accessibilityActivate` still taps the
element's frame (VoiceOver only stops on what shows there).

## Relations

The tree says how nodes relate (`AccessNode`, and `platform::access::relations`
works the relations out once for every bridge):

- **labelled-by / label-for**: a control names its caption with
  `Element::labelled_by(id)`, the caption having `Element::a11y_id(id)`.
- **controller-for / controlled-by**: `Element::controls(id)` names the
  element a control controls. A `select` and a `combo_box` control their
  open list (an id they give it), a `TabBar` with a `panel(id)` has its
  selected tab control that panel; an app marks others itself.
- **member-of**: radio buttons side by side under one parent (a column of
  `radio`s, `choice_chips`) are one group, each a member of it.
- **node-child-of / node-parent-of**: a tree item's parent item. `tree`
  gives each row its depth (`Element::a11y_level`, 1 at the top); in such a
  flat tree the nearest item before one, a level up, is its parent. Tree
  items nested in tree items are related by where they sit.

| Platform | Labels | Controls | Groups and tree items |
| --- | --- | --- | --- |
| Linux (AT-SPI) | `labelled-by` / `label-for` | `controller-for` / `controlled-by` | `member-of`; `node-child-of` / `node-parent-of`, and a tree item's `level` attribute |
| Windows (UIA) | (the field's name is its caption) | `ControllerFor` | a tree item's `Level`; UIA fills in `PositionInSet` / `SizeOfSet` from the siblings |
| macOS | `AXTitleUIElement` / `AXServesAsTitleForUIElements` | `AXLinkedUIElements` | an outline row's `AXDisclosureLevel`, `AXDisclosedByRow`, `AXDisclosedRows` |
| Web | (the field's name is its caption) | `aria-controls` | a tree item's `aria-level` |

Ids are matched within a window's tree. Nothing is inferred beyond the
above: a label beside a field without them isn't related to it.

### Scroll bars

Each scroll bar is a node too, after its container's content (so the
content keeps its places): `Role::ScrollBar`, at the bar's strip (where a
classic bar is, while an overlay one is hidden), with
`AccessNode::scroll_bar`, an `AccessScroll` for the way it goes: its
orientation (`horizontal`) and its value (`offset`, from 0 to `max`). A
container scrolling both ways ([`scroll_both`](widgets.md#scrolling-both-ways-and-grids),
`text_view`) has two, its vertical and its horizontal one. Setting a bar's
value scrolls its container there, by the same wheel as the other scroll
actions.

| Platform | What a scroll bar is |
| --- | --- |
| Windows | a `ScrollBar` control with its `Orientation` and the `RangeValue` pattern (`Value` the offset, `Minimum` 0, `Maximum` the largest offset, `LargeChange` a page, `SmallChange` a line; `SetValue` scrolls) |
| macOS | the scroll area's `AXScrollBar` (above; settable), and an `AXScrollBar` with its `AXOrientation` and `AXValue` for a two-way area's horizontal bar |
| iOS | left out: UIKit has no scroll bar elements |
| Android | a node with its `RangeInfo` (a float range, 0 to the largest offset, at the offset) |
| Linux | a scroll bar (`ROLE_SCROLL_BAR`) with the `HORIZONTAL` or `VERTICAL` state and `Value` (settable) |
| Web | an ARIA `scrollbar` with `aria-orientation`, `aria-valuenow`, `aria-valuemin`, `aria-valuemax`, and `aria-controls` naming its container |

Unit-tested: the AT-SPI role, states and `Value` (set too), the ARIA
overlay's data, and App tests (`src/scroll_bar_more_tests.rs`) of the tree's
bars and of setting one through the platform mirror. UIA's, macOS's and
Android's are build-checked only.

## Toasts

A toast is an `Alert` node. When one appears (a name the tree before didn't
have), each platform announces it: UIA's notification event (Windows),
`AXAnnouncementRequested` (macOS), `UIAccessibilityAnnouncementNotification`
(iOS), `TYPE_ANNOUNCEMENT` (Android), `object:announcement` from the
notification node, which is also a polite live region (`live`,
`container-live` attributes), on Linux, and an ARIA `alert` on the web.
`touchdemo` on a desktop raises one, "Row 2 tapped", when "Row 2" is tapped,
for the out-of-process checks below.

## Range values, tables and captions

**Range values.** A slider, a progress bar and a number input carry their
value along a range (`AccessNode::range`, an `AccessRange`: value, minimum,
maximum, step, and whether a slider's track runs right to left);
`widgets::slider`, `progress_bar` (now `Role::ProgressBar`) and
`number_input` declare theirs, and any element can with
`Element::a11y_range(AccessRange::between(min, max).value(v).step(s))`. A screen reader reads it, and sets a slider's or a
number input's (a progress bar's is read-only). Setting goes through the
same events on every platform (`platform::access`): a slider is pressed just
inside its box at the value's place and the pointer moved to the exact
place (what its drag handler reads; mirrored on a right-to-left track), a
number input gets the number typed and Enter. An increment is a step (the
range's step, else a twentieth of it).

| Platform | What a range is | How a screen reader sets it |
| --- | --- | --- |
| Windows | the RangeValue pattern (Value, Minimum, Maximum, SmallChange, LargeChange, IsReadOnly) | `SetValue` |
| macOS | `AXValue` (a number) with `AXMinValue`, `AXMaxValue`, `AXValueDescription` ("50%") | setting `AXValue`, `AXIncrement` / `AXDecrement` |
| iOS | the element's value as words ("50%"); a slider is adjustable | `accessibilityIncrement` / `…Decrement` (swipe up and down) |
| Android | `RangeInfo` (as floats) | `ACTION_SET_PROGRESS`, `ACTION_SCROLL_FORWARD` / `BACKWARD` (a step, as on a `SeekBar`) |
| Linux | `Value` (current, minimum, maximum, increment, text) | setting `Value.CurrentValue` |
| Web | `aria-valuenow`, `aria-valuemin`, `aria-valuemax`, `aria-valuetext` | not settable (read-only) |

A moved value raises UIA's RangeValue.Value property change,
`AXValueChanged`, `TYPE_VIEW_SELECTED` (as a `SeekBar` does) and
`object:property-change:accessible-value`.

**Tables.** A table's node says how many rows and columns it has
(`AccessNode::table`), all of them: a virtualized `widgets::table` builds
only the rows in view, so it declares its size (`Element::a11y_table`, its
header row counted as row 0) and each built row its index
(`Element::a11y_row`); scrolled sideways it builds only the columns in view
too, so each built cell and column header declares its column
(`Element::a11y_column`). Every row and cell (and column header) gets its place
(`AccessNode::cell`): a row's index is the one it declares, else the one after
the row before; a cell's column is the one it declares, else the one after
the cell before, through the groups that wrap them (a row's plain groups are
left out of the tree: its cells are its children). A table built by hand from `Role::Table`,
`Role::Row` and `Role::Cell` is counted as it is. `AccessTable::frozen` says
how many leading columns stay put while the others scroll sideways.

| Platform | The table | A cell |
| --- | --- | --- |
| Windows | Grid (`RowCount`, `ColumnCount`, `GetItem`) and Table (column headers, row major) | GridItem (row, column, spans of 1, its grid) and TableItem (its column's header) |
| macOS | `AXRowCount`, `AXColumnCount`, `AXRows`, `AXColumnHeaderUIElements` | `AXRowIndexRange`, `AXColumnIndexRange`; a row's `AXIndex` |
| Android | `CollectionInfo` | `CollectionItemInfo` (a header is a heading) |
| Linux | `Table` (size, `GetAccessibleAt`, index math, column headers, selected rows) | `TableCell` (position, spans, its table, its column's header) |
| Web | `aria-rowcount`, `aria-colcount` | `aria-rowindex`, `aria-colindex` |
| iOS | not exposed | not exposed |

A cell in a row (or a column) the table hasn't built isn't there to give: UIA's `GetItem`
fails, AT-SPI's `GetAccessibleAt` returns a null reference.

**Captions.** A control names its caption with `Element::labelled_by(id)`
and the caption `Element::a11y_id(id)`. A plain unnamed group labelled so
(a widget's wrapper, as a number input's field and steppers) hands it to the
first control inside it. Every bridge takes it from the one labelled-by
relation `platform::access::relations` works out (see
[Relations](#relations)): Linux: `labelled-by` / `label-for`; Windows: the
`LabeledBy` property (and a field whose name is its text is named by its
caption); Android: `setLabeledBy`; web: `aria-labelledby` (by the caption's
id); macOS: `AXTitleUIElement`; iOS: none.

**Text attributes.** A node with text says how it is drawn
(`AccessNode::font`: family, size, weight, italic; the family is the app's
font's when the text names none). Windows serves it as UIA text attributes
(`FontName`, `FontSize` in points, `FontWeight`, `IsItalic`, and
`IsReadOnly`), one run for the node's whole text, which `FindAttribute`
finds; the others aren't exposed yet. A field's text changing and its caret
or selection moving raise UIA's `TextChanged` and `TextSelectionChanged`
events (AT-SPI had its text and caret events already).

App tests (`src/a11y_range_tests.rs`) set a slider (both directions,
exactly to its end, a step down) and a number input through those events,
check a progress bar refuses, number a virtualized table's rows as it
scrolls and a hand-built one's, and follow a caption. `touchdemo` with
`TOUCHDEMO_RANGES` set shows a "Volume" slider, a captioned "Level" number
input, a progress bar and a 20-row table (under the light theme, so they are
Stipple's own and not live OS controls) for the out-of-process checks:
`atspi-walk.py` with `ATSPI_WALK_RANGES=1` (passing locally under Xvfb),
`uia-ranges.ps1` (Windows), `axprobe --ranges` (macOS); `androiddemo` has a
"Volume" slider the Android probe reads and sets. Each is a required CI
step.

## Keyboard focus

Text fields, and other elements with a key handler, take keyboard focus.
So do buttons and the other controls a tap activates: an element with
`on_tap` whose role is a button (the role a tap handler implies), checkbox,
radio button, switch, tab, select, list or tree item, table row, cell or column header. Menu items don't: an open menu takes the
arrow keys, Enter and Escape itself. Neither does a control inside another
focusable element (a password field's reveal button is part of the field,
a list's rows are part of a list that takes focus as a whole).

- Tab moves focus to the next of these in the tree's order, Shift+Tab to
  the previous one, wrapping round. While a modal overlay (a dialog, a
  message box) is up, Tab stays in it.
- Enter or Space presses the focused button: its `on_tap` handler runs, as
  for a tap at its middle. Other keys don't reach it, except the arrows,
  Home and End inside an element made a `focus_group` (a group of chips or
  radio buttons): they move focus to the previous, next, first or last
  control in it, wrapping, with Left and Right swapped in a right-to-left
  layout. In a `FocusGroup::Select` group (radio buttons, `choice_chips`) the
  control focus lands on is pressed too, and Tab stops at one control of the
  group only: the checked one, else the first.
- The focus ring goes round it, a little outside its edge (so it shows on a
  button filled with the ring's color).
- The accessibility tree says it is focused, so each platform announces the
  move as it does for a field: UIA's focus-changed event (Windows),
  `AXFocusedUIElementChanged` (macOS), `object:state-changed:focused` and
  `focus:` (AT-SPI), `TYPE_VIEW_FOCUSED` (Android).
- A click or a tap doesn't focus a button, as AppKit's buttons and the web's
  `:focus-visible` behave: a pointer user never sees a ring left on the
  button they clicked. A click on a button takes the focus away from a field,
  as before; a click on a field focuses it.
- **The ring shows for the keyboard** (`:focus-visible`): after Tab, the
  arrow keys, Home, End, Enter or Space, or a gamepad. A click or a tap on
  something that takes the focus (a navigation bar, a list) moves it there
  without a ring, until a key is used; a text field shows its ring however
  it got the focus, as the web does. A game menu's spatial group taking the
  focus by itself shows it unless a tap came last. The ring is drawn with
  the layer its element is in, so a sheet, a dialog's scrim or a popup over
  the element covers it.

### Moving focus from the app

- `.autofocus()` on an element focuses it the first time it's laid out
  (or the first focusable inside it): the search field of a popup that
  opens, the name field of a dialog, the field of a window that opens. It
  doesn't take focus again while it stays; when it goes away with focus in
  it, focus goes back to where it was before.
- `cx.focus(key)` in a view, and `handle.focus(window, key)` from a handler
  or another thread, focus the element keyed `key` (`Element::key`), or the
  first focusable inside it, once the window's next build is laid out, so an
  element the same handler made appear can take it. A find bar that closes
  gives focus back to the terminal with
  `handle.focus(cx.window_id(), "terminal")`. `cx.blur()` and
  `handle.blur(window)` take focus away.
- While a modal overlay is up, focus only moves into it (or the overlays
  above it), as Tab does.

`App::focus_next` and `App::focus_prev` move it from tests. The App tests in
`src/tab_focus_tests.rs` tab across a field, two buttons and a checkbox
both ways, press with Enter and Space, check the ring's pixels, that a click
leaves no focus on a button and that a modal dialog keeps Tab inside it;
`src/focus_tests.rs` checks autofocus, focus by key and a dialog's Enter and
Escape.

## Windows

The window answers `WM_GETOBJECT` with the root of a tree of hand-built UI
Automation providers (`stipple::platform::uia`, COM vtables written by hand).
The root is hosted by the window's own provider (`UiaHostProviderFromHwnd`),
so it sits in the desktop's tree under the window. Each node answers its
control type, name, bounds in screen pixels (logical bounds scaled by the
window's DPI and offset by the client area's position, worked out when
asked), keyboard focusability and focus; navigation goes to the parent,
siblings and children; the root finds the focused node and the deepest node
under a point. Buttons, menu items, checkboxes, radio buttons, switches,
selects, tabs and column headers have the Invoke pattern, which clicks the middle of the node.
The tree is rebuilt only when it changes, and a replaced tree is kept while a
client still holds any of its elements. When focus moves to another node
(Tab), the window raises UIA's focus-changed event
(`UiaRaiseAutomationEvent`, `UIA_AutomationFocusChangedEventId`) for it, if
a client listens: screen readers follow focus through that event, and
without it only learn of a Tab when they ask again. The root, which has no
name of its own, leaves its name to the window's provider, so it reads as
the window's title ("Stipple Touch window").

Besides Invoke, each provider has the control patterns its node calls for,
each an interface with a vtable of its own (`platform/uia/patterns.rs`,
`uia/text.rs`; which node gets what, its name and value, the text units,
the scroll math and the events a swap plans are worked out without COM in
`platform::uia_logic`):

- **RangeValue** on sliders, progress bars and spin buttons, **Grid** and
  **Table** on tables, **GridItem** and **TableItem** on their cells and
  column headers (`platform/uia/grid.rs`; see
  [above](#range-values-tables-and-captions)), and the `LabeledBy` property.
- **Value** on text fields and spin buttons. A field whose text the tree
  carries is named by the caption it is `labelled_by` (or nothing) and
  valued by its text, so a screen reader doesn't read it twice; `SetValue`
  focuses it and types the new text through the input-method path.
- **Toggle** on checkboxes and switches (a click), which have their own
  control types now, as selects, spin buttons, toolbars and separators do.
- **Selection** on lists, trees, tables and tab bars, and **SelectionItem**
  on their items and on radio buttons (select is a click).
- **ExpandCollapse** on what expands and on selects (a tree item opens with
  a click and the arrow key its tree takes).
- **Relations** (`uia/relations.rs`): `ControllerFor` names the elements a
  node controls (a select's open list, a tab's panel) and a tree item has its
  `Level` (see [Relations](#relations)). Invoke, Toggle, SelectionItem and
  ExpandCollapse on a node scrolled out of view scroll it into view before
  their click (`uiademo --relations` checks the properties and a scrolled
  Invoke in-process, in a required step).
- **Scroll** on scroll containers (a plain `Element::scrollable` is a Pane):
  the percentages and view sizes, `Scroll` a page or a line, and
  `SetScrollPercent`; **ScrollItem** on what they hold (`ScrollIntoView`).
- **Text** on fields, spin buttons and labels: the document range, the
  selection (or an empty range at the caret), and ranges that read their
  text, select it in the field, expand to and move by characters, words,
  lines and the whole text, compare their endpoints, find text and report
  their boxes (the node's box shared evenly between its characters). A
  range is a COM object of its own, freed on its last release, holding a
  reference on its element. Its text attributes are the node's font (one
  run: `FontName`, `FontSize`, `FontWeight`, `IsItalic`, `IsReadOnly`),
  which `FindAttribute` finds.

A tree swap raises, besides the focus change, property-changed events
(Name, Value, ToggleState, ExpandCollapseState, SelectionItemIsSelected,
the scroll percentages, RangeValue.Value), `TextChanged` and
`TextSelectionChanged` on what has the Text pattern, a `ChildrenInvalidated`
structure change where
children came or went, `ElementSelected` for a newly selected item, and
`UiaRaiseNotificationEvent` for each toast that appeared (looked up at run
time: Windows 10 1709 and later), which Narrator and NVDA read out. Pattern
actions reach the window as events queued for it (`queue_a11y_events`),
delivered once UIA's call returns.

**CI** (`visual-windows`, *Read and drive a running window through UI
Automation*): `.github/scripts/uia-walk.ps1` starts `touchdemo` and, from
PowerShell's process, uses the .NET UI Automation client (the cross-process
path Narrator, NVDA and JAWS use):

- gets the "Stipple Touch" window from its handle (`AutomationElement.FromHandle`
  on the process's main window, so no other top-level window is asked
  anything), and walks every
  element under it with the control-view walker, printing each one's control
  type, name, bounds, focusability and patterns;
- finds a `Button` "Row 0", an `Edit` (the text field) and a `Text`
  "Rows tapped: N";
- checks the button's bounds are inside the window's on screen, and that
  `AutomationElement.FromPoint` at its middle returns it;
- invokes the button through its Invoke pattern: the app logs the tap and the
  tree, read again, says "Rows tapped: N+1";
- presses Tab and reads `AutomationElement.FocusedElement`, which must be a
  control in the window with keyboard focus (when the window got the
  foreground).

`uiademo` still checks the providers through their vtables in-process, and
with `--patterns` drives every pattern through the interface
`GetPatternProvider` hands out (RangeValue on a slider and a progress bar,
Grid, Table, GridItem and TableItem on a table, `LabeledBy`, and the text
attributes too).

A further step, *UI Automation patterns and events*,
runs `uiademo --patterns`, then `.github/scripts/uia-patterns.ps1` against
`touchdemo`: the field's Value is read and set (the app logs the text) and
its Text read (the document range, the selection); "Remember" is toggled
(the app logs it, the state reads back, a property-changed event comes);
the list's Pane goes to 100% with `SetScrollPercent` (logged) and ScrollItem
brings "Row 0" back; the strip pages with `Scroll`; and invoking "Row 2"
raises a toast, which must appear in the tree with a structure-changed
event. .NET's client has no API for UIA's notification event, so that one
is left to NVDA. It passes on the runner and is required.

**NVDA** (`screen-reader-windows`): `.github/scripts/nvda-windows.ps1`
downloads a pinned NVDA release from nvaccess.org (checked against its
SHA-256), makes a portable copy with the launcher's
`--create-portable-silent` (or unpacks the launcher with 7-Zip if that
fails), and starts it with `-m` (no dialogs, sounds or start message), its
log at debug level, and a fresh configuration: no welcome, update or usage
dialogs, the laptop keyboard layout, Insert as the NVDA key, and a synth that
makes no sound (`.github/a11y/windows/stippleSpy.py`, loaded from the
configuration's scratchpad; NVDA's own "silence" synth never reports its
speech done, which stops say all). NVDA logs everything it says as
`Speaking [...]`, so the script reads its log while it types NVDA's
commands (`keybd_event`, which NVDA's keyboard hook takes as typed keys):

- the window brought to the front: NVDA names it "Stipple Touch";
- Tab: the app focuses its field, raises the focus event, and NVDA says
  "edit"; NVDA+Tab (report the focus) says it again;
- NVDA+B (read the foreground window): NVDA reads the controls through UIA,
  "Row 0" with its role "button" and "Rows tapped: 0";
- NVDA+Shift+Right (next object) from the field, until NVDA's navigator says
  "Row 0" "button";
- NVDA+Enter (activate the navigator object): NVDA invokes the button
  through UIA, the app logs the tap, and NVDA+B then reads "Rows tapped: 1";
- the keyboard alone: Tab until NVDA announces "Row 0" (the app focuses the
  button and raises the focus event), which it must call a button; Space and
  then Enter press it (the app logs both taps); Shift+Tab goes back to the
  field ("edit");
- the mouse moved over "Row 1": NVDA's mouse tracking hit-tests through UIA
  and says what is there (reported, not required);
- "Row 2" invoked through UIA: its toast is announced with UIA's
  notification event, and whether NVDA speaks "Row 2 tapped" is reported,
  not required (not yet run).

The job uploads NVDA's whole log. It passes in CI (the window's name isn't
spoken on the foreground change alone, only with the first focus inside
it, so that is reported, not required); the job is required.

The keyboard steps (Tab to the button, Space, Enter, Shift+Tab) were added
after the rest passed and haven't been seen in CI yet.

A further step, *UI Automation range values, tables and text attributes*
(`.github/scripts/uia-ranges.ps1`, on `touchdemo` with `TOUCHDEMO_RANGES`),
reads and sets the slider's RangeValue (the app logs the new volume, a
property-changed event comes), reads the progress bar's (read-only), sets
the spin button's and follows its `LabeledBy` to "Level", reads the table's
Grid and Table patterns and a cell's GridItem and TableItem, a label's
`FontName`, `FontSize` and `FontWeight`, and waits for the field's
`TextChanged` and `TextSelectionChanged` events. It passes on the runner
and is required (and `uiademo --patterns`, required, drives the new
patterns in-process).

Not covered: text attributes beyond the font (colors, alignment…) and
runs within a node's text (rich text reads as one), glyph-accurate
character boxes, `RangeFromChild`; no Window pattern; Grid and Table on
tables only (not trees or lists); `CanSelectMultiple` is always false; the root's `ChildrenInvalidated`
carries the tree's runtime id, not the window's. The patterns and events
are build- and clippy-checked for Windows and their logic unit-tested on
every host, and run on the Windows runner in CI (required). Narrator and JAWS aren't tried.

## macOS

The view is an accessible `AXGroup` whose `accessibilityChildren` are
`NSAccessibilityElement`s built from the tree, each with its role
(`AXButton`, `AXTextField`, `AXStaticText`...), label and frame in screen
coordinates (the node's bounds converted through the view's window). The
focused node's element is `AXFocused`, the view answers
`accessibilityFocusedUIElement` with it (what an assistive app reads as the
application's `AXFocusedUIElement`), and when focus moves to another node
the window posts `AXFocusedUIElementChanged` on its element, which VoiceOver
follows.

The elements are run-time subclasses of `NSAccessibilityElement` that
remember their node's path (`macos/ax_value.rs`; what they say is worked out
by the AppKit-free `macos/ax_plan.rs`):

- **Values.** A text field whose name is its text has that text as its
  `AXValue`, with `AXNumberOfCharacters`, `AXSelectedTextRange` /
  `AXSelectedText` (UTF-16 units), `AXInsertionPointLineNumber` (0: one
  line) and `AXVisibleCharacterRange`; its label is then its caption
  (`labelled_by`), or none, so VoiceOver doesn't read the text twice. A
  password field is an `AXTextField` with the `AXSecureTextField` subrole
  and no value. Checkboxes are `AXCheckBox` (a switch with the `AXSwitch`
  subrole), combo boxes `AXPopUpButton`; checkboxes, switches, radio buttons
  and tabs have an `AXValue` of 0 or 1.
- **Setting.** An assistive app can set a field's `AXValue` (the field's
  text is replaced through the input-method path, after a click to focus it)
  and its `AXSelectedTextRange` (the field selects it); nothing else is
  settable (`isAccessibilitySelectorAllowed:`).
- **Ranges and tables.** A slider or a progress bar has its number as its
  `AXValue`, with `AXMinValue`, `AXMaxValue` and `AXValueDescription`; a
  slider's element (`StippleAXRangeElement`, `macos/ax_range.rs`) takes a
  new `AXValue` and `AXIncrement` / `AXDecrement`. A table has
  `AXRowCount`, `AXColumnCount`, `AXRows` and `AXColumnHeaderUIElements`, a
  row its `AXIndex`, a cell its `AXRowIndexRange` and `AXColumnIndexRange`.
- **Scrolling.** A plain scroll container is an `AXScrollArea` (a list, tree
  or table keeps its role) with an `AXScrollBar` after its content, also
  its vertical or horizontal scroll bar: the bar's `AXValue` (0 to 1) is how
  far it is scrolled and can be set, and `AXIncrement` / `AXDecrement` page
  through it. There's no `AXScrollToVisible`: the protocol has no method
  for it, and overriding the legacy `accessibilityActionNames` to add one
  left every element with no actions as AppKit reported them (`AXPress`
  included; seen in CI), so it was taken out.
- **Links and static text** (`macos/ax_links.rs`, set once a window's
  elements are made): a control `labelled_by` a caption has the caption's
  element as its `AXTitleUIElement` (and the caption it among its
  `AXServesAsTitleForUIElements`), a control has what it controls among its
  `AXLinkedUIElements`, a tree item its `AXDisclosureLevel` (0 at the top),
  `AXDisclosedByRow` and `AXDisclosedRows`, and static text its text as its
  `AXValue` (its label stays), with `AXNumberOfCharacters` and, for
  selectable text with a selection, `AXSelectedTextRange` / `AXSelectedText`.
  `AXPress` on an element scrolled out of view scrolls it into view before
  its click (a live `NSButton` presses itself).
- **Notifications.** A tree swap posts what changed on the rebuilt elements:
  `AXTitleChanged`, `AXValueChanged` (a field's text, a check, a scroll
  bar), `AXSelectedTextChanged`, `AXSelectedChildrenChanged` /
  `AXSelectedRowsChanged`, `AXRowExpanded` / `AXRowCollapsed` and
  `AXLayoutChanged` (children or scrolling changed).

**CI** (`visual-macos`, *Accessibility from outside the app*): `axprobe`
(`.github/a11y/macos/axprobe.swift`) reads a running `touchdemo` through the
AX API, from its own process, as VoiceOver does: `AXUIElementCreateApplication`,
then `AXWindows`, `AXChildren`, `AXRole`, `AXDescription`, `AXTitle`,
`AXValue`, `AXPosition` / `AXSize`, `AXFocusedUIElement`. The step checks the
window "Stipple Touch", an `AXButton` "Row 0", an `AXTextField` and an
`AXStaticText` "Rows tapped: N", and that the button's frame lies inside the
window's. Then it presses the button with `AXPress`
(`AXUIElementPerformAction`, what VoiceOver's VO-Space sends): the button
must list the action, the call must succeed, and the app must log the tap.

The elements of the controls a click activates (buttons, menu items,
checkboxes, radio buttons, switches, selects, tabs, column headers) are a run-time subclass
of `NSAccessibilityElement` whose `accessibilityPerformPress` clicks the
middle of the element (a pointer move, press and release queued for the
window, delivered by the event loop), so AppKit lists `AXPress` for them.

Reading another app through the AX API needs TCC's accessibility permission
for the reading process. `axprobe` prints `AXIsProcessTrusted()`; when the API
is refused (`kAXErrorAPIDisabled`), the step adds `axprobe` to the system TCC
database (GitHub's macOS runners run with SIP off) and tries again. If it is
still refused, the step passes with a warning and the tree hasn't been read
from outside; the in-process walk (`STIPPLE_COCOA_A11Y=1`) still runs.

**VoiceOver** (`visual-macos`, *Screen reader (VoiceOver)*):
`.github/scripts/voiceover-macos.sh` lets AppleScript control VoiceOver as
VoiceOver Utility's checkbox does (`SCREnableAppleScript` in VoiceOver's
defaults, and `/private/var/db/Accessibility/.VoiceOverAppleScriptEnabled`,
which root can write with SIP off as on GitHub's runners), grants Apple
Events to VoiceOver in the TCC databases for the processes that may run
`osascript` there, and turns off VoiceOver's welcome. Then it starts
`touchdemo` and VoiceOver (`VoiceOverStarter`), and moves VoiceOver's cursor
through the window depth first (`tell vo cursor to move right`, `move into
item`, `move out of item`, or the VO keys through System Events if
AppleScript refuses one), logging each phrase VoiceOver speaks (`content of
last phrase`), until it reaches "Row 0": the phrase must say it is a button.
`perform action` (VO-Space) must then press it: the app logs the tap. Then
the keyboard alone, typed through System Events: Tab twice focuses "Row 0"
(what VoiceOver says then is logged, not required), and Space and Return
must each press it (the app logs the taps); these were added later and
haven't run in CI yet. The step passes in CI and is required (it exits 3,
with a warning, when the runner doesn't let AppleScript control VoiceOver).

A further step, *Accessibility depth from outside the app (AX API, values
and scrolling)*, runs `axprobe --depth` on a fresh `touchdemo`: the field's
value is set to "hi" (the app logs it, and it reads back two characters),
its selected range is set and read back, the "Remember" checkbox reads 0,
then 1 after `AXPress` (the app logs it), "Row 2" raises a toast, the list
is an `AXScrollArea` whose scroll bar's value, set to 0.5, scrolls it; an `AXObserver` reports the
notifications it hears. It passes on the runner and is required. Another,
*Title elements, static text values and a press out of view*, runs
`axprobe --links`: the field's `AXTitleUIElement` must be the static text
"Note", "Rows tapped: N" must have its text as its `AXValue`, and `AXPress`
on "Row 20", below the list, must reach the app. It failed on the runner
because `touchdemo` gave its field the "Note" caption only on Linux; it now
has it on macOS too, which passed in a CI scratch run; the step stays
non-blocking until it has passed on master. Another, *Range values and a table from outside the app*, runs
`axprobe --ranges` on `touchdemo` with `TOUCHDEMO_RANGES`: the slider's
value and bounds, `AXIncrement` and setting its value (the app logs both),
the progress bar's value, the table's row and column counts and `AXRows`,
and a cell's index range; it passes on the runner and is required.

Not covered: the elements are
built again after each tree change (or when the window moves), so an
observer registered on an old element goes quiet; no `AXScrollToVisible`
(see above). The depth checks pass on the runner (required), with the
AppKit-free logic unit-tested on every host; the links (`ax_links.rs`) are
build- and clippy-checked and unit-tested the same way.

## iOS

The view is a `UIAccessibilityContainer` with one `UIAccessibilityElement`
per node: label, value (a field's text), traits, frame in screen
coordinates, and `accessibilityActivate` (VoiceOver's double tap) tapping the
node. UIKit has no public trait for a text field, so a field's element adds
the traits a `UITextField` reports (the text-entry trait VoiceOver reads as
"text field"). See [the iOS page](platforms/ios.md#accessibility).

**CI** (`visual-ios`, *Accessibility from outside the app (XCUITest)*): an
XCUITest bundle (`.github/a11y/ios`, an Xcode project generated by XcodeGen
on the runner) runs in its own process on the simulator and drives
`touchdemo` through the AX channel VoiceOver uses:

- `XCUIApplication(bundleIdentifier:)` launches the app, and the tree is read:
  the counts of all elements, buttons, static texts and text fields (one),
  and the whole snapshot, printed;
- the "Row 0" button exists, has a frame on screen and is hittable there;
- a tap on it (XCUITest taps at the element's frame) reaches the app: the
  static text "Rows tapped: N" becomes "Rows tapped: N+1";
- the field is found among `textFields`, a tap on it brings up the software
  keyboard, "H" and "i" are tapped on the keyboard's keys (found through
  accessibility too), and the field's value then holds "Hi".

`touchdemo`'s self-test also reads the elements in-process and activates a
row through `accessibilityActivate`, which XCUITest can't call.

VoiceOver's three-finger swipe (`accessibilityScroll:`) on an element
scrolls the innermost scroll container around it along the swipe's axis by
a page (up or left for the next page, the other way in a right-to-left
strip), and VoiceOver says where it is ("Page N of M",
`UIAccessibilityPageScrolledNotification`); it answers NO when nothing can
move, so UIKit tries further up. Unit-tested; XCUITest doesn't exercise it.

A slider is adjustable (`UIAccessibilityTraitAdjustable`): VoiceOver's swipe
up and down call `accessibilityIncrement` / `…Decrement`, which step it; a
slider's and a progress bar's value reads as words ("50%"). Unit-tested;
build-checked only.

Not covered: tables (`UIAccessibilityContainerDataTable`) and captions;
VoiceOver's own gestures and speech aren't exercised.

## Android

The host view's `AccessibilityNodeProvider` gives a virtual view per node:
the closest widget class (`Button`, `EditText`, `TextView`...), text, state
(focused, selected, checked, editable, password), bounds on screen, and
actions (accessibility focus, click, focus, select, set text, and scroll on
every scroll container whose content overflows: a page forward or back,
each offered only while it can go that way, sideways for a horizontal
strip; a container that scrolled sends `TYPE_VIEW_SCROLLED`; and, on API 23
and later, `ACTION_SHOW_ON_SCREEN` on a node inside a container that
scrolls, which scrolls it into view), a slider's, progress bar's or number
input's `RangeInfo` (a slider set with `ACTION_SET_PROGRESS` or stepped by
scroll forward and backward, and `TYPE_VIEW_SELECTED` when it moved), a
table's `CollectionInfo` and its cells' `CollectionItemInfo`, and the caption
a control is labelled by (`setLabeledBy`). A click, focus or select on a node
scrolled out of view brings it into view first, then taps it there. See
[the Android page](platforms/android.md#accessibility).

**CI** (`visual-android`): a small accessibility service stands in for
TalkBack in the checks TalkBack can't be scripted for:
`.github/a11y/android` (built with javac, d8 and aapt like the app, no
Gradle). `test-probe.sh` installs it and turns it on with
`settings put secure enabled_accessibility_services`, exactly how TalkBack is
enabled; it runs in its own process and reaches the app only through the
system's accessibility framework. It:

- walks the active window's node tree and logs each node (class, text,
  bounds, visibility, actions): the button "Tap me" (`android.widget.Button`
  with a click action) and the field (`EditText`, editable, with set text);
- moves accessibility focus to every node TalkBack would stop on (visible,
  and clickable, editable or with something to say), one at a time in
  reading order, and checks after each that the window's accessibility focus
  (`findFocus(FOCUS_ACCESSIBILITY)`) is on that node, and that
  `TYPE_VIEW_ACCESSIBILITY_FOCUSED` events came;
- clicks "Tap me" with `ACTION_CLICK` (TalkBack's double tap): the app logs
  the tap, and `TYPE_VIEW_CLICKED` comes;
- sets the field's text with `ACTION_SET_TEXT`: the app logs it, and the
  field's node reads it back;
- tries `ACTION_SCROLL_FORWARD` on a scrollable node, if there is one;
- reads `androiddemo`'s "Volume" slider (a `SeekBar` with a `RangeInfo` of
  0..1 at 0.5), sets it to a quarter with `ACTION_SET_PROGRESS` and steps it
  with a scroll forward: the app logs each (passing on the API 34 emulator;
  required).

`test-on-device.sh` also reads the tree with `uiautomator dump` (another
out-of-process client of the same framework) and taps nodes at their bounds.
It reads only the active window, so for the floating text edit menu (a
window of its own) the script enables the probe APK's second service,
`Menu`, which asks for every interactive window
(`flagRetrieveInteractiveWindows`) and clicks an item it finds in any of
them (`getWindows()`) when told to by a broadcast.

**TalkBack**: the Google APIs emulator images ship TalkBack from API 33 on
(`/product/app/talkback`; the API 30 image doesn't), so on API 34
`.github/a11y/android/test-talkback.sh` turns TalkBack on, with the probe
service beside it; with TalkBack on, the probe only watches, logging the
app's accessibility events. TalkBack only hears input that goes through the
system's input pipeline (its keyboard interceptor and touch explorer are
input filters there), and `adb shell input` injects past those filters: an
`input tap` with touch exploration on is a plain tap, and `input
keycombination` never reaches TalkBack. So the script makes a virtual
keyboard and touch screen with the shell's `uinput` command and moves
TalkBack's focus with its keyboard command (Alt+Right) or, if that doesn't
reach the app, its swipe right, an item at a time, until its focus lands on
"Tap me" (a `TYPE_VIEW_ACCESSIBILITY_FOCUSED` event with that text). On the
way its focus must stop on a plain label ("Running for N s" and the like):
TalkBack reads a focusable node's children that can't act as part of it, so
the host view's node (the window) isn't focusable to accessibility, though
the view takes input focus; when it was, the labels got no stops. Then
TalkBack's click (Alt+Enter, or a double tap) must reach the app. The
notification permission is granted up front to the demo and to TalkBack,
whose dialogs otherwise come up in front of the app and hold TalkBack's
focus. On a failure the script logs the windows, the resumed activity and
`dumpsys accessibility` (the accessibility focused window). TalkBack's speech
isn't read (the emulator may have no voice). The check passes in CI and is
required.

Not covered: TalkBack's speech, explore by touch; the probe's
`ACTION_SCROLL_FORWARD` now reaches plain scroll containers too
(`androiddemo`'s list), not yet seen in CI.

## Web

An invisible overlay of ARIA elements mirrors the tree over the canvas. The
`visual-web` job's headless Chrome checks it from the page. A scroll
container's element carries where it is scrolled; when a screen reader moves
focus to a control in it that is scrolled out of view, the page scrolls the
app's container to show it (a wheel at the container's middle), and a click
(a screen reader's activation) on one does so before clicking it where it
then is. ARIA has no scroll role or action, so nothing else is exposed. A
control that controls another has `aria-controls` naming its element, and a
tree item its `aria-level`. A range's element has `aria-valuenow`,
`aria-valuemin`, `aria-valuemax` and `aria-valuetext` (not settable from a
screen reader: ARIA has no way to), a table's `aria-rowcount` and
`aria-colcount`, a row's and a cell's `aria-rowindex` and `aria-colindex`,
and a captioned control's `aria-labelledby` (its caption's overlay element).
Not yet tried with a screen reader. See [the web page](platforms/web.md#accessibility).

## Linux

X11 and Wayland windows serve their tree over **AT-SPI**, what Orca and the
other Linux screen readers read (`stipple::platform::atspi`, on the
hand-written D-Bus client in `stipple::platform::a11y`; no `libatspi`, `atk`
or `zbus`).

- **Connecting.** On a window's first tree a helper thread finds the
  accessibility bus as `libatspi` does (`atspi/address.rs`), trying each
  in turn until one connects: `$AT_SPI_BUS_ADDRESS`; on X11, the root
  window's `AT_SPI_BUS` property, which `at-spi-bus-launcher` sets on the
  display it serves (read by the X11 backend when it connects, and again
  when the property changes), so an app that can't reach that session bus
  (over `ssh -X`, under `sudo`, in a sandbox) still finds it; then
  `org.a11y.Bus.GetAddress` on the session bus (`$DBUS_SESSION_BUS_ADDRESS`,
  else `$XDG_RUNTIME_DIR/bus`), which starts `at-spi-bus-launcher` when
  needed. With no accessibility bus it gives up quietly and the windows stop
  handing their trees over; startup never waits for it. `NO_AT_BRIDGE=1`
  turns it off, as for GTK. If the
  accessibility bus goes away after it was reached (`at-spi-bus-launcher`
  restarted), the thread keeps the trees, asks for the bus again (which
  starts a new one), reconnects and registers again, waiting 1, 2, 4… up to
  30 seconds between tries.
- **Registering.** It calls `org.a11y.atspi.Socket.Embed` on the registry's
  root, so the app appears among the desktop's applications, and watches
  the registry's name (`NameOwnerChanged` for `org.a11y.atspi.Registry`):
  when `at-spi2-registryd` restarts, the app embeds itself again in the new
  one (once: a registry started by the app's own `Embed` isn't asked twice,
  which would list the app twice). The app's root
  (`/org/a11y/atspi/accessible/root`) has the `Application` role and
  interface (toolkit "Stipple", its version, the id the registry sets) and
  the windows' frames as its children; the app's name is the program's.
- **Serving.** The thread answers calls itself, from the latest tree each
  window handed over (a swap clones the tree into an `Arc`, and is skipped
  when nothing changed), so the UI thread is only involved in actions. A
  node's object path is its place in the tree (`…/w{window}_{i}_{j}`), so it
  keeps its path across rebuilds while the tree's shape stays. Every node has
  `Accessible` (role, name, parent, children, index in parent, state,
  attributes, interfaces, relations) and `Component` (extents in
  screen, window and parent coordinates, `Contains`, `GetAccessibleAtPoint`
  to the deepest node, `GrabFocus` for fields and items). Buttons, menu
  items, checkboxes, radio buttons, switches, selects, tabs, column headers and list and
  tree items have `Action` with one "click", which clicks the middle of the
  node on the UI thread (handed over through the loop's wake, as UIA's Invoke
  on Windows). Labels, fields, password fields and spin buttons have `Text`
  (the text by character, word, sentence and line); fields have
  `EditableText` (setting, inserting or deleting text replaces the field's
  text through the input-method path, after a click to focus it); a slider,
  a progress bar and a spin button have `Value` with their range (setting a
  slider's presses it at the value's place, a spin button's types the
  number; a progress bar's is refused), and a moved one raises
  `object:property-change:accessible-value`. A table has `Table` (its size,
  all its rows; the cell at a row and column; column headers; selected rows)
  and its cells and column headers `TableCell` (position, spans of one, the
  table, the column's header).
- **Scrolling.** A plain scroll container is a scroll pane (`SCROLL_PANE`; a
  list, tree or table keeps its role) with `Action` ("scroll forward",
  "scroll backward": a page) and `Value` (`CurrentValue` its offset in
  logical pixels, 0 to the largest; setting it scrolls there).
  `Component.ScrollTo` and `ScrollToPoint` on a node inside one scroll it
  into view (the least movement that shows it, whatever the type asks).
  A scroll raises `object:property-change:accessible-value`.
- **Toasts.** A toast is a `NOTIFICATION` with the `live` and
  `container-live` attributes `polite`; when one appears, it raises
  `object:announcement` (politeness polite, its message as the data), as
  GTK does for `gtk_accessible_announce`, which Orca speaks.
- **Caret and selection.** A text field's tree node carries its caret and
  selection (byte indices into its text, from the field's editable text,
  as `EditBuffer` has them), so `Text.CaretOffset`, `GetNSelections` and
  `GetSelection` read them in characters. `SetCaretOffset`, `SetSelection`,
  `AddSelection` (when nothing is selected: a field has one selection) and
  `RemoveSelection` move them: a click focuses the field if it hasn't the
  focus, then the field gets `Ime::SetSelection` (`KeyInput::SetSelection`
  for the element; `EditBuffer` selects the range, a plain caret when the
  ends are equal). A field whose name isn't its text (one with an
  `a11y_label`, a password field) carries no caret: its caret reads as the
  end of its text when focused, and the calls that move it are refused.
- **Relations.** `GetRelationSet` gives `labelled-by` on a control that
  names its caption with `Element::labelled_by(id)` and `label-for` on the
  caption (`Element::a11y_id(id)`), the way a `for` attribute ties a label
  to a field on the web: `label(cx, "Note").a11y_id("note")` beside
  `text_field(…).labelled_by("note")`. Ids are matched within the window's
  tree. Nothing is inferred: a label beside a field without them isn't
  related to it (Orca guesses one from the layout, as it does for any
  toolkit). UIA's `LabeledBy`, Android's `setLabeledBy`, the web's
  `aria-labelledby` and macOS's `AXTitleUIElement` follow the same relation.
  The other relations come from the tree too (see
  [Relations](#relations)): `controller-for` / `controlled-by` (a select and
  its open list, a tab and its panel), `member-of` (a radio group, the
  whole group as GTK reports it) and `node-child-of` / `node-parent-of`
  (a tree item's parent), with a tree item's depth as its `level`
  attribute. An action or `GrabFocus` on a node scrolled out of view brings
  it into view first.
- **Events.** Swapping a tree raises what changed, node by node at the same
  place: `object:property-change:accessible-name` (and `…-role`),
  `object:text-changed:delete`/`insert` for text,
  `object:text-caret-moved` (with the new offset) and
  `object:text-selection-changed` for a field's caret and selection,
  `object:state-changed` for
  checked, selected and expanded, `object:children-changed:add`/`remove`, and
  for the focus `object:state-changed:focused` and `focus:`. A window gaining
  or losing the keyboard focus raises `window:activate`/`deactivate` (and its
  frame's `active` state). New and closed windows are children added to and
  removed from the app's root. A swap that adds or removes nodes raises
  `object:children-changed` on their parent for each one (removed from the
  end first), so a client's cached child lists follow.

Screen coordinates on X11 are the window content's position on the root
window (kept up to date from `ConfigureNotify`); Wayland doesn't tell clients
where their windows are, so there they are the window's own.

**CI** (`a11y-dbus`, *Read and drive a running X11 window through AT-SPI*):
in a private session bus (`dbus-run-session`, where `org.a11y.Bus` starts the
accessibility bus and its registry) under Xvfb,
`.github/scripts/atspi-linux.sh` starts `touchdemo` and
`.github/scripts/atspi-walk.py`, a pyatspi client (the library Orca is built
on), from its own process:

- finds `touchdemo` among the registry's applications, with the application
  role and the toolkit name "Stipple", and walks its tree, printing each
  node's role, name, extents on screen and states;
- finds a push button "Row 0", an entry (the text field) and a label
  "Rows tapped: N" under a frame whose parent is the application;
- checks the button's extents are inside the frame's on screen, and that the
  frame's `GetAccessibleAtPoint` at its middle returns it;
- runs the button's "click" action: the app logs the tap, a property-change
  event arrives, and the label reads "Rows tapped: N+1";
- presses Tab (`xdotool`) and gets `object:state-changed:focused` and
  `focus:` for the entry, the only node whose state says focused;
- sets the entry's text through `EditableText`: the app logs it, a
  text-changed event arrives and the entry reads it back;
- reads the entry's caret after the typed text (offset 5), selects "ell"
  (`setSelection(0, 1, 4)`): the entry reports one selection (1, 4) with the
  caret at 4, and `text-selection-changed` and `text-caret-moved` events
  arrive; then `setCaretOffset(2)` moves the caret and clears the selection;
- follows the entry's `labelled-by` relation to its caption "Note" (on
  Linux, `touchdemo` puts a "Note" label beside the field), and the
  caption's `label-for` back to the entry;
- presses Tab again and gets `object:state-changed:focused` and `focus:` for
  the push button "Row 0", whose state says focused; Space and then Enter
  (`xdotool key space`, `Return`) press it, the app logging both taps; and
  Shift+Tab focuses the entry again;
- stops this session's `at-spi2-registryd` (its process found through the
  accessibility bus, never another session's), lets the bus start a new one
  (or starts it), and finds `touchdemo` among the new registry's
  applications again, with its frame and tree.

This is a required check; it passes locally under Xvfb (at-spi2-core 2.58,
pyatspi 2.58), and passed in CI before the caret, selection, relation and
registry-restart checks and the button's keyboard checks were added (those
pass locally; not yet seen in CI).
Without the re-registration the last check fails, as it should. A further
step, *Survive the accessibility bus restarting*
(`.github/scripts/atspi-bus-restart.sh`), kills `at-spi-bus-launcher`
itself, asks for the bus again, and a fresh pyatspi client must find the
app on the new bus; it passes locally and in CI, and is required.
Another, *Scroll panes, a check box and a toast's announcement through
AT-SPI (X11)*, runs the walk with `ATSPI_WALK_DEPTH=1` on a fresh
`touchdemo`: the list of rows is a scroll pane whose "scroll forward" action
scrolls it (the app logs its offset, the pane's `Value` follows, with an
`accessible-value` event), setting its `Value` to 0 scrolls back, and
`ScrollTo` on "Row 20" brings it into view; the strip of chips is a scroll
pane of its own that scrolls; the "Remember" check box's action checks it
(the app logs it; `checked` state and event); and "Row 2"'s action, with
"Row 2" scrolled out of view above the list by then, presses it (the app
logs that row's tap, and it is in view again after) and raises a toast that
arrives as `object:announcement` "Row 2 tapped", with a polite
notification in the tree. It passes locally under Xvfb (at-spi2-core 2.58)
and doesn't fail the job yet (not run under sway). The Orca script then
also presses "Row 2" and reports whether Orca speaks the toast. The same
walk under headless sway (`wtype` for the keys, with a keyboard held in the
seat for the whole walk by a `wtype` that only sleeps: each `wtype` run
makes a virtual keyboard and destroys it on exit, and in a seat with no
other keyboard that takes the keyboard capability away and back every time,
so the app binds a new `wl_keyboard` per run and `wtype`'s press, sent as
soon as the compositor has its keymap, could reach the seat before it did,
leaving only the release; every key now arrives on the first send) and **Orca** itself
(`.github/scripts/orca-linux.sh`: Orca runs headless with its debug log,
which records every utterance as `SPEECH OUTPUT`, and must speak the entry
after Tab; then, added later and not yet run in CI, "Row 0" after another
Tab, and Space must press it) run in the same job, pass in CI and are required too. The sway
walk runs the new checks as well; they haven't been run under a Wayland
compositor outside CI.

*Relations through AT-SPI* (`.github/scripts/atspi-relations.sh` and
`atspi-relations.py`) opens `widgetsdemo`'s "Size" select by its action and
follows its `controller-for` to its list (and `controlled-by` back), checks
its "Format" chips' `member-of` names the whole group, and in
`mediabrowser`'s folder tree that "Music" is `node-child-of` the root folder
a level up (by their `level` attributes), which is `node-parent-of` it.
*Find the accessibility bus through the X root window*
(`.github/scripts/atspi-x11-bus.sh`) puts the bus's address on the root
window (when the launcher didn't), runs `touchdemo` with no session bus to
ask (no `DBUS_SESSION_BUS_ADDRESS`, an empty `XDG_RUNTIME_DIR`), and a
pyatspi client must find it registered, the bridge saying it took the
address from the property. Both pass locally under Xvfb (at-spi2-core
2.58) and in CI, and are required.

Another, *Range values and a table through AT-SPI (X11)*, runs the walk with
`ATSPI_WALK_RANGES=1` on `touchdemo` with `TOUCHDEMO_RANGES`: the slider's
`Value` (0.5 of 0..1, step 0.05) set to 0.25 moves the app's slider (with an
`accessible-value` event) and the progress bar follows, the spin button's
`Value` (3 of 0..10) set to 7 sets the app's number and its labelled-by
caption is "Level", and the table's `Table` (21 rows, 2 columns; the cell at
row 1, column 0; column 1's header; no cell in a row not built) and a cell's
`TableCell` (its position, its table). It passes locally under Xvfb
(at-spi2-core 2.58) and in CI, and is required.

`a11ydemo serve` still serves a fixed tree over the session bus for the
`dbus-send` checks.

Not covered: no text attributes, character extents are the node's box split
evenly (not the glyphs'), a spin button's `Value` has no range, and a
scroll pane's `Value` and its scroll bar children's say the same; nodes
scrolled out of view still say `showing`. The `AT_SPI_BUS` property is read
once at startup and when it changes, not looked up again on each reconnect. The table's selection isn't changed through `Table` (rows are
selected by their own click action).
