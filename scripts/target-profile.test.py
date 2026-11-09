#!/usr/bin/env python3
"""macOS regression: the configured dev profile must not retain linked objects."""

import os
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib
import unittest


@unittest.skipUnless(sys.platform == "darwin", "macOS split-debuginfo retention")
class ProfileTests(unittest.TestCase):
    def test_incremental_builds_preserve_panics_without_retaining_objects(self):
        root = Path(__file__).resolve().parent.parent
        profile = tomllib.loads((root / "Cargo.toml").read_text())["profile"]["dev"]
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            source = directory / "retention_probe.rs"
            source.write_text('fn main() { assert_eq!(1, 2, "assertion remains readable"); }\n')
            deps = directory / "deps"
            deps.mkdir()
            for generation in range(2):
                command = [
                    "rustc", str(source), "--out-dir", str(deps),
                    "-C", "extra-filename=-" + str(generation),
                    "-C", "incremental=" + str(directory / "incremental"),
                    "-C", "debuginfo=" + str(profile["debug"]),
                    "-C", "split-debuginfo=" + profile.get("split-debuginfo", "unpacked"),
                ]
                subprocess.run(command, check=True, capture_output=True)
                with os.scandir(deps) as entries:
                    objects = [entry.name for entry in entries if entry.name.endswith(".o")]
                self.assertEqual(objects, [])
            panic = subprocess.run(
                [str(deps / "retention_probe-1")],
                env=dict(os.environ, RUST_BACKTRACE="1"), capture_output=True, text=True,
            )
            self.assertEqual(panic.returncode, 101)
            self.assertIn("assertion remains readable", panic.stderr)
            self.assertIn("retention_probe.rs:1:", panic.stderr)
            # The panic header embeds file/line even without debug info. Check a frame too.
            frames = panic.stderr.split("stack backtrace:", 1)[1]
            self.assertIn("retention_probe.rs:1:", frames)


if __name__ == "__main__":
    unittest.main()
