#!/usr/bin/env python3
"""Maintenance generator behavior, without a live app."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("herdr-app-layout.py")
spec = importlib.util.spec_from_file_location("layout_generator", SCRIPT)
generator = importlib.util.module_from_spec(spec)
spec.loader.exec_module(generator)


class GeneratorTest(unittest.TestCase):
    def test_anchor_hash_and_unrelated_structure_survive(self):
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            (home / "layouts").mkdir()
            main = {"type": "column", "custom": {"keep": True}, "children": [
                {"id": "status", "type": "block", "content": {"mod": "workspace-status"}}]}
            (home / "layouts/main.json").write_text(json.dumps(main))
            (home / "installed.json").write_text('{"layout:main":{"json":"recorded-hash"}}')
            result = json.loads(generator.generate(home, "0.1.13"))
            self.assertEqual(result["children"][0], main["children"][0])
            self.assertEqual(result["children"][1]["content"], {"mod": "vega"})
            self.assertEqual(result["custom"], main["custom"])
            self.assertEqual(result["vega_source"]["installed_hash"], "recorded-hash")
            # A changed upstream anchor refuses and keeps the previously reviewed output.
            output = home / "vega.json"
            output.write_text("previous reviewed output")
            main["children"][0]["id"] = "renamed"
            (home / "layouts/main.json").write_text(json.dumps(main))
            reply = subprocess.run([sys.executable, str(SCRIPT), "--app-home", str(home),
                "--app-version", "0.1.14", "--output", str(output), "--write"], capture_output=True, text=True)
            self.assertEqual(reply.returncode, 1)
            self.assertIn("anchor", reply.stdout)
            self.assertEqual(output.read_text(), "previous reviewed output")


if __name__ == "__main__":
    unittest.main()
