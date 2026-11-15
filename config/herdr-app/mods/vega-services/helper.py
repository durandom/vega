"""Shared service registry/scan backend and private panel SSH helper; Unix only."""
import argparse
import contextlib
import fcntl
import http.client
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import socket
import ssl
import subprocess
import sys
import tempfile
from urllib.parse import urlsplit
from concurrent.futures import ThreadPoolExecutor

PRIVATE = {"proteus", "ganymede", "io", "io-wsl", "titan"}


def command(args, **kwargs):
    return subprocess.run(args, capture_output=True, text=True, timeout=12, **kwargs)


def protocol(listener):
    """Bounded, manual-refresh-only HEAD checks; never assume a TCP port is HTTP."""
    host = listener["host"]
    if host not in ("127.0.0.1", "::1", "::", "0.0.0.0", "*"):
        return "tcp"
    host = "::1" if host in ("::1", "::") else "127.0.0.1"
    for scheme, cls in (("http", http.client.HTTPConnection), ("https", http.client.HTTPSConnection)):
        # This identifies a local protocol, not remote identity. The browser still
        # performs its own TLS validation when the user opens the URL.
        options = {"context": ssl._create_unverified_context()} if scheme == "https" else {}
        connection = cls(host, listener["port"], timeout=0.4, **options)
        try:
            connection.request("HEAD", "/")
            connection.getresponse()
            return scheme
        except (OSError, http.client.HTTPException):
            pass
        finally:
            connection.close()
    return "tcp"


def scan(enrich=True):
    parents, processes = {}, {}
    for line in command(["ps", "-A", "-o", "pid=", "-o", "ppid=", "-o", "comm=", "-o", "args="]).stdout.splitlines():
        pid, parent, *process = line.split(maxsplit=2)
        parents[pid] = int(parent)
        processes[pid] = process[0] if process else ""
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
    if enrich:
        entries = registrations()
        with ThreadPoolExecutor(max_workers=8) as pool:
            for listener, scheme in zip(listeners, pool.map(protocol, listeners)):
                listener["scheme"] = scheme
                listener["agent"] = agent_runtime(processes.get(str(listener["pid"]), listener["name"]))
                entry = entries.get(listener["port"])
                if entry and entry["pid"] == listener["pid"]:
                    listener.update({k: entry[k] for k in ("title", "description", "url")})
                    listener["registered"] = True
                    listener["also_forward"] = entry.get("also_forward", [])
        for port, entry in entries.items():
            if not any(r["port"] == port and r["pid"] == entry["pid"] for r in listeners):
                listeners.append({**entry, "registered": True, "gone": True,
                                  "host": "127.0.0.1", "name": entry["title"], "scheme": "tcp"})
    return dict(listeners=listeners, parents=parents, cwds=cwds, home=str(Path.home()))


def agent_runtime(process):
    """Match executable names and runtime package scripts, not user arguments."""
    try:
        words = shlex.split(process)
    except ValueError:
        words = process.split()
    runtimes = {"codex", "claude", "claude-code", "opencode", "gemini", "agy", "pi"}
    if words and Path(words[0]).name.lower() in runtimes:
        return True
    # ps supplies comm then argv[0]; a node/bun runtime's script follows those.
    packages = ("@openai/codex/", "@anthropic-ai/claude-code/", "@mariozechner/pi-coding-agent/")
    return any(any(package in word for package in packages) for word in words[:3])


def process_identity(pid):
    r = command(["ps", "-p", str(pid), "-o", "stat=", "-o", "lstart="], env={**os.environ, "LC_ALL": "C"})
    if r.returncode not in (0, 1) or (r.returncode and r.stderr.strip()):
        raise RuntimeError(r.stderr.strip() or "Could not read process identity")
    fields = r.stdout.strip().split(maxsplit=1)
    return fields[1] if len(fields) == 2 and "Z" not in fields[0] else None


def state_dir():
    base = os.environ.get("XDG_STATE_HOME")
    if not base:
        home = os.environ.get("HOME")
        if not home:
            raise RuntimeError("Set HOME or XDG_STATE_HOME for service state")
        base = str(Path(home) / ".local/state")
    return Path(base) / "vega/services"


@contextlib.contextmanager
def registry_lock():
    folder = state_dir()
    folder.mkdir(parents=True, mode=0o700, exist_ok=True)
    with (folder / "lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        yield folder


def read_entries(folder):
    entries = {}
    for path in folder.glob("*.json"):
        entry = json.loads(path.read_text())
        if (not isinstance(entry, dict) or entry.get("schema") != 1
                or not isinstance(entry.get("pid"), int) or entry["pid"] <= 1
                or not isinstance(entry.get("port"), int) or not 0 < entry["port"] < 65536
                or path.stem != str(entry["port"])
                or not isinstance(entry.get("start"), str) or not entry["start"]
                or any(not isinstance(entry.get(k), str) for k in ("title", "description", "url"))):
            raise ValueError(f"Invalid service entry {path}; remove or repair it")
        extra = entry.get("also_forward", [])
        if (not isinstance(extra, list) or any(type(p) is not int or not 0 < p < 65536 for p in extra)
                or len(set([entry["port"], *extra])) != len(extra) + 1):
            raise ValueError(f"Invalid service bundle {path}; remove or repair it")
        if process_identity(entry["pid"]) != entry["start"]:
            path.unlink(missing_ok=True)
        else:
            entries[entry["port"]] = entry
    return entries


def registrations():
    if not state_dir().exists():
        return {}
    with registry_lock() as folder:
        return read_entries(folder)


class UsageError(ValueError):
    pass


class Parser(argparse.ArgumentParser):
    def parse_args(self, args=None, namespace=None):
        options, extra = self.parse_known_args(args, namespace)
        if extra:
            owner = self
            for action in self._actions:
                if isinstance(action, argparse._SubParsersAction):
                    owner = action.choices.get(getattr(options, action.dest, None), self)
            owner.error("unrecognized arguments: " + " ".join(extra))
        return options

    def error(self, message):
        raise UsageError(message + "; valid flags: " + " ".join(self._option_string_actions))


def port_number(value):
    try:
        number = int(value)
        if 0 < number < 65536:
            return number
    except ValueError:
        pass
    raise argparse.ArgumentTypeError("port must be 1..65535")


def pid_number(value):
    try:
        number = int(value)
        if 1 < number <= 2147483647:
            return number
    except ValueError:
        pass
    raise argparse.ArgumentTypeError("pid must be 2..2147483647")


def cli(args):
    # Parse and validate everything before scanning processes or touching state.
    parser = Parser(prog="vega service", allow_abbrev=False, description="Register host-local service metadata")
    commands = parser.add_subparsers(dest="action", parser_class=Parser)
    register = commands.add_parser("register", allow_abbrev=False, help="Register or replace metadata for a port",
        epilog='Example: vega service register --port 5187 --title Spire --url http://127.0.0.1:5187')
    register.add_argument("--port", required=True, type=port_number)
    register.add_argument("--also-forward", action="append", type=port_number, default=[],
                          help="Additional remote loopback port; repeat for bundles (same local ports)")
    register.add_argument("--title", required=True)
    register.add_argument("--description", default="")
    register.add_argument("--url", default="")
    register.add_argument("--pid", type=pid_number, help="Listener owner PID (default: discover unambiguous owner)")
    listing = commands.add_parser("ls", allow_abbrev=False, help="List live registrations and prune expired entries",
        epilog="Examples: vega service ls; vega service ls --json")
    remove = commands.add_parser("rm", allow_abbrev=False, help="Remove metadata; never stop the service",
        epilog="Example: vega service rm --port 5187 (repeating is a successful no-op)")
    remove.add_argument("--port", required=True, type=port_number)
    for sub in (parser, register, listing, remove):
        sub.add_argument("--json", action="store_true", default=argparse.SUPPRESS, help="Emit JSON instead of TOON")
    options = parser.parse_args(args)
    action = options.action or "ls"
    if action == "register":
        if len(set([options.port, *options.also_forward])) != len(options.also_forward) + 1:
            raise UsageError("--also-forward ports must be unique and differ from --port")
        for key in ("title", "description", "url"):
            value = getattr(options, key)
            if (key == "title" and not value.strip()) or any(ord(c) < 32 or ord(c) == 127 for c in value):
                raise UsageError(f"--{key} must be nonempty (title) and contain no controls")
        if options.url:
            try:
                url = urlsplit(options.url)
                valid = (url.scheme in ("http", "https") and url.hostname in ("127.0.0.1", "localhost", "::1")
                         and url.port == options.port and url.username is None and url.password is None)
            except ValueError:
                valid = False
            if not valid:
                raise UsageError("--url must be an HTTP(S) loopback URL with the registered port (paths allowed)")
        owners = {r["pid"] for r in scan(enrich=False)["listeners"] if r["port"] == options.port}
        pid = options.pid
        if pid is None:
            if len(owners) != 1:
                raise ValueError("Port has no unique listener owner; use --pid <listener-pid> to disambiguate")
            pid = owners.pop()
        elif pid not in owners:
            raise ValueError("--pid does not own a listener on --port; use the listener PID")
        start = process_identity(pid)
        if not start:
            raise ValueError("Listener process has exited; retry after starting the service")
        entry = dict(schema=1, port=options.port, pid=pid, start=start,
                     title=options.title, description=options.description, url=options.url,
                     also_forward=options.also_forward)
        with registry_lock() as folder:
            with tempfile.NamedTemporaryFile(mode="w", dir=folder, delete=False) as out:
                temp = Path(out.name)
                try:
                    json.dump(entry, out)
                    out.flush()
                    os.replace(temp, folder / f"{options.port}.json")
                finally:
                    temp.unlink(missing_ok=True)
        data = dict(service=entry)
    elif action == "rm":
        if state_dir().exists():
            with registry_lock() as folder:
                (folder / f"{options.port}.json").unlink(missing_ok=True)
        data = dict(removed=options.port)
    else:
        data = dict(services=sorted(registrations().values(), key=lambda e: e["port"]))
    if getattr(options, "json", False):
        print(json.dumps(data))
    elif action == "ls":
        if not args:
            print("bin: " + json.dumps(os.environ.get("VEGA_SERVICE_BIN", "vega-service")))
            print("description: Register host-local service metadata tied to listener processes")
        entries = data["services"]
        if entries:
            print(f"services[{len(entries)}]{{port,pid,title,url}}:")
            for entry in entries:
                print("  " + ",".join(json.dumps(entry[k]) for k in ("port", "pid", "title", "url")))
        else:
            print("services: 0 live registrations on this host")
        print('help: vega service register --port <port> --title "<title>"; vega service ls --json; vega service rm --port <port>')
    else:
        print(f"service: {options.port} { 'registered' if action == 'register' else 'removed (or already absent)'}")
    return 0


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


def forward(action, owner, key="", target="", host="", port=0, mode="ephemeral", *also):
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
        ports = [port, *(int(p) for p in also)]
        if (mode not in ("same", "ephemeral") or any(not 0 < p < 65536 for p in ports)
                or len(set(ports)) != len(ports) or (also and mode != "same")):
            raise ValueError("Invalid forwarding ports or mode")
        if mode == "same":
            # Hold all probes together so the whole bundle is checked before SSH.
            with contextlib.ExitStack() as stack:
                for number in ports:
                    probe = stack.enter_context(contextlib.closing(socket.socket()))
                    try:
                        probe.bind(("127.0.0.1", number))
                    except OSError as error:
                        raise RuntimeError(f"Local port 127.0.0.1:{number} unavailable; stop its owner before forwarding") from error
        # SSH confirms the actual bind; retry if another process wins the free-port race.
        for _ in range(1 if mode == "same" else 5):
            local = port
            if mode == "ephemeral":
                with contextlib.closing(socket.socket()) as probe:
                    probe.bind(("127.0.0.1", 0))
                    local = probe.getsockname()[1]
            far = f"[{host}]" if ":" in host else host
            bindings = [arg for number in ports for arg in ("-L", f"127.0.0.1:{local if number == port else number}:{far}:{number}")]
            r = command(["ssh", "-fNT", "-M", "-S", str(path), "-o", "ControlPersist=no",
                         "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes",
                         "-o", "ConnectTimeout=5", "-o", "ExitOnForwardFailure=yes",
                         "-o", "ServerAliveInterval=15", "-o", "ServerAliveCountMax=2",
                         *bindings, "--", target])
            if r.returncode == 0:
                if (folder / "closed").exists():
                    end(path)
                    raise ValueError("Panel closed")
                return dict(local=local)
            if "Address already in use" not in r.stderr:
                break
        if mode == "same" and "Address already in use" in r.stderr:
            end(path)
            raise RuntimeError(f"Local port conflict in bundle {ports}; stop the local owner before forwarding: {r.stderr.strip()}")
        raise RuntimeError(r.stderr.strip() or "SSH forwarding failed")


def main():
    if sys.argv[1:2] == ["cli"]:
        try:
            return cli(sys.argv[2:])
        except Exception as error:
            code = "usage" if isinstance(error, UsageError) else "service_failed"
            hint = "vega service " + (sys.argv[2] if sys.argv[2:3] and sys.argv[2] in ("register", "ls", "rm") else "") + " --help"
            if "--json" in sys.argv[2:]:
                print(json.dumps(dict(error=dict(code=code, message=str(error), hint=hint))))
            else:
                print("error: " + json.dumps(str(error)))
                print("help: " + json.dumps(hint))
            return 2 if code == "usage" else 1
    try:
        action, *args = sys.argv[1:]
        result = scan() if action == "scan" else forward(action, *args[:4], int(args[4]), *args[5:]) if action == "start" else forward(action, *args)
        print(json.dumps(result))
    except Exception as error:
        print(json.dumps({"error": str(error)}))
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
