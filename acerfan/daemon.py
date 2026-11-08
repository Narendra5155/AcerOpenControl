"""acerfand: root daemon that runs the fan curves and serves a local JSON API.

Socket: /run/acerfan/acerfan.sock (0660, group 'acerfan'), one JSON object
per line in each direction.
"""

from __future__ import annotations

import grp
import json
import logging
import os
import signal
import socketserver
import threading
import time
from typing import Callable, Optional

from . import __version__
from . import config as C
from .acpi import ensure_module
from .curve import Controller, Curve, validate_points
from .ec import ClevoEC
from .sensors import GpuTemp, cpu_temp

SOCK_DIR = os.environ.get("ACERFAN_RUN_DIR", "/run/acerfan")
SOCK_PATH = os.path.join(SOCK_DIR, "acerfan.sock")
SOCK_GROUP = "acerfan"

log = logging.getLogger("acerfand")

AC_ONLINE = "/sys/class/power_supply/AC/online"
KBD_CYCLE = [(255, 255, 255), (255, 0, 0), (255, 96, 0), (255, 204, 0), (0, 255, 0),
             (0, 255, 255), (0, 0, 255), (128, 0, 255), (255, 32, 128)]
PROFILE_CYCLE = [1, 0, 3, 2]
KBD_LEVELS = [0, 32, 64, 96, 128, 160, 192, 224, 255]   # power saver -> quiet -> balanced -> performance

FEATURE_CMDS = ("get_features", "set_kbd", "set_perf_profile", "set_charge_limit", "set_webcam")

SENSOR_FAIL_LIMIT = 3
HW_FAIL_LIMIT = 3


class FanService:
    def __init__(self, ec, cfg: C.Config, cpu_fn: Callable[[], Optional[float]] = cpu_temp,
                 gpu=None, save_fn: Callable[[C.Config], None] = C.save,
                 clock: Callable[[], float] = time.monotonic, features=None):
        self.ec = ec
        self.features = features          # acerfan.features.Features or None
        self.kbd_led = None               # KbdLedBridge, set by main()
        self.hotkeys = None               # HotkeyService, set by main()
        self._ac_online: Optional[str] = None
        self.cfg = cfg
        self.cpu_fn = cpu_fn
        self.gpu = gpu if gpu is not None else GpuTemp()
        self.save_fn = save_fn
        self.clock = clock
        self.lock = threading.RLock()

        self.controller: Optional[Controller] = None
        self.applied_duty: Optional[int] = None   # None = EC/BIOS in control
        self.last_sent = 0.0
        self.force = True
        self.sensor_fails = 0
        self.hw_fails = 0
        self.fault: Optional[str] = None
        self.emergency = False
        self.last = {"cpu": None, "gpu": None, "gpu_asleep": True, "fans": None,
                     "target": None, "features": {}}
        self._ticks = 0
        self._rebuild_controller()

    # -- control ---------------------------------------------------------------
    def _rebuild_controller(self):
        pts = self.cfg.curve_points()
        self.controller = Controller(
            Curve(pts), self.cfg.hysteresis, self.cfg.ramp_down,
            self.cfg.floor_pct, self.cfg.floor_temp) if pts else None
        self.force = True

    def _to_bios(self, reason: Optional[str] = None):
        self.ec.set_auto()
        self.applied_duty = None
        self.last_sent = self.clock()
        if reason:
            log.warning("fans handed to BIOS: %s", reason)

    def _send(self, duty: int):
        self.ec.set_duty(duty, duty)
        self.applied_duty = duty
        self.last_sent = self.clock()

    def tick(self) -> None:
        with self.lock:
            try:
                self._tick()
                self.hw_fails = 0
            except Exception as e:  # noqa: BLE001 - keep the loop alive
                self.hw_fails += 1
                log.error("tick failed (%d): %s", self.hw_fails, e)
                if self.hw_fails >= HW_FAIL_LIMIT:
                    self.fault = f"hardware error: {e}"
                    try:
                        self._to_bios("repeated hardware errors")
                    except Exception:  # noqa: BLE001
                        pass

    def _tick(self):
        self._watch_ac()
        cpu = self.cpu_fn()
        asleep = self.gpu.asleep()
        gpu = None if asleep else self.gpu.read()
        self.last.update(cpu=cpu, gpu=gpu, gpu_asleep=asleep)

        try:
            self.last["fans"] = self.ec.read_status().to_dict()
        except Exception as e:  # noqa: BLE001 - status is best effort
            log.debug("fan status read failed: %s", e)

        if self.features and self._ticks % 5 == 0:
            self._refresh_feature_state()
        self._ticks += 1

        now = self.clock()
        refresh_due = now - self.last_sent >= self.cfg.refresh_interval
        mode = self.cfg.mode

        if mode == "auto":
            self.emergency = False
            self.last["target"] = None
            if self.applied_duty is not None or self.force:
                self._to_bios()
            self.force = False
            return

        if cpu is None:
            self.sensor_fails += 1
            if self.sensor_fails >= SENSOR_FAIL_LIMIT:
                self.fault = "CPU temperature unavailable"
                if self.applied_duty is not None or self.force:
                    self._to_bios(self.fault)
                self.force = False
            return
        self.sensor_fails = 0
        self.fault = None

        temps = [t for t in (cpu, gpu) if t is not None]
        hottest = max(temps)
        self.emergency = cpu >= self.cfg.cpu_emergency or (
            gpu is not None and gpu >= self.cfg.gpu_emergency)

        if mode == "max" or self.emergency:
            target = 100
            if self.controller:
                self.controller.step(hottest)  # keep its state current
        else:
            target = self.controller.step(hottest)
        self.last["target"] = target

        if self.force or target != self.applied_duty or refresh_due:
            self._send(target)
            self.force = False

    # -- API -------------------------------------------------------------------
    def status(self) -> dict:
        with self.lock:
            return {
                "version": __version__,
                "mode": self.cfg.mode,
                "cpu": self.last["cpu"],
                "gpu": self.last["gpu"],
                "gpu_asleep": self.last["gpu_asleep"],
                "target": self.last["target"],
                "bios_control": self.applied_duty is None,
                "fans": self.last["fans"],
                "emergency": self.emergency,
                "fault": self.fault,
                "features": self.last["features"],
                "hotkey": self.hotkeys.last_event if self.hotkeys else None,
            }

    def handle(self, req: dict) -> dict:
        cmd = req.get("cmd")
        with self.lock:
            if cmd == "status":
                return {"ok": True, **self.status()}
            if cmd == "get_config":
                from dataclasses import asdict
                return {"ok": True, "config": asdict(self.cfg), "modes": list(C.MODES)}
            if cmd == "set_mode":
                mode = req.get("mode")
                if mode not in C.MODES:
                    return {"ok": False, "error": f"unknown mode {mode!r}"}
                self.cfg.mode = mode
                self._rebuild_controller()
                self.save_fn(self.cfg)
                self._tick_safely()
                return {"ok": True, "mode": mode}
            if cmd == "set_curve":
                try:
                    pts = validate_points(req.get("points") or [])
                except (TypeError, ValueError) as e:
                    return {"ok": False, "error": str(e)}
                self.cfg.custom_curve = [list(p) for p in pts]
                if req.get("activate", True):
                    self.cfg.mode = "custom"
                self._rebuild_controller()
                self.save_fn(self.cfg)
                self._tick_safely()
                return {"ok": True}
            if cmd == "reapply":
                self.force = True
                if self.hotkeys:
                    try:
                        self.hotkeys.enable_firmware()   # sleep clears WMI.HKDR
                    except Exception as e:  # noqa: BLE001
                        log.error("re-enabling hotkeys failed: %s", e)
                self.apply_saved_features()
                self._tick_safely()
                return {"ok": True}
            if cmd in FEATURE_CMDS:
                return self._handle_feature(cmd, req)
            return {"ok": False, "error": f"unknown cmd {cmd!r}"}

    def _tick_safely(self):
        try:
            self._tick()
        except Exception as e:  # noqa: BLE001
            log.error("apply failed: %s", e)

    # -- firmware features ---------------------------------------------------------
    def _fstate(self) -> dict:
        return self.cfg.features.setdefault("saved", {})

    def _refresh_feature_state(self):
        f, av = self.features, self.features.available()
        st = {"available": av, "kbd": self._fstate().get("kbd")}
        try:
            if av["perf_profile"]:
                st["perf_profile"] = f.get_perf_profile()
            if av["battery_limit"]:
                st["charge"] = f.get_charge().to_dict()
            if av["webcam"]:
                st["webcam"] = f.webcam_on()
        except Exception as e:  # noqa: BLE001
            log.debug("feature state read failed: %s", e)
        self.last["features"] = st

    def apply_saved_features(self):
        """Reapply what the user chose (boot, resume). Never forces defaults."""
        if not self.features:
            return
        saved, av = self._fstate(), self.features.available()
        steps = []
        if av["keyboard"] and saved.get("kbd"):
            k = saved["kbd"]
            steps.append(("keyboard", lambda: self.features.set_kbd(k["r"], k["g"], k["b"], k["brightness"])))
        if av["perf_profile"] and saved.get("perf_profile") is not None:
            steps.append(("profile", lambda: self.features.set_perf_profile(int(saved["perf_profile"]))))
        if av["battery_limit"] and saved.get("charge"):
            c = saved["charge"]
            steps.append(("charge limit", lambda: self.features.set_charge(c["enabled"], c["start"], c["stop"])))
        if av["webcam"] and saved.get("webcam") is False:
            steps.append(("webcam", lambda: self.features.set_webcam(False)))
        for name, fn in steps:
            try:
                fn()
            except Exception as e:  # noqa: BLE001
                log.error("reapplying %s failed: %s", name, e)
        self._refresh_feature_state()

    def _handle_feature(self, cmd: str, req: dict) -> dict:
        from .features import PERF_BY_KEY, PERF_PROFILES, validate_charge, validate_rgb
        if not self.features:
            return {"ok": False, "error": "firmware features unavailable"}
        av, saved = self.features.available(), self._fstate()
        try:
            if cmd == "get_features":
                self._refresh_feature_state()
                return {"ok": True, **self.last["features"],
                        "profiles": {str(i): {"key": k, "label": l, "desc": d}
                                     for i, (k, l, d) in PERF_PROFILES.items()}}
            if cmd == "set_kbd":
                if not av["keyboard"]:
                    raise ValueError("keyboard backlight not supported")
                cur = dict(saved.get("kbd") or {"r": 255, "g": 255, "b": 255, "brightness": 160})
                for key in ("r", "g", "b", "brightness"):
                    if key in req:
                        cur[key] = int(req[key])
                validate_rgb(cur["r"], cur["g"], cur["b"])
                if set(req) - {"cmd", "brightness", "from_led"}:
                    self.features.set_kbd(cur["r"], cur["g"], cur["b"], cur["brightness"])
                else:
                    self.features.set_kbd_brightness(cur["brightness"])
                saved["kbd"] = cur
                if self.kbd_led and not req.get("from_led"):
                    self.kbd_led.sync(cur["brightness"])
            elif cmd == "set_perf_profile":
                if not av["perf_profile"]:
                    raise ValueError("performance profiles not supported")
                p = req.get("profile")
                if p is None or p == "bios":
                    saved["perf_profile"] = None
                else:
                    idx = PERF_BY_KEY[p] if isinstance(p, str) and p in PERF_BY_KEY else int(p)
                    self.features.set_perf_profile(idx)
                    saved["perf_profile"] = idx
            elif cmd == "set_charge_limit":
                if not av["battery_limit"]:
                    raise ValueError("charge limit not supported")
                lim = validate_charge(req.get("enabled", True), req.get("start", 75), req.get("stop", 80))
                got = self.features.set_charge(lim.enabled, lim.start, lim.stop)
                saved["charge"] = lim.to_dict()
                if got.to_dict() != lim.to_dict():
                    log.warning("charge limit readback %s != requested %s", got, lim)
            elif cmd == "set_webcam":
                if not av["webcam"]:
                    raise ValueError("webcam switch not supported")
                on = bool(req.get("on"))
                self.features.set_webcam(on)
                saved["webcam"] = on
        except (KeyError, TypeError, ValueError) as e:
            return {"ok": False, "error": str(e)}
        self.save_fn(self.cfg)
        self._refresh_feature_state()
        return {"ok": True, **self.last["features"]}

    # -- hotkey actions --------------------------------------------------------------
    def _watch_ac(self):
        """With hotkeys registered (HKDR=1) the firmware's _PSR no longer calls
        AC.ADJP on charger plug/unplug, so the daemon does it instead."""
        try:
            with open(AC_ONLINE) as f:
                online = f.read().strip()
        except OSError:
            return
        if self._ac_online is not None and online != self._ac_online and \
                self.hotkeys and self.hotkeys.enabled:
            try:
                from .hotkeys import AC_ADJP_PATH
                self.ec.acpi.call(AC_ADJP_PATH, "0x0")
                log.info("charger %s: ran AC.ADJP", "connected" if online == "1" else "removed")
            except Exception as e:  # noqa: BLE001
                log.error("AC.ADJP failed: %s", e)
        self._ac_online = online

    def hk_webcam_toggle(self):
        if not self.features or not self.features.available()["webcam"]:
            return
        on = self.features.webcam_on()
        self.handle({"cmd": "set_webcam", "on": not on})
        log.info("webcam %s by hotkey", "off" if on else "on")

    def _kbd_current(self) -> dict:
        return dict(self._fstate().get("kbd") or {"r": 255, "g": 255, "b": 255, "brightness": 160})

    def _kbd_set_brightness(self, value: int):
        if not self.features or not self.features.available()["keyboard"]:
            return
        cur = self._kbd_current()
        if cur["brightness"] > 0:
            self._fstate()["kbd_last_on"] = cur["brightness"]
        if not self._fstate().get("kbd"):
            # First use: send the colour too so the EC state is fully defined.
            self.handle({"cmd": "set_kbd", **{**cur, "brightness": value}})
        else:
            self.handle({"cmd": "set_kbd", "brightness": value})

    def hk_kbd_step(self, direction: int):
        cur = self._kbd_current()["brightness"]
        if direction > 0:
            nxt = next((lv for lv in KBD_LEVELS if lv > cur), KBD_LEVELS[-1])
        else:
            nxt = next((lv for lv in reversed(KBD_LEVELS) if lv < cur), 0)
        self._kbd_set_brightness(nxt)

    def hk_kbd_toggle(self):
        cur = self._kbd_current()["brightness"]
        if cur > 0:
            self._kbd_set_brightness(0)
        else:
            self._kbd_set_brightness(int(self._fstate().get("kbd_last_on") or 160))

    def hk_kbd_function(self):
        """Fn + '*': cycle through preset keyboard colours."""
        cur = self._fstate().get("kbd") or {"r": 255, "g": 255, "b": 255, "brightness": 160}
        key = (cur["r"], cur["g"], cur["b"])
        idx = KBD_CYCLE.index(key) if key in KBD_CYCLE else -1
        r, g, b = KBD_CYCLE[(idx + 1) % len(KBD_CYCLE)]
        self.handle({"cmd": "set_kbd", "r": r, "g": g, "b": b,
                     "brightness": cur["brightness"] or 160})

    def hk_profile_cycle(self):
        if not self.features or not self.features.available()["perf_profile"]:
            return
        cur = self.features.get_perf_profile()
        nxt = PROFILE_CYCLE[(PROFILE_CYCLE.index(cur) + 1) % len(PROFILE_CYCLE)] \
            if cur in PROFILE_CYCLE else 3
        self.handle({"cmd": "set_perf_profile", "profile": nxt})

    def shutdown(self):
        with self.lock:
            try:
                self._to_bios("daemon stopping")
            except Exception as e:  # noqa: BLE001
                log.error("could not restore BIOS control: %s", e)


# -- socket server ---------------------------------------------------------------
class _Handler(socketserver.StreamRequestHandler):
    def handle(self):
        service: FanService = self.server.service  # type: ignore[attr-defined]
        for line in self.rfile:
            if len(line) > 64 * 1024:
                break
            try:
                req = json.loads(line)
                resp = service.handle(req) if isinstance(req, dict) else \
                    {"ok": False, "error": "request must be an object"}
            except json.JSONDecodeError:
                resp = {"ok": False, "error": "invalid JSON"}
            self.wfile.write((json.dumps(resp) + "\n").encode())
            self.wfile.flush()


class _Server(socketserver.ThreadingUnixStreamServer):
    daemon_threads = True


def _secure_socket(path: str):
    try:
        gid = grp.getgrnam(SOCK_GROUP).gr_gid
        os.chown(path, 0, gid)
        os.chmod(path, 0o660)
    except KeyError:
        log.warning("group %r missing; socket is root-only", SOCK_GROUP)
        os.chmod(path, 0o600)


def main() -> int:
    logging.basicConfig(level=logging.INFO, format="%(levelname)s %(message)s")
    if os.geteuid() != 0:
        print("acerfand must run as root")
        return 2
    ensure_module()
    from .features import Features
    from .kbd_led import KbdLedBridge
    ec = ClevoEC()
    service = FanService(ec, C.load(), features=Features(ec))
    service.apply_saved_features()
    if service.features.available()["keyboard"] and service.cfg.features.get("gnome_kbd_slider", True):
        bridge = KbdLedBridge(lambda v: service.handle(
            {"cmd": "set_kbd", "brightness": v, "from_led": True}))
        if bridge.start():
            service.kbd_led = bridge
            kbd = service.cfg.features.get("saved", {}).get("kbd")
            if kbd:
                bridge.sync(kbd["brightness"])
    log.info("acerfand %s starting, mode=%s", __version__, service.cfg.mode)

    if service.cfg.features.get("hotkeys", True):
        from .hotkeys import HotkeyService
        keymap = {int(k, 16): v for k, v in service.cfg.features.get("keymap", {}).items()}
        hk = HotkeyService(ec, {
            "webcam-toggle": service.hk_webcam_toggle,
            "kbd-function": service.hk_kbd_function,
            "kbd-up": lambda: service.hk_kbd_step(+1),
            "kbd-down": lambda: service.hk_kbd_step(-1),
            "kbd-toggle": service.hk_kbd_toggle,
            "profile-cycle": service.hk_profile_cycle,
        }, keymap)
        if hk.start():
            service.hotkeys = hk

    os.makedirs(SOCK_DIR, exist_ok=True)
    if os.path.exists(SOCK_PATH):
        os.unlink(SOCK_PATH)
    server = _Server(SOCK_PATH, _Handler)
    server.service = service  # type: ignore[attr-defined]
    _secure_socket(SOCK_PATH)
    threading.Thread(target=server.serve_forever, daemon=True).start()

    stop = threading.Event()
    for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        signal.signal(sig, lambda *_: stop.set())
    try:
        while not stop.is_set():
            service.tick()
            stop.wait(service.cfg.poll_interval)
    finally:
        server.shutdown()
        if service.kbd_led:
            service.kbd_led.stop()
        if service.hotkeys:
            service.hotkeys.stop()
        service.shutdown()
        try:
            os.unlink(SOCK_PATH)
        except OSError:
            pass
        log.info("acerfand stopped; fans returned to BIOS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
