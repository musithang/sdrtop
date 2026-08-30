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

use crate::app::App;

impl App {
    pub(super) fn build_ui(
        active_preset: &str,
        user_presets: &HashMap<String, crate::config::PresetConfig>,
        presets_dir: Option<&std::path::Path>,
    ) -> (ui::LayoutEngine, HashMap<char, &'static str>) {
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

        let (focus_keys, collisions) = harvest_focus_keys(&registry);
        // A key claimed twice does not merely shadow. The registry is a HashMap, so
        // iteration order is randomised per process and the winner changes between
        // launches: the key then works on some runs and silently does nothing on
        // others. Loud in debug, and pinned by a test, so it can never ship quietly.
        debug_assert!(
            collisions.is_empty(),
            "focus key claimed by more than one panel: {collisions:?}",
        );

        let mut engine = ui::LayoutEngine::new(
            LayoutConfig::with_user_presets(user_presets, presets_dir),
            registry,
        );
        engine.set_preset(active_preset);
        (engine, focus_keys)
    }
}

/// The focus-key lookup, plus every key more than one panel claims.
type FocusHarvest = (HashMap<char, &'static str>, Vec<(char, Vec<&'static str>)>);

/// Collect each panel's focus key into the lookup the key handler uses, and report
/// any key more than one panel claims.
///
/// Split out so the collision is *visible*: `HashMap::insert` would silently drop
/// one of the two, which is exactly how `v` and `t` came to work only on some
/// launches. See the tests at the foot of this file.
fn harvest_focus_keys(registry: &ui::PanelRegistry) -> FocusHarvest {
    let mut claims: HashMap<char, Vec<&'static str>> = HashMap::new();
    for panel in registry.panels_iter() {
        if let Some(key) = panel.focus_key() {
            claims.entry(key).or_default().push(panel.name());
        }
    }
    let mut collisions: Vec<(char, Vec<&'static str>)> = claims
        .iter()
        .filter(|(_, v)| v.len() > 1)
        .map(|(k, v)| {
            let mut v = v.clone();
            v.sort();
            (*k, v)
        })
        .collect();
    collisions.sort();
    let keys = claims
        .into_iter()
        .map(|(k, mut v)| {
            v.sort();
            (k, v[0])
        })
        .collect();
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
        let (_engine, focus_keys) = App::build_ui("command_rail", &HashMap::new(), None);
        // The dispatch table is read as source text: the arms are `&str` matches on
        // a panel name, so nothing in the type system ties them to the registry.
        let dispatch = include_str!("../input/mod.rs");
        assert!(
            !focus_keys.is_empty(),
            "no focus keys were harvested at all"
        );
        for (key, panel) in &focus_keys {
            let arm = format!("Some(\"{panel}\")");
            assert!(
                dispatch.contains(&arm),
                "panel '{panel}' claims focus key '{key}' but handle_normal has no \
                 `{arm}` arm, so its keys fall through to the global handler",
            );
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
        let (engine, _) = App::build_ui("command_rail", &HashMap::new(), None);
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

    /// Two panels claiming one key must be *reported*, not silently resolved.
    ///
    /// `HashMap::insert` would drop one of them, which is exactly how `v` and `t`
    /// came to work only on some launches: the registry is a HashMap, iteration
    /// order is randomised per process, and the winner changed between runs. Two
    /// stand-in panels here prove the detector fires, so the assertion on the real
    /// registry below means something.
    #[test]
    fn a_duplicate_focus_key_is_reported() {
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
        registry.register(First);
        registry.register(Second);
        let (_, collisions) = harvest_focus_keys(&registry);
        assert_eq!(collisions, vec![('z', vec!["first", "second"])]);
    }

    /// The full registry, which is the one that actually ships.
    #[test]
    fn the_real_registry_has_no_focus_key_collisions() {
        // `build_ui` debug-asserts this too; the test states it as a fact rather
        // than relying on someone running a debug build.
        let (_engine, keys) = App::build_ui("command_rail", &HashMap::new(), None);
        let mut by_key: HashMap<char, usize> = HashMap::new();
        for k in keys.keys() {
            *by_key.entry(*k).or_default() += 1;
        }
        assert!(by_key.values().all(|&n| n == 1));
        assert!(
            keys.len() >= 10,
            "expected the full focus set, got {}",
            keys.len()
        );
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
        let (engine, _) = App::build_ui("lab_iq", &HashMap::new(), None);
        assert_eq!(engine.active_preset(), "lab_iq");

        let (engine, _) = App::build_ui("no_such_preset", &HashMap::new(), None);
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
        let (engine, _) = App::build_ui("my_layout", &user, None);
        assert_eq!(engine.active_preset(), "my_layout");
        assert!(engine.is_panel_visible("spectrum"));
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
        );
        for i in 0..WATERFALL_MIN_ROWS {
            let bins: Vec<f32> = (0..256)
                .map(|b| if b % 17 == i % 17 { -40.0 } else { -95.0 })
                .collect();
            m.waterfall.buffer.push(&bins);
        }
        let theme = crate::Theme::sdr();

        for h in [20u16, 30, 45, 60, 90] {
            let (engine, _) = App::build_ui("waterfall", &HashMap::new(), None);
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
}
