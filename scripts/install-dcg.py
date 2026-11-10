#!/usr/bin/env python3
"""Install the pinned dcg archive without running its hook installer."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile
import tempfile

VERSION = "0.16.0"
# GitHub v0.16.0 release asset SHA-256 digests; never fetch a mutable checksum.
ARCHIVES = {
    ("Darwin", "arm64"): ("aarch64-apple-darwin", "06cd609b19b988d267ebfac3e6736e511ba42c7a1746f4ec2af5295a8b300fe0"),
    ("Darwin", "x86_64"): ("x86_64-apple-darwin", "f567c5fe4b3b5dbdf56c1621e5f80d7a8b4463de1cf70eb91b9b99955a623f63"),
    ("Linux", "aarch64"): ("aarch64-unknown-linux-gnu", "ca4f2c4fee3aea774fac3e53a6a4cab1c7f9c6228f4bd9d6941de05fd5ac6308"),
    ("Linux", "x86_64"): ("x86_64-unknown-linux-musl", "e7d860eba7f99cb5d5535b00942196aae3d6ca22ec049491d74c97aab556b642"),
}


def install(*, allow_stale_opencode=False, archive_path=None):
    target, digest = ARCHIVES[(platform.system(), platform.machine())]
    name = f"dcg-{target}.tar.xz"
    dest = Path.home() / ".local/bin/dcg"
    env = dict(os.environ, DCG_NO_SELF_HEAL="1")
    with tempfile.TemporaryDirectory(prefix="vega-dcg-") as scratch:
        scratch = Path(scratch)
        archive = scratch / name
        if archive_path:
            shutil.copyfile(archive_path, archive)
        else:
            subprocess.run(["gh", "release", "download", f"v{VERSION}", "-R",
                            "Dicklesworthstone/destructive_command_guard", "-p", name,
                            "-D", str(scratch)], check=True)
        if hashlib.sha256(archive.read_bytes()).hexdigest() != digest:
            raise RuntimeError("dcg archive checksum mismatch; binary unchanged")
        with tarfile.open(archive) as tar:
            members = [m for m in tar.getmembers() if m.isfile() and Path(m.name).name == "dcg"]
            if len(members) != 1:
                raise RuntimeError("archive must contain exactly one dcg binary")
            candidate = scratch / "dcg"
            candidate.write_bytes(tar.extractfile(members[0]).read())
        candidate.chmod(0o755)
        actual = subprocess.check_output([str(candidate), "--version"], env=env, text=True).strip()
        if actual not in (VERSION, f"dcg {VERSION}"):
            raise RuntimeError(f"unexpected dcg version: {actual}; binary unchanged")
        backup = None
        if dest.exists():
            old = subprocess.check_output([str(dest), "--version"], env=env, text=True).strip().split()[-1]
            if not old or any(c not in "0123456789.-" for c in old):
                raise RuntimeError("cannot identify installed dcg version; binary unchanged")
            backup = Path.home() / ".cache" / f"dcg-{old}" / "dcg"
            backup.parent.mkdir(parents=True, exist_ok=True)
            if not backup.exists():
                shutil.copy2(dest, backup)
            elif backup.read_bytes() != dest.read_bytes():
                raise RuntimeError(f"backup differs: {backup}; binary unchanged")
        dest.parent.mkdir(parents=True, exist_ok=True)
        staged = dest.with_name("dcg.vega-new")
        shutil.copy2(candidate, staged)
        staged.replace(dest)
        try:
            doctor = subprocess.run([str(dest), "doctor", "--strict"], env=env)
            if doctor.returncode:
                if not allow_stale_opencode:
                    raise subprocess.CalledProcessError(doctor.returncode, doctor.args)
                # Explicit host rollout exception: upstream detects unused agents by
                # stale directories. Never excuse a different doctor error.
                report = subprocess.run([str(dest), "doctor", "--strict", "--format", "json"],
                                        env=env, capture_output=True, text=True)
                health = json.loads(report.stdout)
                errors = [c["id"] for c in health["checks"] if c["status"] == "error"]
                if not (allow_stale_opencode and shutil.which("opencode") is None
                        and errors == ["opencode_plugin"]):
                    raise subprocess.CalledProcessError(doctor.returncode, doctor.args)
                print("doctor exception: unused OpenCode config directory; no opencode binary on PATH")
        except BaseException:
            if backup:
                shutil.copy2(backup, staged)
                staged.replace(dest)
                print(f"Restored dcg from {backup}", flush=True)
            else:
                dest.unlink()
            raise
        print(f"Installed dcg {VERSION}: {dest}; backup: {backup or 'none (new install)'}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--allow-stale-opencode", action="store_true",
                        help="keep the binary only if the sole doctor error is unused OpenCode (no binary on PATH)")
    parser.add_argument("--archive", type=Path, help="use a local archive, verifying the same pinned checksum")
    args = parser.parse_args()
    try:
        install(allow_stale_opencode=args.allow_stale_opencode, archive_path=args.archive)
    except (OSError, ValueError, KeyError, RuntimeError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"error: {error}\n")
