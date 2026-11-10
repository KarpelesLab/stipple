"""Window-manager stand-ins for the lifecycle test under Xvfb (no WM runs).

  x11-window.py close TITLE     send WM_DELETE_WINDOW to the window named TITLE,
                                as a window manager's close button does
  x11-window.py focus-away SEC  map a window of our own and give it the keyboard
                                focus for SEC seconds
"""

import sys
import time

from Xlib import X, display, protocol


def find(window, name):
    if window.get_wm_name() == name:
        return window
    for child in window.query_tree().children:
        found = find(child, name)
        if found:
            return found
    return None


def close(name):
    d = display.Display()
    window = find(d.screen().root, name)
    if window is None:
        sys.exit(f"no window named {name!r}")
    protocols = d.intern_atom("WM_PROTOCOLS")
    delete = d.intern_atom("WM_DELETE_WINDOW")
    event = protocol.event.ClientMessage(
        window=window,
        client_type=protocols,
        data=(32, [delete, X.CurrentTime, 0, 0, 0]),
    )
    window.send_event(event)
    d.flush()


def focus_away(seconds):
    d = display.Display()
    screen = d.screen()
    window = screen.root.create_window(600, 500, 50, 50, 0, screen.root_depth)
    window.map()
    d.sync()
    window.set_input_focus(X.RevertToParent, X.CurrentTime)
    d.sync()
    time.sleep(seconds)


if __name__ == "__main__":
    if sys.argv[1] == "close":
        close(sys.argv[2])
    elif sys.argv[1] == "focus-away":
        focus_away(float(sys.argv[2]))
    else:
        sys.exit(__doc__)
