#!/usr/bin/env python3
"""Cache maintenance regression tests run only on temporary fixture targets."""

import importlib.util
import os
from pathlib import Path
import subprocess
import sys
import time
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "target_cache", Path(__file__).with_name("target-cache.py")
)
cache = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cache)


class CacheTests(unittest.TestCase):
    def setUp(self):
        self.fixture = tempfile.TemporaryDirectory()
        self.addCleanup(self.fixture.cleanup)
        self.target = Path(self.fixture.name) / "target"
        self.deps = self.target / "debug/deps"
        self.deps.mkdir(parents=True)
        self.now = time.time()

    def old(self, path):
        if path.is_dir() and not path.is_symlink():
            for child in path.iterdir():
                self.old(child)
        timestamp = self.now - cache.KEEP_SECONDS - 1
        os.utime(path, (timestamp, timestamp), follow_symlinks=False)
        return path

    def file(self, name, old=True):
        path = self.deps / name
        path.write_text("cache data")
        return self.old(path) if old else path

    def run_sweep(self, **kwargs):
        return cache.sweep(self.target, now=self.now, **kwargs)

    def test_old_family_removed_and_fresh_sibling_protects_whole_family(self):
        stale = self.file("libstale-1111111111111111.rlib")
        obj = self.file("stale-1111111111111111.hash.rcgu.o")
        active = self.file("libactive-2222222222222222.rlib")
        self.file("active-2222222222222222.d", old=False)
        unknown = self.file("unrecognized.keep")
        result = self.run_sweep()
        self.assertEqual(result["removed"], 2)
        self.assertFalse(stale.exists())
        self.assertFalse(obj.exists())
        self.assertTrue(active.exists())
        self.assertTrue(unknown.exists())

    def test_incremental_and_dsym_trees_checked_recursively(self):
        stale = self.deps / "test-1111111111111111.dSYM"
        (stale / "Contents/Resources/DWARF").mkdir(parents=True)
        (stale / "Contents/Resources/DWARF/test").write_text("debug info")
        self.old(stale)
        incremental = self.target / "debug/incremental"
        old = incremental / "old-abc123/session"
        old.mkdir(parents=True)
        (old / "object.o").write_text("old object")
        self.old(old.parent)
        active = incremental / "active-def456/session"
        active.mkdir(parents=True)
        self.old(active.parent)
        (active / "new.o").write_text("recent object")
        result = self.run_sweep()
        self.assertEqual(result["deps_before"], 1)
        self.assertEqual(result["deps_removed"], 1)
        self.assertFalse(stale.exists())
        self.assertFalse(old.parent.exists())
        self.assertTrue((active / "new.o").exists())

    def test_dry_run_and_scan_limit_do_not_delete_incomplete_families(self):
        first = self.file("libold-1111111111111111.rlib")
        self.file("old-1111111111111111.d")
        dry = self.run_sweep(dry_run=True)
        self.assertEqual(dry["removed"], 0)
        self.assertEqual(dry["would_remove"], 2)
        limited = self.run_sweep(max_scan=1)
        self.assertTrue(limited["scan_limited"])
        self.assertEqual(limited["removed"], 0)
        self.assertTrue(first.exists())

    def test_delete_budget_keeps_whole_families_and_progresses_next_run(self):
        for digit in ["1", "2", "3"]:
            self.file("libold-" + digit * 16 + ".rlib")
            self.file("old-" + digit * 16 + ".d")
        first = self.run_sweep(max_delete=3)
        self.assertEqual(first["removed"], 2)
        self.assertTrue(first["delete_limited"])
        self.assertFalse((self.deps / ("old-" + "1" * 16 + ".d")).exists())
        second = self.run_sweep(max_delete=3)
        self.assertEqual(second["removed"], 2)
        self.assertFalse((self.deps / ("old-" + "2" * 16 + ".d")).exists())

    def test_nested_recent_member_keeps_old_dsym_and_scan_budget_is_shared(self):
        bundle = self.deps / "old-1111111111111111.dSYM"
        bundle.mkdir()
        child = bundle / "new"
        child.write_text("fresh")
        self.old(bundle)
        os.utime(child, (self.now, self.now))
        self.assertEqual(self.run_sweep()["removed"], 0)
        self.old(bundle)
        result = self.run_sweep(max_scan=1)
        self.assertTrue(result["scan_limited"])
        self.assertTrue(child.exists())

    def test_links_are_not_followed_and_symlinked_roots_are_refused(self):
        outside = Path(self.fixture.name) / "outside"
        outside.mkdir()
        keep = outside / "keep"
        keep.write_text("user data")
        link = self.deps / "old-1111111111111111.dSYM"
        link.symlink_to(outside, target_is_directory=True)
        self.old(link)
        self.run_sweep()
        self.assertFalse(link.is_symlink())
        self.assertEqual(keep.read_text(), "user data")
        incremental = self.target / "debug/incremental"
        incremental.symlink_to(outside, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "symlinks"):
            self.run_sweep()

    def test_busy_build_skips_all_deletion(self):
        old = self.file("libold-1111111111111111.rlib")
        with cache.build_lock(self.target):
            result = self.run_sweep()
        self.assertIn("Cargo build running", result["status"])
        self.assertEqual(result["removed"], 0)
        self.assertTrue(old.exists())

    def test_live_cargo_lock_skips_without_waiting(self):
        with tempfile.TemporaryDirectory() as directory:
            with cache.build_lock(directory) as acquired:
                self.assertTrue(acquired)
                # A separate process exercises the same interprocess lock as Cargo.
                code = (
                    "import fcntl,sys; f=open(sys.argv[1], 'a'); "
                    "fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)"
                )
                result = subprocess.run(
                    [sys.executable, "-c", code, str(Path(directory) / "debug/.cargo-lock")],
                    capture_output=True,
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(b"BlockingIOError", result.stderr)
                with cache.build_lock(directory) as second:
                    self.assertFalse(second)
            with cache.build_lock(directory) as acquired:
                self.assertTrue(acquired)

    def test_profile_symlink_is_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            outside = root / "outside"
            outside.mkdir()
            target = root / "target"
            target.mkdir()
            (target / "debug").symlink_to(outside, target_is_directory=True)
            with self.assertRaisesRegex(ValueError, "symlinks"):
                with cache.build_lock(target):
                    self.fail("must not lock outside the target")
            self.assertEqual(list(outside.iterdir()), [])

    def test_external_target_is_refused(self):
        with patch.dict(os.environ, {"CARGO_TARGET_DIR": "/tmp/other-worktree-target"}):
            with self.assertRaisesRegex(ValueError, "this checkout"):
                cache.checkout_target()


if __name__ == "__main__":
    unittest.main()
