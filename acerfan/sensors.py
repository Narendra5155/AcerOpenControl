"""Temperature sources. hwmon devices are matched by name, never by index."""

from __future__ import annotations

import glob
import os
import shutil
import subprocess
import time
from typing import Optional


def _read(path: str) -> Optional[str]:
    try:
        with open(path) as f:
            return f.read().strip()
    except OSError:
        return None


def find_hwmon(name: str) -> Optional[str]:
    for d in sorted(glob.glob("/sys/class/hwmon/hwmon*")):
        if _read(os.path.join(d, "name")) == name:
            return d
    return None


def cpu_temp() -> Optional[float]:
    """CPU package temperature in °C (coretemp 'Package id 0'), else hottest core."""
    d = find_hwmon("coretemp")
    if d:
        best = None
        for label_path in glob.glob(os.path.join(d, "temp*_label")):
            label = _read(label_path) or ""
            val = _read(label_path.replace("_label", "_input"))
            if val is None:
                continue
            t = int(val) / 1000.0
            if label.startswith("Package"):
                return t
            best = t if best is None else max(best, t)
        if best is not None:
            return best
    # Fall back to x86_pkg_temp thermal zone.
    for z in glob.glob("/sys/class/thermal/thermal_zone*"):
        if _read(os.path.join(z, "type")) == "x86_pkg_temp":
            v = _read(os.path.join(z, "temp"))
            if v:
                return int(v) / 1000.0
    return None


def nvidia_pci_dev() -> Optional[str]:
    for d in glob.glob("/sys/bus/pci/devices/*"):
        if _read(os.path.join(d, "vendor")) == "0x10de" and \
                (_read(os.path.join(d, "class")) or "").startswith("0x03"):
            return d
    return None


class GpuTemp:
    """GPU temperature that never wakes a runtime-suspended dGPU."""

    def __init__(self, cache_s: float = 2.0):
        self.cache_s = cache_s
        self._t = 0.0
        self._val: Optional[float] = None
        self._dev = nvidia_pci_dev()
        self._smi = shutil.which("nvidia-smi")

    def asleep(self) -> bool:
        if not self._dev:
            return True
        status = _read(os.path.join(self._dev, "power", "runtime_status"))
        return status == "suspended"

    def read(self) -> Optional[float]:
        if self.asleep() or not self._smi:
            return None
        now = time.monotonic()
        if now - self._t < self.cache_s:
            return self._val
        self._t = now
        try:
            out = subprocess.run(
                [self._smi, "--query-gpu=temperature.gpu", "--format=csv,noheader,nounits"],
                capture_output=True, text=True, timeout=3)
            self._val = float(out.stdout.strip().splitlines()[0])
        except (subprocess.SubprocessError, ValueError, IndexError, OSError):
            self._val = None
        return self._val
