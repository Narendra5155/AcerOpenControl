"""Client for the aocd Unix socket (protocol: docs/PROTOCOL.md)."""

from __future__ import annotations

import json
import os
import socket

SOCKET_PATH = os.environ.get("AOC_SOCKET", "/run/acer-open-control/aocd.sock")


class DaemonUnavailable(ConnectionError):
    pass


def request(req: dict, path: str | None = None, timeout: float = 5.0) -> dict:
    path = path or SOCKET_PATH
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
            s.settimeout(timeout)
            s.connect(path)
            s.sendall((json.dumps(req) + "\n").encode())
            buf = b""
            while not buf.endswith(b"\n"):
                chunk = s.recv(65536)
                if not chunk:
                    break
                buf += chunk
    except FileNotFoundError as e:
        raise DaemonUnavailable("The aocd service is not running (sudo systemctl start aocd)") from e
    except PermissionError as e:
        raise DaemonUnavailable(f"Permission denied opening {path}") from e
    except (ConnectionRefusedError, socket.timeout) as e:
        raise DaemonUnavailable(f"aocd is not responding: {e}") from e
    return json.loads(buf or b'{"ok": false, "error": "empty reply"}')


def describe_error(res: dict | None, err: Exception | None) -> str:
    """Human-readable message for a failed request."""
    if err is not None:
        return str(err)
    if res and res.get("code") == "forbidden":
        return ("Changing settings needs an administrator account. Ask an admin to run: "
                "sudo usermod -aG acer-open-control $USER")
    return (res or {}).get("error", "request failed")
