// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The panel registry and the layout engine built on it.
//!
//! Registration is the one list that makes a panel addressable: a preset names
//! panels by string and the input dispatch matches them by string, so nothing in
//! the type system connects either to the code. The tests at the foot of this
//! file are what stands in for that - they read the presets and the dispatch
//! table as data and check them against the registry built here.

use std::collections::HashMap;

use crate::config::LayoutConfig;
use crate::ui;

use crate::app::{App, FocusKeys};

impl App {
    #[cfg(test)]
    pub(crate) fn build_ui(
        active_preset: &str,
        user_presets: &HashMap<String, crate::config::PresetConfig>,
        presets_dir: Option<&std::path::Path>,
        net_admitted: bool,
    ) -> (ui::LayoutEngine, FocusKeys) {
        Self::build_ui_for(
            active_preset,
            user_presets,
            presets_dir,
            net_admitted,
            crate::hardware::AcquisitionKind::IqSamples,
        )
        .expect("built-in IQ layouts must include a usable preset")
    }

    /// `net_admitted` comes from `signal::net::gate`: on a radio that cannot
    /// reach the 2.4 GHz band, or cannot run even the cheapest mode there, the
    /// presets in that section are dropped here and the section is **absent**
    /// from the menu rather than present and empty. Rule 2, and the same
    /// decision the RF bench makes about its noise-figure card.
    pub(super) fn build_ui_for(
        active_preset: &str,
        user_presets: &HashMap<String, crate::config::PresetConfig>,
        presets_dir: Option<&std::path::Path>,
        net_admitted: bool,
        acquisition: crate::hardware::AcquisitionKind,
    ) -> anyhow::Result<(ui::LayoutEngine, FocusKeys)> {
        let mut registry = ui::PanelRegistry::new();
        registry.register(ui::HeaderPanel);
        registry.register(ui::SlimHeaderPanel);
        registry.register(ui::CommandRailPanel);
        registry.register(ui::LabBannerPanel);
        registry.register(ui::LabMarkerPanel);
        registry.register(ui::SignalStripPanel);
        registry.register(ui::LogPanel);
        registry.register(ui::FooterPanel);
        registry.register(ui::IqConstellationPanel);
        registry.register(ui::IqDiagnosticsPanel);
        registry.register(ui::ImageScopePanel);
        registry.register(ui::SystemResourcesPanel);
        registry.register(ui::SpectrumPanel);
        registry.register(ui::WaterfallPanel::new());
        registry.register(ui::RfChainPanel);
        registry.register(ui::LevelDiagramPanel);
        registry.register(ui::AdcLoadingPanel);
        registry.register(ui::SignalMetricsPanel);
        registry.register(ui::SignalCharacterizationPanel);
        registry.register(ui::FmDemodPanel);
        registry.register(ui::IqHistogramPanel);
        registry.register(ui::ObserverPanel);
        registry.register(ui::MicroPanel);
        registry.register(ui::MicroSignalPanel);
        registry.register(ui::MicroGainPanel);
        registry.register(ui::MicroHealthPanel);
        registry.register(ui::TimingDiagnosticsPanel);
        registry.register(ui::TimingStripchartPanel);
        registry.register(ui::TimingVitalsPanel);
        registry.register(ui::SweepPanel);
        registry.register(ui::SweepStripPanel);
        registry.register(ui::MicroSweepPanel);
        registry.register(ui::NetCapabilityPanel);
        registry.register(ui::NetBlePacketsPanel);
        registry.register(ui::NetBtCensusPanel);
        registry.register(ui::NetBtHopsPanel);
        registry.register(ui::NetBleDetailPanel);
        registry.register(ui::NetCensusPanel);
        registry.register(ui::NetCoexistPanel);
        registry.register(ui::NetDecodeHealthPanel);
        registry.register(ui::NetOccupancyPanel);

        let mut layout = LayoutConfig::with_user_presets(user_presets, presets_dir);
        if !net_admitted {
            layout
                .presets
                .retain(|_, p| p.section.as_deref() != Some(ui::menu::model::NET));
        }
        let mut warnings = Self::filter_incompatible_layouts(&mut layout, &registry, acquisition)?;
        // Harvested against the presets that will actually be offered, user
        // presets included: a letter two panels share is only a clash where one
        // layout shows both. The built-in presets are held clear of that by a
        // test; a user preset that brings two together is told about it here,
        // and the letter goes to the first of them that is visible.
        let (focus_keys, collisions) = harvest_focus_keys(&registry, &layout.presets);
        for c in &collisions {
            warnings.push(format!(
                "Preset '{}' shows {} with the same focus key '{}'; it focuses '{}'",
                c.preset,
                c.panels.join(" and "),
                c.key,
                c.panels[0]
            ));
        }
        let selected = if layout
            .presets
            .get(active_preset)
            .is_some_and(|preset| has_registered_panel(preset, &registry))
        {
            active_preset.to_string()
        } else {
            ["spectrum_waterfall", "spectrum", "waterfall"]
                .into_iter()
                .find(|name| {
                    layout
                        .presets
                        .get(*name)
                        .is_some_and(|preset| has_registered_panel(preset, &registry))
                })
                .map(str::to_string)
                .or_else(|| {
                    let mut names: Vec<String> = layout
                        .presets
                        .iter()
                        .filter(|(_, preset)| has_registered_panel(preset, &registry))
                        .map(|(name, _)| name.clone())
                        .collect();
                    names.sort();
                    names.into_iter().next()
                })
                .ok_or_else(|| anyhow::anyhow!("No usable presets remain for this device"))?
        };
        if selected != active_preset {
            warnings.push(format!(
                "Preset '{active_preset}' is unavailable; using '{selected}'"
            ));
        }
        layout.active_preset = selected;

        let mut engine =
            ui::LayoutEngine::new_with_saved_preset(layout, registry, active_preset.to_string());
        engine.set_startup_warnings(warnings);
        Ok((engine, focus_keys))
    }

    fn filter_incompatible_layouts(
        config: &mut LayoutConfig,
        registry: &ui::PanelRegistry,
        acquisition: crate::hardware::AcquisitionKind,
    ) -> anyhow::Result<Vec<String>> {
        let mut warnings = Vec::new();
        if acquisition == crate::hardware::AcquisitionKind::PowerTrace {
            if let Some(preset) = config.presets.get_mut("lab_sweep") {
                preset.panels.retain(|panel| panel.name != "signal_metrics");
            }
        }
        config.presets.retain(|name, preset| {
            if preset.panels.is_empty() {
                warnings.push(format!("Preset '{name}' is unavailable because it has no panels"));
                return false;
            }
            for spec in &preset.panels {
                let Some(panel) = registry.get(&spec.name) else {
                    warnings.push(format!(
                        "Preset '{name}' references unknown panel '{}'",
                        spec.name
                    ));
                    continue;
                };
                if !panel.supports_acquisition(acquisition) {
                    warnings.push(format!(
                        "Preset '{name}' is unavailable because panel '{}' does not support this device",
                        spec.name
                    ));
                    return false;
                }
            }
            true
        });
        if !config
            .presets
            .values()
            .any(|preset| has_registered_panel(preset, registry))
        {
            anyhow::bail!(
                "No usable presets remain for this device: {}",
                warnings.join("; ")
            );
        }
        Ok(warnings)
    }
}

fn has_registered_panel(
    preset: &crate::config::PresetConfig,
    registry: &ui::PanelRegistry,
) -> bool {
    preset
        .panels
        .iter()
        .any(|spec| registry.get(&spec.name).is_some())
}

/// A preset that shows two panels claiming one focus letter.
#[derive(Debug, PartialEq)]
struct Collision {
    key: char,
    preset: String,
    panels: Vec<&'static str>,
}

/// Collect each panel's focus letter into the lookup the key handler uses, and
/// report every preset that shows two panels claiming the same letter.
///
/// The report is the point of this being split out. A letter two panels on one
/// screen both claim is the bug that once made `v` and `t` work only on some
/// launches, when the lookup was a map from a letter to one panel and the
/// registry's iteration order picked the winner. Sharing is safe exactly where
/// no screen holds both, so that is what is checked.
fn harvest_focus_keys(
    registry: &ui::PanelRegistry,
    presets: &HashMap<String, crate::config::PresetConfig>,
) -> (FocusKeys, Vec<Collision>) {
    let mut keys: FocusKeys = HashMap::new();
    for panel in registry.panels_iter() {
        if let Some(key) = panel.focus_key() {
            keys.entry(key).or_default().push(panel.name());
        }
    }
    for claimants in keys.values_mut() {
        claimants.sort();
    }
    let mut collisions = Vec::new();
    for (preset, config) in presets {
        for (key, claimants) in &keys {
            let shown: Vec<&'static str> = claimants
                .iter()
                .copied()
                .filter(|name| config.panels.iter().any(|spec| spec.name == *name))
                .collect();
            if shown.len() > 1 {
                collisions.push(Collision {
                    key: *key,
                    preset: preset.clone(),
                    panels: shown,
                });
            }
        }
    }
    collisions.sort_by(|a, b| (a.key, &a.preset).cmp(&(b.key, &b.preset)));
    (keys, collisions)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every panel that can be focused must have a handler arm in `handle_normal`.
    ///
    /// The bug this guards is not hypothetical. The `lab_timing` rebuild replaced
    /// `hardware_health` and `timing_panel` with `timing_vitals` and
    /// `timing_diagnostics`, but the dispatch kept naming the old two. Focusing
    /// either new panel then highlighted its border, printed `[R] Reset drop
    /// counter · [C] Clear history` in the footer, and dropped every key through
    /// to `handle_global` - where `[R]` resets the whole radio to defaults.
    ///
    /// A panel that offers keys and silently ignores them is worse than one with
    /// no focus mode at all, so this is checked rather than remembered.
    #[test]
    fn every_focusable_panel_has_a_dispatch_arm() {
        let (_engine, focus_keys) = App::build_ui("command_rail", &HashMap::new(), None, true);
        // The dispatch table is read as source text: the arms are `&str` matches on
        // a panel name, so nothing in the type system ties them to the registry.
        let dispatch = include_str!("../input/mod.rs");
        assert!(
            !focus_keys.is_empty(),
            "no focus keys were harvested at all"
        );
        for (key, panels) in &focus_keys {
            for panel in panels {
                let arm = format!("Some(\"{panel}\")");
                assert!(
                    dispatch.contains(&arm),
                    "panel '{panel}' claims focus key '{key}' but handle_normal has no \
                     `{arm}` arm, so its keys fall through to the global handler",
                );
            }
        }
    }

    /// Every panel a built-in preset names must exist in the registry.
    ///
    /// The presets are TOML and the registry is Rust, so nothing in the type
    /// system connects them: a panel renamed or removed leaves a preset quietly
    /// asking for a name that resolves to nothing, and the layout engine just
    /// draws a gap. Cheap to check, invisible otherwise.
    #[test]
    fn every_panel_named_by_a_builtin_preset_is_registered() {
        let (engine, _) = App::build_ui("command_rail", &HashMap::new(), None, true);
        let known: std::collections::HashSet<&str> = engine.registered_panel_names().collect();
        assert!(!known.is_empty(), "no panels were registered at all");

        let cfg = crate::config::LayoutConfig::default_config();
        let mut missing: Vec<String> = Vec::new();
        for (preset, spec) in &cfg.presets {
            for panel in &spec.panels {
                if !known.contains(panel.name.as_str()) {
                    missing.push(format!("{preset} -> {}", panel.name));
                }
            }
        }
        missing.sort();
        assert!(
            missing.is_empty(),
            "presets name panels that are not registered: {missing:?}"
        );
    }

    /// **Every NET preset stands in the same frame**: the header band on top,
    /// the log and the footer at the bottom, at the same heights. Switching
    /// presets inside the section should change the question on screen, not
    /// where the log went. Two of six presets had no log before this test
    /// existed, and nothing noticed, because a preset is TOML and a missing
    /// strip is a layout that still draws. Bands in between (a Lab-style
    /// banner or marker line) stay allowed; the frame is the outside edge.
    #[test]
    fn every_net_preset_stands_in_the_same_frame() {
        use crate::config::Position;
        let cfg = crate::config::LayoutConfig::default_config();
        let mut net: Vec<_> = cfg
            .presets
            .iter()
            .filter(|(_, p)| p.section.as_deref() == Some(ui::menu::model::NET))
            .collect();
        net.sort_by_key(|(name, _)| name.as_str());
        assert!(net.len() >= 5, "the NET section went missing: {net:?}");

        let shape = |s: &crate::config::PanelSpec| (s.name.clone(), s.position.clone(), s.height);
        let mut wrong = Vec::new();
        for (name, preset) in net {
            let panels: Vec<_> = preset.panels.iter().map(shape).collect();
            let first = panels.first().cloned();
            let tail: Vec<_> = panels.iter().rev().take(2).rev().cloned().collect();
            if first != Some(("header".into(), Position::Top, Some(5)))
                || tail
                    != [
                        ("log".into(), Position::Bottom, Some(5)),
                        ("footer".into(), Position::Bottom, None),
                    ]
            {
                wrong.push(format!("{name}: {panels:?}"));
            }
        }
        assert!(
            wrong.is_empty(),
            "NET presets outside the frame: {wrong:#?}"
        );
    }

    /// **Every panel in the NET section says how its numbers were gathered.**
    ///
    /// Design section 13.1 makes the mode part of the reading rather than a
    /// setting: a duty-cycle-sampled census and a complete capture are different
    /// claims, and a panel that shows one while the other was running is stating
    /// the wrong one. Asserted here rather than in each panel's own tests,
    /// because the rule is about the section and a rule enforced panel by panel
    /// is a rule the next panel will not know about.
    ///
    /// **The exemption is checkable rather than a list of names.** A panel may
    /// skip the tag only if it declares `Staleness::Never` - that is a panel
    /// saying nothing on it goes out of date, which is a panel saying nothing on
    /// it is a reading. `net_capability` qualifies: it draws the record built
    /// when the device was opened, which is the same record hopping or parked.
    /// A panel that wanted out of this rule by simply not carrying the tag would
    /// have to claim its numbers never age, which is a much harder thing to
    /// write by accident.
    #[test]
    fn every_net_panel_says_how_its_numbers_were_gathered() {
        use crate::state::{NetMode, SdrMetrics};
        use crate::ui::panel::Staleness;

        let (engine, _) = App::build_ui("net", &HashMap::new(), None, true);
        let mut seen = 0;
        for mode in [NetMode::Survey, NetMode::Lock] {
            let mut m = SdrMetrics::fixture();
            m.net.mode = mode;
            for panel in engine.registered_panels() {
                if !panel.name().starts_with("net_") {
                    continue;
                }
                seen += 1;
                let chrome = panel.chrome(&m);
                if chrome.staleness == Staleness::Never {
                    assert!(
                        !chrome.tags.contains(&mode.tag()),
                        "{}: a panel with nothing that ages has no mode to report",
                        panel.name()
                    );
                    continue;
                }
                assert!(
                    chrome.tags.contains(&mode.tag()),
                    "{} carries readings but does not say it is in {}",
                    panel.name(),
                    mode.label()
                );
                assert!(
                    !chrome.tags.contains(&mode.toggled().tag()),
                    "{} claims both modes at once",
                    panel.name()
                );
            }
        }
        assert!(seen >= 4, "only {seen} net panels were checked");
    }

    /// Every NET panel whose numbers are counted from the sample feed says how
    /// far back they reach, so the engine can caveat them when the feed lost
    /// something inside that span (foundation design 13.2). A panel that
    /// forgot would show a lower bound as a total on a lossy link, with
    /// nothing on screen to say so.
    ///
    /// The exemptions are named, each with its reason, so adding a panel
    /// means deciding which it is rather than falling through silently.
    #[test]
    fn every_net_panel_that_counts_from_the_feed_says_over_what_span() {
        use crate::state::{CellReading, SdrMetrics};
        use crate::ui::panel::Staleness;

        // Not counts from the feed, for the stated reason.
        const EXEMPT: &[(&str, &str)] = &[
            (
                "net_decode_health",
                "it is the account of the loss itself; caveating it with its own subject is circular",
            ),
            (
                "net_ble_detail",
                "one received packet's detail, not a count: a block lost elsewhere does not change it",
            ),
            (
                "net_bt_census",
                "a placeholder that shows no numbers (removed in net-ux-polish-plan Stop 6)",
            ),
        ];

        let (engine, _) = App::build_ui("net", &HashMap::new(), None, true);
        let mut m = SdrMetrics::fixture().streaming();
        // One measured cell, so the occupancy panel has numbers to span.
        m.net.band.cells.push(CellReading {
            measured: Some(std::time::Instant::now()),
            ..Default::default()
        });
        let mut checked = 0;
        for panel in engine.registered_panels() {
            let name = panel.name();
            if !name.starts_with("net_") {
                continue;
            }
            let chrome = panel.chrome(&m);
            if chrome.staleness == Staleness::Never || EXEMPT.iter().any(|(n, _)| *n == name) {
                assert!(
                    chrome.feed.is_none(),
                    "{name} is exempt but declares a feed span anyway"
                );
                continue;
            }
            checked += 1;
            assert!(
                chrome.feed.is_some(),
                "{name} counts from the feed but does not say over what span"
            );
        }
        assert!(checked >= 5, "only {checked} net panels were checked");
    }

    /// **Every NET panel that prints a ppm declares that it shows offsets**, so
    /// the engine can say what they are worth (`Tag::Offsets`: relative,
    /// traceable, or a reference that has expired). A ppm on screen without
    /// that tag is an absolute-looking number that may be our own oscillator's
    /// error in disguise, which is the claim `net-ux-polish-plan` 1.5.b exists
    /// to stop. Checked by rendering, against a state where every panel has an
    /// offset to show, rather than by a list of names the next panel would not
    /// be on. And the other way round: a panel that declares offsets and prints
    /// none carries a tag about numbers that are not there.
    #[test]
    fn every_net_panel_showing_a_ppm_says_what_it_is_worth() {
        let rendered = net_panels_with_one_device(crate::state::AddressDisplay::Full);
        let mut checked = 0;
        for (name, text, chrome) in &rendered {
            let prints_ppm = text.contains(" ppm");
            let declares = chrome.offsets;
            if prints_ppm || declares {
                checked += 1;
            }
            assert_eq!(
                prints_ppm, declares,
                "{name}: prints a ppm {prints_ppm}, declares offsets {declares}"
            );
        }
        assert!(checked >= 2, "only {checked} net panels showed an offset");
    }

    /// **Every NET panel that prints an address honours the display switch,
    /// and says so** (foundation design 1.1: one key, every panel). A panel
    /// printing the full address declares `shows_addresses`, so the engine
    /// tags it when the mode is not `full`; and with the mode switched, no
    /// panel anywhere in the section still prints the full address, which is
    /// what makes a screenshot in a masked mode safe to share.
    #[test]
    fn every_net_panel_showing_an_address_follows_the_switch() {
        use crate::state::AddressDisplay;
        const FULL: &str = "01:02:03:04:05:06";
        let mut checked = 0;
        for (name, text, chrome) in &net_panels_with_one_device(AddressDisplay::Full) {
            let prints = text.contains(FULL);
            if prints || chrome.addresses {
                checked += 1;
            }
            assert_eq!(
                prints, chrome.addresses,
                "{name}: prints an address {prints}, declares addresses {}",
                chrome.addresses
            );
        }
        assert!(checked >= 2, "only {checked} net panels showed an address");

        for display in [AddressDisplay::Oui, AddressDisplay::Masked] {
            for (name, text, chrome) in &net_panels_with_one_device(display) {
                assert!(
                    !text.contains(FULL),
                    "{name} ignores the {display:?} switch"
                );
                if display == AddressDisplay::Masked {
                    assert!(!text.contains("..05:06"), "{name} leaks part of it");
                    if chrome.addresses {
                        assert!(text.contains("#1"), "{name} shows no masked number");
                    }
                }
            }
        }
    }

    /// Every NET panel rendered, as text beside its chrome, against a state in
    /// which each has something to show: one advertiser, heard once, with an
    /// offset, in the census and in the packet feed.
    fn net_panels_with_one_device(
        display: crate::state::AddressDisplay,
    ) -> Vec<(String, String, ui::panel::PanelChrome)> {
        use crate::signal::dsp::uncertainty::Uncertain;
        use crate::state::{BlePacket, SdrMetrics};

        let (engine, _) = App::build_ui("net", &HashMap::new(), None, true);
        let now = std::time::Instant::now();
        let mut m = SdrMetrics::fixture().streaming();
        m.net.address_display = display;
        m.net.address_book.number([1, 2, 3, 4, 5, 6]);
        m.net.ble_channel = Some(37);
        m.net.ble_packets.push_front(BlePacket {
            seq: 0,
            channel: 37,
            pdu_type: crate::signal::ble::pdu::PduType::AdvInd,
            tx_add_random: false,
            ch_sel: false,
            rx_add_random: false,
            payload: Vec::new(),
            length: 20,
            adv_addr: Some([1, 2, 3, 4, 5, 6]),
            crc_ok: true,
            snr_db: Some(12.0),
            freq_offset_hz: Some(Uncertain::from_sigma(24_020.0, 500.0)),
            modulation: None,
            drift: None,
            seen: now,
        });
        crate::signal::net::census::observe(
            &mut m.net.census.devices,
            &crate::signal::net::census::Sighting {
                address: [1, 2, 3, 4, 5, 6],
                random: false,
                snr_db: Some(12.0),
                crystal_offset_ppm: Some(Uncertain::from_sigma(10.0, 0.2)),
                ble_pdu_code: Some(0x0),
                modulation_index: None,
                arrival: None,
            },
            now,
        );

        let theme = crate::Theme::sdr();
        engine
            .registered_panels()
            .filter(|panel| panel.name().starts_with("net_"))
            .map(|panel| {
                let backend = ratatui::backend::TestBackend::new(140, 30);
                let mut terminal = ratatui::Terminal::new(backend).unwrap();
                terminal
                    .draw(|f| panel.render(f, f.size(), &m, &theme, false))
                    .unwrap();
                let buffer = terminal.backend().buffer();
                let text: String = buffer.content().iter().map(|c| c.symbol()).collect();
                (panel.name().to_string(), text, panel.chrome(&m))
            })
            .collect()
    }

    /// Two panels claiming one letter are a clash **where one preset shows
    /// both**, and only there: the same pair kept on separate screens is the
    /// sharing that lets a NET panel reuse a Lab letter.
    ///
    /// Two stand-in panels prove the detector fires, so the assertion on the real
    /// presets below means something.
    #[test]
    fn a_shared_focus_letter_clashes_only_where_one_preset_shows_both() {
        struct First;
        struct Second;
        impl ui::panel::Panel for First {
            fn name(&self) -> &'static str {
                "first"
            }
            fn min_size(&self) -> (u16, u16) {
                (1, 1)
            }
            fn focus_key(&self) -> Option<char> {
                Some('z')
            }
            fn render(
                &self,
                _: &mut ratatui::Frame,
                _: ratatui::layout::Rect,
                _: &crate::state::SdrMetrics,
                _: &crate::Theme,
                _: bool,
            ) {
            }
        }
        impl ui::panel::Panel for Second {
            fn name(&self) -> &'static str {
                "second"
            }
            fn min_size(&self) -> (u16, u16) {
                (1, 1)
            }
            fn focus_key(&self) -> Option<char> {
                Some('z')
            }
            fn render(
                &self,
                _: &mut ratatui::Frame,
                _: ratatui::layout::Rect,
                _: &crate::state::SdrMetrics,
                _: &crate::Theme,
                _: bool,
            ) {
            }
        }
        let mut registry = ui::PanelRegistry::new();
        registry.register(Second);
        registry.register(First);
        let preset = |names: &[&str]| -> crate::config::PresetConfig {
            let panels: Vec<String> = names
                .iter()
                .map(|n| format!("{{ name = \"{n}\", position = \"body\" }}"))
                .collect();
            toml::from_str(&format!("panels = [{}]", panels.join(", "))).unwrap()
        };

        let apart: HashMap<String, _> = [
            ("one".to_string(), preset(&["first"])),
            ("two".to_string(), preset(&["second"])),
        ]
        .into();
        let (keys, collisions) = harvest_focus_keys(&registry, &apart);
        assert!(collisions.is_empty(), "{collisions:?}");
        assert_eq!(keys[&'z'], ["first", "second"], "in name order");

        let together: HashMap<String, _> =
            [("both".to_string(), preset(&["first", "second"]))].into();
        let (_, collisions) = harvest_focus_keys(&registry, &together);
        assert_eq!(
            collisions,
            [Collision {
                key: 'z',
                preset: "both".to_string(),
                panels: vec!["first", "second"],
            }]
        );
    }

    /// The built-in presets, which are what ships: no screen shows two panels
    /// with one letter, and the startup warnings say nothing about it.
    #[test]
    fn no_builtin_preset_shows_two_panels_with_one_focus_letter() {
        let (engine, keys) = App::build_ui("command_rail", &HashMap::new(), None, true);
        let registry_panels: usize = keys.values().map(Vec::len).sum();
        assert!(
            registry_panels >= 10,
            "expected the full focus set, got {registry_panels}"
        );
        let clash = engine
            .startup_warnings()
            .iter()
            .filter(|w| w.contains("same focus key"))
            .count();
        assert_eq!(clash, 0, "{:?}", engine.startup_warnings());
    }

    /// The preset a config asks for is the one the engine comes up in - and a
    /// name that resolves to nothing must not leave the engine on it.
    ///
    /// `build_ui` is where a saved `active_preset` meets the merged preset table,
    /// and it is the only place the two are reconciled. A config naming a preset
    /// that no longer exists (renamed built-in, deleted user file) has to land
    /// somewhere sensible rather than on an empty layout.
    #[test]
    fn the_configured_preset_is_the_one_the_engine_starts_on() {
        let (engine, _) = App::build_ui("lab_iq", &HashMap::new(), None, true);
        assert_eq!(engine.active_preset(), "lab_iq");

        let (engine, _) = App::build_ui("no_such_preset", &HashMap::new(), None, true);
        assert!(
            engine.has_preset(engine.active_preset()),
            "fell back to '{}', which is not a preset either",
            engine.active_preset()
        );
    }

    /// A preset defined only in the user's config.toml is selectable at startup.
    ///
    /// The merge itself is tested in `config`; what this adds is that the merged
    /// table is the one `build_ui` hands the engine, so a hand-written preset is
    /// bootable and not merely loadable.
    #[test]
    fn a_user_defined_preset_can_be_the_startup_layout() {
        let mut user = HashMap::new();
        user.insert(
            "my_layout".to_string(),
            crate::config::PresetConfig {
                panels: vec![crate::config::PanelSpec {
                    name: "spectrum".into(),
                    position: crate::config::Position::Body,
                    height: None,
                    width_pct: None,
                }],
                ..Default::default()
            },
        );
        let (engine, _) = App::build_ui("my_layout", &user, None, true);
        assert_eq!(engine.active_preset(), "my_layout");
        assert!(engine.is_panel_visible("spectrum"));
    }

    #[test]
    fn a_power_trace_device_keeps_only_compatible_trace_layouts() {
        let mut user = HashMap::new();
        user.insert(
            "my_trace".to_string(),
            crate::config::PresetConfig {
                panels: vec![
                    crate::config::PanelSpec {
                        name: "header_slim".into(),
                        position: crate::config::Position::Top,
                        height: None,
                        width_pct: None,
                    },
                    crate::config::PanelSpec {
                        name: "spectrum".into(),
                        position: crate::config::Position::Body,
                        height: None,
                        width_pct: None,
                    },
                    crate::config::PanelSpec {
                        name: "footer".into(),
                        position: crate::config::Position::Bottom,
                        height: None,
                        width_pct: None,
                    },
                ],
                ..Default::default()
            },
        );
        user.insert(
            "my_iq".to_string(),
            crate::config::PresetConfig {
                panels: vec![
                    crate::config::PanelSpec {
                        name: "spectrum".into(),
                        position: crate::config::Position::Body,
                        height: None,
                        width_pct: None,
                    },
                    crate::config::PanelSpec {
                        name: "iq_constellation".into(),
                        position: crate::config::Position::Right,
                        height: None,
                        width_pct: None,
                    },
                ],
                ..Default::default()
            },
        );
        user.insert(
            "my_status".to_string(),
            crate::config::PresetConfig {
                panels: vec![
                    crate::config::PanelSpec {
                        name: "system_resources".into(),
                        position: crate::config::Position::Body,
                        height: None,
                        width_pct: None,
                    },
                    crate::config::PanelSpec {
                        name: "log".into(),
                        position: crate::config::Position::Bottom,
                        height: None,
                        width_pct: None,
                    },
                ],
                ..Default::default()
            },
        );
        user.insert(
            "my_sweep".to_string(),
            crate::config::PresetConfig {
                panels: vec![
                    crate::config::PanelSpec {
                        name: "header_slim".into(),
                        position: crate::config::Position::Top,
                        height: None,
                        width_pct: None,
                    },
                    crate::config::PanelSpec {
                        name: "sweep_panel".into(),
                        position: crate::config::Position::Body,
                        height: None,
                        width_pct: None,
                    },
                    crate::config::PanelSpec {
                        name: "system_resources".into(),
                        position: crate::config::Position::Right,
                        height: None,
                        width_pct: None,
                    },
                    crate::config::PanelSpec {
                        name: "footer".into(),
                        position: crate::config::Position::Bottom,
                        height: None,
                        width_pct: None,
                    },
                ],
                ..Default::default()
            },
        );

        let (mut engine, _) = App::build_ui_for(
            "my_trace",
            &user,
            None,
            false,
            crate::hardware::AcquisitionKind::PowerTrace,
        )
        .unwrap();
        for available in [
            "spectrum",
            "waterfall",
            "spectrum_waterfall",
            "lab_sweep",
            "micro_sweep",
            "my_trace",
            "my_status",
            "my_sweep",
        ] {
            assert!(engine.has_preset(available), "{available} was hidden");
        }
        for unavailable in [
            "command_rail",
            "lab_iq",
            "lab_rf",
            "lab_timing",
            "lab_signal",
            "my_iq",
        ] {
            assert!(!engine.has_preset(unavailable), "{unavailable} survived");
        }
        assert_eq!(engine.active_preset(), "my_trace");
        engine.set_preset("my_sweep");
        assert!(engine.is_panel_visible("sweep_panel"));
        assert!(engine.is_panel_visible("system_resources"));
        engine.set_preset("lab_sweep");
        assert!(engine.is_panel_visible("sweep_panel"));
        assert!(!engine.is_panel_visible("signal_metrics"));
    }

    #[test]
    fn an_iq_device_keeps_signal_metrics_in_the_lab_sweep_layout() {
        let (engine, _) = App::build_ui_for(
            "lab_sweep",
            &HashMap::new(),
            None,
            false,
            crate::hardware::AcquisitionKind::IqSamples,
        )
        .unwrap();
        assert_eq!(engine.active_preset(), "lab_sweep");
        assert!(engine.is_panel_visible("signal_metrics"));
    }

    #[test]
    fn automatic_fallback_does_not_replace_the_saved_preference() {
        let (engine, _) = App::build_ui_for(
            "command_rail",
            &HashMap::new(),
            None,
            false,
            crate::hardware::AcquisitionKind::PowerTrace,
        )
        .unwrap();
        assert_eq!(engine.active_preset(), "spectrum_waterfall");
        assert_eq!(engine.saved_active_preset(), "command_rail");
    }

    #[test]
    fn explicitly_selecting_the_fallback_updates_the_saved_preference() {
        let (mut engine, _) = App::build_ui_for(
            "command_rail",
            &HashMap::new(),
            None,
            false,
            crate::hardware::AcquisitionKind::PowerTrace,
        )
        .unwrap();
        engine.set_preset("spectrum_waterfall");
        assert_eq!(engine.saved_active_preset(), "spectrum_waterfall");
    }

    #[test]
    fn a_saved_coexistence_preset_falls_back_like_any_unknown_one() {
        // `net_coexist` was its own preset until Stop 3.3.a2 folded it into
        // the Survey. A config saved on it names a preset that no longer
        // exists, and must land on a drawable layout and say why, the path
        // every unknown preset takes, checked rather than assumed.
        let (engine, _) = App::build_ui("net_coexist", &HashMap::new(), None, true);
        assert!(!engine.has_preset("net_coexist"));
        assert_eq!(engine.active_preset(), "spectrum_waterfall");
        assert!(engine.startup_warnings().iter().any(|w| {
            w.contains("Preset 'net_coexist' is unavailable")
                && w.contains("using 'spectrum_waterfall'")
        }));
    }

    /// **The Survey is one instrument.** The occupancy profile and the
    /// coexistence heatmap bond: one frame round both, the seam between them
    /// is the channel ruler let into the rule, and the heatmap's nameplate is
    /// on the bottom edge, since its top edge is the ruler.
    #[test]
    fn the_survey_draws_the_profile_and_the_history_as_one_instrument() {
        let (engine, _) = App::build_ui("net_survey", &HashMap::new(), None, true);
        let mut m = crate::state::SdrMetrics::fixture().streaming();
        m.net.band.cells = vec![Default::default(); crate::signal::net::occupancy::CELLS];
        m.net.band.trusted = true;
        m.net.band.history = vec![vec![0.1f32; crate::signal::net::occupancy::CELLS]; 8].into();
        let (w, h) = (140u16, 50u16);
        let backend = ratatui::backend::TestBackend::new(w, h);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        term.draw(|f| engine.draw(f, &m, &crate::Theme::sdr()))
            .unwrap();
        let buf = term.backend().buffer();
        let rows: Vec<String> = (0..h)
            .map(|y| (0..w).map(|x| buf.get(x, y).symbol()).collect())
            .collect();
        let all = rows.join("\n");
        let seam = rows
            .iter()
            // A junction on the left and the last channel number: the header
            // has junctions too, but no channel ruler.
            .find(|r| r.contains('\u{251c}') && r.contains("13"))
            .unwrap_or_else(|| panic!("no seam row:\n{all}"));
        for ch in ["1", "6", "11", "13"] {
            assert!(seam.contains(ch), "channel {ch} not on the seam: {seam}");
        }
        assert!(seam.contains('\u{2500}'), "the seam is a rule: {seam}");
        // The ruler is drawn once, on the seam, not also inside either half.
        assert_eq!(
            all.matches(" 13 ").count() + all.matches("\u{2500}13\u{2500}").count(),
            1,
            "{all}"
        );
        assert!(
            rows.iter()
                .any(|r| r.contains('\u{2570}') && r.contains("Coexistence")),
            "the heatmap's plate is not on its bottom edge:\n{all}"
        );
        assert!(
            rows.iter()
                .any(|r| r.contains('\u{256d}') && r.contains("Band Occupancy")),
            "the profile's plate is not on the top edge:\n{all}"
        );
    }

    #[test]
    fn an_unknown_only_preset_warns_and_uses_a_drawable_fallback() {
        let mut user = HashMap::new();
        user.insert(
            "future_panel".to_string(),
            crate::config::PresetConfig {
                panels: vec![crate::config::PanelSpec {
                    name: "not_registered_yet".into(),
                    position: crate::config::Position::Body,
                    height: None,
                    width_pct: None,
                }],
                ..Default::default()
            },
        );

        let (engine, _) = App::build_ui("future_panel", &user, None, true);
        assert!(engine.has_preset("future_panel"));
        assert_eq!(engine.active_preset(), "spectrum_waterfall");
        assert!(engine
            .startup_warnings()
            .iter()
            .any(|warning| warning.contains("unknown panel 'not_registered_yet'")));
        assert!(engine.startup_warnings().iter().any(|warning| {
            warning.contains("Preset 'future_panel' is unavailable")
                && warning.contains("using 'spectrum_waterfall'")
        }));
    }

    #[test]
    fn a_mixed_known_and_unknown_preset_remains_selectable() {
        let mut user = HashMap::new();
        user.insert(
            "future_panel".to_string(),
            crate::config::PresetConfig {
                panels: vec![
                    crate::config::PanelSpec {
                        name: "not_registered_yet".into(),
                        position: crate::config::Position::Body,
                        height: None,
                        width_pct: None,
                    },
                    crate::config::PanelSpec {
                        name: "spectrum".into(),
                        position: crate::config::Position::Body,
                        height: None,
                        width_pct: None,
                    },
                ],
                ..Default::default()
            },
        );

        let (engine, _) = App::build_ui("future_panel", &user, None, true);
        assert_eq!(engine.active_preset(), "future_panel");
        assert!(engine
            .startup_warnings()
            .iter()
            .any(|warning| warning.contains("unknown panel 'not_registered_yet'")));
    }

    #[test]
    fn incompatible_overrides_cannot_leave_a_power_device_without_a_layout() {
        let mut user = HashMap::new();
        for name in [
            "spectrum",
            "waterfall",
            "spectrum_waterfall",
            "lab_sweep",
            "micro_sweep",
        ] {
            user.insert(
                name.to_string(),
                crate::config::PresetConfig {
                    panels: vec![crate::config::PanelSpec {
                        name: "iq_constellation".into(),
                        position: crate::config::Position::Body,
                        height: None,
                        width_pct: None,
                    }],
                    ..Default::default()
                },
            );
        }
        user.insert(
            "future_panel".to_string(),
            crate::config::PresetConfig {
                panels: vec![crate::config::PanelSpec {
                    name: "not_registered_yet".into(),
                    position: crate::config::Position::Body,
                    height: None,
                    width_pct: None,
                }],
                ..Default::default()
            },
        );

        let error = App::build_ui_for(
            "spectrum_waterfall",
            &user,
            None,
            false,
            crate::hardware::AcquisitionKind::PowerTrace,
        )
        .err()
        .expect("all compatible layouts were overridden");
        assert!(error
            .to_string()
            .contains("No usable presets remain for this device"));
        assert!(error.to_string().contains("iq_constellation"));
        assert!(error.to_string().contains("not_registered_yet"));
    }

    /// A full-height waterfall must reach its own bottom border.
    ///
    /// The `waterfall` preset gives the panel the whole body, and each character
    /// cell shows two rows of history - so a tall terminal needs more than twice
    /// its height in buffered rows. With the old 64-row default it ran out and
    /// left a blank strip above the bottom border that never filled: the plot
    /// looked cut off short of its own frame.
    ///
    /// Rendered through the real layout engine, because the bug was in the
    /// interaction between the preset's height and the buffer's depth - neither
    /// the panel nor the buffer is wrong on its own.
    #[test]
    fn a_full_height_waterfall_fills_its_panel() {
        use crate::state::{SdrMetrics, WaterfallState, WATERFALL_MIN_ROWS};

        let mut m = SdrMetrics::fixture()
            .streaming()
            .with_carrier(1_000_000.0, 40.0);
        m.waterfall = WaterfallState::new(
            WATERFALL_MIN_ROWS,
            crate::palette::WaterfallPalette::default(),
            m.caps.level_min_db,
            m.caps.level_max_db,
        );
        for i in 0..WATERFALL_MIN_ROWS {
            let bins: Vec<f32> = (0..256)
                .map(|b| if b % 17 == i % 17 { -40.0 } else { -95.0 })
                .collect();
            m.waterfall.buffer.push(&bins);
        }
        let theme = crate::Theme::sdr();

        for h in [20u16, 30, 45, 60, 90] {
            let (engine, _) = App::build_ui("waterfall", &HashMap::new(), None, true);
            let backend = ratatui::backend::TestBackend::new(100, h);
            let mut term = ratatui::Terminal::new(backend).unwrap();
            term.draw(|f| engine.draw(f, &m, &theme)).unwrap();
            let buf = term.backend().buffer();
            let rows: Vec<String> = (0..h)
                .map(|y| (0..100).map(|x| buf.get(x, y).symbol()).collect())
                .collect();

            let Some(top) = rows.iter().position(|r| r.contains("WATERFALL")) else {
                continue; // too short to place the panel at all
            };
            let Some(bot) = rows[top + 1..]
                .iter()
                .position(|r| r.starts_with('\u{2517}'))
                .map(|i| top + 1 + i)
            else {
                continue;
            };
            let blank = rows[top + 1..bot]
                .iter()
                .filter(|r| r.chars().all(|c| c == ' ' || c == '\u{2502}'))
                .count();
            assert_eq!(
                blank,
                0,
                "height {h}: {blank} blank rows between the waterfall and its border\n{}",
                rows[top..=bot].join("\n")
            );
        }
    }

    /// The gate at the level it actually acts. A radio that cannot work in the
    /// band gets **no** NET section: not one greyed out, not one showing zeros,
    /// and not an empty heading with nothing under it.
    #[test]
    fn a_refused_radio_gets_no_net_section_at_all() {
        let (admitted, _) = App::build_ui("command_rail", &HashMap::new(), None, true);
        assert!(admitted.has_preset("net"));
        assert!(admitted.menu().section("net").is_some());

        let (refused, _) = App::build_ui("command_rail", &HashMap::new(), None, false);
        assert!(!refused.has_preset("net"));
        assert!(
            refused.menu().section("net").is_none(),
            "the section must be absent, not empty"
        );
        // Every other section is untouched: the gate removes one thing.
        for other in ["command_rail", "lab", "sweep", "micro"] {
            assert!(refused.menu().section(other).is_some(), "{other} went too");
        }
    }

    /// A config saved on a HackRF and opened on an RTL-SDR names a layout that
    /// no longer exists. It must fall back rather than start on a preset the
    /// menu cannot even show.
    #[test]
    fn a_saved_net_layout_does_not_survive_a_radio_that_cannot_run_it() {
        let (kept, _) = App::build_ui("net", &HashMap::new(), None, true);
        assert_eq!(kept.active_preset(), "net");

        let (dropped, _) = App::build_ui("net", &HashMap::new(), None, false);
        assert_ne!(dropped.active_preset(), "net");
        assert!(dropped.has_preset(dropped.active_preset()));
    }
}
