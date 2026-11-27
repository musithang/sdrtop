// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

// The folder's own rules, checked on the files as text: nothing fetched
// from elsewhere, nothing kept past a session, nothing personal in the
// config, and the repository's conventions. Prints PASS or FAIL.
//
//   node test/static.mjs        (from research/quartz/)
import fs from "fs";
import path from "path";

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
for (const word of ["localStorage", "sessionStorage", "document.cookie", "indexedDB", "fonts.googleapis"])
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
  const text = fs.readFileSync(f, "utf8");
  if (text.includes(EM_DASH)) fail.push(`${rel} has an em dash`);
  if (text.includes(NOTES)) fail.push(`${rel} points into the gitignored notes`);
  if (/\.(py|mjs|html)$/.test(f) && !text.split("\n").slice(0, 3).some(l => l.includes("SPDX-License-Identifier: GPL-3.0-or-later")))
    fail.push(`${rel} has no SPDX header in its first three lines`);
}

console.log(fail.length ? "FAIL\n" + fail.join("\n") : "PASS");
process.exit(fail.length ? 1 : 0);
