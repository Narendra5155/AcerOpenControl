"""AcerOpenControl GTK4 / libadwaita window. Talks to aocd over its Unix socket."""

from __future__ import annotations

import sys
import threading
import time
from typing import Optional
from collections.abc import Callable

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
from gi.repository import Adw, Gdk, Gio, GLib, Gtk  # noqa: E402

from . import APP_ID, APP_NAME, __version__  # noqa: E402
from .client import DaemonUnavailable, describe_error, request  # noqa: E402
from .compat import ModeSwitcher  # noqa: E402
from .curve_editor import CurveEditor  # noqa: E402
from .pages import PAGE_CSS, KeyboardPage, SystemPage  # noqa: E402

MODES = [
    ("auto", "Auto", "The laptop's BIOS controls the fans itself. AcerOpenControl only watches."),
    ("silent", "Silent", "Quiet curve: fans stay slow until the machine gets warm."),
    ("balanced", "Balanced", "Linux picks the speed from the hotter of CPU and GPU."),
    ("performance", "Performance", "Cool-first curve: fans ramp early and stay faster."),
    ("max", "Max", "All fans at full speed."),
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
        super().__init__(application=app, title=APP_NAME)
        self.set_default_size(620, 760)
        self.cfg: Optional[dict] = None
        self.presets: dict = {}
        self.model: Optional[dict] = None
        self.mode: Optional[str] = None
        self.draft: Optional[list] = None
        self.read_only = False
        self._ext_shown = False
        self._ext_was_curve = False
        self._alive = True

        view = Adw.ToolbarView()
        header = Adw.HeaderBar()
        menu = Gio.Menu()
        menu.append(f"About {APP_NAME}", "app.about")
        header.pack_end(Gtk.MenuButton(icon_name="open-menu-symbolic", menu_model=menu))
        self.stack = Adw.ViewStack()
        header.set_title_widget(Adw.ViewSwitcher(stack=self.stack, policy=Adw.ViewSwitcherPolicy.WIDE))
        view.add_top_bar(header)
        self.banner = Adw.Banner()
        view.add_top_bar(self.banner)

        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=22,
                      margin_top=18, margin_bottom=24, margin_start=16, margin_end=16)

        # -- live stats ---------------------------------------------------------
        self.grid = Gtk.Grid(column_spacing=10, row_spacing=10, column_homogeneous=True)
        self.card_cpu = StatCard("CPU")
        self.card_gpu = StatCard("GPU")
        self.grid.attach(self.card_cpu, 0, 0, 1, 1)
        self.grid.attach(self.card_gpu, 1, 0, 1, 1)
        self.fan_cards: list = []
        self._set_fan_labels(["CPU", "GPU"])
        box.append(self.grid)

        # -- mode -------------------------------------------------------------------
        self.mode_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
        lbl = Gtk.Label(label="Fan mode", xalign=0)
        lbl.add_css_class("heading")
        self.mode_box.append(lbl)
        self.switcher = ModeSwitcher([(k, label) for k, label, _ in MODES], self._on_mode_selected)
        sw = Gtk.ScrolledWindow(vscrollbar_policy=Gtk.PolicyType.NEVER, hscrollbar_policy=Gtk.PolicyType.AUTOMATIC)
        sw.set_child(self.switcher.widget)
        sw.set_propagate_natural_height(True)
        self.mode_box.append(sw)
        self.mode_desc = Gtk.Label(xalign=0, wrap=True)
        self.mode_desc.add_css_class("dim-label")
        self.mode_box.append(self.mode_desc)
        self.control_lbl = Gtk.Label(xalign=0)
        self.control_lbl.add_css_class("caption")
        self.mode_box.append(self.control_lbl)
        box.append(self.mode_box)

        # -- curve ------------------------------------------------------------------
        self.curve_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
        lbl = Gtk.Label(label="Fan curve", xalign=0)
        lbl.add_css_class("heading")
        self.curve_box.append(lbl)
        self.curve_sub = Gtk.Label(xalign=0, wrap=True)
        self.curve_sub.add_css_class("dim-label")
        self.curve_sub.add_css_class("caption")
        self.curve_box.append(self.curve_sub)
        frame = Gtk.Box()
        frame.add_css_class("card")
        self.editor = CurveEditor(on_changed=self._on_curve_dragged)
        self.editor.set_margin_top(8)
        self.editor.set_margin_bottom(4)
        self.editor.set_margin_end(4)
        frame.append(self.editor)
        self.curve_box.append(frame)
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
        self.curve_box.append(btns)
        box.append(self.curve_box)

        clamp = Adw.Clamp(maximum_size=720, child=box)
        scroller = Gtk.ScrolledWindow(hscrollbar_policy=Gtk.PolicyType.NEVER, child=clamp)
        self.stack.add_titled_with_icon(scroller, "fans", "Fans", "weather-windy-symbolic")
        self.kbd_page = KeyboardPage(run_bg, self._toast)
        self.sys_page = SystemPage(run_bg, self._toast)
        self.kbd_stack_page = self.stack.add_titled_with_icon(
            self.kbd_page, "keyboard", "Keyboard", "input-keyboard-symbolic")
        self.stack.add_titled_with_icon(self.sys_page, "system", "System", "computer-symbolic")
        self.kbd_stack_page.set_visible(False)
        view.set_content(self.stack)
        self.set_content(view)

        self.connect("close-request", self._on_close)
        self._load_model()
        self._load_config()
        threading.Thread(target=self._poll_loop, daemon=True).start()

    # -- layout helpers -------------------------------------------------------------
    def _set_fan_labels(self, labels):
        for c in self.fan_cards:
            self.grid.remove(c)
        self.fan_cards = []
        for i, label in enumerate(labels):
            card = StatCard(f"FAN {i + 1} · {label.upper()}")
            self.grid.attach(card, i % 2, 1 + i // 2, 1, 1)
            self.fan_cards.append(card)

    # -- daemon I/O -------------------------------------------------------------------
    def _poll_loop(self):
        while self._alive:
            try:
                st = request({"cmd": "status"}, timeout=4)
                GLib.idle_add(self._on_status, st)
            except DaemonUnavailable as e:
                GLib.idle_add(self._on_unavailable, str(e))
            except Exception as e:  # noqa: BLE001
                GLib.idle_add(self._on_unavailable, f"Error talking to aocd: {e}")
            time.sleep(1.0)

    def _load_model(self):
        def done(res, err):
            if err is not None or not res.get("ok"):
                GLib.timeout_add_seconds(2, lambda: (self._load_model(), False)[1])
                return
            self.model = res
            caps = res.get("capabilities") or {}
            labels = caps.get("fan_labels") or []
            self._set_fan_labels(labels)
            has_fans = bool(labels)
            self.mode_box.set_visible(has_fans)
            self.curve_box.set_visible(has_fans)
            self.kbd_stack_page.set_visible(bool(caps.get("keyboard")))
            self.kbd_page.configure(caps)
            self.sys_page.configure(res)
        run_bg(lambda: request({"cmd": "get_model"}), done)

    def _load_config(self):
        def done(res, err):
            if err is None and res.get("ok"):
                self.cfg = res["config"]
                self.presets = res.get("presets") or {}
                self._refresh_curve()
        run_bg(lambda: request({"cmd": "get_config"}), done)

    def _on_unavailable(self, msg: str):
        self.banner.set_title(msg)
        self.banner.set_revealed(True)
        self.set_controls_sensitive(False)

    def set_controls_sensitive(self, on: bool):
        self.switcher.set_sensitive(on)
        self.btn_apply.set_sensitive(on)

    def _on_status(self, st: dict):
        if not st.get("ok", True):
            return self._on_unavailable(st.get("error", "daemon error"))
        if self.model is None:
            self._load_model()
        self.read_only = bool(st.get("read_only"))
        self.set_controls_sensitive(not self.read_only)
        if self.read_only:
            self.banner.set_title("No model profile matches this laptop: monitoring only.")
            self.banner.set_revealed(True)
        elif st.get("emergency"):
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
            self.card_gpu.set("–" if gpu is None else f"{gpu:.0f}°C", "dGPU")
        for card, fan in zip(self.fan_cards, st.get("fans") or []):
            card.set(f"{fan.get('rpm', 0)} rpm", f"{fan.get('duty', 0)}% duty")

        bios = bool(st.get("bios_control"))
        ext = st.get("external")
        if ext:
            parts = [f"fan {i + 1}: " + ("BIOS" if d is None else f"{d}%") for i, d in enumerate(ext.get("fans") or [])]
            self.control_lbl.set_label(f"Fans controlled by: {ext.get('holder')} · " + ", ".join(parts))
            self.mode_desc.set_label(f"{ext.get('holder')} is controlling the fans. Fan modes come back "
                                     f"when it releases them (reset the fans to Default there).")
        else:
            self.control_lbl.set_label(
                "Fans controlled by: BIOS" if bios else f"Fans controlled by: {APP_NAME} · target {st.get('target')}%")
            if self._ext_shown:
                self.mode_desc.set_label(MODE_DESC.get(self.mode or "", ""))
        self._ext_shown = bool(ext)
        self.switcher.set_sensitive(not self.read_only and not ext)
        self.btn_apply.set_sensitive(not self.read_only and not ext)

        mode = st.get("mode")
        if mode != self.mode:
            self.mode = mode
            self.switcher.set_active(mode)
            self.mode_desc.set_label(MODE_DESC.get(mode or "", ""))
            self._refresh_curve()

        feats = st.get("features") or {}
        if feats:
            self.kbd_page.update(feats.get("kbd"))
            self.sys_page.update(feats)

        temps = [t for t in (cpu, gpu) if t is not None]
        if ext:
            # The curve isn't used while another program holds the fans: dim it.
            self.editor.set_live(max(temps) if temps else None, None, True)
            self.curve_sub.set_label(f"Not used while {ext.get('holder')} controls the fans.")
        else:
            self.editor.set_live(max(temps) if temps else None, st.get("target"), bios)
            if self._ext_was_curve:
                self._refresh_curve()
        self._ext_was_curve = bool(ext)
        return False

    # -- mode / curve -------------------------------------------------------------------
    def _curve_for_mode(self):
        if self.mode and self.mode in self.presets:
            return self.presets[self.mode]
        if self.cfg:
            return self.cfg["custom_curve"]
        return self.presets.get("balanced", [])

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

    def _on_mode_selected(self, mode: str):
        if not mode or mode == self.mode:
            return
        self.mode_desc.set_label(MODE_DESC.get(mode or "", ""))

        def done(res, err):
            if err or not res.get("ok"):
                self._toast(f"Could not set mode: {describe_error(res, err)}")
                self.switcher.set_active(self.mode)
                self.mode_desc.set_label(MODE_DESC.get(self.mode or "", ""))
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
                self._toast(f"Could not apply curve: {describe_error(res, err)}")
                return
            self.draft = None
            self.curve_btns.set_visible(False)
            self._load_config()
        run_bg(lambda: request({"cmd": "set_curve", "points": pts, "activate": True}), done)

    def _toast(self, msg):
        self.banner.set_title(msg)
        self.banner.set_revealed(True)

    def _on_close(self, *_):
        self._alive = False
        return False


class AocApp(Adw.Application):
    def __init__(self, app_id: str = APP_ID):
        super().__init__(application_id=app_id, flags=Gio.ApplicationFlags.DEFAULT_FLAGS)
        about = Gio.SimpleAction.new("about", None)
        about.connect("activate", self._about)
        self.add_action(about)

    def do_startup(self):
        Adw.Application.do_startup(self)
        css = Gtk.CssProvider()
        data = CSS + PAGE_CSS.encode()
        css.load_from_data(data, len(data))
        Gtk.StyleContext.add_provider_for_display(Gdk.Display.get_default(), css,
                                                  Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)

    def do_activate(self):
        win = self.props.active_window or MainWindow(self)
        win.present()

    def _about(self, *_):
        Adw.AboutDialog(application_name=APP_NAME, version=__version__, application_icon=APP_ID,
                        comments="Fan, keyboard and firmware control for Clevo-based Acer laptops.\n"
                                 "Unofficial; not affiliated with or endorsed by Acer Inc.",
                        website="https://github.com/Narendra5155/AcerOpenControl",
                        license_type=Gtk.License.GPL_3_0).present(self.props.active_window)


def main() -> int:
    return AocApp().run(sys.argv[:1])


if __name__ == "__main__":
    raise SystemExit(main())
