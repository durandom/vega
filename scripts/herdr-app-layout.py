#!/usr/bin/env python3
"""Explicit, reviewable maintenance of the vega layout; never run by config apply."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent


def generate(app_home, version):
    raw = (app_home / "layouts/main.json").read_text(encoding="utf-8-sig")
    tree = json.loads(raw)
    installed = json.loads((app_home / "installed.json").read_text())
    if not isinstance(installed, dict) or not isinstance(installed.get("layout:main"), dict):
        raise ValueError("installed.json has no layout:main record")
    fingerprint = installed.get("layout:main", {}).get("json")
    if not isinstance(fingerprint, str) or not fingerprint:
        raise ValueError("installed.json has no layout:main fingerprint")
    anchors = []
    agent_anchors = []

    def walk(node):
        if not isinstance(node, dict):
            raise ValueError("main contains a non-object node")
        if node.get("id") == "vega-status":
            raise ValueError("main already contains vega-status")
        if node.get("content", {}).get("mod") == "agents":
            agent_anchors.append(node)
        children = node.get("children", [])
        if not isinstance(children, list):
            raise ValueError("main contains invalid children")
        for index, child in enumerate(children):
            if not isinstance(child, dict):
                raise ValueError("main contains a non-object child")
            if child.get("id") == "status":
                content = child.get("content")
                if node.get("type") != "column" or not isinstance(content, dict) or content.get("mod") != "workspace-status":
                    raise ValueError("status anchor changed; review the upstream layout")
                anchors.append((children, index))
            walk(child)

    walk(tree)
    if len(anchors) != 1:
        raise ValueError("expected exactly one status anchor; old output kept")
    if len(agent_anchors) != 1:
        raise ValueError("expected exactly one agents block; review the upstream layout")
    agent_anchors[0]["content"]["mod"] = "vega-agents"
    children, index = anchors[0]
    children.insert(index + 1, {
        "id": "vega-status", "type": "block", "size": {"px": 24},
        "content": {"mod": "vega"},
    })
    tree["cloned_from"] = {"name": "main", "version": version}
    tree["vega_source"] = {
        "installed_hash": fingerprint,
        "sha256": hashlib.sha256(raw.encode()).hexdigest(),
    }
    return json.dumps(tree, indent=2) + "\n"


class Parser(argparse.ArgumentParser):
    def error(self, message):
        print("error: " + json.dumps(message))
        self.print_help()
        raise SystemExit(2)


def main():
    parser = Parser(description=__doc__, epilog="Example: python3 scripts/herdr-app-layout.py --write; review and commit its diff.")
    parser.add_argument("--app-home", type=Path, default=Path(os.environ.get("HERDR_APP_HOME", str(Path(os.environ.get("XDG_CONFIG_HOME", str(Path.home() / ".config"))) / "herdr-app"))), help="installed app home (read only)")
    parser.add_argument("--app-version", help="source app version; default: herdr-app --version")
    parser.add_argument("--output", type=Path, default=ROOT / "config/herdr-app/layouts/vega.json", help="reviewable generated file")
    parser.add_argument("--write", action="store_true", help="write the generated layout; otherwise preview provenance only")
    parser.add_argument("-v", "-V", "--version", action="version", version="1")
    args = parser.parse_args()
    try:
        version = args.app_version or subprocess.check_output(["herdr-app", "--version"], text=True, timeout=5).strip().split()[-1]
        text = generate(args.app_home, version)
        if args.write:
            args.output.parent.mkdir(parents=True, exist_ok=True)
            with tempfile.NamedTemporaryFile(mode="w", dir=args.output.parent, delete=False) as staged:
                staged.write(text)
            os.replace(staged.name, args.output)
        print("layout: " + json.dumps(str(args.output)))
        print("state: " + ("written; review and commit the diff" if args.write else "preview; use --write to regenerate"))
        print("source: " + json.dumps(json.loads(text)["vega_source"]))
        return 0
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        print("error: " + json.dumps(str(error)))
        print("help: inspect the installed main layout and installed.json; the old output is kept")
        return 1


if __name__ == "__main__":
    sys.exit(main())
