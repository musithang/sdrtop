// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

// The folder's Python scripts against what can go wrong around them: a
// capture interrupted or fed a bad answer, a recording whose times are wall
// clock, a sigma of zero. A stand-in for monasd answers on a free port.
// Prints PASS or FAIL.
//
//   node test/scripts.mjs        (from research/quartz/)
import fs from "fs";
import http from "http";
import os from "os";
import path from "path";
import { spawn, execFileSync } from "child_process";

const here = path.resolve(path.dirname(new URL(import.meta.url).pathname), "..");
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "quartz-scripts-"));
const fail = [];

// A stand-in for monasd: the provenance with a serial number, and a census
// that answers garbage once.
let asked = 0;
const census = () => ({
  testimony: { stale: false, feed_loss: false, absent: null, at_unix_ms: 1795800000000 },
  devices: [{ address: "Apple #1", kind: "RPA", name: null, company: "Apple, Inc.", packets: 3 + asked,
    crystal_offset_ppm: { value: 1.2, sigma: 0.05 }, modulation_index: { value: 0.5, sigma: 0.001 },
    first_seen_s: asked, last_seen_s: 0 }],
});
const server = http.createServer((req, res) => {
  if (req.url === "/v1/provenance") return res.end(JSON.stringify({ reference: "unreferenced, no reference established", device: "HackRF One  serial 0000000000000000deadbeef" }));
  asked++;
  res.end(asked === 2 ? "not json at all" : JSON.stringify(census()));
});
await new Promise(r => server.listen(0, "127.0.0.1", r));
const port = String(server.address().port);

const python = (args, killAfterMs) => new Promise(resolve => {
  const p = spawn("python3", ["-I", ...args], { stdio: ["ignore", "pipe", "pipe"] });
  let err = "";
  p.stderr.on("data", d => (err += d));
  if (killAfterMs) setTimeout(() => p.kill("SIGINT"), killAfterMs);
  p.on("exit", code => resolve({ code, err }));
});
const read = f => { try { return JSON.parse(fs.readFileSync(f, "utf8")); } catch { return null; } };

// 1. A short capture survives a bad answer and names the radio without its serial.
const one = path.join(tmp, "one.json");
const r1 = await python([path.join(here, "capture.py"), one, "0.07", port]);
const c1 = read(one);
if (r1.code !== 0) fail.push(`capture.py stopped at a bad answer: ${r1.err.trim().split("\n").pop()}`);
else if (!c1 || c1.polls.length < 3) fail.push(`capture.py kept ${c1 ? c1.polls.length : "no"} polls in 4 s`);
else if (c1.device !== "HackRF One") fail.push(`capture.py named the radio ${JSON.stringify(c1.device)}`);

// 2. Ctrl-C keeps what was captured.
const two = path.join(tmp, "two.json");
await python([path.join(here, "capture.py"), two, "5", port], 3500);
const c2 = read(two);
if (!c2 || c2.polls.length < 2) fail.push(`an interrupted capture kept ${c2 ? c2.polls.length : "nothing"}`);
server.close();

// 3. anonymize.py starts any recording's times at zero, whatever clock it came with.
const epoch = { reference: "unreferenced", polls: [1795800000.5, 1795800001.5, 1795800002.5].map(t => ({ t, census: census() })) };
fs.writeFileSync(path.join(tmp, "epoch.json"), JSON.stringify(epoch));
execFileSync("python3", ["-I", path.join(here, "anonymize.py"), path.join(tmp, "epoch.json"), path.join(tmp, "epoch-anon.json")]);
const ts = read(path.join(tmp, "epoch-anon.json")).polls.map(p => p.t);
if (ts.join() !== "0,1,2") fail.push(`anonymize.py kept times ${ts}, wanted 0,1,2`);

// 4. A sigma of zero is no measurement to compare: chance.py says so rather than dividing by it.
const zero = JSON.parse(JSON.stringify(epoch));
for (const p of zero.polls) {
  const b = { ...census().devices[0], address: "d002", crystal_offset_ppm: { value: 1.2, sigma: 0 }, modulation_index: { value: 0.5, sigma: 0 } };
  p.census.devices.push(b);
  p.t -= 1795800000;
  for (const d of p.census.devices) { d.first_seen_s = p.t + 20; d.last_seen_s = 0; }
}
for (const p of zero.polls) for (const d of p.census.devices) { d.crystal_offset_ppm.sigma = 0; d.modulation_index.sigma = 0; }
fs.writeFileSync(path.join(tmp, "zero.json"), JSON.stringify(zero));
try {
  const out = execFileSync("python3", ["-I", path.join(here, "chance.py"), path.join(tmp, "zero.json")], { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
  if (!/^pairs 0, within 2 sigma 0/.test(out)) fail.push(`chance.py on a zero sigma said: ${out.trim().split("\n")[0]}`);
} catch (e) { fail.push(`chance.py failed on a zero sigma: ${String(e.stderr || e.message).trim().split("\n").pop()}`); }

fs.rmSync(tmp, { recursive: true, force: true });
console.log(fail.length ? "FAIL\n" + fail.join("\n") : "PASS");
process.exit(fail.length ? 1 : 0);
