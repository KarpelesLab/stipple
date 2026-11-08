#!/usr/bin/env python3
"""A stand-in Linux desktop on the session bus, for traydemo's selftest.

It owns the three services Stipple's OS integration talks to, and logs what it
is asked (one line each, prefixed so CI can grep them):

- org.freedesktop.Notifications: logs each Notify call (NOTIFY ...).
- org.freedesktop.portal.Desktop: logs each OpenURI call (OPENURI ...).
- org.kde.StatusNotifierWatcher: logs the registration (REGISTER ...), then
  acts as a tray host would: reads the item's properties (ITEM ...), fetches
  its dbusmenu layout (MENU ...), clicks the "Ping" item (CLICKED ...) and
  activates the icon (ACTIVATED).

Needs python3-dbus and python3-gi. Runs until killed.
"""

import sys

import dbus
import dbus.mainloop.glib
import dbus.service
from gi.repository import GLib

WATCHER = "org.kde.StatusNotifierWatcher"
ITEM = "org.kde.StatusNotifierItem"
MENU = "com.canonical.dbusmenu"


def log(*args):
    print(*args, flush=True)


class Notifications(dbus.service.Object):
    @dbus.service.method(
        "org.freedesktop.Notifications", in_signature="susssasa{sv}i", out_signature="u"
    )
    def Notify(self, app, replaces, icon, summary, body, actions, hints, timeout):
        log(f"NOTIFY app={app} summary={summary} body={body}")
        return dbus.UInt32(1)

    @dbus.service.method("org.freedesktop.Notifications", out_signature="as")
    def GetCapabilities(self):
        return ["body"]

    @dbus.service.method("org.freedesktop.Notifications", out_signature="ssss")
    def GetServerInformation(self):
        return ("mock-desktop", "stipple", "1", "1.2")

    @dbus.service.method("org.freedesktop.Notifications", in_signature="u")
    def CloseNotification(self, _id):
        pass


class Portal(dbus.service.Object):
    @dbus.service.method(
        "org.freedesktop.portal.OpenURI", in_signature="ssa{sv}", out_signature="o"
    )
    def OpenURI(self, parent, uri, options):
        log(f"OPENURI {uri}")
        return dbus.ObjectPath("/org/freedesktop/portal/desktop/request/1_1/mock")


class Watcher(dbus.service.Object):
    def __init__(self, bus, path):
        super().__init__(bus, path)
        self.bus = bus

    @dbus.service.method(WATCHER, in_signature="s", sender_keyword="sender")
    def RegisterStatusNotifierItem(self, service, sender=None):
        log(f"REGISTER {service} from {sender}")
        # Talk to the item once this call has returned.
        GLib.timeout_add(300, self.drive, str(service))

    @dbus.service.method(WATCHER, in_signature="s")
    def RegisterStatusNotifierHost(self, service):
        pass

    def drive(self, service):
        try:
            item = self.bus.get_object(service, "/StatusNotifierItem")
            props = dbus.Interface(item, "org.freedesktop.DBus.Properties").GetAll(ITEM)
            sizes = [f"{w}x{h}:{len(px)}" for (w, h, px) in props["IconPixmap"]]
            log(
                f"ITEM id={props['Id']} title={props['Title']} status={props['Status']} "
                f"menu={props['Menu']} pixmaps={','.join(sizes)}"
            )
            menu = dbus.Interface(self.bus.get_object(service, props["Menu"]), MENU)
            _rev, layout = menu.GetLayout(0, -1, dbus.Array([], signature="s"))
            ping = None

            def walk(node, depth):
                nonlocal ping
                ident, item_props, children = node
                label = str(item_props.get("label", item_props.get("type", "")))
                if ident != 0:
                    log(f"MENU {'  ' * depth}{ident} {label}")
                if label == "_Ping":
                    ping = ident
                for child in children:
                    walk(child, depth + 1)

            walk(layout, -1)
            if ping is None:
                log("FAIL no Ping item")
                return False
            menu.Event(ping, "clicked", dbus.Int32(0, variant_level=1), dbus.UInt32(0))
            log(f"CLICKED Ping ({ping})")
            dbus.Interface(item, ITEM).Activate(0, 0)
            log("ACTIVATED")
        except dbus.DBusException as e:
            log(f"FAIL {e}")
        return False


def main():
    dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)
    bus = dbus.SessionBus()
    names = [
        dbus.service.BusName("org.freedesktop.Notifications", bus),
        dbus.service.BusName("org.freedesktop.portal.Desktop", bus),
        dbus.service.BusName(WATCHER, bus),
    ]
    objects = [
        Notifications(bus, "/org/freedesktop/Notifications"),
        Portal(bus, "/org/freedesktop/portal/desktop"),
        Watcher(bus, "/StatusNotifierWatcher"),
    ]
    log("mock desktop ready")
    try:
        GLib.MainLoop().run()
    finally:
        del names, objects


if __name__ == "__main__":
    sys.exit(main())
