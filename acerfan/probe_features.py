"""Interactive probe for the non-fan firmware features (Phase 0, step 5).

    sudo acerfan probe-features

Every change is reverted (or set to a sane default) before moving on.
Results are saved to /etc/acerfan/capabilities.json and to ./feature-probe.json.
"""

from __future__ import annotations

import glob
import json
import os
import subprocess
import time

from .acpi import AcpiError, ensure_module
from .config import CONFIG_DIR
from .ec import ClevoEC

CAPS_PATH = os.path.join(CONFIG_DIR, "capabilities.json")

GET_IDS = {
    0x05: "B8A1 bit1 (webcam?)", 0x06: "B8A1 ?", 0x07: "SLTY", 0x09: "B8A1 bit4",
    0x0A: "B8A1 bit3 (touchpad?)", 0x11: "B8A1 bit6", 0x3D: "kbd CA01",
    0x3E: "B8A2 (wireless?)", 0x52: "PSF1", 0x60: "PSF4", 0x77: "battery limit", 0x7A: "PSF2",
}
CPCM_PATH = r"\_SB.PC00.LPCB.EC.CPCM"

# Keyboard: SCMD 0x67. 0xF0000000 | b16..23 | b8..15 | b0..7 -> EC CA 03 (colour)
KB_COLOUR = 0xF0000000
KB_BRIGHT = 0xF4000000


def ask(q: str) -> bool:
    while True:
        a = input(f"   ? {q} [y/n] ").strip().lower()
        if a in ("y", "n"):
            return a == "y"


def rapl():
    out = {}
    for c in ("constraint_0_power_limit_uw", "constraint_1_power_limit_uw"):
        p = f"/sys/class/powercap/intel-rapl:0/{c}"
        try:
            out[c[:12]] = int(open(p).read()) // 1_000_000
        except OSError:
            out[c[:12]] = None
    try:
        r = subprocess.run(["nvidia-smi", "--query-gpu=power.limit,enforced.power.limit",
                            "--format=csv,noheader"], capture_output=True, text=True, timeout=5)
        out["gpu_limit"] = r.stdout.strip()
    except (OSError, subprocess.SubprocessError):
        out["gpu_limit"] = None
    return out


def cameras() -> int:
    n = 0
    for d in glob.glob("/sys/bus/usb/devices/*/product"):
        try:
            if "cam" in open(d).read().lower():
                n += 1
        except OSError:
            pass
    return n


def rfkill() -> str:
    try:
        return subprocess.run(["rfkill", "-n", "-o", "TYPE,SOFT,HARD"], capture_output=True,
                              text=True, timeout=5).stdout.strip().replace("\n", " | ")
    except (OSError, subprocess.SubprocessError):
        return "?"


def toggle_test(ec, cmd, observe, settle):
    """Try value 0 then 1, see which one changes `observe()`, and always end
    with the observed state equal to what it was at the start."""
    before = observe()
    r = {"before": before, "works": False, "restored": True}
    for off_val, on_val in ((0, 1), (1, 0)):
        ec.set(cmd, off_val)
        time.sleep(settle)
        changed = observe()
        ec.set(cmd, on_val)
        time.sleep(settle)
        back = observe()
        if changed != before:
            r.update(works=True, off_value=off_val, changed=changed, restored=(back == before))
            return r
    r["restored"] = observe() == before
    return r


def run(out_path: str) -> int:
    if os.geteuid() != 0:
        print("run as root: sudo acerfan probe-features")
        return 2
    ensure_module()
    ec = ClevoEC()
    res: dict = {"gets": {}}
    caps: dict = {}

    print("== Reading firmware getters")
    for cid, name in GET_IDS.items():
        try:
            v = ec.get(cid)
            res["gets"][hex(cid)] = hex(v)
            print(f"   {cid:#04x} {name:<24} = {v:#x}")
        except AcpiError as e:
            res["gets"][hex(cid)] = str(e)
    try:
        res["cpcm"] = ec.acpi.call(CPCM_PATH)
        print(f"   CPCM (current perf profile index) = {res['cpcm']}")
    except AcpiError as e:
        res["cpcm"] = str(e)

    # -- keyboard -----------------------------------------------------------------
    print("\n== Keyboard backlight (look at the keyboard)")
    ec.set(0x67, KB_COLOUR | 0xFFFFFF)
    ec.set(0x67, KB_BRIGHT | 0xFF)
    kb_ok = ask("Is the keyboard now WHITE at FULL brightness?")
    caps["kbd_rgb"] = kb_ok
    if kb_ok:
        order = {}
        for byte, shift in (("b16", 16), ("b8", 8), ("b0", 0)):
            ec.set(0x67, KB_COLOUR | (0xFF << shift))
            for colour in ("red", "green", "blue"):
                if ask(f"Is it {colour.upper()} now?"):
                    order[colour] = shift
                    break
        res["kbd_order"] = order
        caps["kbd_channel_shift"] = order
        levels = []
        for lvl in (0, 32, 96, 160, 255):
            ec.set(0x67, KB_COLOUR | 0xFFFFFF)
            ec.set(0x67, KB_BRIGHT | lvl)
            time.sleep(0.3)
            levels.append({"value": lvl, "cnt3d": hex(ec.get(0x3D))})
        res["kbd_levels"] = levels
        caps["kbd_bright_max"] = 255
        ec.set(0x67, KB_BRIGHT | 0)
        caps["kbd_off_works"] = ask("Brightness is set to 0: is the backlight OFF?")
        ec.set(0x67, KB_COLOUR | 0xFFFFFF)
        ec.set(0x67, KB_BRIGHT | 160)
        print("   (keyboard left white at medium brightness)")

    # -- performance profile ---------------------------------------------------------
    print("\n== Performance profile (0x79 sub 0x19), watching CPU/GPU power limits")
    psf4 = int(res["gets"].get("0x60", "0x0"), 16) if res["gets"].get("0x60", "").startswith("0x") else 0
    caps["perf_profile"] = bool(psf4 & 0x04)
    res["perf"] = {"before": rapl()}
    if caps["perf_profile"]:
        for n in range(4):
            ec.set(0x79, (0x19 << 24) | n)
            time.sleep(3)
            res["perf"][n] = rapl()
            print(f"   n={n}: {res['perf'][n]}")
        orig = res["cpcm"] if isinstance(res["cpcm"], int) and 0 <= res["cpcm"] <= 3 else 0
        ec.set(0x79, (0x19 << 24) | orig)
        print(f"   restored profile n={orig}")

    # -- switches --------------------------------------------------------------------
    print("\n== Webcam switch (SCMD 0x22)")
    r = toggle_test(ec, 0x22, cameras, settle=3)
    res["webcam"] = r
    caps["webcam_switch"] = r["works"]
    caps["webcam_off_value"] = r.get("off_value")
    print(f"   {r}")

    print("\n== Wireless switch (SCMD 0x20)")
    r = toggle_test(ec, 0x20, rfkill, settle=2)
    res["wireless"] = r
    caps["wireless_switch"] = r["works"]
    caps["wireless_off_value"] = r.get("off_value")
    print(f"   {r}")
    if not r["restored"]:
        print("   !! could not restore the original state; check Wi-Fi in Settings")

    print("\n== Touchpad switch (SCMD 0x2A). It will be re-enabled after 4 s automatically.")
    if ask("OK to test (keep a hand on the touchpad and try moving the pointer)?"):
        ec.set(0x2A, 0)
        time.sleep(4)
        ec.set(0x2A, 1)
        caps["touchpad_switch"] = ask("Did the touchpad STOP working for those 4 seconds?")

    caps["battery_limit"] = res["gets"].get("0x77", "").startswith("0x")
    res["caps"] = caps
    with open(out_path, "w") as f:
        json.dump(res, f, indent=1, default=str)
    os.chmod(out_path, 0o644)
    os.makedirs(CONFIG_DIR, exist_ok=True)
    with open(CAPS_PATH, "w") as f:
        json.dump(caps, f, indent=1)
    os.chmod(CAPS_PATH, 0o644)
    print(f"\nSaved {out_path} and {CAPS_PATH}")
    return 0
