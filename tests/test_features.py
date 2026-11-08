import unittest
from unittest import mock

from acerfan import config as C
from acerfan.daemon import FanService
from acerfan.ec import FanStatus
from acerfan.features import Features, validate_charge

CAPS = {"kbd_rgb": True, "kbd_channel_shift": {"blue": 16, "red": 8, "green": 0},
        "perf_profile": True, "battery_limit": True, "webcam_switch": True,
        "webcam_off_value": 0}


class FakeAcpi:
    def __init__(self):
        self.cpcm = 3

    def call(self, path):
        return self.cpcm


class FakeEC:
    def __init__(self):
        self.sets = []
        self.acpi = FakeAcpi()
        self.charge = 0x645F00

    def set(self, cmd, arg=0):
        self.sets.append((cmd, arg))
        if cmd == 0x79 and arg >> 24 == 0x06:
            stop, start = (arg >> 8) & 0xFF, arg & 0xFF
            self.charge = (stop << 16) | (start << 8) | (self.charge & 0xFF)
        if cmd == 0x79 and arg >> 24 == 0x05:
            self.charge = (self.charge & ~0xFF) | (arg & 1)
        if cmd == 0x79 and arg >> 24 == 0x19:
            self.acpi.cpcm = arg & 0xFF
        return cmd

    def get(self, cmd, arg=0):
        if cmd == 0x77:
            return self.charge
        return 0

    # fan side, for FanService
    def set_duty(self, a, b=None):
        pass

    def set_auto(self, m=15):
        pass

    def read_status(self):
        return FanStatus(0, 0, 0, 0, {})


class FeaturesTest(unittest.TestCase):
    def test_kbd_brg_packing(self):
        ec = FakeEC()
        Features(ec, CAPS).set_kbd(0x11, 0x22, 0x33, 200)
        self.assertEqual(ec.sets, [(0x67, 0xF0000000 | 0x33 << 16 | 0x11 << 8 | 0x22),
                                   (0x67, 0xF4000000 | 200)])

    def test_charge_roundtrip(self):
        ec = FakeEC()
        f = Features(ec, CAPS)
        self.assertEqual(f.get_charge().to_dict(), {"enabled": False, "start": 95, "stop": 100})
        got = f.set_charge(True, 75, 80)
        self.assertEqual(got.to_dict(), {"enabled": True, "start": 75, "stop": 80})
        self.assertEqual(ec.sets[0], (0x79, 0x06 << 24 | 80 << 8 | 75))
        self.assertEqual(ec.sets[1], (0x79, 0x05 << 24 | 1))

    def test_charge_validation(self):
        for bad in ((True, 80, 80), (True, 30, 80), (True, 70, 120)):
            with self.assertRaises(ValueError):
                validate_charge(*bad)

    def test_perf(self):
        ec = FakeEC()
        f = Features(ec, CAPS)
        f.set_perf_profile(2)
        self.assertEqual(ec.sets[-1], (0x79, 0x19 << 24 | 2))
        self.assertEqual(f.get_perf_profile(), 2)
        with self.assertRaises(ValueError):
            f.set_perf_profile(7)

    def test_webcam(self):
        ec = FakeEC()
        f = Features(ec, CAPS)
        f.set_webcam(False)
        f.set_webcam(True)
        self.assertEqual(ec.sets, [(0x22, 0), (0x22, 1)])


class Gpu:
    def asleep(self):
        return True

    def read(self):
        return None


class DaemonFeatureTest(unittest.TestCase):
    def make(self):
        ec = FakeEC()
        svc = FanService(ec, C.Config(), cpu_fn=lambda: 50.0, gpu=Gpu(),
                         save_fn=lambda c: None, features=Features(ec, CAPS))
        return svc, ec

    @mock.patch("acerfan.features.camera_present", return_value=True)
    def test_api(self, _cam):
        svc, ec = self.make()
        r = svc.handle({"cmd": "set_kbd", "r": 255, "g": 0, "b": 0, "brightness": 100})
        self.assertTrue(r["ok"])
        self.assertEqual(r["kbd"], {"r": 255, "g": 0, "b": 0, "brightness": 100})
        r = svc.handle({"cmd": "set_kbd", "brightness": 10})
        self.assertEqual(ec.sets[-1], (0x67, 0xF4000000 | 10))  # brightness only
        self.assertTrue(svc.handle({"cmd": "set_perf_profile", "profile": "performance"})["ok"])
        self.assertEqual(ec.acpi.cpcm, 2)
        r = svc.handle({"cmd": "set_charge_limit", "enabled": True, "start": 55, "stop": 60})
        self.assertEqual(r["charge"], {"enabled": True, "start": 55, "stop": 60})
        self.assertFalse(svc.handle({"cmd": "set_charge_limit", "start": 90, "stop": 80})["ok"])
        self.assertFalse(svc.handle({"cmd": "set_kbd", "r": 300})["ok"])
        self.assertTrue(svc.handle({"cmd": "get_features"})["available"]["keyboard"])

    @mock.patch("acerfan.features.camera_present", return_value=True)
    def test_reapply_saved(self, _cam):
        svc, ec = self.make()
        svc.handle({"cmd": "set_kbd", "r": 1, "g": 2, "b": 3, "brightness": 50})
        svc.handle({"cmd": "set_perf_profile", "profile": 3})
        svc.handle({"cmd": "set_webcam", "on": False})
        ec.sets.clear()
        svc.apply_saved_features()
        cmds = [c for c, _ in ec.sets]
        self.assertIn(0x67, cmds)
        self.assertIn((0x79, 0x19 << 24 | 3), ec.sets)
        self.assertIn((0x22, 0), ec.sets)

    def test_nothing_forced_by_default(self):
        svc, ec = self.make()
        svc.apply_saved_features()
        self.assertEqual(ec.sets, [])


if __name__ == "__main__":
    unittest.main()
