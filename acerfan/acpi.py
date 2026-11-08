"""Thin wrapper around the acpi_call kernel module (/proc/acpi/call).

acpi_call is a single shared request/response file, so every call is
serialised with an flock on a lock file. That covers the daemon, the CLI
and the probe all at once.
"""

from __future__ import annotations

import fcntl
import os
import re
import subprocess
from contextlib import contextmanager
from typing import Iterator, Union

PROC_CALL = "/proc/acpi/call"
LOCK_PATH = "/run/acerfan-acpi.lock"

# Clevo WMI method; GUID ABBC0F6D-8EA1-11D1-00A0-C90629100000, object "BB".
WMBB_PATH = r"\_SB.WMI.WMBB"

AcpiResult = Union[int, bytes, str, None]


class AcpiError(RuntimeError):
    pass


def encode_int(value: int) -> str:
    if value < 0:
        raise ValueError("negative ACPI integer")
    return hex(value)


def encode_buffer(data: bytes) -> str:
    """acpi_call buffer syntax: 'b' followed by hex byte pairs."""
    return "b" + data.hex()


def u32_buffer(value: int) -> bytes:
    return (value & 0xFFFFFFFF).to_bytes(4, "little")


_INT_RE = re.compile(r"^0x[0-9a-fA-F]+$")
_BUF_RE = re.compile(r"^\{(.*)\}$", re.S)


def parse_reply(raw: str) -> AcpiResult:
    """Parse the text acpi_call leaves in /proc/acpi/call after a call."""
    text = raw.strip("\x00").strip()
    if not text or text == "not called":
        return None
    if text.startswith("Error:"):
        raise AcpiError(text)
    if _INT_RE.match(text):
        return int(text, 16)
    m = _BUF_RE.match(text)
    if m:
        inner = m.group(1).strip()
        if not inner:
            return b""
        parts = [p.strip() for p in inner.split(",") if p.strip()]
        try:
            return bytes(int(p, 16) & 0xFF for p in parts)
        except ValueError:
            # Package of mixed things: hand back the text.
            return text
    if text.startswith('"') and text.endswith('"'):
        return text[1:-1]
    return text


def as_int(result: AcpiResult) -> int:
    if isinstance(result, int):
        return result
    if isinstance(result, (bytes, bytearray)):
        return int.from_bytes(bytes(result[:8]), "little")
    raise AcpiError(f"expected integer result, got {result!r}")


def ensure_module() -> None:
    if os.path.exists(PROC_CALL):
        return
    subprocess.run(["modprobe", "acpi_call"], check=False,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    if not os.path.exists(PROC_CALL):
        raise AcpiError(
            "/proc/acpi/call is missing: load it with 'sudo modprobe acpi_call' "
            "(package acpi-call-dkms)")


@contextmanager
def _locked() -> Iterator[None]:
    fd = os.open(LOCK_PATH, os.O_RDWR | os.O_CREAT, 0o600)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX)
        yield
    finally:
        fcntl.flock(fd, fcntl.LOCK_UN)
        os.close(fd)


class AcpiCall:
    def __init__(self, proc_path: str = PROC_CALL):
        self.proc_path = proc_path

    def call(self, method: str, *args: str) -> AcpiResult:
        line = " ".join((method,) + args)
        with _locked():
            with open(self.proc_path, "w") as f:
                f.write(line)
            with open(self.proc_path, "r") as f:
                raw = f.read()
        return parse_reply(raw)

    def wmbb(self, cmd: int, arg: int = 0) -> AcpiResult:
        """Call WMBB(0, cmd, <4-byte buffer arg>), the Clevo GCMD/SCMD dispatcher.

        The firmware runs SizeOf(Arg2), so the argument has to be a buffer,
        not an integer.
        """
        return self.call(WMBB_PATH, "0x0", encode_int(cmd), encode_buffer(u32_buffer(arg)))
