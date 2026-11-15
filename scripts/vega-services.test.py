#!/usr/bin/env python3
"""No SSH, listeners, hc terminals or real homes: fake subprocess and temp state."""
import contextlib
import io
import json
import os
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
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        env = patch.dict(os.environ, {"HOME": self.tmp.name, "XDG_STATE_HOME": self.tmp.name + "/state"})
        env.start()
        self.addCleanup(env.stop)

    def call_cli(self, args):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            helper.cli(args)
        return out.getvalue()

    def test_registration_lifetime_replacement_and_removal(self):
        rows = dict(listeners=[dict(pid=100, port=5187)])
        with patch.object(helper, "scan", return_value=rows) as scan, patch.object(helper, "process_identity", return_value="start-one"):
            args = ["register", "--port", "5187", "--title", "Spire", "--description", "Evidence browser", "--url", "https://127.0.0.1:5187/path?q=a#hash"]
            self.call_cli(args)
            scan.assert_called_once_with(enrich=False)
            entry = helper.registrations()[5187]
            self.assertEqual(entry["pid"], 100)
            self.assertEqual(entry["description"], "Evidence browser")
            self.assertEqual(json.loads(self.call_cli(["ls", "--json"]))["services"], [entry])
            self.assertIn('services[1]{port,pid,title,url}:', self.call_cli(["ls"]))
            self.call_cli(["register", "--port", "5187", "--pid", "100", "--title", "New name"])
            self.assertEqual(helper.registrations()[5187]["title"], "New name")
            with patch.object(helper, "process_identity", return_value="start-two"):
                self.assertEqual(helper.registrations(), {})
                self.assertFalse((helper.state_dir() / "5187.json").exists())
            self.call_cli(args)
            with patch.object(helper, "process_identity", return_value=None):
                self.assertEqual(helper.registrations(), {})
            self.call_cli(args)
            self.call_cli(["rm", "--port", "5187"])
            self.call_cli(["rm", "--port", "5187"])
            self.assertIn("0 live registrations", self.call_cli([]))
            self.assertEqual(json.loads(self.call_cli(["--json", "ls"]))["services"], [])

    def test_invalid_input_is_rejected_before_processes_or_state(self):
        bad = [[], ["--pid", "100"], ["--port", "0", "--title", "x"],
               ["--port", "80", "--title", ""], ["--port", "80", "--title", "x", "--pid", "-1"],
               ["--port", "80", "--title", "x", "--typo", "x"],
               ["--port", "80", "--title", "x", "--url", "https://example.com:80"],
               ["--port", "80", "--title", "x", "--url", "http://127.0.0.1:81/path"],
               ["--port", "80", "--title", "x", "--url", "javascript:alert(1)"],
               ["--port", "80", "--title", "x\nInjected"]]
        with patch.object(helper, "scan") as scan:
            for flags in bad:
                with self.assertRaises(helper.UsageError, msg=flags):
                    self.call_cli(["register", *flags])
            for args in (["ls", "--typo"], ["rm"], ["rm", "--port", "0"], ["unknown"]):
                with self.assertRaises(helper.UsageError):
                    self.call_cli(args)
            scan.assert_not_called()
        self.assertFalse(helper.state_dir().exists())

    def test_ambiguous_missing_and_wrong_listener_pid(self):
        with patch.object(helper, "scan", return_value=dict(listeners=[dict(pid=100, port=80), dict(pid=101, port=80)])):
            for flags in ([], ["--pid", "102"]):
                with self.assertRaises(ValueError):
                    self.call_cli(["register", "--port", "80", "--title", "x", *flags])
            with patch.object(helper, "process_identity", return_value="start"):
                self.call_cli(["register", "--port", "80", "--title", "x", "--pid", "101"])
                self.assertEqual(helper.registrations()[80]["pid"], 101)
        with patch.object(helper, "scan", return_value=dict(listeners=[])):
            with self.assertRaises(ValueError):
                self.call_cli(["register", "--port", "80", "--title", "x"])

    def test_process_identity_and_runtime_detection(self):
        for output, expected in [("S Tue Nov 10 12:00:00 2026", "Tue Nov 10 12:00:00 2026"), ("Z Tue Nov 10 12:00:00 2026", None), ("", None)]:
            with patch.object(helper, "command", return_value=result(output)):
                self.assertEqual(helper.process_identity(100), expected)
        for process in ("/opt/codex codex app-server", "/bin/claude claude", "node /lib/@openai/codex/bin/codex.js", "node /lib/@mariozechner/pi-coding-agent/dist/cli.js", "opencode serve"):
            self.assertTrue(helper.agent_runtime(process), process)
        for process in ("node vite", "node /repo/spire/server.js", "python3 /repo/app.py", "node node /repo/codex/server.js", "node node /repo/app.js --title codex"):
            self.assertFalse(helper.agent_runtime(process), process)

    def test_protocol_detection_uses_only_bounded_loopback_head(self):
        listener = dict(host="0.0.0.0", port=5187)
        with patch.object(helper.http.client, "HTTPConnection") as http, patch.object(helper.http.client, "HTTPSConnection") as https:
            self.assertEqual(helper.protocol(listener), "http")
            http.assert_called_once_with("127.0.0.1", 5187, timeout=0.4)
            http.return_value.request.assert_called_once_with("HEAD", "/")
            http.return_value.close.assert_called_once()
            https.assert_not_called()
            http.return_value.getresponse.side_effect = helper.http.client.BadStatusLine("SSH")
            self.assertEqual(helper.protocol(dict(host="::", port=8443)), "https")
            self.assertEqual(https.call_args.args, ("::1", 8443))
            https.return_value.request.side_effect = OSError("not HTTP")
            self.assertEqual(helper.protocol(listener), "tcp")
            http.reset_mock()
            self.assertEqual(helper.protocol(dict(host="10.0.0.1", port=80)), "tcp")
            http.assert_not_called()

    def test_linux_and_mac_snapshots(self):
        def linux(args):
            if args[0] == "ps":
                return result("100 10\n10 1\n")
            if args[0] == "ss":
                return result('LISTEN 0 128 127.0.0.1:5187 0.0.0.0:* users:(("node",pid=100,fd=4))\n')
            self.fail(args)
        with patch.object(helper, "protocol", return_value="http"), patch.object(helper, "command", linux), patch.object(helper.shutil, "which", return_value="ss"), patch.object(helper.os, "readlink", return_value="/repo"):
            data = helper.scan()
            self.assertEqual(data["listeners"], [dict(pid=100, name="node", host="127.0.0.1", port=5187, scheme="http", agent=False)])
            self.assertEqual(data["parents"]["100"], 10)
            self.assertEqual(data["cwds"]["100"], "/repo")
        def mac(args):
            if args[0] == "ps":
                return result("100 10\n")
            if "cwd" in args:
                return result("p100\nn/repo\n")
            return result("p100\ncnode\nn[::1]:5187\nn127.0.0.1:5187\n")
        with patch.object(helper, "protocol", return_value="tcp"), patch.object(helper, "command", mac), patch.object(helper.shutil, "which", side_effect=lambda name: None if name == "ss" else "lsof"), patch.object(helper.os, "readlink", side_effect=OSError):
            data = helper.scan()
            self.assertEqual(len(data["listeners"]), 2)
            self.assertEqual(data["cwds"]["100"], "/repo")

    def test_scan_merges_only_matching_pid_and_port(self):
        def processes(args, **kwargs):
            if args[0] == "ps":
                return result("100 10 /bin/codex codex app-server\n101 10 node vite\n")
            return result('LISTEN 0 128 127.0.0.1:5187 0.0.0.0:* users:(("node",pid=100,fd=4))\n'
                          'LISTEN 0 128 127.0.0.1:9000 0.0.0.0:* users:(("node",pid=101,fd=5))\n')
        entries = {5187: dict(pid=100, title="Spire", description="Evidence", url="https://127.0.0.1:5187/path"),
                   9000: dict(pid=999, title="Wrong process", description="", url="")}
        with patch.object(helper, "registrations", return_value=entries), patch.object(helper, "command", processes), patch.object(helper.shutil, "which", return_value="ss"), patch.object(helper.os, "readlink", return_value="/repo"), patch.object(helper, "protocol", return_value="http"):
            rows = helper.scan()["listeners"]
            self.assertEqual(rows[0]["title"], "Spire")
            self.assertTrue(rows[0]["registered"])
            self.assertTrue(rows[0]["agent"])
            self.assertNotIn("title", rows[1])
            self.assertFalse(rows[1]["agent"])
            self.assertTrue(rows[2]["gone"])
            self.assertEqual(rows[2]["title"], "Wrong process")

    def test_main_structured_errors_and_help_without_state(self):
        for args, code in [(["cli", "ls", "--typo"], 2), (["cli", "register", "--port", "80", "--title", "x"], 1)]:
            out = io.StringIO()
            with patch.object(sys, "argv", ["helper", *args, "--json"]), contextlib.redirect_stdout(out), patch.object(helper, "scan", return_value=dict(listeners=[])):
                self.assertEqual(helper.main(), code)
                self.assertIn("hint", json.loads(out.getvalue())["error"])
        for args in (["--help"], ["register", "--help"], ["ls", "--help"], ["rm", "--help"]):
            with contextlib.redirect_stdout(io.StringIO()), self.assertRaises(SystemExit) as caught:
                helper.cli(args)
            self.assertEqual(caught.exception.code, 0)
        self.assertFalse(helper.state_dir().exists())

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

    def test_bundle_registration_and_same_port_forward_conflicts(self):
        with patch.object(helper, "scan", return_value=dict(listeners=[dict(pid=31560, port=5320)])), patch.object(helper, "process_identity", return_value="start"):
            self.call_cli(["register", "--port", "5320", "--also-forward", "8367", "--title", "Classic app"])
            self.assertEqual(helper.registrations()[5320]["also_forward"], [8367])
            for extra in ("5320", "0"):
                with self.assertRaises(helper.UsageError):
                    self.call_cli(["register", "--port", "5320", "--also-forward", extra, "--title", "x"])
        folder = Path(self.tmp.name)
        with patch.object(helper, "owner_dir", return_value=folder), patch.object(helper.socket, "gethostname", return_value="proteus"), patch.object(helper.socket, "socket") as sock, patch.object(helper, "command", return_value=result()) as ssh:
            self.assertEqual(helper.forward("start", "test", "1", "ganymede", "127.0.0.1", 5320, "same", "8367"), {"local":5320})
            args = ssh.call_args.args[0]
            self.assertEqual(args.count("-L"), 2)
            self.assertIn("127.0.0.1:5320:127.0.0.1:5320", args)
            self.assertIn("127.0.0.1:8367:127.0.0.1:8367", args)
            self.assertIn("ExitOnForwardFailure=yes", args)
            ssh.reset_mock()
            sock.return_value.bind.side_effect = [None, OSError("Address already in use")]
            with self.assertRaisesRegex(RuntimeError, "127.0.0.1:8367 unavailable"):
                helper.forward("start", "test", "2", "ganymede", "127.0.0.1", 5320, "same", "8367")
            ssh.assert_not_called()
            sock.return_value.bind.side_effect = None
            ssh.return_value = result(stderr="bind 127.0.0.1:5320: Address already in use", code=255)
            with self.assertRaisesRegex(RuntimeError, "Local port conflict"):
                helper.forward("start", "test", "3", "ganymede", "127.0.0.1", 5320, "same", "8367")
            self.assertEqual(ssh.call_count, 1, "same-port collision must not fall back to random ports")


if __name__ == "__main__":
    unittest.main()
