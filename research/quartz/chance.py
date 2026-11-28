# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

# How often two different devices land within 2 sigma of each other: the
# chance behind every "same quartz?" the page draws.
#
#   python3 -I chance.py RECORDING.json
#
# Two addresses a device can change (RPA, NRPA, static random) of the same
# company that were both heard over a common stretch of 10 s or more are two
# transmitters: a device that changes its address stops the old one before
# it starts the new one. (A device advertising several sets at once is the
# exception, and would count here as several.) For every such pair, the
# packets of the last minute they shared (at most 60 s) are compared as the
# page compares a swap: within 2 combined sigma on both axes, or not.
import json, math, sys
from collections import defaultdict

WINDOW_S = 60
# A window with fewer packets than this is no measurement, as on the page.
MIN_PACKETS = 10

if len(sys.argv) != 2:
    sys.exit("usage: python3 -I chance.py RECORDING.json")
with open(sys.argv[1]) as f:
    rec = json.load(f)

# Per address: when it was first and last heard, and the census's running
# means, one each time the packet count moved.
heard = {}
for p in rec["polls"]:
    t = p["t"]
    for d in p["census"]["devices"]:
        if not (d.get("crystal_offset_ppm") and d.get("modulation_index")):
            continue
        h = heard.get(d["address"])
        at = t - d["last_seen_s"]
        rs = h["rs"] if h else []
        if not rs or rs[-1]["n"] != d.get("packets"):
            rs.append({"at": at, "n": d.get("packets") or 0, "d": d})
        heard[d["address"]] = {"first": h["first"] if h else t - d["first_seen_s"], "last": at, "d": d, "rs": rs}


def windowed(rs, t0, t1):
    """The mean of the packets heard between t0 and t1 as (ppm, sigma, index,
    sigma): the running mean before t0 taken out of the one at t1."""
    a = e = None
    for r in rs:
        if r["at"] <= t0:
            a = r
        if r["at"] <= t1:
            e = r
    if e is None:
        return None
    p, h = e["d"]["crystal_offset_ppm"], e["d"]["modulation_index"]
    if a is None or a["n"] >= e["n"]:
        if e["n"] < MIN_PACKETS:
            return None
        return p["value"], p["sigma"], h["value"], h["sigma"]
    k, n = e["n"] - a["n"], e["n"]
    if k < MIN_PACKETS:
        return None
    f = math.sqrt(n / k)
    ap, ah = a["d"]["crystal_offset_ppm"], a["d"]["modulation_index"]
    return ((p["value"] * n - ap["value"] * a["n"]) / k, p["sigma"] * f,
            (h["value"] * n - ah["value"] * a["n"]) / k, h["sigma"] * f)


def sep(a, b):
    """Combined-sigma distance, or None when an axis has no uncertainty to
    compare by (a reading with sigma 0 is not a measurement)."""
    sp, sh = math.hypot(a[1], b[1]), math.hypot(a[3], b[3])
    if not (sp > 0 and sh > 0):
        return None
    return math.hypot((a[0] - b[0]) / sp, (a[2] - b[2]) / sh)


changeable = [(a, h) for a, h in heard.items() if h["d"]["kind"] in ("RPA", "NRPA", "static")]
pairs, within = defaultdict(int), defaultdict(int)
for i, (a, x) in enumerate(changeable):
    for b, y in changeable[i + 1:]:
        company = x["d"].get("company") or ""
        if company != (y["d"].get("company") or ""):
            continue
        lo, hi = max(x["first"], y["first"]), min(x["last"], y["last"])
        if hi - lo < 10:
            continue
        t0 = max(lo, hi - WINDOW_S)
        wx, wy = windowed(x["rs"], t0, hi), windowed(y["rs"], t0, hi)
        if wx is None or wy is None:
            continue
        s = sep(wx, wy)
        if s is None:
            continue
        pairs[company] += 1
        within[company] += s <= 2

n, m = sum(pairs.values()), sum(within.values())
print(f"pairs {n}, within 2 sigma {m}, share {m / n:.3f}" if n else "pairs 0, within 2 sigma 0")
for company in sorted(pairs, key=lambda c: -pairs[c]):
    if pairs[company] >= 10:
        print(f"  {company or 'no company'}: {within[company]} of {pairs[company]} ({within[company] / pairs[company]:.3f})")
