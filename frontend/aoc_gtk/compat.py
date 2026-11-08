"""Widgets that need a fallback on older libadwaita (Ubuntu 24.04 ships 1.5)."""

from __future__ import annotations

from typing import Callable, List, Optional, Tuple

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
from gi.repository import Adw, Gtk  # noqa: E402

HAS_TOGGLE_GROUP = hasattr(Adw, "ToggleGroup")


class ModeSwitcher:
    """A row of mutually exclusive buttons: Adw.ToggleGroup (libadwaita >= 1.7)
    or linked Gtk.ToggleButtons on older versions."""

    def __init__(self, items: List[Tuple[str, str]], on_changed: Callable[[str], None]):
        self._on_changed = on_changed
        self._quiet = False
        if HAS_TOGGLE_GROUP:
            self.widget = Adw.ToggleGroup()
            self.widget.add_css_class("round")
            for key, label in items:
                self.widget.add(Adw.Toggle(name=key, label=label))
            self.widget.connect("notify::active-name", self._on_group)
            self._buttons = {}
        else:
            self.widget = Gtk.Box(spacing=0)
            self.widget.add_css_class("linked")
            self._buttons = {}
            leader = None
            for key, label in items:
                b = Gtk.ToggleButton(label=label)
                if leader is None:
                    leader = b
                else:
                    b.set_group(leader)
                b.connect("toggled", self._on_button, key)
                self.widget.append(b)
                self._buttons[key] = b

    def set_active(self, name: Optional[str]):
        self._quiet = True
        try:
            if HAS_TOGGLE_GROUP:
                if name:
                    self.widget.set_active_name(name)
            elif name in self._buttons:
                self._buttons[name].set_active(True)
        finally:
            self._quiet = False

    def set_sensitive(self, on: bool):
        self.widget.set_sensitive(on)

    def _on_group(self, group, _pspec):
        if not self._quiet and group.get_active_name():
            self._on_changed(group.get_active_name())

    def _on_button(self, button, key):
        if not self._quiet and button.get_active():
            self._on_changed(key)
