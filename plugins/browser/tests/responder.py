# A minimal DevTools HTTP endpoint for the plugin's tests: answers
# /json/version with 200, or closes the connection when <fix>/hang exists,
# so the CDP probe sees Cdp/Held. Started by the fake chromium with
# <port> <fix>; exits by itself after two minutes as a backstop.
import base64
import hashlib
import json
import struct
import http.server
import os
import socketserver
import sys
import threading

PORT = int(sys.argv[1])
FIX = sys.argv[2]


def page(target, url):
    return dict(targetId=target, type="page", title=url, url=url)


pages = {"foreign": page("foreign", "https://foreign.invalid/")}
next_id = 0


class H(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self):
        if os.path.exists(os.path.join(FIX, "hang")):
            self.connection.close()
            return
        if self.headers.get("Upgrade", "").lower() == "websocket":
            self.websocket()
            return
        body = json.dumps({"Browser": "Chrome/146.0.0.0", "webSocketDebuggerUrl":
            f"ws://127.0.0.1:{PORT}/devtools/browser/fade"}).encode()
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def websocket(self):
        key = self.headers["Sec-WebSocket-Key"] + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"
        self.send_response_only(101)
        self.send_header("Upgrade", "websocket")
        self.send_header("Connection", "Upgrade")
        self.send_header("Sec-WebSocket-Accept", base64.b64encode(hashlib.sha1(key.encode()).digest()).decode())
        self.end_headers()
        while True:
            header = self.rfile.read(2)
            if len(header) != 2 or header[0] != 0x81:
                return
            length = header[1] & 127
            if length == 126:
                length = int.from_bytes(self.rfile.read(2), "big")
            mask = self.rfile.read(4)
            frame = self.rfile.read(length)
            request = json.loads(bytes(v ^ mask[i % 4] for i, v in enumerate(frame)))
            method = request["method"]
            target = request.get("params", {}).get("targetId")
            global next_id
            if method == "Target.createTarget":
                next_id += 1
                target = f"owned{next_id}"
                pages[target] = page(target, "about:blank")
                result = {"targetId":target}
            elif method == "Target.getTargets":
                result = {"targetInfos":list(pages.values())}
            elif method == "Target.getTargetInfo" and target in pages:
                result = {"targetInfo":pages[target]}
            elif method == "Target.closeTarget":
                result = {"success": pages.pop(target, None) is not None}
            elif method == "Target.attachToTarget":
                result = {"sessionId":target}
            elif method == "Runtime.evaluate":
                result = {"target":request.get("sessionId")}
            elif method == "Browser.getWindowForTarget":
                result = {"windowId":1}
            else:
                result = {}
            body = json.dumps({"id":request["id"], "result":result}).encode()
            length = bytes([len(body)]) if len(body) < 126 else b"\x7e" + struct.pack(">H", len(body))
            self.wfile.write(b"\x81" + length + body)

    def log_message(self, *a):
        pass


socketserver.TCPServer.allow_reuse_address = True
with socketserver.ThreadingTCPServer(("127.0.0.1", PORT), H) as server:
    with open(os.path.join(FIX, "responders"), "a") as f:
        f.write(f"{os.getpid()}\n")
    threading.Timer(120, lambda: os._exit(0)).start()
    server.serve_forever()
