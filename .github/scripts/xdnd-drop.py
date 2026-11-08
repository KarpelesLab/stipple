#!/usr/bin/env python3
"""Act as an XDND drag source and drop files or text on an X11 window.

Usage: xdnd-drop.py WINDOW (files PATH... | text STRING) [--at X Y]
                    [--expect accept|refuse] [--hover SECS]

WINDOW is an X window id (decimal or 0x hex), e.g. from `xdotool search
--name`. X Y are window-relative pixels (default 100 100). The script sends
XdndEnter, two XdndPositions, then XdndDrop (or XdndLeave when the target
refuses), serves the selection when the target asks for it, and waits for
XdndFinished; --hover lingers SECS over the target first. It exits 0 when
the target answered as --expect says (accept by default), and prints every
step. Needs python3-xlib.
"""

import sys
import time
from urllib.parse import quote

from Xlib import X, Xatom, display
from Xlib.protocol import event


def main():
    args = sys.argv[1:]
    at = (100, 100)
    expect = "accept"
    hover = 0.0
    if "--hover" in args:
        i = args.index("--hover")
        hover = float(args[i + 1])
        del args[i:i + 2]
    if "--at" in args:
        i = args.index("--at")
        at = (int(args[i + 1]), int(args[i + 2]))
        del args[i:i + 3]
    if "--expect" in args:
        i = args.index("--expect")
        expect = args[i + 1]
        del args[i:i + 2]
    target_id = int(args[0], 0)
    kind = args[1]
    payload = args[2:]

    d = display.Display()
    atom = lambda name: d.intern_atom(name)
    a = {n: atom(n) for n in [
        "XdndAware", "XdndEnter", "XdndPosition", "XdndStatus", "XdndLeave",
        "XdndDrop", "XdndFinished", "XdndSelection", "XdndActionCopy",
        "text/uri-list", "UTF8_STRING", "text/plain", "TARGETS"]}
    root = d.screen().root
    target = d.create_resource_object("window", target_id)

    aware = target.get_full_property(a["XdndAware"], Xatom.ATOM)
    print(f"xdnd-drop: target {target_id:#x} XdndAware = "
          f"{aware.value[0] if aware else None}")
    if not aware:
        sys.exit("xdnd-drop: the target isn't XdndAware")

    if kind == "files":
        offered = a["text/uri-list"]
        data = "".join("file://" + quote(p) + "\r\n" for p in payload).encode()
    else:
        offered = a["UTF8_STRING"]
        data = " ".join(payload).encode()
    types = [offered] + ([a["text/plain"]] if kind == "text" else [])

    source = root.create_window(0, 0, 1, 1, 0, X.CopyFromParent,
                                event_mask=X.PropertyChangeMask)
    source.set_selection_owner(a["XdndSelection"], X.CurrentTime)
    d.flush()

    def send(kind_atom, data32):
        ev = event.ClientMessage(window=target, client_type=kind_atom,
                                 data=(32, (data32 + [0] * 5)[:5]))
        target.send_event(ev, event_mask=0)
        d.flush()

    def wait_for(kind_atom, timeout=5.0):
        """Wait for a ClientMessage of `kind_atom`, serving the selection."""
        end = time.time() + timeout
        while time.time() < end:
            while d.pending_events():
                ev = d.next_event()
                if ev.type == X.SelectionRequest:
                    serve(ev)
                elif ev.type == X.ClientMessage and ev.client_type == kind_atom:
                    return ev
            time.sleep(0.01)
        return None

    def serve(req):
        print(f"xdnd-drop: target asks for {d.get_atom_name(req.target)}")
        prop = req.property or req.target
        if req.target == a["TARGETS"]:
            req.requestor.change_property(prop, Xatom.ATOM, 32, types)
        elif req.target in types:
            req.requestor.change_property(prop, req.target, 8, data)
        else:
            prop = X.NONE
        notify = event.SelectionNotify(time=req.time, requestor=req.requestor,
                                       selection=req.selection,
                                       target=req.target, property=prop)
        req.requestor.send_event(notify, event_mask=0)
        d.flush()

    coords = target.translate_coords(root, 0, 0)
    ox, oy = -coords.x, -coords.y  # target origin on the root window
    send(a["XdndEnter"], [source.id, 5 << 24] + types[:3])
    accepted = False
    for dx in (0, 5):
        x, y = ox + at[0] + dx, oy + at[1]
        send(a["XdndPosition"],
             [source.id, 0, (x << 16) | y, X.CurrentTime, a["XdndActionCopy"]])
        status = wait_for(a["XdndStatus"])
        if status is None:
            sys.exit("xdnd-drop: no XdndStatus from the target")
        flags = status.data[1][1]
        accepted = bool(flags & 1)
        print(f"xdnd-drop: status at ({at[0] + dx}, {at[1]}): "
              f"{'accepted' if accepted else 'refused'}"
              f" action={status.data[1][4]}")
        time.sleep(0.2)
    # Linger over the target (a screenshot can catch its drag-over look).
    time.sleep(hover)

    if not accepted:
        send(a["XdndLeave"], [source.id])
        print("xdnd-drop: left without dropping")
        sys.exit(0 if expect == "refuse" else 1)

    send(a["XdndDrop"], [source.id, 0, X.CurrentTime])
    finished = wait_for(a["XdndFinished"])
    if finished is None:
        sys.exit("xdnd-drop: no XdndFinished from the target")
    ok = bool(finished.data[1][1] & 1)
    print(f"xdnd-drop: finished, {'accepted' if ok else 'refused'}")
    sys.exit(0 if ok == (expect == "accept") else 1)


if __name__ == "__main__":
    main()
