#!/usr/bin/env python3
"""Read and drive a running Stipple app through AT-SPI from another process,
as Orca does: find the app among the registry's applications, walk its tree
(roles, names, extents on screen), hit-test a point, run a button's action
and see the app react (its tree and its log), then press Tab and see the
focus event arrive for a control; type into the text field, read and move
its caret and selection, follow its labelled-by relation to its caption; Tab
on to a button, press it with Space and Enter, and go back with Shift+Tab;
and finally restart the registry (at-spi2-registryd) and find the app again,
re-registered. The Linux counterpart of uia-walk.ps1.

usage: atspi-walk.py APP_NAME APP_LOG TAB_COMMAND...
       atspi-walk.py --find-only APP_NAME
         (only find the app and its tree, as after the bus restarted)

With ATSPI_WALK_DEPTH=1 in the environment it runs other checks instead,
after finding the app: the list of rows is a scroll pane whose actions,
value and Component.ScrollTo scroll it (the app logs its offset), the
sideways strip of chips scrolls too, the "Remember" check box is checked by
its action (state and event), and a tap on "Row 2" raises a toast, which
arrives as an object:announcement and a notification in the tree.

With ATSPI_WALK_RANGES=1 (touchdemo with TOUCHDEMO_RANGES) it checks range
values and a table instead: the "Volume" slider's Value (its range and step;
setting it moves the app's slider, with an accessible-value event), the
progress bar's, the "Level" spin button's (set to 7; its labelled-by caption
"Level"), and the "Items" table's Table and TableCell interfaces (its size,
a cell by row and column, a column's header, a cell's position), and the
text attribute runs of the rich line "Rich bold link bonjour" (Text's
GetAttributeRun, GetDefaultAttributes and GetAttributeValue).
"""
import os
import signal
import subprocess
import sys
import time

import pyatspi
from gi.repository import GLib

find_only = sys.argv[1] == "--find-only"
if find_only:
    app_name, app_log, tab_cmd = sys.argv[2], None, []
else:
    app_name, app_log, tab_cmd = sys.argv[1], sys.argv[2], sys.argv[3:]
failures = []


def check(ok, what):
    print(("ok: " if ok else "FAILED: ") + what, flush=True)
    if not ok:
        failures.append(what)


def pump(seconds):
    """Let the event listeners run."""
    ctx = GLib.MainContext.default()
    end = time.time() + seconds
    while time.time() < end:
        while ctx.iteration(False):
            pass
        time.sleep(0.02)


def wait_for(cond, seconds=10):
    for _ in range(int(seconds / 0.25)):
        pump(0.25)
        try:
            if cond():
                return True
        except Exception as e:  # an object gone mid-swap: look again
            print("atspi: retrying after {}".format(e), flush=True)
    return False


def extents(acc):
    try:
        e = acc.queryComponent().getExtents(pyatspi.DESKTOP_COORDS)
        return (e.x, e.y, e.width, e.height)
    except NotImplementedError:
        return None


def describe(acc):
    return '{} "{}" extents={} states=[{}]'.format(
        acc.getRoleName(), acc.name, extents(acc),
        ",".join(pyatspi.stateToString(s) for s in acc.getState().getStates()))


def walk(acc, depth=0, out=None):
    out = [] if out is None else out
    out.append(acc)
    print("atspi: " + "  " * depth + describe(acc), flush=True)
    for i in range(acc.childCount):
        child = acc.getChildAtIndex(i)
        if child is not None:
            walk(child, depth + 1, out)
    return out


def find_app():
    desktop = pyatspi.Registry.getDesktop(0)
    for _ in range(60):
        names = []
        for i in range(desktop.childCount):
            app = desktop.getChildAtIndex(i)
            if app is None:
                continue
            names.append(app.name)
            if app.name == app_name and app.childCount > 0:
                return app
        print("atspi: waiting for {} among {}".format(app_name, names), flush=True)
        time.sleep(0.5)
    return None


def named(elements, role, prefix):
    for e in elements:
        if e.getRole() == role and e.name.startswith(prefix):
            return e
    return None


events = []


def on_event(event):
    try:
        app = event.source.getApplication()
        if app is None or app.name != app_name:
            return
        events.append((event.type, event.detail1, event.source.getRole(), event.source.name))
        print("atspi event: {} {} on {} \"{}\"".format(
            event.type, event.detail1, event.source.getRoleName(), event.source.name), flush=True)
    except Exception as e:  # a source gone by the time we look
        print("atspi event: {} (unreadable: {})".format(event.type, e), flush=True)


pyatspi.Registry.registerEventListener(
    on_event, "focus:", "object:state-changed:focused", "window:activate",
    "object:property-change:accessible-name", "object:children-changed",
    "object:text-changed", "object:text-caret-moved", "object:text-selection-changed")

depth = os.environ.get("ATSPI_WALK_DEPTH") == "1"
ranges = os.environ.get("ATSPI_WALK_RANGES") == "1"
announcements = []


def on_announcement(event):
    try:
        text = event.any_data
    except Exception as e:
        text = "unreadable: {}".format(e)
    announcements.append((event.detail1, text))
    print("atspi event: {} {} {!r}".format(event.type, event.detail1, text), flush=True)


if depth or ranges:
    pyatspi.Registry.registerEventListener(
        on_event, "object:property-change:accessible-value", "object:state-changed:checked")
    pyatspi.Registry.registerEventListener(on_announcement, "object:announcement")

app = find_app()
check(app is not None, "the registry lists the application '{}'".format(app_name))
if app is None:
    sys.exit(1)
check(app.getRole() == pyatspi.ROLE_APPLICATION, "its root is an application")
print("atspi: application interfaces {}".format(pyatspi.listInterfaces(app)), flush=True)
try:
    toolkit = app.toolkitName
except Exception as e:
    toolkit = "unreadable: {}".format(e)
check(toolkit == "Stipple", "the application's toolkit is Stipple (got {})".format(toolkit))

elements = walk(app)
print("atspi: {} elements".format(len(elements)), flush=True)
frame = app.getChildAtIndex(0)
check(frame.getRole() == pyatspi.ROLE_FRAME, "the app's first child is a frame")
check(frame.parent is not None and frame.parent.name == app.name, "the frame's parent is the app")
row0 = named(elements, pyatspi.ROLE_PUSH_BUTTON, "Row 0")
field = named(elements, pyatspi.ROLE_ENTRY, "")
count = named(elements, pyatspi.ROLE_LABEL, "Rows tapped: ")
check(row0 is not None, "a push button named 'Row 0'")
check(field is not None, "an entry (the text field)")
check(count is not None, "a label 'Rows tapped: N'")
if find_only:
    if failures:
        print("AT-SPI checks failed: " + "; ".join(failures), flush=True)
        sys.exit(1)
    print("atspi: found the app and its tree", flush=True)
    sys.exit(0)



def log_lines(prefix):
    with open(app_log) as f:
        return [line.strip() for line in f if line.startswith(prefix)]


def await_log(prefix, since, seconds=10):
    """The app's log lines starting with `prefix` after the first `since`."""
    for _ in range(int(seconds / 0.25)):
        pump(0.25)
        lines = log_lines(prefix)
        if len(lines) > since:
            return lines[since:]
    return []


def offsets(lines, prefix):
    return [float(line[len(prefix):]) for line in lines]


def scroll_pane_of(acc):
    while acc is not None and acc.getRole() != pyatspi.ROLE_SCROLL_PANE:
        acc = acc.parent
    return acc


if ranges:
    said = "touchdemo: volume "
    slider = named(elements, pyatspi.ROLE_SLIDER, "Volume")
    check(slider is not None, "a slider 'Volume'")
    if slider is not None:
        v = slider.queryValue()
        check((v.currentValue, v.minimumValue, v.maximumValue) == (0.5, 0.0, 1.0),
              "its value is 0.5 of 0..1 (got {} of {}..{})".format(
                  v.currentValue, v.minimumValue, v.maximumValue))
        check(abs(v.minimumIncrement - 0.05) < 1e-9,
              "its step is 0.05 (got {})".format(v.minimumIncrement))
        del events[:]
        n = len(log_lines(said))
        v.currentValue = 0.25
        got = offsets(await_log(said, n), said)
        check(bool(got) and got[-1] == 0.25, "setting it to 0.25 moved the app's slider ({})".format(got))
        check(wait_for(lambda: v.currentValue == 0.25), "its value followed ({})".format(v.currentValue))
        check(any(t.startswith("object:property-change:accessible-value") for t, *_ in events),
              "an accessible-value change event said so")
    bar = named(walk(frame, 0, []), pyatspi.ROLE_PROGRESS_BAR, "Progress")
    check(bar is not None, "a progress bar 'Progress'")
    if bar is not None:
        check(wait_for(lambda: bar.queryValue().currentValue == 0.25),
              "it follows the volume ({})".format(bar.queryValue().currentValue))
    spin = named(walk(frame, 0, []), pyatspi.ROLE_SPIN_BUTTON, "")
    check(spin is not None, "a spin button (the 'Level' input)")
    if spin is not None:
        v = spin.queryValue()
        check((v.currentValue, v.minimumValue, v.maximumValue) == (3.0, 0.0, 10.0),
              "its value is 3 of 0..10 (got {} of {}..{})".format(
                  v.currentValue, v.minimumValue, v.maximumValue))
        labels = [t.name for r in spin.getRelationSet()
                  if r.getRelationType() == pyatspi.RELATION_LABELLED_BY
                  for t in (r.getTarget(i) for i in range(r.getNTargets()))]
        check(labels == ["Level"], "it is labelled by 'Level' (got {})".format(labels))
        n = len(log_lines("touchdemo: level "))
        v.currentValue = 7.0
        got = await_log("touchdemo: level ", n)
        check(bool(got) and got[-1] == "touchdemo: level 7", "setting it to 7 set the app's ({})".format(got))
    table = named(walk(frame, 0, []), pyatspi.ROLE_TABLE, "Items")
    check(table is not None, "a table 'Items'")
    if table is not None:
        print("atspi: table interfaces {}".format(pyatspi.listInterfaces(table)), flush=True)
        t = table.queryTable()
        check((t.nRows, t.nColumns) == (21, 2),
              "it has 21 rows (its header too) and 2 columns (got {}x{})".format(t.nRows, t.nColumns))
        first = t.getAccessibleAt(1, 0)
        check(first is not None and first.name == "Item 0",
              "the cell at row 1, column 0 is 'Item 0' (got {})".format(first and first.name))
        header = t.getColumnHeader(1)
        check(header is not None and header.name.startswith("Count"),
              "column 1's header is 'Count' (got {})".format(header and header.name))
        check(t.getAccessibleAt(15, 0) is None, "a row not built has no cell")
        cell = named(walk(table, 0, []), pyatspi.ROLE_TABLE_CELL, "Item 1")
        check(cell is not None, "a table cell 'Item 1'")
        if cell is not None:
            tc = cell.queryTableCell()
            # (pyatspi gives (ok, row, column).)
            pos = tuple(tc.position)[-2:]
            check(pos == (2, 0), "it is at row 2, column 0 (got {})".format(pos))
            check(tc.table is not None and tc.table.name == "Items", "its table is 'Items'")
    # Text drawn in runs: "Rich bold link bonjour", whose "bold" is bold,
    # "link" underlined and blue, "bonjour" italic and in French. Reported,
    # not required yet: CI's non-blocking step greps for the summary line.
    soft_failures = []

    def soft(ok, what):
        print(("ok: " if ok else "FAILED (not required yet): ") + what, flush=True)
        if not ok:
            soft_failures.append(what)

    rich = named(walk(frame, 0, []), pyatspi.ROLE_LABEL, "Rich bold")
    soft(rich is not None, "a label 'Rich bold link bonjour'")
    if rich is not None:
        rt = rich.queryText()

        def attrs(got):
            # pyatspi gives "key:value" strings (a list, or joined by ";").
            items = got if isinstance(got, (list, tuple)) else str(got).split(";")
            if isinstance(got, dict):
                return dict(got)
            return dict(i.strip().split(":", 1) for i in items if ":" in i)

        def run(offset, defaults=False):
            a, s, e = rt.getAttributeRun(offset, defaults)
            return attrs(a), s, e

        a, s, e = run(6)
        print("atspi: text run at 6: {} {}..{}".format(a, s, e), flush=True)
        soft((s, e) == (5, 9) and a.get("weight") == "700",
              "GetAttributeRun at 'bold' is 5..9 at weight 700 (got {} {}..{})".format(a, s, e))
        a, s, e = run(11)
        print("atspi: text run at 11: {} {}..{}".format(a, s, e), flush=True)
        soft((s, e) == (10, 14) and a.get("underline") == "single"
              and a.get("fg-color") == "{},{},{}".format(0x1a * 257, 0x5f * 257, 0xd0 * 257),
              "GetAttributeRun at 'link' is 10..14, underlined, blue (got {} {}..{})".format(a, s, e))
        a, s, e = run(16, True)
        print("atspi: text run at 16 (with defaults): {} {}..{}".format(a, s, e), flush=True)
        soft((s, e) == (15, 22) and a.get("style") == "italic" and a.get("language") == "fr"
              and a.get("weight") == "400" and "size" in a,
              "GetAttributeRun at 'bonjour' (with defaults) is 15..22, italic, French, weight 400 "
              "(got {} {}..{})".format(a, s, e))
        a, s, e = run(1)
        soft((s, e) == (0, 5) and "weight" not in a,
              "the plain 'Rich ' is 0..5 with nothing differing (got {} {}..{})".format(a, s, e))
        d = attrs(rt.getDefaultAttributes())
        print("atspi: default text attributes: {}".format(d), flush=True)
        soft(d.get("weight") == "400" and d.get("style") == "normal",
              "the default attributes are weight 400, normal (got {})".format(d))
        v = rt.getAttributeValue(6, "weight")
        # (pyatspi may hand back the value, or a (value, ...) tuple.)
        v = v[0] if isinstance(v, (list, tuple)) else v
        soft(v == "700", "GetAttributeValue(6, weight) is 700 (got {!r})".format(v))
    if soft_failures:
        print("atspi: text runs FAILED: " + "; ".join(soft_failures), flush=True)
    else:
        print("atspi: text runs passed", flush=True)
    if failures:
        print("AT-SPI range checks failed: " + "; ".join(failures), flush=True)
        sys.exit(1)
    print("atspi: all range checks passed", flush=True)
    sys.exit(0)

if depth:
    # The list of rows: a plain scroll container, a scroll pane.
    pane = scroll_pane_of(row0)
    check(pane is not None, "'Row 0' is inside a scroll pane")
    said = "touchdemo: scrolled to "
    if pane is not None:
        print("atspi: scroll pane interfaces {}".format(pyatspi.listInterfaces(pane)), flush=True)
        value = pane.queryValue()
        check(value.maximumValue > 0 and value.currentValue == 0,
              "its value is its offset: {} of 0..{}".format(value.currentValue, value.maximumValue))
        action = pane.queryAction()
        names = [action.getName(i) for i in range(action.nActions)]
        check(names == ["scroll forward", "scroll backward"],
              "it has scroll forward and backward actions (got {})".format(names))
        del events[:]
        n = len(log_lines(said))
        check(action.doAction(0), "'scroll forward' is accepted")
        moved = offsets(await_log(said, n), said)
        check(bool(moved) and moved[-1] > 0, "the app scrolled the list ({})".format(moved))
        check(wait_for(lambda: value.currentValue > 0),
              "the pane's value followed ({})".format(value.currentValue))
        check(any(t.startswith("object:property-change:accessible-value") for t, *_ in events),
              "an accessible-value change event said so")
        # Setting the value scrolls there.
        n = len(log_lines(said))
        value.currentValue = 0.0
        moved = offsets(await_log(said, n), said)
        check(bool(moved) and moved[-1] == 0, "setting the value to 0 scrolled back ({})".format(moved))
        # Its scroll bar: the pane's last child, vertical, its value the
        # offset; setting it scrolls the list.
        bars = [pane.getChildAtIndex(i) for i in range(pane.childCount)]
        bars = [b for b in bars if b is not None and b.getRole() == pyatspi.ROLE_SCROLL_BAR]
        check(len(bars) == 1, "the pane has a scroll bar ({})".format(len(bars)))
        if bars:
            bar = bars[0]
            check(bar.getState().contains(pyatspi.STATE_VERTICAL), "...vertical")
            bv = bar.queryValue()
            check(bv.maximumValue == pane.queryValue().maximumValue,
                  "...over the pane's range ({} of 0..{})".format(bv.currentValue, bv.maximumValue))
            # (The list is at 0 here: the bar takes it to the end. The
            # ScrollTo below then scrolls up to "Row 20", which leaves "Row 2"
            # out of view above the list for the toast's check.)
            n = len(log_lines(said))
            end = bv.maximumValue
            bv.currentValue = end
            moved = offsets(await_log(said, n), said)
            check(bool(moved) and moved[-1] == end,
                  "setting the scroll bar's value to its maximum scrolled the list to the end ({})".format(moved))
        # A row below the viewport, scrolled into view.
        row20 = named(walk(frame, 0, []), pyatspi.ROLE_PUSH_BUTTON, "Row 20")
        check(row20 is not None, "a push button 'Row 20'")
        if row20 is not None:
            n = len(log_lines(said))
            check(row20.queryComponent().scrollTo(pyatspi.SCROLL_ANYWHERE),
                  "Component.ScrollTo on 'Row 20' is accepted")
            moved = offsets(await_log(said, n), said)
            px, py, pw, ph = extents(pane)

            def shown():
                x, y, w, h = extents(row20)
                return y >= py and y + h <= py + ph
            check(bool(moved) and wait_for(shown),
                  "...and brought it into view ({}; row {}, pane {})".format(
                      moved, extents(row20), (px, py, pw, ph)))
    # The sideways strip of chips.
    # (Its chips are labels in panels, or buttons where they take a tap.)
    chip = (named(elements, pyatspi.ROLE_LABEL, "Chip 0")
            or named(elements, pyatspi.ROLE_PUSH_BUTTON, "Chip 0"))
    strip = scroll_pane_of(chip)
    check(strip is not None and strip != pane, "the chips are in a scroll pane of their own")
    if strip is not None:
        said = "touchdemo: strip scrolled to "
        n = len(log_lines(said))
        check(strip.queryAction().doAction(0), "'scroll forward' on the strip is accepted")
        moved = offsets(await_log(said, n), said)
        check(bool(moved) and moved[-1] > 0, "the app scrolled the strip ({})".format(moved))

    # The "Remember" check box, checked by its action.
    box = named(elements, pyatspi.ROLE_CHECK_BOX, "Remember")
    check(box is not None, "a check box 'Remember'")
    if box is not None:
        check(not box.getState().contains(pyatspi.STATE_CHECKED), "...not checked")
        del events[:]
        n = len(log_lines("touchdemo: remember on"))
        check(box.queryAction().doAction(0), "its click action is accepted")
        check(bool(await_log("touchdemo: remember on", n)), "the app logged 'remember on'")
        check(wait_for(lambda: box.getState().contains(pyatspi.STATE_CHECKED)), "it is checked now")
        check(any(t.startswith("object:state-changed:checked") and d == 1 for t, d, *_ in events),
              "a state-changed:checked event said so")

    # A toast: a tap on "Row 2" raises one, which is announced. "Row 2" is
    # scrolled out of view above the list now (by the ScrollTo on "Row
    # 20"): its click brings it back into view first, then lands on it.
    row2 = named(walk(frame, 0, []), pyatspi.ROLE_PUSH_BUTTON, "Row 2")
    check(row2 is not None, "a push button 'Row 2'")
    if row2 is not None:
        del announcements[:]
        n = len(log_lines("touchdemo: toast"))
        tapped = len(log_lines("touchdemo: row 2 tapped"))
        py = extents(pane)[1] if pane is not None else None
        check(py is None or extents(row2)[1] < py, "'Row 2' is out of view above the list")
        check(row2.queryAction().doAction(0), "'click' on 'Row 2' is accepted")
        check(bool(await_log("touchdemo: row 2 tapped", tapped)), "the click pressed 'Row 2'")
        if py is not None:
            # (The toast changes the tree's shape: found again.)
            def back_in_view():
                again = named(walk(frame, 0, []), pyatspi.ROLE_PUSH_BUTTON, "Row 2")
                return again is not None and extents(again)[1] >= py
            check(wait_for(back_in_view), "...and 'Row 2' is in view again")
        check(bool(await_log("touchdemo: toast", n)), "the app raised its toast")
        check(wait_for(lambda: any(t == "Row 2 tapped" for _, t in announcements)),
              "an object:announcement said 'Row 2 tapped' ({})".format(announcements))
        toast = named(walk(frame, 0, []), pyatspi.ROLE_NOTIFICATION, "Row 2 tapped")
        check(toast is not None, "...and a notification 'Row 2 tapped' is in the tree")
        if toast is not None:
            attrs = toast.getAttributes()
            check("container-live:polite" in attrs, "...a polite live region ({})".format(attrs))

    if failures:
        print("AT-SPI depth checks failed: " + "; ".join(failures), flush=True)
        sys.exit(1)
    print("atspi: all depth checks passed", flush=True)
    sys.exit(0)

if row0 is not None:
    fx, fy, fw, fh = extents(frame)
    rx, ry, rw, rh = extents(row0)
    check(rw > 0 and rh > 0, "'Row 0' has a size on screen")
    check(fx <= rx and fy <= ry and rx + rw <= fx + fw and ry + rh <= fy + fh,
          "'Row 0' ({}) lies inside its frame ({}) on screen".format(
              (rx, ry, rw, rh), (fx, fy, fw, fh)))
    hit = frame.queryComponent().getAccessibleAtPoint(rx + rw // 2, ry + rh // 2,
                                                      pyatspi.DESKTOP_COORDS)
    check(hit is not None and hit.name == "Row 0",
          "the element at 'Row 0''s middle is 'Row 0' (got '{}')".format(hit and hit.name))
    action = row0.queryAction()
    check(action.nActions >= 1 and action.getName(0) == "click",
          "'Row 0' has a 'click' action")
    before = int(count.name.split(": ")[1])
    want = "Rows tapped: {}".format(before + 1)
    check(action.doAction(0), "doAction(0) on 'Row 0' is accepted")
    after = None
    for _ in range(40):
        pump(0.25)
        # The label keeps its object (its path is its place in the tree);
        # the name it reads now is the app's new one.
        if count.name == want:
            after = count
            break
    check(after is not None, "the action reached the app: the label now reads '{}' (got '{}')".format(
        want, count.name))
    check(any(t.startswith("object:property-change:accessible-name") for t, *_ in events),
          "a property-change event announced the new name")
    with open(app_log) as f:
        check(any(line.startswith("touchdemo: row 0 tapped") for line in f),
              "the app logged the tap")

if field is not None:
    text = field.queryText()
    print("atspi: field text '{}' ({} characters, caret {})".format(
        text.getText(0, -1), text.characterCount, text.caretOffset), flush=True)

# Keyboard focus, as a screen reader follows it: Tab, then the focus event.
# Every key is sent once and must land: under Wayland, atspi-linux.sh holds a
# keyboard in the seat for the whole walk, so wtype's runs don't take the
# app's wl_keyboard away and back (which lost their presses).
def await_focus(cmd):
    """Send `cmd` and wait for a state-changed:focused event."""
    del events[:]
    subprocess.run(cmd, check=False)
    for _ in range(40):
        pump(0.25)
        hits = [e for e in events if e[0].startswith("object:state-changed:focused") and e[1] == 1]
        if hits:
            return hits[-1]
    return None


focused = await_focus(tab_cmd)
check(focused is not None and focused[2] not in (pyatspi.ROLE_FRAME, pyatspi.ROLE_APPLICATION),
      "after Tab, a control in the window reported state-changed:focused ({})".format(focused))
check(any(e[0] == "focus:" for e in events), "...and a focus: event")
if focused is not None:
    # The focused control says so in its state.
    now = [e for e in walk(frame, 0, []) if e.getState().contains(pyatspi.STATE_FOCUSED)]
    check(len(now) == 1 and now[0].name == focused[3],
          "the tree has one focused element, the one the event named ({})".format(
              [e.name for e in now]))

if field is not None:
    # A screen reader's edit (EditableText) types into the field.
    check(field.queryEditableText().setTextContents("hello"), "setTextContents on the entry is accepted")
    typed = False
    for _ in range(40):
        pump(0.25)
        if field.name == "hello" and field.queryText().getText(0, -1) == "hello":
            typed = True
            break
    check(typed, "the entry now reads 'hello' (got '{}')".format(field.name))
    with open(app_log) as f:
        check(any(line.strip() == 'touchdemo: text "hello"' for line in f), "the app logged the new text")
    check(any(t.startswith("object:text-changed:insert") for t, *_ in events),
          "an object:text-changed:insert event announced the typing")




if field is not None:
    # The caret, as the app's field has it: after the text just typed.
    text = field.queryText()
    check(wait_for(lambda: text.caretOffset == 5),
          "the entry's caret is after 'hello' (offset 5, got {})".format(text.caretOffset))
    check(text.getNSelections() == 0, "nothing is selected yet")
    # A screen reader selects "ell": the field selects it, and says so.
    del events[:]
    check(text.setSelection(0, 1, 4), "setSelection(0, 1, 4) on the entry is accepted")
    check(wait_for(lambda: text.getNSelections() == 1 and tuple(text.getSelection(0)) == (1, 4)),
          "the entry now has 'ell' selected (got {} selections, {})".format(
              text.getNSelections(), tuple(text.getSelection(0))))
    check(text.caretOffset == 4, "the caret is at the selection's end (got {})".format(text.caretOffset))
    check(wait_for(lambda: any(t.startswith("object:text-selection-changed") for t, *_ in events)),
          "an object:text-selection-changed event announced it")
    check(any(t.startswith("object:text-caret-moved") for t, *_ in events),
          "...and an object:text-caret-moved event")
    # Moving the caret clears the selection.
    check(text.setCaretOffset(2), "setCaretOffset(2) is accepted")
    check(wait_for(lambda: text.caretOffset == 2 and text.getNSelections() == 0),
          "the caret moved to 2 and the selection went (caret {}, {} selections)".format(
              text.caretOffset, text.getNSelections()))
    check(field.name == "hello", "the text is still 'hello' (got '{}')".format(field.name))

    # The entry's caption, through its relations.
    labels = []
    for rel in field.getRelationSet():
        if rel.getRelationType() == pyatspi.RELATION_LABELLED_BY:
            labels += [rel.getTarget(i) for i in range(rel.getNTargets())]
    names = [l.name for l in labels if l is not None]
    check(names == ["Note"], "the entry is labelled by 'Note' (got {})".format(names))
    if labels and labels[0] is not None:
        targets = []
        for rel in labels[0].getRelationSet():
            if rel.getRelationType() == pyatspi.RELATION_LABEL_FOR:
                targets += [rel.getTarget(i) for i in range(rel.getNTargets())]
        check(any(t is not None and t.getRole() == pyatspi.ROLE_ENTRY for t in targets),
              "...and 'Note' is the label for the entry")


# Buttons take keyboard focus too: Tab on from the field to 'Row 0' (its
# state-changed:focused and focus:), Space and Enter press it, Shift+Tab goes
# back.
def keys(name):
    """TAB_COMMAND with another key (space, Return) instead of Tab, or Shift+Tab."""
    if name == "shift+Tab":
        if tab_cmd[0] == "wtype":
            return ["wtype", "-M", "shift", "-k", "Tab", "-m", "shift"]
        return [a.replace("Tab", "shift+Tab") for a in tab_cmd]
    return [a.replace("Tab", name) for a in tab_cmd]


def taps():
    with open(app_log) as f:
        return sum(1 for line in f if line.startswith("touchdemo: row 0 tapped"))


if row0 is not None and field is not None:
    focused = await_focus(tab_cmd)
    check(focused is not None and focused[2] == pyatspi.ROLE_PUSH_BUTTON and focused[3] == "Row 0",
          "Tab from the field focused the push button 'Row 0' ({})".format(focused))
    check(any(e[0] == "focus:" for e in events), "...with a focus: event")
    check(row0.getState().contains(pyatspi.STATE_FOCUSED), "'Row 0' says it is focused")
    for key, what in (("space", "Space"), ("Return", "Enter")):
        before = taps()
        subprocess.run(keys(key), check=False)
        pressed = wait_for(lambda: taps() > before)
        check(pressed, "{} on the focused 'Row 0' pressed it: the app logged the tap".format(what))
    back = await_focus(keys("shift+Tab"))
    check(back is not None and back[2] == pyatspi.ROLE_ENTRY,
          "Shift+Tab went back to the entry ({})".format(back))


def a11y_bus():
    out = subprocess.run(["dbus-send", "--session", "--print-reply=literal", "--dest=org.a11y.Bus",
                          "/org/a11y/bus", "org.a11y.Bus.GetAddress"],
                         capture_output=True, text=True)
    return out.stdout.strip()


def registry_pid(bus):
    """The registry's process on this session's accessibility bus (never
    another session's), or None while it has none."""
    out = subprocess.run(["dbus-send", "--bus=" + bus, "--print-reply=literal",
                          "--dest=org.freedesktop.DBus", "/org/freedesktop/DBus",
                          "org.freedesktop.DBus.GetConnectionUnixProcessID",
                          "string:org.a11y.atspi.Registry"],
                         capture_output=True, text=True)
    words = out.stdout.split()
    return int(words[-1]) if out.returncode == 0 and words else None


def alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except OSError:
        return False


# The registry restarts (it crashed, or was updated): the app registers
# again and a screen reader finds it as before.
bus = a11y_bus()
old = registry_pid(bus) if bus else None
check(old is not None, "the registry runs on the accessibility bus {} (pid {})".format(bus, old))
if old is not None:
    os.kill(old, signal.SIGTERM)
    for _ in range(40):
        if not alive(old):
            break
        time.sleep(0.25)
    check(not alive(old), "the registry stopped")
    time.sleep(1)
    # The accessibility bus starts it again on the next call to it (here,
    # pyatspi's); start it by hand if that hasn't happened.
    pyatspi.Registry.getDesktop(0).childCount
    time.sleep(2)
    if registry_pid(bus) is None:
        for path in ("/usr/libexec/at-spi2-registryd", "/usr/lib/at-spi2-core/at-spi2-registryd"):
            if os.path.exists(path):
                print("atspi: starting {}".format(path), flush=True)
                subprocess.Popen([path])
                break
        time.sleep(2)
    new = registry_pid(bus)
    check(new is not None and new != old, "a new registry runs (pid {})".format(new))
again = find_app()
check(again is not None, "after the registry restarted, it lists '{}' again".format(app_name))
if again is not None:
    frame = again.getChildAtIndex(0)
    check(frame is not None and frame.getRole() == pyatspi.ROLE_FRAME,
          "...with its frame")
    found = walk(again, 0, [])
    check(named(found, pyatspi.ROLE_PUSH_BUTTON, "Row 0") is not None,
          "...and its tree ('Row 0')")

if failures:
    print("AT-SPI checks failed: " + "; ".join(failures), flush=True)
    sys.exit(1)
print("atspi: all checks passed", flush=True)
