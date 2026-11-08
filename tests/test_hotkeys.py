import struct
import unittest
from unittest import mock

from acerfan import config as C
from acerfan import hotkeys as H
from acerfan.daemon import FanService
from acerfan.features import Features
from tests.test_features import CAPS, FakeEC, Gpu


def genl_event(cls: bytes, bus: bytes, etype: int, data: int = 0) -> bytes:
    body = H.ACPI_EVENT_STRUCT.pack(cls, bus, etype, data)
    attr = struct.pack("HH", 4 + len(body), H.ACPI_GENL_ATTR_EVENT) + body
    return attr


class NetlinkParse(unittest.TestCase):
    def test_parse(self):
        ev = H.parse_acpi_event(genl_event(b"", b"ABBC0F6B-8EA1-1", 0xD0))
        self.assertEqual(ev, ("", "ABBC0F6B-8EA1-1", 0xD0, 0))
        self.assertTrue(H.is_hotkey_event(ev))

    def test_other_events_ignored(self):
        ev = H.parse_acpi_event(genl_event(b"ac_adapter", b"ACPI0003:00", 0x80, 1))
        self.assertFalse(H.is_hotkey_event(ev))


class Dispatch(unittest.TestCase):
    def test_keys_and_actions(self):
        called = []
        svc = H.HotkeyService(ec=None, actions={"webcam-toggle": lambda: called.append("cam")},
                              keymap={0x12: "webcam-toggle"})
        svc.kbd = mock.Mock()
        svc.handle_code(H.EV_TOUCHPAD_TOGGLE)
        svc.kbd.tap.assert_called_with(H.KEY_TOUCHPAD_TOGGLE)
        svc.handle_code(0x12)
        self.assertEqual(called, ["cam"])
        svc.handle_code(0xFD)   # real Fn+F1 code -> toggle, not "on"
        svc.kbd.tap.assert_called_with(H.KEY_TOUCHPAD_TOGGLE)
        self.assertEqual(H.DEFAULT_KEYMAP[0x83], "kbd-toggle")
        self.assertEqual(H.DEFAULT_KEYMAP[0x81], "kbd-down")
        self.assertEqual(H.DEFAULT_KEYMAP[0xF7], "webcam-toggle")
        self.assertEqual(H.DEFAULT_KEYMAP[0x9F], "kbd-function")
        svc.handle_code(0xEE)
        self.assertEqual(svc.last_event["action"], None)


class DaemonActions(unittest.TestCase):
    def make(self):
        ec = FakeEC()
        svc = FanService(ec, C.Config(), cpu_fn=lambda: 50.0, gpu=Gpu(),
                         save_fn=lambda c: None, features=Features(ec, CAPS))
        return svc, ec

    def test_colour_cycle(self):
        svc, ec = self.make()
        svc.hk_kbd_function()
        k = svc._fstate()["kbd"]
        self.assertEqual((k["r"], k["g"], k["b"]), (255, 0, 0))
        svc.hk_kbd_function()
        self.assertEqual(svc._fstate()["kbd"]["g"], 96)

    def test_kbd_toggle_and_steps(self):
        svc, ec = self.make()
        kb = lambda: svc._fstate()["kbd"]["brightness"]
        svc.hk_kbd_toggle()                # default 160 -> off
        self.assertEqual(kb(), 0)
        svc.hk_kbd_toggle()                # back on at the remembered level
        self.assertEqual(kb(), 160)
        svc.hk_kbd_step(+1)
        self.assertEqual(kb(), 192)
        svc.hk_kbd_step(-1); svc.hk_kbd_step(-1)
        self.assertEqual(kb(), 128)
        svc.hk_kbd_toggle(); svc.hk_kbd_toggle()
        self.assertEqual(kb(), 128)        # toggle restores the last level, not a default
        for _ in range(10):
            svc.hk_kbd_step(+1)
        self.assertEqual(kb(), 255)
        self.assertEqual(ec.sets[-1], (0x67, 0xF4000000 | 255))

    @mock.patch("acerfan.features.camera_present", return_value=True)
    def test_webcam_toggle(self, _cam):
        svc, ec = self.make()
        svc.hk_webcam_toggle()
        self.assertEqual(ec.sets[-1], (0x22, 0))

    def test_profile_cycle(self):
        svc, ec = self.make()      # FakeAcpi starts at CPCM=3 (balanced)
        svc.hk_profile_cycle()
        self.assertEqual(ec.acpi.cpcm, 2)
        svc.hk_profile_cycle()
        self.assertEqual(ec.acpi.cpcm, 1)

    def test_ac_change_runs_adjp(self):
        svc, ec = self.make()
        svc.hotkeys = mock.Mock(enabled=True)
        ec.acpi.call = mock.Mock(return_value=0)
        with mock.patch("builtins.open", mock.mock_open(read_data="1")):
            svc._watch_ac()
        with mock.patch("builtins.open", mock.mock_open(read_data="0")):
            svc._watch_ac()
        ec.acpi.call.assert_called_once_with(H.AC_ADJP_PATH, "0x0")


if __name__ == "__main__":
    unittest.main()
