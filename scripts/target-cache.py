#!/usr/bin/env python3
"""Bounded checkout-local eviction; see docs/design/build-cache.md."""

import argparse
import contextlib
import fcntl
import json
import os
from pathlib import Path
import re
import stat
import sys
import time

MAX_SCAN = 100_000
MAX_DELETE = 5_000
KEEP_SECONDS = 7 * 24 * 60 * 60
WARN_ENTRIES = 50_000
HASH = re.compile(r"-([0-9a-f]{16})(?:\.|$)")


class ScanLimit(Exception):
    pass


class Recent(Exception):
    pass


class Budget:
    def __init__(self, limit):
        self.left = limit

    def entries(self, directory):
        try:
            with os.scandir(directory) as entries:
                result = []
                for entry in entries:
                    if self.left == 0:
                        raise ScanLimit
                    self.left -= 1
                    result.append(Path(entry.path))
                return sorted(result)
        except FileNotFoundError:
            return []


@contextlib.contextmanager
def build_lock(target):
    """Use Cargo's own profile lock; never wait for or overlap a build."""
    profile = Path(target) / "debug"
    if profile.is_symlink() or (profile / ".cargo-lock").is_symlink():
        raise ValueError("Cargo profile and lock must not be symlinks")
    profile.mkdir(parents=True, exist_ok=True)
    with (profile / ".cargo-lock").open("a") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            yield False
            return
        try:
            yield True
        finally:
            fcntl.flock(lock, fcntl.LOCK_UN)


def checkout_target():
    """Never honor a shared/global override for destructive maintenance."""
    root = Path(__file__).resolve().parent.parent
    target = root / "target"
    configured = Path(os.environ.get("CARGO_TARGET_DIR", target)).resolve()
    if configured != target or target.is_symlink():
        raise ValueError("target must be this checkout's real target directory")
    return target


def old_tree(path, cutoff, budget):
    """Inspect the whole family before deleting any of it; never follow links."""
    pending = [(path, False)]
    result = []
    while pending:
        current, visited = pending.pop()
        if visited:
            result.append((current, True))
            continue
        metadata = current.lstat()
        if metadata.st_mtime >= cutoff:
            raise Recent
        if stat.S_ISDIR(metadata.st_mode):
            pending.append((current, True))
            pending.extend((child, False) for child in reversed(budget.entries(current)))
        else:
            result.append((current, False))
    return result


def sweep(target, *, now=None, dry_run=False, max_scan=MAX_SCAN, max_delete=MAX_DELETE):
    target = Path(target)
    result = dict(
        status="ok", deps_before=None, removed=0, deps_removed=0,
        would_remove=0, scan_limited=False, delete_limited=False,
    )
    if target.is_symlink():
        raise ValueError("target must not be a symlink")
    for name in ["debug", "debug/deps", "debug/incremental"]:
        if (target / name).is_symlink():
            raise ValueError("Cargo profile, deps and incremental must not be symlinks")
    if not (target / "debug").exists():
        result["status"] = "not built"
        result["deps_before"] = 0
        return result
    with build_lock(target) as acquired:
        if not acquired:
            result["status"] = "skipped: Cargo build running"
            return result
        budget = Budget(max_scan)
        cutoff = (time.time() if now is None else now) - KEEP_SECONDS
        try:
            # An incomplete listing cannot establish a family's youngest member:
            # fail closed rather than evict an unseen fresh sibling.
            deps = budget.entries(target / "debug/deps")
            result["deps_before"] = len(deps)
            families = {}
            for path in deps:
                match = HASH.search(path.name)
                if match:
                    families.setdefault("deps:" + match[1], []).append(path)
            for path in budget.entries(target / "debug/incremental"):
                # Each crate's incremental cache is one family (base36 suffix).
                if stat.S_ISDIR(path.lstat().st_mode):
                    families["incremental:" + path.name] = [path]
            # Deterministic order; all members must be older than the cutoff.
            for family in sorted(families):
                try:
                    paths = []
                    for path in families[family]:
                        paths.extend(old_tree(path, cutoff, budget))
                except Recent:
                    continue
                # Whole families only: partial directory deletion would refresh
                # remaining parents' mtimes and keep old cache data for another week.
                if len(paths) > max_delete - result["would_remove"]:
                    result["delete_limited"] = True
                    continue
                for path, directory in paths:
                    if not dry_run:
                        if directory:
                            path.rmdir()
                        else:
                            path.unlink()
                        result["removed"] += 1
                        if path.parent == target / "debug/deps":
                            result["deps_removed"] += 1
                    result["would_remove"] += 1
        except ScanLimit:
            result["scan_limited"] = True
    return result


class Parser(argparse.ArgumentParser):
    def error(self, message):
        print("error: " + json.dumps(message))
        self.print_help()
        raise SystemExit(2)


def main():
    parser = Parser(description="Evict seven-day-old local Cargo cache families between builds.")
    parser.add_argument("--dry-run", action="store_true", help="report without deleting entries")
    args = parser.parse_args()
    try:
        result = sweep(checkout_target(), dry_run=args.dry_run)
    except (OSError, ValueError) as error:
        print("error: " + json.dumps(str(error)))
        print("help: run mise run target-sweep in the checkout with its real local target")
        return 1
    print("sweep: " + json.dumps(result["status"]))
    for key in ["deps_before", "removed", "would_remove"]:
        print(f"{key}: {json.dumps(result[key])}")
    if result["scan_limited"]:
        print("warning: scan budget reached; incomplete families kept")
        print("help: see docs/design/build-cache.md for oversized cache recovery")
    if result["delete_limited"]:
        print("warning: deletion budget reached; whole families kept for another run")
        print("help: see docs/design/build-cache.md for oversized cache recovery")
    if result["deps_before"] is not None and result["deps_before"] - result["deps_removed"] > WARN_ENTRIES:
        print("warning: deps may still exceed 50000 entries; run vega config infra")
    return 0


if __name__ == "__main__":
    sys.exit(main())
