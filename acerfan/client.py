"""Client for the acerfand Unix socket (used by the CLI, GUI and tray)."""

from __future__ import annotations

import json
import socket

from .daemon import SOCK_PATH


class DaemonUnavailable(ConnectionError):
    pass


def request(req: dict, path: str = SOCK_PATH, timeout: float = 5.0) -> dict:
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
        raise DaemonUnavailable("acerfand is not running (sudo systemctl start acerfan)") from e
    except PermissionError as e:
        raise DaemonUnavailable(
            "permission denied: add yourself to the 'acerfan' group and log in again") from e
    except (ConnectionRefusedError, socket.timeout) as e:
        raise DaemonUnavailable(f"acerfand not responding: {e}") from e
    return json.loads(buf or b'{"ok": false, "error": "empty reply"}')
