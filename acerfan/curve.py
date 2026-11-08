"""Temperature -> fan duty curves with hysteresis and ramp limiting."""

from __future__ import annotations

from typing import Iterable, List, Optional, Sequence, Tuple

Point = Tuple[float, float]  # (temp °C, duty %)

PROFILES = {
    "silent":      [(50, 0), (60, 20), (70, 35), (78, 55), (86, 80), (92, 100)],
    "balanced":    [(45, 0), (55, 25), (65, 40), (75, 60), (85, 85), (90, 100)],
    "performance": [(40, 20), (50, 35), (60, 50), (70, 70), (80, 90), (86, 100)],
}


def validate_points(points: Iterable[Sequence[float]]) -> List[Point]:
    pts = [(float(t), float(d)) for t, d in points]
    if len(pts) < 2:
        raise ValueError("curve needs at least 2 points")
    for t, d in pts:
        if not 20 <= t <= 105:
            raise ValueError(f"temperature {t} out of range 20..105")
        if not 0 <= d <= 100:
            raise ValueError(f"duty {d} out of range 0..100")
    for (t0, d0), (t1, d1) in zip(pts, pts[1:]):
        if t1 <= t0:
            raise ValueError("temperatures must strictly increase")
        if d1 < d0:
            raise ValueError("duty must not decrease as temperature rises")
    return pts


class Curve:
    def __init__(self, points: Iterable[Sequence[float]]):
        self.points = validate_points(points)

    def duty(self, temp: float) -> float:
        pts = self.points
        if temp <= pts[0][0]:
            return pts[0][1]
        if temp >= pts[-1][0]:
            return pts[-1][1]
        for (t0, d0), (t1, d1) in zip(pts, pts[1:]):
            if t0 <= temp <= t1:
                return d0 + (d1 - d0) * (temp - t0) / (t1 - t0)
        return pts[-1][1]


class Controller:
    """Turns temperatures into a stable duty value.

    - Hysteresis: the duty only goes down once the temperature has fallen
      `hysteresis` °C below the temperature that set the current duty.
    - Ramp: rises jump straight to the target; falls are limited to
      `ramp_down` percent per tick.
    - Floor: duty never drops below `floor_pct` while temp >= `floor_temp`.
    """

    def __init__(self, curve: Curve, hysteresis: float = 4.0, ramp_down: float = 5.0,
                 floor_pct: float = 20.0, floor_temp: float = 60.0):
        self.curve = curve
        self.hysteresis = hysteresis
        self.ramp_down = ramp_down
        self.floor_pct = floor_pct
        self.floor_temp = floor_temp
        self.current: Optional[float] = None
        self._peak_temp: float = -1.0

    def step(self, temp: float) -> int:
        target = self.curve.duty(temp)
        if self.current is None or target >= self.current:
            if self.current is None or target > self.current:
                self._peak_temp = temp
            self.current = target
        elif temp <= self._peak_temp - self.hysteresis:
            # Cooled enough: allow falling, limited by ramp.
            self.current = max(target, self.current - self.ramp_down)
            if self.current == target:
                self._peak_temp = temp
        if temp >= self.floor_temp:
            self.current = max(self.current, self.floor_pct)
        return int(round(self.current))
