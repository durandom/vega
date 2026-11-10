"""Installer checksum and rollback regression tests; no network or live HOME."""
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("install_dcg", Path(__file__).with_name("install-dcg.py"))
installer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(installer)


class InstallTests(unittest.TestCase):
    def exercise(self, *, bad_checksum=False, doctor_failure=False, doctor_errors=None,
                 allow_stale_opencode=False, opencode_present=False):
        with tempfile.TemporaryDirectory() as temp:
            home = Path(temp)
            dest = home / ".local/bin/dcg"
            dest.parent.mkdir(parents=True)
            dest.write_bytes(b"old binary")
            archive = io.BytesIO()
            with tarfile.open(fileobj=archive, mode="w:xz") as tar:
                member = tarfile.TarInfo("dcg")
                member.size = len(b"new binary")
                tar.addfile(member, io.BytesIO(b"new binary"))
            data = archive.getvalue()
            digest = "bad" if bad_checksum else hashlib.sha256(data).hexdigest()

            def run(args, **kwargs):
                if args[0] == "gh":
                    (Path(args[-1]) / "dcg-test.tar.xz").write_bytes(data)
                elif doctor_failure:
                    raise subprocess.CalledProcessError(1, args)
                elif doctor_errors:
                    if "--format" in args:
                        return subprocess.CompletedProcess(args, 1, stdout=json.dumps({
                            "checks": [{"id": error, "status": "error"} for error in doctor_errors]
                        }))
                    return subprocess.CompletedProcess(args, 1)
                return subprocess.CompletedProcess(args, 0)

            def version(args, **kwargs):
                return "0.9.2\n" if args[0] == str(dest) else "dcg 0.16.0\n"

            rollback = doctor_failure or bool(doctor_errors and not (
                allow_stale_opencode and doctor_errors == ["opencode_plugin"] and not opencode_present))
            with patch.object(Path, "home", return_value=home), \
                 patch.object(installer.platform, "system", return_value="Test"), \
                 patch.object(installer.platform, "machine", return_value="test"), \
                 patch.dict(installer.ARCHIVES, {("Test", "test"): ("test", digest)}), \
                 patch.object(installer.shutil, "which", return_value="/bin/opencode" if opencode_present else None), \
                 patch.object(installer.subprocess, "run", side_effect=run), \
                 patch.object(installer.subprocess, "check_output", side_effect=version):
                if bad_checksum:
                    with self.assertRaisesRegex(RuntimeError, "checksum mismatch"):
                        installer.install(allow_stale_opencode=allow_stale_opencode)
                elif rollback:
                    with self.assertRaises(subprocess.CalledProcessError):
                        installer.install(allow_stale_opencode=allow_stale_opencode)
                else:
                    installer.install(allow_stale_opencode=allow_stale_opencode)
            self.assertEqual(dest.read_bytes(), b"old binary" if bad_checksum or rollback else b"new binary")
            backup = home / ".cache/dcg-0.9.2/dcg"
            if not bad_checksum:
                self.assertEqual(backup.read_bytes(), b"old binary")

    def test_valid_install_keeps_old_binary(self):
        self.exercise()

    def test_checksum_failure_does_not_replace_binary(self):
        self.exercise(bad_checksum=True)

    def test_doctor_failure_restores_backup(self):
        self.exercise(doctor_failure=True)

    def test_explicit_unused_opencode_exception(self):
        self.exercise(doctor_errors=["opencode_plugin"], allow_stale_opencode=True)

    def test_unused_opencode_error_rolls_back_by_default(self):
        self.exercise(doctor_errors=["opencode_plugin"])

    def test_exception_does_not_excuse_other_errors(self):
        self.exercise(doctor_errors=["opencode_plugin", "hook_wiring"], allow_stale_opencode=True)

    def test_exception_does_not_excuse_installed_opencode(self):
        self.exercise(doctor_errors=["opencode_plugin"], allow_stale_opencode=True, opencode_present=True)


if __name__ == "__main__":
    unittest.main()
