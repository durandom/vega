# A minimal DevTools HTTP endpoint for the plugin's tests: answers
# /json/version with 200, or closes the connection when <fix>/hang exists,
# so the CDP probe sees Cdp/Held. Started by the fake chromium with
# <port> <fix>; exits by itself after two minutes as a backstop.
import http.server
import os
import socketserver
import sys
import threading

PORT = int(sys.argv[1])
FIX = sys.argv[2]


class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if os.path.exists(os.path.join(FIX, "hang")):
            self.connection.close()
            return
        body = b'{"Browser":"fake"}'
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *a):
        pass


socketserver.TCPServer.allow_reuse_address = True
with socketserver.TCPServer(("127.0.0.1", PORT), H) as server:
    with open(os.path.join(FIX, "responders"), "a") as f:
        f.write(f"{os.getpid()}\n")
    threading.Timer(120, lambda: os._exit(0)).start()
    server.serve_forever()
