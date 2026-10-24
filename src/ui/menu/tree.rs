// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The menu as a tree: tiles at the top, a cascade of columns under each.
//!
//! **Above the sections, never instead of them.** A preset's `section` is still
//! its group, the group is still what a number key counts in, and
//! [`super::model`] still builds the groups with its slot rules and warnings.
//! This module only says where each group hangs: `Lab › Bluetooth › 2.4 GHz ›
//! Low Energy` is the `le` section with three headings above it. So a user's
//! preset filed under `le` lands in the same place it always did, and nothing
//! that resolves a number key needs to know the tree exists.
//!
//! **Sorted by where it listens.** The tree's levels are what a radio is tuned
//! by: what you are looking at, then the band, then the question. Bluetooth's
//! band is a level of its own although it has one child, because it says
//! where Bluetooth lives; [`Tree::descend`] passes through it so it costs no
//! key press.
//!
//! **A refused row is listed, not hidden.** Views the radio cannot run (the
//! NET gate's refusals) never reach the layout engine, but they are hung here
//! as [`Refusal`]s and drawn quiet with the gate's sentence, so the user finds
//! the reason where they look for the thing. A branch with nothing it can
//! open and at least one refusal under it carries that refusal too, which is
//! what greys Bluetooth itself on a radio that stops at 1.8 GHz.
//!
//! **Later rows are part of the table.** A group a design has named but no
//! preset fills yet is listed as later, quiet and unopenable, and becomes a
//! real row the moment a preset is filed under it. A group the table does not
//! mark later and nothing fills is left out, which is what keeps a user's
//! removed built-in from leaving an empty row behind.
//!
//! Pure, like [`super::model`]: no ratatui, no theme, no device. Colours are a
//! [`Tone`] the renderer maps onto the theme.

use super::model::{Entry, Menu, Section, OTHER, WIFI};
use crate::signal::net::gate::{HIGHEST_CENTRE_HZ, LOWEST_CENTRE_HZ};

/// The two tiles that open a tree. The other tiles open a pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tile {
    Radio,
    Lab,
}

/// Which theme colour a row is drawn in. Named for what the row is, so the
/// mapping onto a palette is the renderer's one decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Spectrum,
    Sweep,
    Receiver,
    Ble,
    Classic,
    Coded,
    Wifi,
    Quiet,
    Radio,
}

/// A stretch of spectrum a row listens in, in hertz.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Band {
    pub lo_hz: u64,
    pub hi_hz: u64,
}

impl Band {
    /// Whether a radio tuning `min_hz..=max_hz` covers all of it. All, not
    /// some: a view that silently covered part of its band would be worse than
    /// one refused.
    pub fn within(&self, min_hz: u64, max_hz: u64) -> bool {
        min_hz <= self.lo_hz && self.hi_hz <= max_hz
    }
}

/// Bluetooth's and Wi-Fi's band, as far as anything here tunes in it: the
/// NET gate's own limits, so the menu and the gate cannot disagree about
/// what reaching it means.
const BAND_24: Band = Band {
    lo_hz: LOWEST_CENTRE_HZ,
    hi_hz: HIGHEST_CENTRE_HZ,
};

/// Wi-Fi's 5 GHz channels, from channel 36's lower edge to channel 177's
/// upper one. Only the menu's mark: nothing here tunes there yet, and the
/// gate will say what reaching it means when something does.
const BAND_5: Band = Band {
    lo_hz: 5_170_000_000,
    hi_hz: 5_895_000_000,
};

/// One row of a column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub title: String,
    pub tone: Tone,
    /// A word or two drawn quietly at the row's right: a band's span, "later".
    pub hint: Option<&'static str>,
    pub band: Option<Band>,
    /// What the card says about a row that is not a view.
    pub about: Option<&'static str>,
    /// Named by a design, filled by nothing yet: drawn quiet, opens nothing.
    pub later: bool,
    /// Why this radio cannot open it, in the gate's words.
    pub refused: Option<String>,
    pub kind: Kind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A label over the rows below it in a column. Never a cursor stop.
    Heading,
    /// A row that opens another column. `section` is set on the rows whose
    /// children are one menu section's views, which is where a number key
    /// counts.
    Branch {
        section: Option<String>,
        children: Vec<Node>,
    },
    /// A layout.
    View(Entry),
}

impl Node {
    pub fn children(&self) -> &[Node] {
        match &self.kind {
            Kind::Branch { children, .. } => children,
            _ => &[],
        }
    }

    /// Whether `Enter` on this row opens a layout.
    pub fn opens(&self) -> bool {
        self.view().is_some() && self.refused.is_none()
    }

    /// Whether the cursor may stand here.
    pub fn is_stop(&self) -> bool {
        !matches!(self.kind, Kind::Heading)
    }

    pub fn view(&self) -> Option<&Entry> {
        match &self.kind {
            Kind::View(e) => Some(e),
            _ => None,
        }
    }

    /// The views under this row, at any depth.
    pub fn view_count(&self) -> usize {
        match &self.kind {
            Kind::View(_) => 1,
            Kind::Branch { children, .. } => children.iter().map(Node::view_count).sum(),
            Kind::Heading => 0,
        }
    }
}

/// A menu section whose views this radio cannot run, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub section: Section,
    pub why: String,
}

/// Both trees, built once from the menu's sections.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tree {
    pub radio: Vec<Node>,
    pub lab: Vec<Node>,
}

/// A place in a tree: the tile, then a row index in each column, headings
/// counted, so an index is the row as drawn.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Path {
    pub rows: Vec<usize>,
}

impl From<crate::state::MenuPath> for Path {
    fn from(p: crate::state::MenuPath) -> Self {
        Path { rows: p.rows() }
    }
}

impl From<&Path> for crate::state::MenuPath {
    fn from(p: &Path) -> Self {
        crate::state::MenuPath::from_rows(&p.rows)
    }
}

impl Tree {
    pub fn root(&self, tile: Tile) -> &[Node] {
        match tile {
            Tile::Radio => &self.radio,
            Tile::Lab => &self.lab,
        }
    }

    /// The row `path` ends on, if every index is in range.
    pub fn node(&self, tile: Tile, path: &Path) -> Option<&Node> {
        let mut rows = self.root(tile);
        let mut node = None;
        for &i in &path.rows {
            let n = rows.get(i)?;
            rows = n.children();
            node = Some(n);
        }
        node
    }

    /// The rows `path` passes through, first column first: what the crumb
    /// names. Stops at the first index out of range.
    pub fn trail(&self, tile: Tile, path: &Path) -> Vec<&Node> {
        let mut rows = self.root(tile);
        let mut out = Vec::new();
        for &i in &path.rows {
            let Some(n) = rows.get(i) else { break };
            out.push(n);
            rows = n.children();
        }
        out
    }

    /// The rows of each column `path` passes through, the last being the
    /// column the cursor is in.
    pub fn columns(&self, tile: Tile, path: &Path) -> Vec<&[Node]> {
        let mut out = vec![self.root(tile)];
        for &i in path.rows.iter().take(path.rows.len().saturating_sub(1)) {
            match out.last().and_then(|rows| rows.get(i)) {
                Some(n) => out.push(n.children()),
                None => break,
            }
        }
        out
    }

    /// Where a preset sits, if the Lab tree lists it. Every preset the menu
    /// lists is in the Lab tree, since the Radio tree has no views yet.
    pub fn locate(&self, preset: &str) -> Option<(Tile, Path)> {
        for tile in [Tile::Lab, Tile::Radio] {
            let mut rows = Vec::new();
            if find(self.root(tile), preset, &mut rows) {
                return Some((tile, Path { rows }));
            }
        }
        None
    }

    /// A path certainly in range and on a cursor stop, or `None` for a tree
    /// with nothing in it.
    ///
    /// The cursor is cloned into the frame snapshot every frame, so it is
    /// pulled into range here rather than trusted: an index past the end
    /// would panic mid-draw and take the terminal with it. An index on a heading moves to the next
    /// stop in its column, or the one before it when the heading is last.
    pub fn clamp(&self, tile: Tile, path: &Path) -> Option<Path> {
        let mut rows = self.root(tile);
        let mut out = Vec::new();
        let wanted = if path.rows.is_empty() {
            &[0][..]
        } else {
            &path.rows[..]
        };
        for &i in wanted {
            let Some(at) = nearest_stop(rows, i) else {
                break;
            };
            out.push(at);
            rows = rows[at].children();
        }
        (!out.is_empty()).then_some(Path { rows: out })
    }

    /// One column right: onto the first stop of what the row at `path` holds,
    /// and through any row that holds only one branch, so a level that is
    /// there to say where something lives costs no key press. `None` on a
    /// view, a later row or an empty branch.
    pub fn descend(&self, tile: Tile, path: &Path) -> Option<Path> {
        let node = self.node(tile, path)?;
        if node.later {
            return None;
        }
        let mut next = path.clone();
        let mut at = node;
        loop {
            let first = nearest_stop(at.children(), 0)?;
            next.rows.push(first);
            if !passes_through(at) {
                return Some(next);
            }
            at = &at.children()[first];
        }
    }

    /// One column left, out through the levels [`Self::descend`] passed
    /// through. `None` from the first column.
    pub fn ascend(&self, tile: Tile, path: &Path) -> Option<Path> {
        let mut up = path.clone();
        up.rows.pop();
        while up.rows.len() > 1 {
            let parent = Path {
                rows: up.rows[..up.rows.len() - 1].to_vec(),
            };
            if !self.node(tile, &parent).is_some_and(passes_through) {
                break;
            }
            up.rows.pop();
        }
        (!up.rows.is_empty()).then_some(up)
    }

    /// The next stop up or down the column `path` ends in, staying put at
    /// either end rather than wrapping: a list that wraps under the cursor
    /// is a list you lose your place in.
    pub fn step(&self, tile: Tile, path: &Path, down: bool) -> Path {
        let mut next = path.clone();
        let Some(&at) = path.rows.last() else {
            return next;
        };
        let columns = self.columns(tile, path);
        let Some(rows) = columns.last() else {
            return next;
        };
        let found = if down {
            (at + 1..rows.len()).find(|&i| rows[i].is_stop())
        } else {
            (0..at).rev().find(|&i| rows[i].is_stop())
        };
        if let (Some(i), Some(last)) = (found, next.rows.last_mut()) {
            *last = i;
        }
        next
    }

    /// The menu section the row at `path` counts number keys in: its own
    /// when it is a group, its group's when it is a view.
    pub fn section_at(&self, tile: Tile, path: &Path) -> Option<&str> {
        let node = self.node(tile, path)?;
        if let Kind::Branch {
            section: Some(id), ..
        } = &node.kind
        {
            return Some(id);
        }
        let mut up = path.clone();
        up.rows.pop();
        match &self.node(tile, &up)?.kind {
            Kind::Branch {
                section: Some(id), ..
            } => Some(id),
            _ => None,
        }
    }
}

/// A row that only says where its one branch lives.
fn passes_through(node: &Node) -> bool {
    let stops: Vec<&Node> = node.children().iter().filter(|n| n.is_stop()).collect();
    matches!(stops.as_slice(), [only] if matches!(only.kind, Kind::Branch { .. }) && !only.later)
}

fn nearest_stop(rows: &[Node], i: usize) -> Option<usize> {
    let i = i.min(rows.len().checked_sub(1)?);
    (i..rows.len())
        .find(|&k| rows[k].is_stop())
        .or_else(|| (0..i).rev().find(|&k| rows[k].is_stop()))
}

fn find(rows: &[Node], preset: &str, path: &mut Vec<usize>) -> bool {
    for (i, n) in rows.iter().enumerate() {
        path.push(i);
        match &n.kind {
            Kind::View(e) if e.preset == preset => return true,
            Kind::Branch { children, .. } if find(children, preset, path) => return true,
            _ => {}
        }
        path.pop();
    }
    false
}

// ---------------------------------------------------------------------------
// The table.

fn heading(title: &str) -> Node {
    Node {
        title: title.to_string(),
        tone: Tone::Quiet,
        hint: None,
        band: None,
        about: None,
        later: false,
        refused: None,
        kind: Kind::Heading,
    }
}

fn branch(title: &str, tone: Tone, children: Vec<Node>) -> Node {
    Node {
        title: title.to_string(),
        tone,
        hint: None,
        band: None,
        about: None,
        later: false,
        refused: None,
        kind: Kind::Branch {
            section: None,
            children,
        },
    }
}

/// A row whose children are the views of menu section `id`, filled by
/// [`build`].
fn group(title: &str, tone: Tone, id: &str) -> Node {
    Node {
        kind: Kind::Branch {
            section: Some(id.to_string()),
            children: Vec::new(),
        },
        ..branch(title, tone, Vec::new())
    }
}

impl Node {
    fn hint(mut self, hint: &'static str) -> Self {
        self.hint = Some(hint);
        self
    }

    fn band(mut self, band: Band) -> Self {
        self.band = Some(band);
        self
    }

    fn about(mut self, about: &'static str) -> Self {
        self.about = Some(about);
        self
    }

    fn later(mut self) -> Self {
        self.later = true;
        self.hint = Some("later");
        self
    }
}

fn lab_table() -> Vec<Node> {
    use Tone::*;
    let net = crate::signal::net::SECTIONS;
    let wifi = crate::signal::net::WIFI_SECTIONS;
    vec![
        heading("SEE THE BAND"),
        group("Spectrum", Spectrum, "command_rail")
            .hint("any freq.")
            .about("The band around your tuning, live: the spectrum, the waterfall, or both on one ruler."),
        group("Sweep", Sweep, "sweep")
            .hint("wide")
            .about("Steps the radio across a range wider than it can see at once, and holds the peaks."),
        heading("CHECK THE RADIO"),
        group("Receiver", Receiver, "lab")
            .hint("health")
            .about("Whether the radio itself is telling the truth: its balance, its gain and noise, the USB link, and the signal it hands over."),
        heading("DECODE DEVICES"),
        branch(
            "Bluetooth",
            Ble,
            vec![branch(
                "2.4 GHz",
                Ble,
                vec![
                    group("Overview", Quiet, net[0])
                        .hint("the band")
                        .about("The band as a whole, before any one device: what this radio can reach here, and what is busy."),
                    group("Low Energy", Ble, net[1])
                        .hint("watches, tags")
                        .about("Bluetooth Low Energy: fitness bands, smart watches, item trackers, phones announcing themselves."),
                    group("Classic", Classic, net[2])
                        .hint("headsets, cars")
                        .about("Classic Bluetooth: headsets, car kits, speakers, hopping 1600 times a second across 79 channels."),
                    group("Long range", Coded, net[3])
                        .hint("LE Coded")
                        .about("LE Coded: the slow, long-reach Low Energy mode."),
                ],
            )
            .hint("2402–2480")
            .band(BAND_24)
            .about("Bluetooth lives only here: 79 classic channels and 40 Low Energy channels between 2402 and 2480 MHz.")],
        )
        .hint("2.4 GHz")
        .band(BAND_24)
        .about("Phones, watches, headsets and tags: what is talking, and how well."),
        branch(
            "Wi-Fi",
            Wifi,
            vec![
                branch(
                    "2.4 GHz",
                    Wifi,
                    vec![
                        group("Channel", Wifi, wifi[0])
                            .hint("busy or quiet")
                            .about("One channel at a time: which is busy, what is on it, where its time goes."),
                        group("Room", Wifi, wifi[1])
                            .later()
                            .about("Echoes and movement in the room, read from the frames themselves. Not built yet."),
                        group("Network", Wifi, wifi[2])
                            .later()
                            .about("One network and its devices. Not built yet."),
                        group("Transmitter", Wifi, wifi[3])
                            .later()
                            .about("One transmitter on the bench. Not built yet."),
                    ],
                )
                .hint("ch 1–13")
                .band(BAND_24)
                .about("Thirteen overlapping 20 MHz channels; 1, 6 and 11 are the three that do not overlap."),
                branch("5 GHz", Wifi, Vec::new())
                    .later()
                    .band(BAND_5)
                    .about("Channels 36 to 177, 20 to 160 MHz wide. Not built yet."),
            ],
        )
        .hint("2.4 · 5 GHz")
        .about("Routers, phones, laptops: who is using the air, and how."),
        heading("SMALL WINDOW"),
        group("Compact", Quiet, "micro")
            .hint("large type")
            .about("The same readings in large type, for a small terminal or a screen across the room."),
    ]
}

/// No Radio view exists yet: the tile says what it will hold, every row
/// later, and opens nothing.
fn radio_table() -> Vec<Node> {
    use Tone::Radio;
    let soon = |title: &str, hint: &'static str, about: &'static str| {
        branch(title, Radio, Vec::new())
            .later()
            .hint(hint)
            .about(about)
    };
    vec![
        heading("LISTEN"),
        soon(
            "FM broadcast",
            "88–108",
            "The stations on a kitchen radio. Not built yet.",
        ),
        soon(
            "Aviation",
            "118–137",
            "Air traffic control and pilots, in AM. Not built yet.",
        ),
        soon(
            "Marine",
            "156–162",
            "Ships and harbours; channel 16 is the calling and distress channel. Not built yet.",
        ),
        heading("AMATEUR RADIO"),
        soon(
            "Ham bands",
            "licensed",
            "Licensed radio amateurs: repeaters, simplex, satellites. Not built yet.",
        ),
        heading("PERSONAL RADIO"),
        soon(
            "Walkie-talkies",
            "licence-free",
            "PMR446, CB and LPD433 handhelds anyone can buy. Not built yet.",
        ),
    ]
}

/// Hang the menu's sections on the table, and the refused ones beside them.
pub fn build(menu: &Menu, refused: &[Refusal]) -> Tree {
    let mut lab = lab_table();
    let mut placed: Vec<&str> = Vec::new();
    fill(&mut lab, &menu.sections, refused, &mut placed);
    carry_refusals(&mut lab);

    // A section the table does not name: a user's own, or the presets that
    // named none. Each its own row under Other, in the menu's order, with
    // Other's own last as the menu already sorts it.
    let others: Vec<Node> = menu
        .sections
        .iter()
        .filter(|s| !placed.contains(&s.id.as_str()) && s.id != WIFI)
        .map(|s| {
            let mut row = group(&s.title, Tone::Quiet, &s.id);
            if let Kind::Branch { children, .. } = &mut row.kind {
                *children = views(s, None);
            }
            row
        })
        .collect();
    if !others.is_empty() {
        lab.push(heading("YOUR OWN"));
        lab.push(Node {
            kind: Kind::Branch {
                section: None,
                children: others,
            },
            ..branch(&super::model::section_title(OTHER), Tone::Quiet, Vec::new())
        });
    }

    prune(&mut lab);
    Tree {
        radio: radio_table(),
        lab,
    }
}

fn views(section: &Section, refused: Option<&str>) -> Vec<Node> {
    section
        .entries
        .iter()
        .map(|e| Node {
            title: e.title.clone(),
            tone: Tone::Quiet,
            hint: None,
            band: None,
            about: None,
            later: false,
            refused: refused.map(str::to_string),
            kind: Kind::View(e.clone()),
        })
        .collect()
}

fn fill<'a>(
    rows: &mut [Node],
    sections: &'a [Section],
    refusals: &[Refusal],
    placed: &mut Vec<&'a str>,
) {
    for row in rows {
        let later = row.later;
        let mut refused = None;
        if let Kind::Branch { section, children } = &mut row.kind {
            match section {
                Some(id) => {
                    if let Some(s) = sections.iter().find(|s| s.id == *id) {
                        placed.push(&s.id);
                        *children = views(s, None);
                    } else if let Some(r) = refusals.iter().find(|r| r.section.id == *id) {
                        *children = views(&r.section, Some(&r.why));
                        refused = Some(r.why.clone());
                    }
                }
                None => fill(children, sections, refusals, placed),
            }
        }
        if refused.is_some() {
            row.refused = refused;
        }
        // A preset filed under a later group makes it a real one.
        if later && !row.children().is_empty() {
            row.later = false;
            row.hint = None;
        }
    }
}

/// Grey a branch with nothing it can open and a refusal under it, in that
/// refusal's words. A later row beside a refused one does not keep its parent
/// open: it opens nothing either.
fn carry_refusals(rows: &mut [Node]) {
    for row in rows {
        let Kind::Branch { children, .. } = &mut row.kind else {
            continue;
        };
        carry_refusals(children);
        if row.refused.is_some() || children.is_empty() {
            continue;
        }
        let stops = || children.iter().filter(|n| n.is_stop());
        let openable = stops().any(|n| n.refused.is_none() && !n.later);
        if !openable {
            row.refused = stops().find_map(|n| n.refused.clone());
        }
    }
}

/// Drop the branches nothing filled and the table does not keep as later,
/// then the headings left with nothing under them.
fn prune(rows: &mut Vec<Node>) {
    for row in rows.iter_mut() {
        if let Kind::Branch { children, .. } = &mut row.kind {
            prune(children);
        }
    }
    rows.retain(|n| match &n.kind {
        Kind::Branch { children, .. } => n.later || children.iter().any(Node::is_stop),
        _ => true,
    });
    let mut keep = vec![true; rows.len()];
    for i in 0..rows.len() {
        if rows[i].is_stop() {
            continue;
        }
        let next = rows[i + 1..].iter().position(|n| !n.is_stop());
        let end = next.map_or(rows.len(), |k| i + 1 + k);
        keep[i] = end > i + 1;
    }
    let mut k = keep.into_iter();
    rows.retain(|_| k.next().unwrap_or(true));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{LayoutConfig, PresetConfig};

    fn tree_of(presets: &std::collections::HashMap<String, PresetConfig>) -> Tree {
        build(&super::super::model::build(presets), &[])
    }

    fn builtins() -> Tree {
        tree_of(&LayoutConfig::default_config().presets)
    }

    fn titles(rows: &[Node]) -> Vec<&str> {
        rows.iter().map(|n| n.title.as_str()).collect()
    }

    fn path_to(tree: &Tree, tile: Tile, want: &[&str]) -> Path {
        let mut rows = tree.root(tile);
        let mut out = Vec::new();
        for title in want {
            let i = rows
                .iter()
                .position(|n| n.is_stop() && n.title == *title)
                .unwrap_or_else(|| panic!("{title} in {:?}", titles(rows)));
            out.push(i);
            rows = rows[i].children();
        }
        Path { rows: out }
    }

    fn bare(section: Option<&str>, slot: Option<u8>) -> PresetConfig {
        PresetConfig {
            section: section.map(str::to_string),
            slot,
            ..Default::default()
        }
    }

    /// The Lab tree's first column, in the order a session goes: what is on
    /// the air, whether the radio is sound, who is talking, a small window.
    #[test]
    fn the_lab_tree_opens_on_its_areas() {
        let tree = builtins();
        assert_eq!(
            titles(&tree.lab),
            [
                "SEE THE BAND",
                "Spectrum",
                "Sweep",
                "CHECK THE RADIO",
                "Receiver",
                "DECODE DEVICES",
                "Bluetooth",
                "Wi-Fi",
                "SMALL WINDOW",
                "Compact"
            ]
        );
    }

    /// Bluetooth and Wi-Fi go through their band, then their groups.
    #[test]
    fn the_decoders_hang_under_their_band() {
        let tree = builtins();
        let bt = tree.node(
            Tile::Lab,
            &path_to(&tree, Tile::Lab, &["Bluetooth", "2.4 GHz"]),
        );
        assert_eq!(
            titles(bt.unwrap().children()),
            ["Overview", "Low Energy", "Classic", "Long range"]
        );
        let wifi = path_to(&tree, Tile::Lab, &["Wi-Fi"]);
        assert_eq!(
            titles(tree.node(Tile::Lab, &wifi).unwrap().children()),
            ["2.4 GHz", "5 GHz"]
        );
        let band = path_to(&tree, Tile::Lab, &["Wi-Fi", "2.4 GHz"]);
        let groups = tree.node(Tile::Lab, &band).unwrap().children();
        assert_eq!(
            titles(groups),
            ["Channel", "Room", "Network", "Transmitter"]
        );
        let later: Vec<bool> = groups.iter().map(|n| n.later).collect();
        assert_eq!(later, [false, true, true, true]);
    }

    /// Every built-in preset the menu lists is in the Lab tree exactly once,
    /// and at the view a section's slot names.
    #[test]
    fn every_listed_preset_has_one_place() {
        let presets = LayoutConfig::default_config().presets;
        let menu = super::super::model::build(&presets);
        let tree = build(&menu, &[]);
        let mut seen = 0;
        for section in &menu.sections {
            for entry in &section.entries {
                let (tile, path) = tree.locate(&entry.preset).expect(&entry.preset);
                assert_eq!(tile, Tile::Lab);
                let node = tree.node(tile, &path).unwrap();
                assert_eq!(node.view(), Some(entry));
                assert_eq!(tree.section_at(tile, &path), Some(section.id.as_str()));
                seen += 1;
            }
        }
        let lab_views: usize = tree.lab.iter().map(Node::view_count).sum();
        assert_eq!(lab_views, seen);
        assert!(tree.locate("observer").is_none(), "hidden stays hidden");
    }

    /// The Radio tile holds nothing that opens yet, and says what it will.
    #[test]
    fn the_radio_tree_is_all_later() {
        let tree = builtins();
        let rows: Vec<&Node> = tree.radio.iter().filter(|n| n.is_stop()).collect();
        assert!(!rows.is_empty());
        assert!(rows.iter().all(|n| n.later && n.about.is_some()));
        assert_eq!(tree.radio.iter().map(Node::view_count).sum::<usize>(), 0);
        let first = tree.clamp(Tile::Radio, &Path::default()).unwrap();
        assert_eq!(tree.descend(Tile::Radio, &first), None);
    }

    /// `→` from Bluetooth lands in its groups, through the band, and `←`
    /// comes back out the same way.
    #[test]
    fn a_single_band_is_passed_through() {
        let tree = builtins();
        let bt = path_to(&tree, Tile::Lab, &["Bluetooth"]);
        let inside = tree.descend(Tile::Lab, &bt).unwrap();
        assert_eq!(
            inside,
            path_to(&tree, Tile::Lab, &["Bluetooth", "2.4 GHz", "Overview"])
        );
        assert_eq!(tree.ascend(Tile::Lab, &inside), Some(bt));
        // Wi-Fi has two bands, so its band column is a stop.
        let wifi = path_to(&tree, Tile::Lab, &["Wi-Fi"]);
        assert_eq!(
            tree.descend(Tile::Lab, &wifi),
            Some(path_to(&tree, Tile::Lab, &["Wi-Fi", "2.4 GHz"]))
        );
    }

    /// Nothing opens past a view or a later row, and nothing is above the
    /// first column.
    #[test]
    fn descend_and_ascend_stop_at_the_ends() {
        let tree = builtins();
        let view = path_to(&tree, Tile::Lab, &["Receiver", "IQ"]);
        assert_eq!(tree.descend(Tile::Lab, &view), None);
        let room = path_to(&tree, Tile::Lab, &["Wi-Fi", "2.4 GHz", "Room"]);
        assert_eq!(tree.descend(Tile::Lab, &room), None);
        let top = path_to(&tree, Tile::Lab, &["Spectrum"]);
        assert_eq!(tree.ascend(Tile::Lab, &top), None);
    }

    /// Up and down skip headings and stop at the ends.
    #[test]
    fn steps_skip_headings_and_do_not_wrap() {
        let tree = builtins();
        let sweep = path_to(&tree, Tile::Lab, &["Sweep"]);
        let receiver = tree.step(Tile::Lab, &sweep, true);
        assert_eq!(receiver, path_to(&tree, Tile::Lab, &["Receiver"]));
        assert_eq!(tree.step(Tile::Lab, &receiver, false), sweep);
        let first = path_to(&tree, Tile::Lab, &["Spectrum"]);
        assert_eq!(tree.step(Tile::Lab, &first, false), first);
        let last = path_to(&tree, Tile::Lab, &["Compact"]);
        assert_eq!(tree.step(Tile::Lab, &last, true), last);
    }

    /// A path out of range, or on a heading, is pulled onto a stop.
    #[test]
    fn a_bad_path_is_clamped_onto_a_stop() {
        let tree = builtins();
        let on_heading = Path { rows: vec![0] };
        assert_eq!(
            tree.clamp(Tile::Lab, &on_heading),
            Some(path_to(&tree, Tile::Lab, &["Spectrum"]))
        );
        let past = Path {
            rows: vec![99, 99, 99, 99, 99],
        };
        // Each index is clamped in its own column, so the path goes as deep
        // as the tree does: the last area's last view.
        let clamped = tree.clamp(Tile::Lab, &past).unwrap();
        assert_eq!(clamped.rows.len(), 2);
        assert_eq!(tree.node(Tile::Lab, &clamped).unwrap().title, "Health");
        let empty = Tree {
            radio: Vec::new(),
            lab: Vec::new(),
        };
        assert_eq!(empty.clamp(Tile::Lab, &Path::default()), None);
    }

    /// A user preset in a known group lands there; in a group of its own
    /// making, or none, under Other, which is absent until then.
    #[test]
    fn user_presets_land_where_their_section_says() {
        assert!(!titles(&builtins().lab).contains(&"Other"));
        let mut presets = LayoutConfig::default_config().presets;
        presets.insert("mine_le".into(), bare(Some("le"), Some(9)));
        presets.insert("mine_night".into(), bare(Some("nightwatch"), Some(1)));
        presets.insert("mine_loose".into(), bare(None, None));
        let tree = tree_of(&presets);
        let (_, le) = tree.locate("mine_le").unwrap();
        assert_eq!(tree.section_at(Tile::Lab, &le), Some("le"));
        let other = path_to(&tree, Tile::Lab, &["Other"]);
        assert_eq!(
            titles(tree.node(Tile::Lab, &other).unwrap().children()),
            ["nightwatch", "Other"]
        );
        let (_, night) = tree.locate("mine_night").unwrap();
        assert_eq!(tree.section_at(Tile::Lab, &night), Some("nightwatch"));
        assert!(tree.locate("mine_loose").is_some());
    }

    /// A view filed under a later group makes it a real row.
    #[test]
    fn a_later_group_comes_alive_with_its_first_view() {
        let mut presets = LayoutConfig::default_config().presets;
        presets.insert("wifi_paths".into(), bare(Some("wifi_room"), Some(1)));
        let tree = tree_of(&presets);
        let room = path_to(&tree, Tile::Lab, &["Wi-Fi", "2.4 GHz", "Room"]);
        let node = tree.node(Tile::Lab, &room).unwrap();
        assert!(!node.later);
        assert_eq!(node.view_count(), 1);
        assert!(tree.descend(Tile::Lab, &room).is_some());
    }

    /// With the NET presets gone, Bluetooth has nothing in it and leaves;
    /// Wi-Fi keeps its later rows, so it stays with something to say.
    #[test]
    fn an_empty_area_is_left_out() {
        let mut presets = LayoutConfig::default_config().presets;
        presets.retain(|_, p| !p.section.as_deref().is_some_and(crate::signal::net::is_net));
        let tree = tree_of(&presets);
        let first = titles(&tree.lab);
        assert!(!first.contains(&"Bluetooth"), "{first:?}");
        assert!(first.contains(&"Wi-Fi"), "{first:?}");
    }

    /// A heading with nothing left under it goes with its rows.
    #[test]
    fn an_orphaned_heading_is_left_out() {
        let mut presets = LayoutConfig::default_config().presets;
        presets.retain(|_, p| p.section.as_deref() != Some("micro"));
        let first = titles(&tree_of(&presets).lab).join(" ");
        assert!(!first.contains("SMALL WINDOW"), "{first}");
        assert!(!first.contains("Compact"), "{first}");
    }

    /// The NET presets refused as the gate refuses them on a radio that
    /// stops below the band: every Bluetooth and Wi-Fi view listed by its
    /// real title, refused with the reason, and the reason carried up to
    /// Bluetooth and Wi-Fi themselves.
    fn refused_tree() -> Tree {
        let mut presets = LayoutConfig::default_config().presets;
        let net: std::collections::HashMap<String, PresetConfig> = presets
            .extract_if(|_, p| p.section.as_deref().is_some_and(crate::signal::net::is_net))
            .collect();
        let menu = super::super::model::build(&presets);
        let refusals: Vec<Refusal> = super::super::model::build(&net)
            .sections
            .into_iter()
            .map(|section| Refusal {
                section,
                why: "this radio tunes 24.000 to 1800.000 MHz".into(),
            })
            .collect();
        build(&menu, &refusals)
    }

    #[test]
    fn refused_views_are_listed_with_the_reason() {
        let tree = refused_tree();
        assert_eq!(titles(&builtins().lab), titles(&tree.lab), "nothing hidden");
        let (_, census) = tree.locate("net_census").expect("listed");
        let node = tree.node(Tile::Lab, &census).unwrap();
        assert!(!node.opens());
        assert_eq!(node.title, "Census");
        assert!(node.refused.as_deref().unwrap().contains("1800.000"));
        for area in ["Bluetooth", "Wi-Fi"] {
            let path = path_to(&tree, Tile::Lab, &[area]);
            let row = tree.node(Tile::Lab, &path).unwrap();
            assert!(row.refused.is_some(), "{area} carries the refusal");
            // Still browsable: what exists is shown, it just does not open.
            assert!(tree.descend(Tile::Lab, &path).is_some(), "{area}");
        }
        let later = path_to(&tree, Tile::Lab, &["Wi-Fi", "2.4 GHz", "Room"]);
        let room = tree.node(Tile::Lab, &later).unwrap();
        assert!(room.later && room.refused.is_none(), "later stays later");
        let spectrum = path_to(&tree, Tile::Lab, &["Spectrum"]);
        assert!(tree.node(Tile::Lab, &spectrum).unwrap().refused.is_none());
    }

    /// An admitted view opens; a group never does.
    #[test]
    fn only_an_admitted_view_opens() {
        let tree = builtins();
        let (_, census) = tree.locate("net_census").unwrap();
        assert!(tree.node(Tile::Lab, &census).unwrap().opens());
        let le = path_to(&tree, Tile::Lab, &["Bluetooth", "2.4 GHz", "Low Energy"]);
        assert!(!tree.node(Tile::Lab, &le).unwrap().opens());
    }

    /// Each band row marks its own band: Wi-Fi spans two and claims
    /// neither, so 5 GHz is never drawn at 2.4.
    #[test]
    fn every_band_row_carries_its_own_band() {
        let tree = builtins();
        let wifi = path_to(&tree, Tile::Lab, &["Wi-Fi"]);
        assert_eq!(tree.node(Tile::Lab, &wifi).unwrap().band, None);
        let five = path_to(&tree, Tile::Lab, &["Wi-Fi", "5 GHz"]);
        assert_eq!(tree.node(Tile::Lab, &five).unwrap().band, Some(BAND_5));
        let two = path_to(&tree, Tile::Lab, &["Wi-Fi", "2.4 GHz"]);
        assert_eq!(tree.node(Tile::Lab, &two).unwrap().band, Some(BAND_24));
    }

    /// The band rows reach exactly what the NET gate admits.
    #[test]
    fn the_band_is_the_gates_band() {
        assert!(BAND_24.within(1_000_000, 6_000_000_000));
        assert!(!BAND_24.within(24_000_000, 1_800_000_000));
        assert!(!BAND_24.within(1_000_000, LOWEST_CENTRE_HZ + 1));
    }

    #[test]
    fn the_build_is_deterministic() {
        assert_eq!(builtins(), builtins());
    }

    /// The trail names each row the path passes through, and stops at a
    /// bad index rather than inventing one.
    #[test]
    fn the_trail_names_the_path() {
        let tree = builtins();
        let path = path_to(&tree, Tile::Lab, &["Wi-Fi", "2.4 GHz", "Channel", "Frames"]);
        let names: Vec<&str> = tree
            .trail(Tile::Lab, &path)
            .iter()
            .map(|n| n.title.as_str())
            .collect();
        assert_eq!(names, ["Wi-Fi", "2.4 GHz", "Channel", "Frames"]);
        let mut bad = path.clone();
        bad.rows[1] = 99;
        assert_eq!(tree.trail(Tile::Lab, &bad).len(), 1);
    }
}
