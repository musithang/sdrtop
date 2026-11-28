# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

# How often two different devices land within 2 sigma of each other: the
# chance behind every "same quartz?" the page draws.
#
#   python3 -I chance.py RECORDING.json
#
# Two rotating addresses of the same company that were both heard over a
# common stretch of 10 s or more are two transmitters: a device that rotates
# stops the old address before it starts the new one. (A device advertising
# several sets at once is the exception, and would count here as several.)
# For every such pair, their latest readings are compared as the page
# compares them: within 2 combined sigma on both axes, or not.
import json, math, sys
from collections import defaultdict

if len(sys.argv) != 2:
    sys.exit("usage: python3 -I chance.py RECORDING.json")
with open(sys.argv[1]) as f:
    rec = json.load(f)

heard = {}
for p in rec["polls"]:
    t = p["t"]
    for d in p["census"]["devices"]:
        if not (d.get("crystal_offset_ppm") and d.get("modulation_index")):
            continue
        h = heard.get(d["address"])
        heard[d["address"]] = {"first": h["first"] if h else t - d["first_seen_s"], "last": t - d["last_seen_s"], "d": d}


def sep(a, b):
    """Combined-sigma distance, or None when an axis has no uncertainty to
    compare by (a reading with sigma 0 is not a measurement)."""
    p, q = a["crystal_offset_ppm"], b["crystal_offset_ppm"]
    h, k = a["modulation_index"], b["modulation_index"]
    sp, sh = math.hypot(p["sigma"], q["sigma"]), math.hypot(h["sigma"], k["sigma"])
    if not (sp > 0 and sh > 0):
        return None
    return math.hypot((p["value"] - q["value"]) / sp, (h["value"] - k["value"]) / sh)


rotating = [(a, h) for a, h in heard.items() if h["d"]["kind"] in ("RPA", "NRPA")]
pairs, within = defaultdict(int), defaultdict(int)
for i, (a, x) in enumerate(rotating):
    for b, y in rotating[i + 1:]:
        company = x["d"].get("company") or ""
        if company != (y["d"].get("company") or ""):
            continue
        if min(x["last"], y["last"]) - max(x["first"], y["first"]) < 10:
            continue
        s = sep(x["d"], y["d"])
        if s is None:
            continue
        pairs[company] += 1
        within[company] += s <= 2

n, m = sum(pairs.values()), sum(within.values())
print(f"pairs {n}, within 2 sigma {m}, share {m / n:.3f}" if n else "pairs 0, within 2 sigma 0")
for company in sorted(pairs, key=lambda c: -pairs[c]):
    if pairs[company] >= 10:
        print(f"  {company or 'no company'}: {within[company]} of {pairs[company]} ({within[company] / pairs[company]:.3f})")
