#!/usr/bin/env python3
"""Vendor the released hc API; never snapshot an unreleased/live core."""
import json
import pathlib
import shutil
import sys

root = pathlib.Path(__file__).resolve().parents[1]
source = pathlib.Path(sys.argv[1]) if len(sys.argv) == 2 else root / 'repos/herdr-core'
if len(sys.argv) > 2:
    sys.exit('usage: scripts/refresh-hc-schema.py [path/to/herdr-core]')
source = source / 'schema/released.openrpc.json'
doc = json.loads(source.read_text())
version = doc['info']['version']
if not version or any(c not in '0123456789.-abcdefghijklmnopqrstuvwxyz' for c in version):
    sys.exit('invalid hc version in released schema')
target = root / f'crates/vega-herdr/schema/hc-{version}.openrpc.json'
shutil.copyfile(source, target)
print(f'Vendored hc {version}: {target.relative_to(root)}')
print('If the version changed, update hc_schema.rs include_str!, remove the old snapshot,')
print('and update docs/design/hc-migration.md; run nice -n 10 mise run check.')
