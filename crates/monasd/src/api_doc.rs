// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The API reference, `user_docs/api.md`, generated from the source and held
//! to it.
//!
//! A reference typed by hand is the one that drifts (POLICY rule 7), so this
//! reads what the API is made of instead: the routes from the router's
//! `.route(` lines, what each serves from its handler's doc comment and
//! return type, every type it returns from its struct or enum and the doc
//! comments on its fields, the commands from `Command`, the events from
//! `Event`. The test renders it and fails if the file differs; run it with
//! `MONAS_WRITE_API=1` to write the file anew. A second test serialises a
//! snapshot and holds its keys to the fields documented, so a rename or a
//! flatten serde does cannot slip past the parser.
//!
//! The parser reads Rust as rustfmt writes it: a `pub struct` or `pub enum`
//! opening on one line, a field to a line, the closing brace in the first
//! column. That is the shape of every file it reads, and a test breaks if a
//! type it needs goes missing.

use std::collections::{BTreeSet, VecDeque};

/// One field of a struct or a struct variant.
#[derive(Clone, Debug)]
struct Field {
    name: String,
    ty: String,
    doc: String,
    flatten: bool,
}

#[derive(Clone, Debug)]
struct Variant {
    /// As serde writes it.
    name: String,
    doc: String,
    fields: Vec<Field>,
}

#[derive(Clone, Debug)]
enum Shape {
    Struct(Vec<Field>),
    Enum {
        tag: Option<String>,
        variants: Vec<Variant>,
    },
}

#[derive(Clone, Debug)]
struct Item {
    module: String,
    name: String,
    doc: String,
    shape: Shape,
}

/// `CamelCase` as serde's `snake_case` writes it.
fn snake(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// Doc comment lines as one line of Markdown: rustdoc's links turned into
/// plain code.
fn doc_text(lines: &[String]) -> String {
    let joined = lines
        .iter()
        .map(|l| l.trim())
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string();
    let mut out = String::new();
    let mut rest = joined.as_str();
    while let Some(at) = rest.find("[`") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let Some(end) = after.find("`]") else {
            out.push_str(&rest[at..]);
            rest = "";
            break;
        };
        out.push_str(&after[..end + 1]);
        rest = &after[end + 2..];
        // `[`x`](path)` and `[`x`][path]`: the target goes too.
        for (open, close) in [('(', ')'), ('[', ']')] {
            if rest.starts_with(open) {
                if let Some(e) = rest.find(close) {
                    rest = &rest[e + 1..];
                }
            }
        }
    }
    out.push_str(rest);
    out
}

/// A `pub name: Type,` line, or a `name: Type` inside a one-line variant.
fn field_of(text: &str, doc: &[String], flatten: bool) -> Option<Field> {
    let text = text.trim().trim_end_matches(',');
    let text = text.strip_prefix("pub ").unwrap_or(text);
    let (name, ty) = text.split_once(':')?;
    let name = name.trim();
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    Some(Field {
        name: name.to_string(),
        ty: ty.trim().to_string(),
        doc: doc_text(doc),
        flatten,
    })
}

/// Fields written on one line: `a: u64, b: String`.
fn inline_fields(text: &str) -> Vec<Field> {
    let mut out = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (i, c) in text.char_indices() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                out.extend(field_of(&text[start..i], &[], false));
                start = i + 1;
            }
            _ => {}
        }
    }
    out.extend(field_of(&text[start..], &[], false));
    out
}

/// Every `pub struct` and `pub enum` in `src`, outside its tests.
fn parse(module: &str, src: &str) -> Vec<Item> {
    let src = src.split("\n#[cfg(test)]\nmod tests").next().unwrap_or(src);
    let lines: Vec<&str> = src.lines().collect();
    let mut items = Vec::new();
    let mut doc: Vec<String> = Vec::new();
    let mut tag: Option<String> = None;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some(d) = line.strip_prefix("///") {
            doc.push(d.strip_prefix(' ').unwrap_or(d).to_string());
            i += 1;
            continue;
        }
        if line.starts_with("#[") {
            if let Some(t) = line.split("tag = \"").nth(1) {
                tag = t.split('"').next().map(str::to_string);
            }
            i += 1;
            continue;
        }
        let header = |kw: &str| {
            line.strip_prefix(kw)
                .filter(|_| line.ends_with('{'))
                .map(|rest| {
                    rest.trim_end_matches('{')
                        .split(['<', ' '])
                        .next()
                        .unwrap_or("")
                        .to_string()
                })
        };
        if let Some(name) = header("pub struct ") {
            let mut fields = Vec::new();
            let mut fdoc: Vec<String> = Vec::new();
            let mut flatten = false;
            i += 1;
            while i < lines.len() && lines[i] != "}" {
                let l = lines[i].trim();
                if let Some(d) = l.strip_prefix("///") {
                    fdoc.push(d.strip_prefix(' ').unwrap_or(d).to_string());
                } else if l.starts_with("#[") {
                    flatten |= l.contains("flatten");
                } else if l.starts_with("pub ") {
                    fields.extend(field_of(l, &fdoc, flatten));
                    fdoc.clear();
                    flatten = false;
                } else {
                    fdoc.clear();
                }
                i += 1;
            }
            items.push(Item {
                module: module.to_string(),
                name,
                doc: doc_text(&doc),
                shape: Shape::Struct(fields),
            });
        } else if let Some(name) = header("pub enum ") {
            let mut variants = Vec::new();
            let mut vdoc: Vec<String> = Vec::new();
            i += 1;
            while i < lines.len() && lines[i] != "}" {
                let raw = lines[i];
                let l = raw.trim();
                if let Some(d) = l.strip_prefix("///") {
                    vdoc.push(d.strip_prefix(' ').unwrap_or(d).to_string());
                    i += 1;
                    continue;
                }
                if l.starts_with("#[")
                    || l.is_empty()
                    || !raw.starts_with("    ")
                    || raw.starts_with("     ")
                {
                    i += 1;
                    continue;
                }
                let vname: String = l.chars().take_while(|c| c.is_alphanumeric()).collect();
                let rest = l[vname.len()..].trim();
                let fields = if let Some(body) = rest.strip_prefix('{') {
                    if let Some(inner) = body.trim().strip_suffix("},") {
                        inline_fields(inner)
                    } else {
                        // Its fields, one to a line, to the closing brace.
                        let mut fs = Vec::new();
                        let mut fdoc: Vec<String> = Vec::new();
                        i += 1;
                        while i < lines.len() && !lines[i].trim().starts_with('}') {
                            let fl = lines[i].trim();
                            if let Some(d) = fl.strip_prefix("///") {
                                fdoc.push(d.strip_prefix(' ').unwrap_or(d).to_string());
                            } else {
                                fs.extend(field_of(fl, &fdoc, false));
                                fdoc.clear();
                            }
                            i += 1;
                        }
                        fs
                    }
                } else {
                    Vec::new()
                };
                if !vname.is_empty() {
                    variants.push(Variant {
                        name: snake(&vname),
                        doc: doc_text(&vdoc),
                        fields,
                    });
                }
                vdoc.clear();
                i += 1;
            }
            items.push(Item {
                module: module.to_string(),
                name,
                doc: doc_text(&doc),
                shape: Shape::Enum {
                    tag: tag.take(),
                    variants,
                },
            });
        }
        doc.clear();
        tag = None;
        i += 1;
    }
    items
}

/// The parsed types, and how to find one from a reference to it.
struct Types {
    items: Vec<Item>,
    /// `(module, name)` to the module a `use super::<module>::<name>` brings
    /// it from.
    imports: Vec<(String, String, String)>,
}

/// The names a file takes from its sibling modules: `use super::net::Sigma;`
/// and `use super::{net::Sigma, ...}`'s plain form.
fn imports(module: &str, src: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for line in src.lines() {
        let Some(rest) = line.trim().strip_prefix("use super::") else {
            continue;
        };
        let rest = rest.trim_end_matches(';');
        let Some((from, names)) = rest.split_once("::") else {
            continue;
        };
        for name in names.trim_matches(['{', '}']).split(',') {
            let name = name.trim();
            if !name.is_empty() {
                out.push((module.to_string(), name.to_string(), from.to_string()));
            }
        }
    }
    out
}

impl Types {
    fn load() -> Types {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
        let view = format!("{root}/monas/src/engine/view");
        let mut files: Vec<(String, String)> = std::fs::read_dir(&view)
            .expect("the view's sources")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "rs"))
            .map(|p| {
                let stem = p.file_stem().unwrap().to_string_lossy().into_owned();
                let module = if stem == "mod" {
                    "view".to_string()
                } else {
                    stem
                };
                (module, p.to_string_lossy().into_owned())
            })
            .collect();
        files.sort();
        files.push((
            "testimony".into(),
            format!("{root}/monas/src/engine/testimony.rs"),
        ));
        files.push((
            "command".into(),
            format!("{root}/monas/src/engine/command.rs"),
        ));
        files.push((
            "events".into(),
            format!("{root}/monas/src/engine/events.rs"),
        ));
        files.push(("log".into(), format!("{root}/monas/src/state/log.rs")));
        // A carrier, as the doppler command and `[doppler]` take it.
        files.push((
            "doppler_settings".into(),
            format!("{root}/monas/src/signal/doppler/settings.rs"),
        ));
        files.push(("server".into(), format!("{root}/monasd/src/server.rs")));
        let mut items = Vec::new();
        let mut uses = Vec::new();
        for (module, path) in files {
            let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
            items.extend(parse(&module, &src));
            uses.extend(imports(&module, &src));
        }
        Types {
            items,
            imports: uses,
        }
    }

    /// The type a reference names, from `context`'s module: by its module
    /// where the path names one, else the same module's, else the only one
    /// by that name.
    fn find(&self, path: &str, context: &str) -> Option<&Item> {
        let segs: Vec<&str> = path.split("::").collect();
        let name = *segs.last()?;
        if segs.len() >= 2 {
            let module = segs[segs.len() - 2];
            if let Some(it) = self
                .items
                .iter()
                .find(|i| i.module == module && i.name == name)
            {
                return Some(it);
            }
        }
        if let Some(it) = self
            .items
            .iter()
            .find(|i| i.module == context && i.name == name)
        {
            return Some(it);
        }
        if let Some((_, _, from)) = self
            .imports
            .iter()
            .find(|(m, n, _)| m == context && n == name)
        {
            if let Some(it) = self
                .items
                .iter()
                .find(|i| &i.module == from && i.name == name)
            {
                return Some(it);
            }
        }
        let mut all = self.items.iter().filter(|i| i.name == name);
        let first = all.next()?;
        all.next().is_none().then_some(first)
    }

    /// Whether another type has this one's name, so its heading needs the
    /// module.
    fn ambiguous(&self, item: &Item) -> bool {
        self.items.iter().filter(|i| i.name == item.name).count() > 1
    }

    fn title(&self, item: &Item) -> String {
        if self.ambiguous(item) {
            format!("{}::{}", item.module, item.name)
        } else {
            item.name.clone()
        }
    }
}

/// GitHub's anchor for a heading.
fn slug(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-' || *c == '_')
        .map(|c| if c == ' ' { '-' } else { c })
        .collect()
}

/// A type expression's head and its generic arguments, split at the top
/// level.
fn split_generic(ty: &str) -> (String, Vec<String>) {
    let Some(open) = ty.find('<') else {
        return (ty.to_string(), Vec::new());
    };
    let head = ty[..open].to_string();
    let inner = &ty[open + 1..ty.rfind('>').unwrap_or(ty.len())];
    let mut args = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (i, c) in inner.char_indices() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                args.push(inner[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    args.push(inner[start..].trim().to_string());
    (head, args)
}

/// A type expression in words, the documented types linked; every type it
/// reaches pushed onto `queue`.
fn render(
    ty: &str,
    context: &str,
    types: &Types,
    queue: &mut VecDeque<(String, String)>,
) -> String {
    let ty = ty
        .trim()
        .trim_start_matches("&'static ")
        .trim_start_matches('&');
    if let Some(inner) = ty.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
        let (elem, n) = inner.rsplit_once(';').unwrap_or((inner, "?"));
        return format!("{} of {}", n.trim(), render(elem, context, types, queue));
    }
    let (head, args) = split_generic(ty);
    let last = head.rsplit("::").next().unwrap_or(&head);
    let arg = |i: usize, q: &mut VecDeque<(String, String)>| {
        args.get(i)
            .map(|a| render(a, context, types, q))
            .unwrap_or_default()
    };
    match last {
        "Option" => format!("{} or null", arg(0, queue)),
        "Vec" | "VecDeque" => format!("list of {}", arg(0, queue)),
        "Reading" => format!("{} or absent", arg(0, queue)),
        "Group" => format!("group of {}", arg(0, queue)),
        "Arc" | "Box" => arg(0, queue),
        "u8" | "u16" | "u32" | "u64" | "usize" | "i8" | "i16" | "i32" | "i64" => "integer".into(),
        "f32" | "f64" => "number".into(),
        "bool" => "boolean".into(),
        "String" | "str" | "char" | "PathBuf" => "string".into(),
        _ => match types.find(&head, context) {
            // Described in a section of their own, not under Types.
            Some(it) if section_of(it).is_some() => {
                format!("[`{}`](#{})", it.name, section_of(it).unwrap_or_default())
            }
            Some(it) => {
                queue.push_back((it.module.clone(), it.name.clone()));
                let title = types.title(it);
                format!("[`{title}`](#{})", slug(&title))
            }
            None => format!("`{last}`"),
        },
    }
}

/// The section that describes `item`, for the types with one of their own.
fn section_of(item: &Item) -> Option<&'static str> {
    match (item.module.as_str(), item.name.as_str()) {
        ("command", "Command" | "Reply") => Some("commands"),
        ("events", "Event" | "Delivered") => Some("events"),
        ("testimony", "Testimony") => Some("conventions"),
        _ => None,
    }
}

/// One route: its method, path, what it serves, and what it returns.
struct Route {
    method: String,
    path: String,
    doc: String,
    returns: String,
}

/// The routes, from the router and their handlers.
fn routes(server: &str) -> Vec<Route> {
    let lines: Vec<&str> = server.lines().collect();
    let mut out = Vec::new();
    for l in &lines {
        let Some(rest) = l.trim().strip_prefix(".route(\"") else {
            continue;
        };
        let Some((path, rest)) = rest.split_once("\", ") else {
            continue;
        };
        let Some((method, rest)) = rest.split_once('(') else {
            continue;
        };
        let handler = rest.split(')').next().unwrap_or("").to_string();
        let at = lines
            .iter()
            .position(|x| x.starts_with(&format!("async fn {handler}(")))
            .unwrap_or_else(|| panic!("no handler {handler}"));
        let mut doc = Vec::new();
        let mut k = at;
        while k > 0 && lines[k - 1].starts_with("///") {
            k -= 1;
            doc.insert(0, lines[k].trim_start_matches("///").trim().to_string());
        }
        let mut signature = String::new();
        for x in &lines[at..] {
            signature.push_str(x.trim());
            signature.push(' ');
            if x.trim_end().ends_with('{') {
                break;
            }
        }
        let returns = signature
            .split("-> ")
            .nth(1)
            .map(|r| r.trim().trim_end_matches('{').trim().to_string())
            .unwrap_or_default();
        out.push(Route {
            method: method.to_uppercase(),
            path: path.to_string(),
            doc: doc_text(&doc),
            returns,
        });
    }
    out
}

const INTRO: &str = "\
# The monasd API

<!-- Generated from the source by crates/monasd/src/api_doc.rs. Do not edit:
     MONAS_WRITE_API=1 cargo test -p monasd api_reference writes it anew. -->

← [monasd](monasd.md)

---

This page is written by a test, from the code that serves the API: the
routes from the router, every field from the type it is serialised from,
each description from the comment above it in the source. If the two ever
disagree, the test fails before a release does. So everything here is true
of the version it came with, and a little dry, because I didn't write it.
[monasd](monasd.md) is the page with the context, the token and examples.

Everything is under `http://ADDR/v1`, JSON in and out. Off loopback every
request carries `Authorization: Bearer TOKEN`.

**Contents:** [Conventions](#conventions) · [Routes](#routes) ·
[Commands](#commands) · [Events](#events) · [Types](#types)

## Conventions

**A reading** is `{\"value\": ...}` or `{\"absent\": \"why\"}`, never a bare
number: a figure the screen would dash arrives as the reason it is missing,
so no client can read it as zero. Written below as *X or absent*.

**A group** is a set of readings with its testimony beside them: the
object has a `testimony` key, and the group's own fields at the same level.
Written below as *group of X*.

**`?since=N`** on a list leaves out the entries numbered `N` and below, so a
client that polls is sent each entry once.

**`null`** is a field that has nothing to say for this entry, for the reason
its description gives: a message with no locator, a stage with no measured
dB here, a header that could not be read.
";

/// The Markdown of the reference.
fn reference() -> String {
    let types = Types::load();
    let server = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/server.rs"))
        .expect("server.rs");
    let mut queue: VecDeque<(String, String)> = VecDeque::new();
    let mut out = String::from(INTRO);

    let testimony = types
        .find("testimony::Testimony", "testimony")
        .expect("the Testimony type");
    out.push_str("\n**The testimony** of a group:\n\n");
    out.push_str(&fields_table(testimony, &types, &mut VecDeque::new()));

    out.push_str("\n## Routes\n\n| Route | What it serves | Returns |\n|---|---|---|\n");
    for r in routes(&server) {
        let returns = match r.path.as_str() {
            "/v1/command" | "/v1/recording" | "/v1/recording/stop" | "/v1/export" => {
                "[a reply](#commands)".to_string()
            }
            "/v1/recordings/{name}" => {
                "the recording's `.sigmf-meta`, as JSON; 404 for a name the folder does not hold"
                    .to_string()
            }
            "/v1/events" => "[a WebSocket of events](#events)".to_string(),
            "/v1/config" => "the engine's blocks of the config, as JSON".to_string(),
            "/v1/provenance" => {
                "an object: `monas` (the version) and each header field by name, as text"
                    .to_string()
            }
            _ => {
                let ty = r
                    .returns
                    .strip_prefix("Json<")
                    .and_then(|t| t.strip_suffix('>'));
                match ty {
                    Some(t) => render(t, "server", &types, &mut queue),
                    None => r.returns.clone(),
                }
            }
        };
        out.push_str(&format!(
            "| `{} {}` | {} | {} |\n",
            r.method, r.path, r.doc, returns
        ));
    }

    let command = types.find("command::Command", "command").expect("Command");
    let reply = types.find("command::Reply", "command").expect("Reply");
    out.push_str("\n## Commands\n\n");
    out.push_str(
        "`POST /v1/command` takes one command as JSON, named by its `command` field. \
         It is answered `200` with `{\"reply\": \"applied\", ...}` when it was done, \
         or `422` with `{\"reply\": \"refused\", \"reason\": ...}` when it was not, and \
         then nothing was changed.\n\n",
    );
    out.push_str(&enum_section(command, &types, &mut queue));
    out.push_str("\nThe reply:\n\n");
    out.push_str(&enum_section(reply, &types, &mut queue));

    let delivered = types
        .find("events::Delivered", "events")
        .expect("Delivered");
    let event = types.find("events::Event", "events").expect("Event");
    out.push_str("\n## Events\n\n");
    out.push_str(
        "`GET /v1/events` opens a WebSocket that sends one JSON message per event, \
         each as it is delivered to this client:\n\n",
    );
    out.push_str(&fields_table(delivered, &types, &mut queue));
    out.push_str("\nThe event, named by its `event` field:\n\n");
    out.push_str(&enum_section(event, &types, &mut queue));

    out.push_str("\n## Types\n");
    let mut done: BTreeSet<(String, String)> = BTreeSet::new();
    // The conventions and the sections above describe these already.
    for skip in [
        ("view", "Reading"),
        ("view", "Group"),
        ("testimony", "Testimony"),
        ("command", "Command"),
        ("command", "Reply"),
        ("events", "Event"),
        ("events", "Delivered"),
    ] {
        done.insert((skip.0.into(), skip.1.into()));
    }
    while let Some(key) = queue.pop_front() {
        if !done.insert(key.clone()) {
            continue;
        }
        let item = types
            .items
            .iter()
            .find(|i| i.module == key.0 && i.name == key.1)
            .expect("a queued type");
        out.push_str(&format!("\n### {}\n\n", types.title(item)));
        if !item.doc.is_empty() {
            out.push_str(&format!("{}\n\n", item.doc));
        }
        match &item.shape {
            Shape::Struct(_) => out.push_str(&fields_table(item, &types, &mut queue)),
            Shape::Enum { .. } => out.push_str(&enum_section(item, &types, &mut queue)),
        }
    }
    out.push_str("\n---\n\n← [monasd](monasd.md)\n");
    out
}

fn fields_table(item: &Item, types: &Types, queue: &mut VecDeque<(String, String)>) -> String {
    let Shape::Struct(fields) = &item.shape else {
        return String::new();
    };
    let mut out = String::from("| Field | Type | |\n|---|---|---|\n");
    for f in fields {
        let ty = render(&f.ty, &item.module, types, queue);
        if f.flatten {
            out.push_str(&format!(
                "| *(its fields, at this level)* | {ty} | {} |\n",
                f.doc
            ));
        } else {
            out.push_str(&format!("| `{}` | {ty} | {} |\n", f.name, f.doc));
        }
    }
    out
}

fn enum_section(item: &Item, types: &Types, queue: &mut VecDeque<(String, String)>) -> String {
    let Shape::Enum { tag, variants } = &item.shape else {
        return String::new();
    };
    let mut out = String::new();
    match tag {
        Some(tag) => {
            out.push_str("| Name | Fields | |\n|---|---|---|\n");
            for v in variants {
                let fields = v
                    .fields
                    .iter()
                    .map(|f| {
                        format!(
                            "`{}`: {}",
                            f.name,
                            render(&f.ty, &item.module, types, queue)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("<br>");
                out.push_str(&format!(
                    "| `\"{tag}\": \"{}\"` | {fields} | {} |\n",
                    v.name, v.doc
                ));
            }
        }
        None => {
            out.push_str("One of: ");
            out.push_str(
                &variants
                    .iter()
                    .map(|v| format!("`\"{}\"`", v.name))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            out.push_str(".\n");
        }
    }
    out
}

/// The keys a value of `ty` serialises with, where the parsed types can say:
/// a struct's fields, a flattened field's own, a reading's `value` or
/// `absent`.
fn keys_of(ty: &str, context: &str, types: &Types) -> Option<BTreeSet<String>> {
    let (head, args) = split_generic(ty.trim());
    let last = head.rsplit("::").next().unwrap_or(&head);
    if last == "Reading" {
        return None;
    }
    if last == "Group" {
        let mut keys: BTreeSet<String> = ["testimony".to_string()].into();
        let inner = args.first()?;
        let (ih, _) = split_generic(inner);
        if ih.rsplit("::").next() == Some("Reading") {
            return None;
        }
        keys.extend(keys_of(inner, context, types)?);
        return Some(keys);
    }
    let item = types.find(&head, context)?;
    let Shape::Struct(fields) = &item.shape else {
        return None;
    };
    let mut keys = BTreeSet::new();
    for f in fields {
        if f.flatten {
            keys.extend(keys_of(&f.ty, &item.module, types)?);
        } else {
            keys.insert(f.name.clone());
        }
    }
    Some(keys)
}

#[test]
fn api_reference_is_generated_from_the_source_and_current() {
    let text = reference();
    assert!(!text.contains('\u{2014}'), "an em dash in the reference");
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../user_docs/api.md");
    if std::env::var_os("MONAS_WRITE_API").is_some() {
        std::fs::write(path, &text).expect("write user_docs/api.md");
    }
    let on_disk = std::fs::read_to_string(path).unwrap_or_default();
    if on_disk != text {
        let fresh = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/api.md");
        let _ = std::fs::write(fresh, &text);
        panic!(
            "user_docs/api.md is not what the source says (the fresh one is in target/api.md); \
             MONAS_WRITE_API=1 cargo test -p monasd api_reference writes it"
        );
    }
}

/// Every route has a description, and every type a route returns was found
/// and documented.
#[test]
fn api_reference_every_route_says_what_it_serves() {
    let server =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/server.rs")).unwrap();
    let rs = routes(&server);
    assert!(rs.len() >= 19, "the routes were not found: {}", rs.len());
    for r in &rs {
        assert!(
            !r.doc.is_empty(),
            "{} {} has no description",
            r.method,
            r.path
        );
    }
    let text = reference();
    for r in &rs {
        assert!(
            text.contains(&format!("`{} {}`", r.method, r.path)),
            "{}",
            r.path
        );
    }
    assert!(
        !text.contains("](#)"),
        "a link with no target in the reference"
    );
    // Every type a field names is found: a bare `Name` in a type column is
    // one the parser lost.
    for line in text
        .lines()
        .filter(|l| l.starts_with("| `") && !l.starts_with("| `GET ") && !l.starts_with("| `POST "))
    {
        let ty = line.split(" | ").nth(1).unwrap_or("");
        let bare = ty.split('`').skip(1).step_by(2).any(|t| {
            t.chars().next().is_some_and(char::is_uppercase) && !ty.contains(&format!("[`{t}`]"))
        });
        assert!(!bare, "an unresolved type in: {line}");
    }
    // Every link points at a heading that exists.
    let headings: BTreeSet<String> = text
        .lines()
        .filter_map(|l| l.strip_prefix("### "))
        .map(slug)
        .collect();
    for part in text.split("](#").skip(1) {
        let target = part.split(')').next().unwrap();
        let sections = ["conventions", "routes", "commands", "events", "types"];
        assert!(
            headings.contains(target) || sections.contains(&target),
            "a link to #{target}, which is no heading"
        );
    }
}

/// What a snapshot serialises to has exactly the keys the reference
/// documents, group by group: the parser read what serde writes.
#[test]
fn api_reference_documents_the_keys_serde_writes() {
    let types = Types::load();
    let m = monas::state::SdrMetrics::fixture().streaming();
    let json = serde_json::to_value(monas::engine::view::snapshot(
        &m,
        monas::engine::view::Clock::now(),
    ))
    .unwrap();
    let snapshot = types.find("view::Snapshot", "view").expect("Snapshot");
    let Shape::Struct(fields) = &snapshot.shape else {
        panic!("Snapshot is a struct");
    };
    let top: BTreeSet<String> = json.as_object().unwrap().keys().cloned().collect();
    let documented: BTreeSet<String> = fields.iter().map(|f| f.name.clone()).collect();
    assert_eq!(top, documented, "the snapshot's keys");
    let mut checked = 0;
    for f in fields {
        if let Some(want) = keys_of(&f.ty, "view", &types) {
            let got: BTreeSet<String> = json[&f.name]
                .as_object()
                .map(|o| o.keys().cloned().collect())
                .unwrap_or_default();
            assert_eq!(got, want, "the keys of {}", f.name);
            checked += 1;
        }
    }
    assert!(checked >= 8, "only {checked} groups were checked");
}
