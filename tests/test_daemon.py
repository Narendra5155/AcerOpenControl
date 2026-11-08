import os
import tempfile
import threading
import unittest

from acerfan import config as C
from acerfan.daemon import FanService
from acerfan.ec import FanStatus


class FakeEC:
    def __init__(self):
        self.log = []
        self.fail = False

    def set_duty(self, a, b=None):
        if self.fail:
            raise RuntimeError("boom")
        self.log.append(("duty", a))

    def set_auto(self, mask=0x0F):
        self.log.append(("auto",))

    def read_status(self):
        return FanStatus(10, 10, 2000, 2000, {})


class FakeGpu:
    def __init__(self, temp=None):
        self.temp = temp

    def asleep(self):
        return self.temp is None

    def read(self):
        return self.temp


class Clock:
    t = 0.0

    def __call__(self):
        return self.t


def make(mode="balanced", cpu=50.0, gpu=None):
    ec, cfg, temps, clock = FakeEC(), C.Config(mode=mode), {"cpu": cpu}, Clock()
    svc = FanService(ec, cfg, cpu_fn=lambda: temps["cpu"], gpu=FakeGpu(gpu),
                     save_fn=lambda c: None, clock=clock)
    return svc, ec, temps, clock


class ServiceTest(unittest.TestCase):
    def test_curve_applied_once(self):
        svc, ec, *_ = make(cpu=65)
        svc.tick(); svc.tick()
        self.assertEqual(ec.log, [("duty", 40)])

    def test_refresh_resends(self):
        svc, ec, _, clock = make(cpu=65)
        svc.tick()
        clock.t = 31
        svc.tick()
        self.assertEqual(len(ec.log), 2)

    def test_auto_mode_hands_to_bios(self):
        svc, ec, *_ = make(mode="auto")
        svc.tick(); svc.tick()
        self.assertEqual(ec.log, [("auto",)])
        self.assertTrue(svc.status()["bios_control"])

    def test_gpu_drives_when_hotter(self):
        svc, ec, *_ = make(cpu=45, gpu=75)
        svc.tick()
        self.assertEqual(ec.log[-1], ("duty", 60))

    def test_emergency(self):
        svc, ec, *_ = make(mode="silent", cpu=93)
        svc.tick()
        self.assertEqual(ec.log[-1], ("duty", 100))
        self.assertTrue(svc.status()["emergency"])

    def test_sensor_failure_falls_back_to_bios(self):
        svc, ec, temps, _ = make(cpu=65)
        svc.tick()
        temps["cpu"] = None
        for _ in range(3):
            svc.tick()
        self.assertEqual(ec.log[-1], ("auto",))
        self.assertIsNotNone(svc.status()["fault"])

    def test_hw_errors_fall_back_to_bios(self):
        svc, ec, *_ = make(cpu=65)
        ec.fail = True
        for _ in range(3):
            svc.tick()
        self.assertEqual(ec.log[-1], ("auto",))

    def test_set_mode_and_curve(self):
        svc, ec, *_ = make(cpu=65)
        self.assertTrue(svc.handle({"cmd": "set_mode", "mode": "max"})["ok"])
        self.assertEqual(ec.log[-1], ("duty", 100))
        self.assertFalse(svc.handle({"cmd": "set_mode", "mode": "turbo"})["ok"])
        self.assertFalse(svc.handle({"cmd": "set_curve", "points": [[50, 90], [60, 10]]})["ok"])
        r = svc.handle({"cmd": "set_curve", "points": [[40, 0], [90, 100]]})
        self.assertTrue(r["ok"])
        self.assertEqual(svc.cfg.mode, "custom")
        self.assertEqual(ec.log[-1], ("duty", 50))

    def test_shutdown_restores(self):
        svc, ec, *_ = make(cpu=65)
        svc.tick()
        svc.shutdown()
        self.assertEqual(ec.log[-1], ("auto",))


class SocketTest(unittest.TestCase):
    def test_roundtrip(self):
        from acerfan import client, daemon
        svc, *_ = make(cpu=65)
        d = tempfile.mkdtemp()
        path = os.path.join(d, "s.sock")
        srv = daemon._Server(path, daemon._Handler)
        srv.service = svc
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        try:
            r = client.request({"cmd": "status"}, path=path)
            self.assertTrue(r["ok"])
            self.assertEqual(r["mode"], "balanced")
            self.assertFalse(client.request({"cmd": "nope"}, path=path)["ok"])
        finally:
            srv.shutdown()
            srv.server_close()

    def test_config_roundtrip(self):
        d = tempfile.mkdtemp()
        p = os.path.join(d, "config.json")
        C.save(C.Config(mode="silent"), p)
        self.assertEqual(C.load(p).mode, "silent")
        with open(p, "w") as f:
            f.write('{"mode": "bogus"}')
        self.assertEqual(C.load(p).mode, "balanced")


if __name__ == "__main__":
    unittest.main()
