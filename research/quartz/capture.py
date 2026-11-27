# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

# Records what the quartz page reads, once a second, for MINUTES: monasd's
# census of Bluetooth LE transmitters, and the reference its offsets are
# measured against. The output is raw (addresses as monasd shows them): run
# anonymize.py on it before it goes anywhere.
#
#   python3 -I capture.py OUT.json MINUTES [PORT]      (PORT 7373 when left out)
import json, sys, time, urllib.request

if len(sys.argv) not in (3, 4):
    sys.exit("usage: python3 -I capture.py OUT.json MINUTES [PORT]")
out, minutes = sys.argv[1], float(sys.argv[2])
api = "http://127.0.0.1:%s" % (sys.argv[3] if len(sys.argv) == 4 else "7373")


def ask(route):
    with urllib.request.urlopen(api + route, timeout=5) as r:
        return json.loads(r.read())


provenance = ask("/v1/provenance")
reference = provenance.get("reference") or "not said"
# The radio by its name only: its serial number names one unit.
device = (provenance.get("device") or "").split("  serial")[0].strip() or None
polls, start = [], time.monotonic()
while time.monotonic() - start < minutes * 60:
    t = time.monotonic()
    try:
        polls.append({"t": round(t - start, 3), "census": ask("/v1/net/census")})
    except OSError as e:
        print("skipped a poll:", e, file=sys.stderr)
    if len(polls) % 60 == 0 and polls:
        n = len({d.get("address") for p in polls for d in p["census"].get("devices", [])})
        print(f"{len(polls)} polls, {n} addresses so far", file=sys.stderr)
    time.sleep(max(0.0, 1.0 - (time.monotonic() - t)))
with open(out, "w") as f:
    json.dump({"reference": reference, "device": device, "polls": polls}, f)
print(f"{len(polls)} polls written to {out}; anonymize it before sharing")
