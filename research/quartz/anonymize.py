# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

# Makes a census recording safe to share: per device, only what the page
# draws and the method counts; every address replaced by d001, d002, ... in
# the order it first appears; the reference cut to its first clause; no
# wall-clock time.
#
#   python3 -I anonymize.py [--every SECONDS] IN.json OUT.json
#
# --every keeps one poll each SECONDS (the first at or after each step), for
# a recording small enough to share; numbers are rounded to 5 decimals,
# finer than any sigma the census gives. The vendor a masked or
# registrant-form address names ("GREE ... #13", "Apple ..ee:ff") is kept as
# `vendor`: a company, not a person; the session number and the bytes are not.
#
# The tokens are not a function of the address. A hash would be: a 48-bit
# address with a known vendor prefix can be found from its hash by trying
# them all. The mapping lives in this process and is never written.
import json, re, sys

KEPT = ("kind", "company", "company_id", "tx_power_dbm", "packets", "crc_failed",
        "crc_pass_pct", "best_snr_db", "mean_snr_db", "crystal_offset_ppm",
        "modulation_index", "first_seen_s", "last_seen_s")


VENDOR = re.compile(r"^(.*?)(·\w+)?\s+(#\d+|#-|\.\.[0-9a-f]{2}:[0-9a-f]{2})$", re.I)


def vendor_of(address):
    m = VENDOR.match(address or "")
    # An unlisted prefix comes as its bytes ("1C-2B-3D #12"): part of an
    # address, not a maker.
    if (not m or re.fullmatch(r"RPA|NRPA|static|public", m.group(1), re.I)
            or re.search(r"([0-9a-f]{2}:){2}", m.group(1), re.I)
            or re.fullmatch(r"[0-9a-f]{2}(-[0-9a-f]{2}){2}", m.group(1), re.I)):
        return None
    return m.group(1)


def rounded(v):
    if isinstance(v, float):
        return round(v, 5)
    if isinstance(v, dict):
        return {k: rounded(x) for k, x in v.items()}
    if isinstance(v, list):
        return [rounded(x) for x in v]
    return v


def anonymize(rec, every=0.0):
    tokens = {}

    def token(address):
        if address not in tokens:
            tokens[address] = "d%03d" % (len(tokens) + 1)
        return tokens[address]

    polls, step = [], None
    for p in rec.get("polls", []):
        if every:
            k = int((p.get("t") or 0) // every)
            if k == step:
                continue
            step = k
        census = p.get("census") or {}
        testimony = {k: v for k, v in (census.get("testimony") or {}).items() if k != "at_unix_ms"}
        devices = []
        for d in census.get("devices") or []:
            kept = {"address": token(d.get("address"))}
            v = vendor_of(d.get("address"))
            if v:
                kept["vendor"] = v
            kept.update({k: rounded(d[k]) for k in KEPT if k in d})
            devices.append(kept)
        polls.append({"t": p.get("t"), "census": {"testimony": testimony, "devices": devices}})
    reference = (rec.get("reference") or "not said").split(",")[0]
    # The radio by its name only, whatever the capture kept: a serial number
    # names one unit.
    device = re.split(r"\s+serial\s", rec.get("device") or "")[0].strip() or None
    return {"reference": reference, "device": device, "polls": polls}


if __name__ == "__main__":
    args, every = sys.argv[1:], 0.0
    if args[:1] == ["--every"] and len(args) >= 2:
        every, args = float(args[1]), args[2:]
    if len(args) != 2:
        sys.exit("usage: python3 -I anonymize.py [--every SECONDS] IN.json OUT.json")
    with open(args[0]) as f:
        rec = json.load(f)
    out = anonymize(rec, every)
    with open(args[1], "w") as f:
        json.dump(out, f, separators=(",", ":"))
    n = len({d["address"] for p in out["polls"] for d in p["census"]["devices"]})
    print(f"{len(out['polls'])} polls, {n} devices, written to {args[1]}")
