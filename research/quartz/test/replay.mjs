// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

// The page's own script against a recording, played as the page plays it
// (?fixture), with a stubbed document and canvas that remember every string
// drawn or written. Checks that no name and no part of an address ever
// reaches the screen, that the "same quartz?" links are exactly the ones the
// rule gives, and that a monasd that does not answer is said so.
// Prints PASS or FAIL.
//
//   node test/replay.mjs index.html FIXTURE.json      (from research/quartz/)
import fs from "fs";
import path from "path";
import vm from "vm";

const [, , htmlPath, fixturePath] = process.argv;
const src = fs.readFileSync(htmlPath, "utf8").split("<script>")[1].split("</script>")[0];
const base = path.dirname(path.resolve(htmlPath));
const fx = JSON.parse(fs.readFileSync(fixturePath, "utf8"));
const fail = [];

// Run the page's script in a fresh context for `seconds` of simulated time.
function run({ search, fetchImpl, seconds }) {
  const drawn = [];
  let now = 0;
  const stub = new Proxy(function () {}, {
    get: (t, k) => (k === Symbol.toPrimitive ? () => 0 : stub),
    apply: () => stub,
    set: () => true,
  });
  const ctx = new Proxy({}, {
    get: (t, k) => {
      if (k === "fillText" || k === "strokeText") return s => drawn.push(String(s));
      if (k === "measureText") return s => ({ width: String(s).length * 10 });
      return stub;
    },
    set: () => true,
  });
  const element = () => {
    const e = { style: { setProperty() {} }, getContext: () => ctx, width: 0, height: 0 };
    let text = "", html = "";
    Object.defineProperty(e, "textContent", { get: () => text, set: v => { text = String(v); drawn.push(text); } });
    Object.defineProperty(e, "innerHTML", { get: () => html, set: v => { html = String(v); drawn.push(html); } });
    return e;
  };
  const els = {};
  const timers = [];
  let frames = [];
  let fetches = 0;
  const context = {
    document: {
      getElementById: id => (els[id] ??= element()),
      createElement: () => element(),
      body: { setAttribute() {} },
      documentElement: { style: { setProperty() {} } },
    },
    innerWidth: 1920, innerHeight: 1080, devicePixelRatio: 1,
    addEventListener() {},
    location: { search },
    performance: { now: () => now },
    setInterval: (fn, ms) => timers.push({ fn, ms, next: ms }),
    setTimeout: (fn, ms) => timers.push({ fn, ms, next: now + ms, once: true }),
    requestAnimationFrame: fn => frames.push(fn),
    fetch: async url => { fetches++; return fetchImpl(url); },
    URLSearchParams, Math, JSON, Map, Set, Promise, console,
  };
  context.window = context;
  vm.createContext(context);
  vm.runInContext(src, context);
  const step = async () => {
    for (const t of timers) if (!t.done && now >= t.next) {
      if (t.once) t.done = true; else t.next += t.ms;
      await t.fn();
    }
    const due = frames; frames = [];
    for (const f of due) f(now);
    await new Promise(r => setImmediate(r));
  };
  return (async () => {
    await new Promise(r => setImmediate(r));
    while (now < seconds * 1000) { now += 250; await step(); }
    return { drawn, els, fetches, q: context.__quartz };
  })();
}

const serveFixture = async url => {
  const file = path.resolve(base, url.split("?")[0]);
  return { json: async () => JSON.parse(fs.readFileSync(file, "utf8")) };
};
const span = Math.max(...fx.polls.map(p => p.t)) + 15;
const played = await run({ search: `?fixture=${path.relative(base, path.resolve(fixturePath))}`, fetchImpl: serveFixture, seconds: span });

// 1. Nothing personal reaches the screen.
const devs = fx.polls.flatMap(p => p.census.devices || []);
const addresses = [...new Set(devs.map(d => d.address))];
const names = [...new Set(devs.map(d => d.name).filter(Boolean))];
// A name is drawn cut and uppercased, so its words are looked for too.
const words = [...new Set(names.flatMap(n => n.toLowerCase().split(/[^a-z0-9']+/)).filter(w => w.length >= 4))];
for (const s of played.drawn) {
  const low = s.toLowerCase();
  if (/([0-9a-f]{2}:){2,}/i.test(s)) fail.push(`an address run drawn: ${JSON.stringify(s)}`);
  if (/#\d+/.test(s)) fail.push(`a masked address drawn: ${JSON.stringify(s)}`);
  if (/\.\.[0-9a-f]{2}/i.test(s)) fail.push(`an address tail drawn: ${JSON.stringify(s)}`);
  for (const a of addresses) if (a.length > 3 && low.includes(a.toLowerCase())) fail.push(`address ${a} drawn: ${JSON.stringify(s)}`);
  for (const n of names) if (low.includes(n.toLowerCase())) fail.push(`name ${JSON.stringify(n)} drawn: ${JSON.stringify(s)}`);
  for (const w of words) if (low.includes(w)) fail.push(`a word of a name, ${JSON.stringify(w)}, drawn: ${JSON.stringify(s)}`);
  if (s.includes("NaN")) fail.push(`NaN drawn: ${JSON.stringify(s)}`);
}

// 2. A device with no offset or no index is not drawn.
const q = played.q;
if (!q) fail.push("window.__quartz is not there");
else for (const d of q.devices()) if (!d.crystal_offset_ppm || !d.modulation_index)
  fail.push(`${d.address} drawn without an offset or an index`);

// 3. A device with no company is labelled UNNAMED, whatever it is called.
if (devs.some(d => !d.company && d.crystal_offset_ppm && d.modulation_index) && !played.drawn.some(s => s.includes("UNNAMED")))
  fail.push("a device with no company was not labelled UNNAMED");

// 4. The links are exactly the rule's. A rotating address that fell silent
// (gone from the answer, or not heard for over 20 s) and a new one of the
// same company that started within 30 s of it, within 2 combined sigma on
// both axes; one match reads NEW ADDRESS, SAME QUARTZ?, several read
// "N CRYSTALS MATCH". Counted for new addresses first heard in the last
// 600 s, at the end of the recording.
const SILENT_S = 20, SWAP_S = 30, MEMORY_S = 600;
const rotating = d => d.kind === "RPA" || d.kind === "NRPA";
const sep = (a, b) => Math.hypot(
  (a.crystal_offset_ppm.value - b.crystal_offset_ppm.value) / Math.hypot(a.crystal_offset_ppm.sigma, b.crystal_offset_ppm.sigma),
  (a.modulation_index.value - b.modulation_index.value) / Math.hypot(a.modulation_index.sigma, b.modulation_index.sigma));
const heard = new Map();
let T = 0, inLast = new Set();
for (const p of fx.polls) {
  T = p.t;
  const fresh = (p.census.devices || []).filter(d => d.crystal_offset_ppm && d.modulation_index);
  inLast = new Set(fresh.map(d => d.address));
  for (const d of fresh) {
    const h = heard.get(d.address);
    heard.set(d.address, { first: h ? h.first : T - d.first_seen_s, last: T - d.last_seen_s, d });
  }
}
const want = new Set(), counts = new Map();
for (const [xa, x] of heard) {
  // The new address must itself be live: a link to a point no longer drawn says nothing.
  if (!rotating(x.d) || x.first < T - MEMORY_S || !inLast.has(xa) || T - x.last > SILENT_S) continue;
  const c = [...heard].filter(([ga, g]) => ga !== xa && rotating(g.d)
    && (g.d.company || "") === (x.d.company || "")
    && (!inLast.has(ga) || T - g.last > SILENT_S)
    && x.first - g.last >= -2 && x.first - g.last <= SWAP_S && sep(g.d, x.d) <= 2);
  for (const [ga] of c) want.add(`${ga} -> ${xa}`);
  if (c.length) counts.set(xa, c.length);
}
if (q) {
  const got = new Set(q.links().map(l => `${l.from.address} -> ${l.to}`));
  if ([...want].sort().join() !== [...got].sort().join())
    fail.push(`links ${JSON.stringify([...got].sort())}, the rule gives ${JSON.stringify([...want].sort())}`);
  for (const k of new Set(counts.values())) {
    const label = k === 1 ? "NEW ADDRESS, SAME QUARTZ?" : `${k} CRYSTALS MATCH`;
    if (!played.drawn.includes(label)) fail.push(`a link was not labelled ${label}`);
  }
}

// 4b. A device with no company is called by the vendor its address's own
// prefix names, never by the session number or the bytes.
const vendorOf = d => d.vendor || ((/^(.*?)(·\w+)?\s+(#\d+|#-|\.\.[0-9a-f]{2}:[0-9a-f]{2})$/i.exec(d.address) || [])[1]);
if (q && q.label) {
  for (const d of devs) {
    const v = vendorOf(d);
    if (d.company || !v || /^(RPA|NRPA|static|public)$/i.test(v)) continue;
    if (!q.label(d).toUpperCase().startsWith(v.toUpperCase().slice(0, 10))) fail.push(`${JSON.stringify(v)} labelled ${JSON.stringify(q.label(d))}`);
  }
} else if (q) fail.push("window.__quartz.label is not there");

// 5. A monasd that does not answer is said so, and the page keeps asking.
const dead = await run({ search: "", fetchImpl: async () => { throw new Error("refused"); }, seconds: 6 });
if (!dead.drawn.includes("monasd is not answering")) fail.push("an unanswering monasd was not said so");
if (dead.fetches < 5) fail.push(`the page stopped asking: ${dead.fetches} requests in 6 s`);

console.log(fail.length ? "FAIL\n" + [...new Set(fail)].join("\n") : `PASS (${want.size} link${want.size === 1 ? "" : "s"})`);
process.exit(fail.length ? 1 : 0);
