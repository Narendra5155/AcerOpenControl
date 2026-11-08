"""Fn hotkeys that the EC routes through the Clevo WMI event (NEVT -> Notify(WMI, 0xD0)).

The firmware only delivers these once a driver has registered: SCMD 0x46
sets \\_SB.WMI.HKDR = 1. Sleep (_PTS) clears it, so it must be re-enabled after resume.
The kernel WMI core forwards the 0xD0 notify as an ACPI netlink event; the
key code is then read with GCMD 0x01 (WMI.EVNT).

Keys GNOME already understands are re-emitted as normal key presses through
a uinput keyboard, so GNOME shows its own OSD and handles them natively.
"""

from __future__ import annotations

import fcntl
import logging
import os
import socket
import struct
import threading
import time
from typing import Callable, Iterator, Optional, Tuple

log = logging.getLogger("acerfand.hotkeys")

CMD_ENABLE_HOTKEYS = 0x46
CMD_GET_EVENT = 0x01
WMI_EVENT_NOTIFY = 0xD0
AC_ADJP_PATH = r"\_SB.AC.ADJP"

# Event codes CONFIRMED on the A715-79G with `acerfan keys` (2026-11-08):
#   Fn+F1 -> 0xFD, Fn+F10 -> 0xF7, Fn+num- -> 0x81, Fn+num+ -> 0x82,
#   Fn+/ -> 0x83, Fn+* -> 0x9F.
# The others come from this DSDT's _Q table and haven't been seen yet.
EV_KBD_DOWN = 0x81
EV_KBD_UP = 0x82
EV_KBD_TOGGLE = 0x83        # Fn + '/'
EV_KBD_FUNCTION = 0x9F      # Fn + '*': we cycle colours
EV_KBD_CYCLE_BRIGHT = 0x8A
EV_TOUCHPAD_TOGGLE = 0x5D
EV_TOUCHPAD_OFF = 0xFC      # Fn+F1 sends these; the EC's on/off state can disagree
EV_TOUCHPAD_ON = 0xFD       # with GNOME's, so both are treated as a toggle.
EV_WEBCAM_TOGGLE = 0xF7     # Fn+F10
EV_PROFILE_CYCLE = 0x59

KNOWN = {
    EV_KBD_DOWN: "kbd-backlight-down", EV_KBD_UP: "kbd-backlight-up",
    EV_KBD_FUNCTION: "kbd-backlight-function", EV_KBD_CYCLE_BRIGHT: "kbd-backlight-cycle",
    EV_KBD_TOGGLE: "kbd-backlight-toggle", EV_TOUCHPAD_TOGGLE: "touchpad-toggle",
    EV_TOUCHPAD_OFF: "touchpad (fn+f1)", EV_TOUCHPAD_ON: "touchpad (fn+f1)",
    EV_WEBCAM_TOGGLE: "webcam-toggle", EV_PROFILE_CYCLE: "profile-cycle",
}

# -- Linux input key codes ------------------------------------------------------------
KEY_KBDILLUMTOGGLE = 228
KEY_KBDILLUMDOWN = 229
KEY_KBDILLUMUP = 230
KEY_TOUCHPAD_TOGGLE = 530
KEY_TOUCHPAD_ON = 531
KEY_TOUCHPAD_OFF = 532
UINPUT_KEYS = (KEY_KBDILLUMTOGGLE, KEY_KBDILLUMDOWN, KEY_KBDILLUMUP,
               KEY_TOUCHPAD_TOGGLE, KEY_TOUCHPAD_ON, KEY_TOUCHPAD_OFF)


# =========================================================================== netlink
NETLINK_GENERIC = 16
SOL_NETLINK = 270
NETLINK_ADD_MEMBERSHIP = 1
GENL_ID_CTRL = 0x10
CTRL_CMD_GETFAMILY = 3
CTRL_ATTR_FAMILY_ID = 1
CTRL_ATTR_FAMILY_NAME = 2
CTRL_ATTR_MCAST_GROUPS = 7
CTRL_ATTR_MCAST_GRP_NAME = 1
CTRL_ATTR_MCAST_GRP_ID = 2
NLM_F_REQUEST = 1
NLMSG_ERROR = 2
ACPI_GENL_ATTR_EVENT = 1
ACPI_EVENT_STRUCT = struct.Struct("20s15sxII")   # class[20], bus_id[15], pad, type, data


def _align4(n: int) -> int:
    return (n + 3) & ~3


def _attrs(buf: bytes) -> Iterator[Tuple[int, bytes]]:
    off = 0
    while off + 4 <= len(buf):
        ln, typ = struct.unpack_from("HH", buf, off)
        if ln < 4:
            break
        yield typ & 0x3FFF, buf[off + 4:off + ln]
        off += _align4(ln)


def parse_acpi_event(payload: bytes) -> Optional[Tuple[str, str, int, int]]:
    """Parse the genetlink payload (after genlmsghdr) of an acpi_event message."""
    for typ, val in _attrs(payload):
        if typ == ACPI_GENL_ATTR_EVENT and len(val) >= ACPI_EVENT_STRUCT.size:
            cls, bus, etype, data = ACPI_EVENT_STRUCT.unpack_from(val)
            return (cls.split(b"\0")[0].decode(errors="replace"),
                    bus.split(b"\0")[0].decode(errors="replace"), etype, data)
    return None


class AcpiEventListener:
    """Subscribes to the kernel's 'acpi_event' generic-netlink multicast group."""

    def __init__(self):
        self.sock = socket.socket(socket.AF_NETLINK, socket.SOCK_RAW, NETLINK_GENERIC)
        self.sock.bind((0, 0))
        family, group = self._resolve("acpi_event", "acpi_mc_group")
        self.family = family
        self.sock.setsockopt(SOL_NETLINK, NETLINK_ADD_MEMBERSHIP, group)

    def _resolve(self, fam_name: str, grp_name: str) -> Tuple[int, int]:
        name = fam_name.encode() + b"\0"
        attr = struct.pack("HH", 4 + len(name), CTRL_ATTR_FAMILY_NAME) + name
        attr += b"\0" * (_align4(len(attr)) - len(attr))
        genl = struct.pack("BBH", CTRL_CMD_GETFAMILY, 1, 0)
        body = genl + attr
        msg = struct.pack("IHHII", 16 + len(body), GENL_ID_CTRL, NLM_F_REQUEST, 1, 0) + body
        self.sock.send(msg)
        data = self.sock.recv(65536)
        ln, mtype = struct.unpack_from("IH", data, 0)
        if mtype == NLMSG_ERROR:
            raise OSError("acpi_event netlink family not found")
        fam_id, grp_id = None, None
        for typ, val in _attrs(data[20:ln]):
            if typ == CTRL_ATTR_FAMILY_ID:
                fam_id = struct.unpack_from("H", val)[0]
            elif typ == CTRL_ATTR_MCAST_GROUPS:
                for _, grp in _attrs(val):
                    g = dict(_attrs(grp))
                    gname = g.get(CTRL_ATTR_MCAST_GRP_NAME, b"").split(b"\0")[0].decode()
                    if gname == grp_name and CTRL_ATTR_MCAST_GRP_ID in g:
                        grp_id = struct.unpack_from("I", g[CTRL_ATTR_MCAST_GRP_ID])[0]
        if fam_id is None or grp_id is None:
            raise OSError("acpi_event multicast group not found")
        return fam_id, grp_id

    def events(self) -> Iterator[Tuple[str, str, int, int]]:
        while True:
            data = self.sock.recv(65536)
            off = 0
            while off + 16 <= len(data):
                ln, mtype = struct.unpack_from("IH", data, off)
                if ln < 16:
                    break
                if mtype == self.family:
                    ev = parse_acpi_event(data[off + 20:off + ln])
                    if ev:
                        yield ev
                off += _align4(ln)

    def close(self):
        self.sock.close()


def is_hotkey_event(ev: Tuple[str, str, int, int]) -> bool:
    cls, bus, etype, _ = ev
    # The Clevo event GUID is ABBC0F6B-...; the bus id is truncated to 14 chars.
    return etype == WMI_EVENT_NOTIFY and (bus.upper().startswith("ABBC0F6B") or "PNP0C14" in bus
                                          or cls.lower() == "wmi")


# ============================================================================ uinput
UI_SET_EVBIT = 0x40045564
UI_SET_KEYBIT = 0x40045565
UI_DEV_SETUP = 0x405C5503
UI_DEV_CREATE = 0x5501
UI_DEV_DESTROY = 0x5502
EV_SYN, EV_KEY, SYN_REPORT = 0, 1, 0
BUS_HOST = 0x19


class VirtualKeyboard:
    def __init__(self, name: str = "AcerFan hotkeys"):
        self.fd = os.open("/dev/uinput", os.O_WRONLY | os.O_NONBLOCK)
        fcntl.ioctl(self.fd, UI_SET_EVBIT, EV_KEY)
        for k in UINPUT_KEYS:
            fcntl.ioctl(self.fd, UI_SET_KEYBIT, k)
        setup = struct.pack("HHHH80sI", BUS_HOST, 0xAC37, 0x0001, 1, name.encode(), 0)
        fcntl.ioctl(self.fd, UI_DEV_SETUP, setup)
        fcntl.ioctl(self.fd, UI_DEV_CREATE)
        time.sleep(0.2)  # let udev/libinput pick the device up

    def _emit(self, etype, code, value):
        now = time.time()
        sec = int(now)
        os.write(self.fd, struct.pack("llHHi", sec, int((now - sec) * 1e6), etype, code, value))

    def tap(self, key: int):
        self._emit(EV_KEY, key, 1)
        self._emit(EV_SYN, SYN_REPORT, 0)
        self._emit(EV_KEY, key, 0)
        self._emit(EV_SYN, SYN_REPORT, 0)

    def close(self):
        try:
            fcntl.ioctl(self.fd, UI_DEV_DESTROY)
        finally:
            os.close(self.fd)


# =========================================================================== service
class HotkeyService:
    """Enables firmware hotkeys, listens for them and dispatches actions.

    `actions` maps an action name to a callable; the daemon provides
    'webcam-toggle', 'kbd-function' and 'profile-cycle'. Key-style actions go
    through the virtual keyboard.
    """

    def __init__(self, ec, actions: dict, keymap: Optional[dict] = None):
        self.ec = ec
        self.actions = actions
        self.keymap = {**DEFAULT_KEYMAP, **(keymap or {})}
        self.kbd: Optional[VirtualKeyboard] = None
        self.listener: Optional[AcpiEventListener] = None
        self.last_event: Optional[dict] = None
        self.enabled = False

    def enable_firmware(self) -> None:
        """SCMD 0x46: sets WMI.HKDR. Returns PSF3, not the command id."""
        self.ec.acpi.wmbb(CMD_ENABLE_HOTKEYS, 0)
        self.enabled = True

    def start(self) -> bool:
        try:
            self.listener = AcpiEventListener()
        except OSError as e:
            log.warning("hotkeys disabled: cannot subscribe to ACPI netlink events: %s", e)
            return False
        try:
            self.kbd = VirtualKeyboard()
        except OSError as e:
            log.warning("virtual keyboard unavailable (%s); GNOME-handled keys won't work", e)
        self.enable_firmware()
        threading.Thread(target=self._loop, daemon=True).start()
        log.info("Fn hotkeys enabled")
        return True

    def _loop(self):
        for ev in self.listener.events():
            if not is_hotkey_event(ev):
                continue
            try:
                code = int(self.ec.get(CMD_GET_EVENT)) & 0xFF
            except Exception as e:  # noqa: BLE001
                log.error("reading hotkey code failed: %s", e)
                continue
            self.handle_code(code)

    def handle_code(self, code: int):
        action = self.keymap.get(code)
        self.last_event = {"code": code, "action": action, "time": time.time()}
        log.info("hotkey 0x%02x -> %s", code, action or "unmapped")
        if action is None:
            return
        try:
            if action.startswith("key:"):
                if self.kbd:
                    self.kbd.tap(int(action[4:]))
            elif action in self.actions:
                self.actions[action]()
        except Exception as e:  # noqa: BLE001
            log.error("hotkey action %s failed: %s", action, e)

    def stop(self):
        if self.kbd:
            self.kbd.close()
            self.kbd = None
        if self.listener:
            self.listener.close()


# Keyboard-light keys are handled by the daemon itself rather than re-emitted
# as KEY_KBDILLUM*: gnome-settings-daemon only looks for a keyboard backlight
# at login, and our uleds LED appears later (and again on every daemon restart),
# so GNOME silently ignores those keys. The touchpad key does work through GNOME.
DEFAULT_KEYMAP = {
    EV_KBD_DOWN: "kbd-down",
    EV_KBD_UP: "kbd-up",
    EV_KBD_TOGGLE: "kbd-toggle",
    EV_KBD_CYCLE_BRIGHT: "kbd-up",
    EV_KBD_FUNCTION: "kbd-function",
    EV_TOUCHPAD_TOGGLE: f"key:{KEY_TOUCHPAD_TOGGLE}",
    EV_TOUCHPAD_OFF: f"key:{KEY_TOUCHPAD_TOGGLE}",
    EV_TOUCHPAD_ON: f"key:{KEY_TOUCHPAD_TOGGLE}",
    EV_WEBCAM_TOGGLE: "webcam-toggle",
    EV_PROFILE_CYCLE: "profile-cycle",
}


def run_monitor(seconds: int = 60) -> int:
    """`sudo acerfan keys`: enable firmware hotkeys and print every event."""
    from .acpi import ensure_module
    from .ec import ClevoEC
    if os.geteuid() != 0:
        print("run as root: sudo acerfan keys")
        return 2
    ensure_module()
    ec = ClevoEC()
    listener = AcpiEventListener()
    ec.acpi.wmbb(CMD_ENABLE_HOTKEYS, 0)
    print(f"Hotkeys enabled. Press Fn keys now (listening {seconds} s, Ctrl+C to stop).\n")
    deadline = time.monotonic() + seconds
    listener.sock.settimeout(1.0)
    try:
        while time.monotonic() < deadline:
            try:
                for ev in listener.events():
                    cls, bus, etype, data = ev
                    if is_hotkey_event(ev):
                        code = int(ec.get(CMD_GET_EVENT)) & 0xFF
                        name = KNOWN.get(code, "UNKNOWN")
                        print(f"  HOTKEY code=0x{code:02x}  {name}")
                    else:
                        print(f"  (acpi event: class={cls!r} bus={bus!r} type=0x{etype:x} data=0x{data:x})")
                    if time.monotonic() >= deadline:
                        break
            except socket.timeout:
                continue
    except KeyboardInterrupt:
        pass
    listener.close()
    print("\nDone. Hotkeys stay enabled until the next sleep/reboot.")
    return 0
