#!/usr/bin/env python3
"""No SSH, listeners, hc terminals or real homes: fake subprocess and temp state."""
import importlib.util
from pathlib import Path
from types import SimpleNamespace
import tempfile
import sys
sys.dont_write_bytecode = True
import unittest
from unittest.mock import patch

SOURCE = Path(__file__).resolve().parents[1] / "config/herdr-app/mods/vega-services/helper.py"
spec = importlib.util.spec_from_file_location("services", SOURCE)
helper = importlib.util.module_from_spec(spec)
spec.loader.exec_module(helper)


def result(stdout="", stderr="", code=0):
    return SimpleNamespace(stdout=stdout, stderr=stderr, returncode=code)


class ServicesTest(unittest.TestCase):
    def test_linux_and_mac_snapshots(self):
        def linux(args):
            if args[0] == "ps":
                return result("100 10\n10 1\n")
            if args[0] == "ss":
                return result('LISTEN 0 128 127.0.0.1:5187 0.0.0.0:* users:(("node",pid=100,fd=4))\n')
            self.fail(args)
        with patch.object(helper, "command", linux), patch.object(helper.shutil, "which", return_value="ss"), patch.object(helper.os, "readlink", return_value="/repo"):
            data = helper.scan()
            self.assertEqual(data["listeners"], [dict(pid=100, name="node", host="127.0.0.1", port=5187)])
            self.assertEqual(data["parents"]["100"], 10)
            self.assertEqual(data["cwds"]["100"], "/repo")
        def mac(args):
            if args[0] == "ps":
                return result("100 10\n")
            if "cwd" in args:
                return result("p100\nn/repo\n")
            return result("p100\ncnode\nn[::1]:5187\nn127.0.0.1:5187\n")
        with patch.object(helper, "command", mac), patch.object(helper.shutil, "which", side_effect=lambda name: None if name == "ss" else "lsof"), patch.object(helper.os, "readlink", side_effect=OSError):
            data = helper.scan()
            self.assertEqual(len(data["listeners"]), 2)
            self.assertEqual(data["cwds"]["100"], "/repo")

    def test_ssh_lifecycle_bind_retry_and_trust(self):
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            calls = []
            class Socket:
                def bind(self, addr):
                    self.addr = addr
                def getsockname(self):
                    return ("127.0.0.1", 40518)
                def close(self):
                    pass
            def ssh(args):
                calls.append(args)
                path = Path(args[args.index("-S") + 1])
                if "check" in args:
                    return result(code=0 if path.exists() else 255)
                if "-O" in args:
                    path.unlink(missing_ok=True)
                    return result()
                if len(calls) == 1:
                    return result(stderr="Address already in use", code=255)
                path.touch()
                return result()
            with patch.object(helper, "owner_dir", return_value=folder), patch.object(helper, "command", ssh), patch.object(helper.socket, "gethostname", return_value="proteus"), patch.object(helper.socket, "socket", Socket):
                self.assertEqual(helper.forward("start", "test", "1", "ganymede", "127.0.0.1", 5187), {"local":40518})
                self.assertEqual(len(calls), 2)
                self.assertTrue(helper.forward("status", "test", "1")["alive"])
                self.assertIn("127.0.0.1:40518:127.0.0.1:5187", calls[1])
                self.assertIn("BatchMode=yes", calls[1])
                self.assertIn("StrictHostKeyChecking=yes", calls[1])
                helper.forward("stop", "test", "1")
                self.assertFalse((folder / "s1").exists())
                for target in ("puck", "user@puck", "-oProxyCommand=bad", "unknown", "127.0.0.1"):
                    with self.assertRaises(ValueError):
                        helper.forward("start", "test", "2", target, "127.0.0.1", 5187)
                with patch.object(helper.socket, "gethostname", return_value="puck"):
                    with self.assertRaises(ValueError):
                        helper.forward("start", "test", "2", "ganymede", "127.0.0.1", 5187)
                (folder / "s3").touch()
                helper.forward("close", "test")
                self.assertFalse((folder / "s3").exists())
                with self.assertRaisesRegex(ValueError, "Panel closed"):
                    helper.forward("start", "test", "4", "ganymede", "127.0.0.1", 5187)

    def test_close_during_ssh_start_cleans_up(self):
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            def ssh(args):
                path = Path(args[args.index("-S")+1])
                if "-O" in args:
                    path.unlink()
                else:
                    path.touch()
                    (folder / "closed").touch()
                return result()
            with patch.object(helper, "owner_dir", return_value=folder), patch.object(helper, "command", ssh), patch.object(helper.socket, "gethostname", return_value="proteus"), patch.object(helper.socket, "socket") as socket:
                socket.return_value.getsockname.return_value = ("127.0.0.1", 40518)
                with self.assertRaisesRegex(ValueError, "Panel closed"):
                    helper.forward("start", "test", "1", "ganymede", "127.0.0.1", 5187)
                self.assertFalse((folder / "s1").exists())


if __name__ == "__main__":
    unittest.main()
