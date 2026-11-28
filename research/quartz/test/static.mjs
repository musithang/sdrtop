// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

// The folder's own rules, checked on the files as text: nothing fetched
// from elsewhere, nothing kept past a session, nothing personal in the
// config, and the repository's conventions. Prints PASS or FAIL.
//
//   node test/static.mjs        (from research/quartz/)
import fs from "fs";
import path from "path";
import os from "os";
import { execFileSync } from "child_process";

const here = path.resolve(path.dirname(new URL(import.meta.url).pathname), "..");
const research = path.resolve(here, "..");
const fail = [];
// Spelled so this file does not trip its own checks.
const EM_DASH = String.fromCharCode(0x2014), NOTES = "dev" + "_docs";
const read = f => {
  try { return fs.readFileSync(path.join(here, f), "utf8"); }
  catch { fail.push(`${f} is missing`); return ""; }
};

// The page: no request leaves its origin, nothing outlives the session.
const page = read("index.html");
for (const m of page.matchAll(/https?:\/\/[^\s"'<>)]+/g)) {
  const before = page.slice(Math.max(0, m.index - 8), m.index);
  if (!/xmlns(:\w+)?="$/.test(before)) fail.push(`index.html fetches or links ${m[0]}`);
}
for (const word of ["localStorage", "sessionStorage", "document.cookie", "indexedDB", "fonts.googleapis", "PortaPack"])
  if (page.includes(word)) fail.push(`index.html uses ${word}`);

// The config: the radio and masked addresses, nothing about the place.
const config = read("config.toml");
for (const word of ["[station]", "latitude", "longitude", "token"])
  if (config.includes(word)) fail.push(`config.toml holds ${word}`);
if (!/\[monasd\][^[]*addresses\s*=\s*"masked"/.test(config)) fail.push(`config.toml does not mask addresses under [monasd]`);

// Every file of the research: no em dash, nothing pointing at the
// gitignored notes, and a licence on every source file.
const walk = d => fs.readdirSync(d, { withFileTypes: true }).flatMap(e =>
  e.isDirectory() ? walk(path.join(d, e.name)) : [path.join(d, e.name)]);
for (const f of walk(research)) {
  const rel = path.relative(research, f);
  if (/raw.*\.json$/.test(rel)) continue;
  if (/\.(png|jpe?g|svg|ico)$/i.test(rel)) continue;
  const text = fs.readFileSync(f, "utf8");
  if (text.includes(EM_DASH)) fail.push(`${rel} has an em dash`);
  if (text.includes(NOTES)) fail.push(`${rel} points into the gitignored notes`);
  if (/\.(py|mjs|html)$/.test(f) && !text.split("\n").slice(0, 3).some(l => l.includes("SPDX-License-Identifier: GPL-3.0-or-later")))
    fail.push(`${rel} has no SPDX header in its first three lines`);
}

// A recording that leaves this folder: only the kept fields, every address a
// token, and nothing anywhere shaped like an address, a name or a place.
const KEPT = new Set(["address", "vendor", "kind", "company", "company_id", "tx_power_dbm", "packets", "crc_failed",
  "crc_pass_pct", "best_snr_db", "mean_snr_db", "crystal_offset_ppm", "modulation_index", "first_seen_s", "last_seen_s"]);
function checkRecording(label, text) {
  let rec;
  try { rec = JSON.parse(text); } catch { fail.push(`${label} is not JSON`); return; }
  for (const p of rec.polls || []) {
    if ("at_unix_ms" in (p.census?.testimony || {})) fail.push(`${label} keeps a wall-clock time`);
    for (const d of p.census?.devices || []) {
      for (const k of Object.keys(d)) if (!KEPT.has(k)) fail.push(`${label} keeps a device's ${k}`);
      if (!/^d\d{3}$/.test(d.address)) fail.push(`${label} keeps an address ${JSON.stringify(d.address)}`);
    }
  }
  const strings = [];
  (function walk(v) {
    if (typeof v === "string") strings.push(v);
    else if (v && typeof v === "object") for (const x of Object.values(v)) walk(x);
  })(rec);
  for (const s of strings) {
    if (/serial|[0-9a-f]{12,}/i.test(s)) fail.push(`${label} holds a serial number ${JSON.stringify(s)}`);
    if (/([0-9a-f]{2}:){2,}|\.\.[0-9a-f]{2}|#\d+|\b[0-9a-f]{2}-[0-9a-f]{2}-[0-9a-f]{2}\b/i.test(s)) fail.push(`${label} holds an address form ${JSON.stringify(s)}`);
    if (/\b(lat|lon)\w*\b.*\d{1,3}\.\d+/i.test(s)) fail.push(`${label} holds a coordinate ${JSON.stringify(s)}`);
    if (/name/i.test(s)) fail.push(`${label} holds a name ${JSON.stringify(s)}`);
  }
}
checkRecording("fixture.json", read("fixture.json"));
// The anonymiser itself, on every address form monasd can answer with.
try {
  const out = path.join(fs.mkdtempSync(path.join(os.tmpdir(), "quartz-")), "anon.json");
  execFileSync("python3", ["-I", path.join(here, "anonymize.py"), path.join(here, "test/anon_input.json"), out]);
  checkRecording("anonymize.py's output", fs.readFileSync(out, "utf8"));
  const tokens = new Set(JSON.parse(fs.readFileSync(out, "utf8")).polls.flatMap(p => p.census.devices.map(d => d.address)));
  if (tokens.size !== 4) fail.push(`anonymize.py gave ${tokens.size} tokens for 4 addresses`);
  const anon = JSON.parse(fs.readFileSync(out, "utf8"));
  const vendors = new Set(anon.polls.flatMap(p => p.census.devices.map(d => d.vendor)).filter(Boolean));
  if ([...vendors].join() !== "Apple") fail.push(`anonymize.py kept vendors ${JSON.stringify([...vendors])}, wanted only "Apple"`);
  if (/\d\.\d{6,}/.test(fs.readFileSync(out, "utf8"))) fail.push("anonymize.py left a number unrounded");
  const thin = path.join(path.dirname(out), "thin.json");
  execFileSync("python3", ["-I", path.join(here, "anonymize.py"), "--every", "2", path.join(here, "test/anon_input.json"), thin]);
  const ts = JSON.parse(fs.readFileSync(thin, "utf8")).polls.map(p => p.t);
  if (ts.join() !== "0,2") fail.push(`anonymize.py --every 2 kept polls at ${ts}, wanted 0,2`);
} catch (e) { fail.push(`anonymize.py did not run: ${String(e.message).split("\n")[0]}`); }

// The chance figure the method quotes, on the hand-made recording: two pairs
// of different same-company rotating devices heard together for 10 s or
// more, one of them within 2 sigma.
try {
  const out = execFileSync("python3", ["-I", path.join(here, "chance.py"), path.join(here, "test/tiny.json")], { encoding: "utf8" });
  if (!out.includes("pairs 2, within 2 sigma 1")) fail.push(`chance.py on tiny.json said: ${out.trim().split("\n")[0]}`);
} catch (e) { fail.push(`chance.py did not run: ${String(e.message).split("\n")[0]}`); }

console.log(fail.length ? "FAIL\n" + fail.join("\n") : "PASS");
process.exit(fail.length ? 1 : 0);
