# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

# Serves this folder and passes /v1/* to monasd on 127.0.0.1:7373, so the
# page and the API share one origin (monasd sends no CORS headers).
#
#   python3 -I server.py [PORT]      (7380 when left out)
import http.server, urllib.request, sys, os
API = "http://127.0.0.1:7373"
class H(http.server.SimpleHTTPRequestHandler):
    def do_GET(self):
        if self.path.startswith("/v1/"):
            try:
                with urllib.request.urlopen(API + self.path, timeout=3) as r:
                    body = r.read(); code = r.status
            except Exception as e:
                body = ('{"error":"%s"}' % str(e).replace('"', "'")).encode(); code = 502
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Cache-Control", "no-store")
            self.end_headers(); self.wfile.write(body)
        else:
            super().do_GET()
    def end_headers(self):
        if not self.path.startswith("/v1/"):
            self.send_header("Cache-Control", "no-store")
        super().end_headers()
    def log_message(self, *a): pass
os.chdir(os.path.dirname(os.path.abspath(__file__)))
http.server.ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1]) if len(sys.argv) > 1 else 7380), H).serve_forever()
