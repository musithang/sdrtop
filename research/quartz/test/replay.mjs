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
function run({ search, fetchImpl, seconds, during = [] }) {
  const drawn = [], html = [];
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
    const e = { style: { setProperty() {} }, classList: { toggle() {}, add() {}, remove() {} }, getContext: () => ctx, width: 0, height: 0 };
    let text = "", inner = "";
    Object.defineProperty(e, "textContent", { get: () => text, set: v => { text = String(v); drawn.push(text); } });
    Object.defineProperty(e, "innerHTML", { get: () => inner, set: v => { inner = String(v); drawn.push(inner); html.push(inner); } });
    return e;
  };
  const els = {};
  const listeners = {};
  const timers = [];
  let frames = [];
  let fetches = 0;
  const asked = [];
  const context = {
    document: {
      getElementById: id => (els[id] ??= element()),
      createElement: () => element(),
      body: { setAttribute() {} },
      documentElement: { style: { setProperty() {} } },
    },
    innerWidth: 1920, innerHeight: 1080, devicePixelRatio: 1,
    addEventListener: (type, fn) => (listeners[type] ??= []).push(fn),
    location: { search },
    performance: { now: () => now },
    setInterval: (fn, ms) => timers.push({ fn, ms, next: ms }),
    setTimeout: (fn, ms) => timers.push({ fn, ms, next: now + ms, once: true }),
    requestAnimationFrame: fn => frames.push(fn),
    fetch: async url => { fetches++; asked.push(String(url)); return fetchImpl(url); },
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
    while (now < seconds * 1000) {
      now += 250;
      for (const d of during) if (!d.done && now >= d.at) { d.done = true; d.fn(context.__quartz, listeners, drawn); }
      await step();
    }
    return { drawn, html, els, fetches, asked, q: context.__quartz };
  })();
}

const serveFixture = async url => {
  const file = path.resolve(base, url.split("?")[0]);
  return { ok: true, status: 200, json: async () => JSON.parse(fs.readFileSync(file, "utf8")) };
};
const span = Math.max(...fx.polls.map(p => p.t)) + 15;
// The page's links, sampled every second of the playback.
const seen = new Map(), seenGroups = new Set();
const sample = [];
for (let at = 1000; at <= span * 1000; at += 1000) sample.push({ at, fn: Q => {
  for (const L of Q ? Q.links() : []) seen.set(`${L.from.address} -> ${L.to}`, L.k);
  for (const g of Q && Q.groups ? Q.groups() : []) seenGroups.add([...g].sort().join(" + "));
} });
const played = await run({ search: `?fixture=${path.relative(base, path.resolve(fixturePath))}`, fetchImpl: serveFixture, seconds: span, during: sample });

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
  if (s.includes("NaN") || s.includes("Infinity")) fail.push(`a non-number drawn: ${JSON.stringify(s)}`);
  if (/serial|[0-9a-f]{12,}/i.test(s)) fail.push(`a serial number drawn: ${JSON.stringify(s)}`);
  if (/\b[0-9a-f]{2}-[0-9a-f]{2}-[0-9a-f]{2}\b/i.test(s)) fail.push(`an address prefix drawn as a maker: ${JSON.stringify(s)}`);
}

// Markup in a recording stays text: what is written as HTML holds no tag
// but the page's own <b> and <br>.
for (const s of played.html) if (/<(?!\/?b>|br>)/i.test(s)) fail.push(`markup from a recording reached the page: ${JSON.stringify(s)}`);

// 1b. The receiver is named as the recording names it, its serial cut off.
if (fx.device && !played.drawn.includes(fx.device.split(/\s+serial\s/)[0].trim()))
  fail.push(`the receiver ${JSON.stringify(fx.device.split(/\s+serial\s/)[0].trim())} is not named`);

// 2. A device with no offset or no index is not drawn.
const q = played.q;
if (!q) fail.push("window.__quartz is not there");
else for (const d of q.devices()) if (!d.crystal_offset_ppm || !d.modulation_index)
  fail.push(`${d.address} drawn without an offset or an index`);

// 2b. Two readings with no uncertainty cannot be compared: their distance is
// infinite (never within 2 sigma), not NaN.
if (q && q.sep) {
  const z = { crystal_offset_ppm: { value: 1, sigma: 0 }, modulation_index: { value: 0.5, sigma: 0 } };
  const d = q.sep(z, { ...z });
  if (!(d === Infinity)) fail.push(`two zero-sigma readings are ${d} sigma apart, wanted Infinity`);
} else if (q) fail.push("window.__quartz.sep is not there");

// 3. A device with no company is labelled UNNAMED, whatever it is called.
if (devs.some(d => !d.company && d.crystal_offset_ppm && d.modulation_index) && !played.drawn.some(s => s.includes("UNNAMED")))
  fail.push("a device with no company was not labelled UNNAMED");

// 4. The links are exactly the rule's. An address that can change (RPA, NRPA
// or static random) and fell silent (gone from the answer, or not heard for
// over 20 s), and a new one of the same company that started within 30 s of
// it, compared as they were heard at the swap: the old one's last minute
// against the new one's first, within 2 combined sigma on both axes. One
// match reads NEW ADDRESS, SAME QUARTZ?, several "N CRYSTALS MATCH".
// Collected at every poll over the whole recording.
const SILENT_S = 20, SWAP_S = 30, MEMORY_S = 600, WINDOW_S = 60, MIN_PACKETS = 10;
const changeable = d => d.kind === "RPA" || d.kind === "NRPA" || d.kind === "static";
const sepOf = (a, b) => {
  if (!a || !b) return Infinity;
  const sp = Math.hypot(a.sp, b.sp), sh = Math.hypot(a.sh, b.sh);
  if (!(sp > 0) || !(sh > 0)) return Infinity;
  return Math.hypot((a.mp - b.mp) / sp, (a.mh - b.mh) / sh);
};
// The mean of the packets heard between t0 and t1, from the census's running
// means: the reading before t0 taken out of the one at t1.
const windowed = (rs, t0, t1) => {
  const upTo = t => { let r = null; for (const x of rs) if (x.at <= t) r = x; return r; };
  const e = upTo(t1); if (!e) return null;
  const a = upTo(t0);
  const p = e.d.crystal_offset_ppm, h = e.d.modulation_index;
  // A window with fewer than MIN_PACKETS packets is no measurement: its sigma
  // would be wide enough to match anything.
  if (!a || a.n >= e.n) return e.n >= MIN_PACKETS ? { mp: p.value, sp: p.sigma, mh: h.value, sh: h.sigma } : null;
  const k = e.n - a.n, f = Math.sqrt(e.n / k);
  if (k < MIN_PACKETS) return null;
  return {
    mp: (p.value * e.n - a.d.crystal_offset_ppm.value * a.n) / k, sp: p.sigma * f,
    mh: (h.value * e.n - a.d.modulation_index.value * a.n) / k, sh: h.sigma * f,
  };
};
const heard = new Map(), want = new Map(), wantGroups = new Set();
for (const p of fx.polls) {
  const T = p.t;
  const fresh = (p.census.devices || []).filter(d => d.crystal_offset_ppm && d.modulation_index);
  const inAnswer = new Set(fresh.map(d => d.address));
  for (const d of fresh) {
    const h = heard.get(d.address);
    const at = T - d.last_seen_s;
    const rs = h ? h.rs : [];
    if (!rs.length || rs[rs.length - 1].n !== d.packets) rs.push({ at, n: d.packets, d });
    heard.set(d.address, { first: h ? h.first : T - d.first_seen_s, last: at, d, rs });
  }
  const live = a => inAnswer.has(a) && T - heard.get(a).last <= SILENT_S;
  // Parallel addresses: live changeable addresses of one company, heard
  // together for 10 s or more, every pair within 2 sigma over all the time
  // they shared (one device's drift moves both alike). Grown from the most
  // packets down, a member joining only if it is close to every one already in.
  const close = (x, y) => {
    const lo = Math.max(x.first, y.first), hi = Math.min(x.last, y.last);
    if (hi - lo < 10) return false;
    return sepOf(windowed(x.rs, lo, hi), windowed(y.rs, lo, hi)) <= 2;
  };
  const cand = [...heard].filter(([a, h]) => changeable(h.d) && live(a))
    .sort(([a, x], [b, y]) => (y.d.packets - x.d.packets) || (a < b ? -1 : 1));
  const used = new Set();
  for (const [a, x] of cand) {
    if (used.has(a)) continue;
    const g = [[a, x]];
    for (const [b, y] of cand)
      if (b !== a && !used.has(b) && (y.d.company || "") === (x.d.company || "") && g.every(([, m]) => close(m, y))) g.push([b, y]);
    if (g.length > 1) { for (const [m] of g) used.add(m); wantGroups.add(g.map(([m]) => m).sort().join(" + ")); }
  }
  for (const [xa, x] of heard) {
    if (!changeable(x.d) || !live(xa) || x.first < T - MEMORY_S) continue;
    const xw = windowed(x.rs, x.first, x.first + WINDOW_S);
    const c = [...heard].filter(([ga, g]) => ga !== xa && changeable(g.d) && !live(ga)
      && (g.d.company || "") === (x.d.company || "")
      && x.first - g.last >= -2 && x.first - g.last <= SWAP_S
      && sepOf(windowed(g.rs, g.last - WINDOW_S, g.last), xw) <= 2);
    for (const [ga] of c) if (!want.has(`${ga} -> ${xa}`)) want.set(`${ga} -> ${xa}`, c.length);
  }
}
const counts = new Map([...want].map(([k, n]) => [k.split(" -> ")[1], n]));
if (q) {
  const got = [...seen.keys()].sort(), wanted = [...want.keys()].sort();
  if (wanted.join() !== got.join())
    fail.push(`links ${JSON.stringify(got)}, the rule gives ${JSON.stringify(wanted)}`);
  for (const k of new Set(counts.values())) {
    const label = k === 1 ? "NEW ADDRESS, SAME QUARTZ?" : `${k} CRYSTALS MATCH`;
    if (!played.drawn.includes(label)) fail.push(`a link was not labelled ${label}`);
  }
  // The moment takes the right-hand panel: its question, and the two numbers
  // it rests on (how close in quartz, how soon after the old one fell quiet).
  if (counts.size) {
    if (!played.drawn.some(t => t === "SAME QUARTZ?" || /^\d+ CRYSTALS MATCH$/.test(t))) fail.push("the moment's heading was never shown");
    if (!played.drawn.some(t => /\d(\.\d)?σ<\/b> apart/.test(t))) fail.push("the moment never said how far apart in quartz");
    if (!played.drawn.some(t => /later|same moment|within 30 s/.test(t))) fail.push("the moment never said when the new address began");
  }
}

// 4a. The parallel groups are exactly the rule's, and are said on the plot.
if (q) {
  if (!q.groups) fail.push("window.__quartz.groups is not there");
  else {
    const got = [...seenGroups].sort(), wanted = [...wantGroups].sort();
    if (got.join() !== wanted.join()) fail.push(`groups ${JSON.stringify(got)}, the rule gives ${JSON.stringify(wanted)}`);
    if (wanted.length && !played.drawn.some(t => /^\d+ ADDRESSES, ONE QUARTZ\?$/.test(t))) fail.push("a group was never labelled N ADDRESSES, ONE QUARTZ?");
  }
}

// 4b. A device with no company is called by the vendor its address's own
// prefix names, never by the session number or the bytes.
const vendorOf = d => d.vendor || ((/^(.*?)(·\w+)?\s+(#\d+|#-|\.\.[0-9a-f]{2}:[0-9a-f]{2})$/i.exec(d.address) || [])[1]);
if (q && q.label) {
  for (const d of devs) {
    const v = vendorOf(d);
    if (d.company || !v || /^(RPA|NRPA|static|public)$/i.test(v) || /^[0-9a-f]{2}(-[0-9a-f]{2}){2}$/i.test(v)) continue;
    if (!q.label(d).toUpperCase().startsWith(v.toUpperCase().slice(0, 10))) fail.push(`${JSON.stringify(v)} labelled ${JSON.stringify(q.label(d))}`);
  }
} else if (q) fail.push("window.__quartz.label is not there");

// 4c. A moment holds the stage only until the viewer looks elsewhere:
// pointing at another device ends it, and the link keeps its small label
// beside the point.
if (counts.size && q && !q.moment) fail.push("window.__quartz.moment is not there");
if (counts.size && q && q.moment) {
  let started = -1, checked = false, ended = null, labelAfter = false, mark = 0;
  const probe = [];
  for (let at = 1000; at < span * 1000; at += 1000) probe.push({ at, fn: (Q, L, drawn) => {
    if (started < 0 && Q.moment()) {
      started = at;
      const other = Q.devices().find(d => d.address !== Q.moment().to && d._px != null);
      if (other) for (const f of L.mousemove || []) f({ clientX: other._px, clientY: other._py });
      mark = drawn.length;
    } else if (started > 0 && !checked && at >= started + 2000) {
      checked = true;
      ended = Q.moment();
      labelAfter = drawn.slice(mark).some(t => t === "NEW ADDRESS, SAME QUARTZ?" || /CRYSTALS MATCH$/.test(t));
    }
  } });
  await run({ search: `?fixture=${path.relative(base, path.resolve(fixturePath))}`, fetchImpl: serveFixture, seconds: span, during: probe });
  if (started < 0) fail.push("no moment began");
  else {
    if (ended) fail.push("pointing at another device did not end the moment");
    if (!labelAfter) fail.push("after the moment the link lost its label beside the point");
  }
}

// 5b. server.py up and monasd down: server.py answers 502 with an error body,
// which is monasd not answering too, not an empty room.
const proxyOnly = await run({ search: "", fetchImpl: async () => ({ ok: false, status: 502, json: async () => ({ error: "<urlopen error [Errno 111] Connection refused>" }) }), seconds: 6 });
if (!proxyOnly.drawn.includes("monasd is not answering")) fail.push("a 502 from server.py was not said as monasd not answering");

// 5c. ?fixture= plays files from the page's own folder only: anything with a
// scheme or a host is refused, never fetched.
for (const bad of ["data:application/json,{}", "//example.org/x.json", "https://example.org/x.json"]) {
  const r = await run({ search: `?fixture=${encodeURIComponent(bad)}`, fetchImpl: serveFixture, seconds: 3 });
  if (r.asked.some(u => u.includes(":") || u.startsWith("//"))) fail.push(`?fixture=${bad} was fetched`);
  if (!r.drawn.some(t => /own folder/.test(t))) fail.push(`?fixture=${bad} was not refused in words`);
}

// 5. A monasd that does not answer is said so, and the page keeps asking.
const dead = await run({ search: "", fetchImpl: async () => { throw new Error("refused"); }, seconds: 6 });
if (!dead.drawn.includes("monasd is not answering")) fail.push("an unanswering monasd was not said so");
if (dead.fetches < 5) fail.push(`the page stopped asking: ${dead.fetches} requests in 6 s`);

console.log(fail.length ? "FAIL\n" + [...new Set(fail)].join("\n") : `PASS (${want.size} link${want.size === 1 ? "" : "s"} over the playback)`);
process.exit(fail.length ? 1 : 0);
