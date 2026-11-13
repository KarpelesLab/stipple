#!/usr/bin/env python3
"""Read and drive a running Stipple app through AT-SPI from another process,
as Orca does: find the app among the registry's applications, walk its tree
(roles, names, extents on screen), hit-test a point, run a button's action
and see the app react (its tree and its log), then press Tab and see the
focus event arrive for a control. The Linux counterpart of uia-walk.ps1.

usage: atspi-walk.py APP_NAME APP_LOG TAB_COMMAND...
"""
import subprocess
import sys
import time

import pyatspi
from gi.repository import GLib

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
    "object:text-changed")

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
del events[:]
subprocess.run(tab_cmd, check=False)
focused = None
for _ in range(40):
    pump(0.25)
    hits = [e for e in events if e[0].startswith("object:state-changed:focused") and e[1] == 1]
    if hits:
        focused = hits[-1]
        break
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

if failures:
    print("AT-SPI checks failed: " + "; ".join(failures), flush=True)
    sys.exit(1)
print("atspi: all checks passed", flush=True)
