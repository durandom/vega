#!/usr/bin/env python3
"""The compiler wrapper must preserve Cargo's hyphenated environment names."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


wrapper = Path(__file__).with_name("rustc-wrapper.sh").resolve()
with tempfile.TemporaryDirectory() as directory:
    root = Path(directory)
    compiler = root / "fake compiler"
    compiler.write_text(
        f"#!{sys.executable}\n"
        "import json, os, sys\n"
        "print(json.dumps([os.environ.get('CARGO_BIN_EXE_vega-review'), sys.argv[1:]]))\n"
        "sys.exit(17)\n"
    )
    compiler.chmod(0o755)
    cache = root / "sccache"
    cache.write_text(
        f"#!{sys.executable}\n"
        "import os, sys\n"
        "os.execv(sys.argv[1], sys.argv[1:])\n"
    )
    cache.chmod(0o755)
    arguments = ["--crate-name", "vega_review", "path with spaces.rs"]
    for mode in ("incremental", "cached", "uncached"):
        if mode == "uncached":
            cache.unlink()
        environment = dict(
            os.environ,
            PATH=directory,
            CARGO_INCREMENTAL="1" if mode == "incremental" else "0",
        )
        environment["CARGO_BIN_EXE_vega-review"] = "/target/debug/vega-review"
        result = subprocess.run(
            [str(wrapper), str(compiler), *arguments],
            env=environment,
            capture_output=True,
            text=True,
            check=False,
        )
        assert result.returncode == 17, (mode, result)
        assert json.loads(result.stdout) == [
            "/target/debug/vega-review",
            arguments,
        ], (mode, result)
        assert not result.stderr, (mode, result.stderr)
print("rustc-wrapper.test: ok")
