"""Clevo WMI (WMBB) fan interface for the Acer Aspire 7 A715-79G (Clevo NP50RN).

Command IDs come from the DSDT (acpi-analysis/dsdt.dsl, \\_SB.WMI.GCMD/SCMD).
Fan set/auto/readback verified on hardware (BIOS 1.07.01RAC5).
"""

from __future__ import annotations

from dataclasses import dataclass, asdict
from typing import Optional

from .acpi import AcpiCall, AcpiError, as_int

# --- WMBB command IDs -------------------------------------------------------
CMD_GET_FEATURES_1 = 0x52      # PSF1
CMD_GET_PSF4 = 0x60
CMD_GET_FANINFO1 = 0x63
CMD_GET_FANINFO2 = 0x64
CMD_GET_FANINFO3 = 0x6E
CMD_SET_FAN_DUTY = 0x68        # byte n -> fan n+1, raw 0..255 (EC C1 0n)
CMD_SET_FAN_AUTO = 0x69        # bitmask of fans returned to EC control
CMD_SET_INDEX = 0x6B           # INDX = (size_code << 16) | offset
CMD_READ_EC = 0x73             # read EC RAM @ 0xFE0B0100 + INDX.offset
CMD_GET_FEATURES_2 = 0x7A      # PSF2

FAN_ALL_MASK = 0x0F

# --- EC RAM offsets (DSDT field list at EC.RAM) ------------------------------
EC_DUT1 = 0xCE
EC_DUT2 = 0xCF
EC_RPM1 = 0xD0
EC_RPM2 = 0xD2
EC_RPM4 = 0xD4
EC_RPM3 = 0xE0

# --- RPM decoding: CONFIRMED by probe on 2026-11-08 ---------------------------
# Big-endian tach period, rpm = 2156220 / raw.
# Duty 255 -> fan1 ~6360 / fan2 ~5780 rpm; duty 76 -> ~2500 / ~2370 rpm.
# The EC keeps a manual duty (it didn't revert within 20 s); 0x69 hands control back cleanly.
RPM_DIVIDEND = 2156220
RPM_BIG_ENDIAN = True

_SIZE_CODE = {1: 0, 2: 1, 4: 2}


def pct_to_raw(pct: float) -> int:
    pct = max(0.0, min(100.0, float(pct)))
    return round(pct * 255 / 100)


def raw_to_pct(raw: int) -> int:
    return round((raw & 0xFF) * 100 / 255)


def decode_rpm(lo_first: bytes, big_endian: bool = RPM_BIG_ENDIAN,
               dividend: int = RPM_DIVIDEND) -> int:
    """Decode two bytes read from EC RAM starting at the RPM field."""
    if len(lo_first) < 2:
        return 0
    raw = int.from_bytes(lo_first[:2], "big" if big_endian else "little")
    if raw in (0, 0xFFFF):
        return 0
    if dividend:
        return int(dividend / raw)
    return raw


@dataclass
class FanStatus:
    duty1: int          # percent
    duty2: int
    rpm1: int
    rpm2: int
    raw: dict

    def to_dict(self) -> dict:
        return asdict(self)


class ClevoEC:
    def __init__(self, acpi: Optional[AcpiCall] = None):
        self.acpi = acpi or AcpiCall()

    # -- generic -------------------------------------------------------------
    def get(self, cmd: int, arg: int = 0) -> int:
        return as_int(self.acpi.wmbb(cmd, arg))

    def set(self, cmd: int, arg: int = 0) -> int:
        """SCMD calls return their own command ID on success. A return of
        Ones (0xffffffff) means the firmware couldn't take the EC mutex."""
        ret = as_int(self.acpi.wmbb(cmd, arg))
        if ret != cmd:
            raise AcpiError(f"WMBB set {cmd:#x} failed (returned {ret:#x})")
        return ret

    def read_ec(self, offset: int, size: int = 1) -> bytes:
        """Read 1, 2 or 4 bytes of EC RAM through the firmware (GCMD 0x73)."""
        if size not in _SIZE_CODE:
            raise ValueError("size must be 1, 2 or 4")
        if not 0 <= offset <= 0xFF - (size - 1):
            raise ValueError("offset out of range")
        self.acpi.wmbb(CMD_SET_INDEX, (_SIZE_CODE[size] << 16) | offset)
        res = self.acpi.wmbb(CMD_READ_EC, 0)
        if isinstance(res, (bytes, bytearray)):
            return bytes(res[:size])
        return as_int(res).to_bytes(4, "little")[:size]

    def read_ec_range(self, start: int, end_inclusive: int) -> bytes:
        return b"".join(self.read_ec(o, 1) for o in range(start, end_inclusive + 1))

    # -- fans ----------------------------------------------------------------
    def set_duty(self, fan1_pct: float, fan2_pct: Optional[float] = None) -> None:
        if fan2_pct is None:
            fan2_pct = fan1_pct
        arg = pct_to_raw(fan1_pct) | (pct_to_raw(fan2_pct) << 8)
        self.set(CMD_SET_FAN_DUTY, arg)

    def set_auto(self, mask: int = FAN_ALL_MASK) -> None:
        self.set(CMD_SET_FAN_AUTO, mask)

    def read_status(self) -> FanStatus:
        duty = self.read_ec(EC_DUT1, 2)
        rpm1 = self.read_ec(EC_RPM1, 2)
        rpm2 = self.read_ec(EC_RPM2, 2)
        return FanStatus(
            duty1=raw_to_pct(duty[0]),
            duty2=raw_to_pct(duty[1]),
            rpm1=decode_rpm(rpm1),
            rpm2=decode_rpm(rpm2),
            raw={"dut": duty.hex(), "rpm1": rpm1.hex(), "rpm2": rpm2.hex()},
        )
