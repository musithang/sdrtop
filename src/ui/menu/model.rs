//! Presets to sections.
//!
//! The only part of the menu with logic, and the only part that imports no
//! `Theme`, knows no width and never sees ratatui. Presets go in, sections come
//! out, and the two things that can go wrong (a preset filed nowhere, two
//! presets claiming one number key) are answers rather than panics.
//!
//! Keeping it drawing-free is what makes it testable with real assertions
//! instead of a rendered buffer, the same reason `iq_diagnostics/verdict.rs`
//! imports no theme.

use std::collections::HashMap;

use crate::config::PresetConfig;

/// The section id that keeps a preset out of the menu altogether.
///
/// `observer` uses it: that layout is what you are given when another process
/// holds the radio, not something you pick, and offering it in a list of choices
/// would say otherwise.
pub const HIDDEN: &str = "hidden";

/// The section a preset lands in when it names none. Only exists when something
/// is actually in it, so a default install never shows an empty row.
pub const OTHER: &str = "other";

/// Display order and titles for the sections the built-ins use.
///
/// A section id a user preset invents is not in here: it sorts after these by
/// id, and [`OTHER`] sorts last of all.
const KNOWN: &[(&str, &str)] = &[
    ("command_rail", "Command Rail"),
    ("lab", "Lab"),
    ("sweep", "Sweep"),
    ("micro", "Micro"),
];

/// One layout, as the menu shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The preset name, which is what `LayoutEngine::set_preset` wants.
    pub preset: String,
    /// The number key, if it won one.
    pub slot: Option<u8>,
    pub title: String,
    pub blurb: Option<String>,
}

/// A named group of layouts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub id: String,
    pub title: String,
    /// Slotted entries first in slot order, then the slotless ones by title.
    pub entries: Vec<Entry>,
}

/// Everything the menu draws, plus anything odd found while building it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Menu {
    pub sections: Vec<Section>,
    /// Lines for the caller to log. Kept out of the build itself so this module
    /// stays pure: a slot collision is worth telling the user about, but not at
    /// the cost of this function needing a mutex.
    pub warnings: Vec<String>,
}

impl Menu {
    pub fn section(&self, id: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.id == id)
    }

    /// Where a preset sits, as `(section index, entry index)`, if the menu lists
    /// it at all. This is both the cursor position the menu opens on and the
    /// answer to "which section am I in".
    pub fn locate(&self, preset: &str) -> Option<(usize, usize)> {
        self.sections.iter().enumerate().find_map(|(si, s)| {
            s.entries
                .iter()
                .position(|e| e.preset == preset)
                .map(|ei| (si, ei))
        })
    }

    pub fn entry(&self, preset: &str) -> Option<&Entry> {
        let (si, ei) = self.locate(preset)?;
        Some(&self.sections[si].entries[ei])
    }
}

/// Build the menu from the merged preset table.
pub fn build(presets: &HashMap<String, PresetConfig>) -> Menu {
    // A HashMap has no order, so sort by preset name first. Everything below
    // inherits that determinism, including which preset wins a contested slot.
    // Without this the menu would reshuffle itself between runs.
    let mut named: Vec<(&String, &PresetConfig)> = presets.iter().collect();
    named.sort_by(|a, b| a.0.cmp(b.0));

    let mut warnings = Vec::new();
    let mut grouped: HashMap<String, Vec<Entry>> = HashMap::new();

    for (name, preset) in named {
        let id = preset.section.as_deref().unwrap_or(OTHER);
        if id == HIDDEN {
            continue;
        }
        let bucket = grouped.entry(id.to_string()).or_default();

        // First by name keeps a contested slot. The loser stays on the list
        // without a key rather than disappearing: dropping it would hide a
        // layout the user wrote, which is a worse answer than "no shortcut".
        let slot = match preset.slot {
            Some(s) if bucket.iter().any(|e| e.slot == Some(s)) => {
                warnings.push(format!(
                    "preset '{name}' wanted slot {s} in section '{id}', which is taken"
                ));
                None
            }
            other => other,
        };

        bucket.push(Entry {
            preset: name.clone(),
            slot,
            title: preset.title.clone().unwrap_or_else(|| name.clone()),
            blurb: preset.blurb.clone(),
        });
    }

    let mut sections: Vec<Section> = grouped
        .into_iter()
        .map(|(id, mut entries)| {
            // Slotted first in slot order, then the slotless by title. `None`
            // sorts after `Some` because the key leads with `slot.is_none()`.
            entries.sort_by(|a, b| {
                (a.slot.is_none(), a.slot, &a.title).cmp(&(b.slot.is_none(), b.slot, &b.title))
            });
            let title = section_title(&id);
            Section { id, title, entries }
        })
        .collect();

    sections.sort_by(|a, b| (section_rank(&a.id), &a.id).cmp(&(section_rank(&b.id), &b.id)));
    Menu { sections, warnings }
}

fn section_title(id: &str) -> String {
    if let Some((_, title)) = KNOWN.iter().find(|(k, _)| *k == id) {
        return (*title).to_string();
    }
    if id == OTHER {
        return "Other".to_string();
    }
    id.to_string()
}

/// Known sections in their declared order, then anything a user invented, then
/// Other, which is always last.
fn section_rank(id: &str) -> usize {
    if id == OTHER {
        return KNOWN.len() + 1;
    }
    KNOWN
        .iter()
        .position(|(k, _)| *k == id)
        .unwrap_or(KNOWN.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LayoutConfig;

    fn bare_preset(section: Option<&str>, slot: Option<u8>) -> PresetConfig {
        PresetConfig {
            section: section.map(str::to_string),
            slot,
            ..Default::default()
        }
    }

    /// The built-ins land in the four sections the design names, in order.
    #[test]
    fn the_builtins_build_four_sections() {
        let menu = build(&LayoutConfig::default_config().presets);
        let ids: Vec<&str> = menu.sections.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["command_rail", "lab", "sweep", "micro"]);
        assert!(menu.warnings.is_empty(), "{:?}", menu.warnings);
    }

    /// Slot order, not file order and not hash order.
    #[test]
    fn entries_come_out_in_slot_order() {
        let menu = build(&LayoutConfig::default_config().presets);
        let lab = menu.section("lab").expect("lab section");
        let names: Vec<&str> = lab.entries.iter().map(|e| e.preset.as_str()).collect();
        assert_eq!(names, ["lab_iq", "lab_rf", "lab_timing", "lab_signal"]);
    }

    /// `main` was unreachable before this change: a built-in layout that no key
    /// selected, findable only by accident in the `[P]` cycle.
    #[test]
    fn the_classic_layout_is_reachable() {
        let menu = build(&LayoutConfig::default_config().presets);
        let rail = menu.section("command_rail").expect("rail section");
        let classic = rail
            .entries
            .iter()
            .find(|e| e.preset == "main")
            .expect("main must be in the menu");
        assert_eq!(classic.slot, Some(5));
        assert_eq!(classic.title, "Classic");
    }

    /// Sweep keeps `lab_sweep` under its own section without renaming it. The
    /// name drives `is_lab_mode` and the sweep task, so the section had to be
    /// data for this to be possible at all.
    #[test]
    fn the_sweep_section_holds_a_lab_prefixed_preset() {
        let menu = build(&LayoutConfig::default_config().presets);
        let sweep = menu.section("sweep").expect("sweep section");
        let names: Vec<&str> = sweep.entries.iter().map(|e| e.preset.as_str()).collect();
        assert_eq!(names, ["lab_sweep", "micro_sweep"]);
    }

    /// `observer` is given, not chosen, so it stays out of the menu.
    #[test]
    fn hidden_presets_are_not_listed() {
        let menu = build(&LayoutConfig::default_config().presets);
        assert!(menu.entry("observer").is_none());
        assert!(menu.locate("observer").is_none());
    }

    /// A user preset with no section is still reachable, in Other, which exists
    /// only when something is in it.
    #[test]
    fn a_preset_without_a_section_lands_in_other() {
        let mut presets = LayoutConfig::default_config().presets;
        presets.insert("my_thing".into(), bare_preset(None, None));
        let menu = build(&presets);

        let other = menu.section(OTHER).expect("other section");
        assert_eq!(other.title, "Other");
        assert_eq!(other.entries.len(), 1);
        // No title declared, so the preset name is the title.
        assert_eq!(other.entries[0].title, "my_thing");
        assert_eq!(other.entries[0].slot, None);
        // Other sorts last, after the four built-in sections.
        assert_eq!(menu.sections.last().unwrap().id, OTHER);
    }

    /// Nothing lands in Other unless something asks to.
    #[test]
    fn other_is_absent_when_every_preset_is_filed() {
        let menu = build(&LayoutConfig::default_config().presets);
        assert!(menu.section(OTHER).is_none());
    }

    /// A section id nobody declared still works: it sorts after the known ones
    /// and takes its id as its title.
    #[test]
    fn an_invented_section_sorts_after_the_known_ones() {
        let mut presets = LayoutConfig::default_config().presets;
        presets.insert("mine".into(), bare_preset(Some("nightwatch"), Some(1)));
        let menu = build(&presets);

        let ids: Vec<&str> = menu.sections.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["command_rail", "lab", "sweep", "micro", "nightwatch"]);
        assert_eq!(menu.section("nightwatch").unwrap().title, "nightwatch");
    }

    /// Two presets cannot own one number key in one section. The first by name
    /// keeps it, the second stays listed without a key, and the caller is told.
    #[test]
    fn a_slot_collision_is_reported_and_the_loser_stays_listed() {
        let mut presets = LayoutConfig::default_config().presets;
        presets.insert("zz_intruder".into(), bare_preset(Some("lab"), Some(1)));
        let menu = build(&presets);
        let lab = menu.section("lab").expect("lab section");

        let iq = lab.entries.iter().find(|e| e.preset == "lab_iq").unwrap();
        let intruder = lab
            .entries
            .iter()
            .find(|e| e.preset == "zz_intruder")
            .expect("the loser must still be listed");
        assert_eq!(iq.slot, Some(1), "the first by name keeps the slot");
        assert_eq!(
            intruder.slot, None,
            "the loser keeps its place, loses its key"
        );

        assert_eq!(menu.warnings.len(), 1);
        assert!(
            menu.warnings[0].contains("zz_intruder") && menu.warnings[0].contains("lab"),
            "the warning must name the preset and the section: {:?}",
            menu.warnings[0]
        );
    }

    /// The same slot in two different sections is the whole point, not a clash.
    #[test]
    fn the_same_slot_in_two_sections_is_fine() {
        let menu = build(&LayoutConfig::default_config().presets);
        assert!(menu.warnings.is_empty());
        for section in ["command_rail", "lab", "sweep", "micro"] {
            let s = menu.section(section).unwrap();
            assert_eq!(s.entries[0].slot, Some(1), "{section} has no slot 1");
        }
    }

    /// Building twice gives the same answer. `presets` is a `HashMap`, so an
    /// iteration-order dependency here would surface as a menu that reshuffles
    /// itself between runs, which is exactly the kind of bug nobody reports.
    #[test]
    fn the_build_is_deterministic() {
        let presets = LayoutConfig::default_config().presets;
        assert_eq!(build(&presets), build(&presets));
    }

    /// An empty config is not a crash. `LayoutConfig` can in principle carry no
    /// presets at all, and the menu has to render that as nothing rather than
    /// panic on an empty section list.
    #[test]
    fn an_empty_preset_table_builds_an_empty_menu() {
        let menu = build(&HashMap::new());
        assert!(menu.sections.is_empty());
        assert!(menu.warnings.is_empty());
        assert!(menu.locate("anything").is_none());
    }
}
