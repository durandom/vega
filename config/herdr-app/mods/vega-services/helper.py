"""Private implementation helper for the Services mod; Unix hosts only."""
import contextlib
import fcntl
import json
import os
from pathlib import Path
import re
import shutil
import socket
import subprocess
import sys

PRIVATE = {"proteus", "ganymede", "io", "io-wsl", "titan"}


def command(args, **kwargs):
    return subprocess.run(args, capture_output=True, text=True, timeout=12, **kwargs)


def scan():
    parents = {}
    for line in command(["ps", "-A", "-o", "pid=", "-o", "ppid="]).stdout.splitlines():
        pid, parent = line.split()
        parents[pid] = int(parent)
    listeners = []
    if shutil.which("ss"):
        result = command(["ss", "-tlnpH"])
        for line in result.stdout.splitlines():
            fields = line.split()
            if len(fields) < 5:
                continue
            address = fields[3]
            for name, pid in re.findall(r'\("([^"]+)",pid=(\d+)', line):
                host, port = address.rsplit(":", 1)
                listeners.append(dict(pid=int(pid), name=name, host=host.strip("[]"), port=int(port)))
    else:
        result = command(["lsof", "-nP", "-iTCP", "-sTCP:LISTEN", "-Fpcn"])
        pid, name = None, ""
        for line in result.stdout.splitlines():
            if line.startswith("p"):
                pid = int(line[1:])
            elif line.startswith("c"):
                name = line[1:]
            elif line.startswith("n") and pid:
                host, port = line[1:].rsplit(":", 1)
                listeners.append(dict(pid=pid, name=name, host=host.strip("[]"), port=int(port)))
    if result.returncode not in (0, 1) or (result.returncode and result.stderr.strip()):
        raise RuntimeError(result.stderr.strip() or "Listener scan failed")
    cwds = {}
    for pid in {row["pid"] for row in listeners}:
        try:
            cwds[str(pid)] = os.readlink(f"/proc/{pid}/cwd")
        except OSError:
            if shutil.which("lsof"):
                r = command(["lsof", "-a", "-p", str(pid), "-d", "cwd", "-Fn"])
                cwds[str(pid)] = next((s[1:] for s in r.stdout.splitlines() if s.startswith("n")), "")
    return dict(listeners=listeners, parents=parents, cwds=cwds, home=str(Path.home()))


def private_host(target):
    # Fail closed for aliases, IPs and domains not explicitly in our estate.
    return target.rsplit("@", 1)[-1] in PRIVATE


def owner_dir(owner):
    if not re.fullmatch(r"[a-z0-9-]{1,70}", owner):
        raise ValueError("Invalid panel owner")
    root = Path("/tmp") / f"vega-services-{os.getuid()}"
    root.mkdir(mode=0o700, exist_ok=True)
    if root.is_symlink() or root.stat().st_uid != os.getuid() or root.stat().st_mode & 0o077:
        raise ValueError("Unsafe services directory")
    folder = root / owner
    folder.mkdir(mode=0o700, exist_ok=True)
    if folder.is_symlink() or folder.stat().st_uid != os.getuid() or folder.stat().st_mode & 0o077:
        raise ValueError("Unsafe panel directory")
    return folder


def end(path):
    if path.exists():
        r = command(["ssh", "-S", str(path), "-O", "exit", "ignored"])
        if r.returncode and path.exists():
            check = command(["ssh", "-S", str(path), "-O", "check", "ignored"])
            if check.returncode == 0:
                raise RuntimeError(r.stderr.strip() or "Could not stop forward")
            path.unlink(missing_ok=True)  # stale socket from an exited master


def forward(action, owner, key="", target="", host="", port=0):
    folder = owner_dir(owner)
    if action == "close":
        (folder / "closed").touch()
    with (folder / "lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        if action == "close":
            errors = []
            for path in folder.glob("s*"):
                try:
                    end(path)
                except Exception as error:
                    errors.append(str(error))
            if errors:
                raise RuntimeError("; ".join(errors))
            return {}
        if not re.fullmatch(r"[0-9]+", key):
            raise ValueError("Invalid forward key")
        path = folder / f"s{key}"
        if action == "status":
            return dict(alive=path.exists() and command(["ssh", "-S", str(path), "-O", "check", "ignored"]).returncode == 0)
        if action == "stop":
            end(path)
            return {}
        if (folder / "closed").exists():
            raise ValueError("Panel closed")
        if not private_host(socket.gethostname().split(".")[0]) or not private_host(target):
            raise ValueError("Forwarding requires a private app host and private SSH target")
        if not 0 < port < 65536 or host not in ("127.0.0.1", "::1"):
            raise ValueError("Invalid loopback destination")
        # SSH confirms the actual bind; retry if another process wins the free-port race.
        for _ in range(5):
            with contextlib.closing(socket.socket()) as probe:
                probe.bind(("127.0.0.1", 0))
                local = probe.getsockname()[1]
            far = f"[{host}]" if ":" in host else host
            r = command(["ssh", "-fNT", "-M", "-S", str(path), "-o", "ControlPersist=no",
                         "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes",
                         "-o", "ConnectTimeout=5", "-o", "ExitOnForwardFailure=yes",
                         "-o", "ServerAliveInterval=15", "-o", "ServerAliveCountMax=2",
                         "-L", f"127.0.0.1:{local}:{far}:{port}", "--", target])
            if r.returncode == 0:
                if (folder / "closed").exists():
                    end(path)
                    raise ValueError("Panel closed")
                return dict(local=local)
            if "Address already in use" not in r.stderr:
                break
        raise RuntimeError(r.stderr.strip() or "SSH forwarding failed")


def main():
    try:
        action, *args = sys.argv[1:]
        result = scan() if action == "scan" else forward(action, *args[:-1], int(args[-1])) if action == "start" else forward(action, *args)
        print(json.dumps(result))
    except Exception as error:
        print(json.dumps({"error": str(error)}))
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
