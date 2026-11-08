"""AcerFan GTK4 / libadwaita window. Talks to acerfand over its Unix socket."""

from __future__ import annotations

import sys
import threading
import time
from typing import Callable, Optional

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
from gi.repository import Adw, Gdk, Gio, GLib, Gtk  # noqa: E402

from .. import __version__  # noqa: E402
from ..client import DaemonUnavailable, request  # noqa: E402
from ..curve import PROFILES  # noqa: E402
from ..features import PERF_PROFILES  # noqa: E402
from .curve_editor import CurveEditor  # noqa: E402
from .pages import PAGE_CSS, KeyboardPage, SystemPage  # noqa: E402

APP_ID = "io.github.acerfan.AcerFan"

MODES = [
    ("auto", "Auto", "The laptop's BIOS controls the fans itself. AcerFan only watches."),
    ("silent", "Silent", "Quiet curve: fans stay slow until the machine gets warm."),
    ("balanced", "Balanced", "Linux picks the speed from the hotter of CPU and GPU."),
    ("performance", "Performance", "Cool-first curve: fans ramp early and stay faster."),
    ("max", "Max", "Both fans at full speed."),
    ("custom", "Custom", "Your own curve. Drag the points below, then Apply."),
]
MODE_DESC = {k: d for k, _, d in MODES}

CSS = b"""
.stat-card { padding: 12px 14px; }
.stat-value { font-size: 1.6em; font-weight: 700; }
"""


def run_bg(fn: Callable, done: Optional[Callable] = None):
    """Run fn() off the UI thread; call done(result, error) back on it."""
    def worker():
        try:
            res, err = fn(), None
        except Exception as e:  # noqa: BLE001
            res, err = None, e
        if done:
            GLib.idle_add(done, res, err)
    threading.Thread(target=worker, daemon=True).start()


class StatCard(Gtk.Box):
    def __init__(self, title: str):
        super().__init__(orientation=Gtk.Orientation.VERTICAL, spacing=2)
        self.add_css_class("card")
        self.add_css_class("stat-card")
        self.set_hexpand(True)
        t = Gtk.Label(label=title, xalign=0)
        t.add_css_class("caption-heading")
        t.add_css_class("dim-label")
        self.value = Gtk.Label(label="–", xalign=0)
        self.value.add_css_class("stat-value")
        self.value.add_css_class("numeric")
        self.sub = Gtk.Label(label=" ", xalign=0)
        self.sub.add_css_class("caption")
        self.sub.add_css_class("dim-label")
        for w in (t, self.value, self.sub):
            self.append(w)

    def set(self, value: str, sub: str = " "):
        self.value.set_label(value)
        self.sub.set_label(sub)


class MainWindow(Adw.ApplicationWindow):
    def __init__(self, app):
        super().__init__(application=app, title="AcerFan")
        self.set_default_size(620, 760)
        self.cfg: Optional[dict] = None
        self.mode: Optional[str] = None
        self.draft: Optional[list] = None
        self._updating = False
        self._alive = True

        view = Adw.ToolbarView()
        header = Adw.HeaderBar()
        menu = Gio.Menu()
        menu.append("About AcerFan", "app.about")
        header.pack_end(Gtk.MenuButton(icon_name="open-menu-symbolic", menu_model=menu))
        self.stack = Adw.ViewStack()
        header.set_title_widget(Adw.ViewSwitcher(stack=self.stack,
                                                 policy=Adw.ViewSwitcherPolicy.WIDE))
        view.add_top_bar(header)
        self.banner = Adw.Banner()
        view.add_top_bar(self.banner)

        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=22,
                      margin_top=18, margin_bottom=24, margin_start=16, margin_end=16)

        # -- live stats --------------------------------------------------------
        grid = Gtk.Grid(column_spacing=10, row_spacing=10, column_homogeneous=True)
        self.card_cpu = StatCard("CPU")
        self.card_gpu = StatCard("GPU")
        self.card_f1 = StatCard("FAN 1 · CPU")
        self.card_f2 = StatCard("FAN 2 · GPU")
        grid.attach(self.card_cpu, 0, 0, 1, 1)
        grid.attach(self.card_gpu, 1, 0, 1, 1)
        grid.attach(self.card_f1, 0, 1, 1, 1)
        grid.attach(self.card_f2, 1, 1, 1, 1)
        box.append(grid)

        # -- mode ----------------------------------------------------------------
        mode_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
        lbl = Gtk.Label(label="Fan mode", xalign=0)
        lbl.add_css_class("heading")
        mode_box.append(lbl)
        self.toggles = Adw.ToggleGroup()
        self.toggles.add_css_class("round")
        for key, label, _ in MODES:
            self.toggles.add(Adw.Toggle(name=key, label=label))
        self.toggles.connect("notify::active-name", self._on_mode_toggled)
        sw = Gtk.ScrolledWindow(vscrollbar_policy=Gtk.PolicyType.NEVER,
                                hscrollbar_policy=Gtk.PolicyType.AUTOMATIC)
        sw.set_child(self.toggles)
        sw.set_propagate_natural_height(True)
        mode_box.append(sw)
        self.mode_desc = Gtk.Label(xalign=0, wrap=True)
        self.mode_desc.add_css_class("dim-label")
        mode_box.append(self.mode_desc)
        self.control_lbl = Gtk.Label(xalign=0)
        self.control_lbl.add_css_class("caption")
        mode_box.append(self.control_lbl)
        box.append(mode_box)

        # -- curve -----------------------------------------------------------------
        curve_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
        lbl = Gtk.Label(label="Fan curve", xalign=0)
        lbl.add_css_class("heading")
        curve_box.append(lbl)
        self.curve_sub = Gtk.Label(xalign=0, wrap=True)
        self.curve_sub.add_css_class("dim-label")
        self.curve_sub.add_css_class("caption")
        curve_box.append(self.curve_sub)
        frame = Gtk.Box()
        frame.add_css_class("card")
        self.editor = CurveEditor(on_changed=self._on_curve_dragged)
        self.editor.set_margin_top(8)
        self.editor.set_margin_bottom(4)
        self.editor.set_margin_end(4)
        frame.append(self.editor)
        curve_box.append(frame)
        btns = Gtk.Box(spacing=8, halign=Gtk.Align.END)
        self.btn_reset = Gtk.Button(label="Discard")
        self.btn_reset.connect("clicked", self._on_discard)
        self.btn_apply = Gtk.Button(label="Apply as Custom")
        self.btn_apply.add_css_class("suggested-action")
        self.btn_apply.connect("clicked", self._on_apply)
        btns.append(self.btn_reset)
        btns.append(self.btn_apply)
        self.curve_btns = btns
        btns.set_visible(False)
        curve_box.append(btns)
        box.append(curve_box)

        clamp = Adw.Clamp(maximum_size=720, child=box)
        scroller = Gtk.ScrolledWindow(hscrollbar_policy=Gtk.PolicyType.NEVER, child=clamp)
        self.stack.add_titled_with_icon(scroller, "fans", "Fans", "weather-windy-symbolic")
        self.kbd_page = KeyboardPage(run_bg, self._toast)
        profiles = {str(i): {"key": k, "label": l, "desc": d}
                    for i, (k, l, d) in PERF_PROFILES.items()}
        self.sys_page = SystemPage(run_bg, self._toast, profiles)
        self.kbd_stack_page = self.stack.add_titled_with_icon(
            self.kbd_page, "keyboard", "Keyboard", "input-keyboard-symbolic")
        self.sys_stack_page = self.stack.add_titled_with_icon(
            self.sys_page, "system", "System", "computer-symbolic")
        view.set_content(self.stack)
        self.set_content(view)

        self.connect("close-request", self._on_close)
        self._load_config()
        threading.Thread(target=self._poll_loop, daemon=True).start()

    # -- daemon I/O ---------------------------------------------------------------
    def _poll_loop(self):
        while self._alive:
            try:
                st = request({"cmd": "status"}, timeout=4)
                GLib.idle_add(self._on_status, st)
            except DaemonUnavailable as e:
                GLib.idle_add(self._on_unavailable, str(e))
            except Exception as e:  # noqa: BLE001
                GLib.idle_add(self._on_unavailable, f"error talking to acerfand: {e}")
            time.sleep(1.0)

    def _load_config(self):
        def done(res, err):
            if err is None and res.get("ok"):
                self.cfg = res["config"]
                self._refresh_curve()
        run_bg(lambda: request({"cmd": "get_config"}), done)

    def _on_unavailable(self, msg: str):
        self.banner.set_title(msg)
        self.banner.set_revealed(True)
        self.set_controls_sensitive(False)

    def set_controls_sensitive(self, on: bool):
        self.toggles.set_sensitive(on)
        self.btn_apply.set_sensitive(on)

    def _on_status(self, st: dict):
        if not st.get("ok", True):
            return self._on_unavailable(st.get("error", "daemon error"))
        self.set_controls_sensitive(True)
        if st.get("emergency"):
            self.banner.set_title("Temperature limit reached: fans forced to 100%")
            self.banner.set_revealed(True)
        elif st.get("fault"):
            self.banner.set_title(f"{st['fault']}. Fans handed back to the BIOS.")
            self.banner.set_revealed(True)
        else:
            self.banner.set_revealed(False)

        cpu, gpu = st.get("cpu"), st.get("gpu")
        self.card_cpu.set("–" if cpu is None else f"{cpu:.0f}°C", "package")
        if st.get("gpu_asleep"):
            self.card_gpu.set("Idle", "dGPU asleep")
        else:
            self.card_gpu.set("–" if gpu is None else f"{gpu:.0f}°C", "RTX 3050")
        fans = st.get("fans") or {}
        if fans:
            self.card_f1.set(f"{fans.get('rpm1', 0)} rpm", f"{fans.get('duty1', 0)}% duty")
            self.card_f2.set(f"{fans.get('rpm2', 0)} rpm", f"{fans.get('duty2', 0)}% duty")

        bios = bool(st.get("bios_control"))
        self.control_lbl.set_label(
            "Fans controlled by: BIOS" if bios else f"Fans controlled by: AcerFan · target {st.get('target')}%")

        mode = st.get("mode")
        if mode != self.mode:
            self.mode = mode
            self._updating = True
            self.toggles.set_active_name(mode)
            self._updating = False
            self.mode_desc.set_label(MODE_DESC.get(mode, ""))
            self._refresh_curve()

        feats = st.get("features") or {}
        if feats:
            av = feats.get("available") or {}
            self.kbd_stack_page.set_visible(bool(av.get("keyboard")))
            self.kbd_page.update(feats.get("kbd"))
            self.sys_page.update(feats)

        temps = [t for t in (cpu, gpu) if t is not None]
        self.editor.set_live(max(temps) if temps else None, st.get("target"), bios)
        return False

    # -- mode / curve -------------------------------------------------------------
    def _curve_for_mode(self):
        if self.mode in PROFILES:
            return PROFILES[self.mode]
        if self.cfg:
            return self.cfg["custom_curve"]
        return PROFILES["balanced"]

    def _refresh_curve(self):
        if self.draft is not None:
            self.editor.set_points(self.draft)
            return
        self.editor.set_points(self._curve_for_mode())
        sub = {
            "auto": "Not used while the BIOS is in control. Showing your custom curve; drag to edit it.",
            "max": "Not used in Max mode. Showing your custom curve; drag to edit it.",
            "custom": "Your custom curve. Drag a point to change it.",
        }.get(self.mode, "Preset curve. Drag a point to start a custom curve from it.")
        self.curve_sub.set_label(sub + " The x-axis is the hotter of CPU and GPU.")

    def _on_mode_toggled(self, group, _pspec):
        if self._updating:
            return
        mode = group.get_active_name()
        if not mode or mode == self.mode:
            return
        self.mode_desc.set_label(MODE_DESC.get(mode, ""))

        def done(res, err):
            if err or not res.get("ok"):
                self._toast(f"Could not set mode: {err or res.get('error')}")
                self._updating = True
                if self.mode:
                    self.toggles.set_active_name(self.mode)
                self._updating = False
            else:
                self.mode = mode
                self.draft = None
                self.curve_btns.set_visible(False)
                self._refresh_curve()
        run_bg(lambda: request({"cmd": "set_mode", "mode": mode}), done)

    def _on_curve_dragged(self, points):
        self.draft = points
        self.curve_btns.set_visible(True)
        self.curve_sub.set_label("Edited. Apply to save it as your Custom curve and switch to it.")

    def _on_discard(self, _btn):
        self.draft = None
        self.curve_btns.set_visible(False)
        self._refresh_curve()

    def _on_apply(self, _btn):
        pts = self.draft
        if pts is None:
            return

        def done(res, err):
            if err or not res.get("ok"):
                self._toast(f"Could not apply curve: {err or res.get('error')}")
                return
            self.draft = None
            self.curve_btns.set_visible(False)
            if self.cfg is not None:
                self.cfg["custom_curve"] = pts
            self._load_config()
        run_bg(lambda: request({"cmd": "set_curve", "points": pts, "activate": True}), done)

    def _toast(self, msg):
        self.banner.set_title(msg)
        self.banner.set_revealed(True)

    def _on_close(self, *_):
        self._alive = False
        return False


class AcerFanApp(Adw.Application):
    def __init__(self):
        super().__init__(application_id=APP_ID, flags=Gio.ApplicationFlags.DEFAULT_FLAGS)
        about = Gio.SimpleAction.new("about", None)
        about.connect("activate", self._about)
        self.add_action(about)

    def do_startup(self):
        Adw.Application.do_startup(self)
        css = Gtk.CssProvider()
        data = CSS + PAGE_CSS.encode()
        css.load_from_data(data, len(data))
        Gtk.StyleContext.add_provider_for_display(
            Gdk.Display.get_default(), css, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)

    def do_activate(self):
        win = self.props.active_window or MainWindow(self)
        win.present()

    def _about(self, *_):
        Adw.AboutDialog(application_name="AcerFan", version=__version__,
                        application_icon="acerfan",
                        comments="Fan control for the Acer Aspire 7 A715-79G (Clevo NP50RN)",
                        license_type=Gtk.License.GPL_3_0).present(self.props.active_window)


def main() -> int:
    return AcerFanApp().run(sys.argv[:1])


if __name__ == "__main__":
    raise SystemExit(main())
