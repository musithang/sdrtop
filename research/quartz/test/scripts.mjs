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

// 5. server.py: one command for the recording, one for the radio.
const pythonBin = execFileSync("sh", ["-c", "command -v python3"], { encoding: "utf8" }).trim();
const freePort = () => new Promise(r => { const s = http.createServer(); s.listen(0, "127.0.0.1", () => { const p = s.address().port; s.close(() => r(p)); }); });
const get = (port, p) => new Promise(r => http.get({ host: "127.0.0.1", port, path: p }, res => { let b = ""; res.on("data", d => (b += d)); res.on("end", () => { try { r(JSON.parse(b)); } catch { r(null); } }); }).on("error", () => r(null)));
const sleep = ms => new Promise(r => setTimeout(r, ms));
// A server.py with PATH and the API it watches set, its stdout kept.
const serve = async (args, binDir) => {
  const port = await freePort(), api = await freePort();
  const p = spawn(pythonBin, ["-I", path.join(here, "server.py"), String(port), ...args], {
    stdio: ["ignore", "pipe", "pipe"],
    env: { PATH: `${binDir}:/usr/bin:/bin`, QUARTZ_API: `http://127.0.0.1:${api}`, QUARTZ_FAKE: path.join(binDir, "seen") },
  });
  let out = "";
  p.stdout.on("data", d => (out += d)); p.stderr.on("data", d => (out += d));
  for (let i = 0; i < 40 && !(await get(port, "/quartz/status")); i++) await sleep(100);
  return { p, port, out: () => out, stop: () => new Promise(r => { p.on("exit", r); p.kill("SIGINT"); }) };
};
const fakeBin = (name, script) => { const d = fs.mkdtempSync(path.join(tmp, name + "-")); if (script) { fs.writeFileSync(path.join(d, "monasd"), script); fs.chmodSync(path.join(d, "monasd"), 0o755); } return d; };

// 5a. Without --live: the recording's address, said at start, and no monasd started.
{
  const s = await serve([], fakeBin("none"));
  const st = await get(s.port, "/quartz/status");
  if (!s.out().includes(`http://127.0.0.1:${s.port}/?fixture&speed=10`)) fail.push(`server.py did not print the recording's address: ${JSON.stringify(s.out())}`);
  if (!s.out().includes("--live")) fail.push("server.py did not say how to go live");
  if (!st || st.live !== false) fail.push(`server.py without --live said ${JSON.stringify(st)}`);
  await s.stop();
}
// 5b. --live with no monasd installed: said, with how to install it.
{
  const s = await serve(["--live"], fakeBin("none"));
  const st = await get(s.port, "/quartz/status");
  if (!st || st.monasd !== "not installed") fail.push(`--live without monasd said ${JSON.stringify(st)}`);
  if (!s.out().includes("cargo install monasd --locked")) fail.push("--live without monasd did not say how to install it");
  await s.stop();
}
// 5c. --live, monasd finds no radio: its last words reach the status and the
// terminal; it was started on a copy of the folder's config, for the HackRF,
// in the census.
{
  const bin = fakeBin("noradio", `#!/bin/sh\necho "$@" > "$QUARTZ_FAKE"\necho "monasd: no radio found." >&2\nexit 1\n`);
  const s = await serve(["--live"], bin);
  let st = null;
  for (let i = 0; i < 30 && !(st && st.monasd === "stopped"); i++) { await sleep(100); st = await get(s.port, "/quartz/status"); }
  if (!st || st.monasd !== "stopped" || !/no radio found/.test(st.last || "")) fail.push(`--live with no radio said ${JSON.stringify(st)}`);
  if (!/no radio found/.test(s.out()) || !/plug/i.test(s.out())) fail.push(`--live with no radio did not say so in the terminal: ${JSON.stringify(s.out())}`);
  const args = fs.existsSync(path.join(bin, "seen")) ? fs.readFileSync(path.join(bin, "seen"), "utf8").trim().split(/\s+/) : [];
  const cfg = args[args.indexOf("--config") + 1];
  if (args.join(" ").replace(cfg, "CFG") !== "--config CFG --device hackrf --mode net_census") fail.push(`monasd was started with ${JSON.stringify(args)}`);
  else if (path.resolve(cfg) === path.join(here, "config.toml")) fail.push("monasd was given the folder's own config, which it would write over");
  await s.stop();
}
// 5d. --live, monasd running: Ctrl-C stops it too, and its config copy goes.
{
  const bin = fakeBin("alive", `#!/bin/sh\necho "$$ $3" > "$QUARTZ_FAKE"\ntrap 'exit 0' INT TERM\nwhile :; do sleep 0.1; done\n`);
  const s = await serve(["--live"], bin);
  let st = null;
  for (let i = 0; i < 30 && !(st && st.monasd === "running"); i++) { await sleep(100); st = await get(s.port, "/quartz/status"); }
  if (!st || st.monasd !== "running") fail.push(`--live with monasd running said ${JSON.stringify(st)}`);
  const [pid, cfg] = fs.existsSync(path.join(bin, "seen")) ? fs.readFileSync(path.join(bin, "seen"), "utf8").trim().split(" ") : [];
  await s.stop(); await sleep(300);
  let alive = false; try { process.kill(Number(pid), 0); alive = true; } catch {}
  if (!pid || alive) fail.push("Ctrl-C on server.py left monasd running");
  if (cfg && fs.existsSync(cfg)) fail.push("Ctrl-C on server.py left monasd's config copy behind");
}

// 5e. A port already taken is said in a sentence, with another to try, not a traceback.
{
  const s = await serve([], fakeBin("none"));
  const r = await new Promise(res => {
    const p = spawn(pythonBin, ["-I", path.join(here, "server.py"), String(s.port)], { stdio: ["ignore", "pipe", "pipe"] });
    let out = ""; p.stdout.on("data", d => (out += d)); p.stderr.on("data", d => (out += d));
    p.on("exit", code => res({ code, out }));
  });
  if (r.code === 0 || /Traceback/.test(r.out) || !r.out.includes(`server.py ${s.port + 1}`)) fail.push(`a taken port said: ${JSON.stringify(r.out.trim().split("\n").pop())}`);
  await s.stop();
}

fs.rmSync(tmp, { recursive: true, force: true });
console.log(fail.length ? "FAIL\n" + fail.join("\n") : "PASS");
process.exit(fail.length ? 1 : 0);
