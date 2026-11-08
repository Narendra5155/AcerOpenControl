"""Draggable temperature → duty curve widget (GTK4 DrawingArea + cairo)."""

from __future__ import annotations

import math
from typing import Callable, List, Optional

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
from gi.repository import Adw, Gtk  # noqa: E402

T_MIN, T_MAX = 30.0, 100.0
D_MIN, D_MAX = 0.0, 100.0
PAD_L, PAD_R, PAD_T, PAD_B = 44, 16, 14, 30
HIT_RADIUS = 14


class CurveEditor(Gtk.DrawingArea):
    def __init__(self, on_changed: Optional[Callable[[List[List[float]]], None]] = None):
        super().__init__()
        self.points: List[List[float]] = []
        self.on_changed = on_changed
        self.current_temp: Optional[float] = None
        self.current_duty: Optional[float] = None
        self.bios = False
        self._drag_idx: Optional[int] = None
        self._drag_origin = (0.0, 0.0)
        self.set_content_height(260)
        self.set_hexpand(True)
        self.set_draw_func(self._draw)

        drag = Gtk.GestureDrag()
        drag.connect("drag-begin", self._on_begin)
        drag.connect("drag-update", self._on_update)
        drag.connect("drag-end", self._on_end)
        self.add_controller(drag)

    # -- public ----------------------------------------------------------------
    def set_points(self, points):
        if self._drag_idx is None:
            self.points = [list(map(float, p)) for p in points]
            self.queue_draw()

    def set_live(self, temp, duty, bios: bool):
        self.current_temp, self.current_duty, self.bios = temp, duty, bios
        self.queue_draw()

    # -- geometry --------------------------------------------------------------
    def _plot(self):
        w, h = self.get_width(), self.get_height()
        return PAD_L, PAD_T, max(1, w - PAD_L - PAD_R), max(1, h - PAD_T - PAD_B)

    def _to_px(self, t, d):
        x0, y0, pw, ph = self._plot()
        return (x0 + (t - T_MIN) / (T_MAX - T_MIN) * pw,
                y0 + ph - (d - D_MIN) / (D_MAX - D_MIN) * ph)

    def _from_px(self, x, y):
        x0, y0, pw, ph = self._plot()
        return (T_MIN + (x - x0) / pw * (T_MAX - T_MIN),
                D_MIN + (y0 + ph - y) / ph * (D_MAX - D_MIN))

    # -- drawing ---------------------------------------------------------------
    def _draw(self, area, cr, w, h):
        fg = self.get_color()
        try:
            acc = Adw.StyleManager.get_default().get_accent_color_rgba()
        except AttributeError:
            acc = fg
        x0, y0, pw, ph = self._plot()

        # grid + labels
        cr.set_line_width(1)
        cr.select_font_face("Sans")
        cr.set_font_size(10)
        for t in range(int(T_MIN), int(T_MAX) + 1, 10):
            x, _ = self._to_px(t, 0)
            cr.set_source_rgba(fg.red, fg.green, fg.blue, 0.10)
            cr.move_to(x + .5, y0); cr.line_to(x + .5, y0 + ph); cr.stroke()
            cr.set_source_rgba(fg.red, fg.green, fg.blue, 0.6)
            label = f"{t}°"
            ext = cr.text_extents(label)
            cr.move_to(x - ext.width / 2, y0 + ph + 16); cr.show_text(label)
        for d in range(0, 101, 25):
            _, y = self._to_px(T_MIN, d)
            cr.set_source_rgba(fg.red, fg.green, fg.blue, 0.10)
            cr.move_to(x0, y + .5); cr.line_to(x0 + pw, y + .5); cr.stroke()
            cr.set_source_rgba(fg.red, fg.green, fg.blue, 0.6)
            label = f"{d}%"
            ext = cr.text_extents(label)
            cr.move_to(x0 - ext.width - 6, y + 4); cr.show_text(label)

        if not self.points:
            return
        alpha = 0.35 if self.bios else 1.0
        pts = [self._to_px(t, d) for t, d in self.points]
        ext_pts = [self._to_px(T_MIN, self.points[0][1])] + pts + \
                  [self._to_px(T_MAX, self.points[-1][1])]

        # filled area
        cr.move_to(ext_pts[0][0], y0 + ph)
        for x, y in ext_pts:
            cr.line_to(x, y)
        cr.line_to(ext_pts[-1][0], y0 + ph)
        cr.close_path()
        cr.set_source_rgba(acc.red, acc.green, acc.blue, 0.12 * alpha)
        cr.fill()
        # line
        cr.set_line_width(2.5)
        cr.set_source_rgba(acc.red, acc.green, acc.blue, alpha)
        cr.move_to(*ext_pts[0])
        for p in ext_pts[1:]:
            cr.line_to(*p)
        cr.stroke()
        # handles
        for i, (x, y) in enumerate(pts):
            cr.arc(x, y, 7 if i == self._drag_idx else 5.5, 0, 2 * math.pi)
            cr.set_source_rgba(acc.red, acc.green, acc.blue, alpha)
            cr.fill_preserve()
            cr.set_source_rgba(1, 1, 1, 0.9 * alpha)
            cr.set_line_width(1.5)
            cr.stroke()

        # live marker
        if self.current_temp is not None:
            t = min(max(self.current_temp, T_MIN), T_MAX)
            x, _ = self._to_px(t, 0)
            cr.set_source_rgba(fg.red, fg.green, fg.blue, 0.5)
            cr.set_dash([4, 4])
            cr.set_line_width(1)
            cr.move_to(x + .5, y0); cr.line_to(x + .5, y0 + ph); cr.stroke()
            cr.set_dash([])
            if self.current_duty is not None and not self.bios:
                _, y = self._to_px(t, self.current_duty)
                cr.arc(x, y, 4, 0, 2 * math.pi)
                cr.set_source_rgba(fg.red, fg.green, fg.blue, 0.9)
                cr.fill()
            if self._drag_idx is None:
                label = f"{self.current_temp:.0f}°C"
                cr.set_source_rgba(fg.red, fg.green, fg.blue, 0.8)
                cr.move_to(min(x + 5, x0 + pw - 34), y0 + 11)
                cr.show_text(label)

        # tooltip-ish readout while dragging
        if self._drag_idx is not None:
            t, d = self.points[self._drag_idx]
            x, y = pts[self._drag_idx]
            label = f"{t:.0f}°C → {d:.0f}%"
            cr.set_source_rgba(fg.red, fg.green, fg.blue, 0.9)
            cr.set_font_size(11)
            cr.move_to(min(x + 10, x0 + pw - 90), max(y - 10, y0 + 12))
            cr.show_text(label)

    # -- dragging ---------------------------------------------------------------
    def _on_begin(self, gesture, x, y):
        best, best_d = None, HIT_RADIUS
        for i, (t, d) in enumerate(self.points):
            px, py = self._to_px(t, d)
            dist = math.hypot(px - x, py - y)
            if dist <= best_d:
                best, best_d = i, dist
        self._drag_idx = best
        self._drag_origin = (x, y)
        if best is None:
            gesture.set_state(Gtk.EventSequenceState.DENIED)

    def _on_update(self, gesture, dx, dy):
        i = self._drag_idx
        if i is None:
            return
        t, d = self._from_px(self._drag_origin[0] + dx, self._drag_origin[1] + dy)
        lo_t = self.points[i - 1][0] + 1 if i > 0 else T_MIN
        hi_t = self.points[i + 1][0] - 1 if i < len(self.points) - 1 else T_MAX
        lo_d = self.points[i - 1][1] if i > 0 else D_MIN
        hi_d = self.points[i + 1][1] if i < len(self.points) - 1 else D_MAX
        self.points[i] = [round(min(max(t, lo_t), hi_t)), round(min(max(d, lo_d), hi_d))]
        self.queue_draw()

    def _on_end(self, gesture, dx, dy):
        if self._drag_idx is not None:
            self._drag_idx = None
            self.queue_draw()
            if self.on_changed:
                self.on_changed([list(p) for p in self.points])
