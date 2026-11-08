"""Hardware probe (Phase 0). Run as root: sudo python3 -m acerfan probe

Reads firmware values, briefly drives the fans, and ALWAYS hands them
back to EC automatic control at the end. Saves everything to a JSON file.
"""

from __future__ import annotations

import json
import os
import time

from . import ec as E
from .acpi import AcpiError, ensure_module
from .sensors import cpu_temp


def _rpm_candidates(two: bytes) -> dict:
    be = int.from_bytes(two, "big")
    le = int.from_bytes(two, "little")
    return {
        "hex": two.hex(),
        "be": be, "le": le,
        "be_div": int(E.RPM_DIVIDEND / be) if be not in (0, 0xFFFF) else 0,
        "le_div": int(E.RPM_DIVIDEND / le) if le not in (0, 0xFFFF) else 0,
    }


def _sample(ec: E.ClevoEC, label: str) -> dict:
    s = {
        "label": label,
        "t": round(time.monotonic(), 1),
        "cpu_temp": cpu_temp(),
        "dut1": ec.read_ec(E.EC_DUT1)[0],
        "dut2": ec.read_ec(E.EC_DUT2)[0],
        "rpm1": _rpm_candidates(ec.read_ec(E.EC_RPM1, 2)),
        "rpm2": _rpm_candidates(ec.read_ec(E.EC_RPM2, 2)),
        "faninfo1": hex(ec.get(E.CMD_GET_FANINFO1)),
        "faninfo2": hex(ec.get(E.CMD_GET_FANINFO2)),
        "faninfo3": hex(ec.get(E.CMD_GET_FANINFO3)),
    }
    print(f"  [{label:>10}] cpu={s['cpu_temp']}C dut1={s['dut1']:3d} dut2={s['dut2']:3d} "
          f"rpm1(be/div)={s['rpm1']['be']:5d}/{s['rpm1']['be_div']:5d} "
          f"rpm2(be/div)={s['rpm2']['be']:5d}/{s['rpm2']['be_div']:5d} "
          f"fi1={s['faninfo1']} fi2={s['faninfo2']} fi3={s['faninfo3']}")
    return s


def _phase(ec, label, seconds, results):
    for _ in range(seconds):
        time.sleep(1)
        results["samples"].append(_sample(ec, label))


def run_fan_probe(out_path: str) -> int:
    if os.geteuid() != 0:
        print("probe must run as root: sudo python3 -m acerfan probe")
        return 2
    if os.path.exists("/run/acerfan/acerfan.sock"):
        print("acerfand is running; stop it first: sudo systemctl stop acerfan")
        return 2
    ensure_module()
    ec = E.ClevoEC()
    results: dict = {"samples": []}

    print("== Firmware identity / feature flags")
    for name, cmd in (("PSF1(0x52)", E.CMD_GET_FEATURES_1),
                      ("PSF4(0x60)", E.CMD_GET_PSF4),
                      ("PSF2(0x7A)", E.CMD_GET_FEATURES_2)):
        try:
            v = ec.get(cmd)
            results[name] = hex(v)
            print(f"  {name} = {v:#x}")
        except AcpiError as e:
            results[name] = str(e)
            print(f"  {name} error: {e}")

    print("== EC RAM 0xC0..0xFF (via firmware read)")
    dump = ec.read_ec_range(0xC0, 0xFF)
    results["ec_c0_ff"] = dump.hex()
    for row in range(0, len(dump), 16):
        chunk = dump[row:row + 16]
        print(f"  {0xC0 + row:02X}: " + " ".join(f"{b:02x}" for b in chunk))

    try:
        print("== Baseline (BIOS auto), 4 s")
        _phase(ec, "auto", 4, results)

        print("== Manual 100% for 10 s  (fans should get LOUD)")
        ec.set_duty(100, 100)
        _phase(ec, "set100", 10, results)

        print("== Manual 30% for 20 s  (watch whether the EC overrides it)")
        ec.set_duty(30, 30)
        _phase(ec, "set30", 20, results)
    except KeyboardInterrupt:
        print("\ninterrupted")
    finally:
        print("== Restoring BIOS automatic fan control (0x69 0x0F)")
        try:
            ec.set_auto()
        except Exception as e:  # never leave this silently
            print(f"!! FAILED to restore auto: {e}. Run: sudo python3 -m acerfan restore-auto")
            raise
    _phase(ec, "restored", 6, results)

    with open(out_path, "w") as f:
        json.dump(results, f, indent=1)
    os.chmod(out_path, 0o644)
    print(f"\nSaved results to {out_path}")
    return 0
