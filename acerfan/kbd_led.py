"""Expose the keyboard backlight as /sys/class/leds/acerfan::kbd_backlight via uleds.

UPower picks up any LED named '*::kbd_backlight', so GNOME's keyboard
brightness slider and keys start working. Writes to the LED show up as
4-byte ints on the /dev/uleds fd; we forward them to the firmware.
"""

from __future__ import annotations

import logging
import os
import struct
import subprocess
import threading
from typing import Callable, Optional

log = logging.getLogger("acerfand.kbdled")

LED_NAME = "acerfan::kbd_backlight"
LED_MAX_NAME_SIZE = 64
MAX_BRIGHTNESS = 255


def pack_uleds(name: str, max_brightness: int) -> bytes:
    raw = name.encode()[:LED_MAX_NAME_SIZE - 1]
    return struct.pack(f"{LED_MAX_NAME_SIZE}si", raw, max_brightness)


class KbdLedBridge:
    def __init__(self, on_brightness: Callable[[int], None]):
        self.on_brightness = on_brightness
        self.fd: Optional[int] = None
        self._suppress: Optional[int] = None

    def start(self) -> bool:
        if not os.path.exists("/dev/uleds"):
            subprocess.run(["modprobe", "uleds"], check=False,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            self.fd = os.open("/dev/uleds", os.O_RDWR)
            os.write(self.fd, pack_uleds(LED_NAME, MAX_BRIGHTNESS))
        except OSError as e:
            log.warning("keyboard LED bridge unavailable: %s", e)
            self.fd = None
            return False
        threading.Thread(target=self._loop, daemon=True).start()
        log.info("created /sys/class/leds/%s", LED_NAME)
        return True

    def sync(self, brightness: int) -> None:
        """Mirror a brightness change made elsewhere (app, CLI) into the LED device."""
        path = f"/sys/class/leds/{LED_NAME}/brightness"
        try:
            self._suppress = int(brightness)
            with open(path, "w") as f:
                f.write(str(int(brightness)))
        except OSError:
            self._suppress = None

    def _loop(self):
        while self.fd is not None:
            try:
                data = os.read(self.fd, 4)
            except OSError:
                break
            if len(data) != 4:
                continue
            value = struct.unpack("i", data)[0]
            if self._suppress is not None and value == self._suppress:
                self._suppress = None
                continue
            try:
                self.on_brightness(max(0, min(MAX_BRIGHTNESS, value)))
            except Exception as e:  # noqa: BLE001
                log.error("applying keyboard brightness failed: %s", e)

    def stop(self):
        fd, self.fd = self.fd, None
        if fd is not None:
            os.close(fd)  # removes the LED device
