"""Persistent daemon configuration (/etc/acerfan/config.json)."""

from __future__ import annotations

import json
import os
import tempfile
from dataclasses import asdict, dataclass, field
from typing import List

from .curve import PROFILES, validate_points

CONFIG_DIR = os.environ.get("ACERFAN_CONFIG_DIR", "/etc/acerfan")
CONFIG_PATH = os.path.join(CONFIG_DIR, "config.json")

MODES = ("auto", "silent", "balanced", "performance", "max", "custom")


@dataclass
class Config:
    mode: str = "balanced"
    custom_curve: List[List[float]] = field(
        default_factory=lambda: [list(p) for p in PROFILES["balanced"]])
    poll_interval: float = 1.5
    refresh_interval: float = 30.0      # resend duty even when unchanged
    hysteresis: float = 4.0
    ramp_down: float = 5.0
    floor_pct: float = 20.0
    floor_temp: float = 60.0
    cpu_emergency: float = 92.0
    gpu_emergency: float = 85.0
    # Settings for the extra firmware features; filled in by step 4.
    features: dict = field(default_factory=dict)

    def validate(self) -> "Config":
        if self.mode not in MODES:
            raise ValueError(f"unknown mode {self.mode!r}")
        self.custom_curve = [list(p) for p in validate_points(self.custom_curve)]
        if not 0.5 <= self.poll_interval <= 10:
            raise ValueError("poll_interval must be 0.5..10 s")
        return self

    def curve_points(self, mode: str = None):
        mode = mode or self.mode
        if mode == "custom":
            return self.custom_curve
        return PROFILES.get(mode)


def load(path: str = CONFIG_PATH) -> Config:
    try:
        with open(path) as f:
            data = json.load(f)
    except FileNotFoundError:
        return Config()
    known = {k: v for k, v in data.items() if k in Config.__dataclass_fields__}
    try:
        return Config(**known).validate()
    except (TypeError, ValueError):
        # Bad config file: fall back to safe defaults rather than refusing to start.
        return Config()


def save(cfg: Config, path: str = CONFIG_PATH) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=os.path.dirname(path), prefix=".config.")
    with os.fdopen(fd, "w") as f:
        json.dump(asdict(cfg), f, indent=2)
    os.chmod(tmp, 0o644)
    os.replace(tmp, path)
