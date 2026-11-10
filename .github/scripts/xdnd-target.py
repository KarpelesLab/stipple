#!/usr/bin/env python3
"""Act as an XDND drop target: a window other apps can drag files and text to.

Usage: xdnd-target.py [--at X Y] [--size W H] [--count N] [--timeout SECS]
                      [--refuse]

Maps an XdndAware (version 5) window at X Y (default 600 100) of W x H
pixels (default 300 300), prints `xdnd-target: ready WINDOW` and answers
drags: XdndEnter (the offered types, from XdndTypeList when there are more
than three), XdndPosition (an XdndStatus accepting a copy, or refusing with
--refuse), XdndLeave, and XdndDrop, on which it converts XdndSelection to
text/uri-list (or UTF-8 text), prints what arrived as

    xdnd-target: dropped text/uri-list 'file:///tmp/a.txt\\r\\n'

and sends XdndFinished. It exits 0 after N drops (default 1), or 1 after
SECS seconds (default 30). Leaves are printed too. Needs python3-xlib. The
mirror of xdnd-drop.py, which is a source.
"""

import sys
import time

from Xlib import X, Xatom, display
from Xlib.protocol import event


def take(args, name, n):
    if name not in args:
        return None
    i = args.index(name)
    values = args[i + 1:i + 1 + n]
    del args[i:i + 1 + n]
    return values


def main():
    args = sys.argv[1:]
    at = [int(v) for v in (take(args, "--at", 2) or [600, 100])]
    size = [int(v) for v in (take(args, "--size", 2) or [300, 300])]
    count = int((take(args, "--count", 1) or [1])[0])
    timeout = float((take(args, "--timeout", 1) or [30])[0])
    refuse = take(args, "--refuse", 0) is not None

    d = display.Display()
    a = {n: d.intern_atom(n) for n in [
        "XdndAware", "XdndEnter", "XdndPosition", "XdndStatus", "XdndLeave",
        "XdndDrop", "XdndFinished", "XdndSelection", "XdndActionCopy",
        "XdndTypeList", "text/uri-list", "UTF8_STRING",
        "text/plain;charset=utf-8", "text/plain", "XDND_TARGET_DATA"]}
    screen = d.screen()
    win = screen.root.create_window(
        at[0], at[1], size[0], size[1], 0, screen.root_depth,
        background_pixel=screen.white_pixel, override_redirect=True,
        event_mask=X.PropertyChangeMask)
    win.change_property(a["XdndAware"], Xatom.ATOM, 32, [5])
    win.set_wm_name("xdnd target")
    win.map()
    d.sync()
    print(f"xdnd-target: ready {win.id:#x}", flush=True)

    def send(dest, kind, data32):
        ev = event.ClientMessage(window=dest, client_type=kind,
                                 data=(32, (data32 + [0] * 5)[:5]))
        dest.send_event(ev, event_mask=0)
        d.flush()

    def name(atom):
        return d.get_atom_name(atom) if atom else "None"

    source = None
    types = []
    drops = 0
    end = time.time() + timeout
    while time.time() < end:
        if not d.pending_events():
            time.sleep(0.01)
            continue
        ev = d.next_event()
        if ev.type == X.ClientMessage:
            kind = ev.client_type
            data = ev.data[1]
            if kind == a["XdndEnter"]:
                source = d.create_resource_object("window", data[0])
                version = data[1] >> 24
                types = [t for t in data[2:5] if t]
                if data[1] & 1:
                    prop = source.get_full_property(a["XdndTypeList"],
                                                    Xatom.ATOM)
                    types = list(prop.value) if prop else []
                print(f"xdnd-target: enter from {data[0]:#x} v{version}, "
                      f"types {[name(t) for t in types]}", flush=True)
            elif kind == a["XdndPosition"] and source is not None:
                x, y = data[2] >> 16, data[2] & 0xffff
                ok = not refuse and bool(types)
                send(source, a["XdndStatus"],
                     [win.id, 1 if ok else 0, 0, 0,
                      a["XdndActionCopy"] if ok else 0])
                print(f"xdnd-target: position ({x}, {y}) action "
                      f"{name(data[4])}: {'accepted' if ok else 'refused'}",
                      flush=True)
            elif kind == a["XdndLeave"]:
                print("xdnd-target: leave", flush=True)
                source = None
            elif kind == a["XdndDrop"] and source is not None:
                want = next((t for t in [a["text/uri-list"], a["UTF8_STRING"],
                                         a["text/plain;charset=utf-8"],
                                         a["text/plain"]] if t in types),
                            None)
                if want is None or refuse:
                    send(source, a["XdndFinished"], [win.id, 0, 0])
                    continue
                win.convert_selection(a["XdndSelection"], want,
                                      a["XDND_TARGET_DATA"], data[2])
                d.flush()
                got = None
                wait = time.time() + 5
                while time.time() < wait and got is None:
                    while d.pending_events():
                        e = d.next_event()
                        if e.type == X.SelectionNotify:
                            got = e
                            break
                    else:
                        time.sleep(0.01)
                payload = None
                if got is not None and got.property:
                    prop = win.get_full_property(got.property, X.AnyPropertyType)
                    if prop is not None:
                        payload = prop.value
                        if isinstance(payload, str):
                            payload = payload.encode("latin-1")
                        payload = bytes(payload).decode("utf-8", "replace")
                if payload is None:
                    print(f"xdnd-target: conversion to {name(want)} failed",
                          flush=True)
                else:
                    print(f"xdnd-target: dropped {name(want)} {payload!r}",
                          flush=True)
                send(source, a["XdndFinished"],
                     [win.id, 1 if payload is not None else 0,
                      a["XdndActionCopy"]])
                source = None
                if payload is not None:
                    drops += 1
                    if drops >= count:
                        sys.exit(0)
    print(f"xdnd-target: timed out after {drops} of {count} drops", flush=True)
    sys.exit(1)


if __name__ == "__main__":
    main()
