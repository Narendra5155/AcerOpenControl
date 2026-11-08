"""Keyboard and System pages for the AcerFan window."""

from __future__ import annotations

from typing import Callable, Optional

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
from gi.repository import Adw, Gdk, GLib, Gtk  # noqa: E402

from ..client import request  # noqa: E402

SWATCHES = [("White", "ffffff"), ("Red", "ff0000"), ("Orange", "ff6000"), ("Yellow", "ffcc00"),
            ("Green", "00ff00"), ("Cyan", "00ffff"), ("Blue", "0000ff"), ("Purple", "8000ff"),
            ("Pink", "ff2080")]

PAGE_CSS = "".join(
    f".swatch-{h} {{ background: #{h}; min-width: 28px; min-height: 28px; border-radius: 99px;"
    f" padding: 0; box-shadow: inset 0 0 0 1px alpha(currentColor, .25); }}\n" for _, h in SWATCHES)

# Profile order in the UI (firmware index, see acerfan.features.PERF_PROFILES)
PROFILE_ORDER = [0, 1, 3, 2]


def _call(run_bg, req: dict, on_done: Callable[[dict], None], on_error: Callable[[str], None]):
    def done(res, err):
        if err is not None or not res.get("ok"):
            on_error(str(err) if err else res.get("error", "failed"))
        else:
            on_done(res)
    run_bg(lambda: request(req), done)


class KeyboardPage(Adw.PreferencesPage):
    def __init__(self, run_bg, on_error):
        super().__init__(title="Keyboard", icon_name="input-keyboard-symbolic")
        self.run_bg, self.on_error = run_bg, on_error
        self.state = {"r": 255, "g": 255, "b": 255, "brightness": 160}
        self._last_on_brightness = 160
        self._updating = False
        self._debounce: Optional[int] = None

        g = Adw.PreferencesGroup(title="Backlight",
                                 description="Single-zone RGB keyboard light, set in the laptop firmware.")
        self.add(g)

        self.on_row = Adw.SwitchRow(title="Keyboard light")
        self.on_row.connect("notify::active", self._on_toggle)
        g.add(self.on_row)

        row = Adw.ActionRow(title="Brightness")
        self.scale = Gtk.Scale.new_with_range(Gtk.Orientation.HORIZONTAL, 0, 255, 1)
        self.scale.set_draw_value(False)
        self.scale.set_hexpand(True)
        self.scale.set_size_request(200, -1)
        self.scale.set_valign(Gtk.Align.CENTER)
        self.scale.connect("value-changed", self._on_scale)
        row.add_suffix(self.scale)
        g.add(row)

        row = Adw.ActionRow(title="Colour", subtitle="Pick any colour")
        self.color_btn = Gtk.ColorDialogButton(dialog=Gtk.ColorDialog(with_alpha=False))
        self.color_btn.set_valign(Gtk.Align.CENTER)
        self.color_btn.connect("notify::rgba", self._on_color_picked)
        row.add_suffix(self.color_btn)
        g.add(row)

        sw_row = Adw.PreferencesRow(activatable=False)
        flow = Gtk.FlowBox(selection_mode=Gtk.SelectionMode.NONE, max_children_per_line=9,
                           min_children_per_line=5, column_spacing=10, row_spacing=10,
                           margin_top=12, margin_bottom=12, margin_start=12, margin_end=12,
                           homogeneous=True)
        for name, hx in SWATCHES:
            b = Gtk.Button(tooltip_text=name, halign=Gtk.Align.CENTER, valign=Gtk.Align.CENTER)
            b.add_css_class(f"swatch-{hx}")
            b.connect("clicked", self._on_swatch, hx)
            flow.append(b)
        sw_row.set_child(flow)
        g.add(sw_row)

        g2 = Adw.PreferencesGroup(title="Desktop integration")
        self.add(g2)
        g2.add(Adw.ActionRow(
            title="GNOME brightness slider",
            subtitle="The keyboard light is also exposed as a system LED, so GNOME's "
                     "Quick Settings and keyboard-brightness keys can control it."))

    # -- updates from daemon -----------------------------------------------------------
    def update(self, kbd: Optional[dict]):
        if not kbd or self._debounce is not None:
            return
        self.state = dict(kbd)
        self._updating = True
        self.scale.set_value(kbd["brightness"])
        self.on_row.set_active(kbd["brightness"] > 0)
        if kbd["brightness"] > 0:
            self._last_on_brightness = kbd["brightness"]
        rgba = Gdk.RGBA()
        rgba.parse(f"#{kbd['r']:02x}{kbd['g']:02x}{kbd['b']:02x}")
        if not rgba.equal(self.color_btn.get_rgba()):
            self.color_btn.set_rgba(rgba)
        self._updating = False

    # -- user actions --------------------------------------------------------------------
    def _send(self, **changes):
        self.state.update(changes)
        _call(self.run_bg, {"cmd": "set_kbd", **changes},
              lambda r: None, lambda e: self.on_error(f"Keyboard: {e}"))

    def _on_scale(self, scale):
        if self._updating:
            return
        if self._debounce is not None:
            GLib.source_remove(self._debounce)
        self._debounce = GLib.timeout_add(150, self._flush_brightness)

    def _flush_brightness(self):
        self._debounce = None
        v = int(self.scale.get_value())
        if v > 0:
            self._last_on_brightness = v
        self._updating = True
        self.on_row.set_active(v > 0)
        self._updating = False
        self._send(brightness=v)
        return False

    def _on_toggle(self, row, _p):
        if self._updating:
            return
        v = max(self._last_on_brightness, 32) if row.get_active() else 0
        self._updating = True
        self.scale.set_value(v)
        self._updating = False
        self._send(brightness=v)

    def _apply_colour(self, r, g, b):
        bright = self.state.get("brightness") or max(self._last_on_brightness, 32)
        self._send(r=r, g=g, b=b, brightness=bright)

    def _on_swatch(self, _btn, hx):
        rgba = Gdk.RGBA()
        rgba.parse(f"#{hx}")
        self._updating = True
        self.color_btn.set_rgba(rgba)
        self._updating = False
        self._apply_colour(int(hx[0:2], 16), int(hx[2:4], 16), int(hx[4:6], 16))

    def _on_color_picked(self, btn, _p):
        if self._updating:
            return
        c = btn.get_rgba()
        self._apply_colour(round(c.red * 255), round(c.green * 255), round(c.blue * 255))


class SystemPage(Adw.PreferencesPage):
    def __init__(self, run_bg, on_error, profiles: dict):
        super().__init__(title="System", icon_name="preferences-system-symbolic")
        self.run_bg, self.on_error = run_bg, on_error
        self._updating = False

        # -- performance profile --
        self.g_perf = Adw.PreferencesGroup(
            title="Performance profile",
            description="Sets CPU and GPU power limits in the firmware. Higher limits mean more "
                        "speed, more heat and more fan noise.")
        self.add(self.g_perf)
        self.profile_checks = {}
        group_leader = None
        for idx in PROFILE_ORDER:
            info = profiles.get(str(idx)) or {"label": str(idx), "desc": ""}
            row = Adw.ActionRow(title=info["label"], subtitle=info["desc"], activatable=True)
            chk = Gtk.CheckButton(valign=Gtk.Align.CENTER)
            if group_leader is None:
                group_leader = chk
            else:
                chk.set_group(group_leader)
            chk.connect("toggled", self._on_profile, idx)
            row.add_prefix(chk)
            row.set_activatable_widget(chk)
            self.g_perf.add(row)
            self.profile_checks[idx] = chk

        # -- battery --
        self.g_batt = Adw.PreferencesGroup(
            title="Battery",
            description="Stopping the charge early keeps the battery healthier if the laptop "
                        "is plugged in most of the time.")
        self.add(self.g_batt)
        self.limit_row = Adw.SwitchRow(title="Limit charging")
        self.limit_row.connect("notify::active", self._on_charge_changed)
        self.g_batt.add(self.limit_row)
        self.stops = [60, 70, 80, 90]
        self.stop_row = Adw.ComboRow(title="Stop charging at",
                                     model=Gtk.StringList.new([f"{s}%" for s in self.stops]))
        self.stop_row.set_selected(2)
        self.stop_row.connect("notify::selected", self._on_charge_changed)
        self.g_batt.add(self.stop_row)
        self.batt_status = Adw.ActionRow(title="Firmware setting", subtitle="–")
        self.g_batt.add(self.batt_status)

        # -- privacy --
        self.g_priv = Adw.PreferencesGroup(title="Privacy")
        self.add(self.g_priv)
        self.cam_row = Adw.SwitchRow(title="Webcam",
                                     subtitle="Off cuts the camera at the firmware level; it "
                                              "disappears from the system entirely.")
        self.cam_row.connect("notify::active", self._on_webcam)
        self.g_priv.add(self.cam_row)

    def update(self, f: dict):
        av = f.get("available") or {}
        self.g_perf.set_visible(bool(av.get("perf_profile")))
        self.g_batt.set_visible(bool(av.get("battery_limit")))
        self.g_priv.set_visible(bool(av.get("webcam")))
        self._updating = True
        p = f.get("perf_profile")
        if p in self.profile_checks and not self.profile_checks[p].get_active():
            self.profile_checks[p].set_active(True)
        c = f.get("charge")
        if c:
            self.limit_row.set_active(c["enabled"])
            if c["stop"] in self.stops:
                self.stop_row.set_selected(self.stops.index(c["stop"]))
            self.stop_row.set_sensitive(c["enabled"])
            self.batt_status.set_subtitle(
                f"Stops at {c['stop']}%, resumes below {c['start']}%" if c["enabled"]
                else "No limit: charges to 100%")
        if "webcam" in f:
            self.cam_row.set_active(bool(f["webcam"]))
        self._updating = False

    def _on_profile(self, chk, idx):
        if self._updating or not chk.get_active():
            return
        _call(self.run_bg, {"cmd": "set_perf_profile", "profile": idx},
              lambda r: None, lambda e: self.on_error(f"Profile: {e}"))

    def _on_charge_changed(self, *_):
        if self._updating:
            return
        enabled = self.limit_row.get_active()
        self.stop_row.set_sensitive(enabled)
        stop = self.stops[self.stop_row.get_selected()]
        req = {"cmd": "set_charge_limit", "enabled": enabled, "stop": stop if enabled else 100,
               "start": stop - 5 if enabled else 95}
        _call(self.run_bg, req, lambda r: None, lambda e: self.on_error(f"Battery: {e}"))

    def _on_webcam(self, row, _p):
        if self._updating:
            return
        _call(self.run_bg, {"cmd": "set_webcam", "on": row.get_active()},
              lambda r: None, lambda e: self.on_error(f"Webcam: {e}"))
