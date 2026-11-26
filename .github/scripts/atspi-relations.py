#!/usr/bin/env python3
"""Follow a running Stipple app's relations through AT-SPI, as Orca does.

usage: atspi-relations.py APP_NAME CHECK...
  controls  the first combo box, opened by its "click" action, is
            controller-for its list, which is controlled-by it back
            (widgetsdemo's "Size" select)
  radios    a radio button's member-of relation names its whole group
            (widgetsdemo's "Format" choice chips: PNG, JPEG, ...)
  tree=NAME the tree item NAME is node-child-of a tree item one level up
            (its "level" attribute), which is node-parent-of it back
            (mediabrowser's folder tree: tree=Music)
  places    a radio button's "posinset" / "setsize" attributes are its
            place in its member-of group (widgetsdemo's "Format" chips)
  places=NAME
            the tree item NAME's "posinset" / "setsize" are its place among
            the node-parent-of items of its parent (mediabrowser: places=Music)

Prints what it finds and `atspi-relations: ok`, or the failures; exits 1 on
any failure.
"""
import sys
import time

import pyatspi

app_name, checks = sys.argv[1], sys.argv[2:]
failures = []


def check(ok, what):
    print("atspi-relations: {} {}".format("ok  " if ok else "FAIL", what), flush=True)
    if not ok:
        failures.append(what)


def find_app():
    desktop = pyatspi.Registry.getDesktop(0)
    for _ in range(60):
        for i in range(desktop.childCount):
            app = desktop.getChildAtIndex(i)
            if app is not None and app.name == app_name and app.childCount > 0:
                return app
        time.sleep(0.5)
    return None


def walk(acc, out):
    try:
        out.append(acc)
        for i in range(acc.childCount):
            child = acc.getChildAtIndex(i)
            if child is not None:
                walk(child, out)
    except Exception:  # an object replaced mid-walk
        pass
    return out


def find(app, role, name=None):
    for acc in walk(app, []):
        try:
            if acc.getRole() == role and (name is None or acc.name == name):
                return acc
        except Exception:
            pass
    return None


def targets(acc, kind):
    out = []
    for rel in acc.getRelationSet():
        if rel.getRelationType() == kind:
            out.extend(rel.getTarget(i) for i in range(rel.getNTargets()))
    return out


def attribute(acc, name):
    for attr in acc.getAttributes():
        key, _, value = attr.partition(":")
        if key == name:
            return int(value)
    return None


def level(acc):
    return attribute(acc, "level")


def place(acc):
    """(posinset, setsize), or None where either is missing."""
    at, size = attribute(acc, "posinset"), attribute(acc, "setsize")
    return None if at is None or size is None else (at, size)


app = find_app()
if app is None:
    print("atspi-relations: FAILED: {} isn't registered".format(app_name))
    sys.exit(1)

for want in checks:
    if want == "controls":
        combo = find(app, pyatspi.ROLE_COMBO_BOX)
        check(combo is not None, "a combo box")
        if combo is None:
            continue
        check(not targets(combo, pyatspi.RELATION_CONTROLLER_FOR), "...controlling nothing while closed")
        check(combo.queryAction().doAction(0), "...its click action is accepted")
        lists = []
        for _ in range(20):
            time.sleep(0.5)
            combo = find(app, pyatspi.ROLE_COMBO_BOX)
            lists = targets(combo, pyatspi.RELATION_CONTROLLER_FOR) if combo else []
            if lists:
                break
        check(len(lists) == 1, "...then controller-for one object ({})".format(lists))
        if lists:
            lst = lists[0]
            check(lst.getRoleName() == "list" and lst.childCount > 0,
                  "...a list with items ({} '{}', {} children)".format(lst.getRoleName(), lst.name, lst.childCount))
            back = targets(lst, pyatspi.RELATION_CONTROLLED_BY)
            check(len(back) == 1 and back[0].getRole() == pyatspi.ROLE_COMBO_BOX,
                  "...controlled-by the combo box back ({})".format(back))
    elif want == "radios":
        radio = find(app, pyatspi.ROLE_RADIO_BUTTON)
        check(radio is not None, "a radio button")
        if radio is None:
            continue
        group = [r.name for r in targets(radio, pyatspi.RELATION_MEMBER_OF)]
        check(len(group) > 1 and radio.name in group,
              "'{}' is a member of its group {}".format(radio.name, group))
    elif want.startswith("tree="):
        name = want[len("tree="):]
        item = find(app, pyatspi.ROLE_TREE_ITEM, name)
        check(item is not None, "a tree item '{}'".format(name))
        if item is None:
            continue
        parents = targets(item, pyatspi.RELATION_NODE_CHILD_OF)
        check(len(parents) == 1 and parents[0].getRole() == pyatspi.ROLE_TREE_ITEM,
              "...node-child-of a tree item ({})".format([p.name for p in parents]))
        if parents:
            parent = parents[0]
            mine, theirs = level(item), level(parent)
            check(mine is not None and theirs is not None and mine == theirs + 1,
                  "...one level below it (levels {} and {})".format(mine, theirs))
            kids = [k.name for k in targets(parent, pyatspi.RELATION_NODE_PARENT_OF)]
            check(name in kids, "...which is node-parent-of it back ({})".format(kids))
    elif want == "places":
        radio = find(app, pyatspi.ROLE_RADIO_BUTTON)
        check(radio is not None, "a radio button")
        if radio is None:
            continue
        group = [r.name for r in targets(radio, pyatspi.RELATION_MEMBER_OF)]
        got = place(radio)
        want_at = (group.index(radio.name) + 1, len(group)) if radio.name in group else None
        check(got is not None and got == want_at,
              "'{}' is {} of its group {} (posinset, setsize: {})".format(radio.name, want_at, group, got))
    elif want.startswith("places="):
        name = want[len("places="):]
        item = find(app, pyatspi.ROLE_TREE_ITEM, name)
        check(item is not None, "a tree item '{}'".format(name))
        if item is None:
            continue
        got = place(item)
        parents = targets(item, pyatspi.RELATION_NODE_CHILD_OF)
        siblings = [k.name for k in targets(parents[0], pyatspi.RELATION_NODE_PARENT_OF)] if parents else []
        want_at = (siblings.index(name) + 1, len(siblings)) if name in siblings else None
        check(got is not None and got == want_at,
              "...is {} of its parent's items {} (posinset, setsize: {})".format(want_at, siblings, got))
    else:
        check(False, "unknown check {}".format(want))

if failures:
    print("atspi-relations: FAILED: {}".format(failures))
    sys.exit(1)
print("atspi-relations: ok")
