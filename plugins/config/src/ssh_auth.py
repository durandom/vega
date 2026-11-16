"""Read-only Git SSH auth diagnostic, also run on fleet hosts via hc exec.

Only explicit supported Host/HostName entries opt in; wildcard-only config does
not opt in. Includes are bounded and relative to ~/.ssh, as in OpenSSH.
"""
import glob
import json
import os
import pathlib
import shlex
import subprocess

home = pathlib.Path.home()
supported = {'github.com', 'gitlab.cee.redhat.com'}
targets = set()
visited = set()


def config(path, hosts=()):
    path = pathlib.Path(path)
    if str(path) in visited:
        return hosts
    if len(visited) >= 64:
        raise ValueError('too many Includes')
    visited.add(str(path))
    try:
        lines = path.read_text().splitlines()
    except FileNotFoundError:
        return hosts
    for line in lines:
        words = shlex.split(line, comments=True)
        if not words:
            continue
        # OpenSSH accepts keyword=value as well as keyword value.
        key, sep, value = words[0].partition('=')
        args = ([value] if sep and value else []) + words[1:]
        if args[:1] == ['=']:
            args = args[1:]
        key = key.lower()
        if key == 'host':
            hosts = tuple(h for h in args if not any(c in h for c in '*?!'))
            targets.update(h for h in hosts if h.lower() in supported)
        elif key == 'match':
            hosts = ()
        elif key == 'hostname' and args and args[0].lower() in supported:
            targets.update(h for h in hosts if all(c.isalnum() or c in '._-' for c in h) and not h.startswith('-'))
        elif key == 'include':
            for pattern in args:
                expanded = pathlib.Path(os.path.expanduser(pattern))
                if not expanded.is_absolute():
                    expanded = home / '.ssh' / expanded
                for included in sorted(glob.glob(str(expanded))):
                    hosts = config(included, hosts)
    return hosts


rows = []
try:
    config(home / '.ssh/config')
except (OSError, ValueError):
    rows.append({'host': 'config', 'state': 'fail: cannot read SSH config', 'ok': False})
# Cap discovery and execution even with an unexpectedly large config.
if len(targets) > 16:
    rows.append({'host': 'config', 'state': 'fail: more than 16 Git host aliases', 'ok': False})
for host in sorted(targets)[:16]:
    try:
        result = subprocess.run([
            'ssh', '-T', '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=5',
            '-o', 'ConnectionAttempts=1', '-o', 'StrictHostKeyChecking=yes',
            '-o', 'UpdateHostKeys=no', '-o', 'ControlMaster=no',
            '-o', 'ControlPath=none', '-o', 'PermitLocalCommand=no',
            '-o', 'RemoteCommand=none', '-o', 'RequestTTY=no', '-l', 'git', host,
        ], stdin=subprocess.DEVNULL, capture_output=True, timeout=8)
        # GitHub deliberately exits 1 after successful auth (no shell access).
        # GitLab emits its welcome greeting. Transport/auth failures exit 255.
        greeting = (result.stdout + result.stderr).decode('utf-8', errors='replace')
        ok = result.returncode in (0, 1) and (
            "successfully authenticated" in greeting or 'Welcome to GitLab,' in greeting)
        state = 'ok' if ok else 'fail: SSH authentication or transport'
    except subprocess.TimeoutExpired:
        ok, state = False, 'fail: SSH timeout'
    except OSError:
        ok, state = False, 'fail: ssh unavailable'
    rows.append({'host': host, 'state': state, 'ok': ok})
# No raw SSH output, identity comments, fingerprints or credentials cross the wire.
print(json.dumps(rows))
