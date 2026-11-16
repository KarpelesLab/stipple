#!/usr/bin/env python3
"""Find a running Stipple app through AT-SPI, as a screen reader does, walk
its whole tree, and check that it holds an element of each role and name
asked for (the name a prefix; `*` for any name).

usage: atspi-roles.py APP_NAME "ROLE NAME|NAME PREFIX"...
  e.g. atspi-roles.py mediabrowser "tree|Folders" "table|Files" "slider|*"

Prints the tree (role, name, states) and `atspi-roles: ok` or the missing
ones; exits 1 if any is missing.
"""
import sys
import time

import pyatspi

app_name, wanted = sys.argv[1], sys.argv[2:]


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
        print("atspi-roles: waiting for {} among {}".format(app_name, names), flush=True)
        time.sleep(0.5)
    return None


def walk(acc, depth, out):
    try:
        states = ",".join(pyatspi.stateToString(s) for s in acc.getState().getStates())
        print("atspi-roles: {}{} \"{}\" [{}]".format("  " * depth, acc.getRoleName(), acc.name, states), flush=True)
        out.append((acc.getRoleName(), acc.name))
        for i in range(acc.childCount):
            child = acc.getChildAtIndex(i)
            if child is not None:
                walk(child, depth + 1, out)
    except Exception as e:  # an object replaced mid-walk
        print("atspi-roles: (unreadable: {})".format(e), flush=True)


app = find_app()
if app is None:
    print("atspi-roles: FAILED: {} isn't registered".format(app_name))
    sys.exit(1)
seen = []
walk(app, 0, seen)
missing = []
for want in wanted:
    role, _, name = want.partition("|")
    if not any(r == role and (name == "*" or n.startswith(name)) for r, n in seen):
        missing.append(want)
if missing:
    print("atspi-roles: FAILED: missing {}".format(missing))
    sys.exit(1)
print("atspi-roles: ok ({} elements)".format(len(seen)))
