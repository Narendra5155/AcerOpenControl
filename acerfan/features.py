"""Non-fan firmware features, all verified by `acerfan probe-features` on 2026-11-08.

Keyboard   SCMD 0x67: 0xF0000000 | B<<16 | R<<8 | G  -> EC CA 03 (single-zone RGB, BRG wiring)
           SCMD 0x67: 0xF4000000 | level(0..255)     -> EC CA 06 (brightness, 0 = off)
           No readback (GCMD 0x3D always returns 1), so the daemon remembers the state.
Profile    0x79: 0x19<<24 | n, current index in \\_SB.PC00.LPCB.EC.CPCM
           Measured: n=0/1 -> CPU 15/30 W; n=3 -> 45/115 W; n=2 -> 90/115 W + GPU 70 W.
Battery    0x79: 0x06<<24 | stop<<8 | start ; then 0x79: 0x05<<24 | enable
           GCMD 0x77 -> (stop<<16) | (start<<8) | enable
Webcam     SCMD 0x22: 0 = off, 1 = on (state observed through USB camera presence)
"""

from __future__ import annotations

import glob
import json
import os
from dataclasses import dataclass
from typing import Dict, Optional

from .acpi import AcpiError
from .config import CONFIG_DIR

CAPS_PATH = os.path.join(CONFIG_DIR, "capabilities.json")

CMD_KBD = 0x67
KBD_COLOUR = 0xF0000000
KBD_BRIGHT = 0xF4000000
CMD_OPT = 0x79
OPT_CHARGE_ENABLE = 0x05
OPT_CHARGE_THRESH = 0x06
OPT_PERF_PROFILE = 0x19
CMD_GET_CHARGE = 0x77
CMD_WEBCAM = 0x22
CPCM_PATH = r"\_SB.PC00.LPCB.EC.CPCM"

# index -> (key, label, description from measured limits)
PERF_PROFILES = {
    0: ("quiet", "Quiet", "CPU 15 W sustained / 30 W boost; quietest"),
    1: ("power-saver", "Power saver", "CPU 15 W / 30 W; maximum battery life"),
    3: ("balanced", "Balanced", "CPU 45 W / 115 W, GPU 60 W"),
    2: ("performance", "Performance", "CPU 90 W / 115 W, GPU 70 W"),
}
PERF_BY_KEY = {v[0]: k for k, v in PERF_PROFILES.items()}


def load_caps(path: str = CAPS_PATH) -> dict:
    try:
        with open(path) as f:
            return json.load(f)
    except (OSError, ValueError):
        return {}


def camera_present() -> bool:
    for p in glob.glob("/sys/bus/usb/devices/*/product"):
        try:
            with open(p) as f:
                if "cam" in f.read().lower():
                    return True
        except OSError:
            pass
    return False


@dataclass
class ChargeLimit:
    enabled: bool
    start: int
    stop: int

    def to_dict(self):
        return {"enabled": self.enabled, "start": self.start, "stop": self.stop}


def validate_charge(enabled, start, stop) -> ChargeLimit:
    start, stop = int(start), int(stop)
    if not 50 <= stop <= 100:
        raise ValueError("stop must be 50..100 %")
    if not 40 <= start < stop:
        raise ValueError("start must be at least 40 % and below stop")
    return ChargeLimit(bool(enabled), start, stop)


def validate_rgb(r, g, b):
    vals = [int(v) for v in (r, g, b)]
    if any(not 0 <= v <= 255 for v in vals):
        raise ValueError("colour components must be 0..255")
    return vals


class Features:
    def __init__(self, ec, caps: Optional[dict] = None):
        self.ec = ec
        self.caps = caps if caps is not None else load_caps()
        shift = self.caps.get("kbd_channel_shift") or {"blue": 16, "red": 8, "green": 0}
        self.shift: Dict[str, int] = {k: int(v) for k, v in shift.items()}

    def available(self) -> dict:
        return {
            "keyboard": bool(self.caps.get("kbd_rgb")),
            "perf_profile": bool(self.caps.get("perf_profile")),
            "battery_limit": bool(self.caps.get("battery_limit")),
            "webcam": bool(self.caps.get("webcam_switch")),
        }

    # -- keyboard ---------------------------------------------------------------
    def kbd_colour_arg(self, r, g, b) -> int:
        r, g, b = validate_rgb(r, g, b)
        return KBD_COLOUR | (r << self.shift["red"]) | (g << self.shift["green"]) | \
            (b << self.shift["blue"])

    def set_kbd(self, r, g, b, brightness) -> None:
        brightness = int(brightness)
        if not 0 <= brightness <= 255:
            raise ValueError("brightness must be 0..255")
        self.ec.set(CMD_KBD, self.kbd_colour_arg(r, g, b))
        self.ec.set(CMD_KBD, KBD_BRIGHT | brightness)

    def set_kbd_brightness(self, brightness) -> None:
        brightness = int(brightness)
        if not 0 <= brightness <= 255:
            raise ValueError("brightness must be 0..255")
        self.ec.set(CMD_KBD, KBD_BRIGHT | brightness)

    # -- performance profile -----------------------------------------------------
    def get_perf_profile(self) -> Optional[int]:
        try:
            v = self.ec.acpi.call(CPCM_PATH)
        except AcpiError:
            return None
        return v if isinstance(v, int) and v in PERF_PROFILES else None

    def set_perf_profile(self, idx: int) -> None:
        if idx not in PERF_PROFILES:
            raise ValueError(f"profile must be one of {sorted(PERF_PROFILES)}")
        self.ec.set(CMD_OPT, (OPT_PERF_PROFILE << 24) | idx)

    # -- battery -------------------------------------------------------------------
    def get_charge(self) -> ChargeLimit:
        v = self.ec.get(CMD_GET_CHARGE)
        return ChargeLimit(bool(v & 0xFF), (v >> 8) & 0xFF, (v >> 16) & 0xFF)

    def set_charge(self, enabled, start, stop) -> ChargeLimit:
        lim = validate_charge(enabled, start, stop)
        self.ec.set(CMD_OPT, (OPT_CHARGE_THRESH << 24) | (lim.stop << 8) | lim.start)
        self.ec.set(CMD_OPT, (OPT_CHARGE_ENABLE << 24) | (1 if lim.enabled else 0))
        return self.get_charge()

    # -- webcam ----------------------------------------------------------------------
    def set_webcam(self, on: bool) -> None:
        off_val = self.caps.get("webcam_off_value", 0)
        self.ec.set(CMD_WEBCAM, (1 - off_val) if on else off_val)

    def webcam_on(self) -> bool:
        return camera_present()
