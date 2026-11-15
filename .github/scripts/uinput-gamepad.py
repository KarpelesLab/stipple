#!/usr/bin/env python3
"""Create a virtual gamepad through /dev/uinput (standard library only) and
keep it until killed. CI runs `gamepaddemo --list` against it.

Usage: uinput-gamepad.py [seconds]   (default 30)
"""
import fcntl
import os
import struct
import sys
import time


def ioc(direction, nr, size):
    return (direction << 30) | (size << 16) | (ord("U") << 8) | nr


IOW = 1
UI_DEV_CREATE = ioc(0, 1, 0)
UI_DEV_DESTROY = ioc(0, 2, 0)
UI_DEV_SETUP = ioc(IOW, 3, 92)
UI_ABS_SETUP = ioc(IOW, 4, 28)
UI_SET_EVBIT = ioc(IOW, 100, 4)
UI_SET_KEYBIT = ioc(IOW, 101, 4)
UI_SET_ABSBIT = ioc(IOW, 103, 4)

EV_KEY, EV_ABS = 0x01, 0x03
KEYS = list(range(0x130, 0x132)) + list(range(0x133, 0x135)) + list(range(0x136, 0x13F))
STICKS = [0x00, 0x01, 0x03, 0x04]  # ABS_X, ABS_Y, ABS_RX, ABS_RY
TRIGGERS = [0x02, 0x05]  # ABS_Z, ABS_RZ
HATS = [0x10, 0x11]


def main():
    seconds = float(sys.argv[1]) if len(sys.argv) > 1 else 30.0
    fd = os.open("/dev/uinput", os.O_WRONLY | os.O_NONBLOCK)
    fcntl.ioctl(fd, UI_SET_EVBIT, EV_KEY)
    fcntl.ioctl(fd, UI_SET_EVBIT, EV_ABS)
    for k in KEYS:
        fcntl.ioctl(fd, UI_SET_KEYBIT, k)
    for code, lo, hi, flat in (
        [(c, -32768, 32767, 128) for c in STICKS]
        + [(c, 0, 255, 0) for c in TRIGGERS]
        + [(c, -1, 1, 0) for c in HATS]
    ):
        fcntl.ioctl(fd, UI_SET_ABSBIT, code)
        # struct uinput_abs_setup: u16 code, (pad), struct input_absinfo.
        fcntl.ioctl(fd, UI_ABS_SETUP, struct.pack("<H2x6i", code, 0, lo, hi, 0, flat, 0))
    # struct uinput_setup: struct input_id, char name[80], u32 ff_effects_max.
    name = b"Stipple Virtual Pad"
    fcntl.ioctl(fd, UI_DEV_SETUP, struct.pack("<4H80sI", 0x06, 0x1234, 0x5678, 1, name, 0))
    fcntl.ioctl(fd, UI_DEV_CREATE)
    print("virtual gamepad created", flush=True)
    try:
        time.sleep(seconds)
    finally:
        fcntl.ioctl(fd, UI_DEV_DESTROY)
        os.close(fd)


if __name__ == "__main__":
    main()
