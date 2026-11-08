"""Command line entry point: acerfan <command>  (or python3 -m acerfan)."""

from __future__ import annotations

import argparse
import json
import os
import sys

from .config import MODES


def _fmt_temp(v, asleep=False):
    if asleep:
        return "asleep"
    return "n/a" if v is None else f"{v:.0f}°C"


def _print_status(st: dict) -> None:
    fans = st.get("fans") or {}
    ctl = "BIOS" if st.get("bios_control") else f"{st.get('target')}%"
    print(f"Mode      : {st['mode']}   (control: {ctl})")
    print(f"CPU       : {_fmt_temp(st.get('cpu'))}")
    print(f"GPU       : {_fmt_temp(st.get('gpu'), st.get('gpu_asleep'))}")
    if fans:
        print(f"Fan 1 CPU : {fans.get('rpm1', 0):5d} rpm  ({fans.get('duty1')}%)")
        print(f"Fan 2 GPU : {fans.get('rpm2', 0):5d} rpm  ({fans.get('duty2')}%)")
    f = st.get("features") or {}
    if f.get("kbd"):
        k = f["kbd"]
        print(f"Keyboard  : #{k['r']:02x}{k['g']:02x}{k['b']:02x}  brightness {k['brightness']}/255")
    if f.get("perf_profile") is not None:
        from .features import PERF_PROFILES
        print(f"Profile   : {PERF_PROFILES[f['perf_profile']][1]}")
    if f.get("charge"):
        c = f["charge"]
        print(f"Charging  : " + (f"limited, stops at {c['stop']}% (resumes below {c['start']}%)"
                                  if c["enabled"] else "no limit"))
    if "webcam" in f:
        print(f"Webcam    : {'on' if f['webcam'] else 'off'}")
    if st.get("emergency"):
        print("!! EMERGENCY: temperature limit hit, fans forced to 100%")
    if st.get("fault"):
        print(f"!! FAULT: {st['fault']}")


NAMED_COLOURS = {"white": "ffffff", "red": "ff0000", "green": "00ff00", "blue": "0000ff",
                 "orange": "ff6000", "purple": "8000ff", "cyan": "00ffff", "pink": "ff2080"}


def _feature_cmd(args) -> int:
    from .client import DaemonUnavailable, request
    try:
        if args.cmd == "kbd":
            req = {"cmd": "set_kbd"}
            if args.color:
                hx = NAMED_COLOURS.get(args.color.lower(), args.color.lstrip("#"))
                if len(hx) != 6:
                    raise ValueError("colour must be RRGGBB")
                req.update(r=int(hx[0:2], 16), g=int(hx[2:4], 16), b=int(hx[4:6], 16))
            if args.brightness is not None:
                req["brightness"] = 0 if args.brightness == "off" else int(args.brightness)
            if len(req) == 1:
                raise ValueError("give --color and/or --brightness")
        elif args.cmd == "profile":
            req = {"cmd": "set_perf_profile", "profile": args.profile}
        elif args.cmd == "charge":
            if args.stop == "off":
                req = {"cmd": "set_charge_limit", "enabled": False, "start": 95, "stop": 100}
            else:
                stop = int(args.stop)
                req = {"cmd": "set_charge_limit", "enabled": True, "stop": stop,
                       "start": args.start if args.start is not None else stop - 5}
        else:
            req = {"cmd": "set_webcam", "on": args.state == "on"}
        r = request(req)
    except DaemonUnavailable as e:
        print(f"error: {e}", file=sys.stderr)
        return 3
    except ValueError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2
    if not r.get("ok"):
        print(f"error: {r.get('error')}", file=sys.stderr)
        return 1
    print("ok")
    return 0


def main(argv=None) -> int:
    p = argparse.ArgumentParser(prog="acerfan", description="Acer A715-79G fan control")
    sub = p.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("status", help="show temperatures, fan speed and mode")
    s.add_argument("--json", action="store_true")
    m = sub.add_parser("mode", help="set fan mode")
    m.add_argument("mode", choices=MODES)
    c = sub.add_parser("curve", help="show or set the custom curve")
    c.add_argument("points", nargs="*", help="TEMP:DUTY pairs, e.g. 50:0 70:40 90:100")
    k = sub.add_parser("kbd", help="keyboard backlight: acerfan kbd [--color RRGGBB] [--brightness 0-255|off]")
    k.add_argument("--color", "-c", help="hex colour, e.g. ff8800 or white/red/green/blue/orange/purple/cyan")
    k.add_argument("--brightness", "-b", help="0..255, or 'off'")
    pp = sub.add_parser("profile", help="firmware performance profile")
    pp.add_argument("profile", choices=["quiet", "power-saver", "balanced", "performance", "bios"])
    ch = sub.add_parser("charge", help="battery charge limit: acerfan charge 80 | acerfan charge off")
    ch.add_argument("stop", help="stop charging at this %%, or 'off'")
    ch.add_argument("--start", type=int, help="resume charging below this %% (default stop-5)")
    wc = sub.add_parser("webcam", help="turn the webcam on or off")
    wc.add_argument("state", choices=["on", "off"])
    ks = sub.add_parser("keys", help="show Fn hotkey events as you press them (root)")
    ks.add_argument("-t", "--seconds", type=int, default=60)
    sub.add_parser("reapply", help="ask the daemon to resend settings (used after resume)")
    sub.add_parser("gui", help="open the AcerFan window")
    sub.add_parser("daemon", help="run the root daemon in the foreground")
    pr = sub.add_parser("probe", help="Phase 0 hardware probe (root)")
    pr.add_argument("-o", "--out", default=os.path.abspath("probe-results.json"))
    pf = sub.add_parser("probe-features", help="interactive probe of keyboard/battery/profile/switches (root)")
    pf.add_argument("-o", "--out", default=os.path.abspath("feature-probe.json"))
    sub.add_parser("restore-auto", help="hand fans back to BIOS control (root)")
    sub.add_parser("temps", help="print CPU/GPU temperatures (no daemon needed)")
    args = p.parse_args(argv)

    if args.cmd in ("kbd", "profile", "charge", "webcam"):
        return _feature_cmd(args)

    if args.cmd in ("status", "mode", "curve", "reapply"):
        from .client import DaemonUnavailable, request
        try:
            if args.cmd == "status":
                st = request({"cmd": "status"})
                print(json.dumps(st, indent=1)) if args.json else _print_status(st)
                return 0
            if args.cmd == "reapply":
                r = request({"cmd": "reapply"})
            elif args.cmd == "mode":
                r = request({"cmd": "set_mode", "mode": args.mode})
            elif args.points:
                pts = [[float(x) for x in pair.split(":")] for pair in args.points]
                r = request({"cmd": "set_curve", "points": pts})
            else:
                cfg = request({"cmd": "get_config"})["config"]
                print("  ".join(f"{t:g}°C→{d:g}%" for t, d in cfg["custom_curve"]))
                return 0
        except DaemonUnavailable as e:
            print(f"error: {e}", file=sys.stderr)
            return 3
        except ValueError:
            print("error: points must look like 50:0 70:40 90:100", file=sys.stderr)
            return 2
        if not r.get("ok"):
            print(f"error: {r.get('error')}", file=sys.stderr)
            return 1
        print("ok")
        return 0

    if args.cmd == "gui":
        from .gui.app import main as gmain
        return gmain()

    if args.cmd == "daemon":
        from .daemon import main as dmain
        return dmain()

    if args.cmd == "probe":
        from .probe import run_fan_probe
        return run_fan_probe(args.out)

    if args.cmd == "keys":
        from .hotkeys import run_monitor
        return run_monitor(args.seconds)

    if args.cmd == "probe-features":
        from .probe_features import run
        return run(args.out)

    if args.cmd == "restore-auto":
        from .acpi import ensure_module
        from .ec import ClevoEC
        ensure_module()
        ClevoEC().set_auto()
        print("fans returned to BIOS automatic control")
        return 0

    if args.cmd == "temps":
        from .sensors import GpuTemp, cpu_temp
        g = GpuTemp()
        gpu = "asleep" if g.asleep() else g.read()
        print(f"CPU {cpu_temp()} C   GPU {gpu}")
        return 0
    return 1


if __name__ == "__main__":
    sys.exit(main())
