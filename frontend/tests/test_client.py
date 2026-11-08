import json
import os
import socketserver
import tempfile
import threading
import unittest

from aoc_gtk import client


class _Echo(socketserver.StreamRequestHandler):
    def handle(self):
        for line in self.rfile:
            req = json.loads(line)
            if req.get("cmd") == "set_mode":
                resp = {"ok": False, "code": "forbidden", "error": "permission denied"}
            else:
                resp = {"ok": True, "echo": req}
            self.wfile.write((json.dumps(resp) + "\n").encode())


class ClientTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dir = tempfile.mkdtemp()
        cls.path = os.path.join(cls.dir, "s.sock")
        cls.srv = socketserver.ThreadingUnixStreamServer(cls.path, _Echo)
        threading.Thread(target=cls.srv.serve_forever, daemon=True).start()

    @classmethod
    def tearDownClass(cls):
        cls.srv.shutdown()
        cls.srv.server_close()

    def test_roundtrip(self):
        r = client.request({"cmd": "status"}, path=self.path)
        self.assertTrue(r["ok"])
        self.assertEqual(r["echo"], {"cmd": "status"})

    def test_forbidden_message(self):
        r = client.request({"cmd": "set_mode", "mode": "max"}, path=self.path)
        self.assertIn("administrator", client.describe_error(r, None))

    def test_missing_daemon(self):
        with self.assertRaises(client.DaemonUnavailable):
            client.request({"cmd": "status"}, path=os.path.join(self.dir, "nope.sock"))

    def test_error_from_exception(self):
        self.assertEqual(client.describe_error(None, RuntimeError("boom")), "boom")


class ImportTest(unittest.TestCase):
    def test_gui_modules_import(self):
        try:
            import gi
            gi.require_version("Gtk", "4.0")
            gi.require_version("Adw", "1")
        except (ImportError, ValueError):
            self.skipTest("GTK4/libadwaita introspection not installed")
        from aoc_gtk import app, compat, pages  # noqa: F401
        self.assertTrue(hasattr(compat, "ModeSwitcher"))


if __name__ == "__main__":
    unittest.main()
