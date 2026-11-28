# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

# The page's server. It serves this folder and passes /v1/* to monasd, so the
# page and the API share one origin (monasd sends no CORS headers).
#
#   python3 -I server.py [PORT]                     the recording, no radio
#   python3 -I server.py [PORT] --live [--device D] a HackRF, live
#
# PORT is 7380 when left out. With --live it starts monasd itself (it must be
# installed), on a copy of this folder's config.toml in a folder of its own,
# for the HackRF unless --device names another radio, in the census; Ctrl-C
# stops both and removes the copy. /quartz/status tells the page how that
# went, so it can say what to do.
import http.server, json, os, shutil, signal, subprocess, sys, tempfile, threading, urllib.request

# Where monasd answers. Only the tests move it.
API = os.environ.get("QUARTZ_API", "http://127.0.0.1:7373")
HERE = os.path.dirname(os.path.abspath(__file__))

args = sys.argv[1:]
LIVE = "--live" in args
DEVICE = args[args.index("--device") + 1] if "--device" in args and args.index("--device") + 1 < len(args) else "hackrf"
PORT = next((int(a) for a in args if a.isdigit()), 7380)
# The radio as people call it, for what is said about it.
RADIO = {"hackrf": "HackRF", "rtlsdr": "RTL-SDR", "plutosky": "PlutoSky"}.get(DEVICE, DEVICE)

# What the page is told: live or not, and how monasd is doing.
status = {"live": LIVE, "monasd": None, "last": ""}
lock = threading.Lock()


def say(*lines):
    print("\n".join(lines), flush=True)


class H(http.server.SimpleHTTPRequestHandler):
    def do_GET(self):
        if self.path.startswith("/quartz/status"):
            with lock:
                body = json.dumps(status).encode()
            self.reply(200, body)
        elif self.path.startswith("/v1/"):
            try:
                with urllib.request.urlopen(API + self.path, timeout=3) as r:
                    body = r.read(); code = r.status
            except Exception as e:
                body = json.dumps({"error": str(e)}).encode(); code = 502
            self.reply(code, body)
        else:
            super().do_GET()

    def reply(self, code, body):
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.end_headers(); self.wfile.write(body)

    def end_headers(self):
        self.send_header("Cache-Control", "no-store")
        super().end_headers()

    def log_message(self, *a):
        pass


def answering():
    try:
        with urllib.request.urlopen(API + "/v1/health", timeout=1):
            return True
    except Exception:
        return False


def start_monasd():
    """Starts monasd on a copy of the config, or says why it did not. Returns
    the process and the copy's folder, or (None, None)."""
    if answering():
        with lock:
            status["monasd"] = "running"
        say(f"monasd already answers at {API}; the page uses that one.")
        return None, None
    exe = shutil.which("monasd")
    if not exe:
        with lock:
            status["monasd"] = "not installed"
        say("monasd is not installed. Install it with",
            "    cargo install monasd --locked",
            "and libhackrf with your distribution's packages (the README says more), then run this again.")
        return None, None
    folder = tempfile.mkdtemp(prefix="quartz-")
    cfg = os.path.join(folder, "quartz.toml")
    shutil.copyfile(os.path.join(HERE, "config.toml"), cfg)
    proc = subprocess.Popen([exe, "--config", cfg, "--device", DEVICE, "--mode", "net_census"],
                            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True,
                            # Its own session: Ctrl-C reaches it once, from stop(), after the page has stopped.
                            start_new_session=True)
    with lock:
        status["monasd"] = "running"
    say(f"Starting monasd ({RADIO}, the census) on a copy of config.toml.")

    def watch():
        for line in proc.stderr:
            line = line.rstrip()
            if line:
                with lock:
                    status["last"] = line
                say("  " + line)
        code = proc.wait()
        with lock:
            status["monasd"] = "stopped"
            last = status["last"]
        if stopping.is_set():
            return
        say("", f"monasd stopped (exit {code}): {last or 'it said nothing'}")
        if "no radio" in last.lower():
            say(f"Plug the {RADIO} in, and run python3 -I server.py --live again.")
        say(f"The recording still plays: http://127.0.0.1:{PORT}/?fixture&speed=10")

    threading.Thread(target=watch, daemon=True).start()
    return proc, folder


stopping = threading.Event()


def stop(proc, folder):
    stopping.set()
    if proc and proc.poll() is None:
        proc.send_signal(signal.SIGINT)
        try:
            proc.wait(timeout=8)
        except subprocess.TimeoutExpired:
            proc.kill(); proc.wait()
    if folder:
        shutil.rmtree(folder, ignore_errors=True)


def interrupted(*_):
    raise KeyboardInterrupt


os.chdir(HERE)
signal.signal(signal.SIGTERM, interrupted)
try:
    server = http.server.ThreadingHTTPServer(("127.0.0.1", PORT), H)
except OSError as e:
    sys.exit(f"Port {PORT} is taken ({e.strerror}): another server.py is probably running."
             f" Stop it, or give another port: python3 -I server.py {PORT + 1}" + (" --live" if LIVE else ""))
if LIVE:
    say("The flaw in the quartz, live", "",
        f"  Open:           http://127.0.0.1:{PORT}/",
        f"  The recording:  http://127.0.0.1:{PORT}/?fixture&speed=10", "")
    proc, folder = start_monasd()
    say("Ctrl-C stops it all.")
else:
    say("The flaw in the quartz", "",
        f"  The recording:  http://127.0.0.1:{PORT}/?fixture&speed=10",
        "  Live:           python3 -I server.py --live, with a HackRF plugged in", "",
        "Ctrl-C stops it.")
    proc, folder = None, None
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
finally:
    server.server_close()
    stop(proc, folder)
