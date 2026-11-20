"""Opt-in review regression against an isolated Chromium and fixture app."""

import argparse
from contextlib import ExitStack
import http.server
import json
import os
import pathlib
import re
import signal
import socket
import subprocess as sp
import tempfile
import threading
import time
import urllib.request


def session_groups(session):
    # The inbox uses a separate process group within the start command's
    # session. Match that session before signaling any fixture process group.
    rows = sp.check_output(["/bin/ps", "-A", "-o", "pid=,stat="], text=True)
    groups = set()
    for row in rows.splitlines():
        raw_pid, status = row.split()
        if status.startswith("Z"):
            continue
        pid = int(raw_pid)
        try:
            if os.getsid(pid) == session:
                groups.add(os.getpgid(pid))
        except ProcessLookupError:
            pass
    return groups


def reap(child):
    deadline = time.monotonic() + 5
    try:
        while groups := session_groups(child.pid):
            for group in groups:
                try:
                    os.killpg(group, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            if time.monotonic() >= deadline:
                raise RuntimeError(f"fixture processes survived in session {child.pid}")
            child.poll()
            time.sleep(0.05)
    finally:
        child.wait(timeout=5)


class Processes(ExitStack):
    def spawn(self, args, **kwargs):
        child = sp.Popen(args, start_new_session=True, **kwargs)
        self.callback(reap, child)
        return child

    def run(self, args, timeout=40, **kwargs):
        child = self.spawn(args, stdout=sp.PIPE, stderr=sp.PIPE, text=True, **kwargs)
        try:
            stdout, stderr = child.communicate(timeout=timeout)
        except sp.TimeoutExpired:
            reap(child)
            child.communicate(timeout=5)
            raise
        if child.returncode:
            raise RuntimeError(f"{args}: {stdout} {stderr}")
        return stdout.strip()


class App(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_GET(self):
        body = b'<!doctype html><title>Local throwaway</title><h1 id="hero">Local test page</h1>'
        self.send_response(200)
        self.send_header(
            "Content-Security-Policy", "default-src 'self'; connect-src 'self'"
        )
        self.send_header("Content-Type", "text/html")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def retry(fn):
    for _ in range(100):
        try:
            v = fn()
            if v:
                return v
        except OSError:
            pass
        time.sleep(0.1)
    raise RuntimeError("timeout")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--agent-browser", required=True, type=pathlib.Path)
    parser.add_argument(
        "--chromium", default="/Applications/Chromium.app/Contents/MacOS/Chromium"
    )
    parser.add_argument(
        "--bin-dir",
        type=pathlib.Path,
        default=pathlib.Path(__file__).resolve().parents[3] / "target/debug",
    )
    args = parser.parse_args()
    real = args.agent_browser.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="vega-tdeu-live-", dir="/tmp") as td:
        with Processes() as processes:
            run_fixture(
                pathlib.Path(td), args.bin_dir.resolve(), real, args.chromium, processes
            )


def run_fixture(root, bin_dir, real, chromium, processes):
    for sub in ["bin", "state", "config"]:
        (root / sub).mkdir()
    app = http.server.ThreadingHTTPServer(("127.0.0.1", 0), App)
    server = threading.Thread(target=app.serve_forever, daemon=True)
    server.start()
    processes.callback(app.server_close)
    processes.callback(server.join)
    processes.callback(app.shutdown)
    url = f"http://127.0.0.1:{app.server_port}"
    with socket.socket() as free:
        free.bind(("127.0.0.1", 0))
        port = free.getsockname()[1]
        assert port not in (9223, 9224)
    herdr = root / "bin/herdr"
    herdr.write_text("""#!/bin/sh
case "$1 $2" in
'pane list') printf '%s\n' '{"result":{"panes":[{"pane_id":"w1:p1","agent":"test"}]}}' ;;
'pane process-info') printf '{"result":{"process_info":{"shell_pid":%s}}}\n' "$TEST_PARENT_PID" ;;
'agent get') printf '%s\n' '{"result":{"agent":{"name":"test"}}}' ;;
*) exit 1 ;;
esac
""")
    mail = root / "bin/fake-post"
    mail.write_text(f'#!/bin/sh\nprintf "%s\\n" "$@" >> {root}/mail.txt\n')
    for script in [herdr, mail]:
        script.chmod(0o755)
    env = {
        k: v
        for k, v in os.environ.items()
        if not k.startswith(("AGENT_BROWSER_", "CDP_", "VEGA_"))
    }
    env.update(
        PATH=f"{root}/bin:{bin_dir}:/usr/bin:/bin:/opt/homebrew/bin",
        XDG_STATE_HOME=str(root / "state"),
        XDG_CONFIG_HOME=str(root / "config"),
        VEGA_AGENT_BROWSER_BIN=str(real),
        CDP_PORT=str(port),
        CDP_PROFILE=str(root / "chrome"),
        CDP_CHROMIUM=chromium,
        HERDR_PANE_ID="w1:p1",
        TEST_PARENT_PID=str(os.getpid()),
        VEGA_REVIEW_POST=str(mail),
    )

    def run(binary, *args):
        return processes.run([binary, *args], env=env)

    def targets():
        with urllib.request.urlopen(
            f"http://127.0.0.1:{port}/json/list", timeout=1
        ) as r:
            return json.load(r)

    def review(*args):
        return run("vega-review", *args)

    process = processes.spawn(
        [
            chromium,
            f"--remote-debugging-port={port}",
            f"--user-data-dir={root}/chrome",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-background-networking",
            url + "/spectator",
        ],
        stdout=sp.DEVNULL,
        stderr=sp.DEVNULL,
    )
    try:
        spectator = retry(
            lambda: next((t for t in targets() if t["url"] == url + "/spectator"), None)
        )
        output = review("start", url + "/review", "--profile", "global")
        print(output)
        session = re.search(r"agent-browser session (\S+)", output)[1]

        def ab(*args):
            return run("vega-browser", "ab", "--session", session, *args)

        def click(selector):
            js = (
                "(() => { const r = document.querySelector('vega-review').shadowRoot.querySelector("
                + json.dumps(selector)
                + ").getBoundingClientRect(); return [r.x+r.width/2,r.y+r.height/2] })()"
            )
            x, y = json.loads(ab("eval", js))
            ab("mouse", "move", str(round(x)), str(round(y)))
            ab("mouse", "down")
            ab("mouse", "up")

        reviewed = retry(
            lambda: next((t for t in targets() if t["url"] == url + "/review"), None)
        )
        assert {t["id"] for t in targets() if t["type"] == "page"} == {
            spectator["id"],
            reviewed["id"],
        }
        records = json.loads((root / "state/vega/browser/sessions.json").read_text())
        assert records[session]["pane"] == "w1:p1"
        print(review("highlight", "#hero", "--label", "Local highlight"))
        patch = root / "hero.js"
        patch.write_text(
            "const h = document.querySelector('#hero'); if (h && !h.hasAttribute('data-patched')) h.setAttribute('data-patched', 'hero');"
        )
        print(review("patch", str(patch)))
        print(review("shot", "--selector", "#hero", "--out", str(root / "hero.png")))
        assert (root / "hero.png").stat().st_size > 0

        def ready(expression):
            return retry(lambda: json.loads(ab("eval", expression)))

        ready(
            "!!document.querySelector('vega-review')?.shadowRoot.querySelector('.page-note')"
        )
        click(".page-note")
        ab("keyboard", "inserttext", "local CSP test note")
        ab("press", "Enter")
        ready(
            "document.querySelector('vega-review').shadowRoot.querySelector('.row .primary').textContent === 'Send 1 note(s)'"
        )
        click(".row .primary")
        retry(
            lambda: (
                (root / "mail.txt").exists()
                and "local CSP test note" in (root / "mail.txt").read_text()
            )
        )
        print(review("notes"))
        print(review("reply", "1", "Local reply"))
        print(review("stop"))
        assert {t["id"] for t in targets() if t["type"] == "page"} == {spectator["id"]}
        assert process.poll() is None
        print(
            "PASS: pinned tab, ownership, highlight, patch, cropped screenshot, strict-CSP note delivery, reply, targeted stop and preserved spectator."
        )
    finally:
        try:
            review("stop")
        except (RuntimeError, sp.TimeoutExpired):
            pass
        # Upstream daemonizes into a new session. Close only daemons in
        # the socket directory created inside this fixture's private state.
        sockets = root / "state/vega/browser/shared/ab"
        if sockets.is_dir():
            processes.run(
                [str(real), "close", "--all"],
                env={**env, "AGENT_BROWSER_SOCKET_DIR": str(sockets)},
            )


if __name__ == "__main__":
    main()
