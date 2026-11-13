import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


class CheckMods(unittest.TestCase):
    def test_checks_every_mod_and_cleans_success_and_failure(self):
        with tempfile.TemporaryDirectory(prefix="vega mods ") as tmp:
            root = Path(tmp)
            (root / "scripts").mkdir()
            shutil.copy(Path(__file__).with_name("check-mods.sh"), root / "scripts/check-mods.sh")
            for name in ["a", "b", "c"]:
                mod = root / "config/herdr-app/mods" / name
                mod.mkdir(parents=True)
                (mod / "mod.json").write_text("{}")
            binary = root / "herdr-app"
            binary.write_text('''#!/bin/sh
touch "$2/gpui-kit.d.ts" "$2/jsconfig.json" "$2/checked"
case "$2" in */b) exit 7;; esac
''')
            binary.chmod(0o755)
            env = dict(os.environ, PATH=f"{root}:{os.environ['PATH']}")
            result = subprocess.run(["sh", str(root / "scripts/check-mods.sh")], env=env)
            self.assertEqual(result.returncode, 7)
            for mod in (root / "config/herdr-app/mods").iterdir():
                self.assertTrue((mod / "checked").exists())
                self.assertFalse((mod / "gpui-kit.d.ts").exists())
                self.assertFalse((mod / "jsconfig.json").exists())
            binary.write_text(binary.read_text().replace('case "$2" in */b) exit 7;; esac', 'exit 0'))
            self.assertEqual(subprocess.run(["sh", str(root / "scripts/check-mods.sh")], env=env).returncode, 0)


if __name__ == "__main__":
    unittest.main()
