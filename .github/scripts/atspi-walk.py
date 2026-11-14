#!/usr/bin/env python3
"""Read and drive a running Stipple app through AT-SPI from another process,
as Orca does: find the app among the registry's applications, walk its tree
(roles, names, extents on screen), hit-test a point, run a button's action
and see the app react (its tree and its log), then press Tab and see the
focus event arrive for a control; type into the text field, read and move
its caret and selection, follow its labelled-by relation to its caption; and
finally restart the registry (at-spi2-registryd) and find the app again,
re-registered. The Linux counterpart of uia-walk.ps1.

usage: atspi-walk.py APP_NAME APP_LOG TAB_COMMAND...
       atspi-walk.py --find-only APP_NAME
         (only find the app and its tree, as after the bus restarted)
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



def wait_for(cond, seconds=10):
    for _ in range(int(seconds / 0.25)):
        pump(0.25)
        try:
            if cond():
                return True
        except Exception as e:  # an object gone mid-swap: look again
            print("atspi: retrying after {}".format(e), flush=True)
    return False


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
