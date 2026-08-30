use std::collections::HashSet;

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    Frame,
};

use crate::config::{LayoutConfig, Position};
use crate::state::SdrMetrics;
use crate::ui::chrome;
use crate::ui::menu;
use crate::ui::panel::Bond;
use crate::ui::panels::core::{spectrum, waterfall};
use crate::ui::registry::PanelRegistry;

pub struct LayoutEngine {
    pub config: LayoutConfig,
    registry: PanelRegistry,
    focused_panel: Option<String>,
    hidden_panels: HashSet<String>,
    /// The sections, built once from the merged presets.
    ///
    /// The menu renders this and the number keys resolve through it, so both
    /// read one table instead of keeping two lists in step.
    ///
    /// Built in [`LayoutEngine::new`] and not rebuilt, because the preset table
    /// is fully merged before the engine is constructed (`App::build_ui` folds
    /// the built-ins, the user's preset directory and `config.toml` together
    /// first). `config` is `pub` and so could in principle gain a preset later;
    /// nothing does that outside the tests, and a preset added that way would be
    /// absent from the menu rather than break it.
    menu: menu::model::Menu,
}

impl LayoutEngine {
    pub fn new(config: LayoutConfig, registry: PanelRegistry) -> Self {
        let menu = menu::model::build(&config.presets);
        Self {
            config,
            registry,
            focused_panel: None,
            hidden_panels: HashSet::new(),
            menu,
        }
    }

    /// Anything odd found while building the menu, for the caller to log once at
    /// startup. Collected rather than logged in `model` so that module needs no
    /// mutex and stays testable as a pure function.
    pub fn menu_warnings(&self) -> &[String] {
        &self.menu.warnings
    }

    /// The section table the menu draws.
    pub fn menu(&self) -> &menu::model::Menu {
        &self.menu
    }
}

/// Where the deck is, and what a number key means there.
///
/// Its own impl block so the group reads together, and so one `allow` covers it:
/// nothing outside the tests calls these three until the number keys are scoped
/// to the active section. This is a binary crate, so `pub` does not keep an
/// uncalled item alive, and CI runs clippy with `-D warnings`. **Delete the
/// attribute in the checkpoint that scopes the digits.**
#[allow(dead_code)]
impl LayoutEngine {
    /// Which section the deck is in: the section of the active preset.
    ///
    /// **Derived, never stored.** A second copy of "where am I" is a copy that
    /// can disagree with the screen, which is the bug this whole design exists to
    /// make impossible. `None` for a preset the menu hides, which today means
    /// `observer`, and for a preset that is not in the config at all.
    pub fn scope(&self) -> Option<&menu::model::Section> {
        let (si, _) = self.menu.locate(&self.config.active_preset)?;
        self.menu.sections.get(si)
    }

    /// The preset a number key selects right now, or `None` when this scope has
    /// no such slot.
    ///
    /// A digit past the end of a section names nothing. It deliberately does not
    /// wrap: a key that quietly means "the last one" is a key you cannot trust,
    /// and the menu would then be showing something other than what the keys do.
    pub fn preset_in_scope(&self, slot: u8) -> Option<&str> {
        let section = self.scope()?;
        section
            .entries
            .iter()
            .find(|e| e.slot == Some(slot))
            .map(|e| e.preset.as_str())
    }

    /// The next layout in this section, wrapping at its end. What `[P]` uses once
    /// it stops walking every preset in the config alphabetically.
    pub fn next_in_scope(&self) -> Option<String> {
        let (si, ei) = self.menu.locate(&self.config.active_preset)?;
        let entries = &self.menu.sections[si].entries;
        Some(entries[(ei + 1) % entries.len()].preset.clone())
    }
}

impl LayoutEngine {
    pub fn set_panel_hidden(&mut self, name: &str, hidden: bool) {
        if hidden {
            self.hidden_panels.insert(name.to_string());
        } else {
            self.hidden_panels.remove(name);
        }
    }

    pub fn active_preset(&self) -> &str {
        &self.config.active_preset
    }

    /// Names of every panel the registry knows.
    ///
    /// Nothing draws this. It exists so `builder.rs` can check the built-in
    /// presets against the registry: presets are data and the registry is code,
    /// and a panel renamed on one side leaves the other quietly asking for a name
    /// that resolves to nothing.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn registered_panel_names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.registry.panels_iter().map(|p| p.name())
    }

    /// Names of all defined presets (built-in + user). Used by the footer to
    /// show only the lab presets that actually exist.
    pub fn preset_names(&self) -> Vec<String> {
        self.config.presets.keys().cloned().collect()
    }

    pub fn cycle_preset(&mut self) {
        let mut names: Vec<String> = self.config.presets.keys().cloned().collect();
        names.sort();
        let current = names
            .iter()
            .position(|n| n == &self.config.active_preset)
            .unwrap_or(0);
        self.config.active_preset = names[(current + 1) % names.len()].clone();
    }

    pub fn set_preset(&mut self, name: &str) {
        if self.config.presets.contains_key(name) {
            self.config.active_preset = name.to_string();
        }
    }

    /// Whether a preset with this name is defined. Used by the number-key
    /// handlers to distinguish "switch" from "not yet available" (the [6]–[9]
    /// and [0] slots light up automatically as their presets get defined).
    pub fn has_preset(&self, name: &str) -> bool {
        self.config.presets.contains_key(name)
    }

    pub fn focus(&mut self, name: &str) {
        self.focused_panel = Some(name.to_string());
    }

    pub fn clear_focus(&mut self) {
        self.focused_panel = None;
    }

    #[allow(dead_code)]
    pub fn is_focused(&self, name: &str) -> bool {
        self.focused_panel.as_deref() == Some(name)
    }

    pub fn focused_panel_name(&self) -> Option<&str> {
        self.focused_panel.as_deref()
    }

    /// Whether this panel is actually on screen: named by the active preset
    /// **and** not hidden.
    ///
    /// The hidden check used to be missing, so the answer was really "does the
    /// preset name it". Nothing was wrong today - `footer` is the only panel ever
    /// hidden and nothing asks about it - but `App::draw` gates the demodulator on
    /// `is_panel_visible("fm_demod")`, so the first panel hidden other than the
    /// footer would have kept its worker running behind a panel that is not drawn.
    pub fn is_panel_visible(&self, name: &str) -> bool {
        !self.hidden_panels.contains(name)
            && self.config.active_panels().iter().any(|s| s.name == name)
    }

    pub fn get_panel_bindings(&self, name: &str) -> &'static [(&'static str, &'static str)] {
        self.registry
            .get(name)
            .map(|p| p.focus_bindings())
            .unwrap_or(&[])
    }

    pub fn draw(&self, f: &mut Frame, state: &SdrMetrics, theme: &crate::Theme) {
        let specs = self.config.active_panels();
        let size = f.size();
        let focused = self.focused_panel.as_deref();

        let visible = |name: &str| !self.hidden_panels.contains(name);

        let top_specs: Vec<_> = specs
            .iter()
            .filter(|s| s.position == Position::Top && visible(&s.name))
            .collect();
        let bottom_specs: Vec<_> = specs
            .iter()
            .filter(|s| s.position == Position::Bottom && visible(&s.name))
            .collect();
        let body_specs: Vec<_> = specs
            .iter()
            .filter(|s| {
                matches!(
                    s.position,
                    Position::Left | Position::Right | Position::Body
                ) && visible(&s.name)
            })
            .collect();

        let panel_h = |s: &&crate::config::PanelSpec| -> u16 {
            s.height.unwrap_or_else(|| {
                // Call footer height directly to avoid dyn-dispatch ambiguity
                if s.name == "footer" {
                    return crate::ui::panels::core::footer::compute_footer_height(
                        size.width, state,
                    );
                }
                self.registry
                    .get(&s.name)
                    .map(|p| p.preferred_height(size.width, state))
                    .unwrap_or(3)
            })
        };

        // Compute heights once - reused for both total-height sum and per-panel Rect.
        let top_heights: Vec<u16> = top_specs.iter().map(panel_h).collect();
        let bottom_heights: Vec<u16> = bottom_specs.iter().map(panel_h).collect();
        let top_h: u16 = top_heights.iter().sum();
        let bot_h: u16 = bottom_heights.iter().sum();

        let outer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(top_h),
                Constraint::Min(0),
                Constraint::Length(bot_h),
            ])
            .split(size);

        // Top panels - stacked downward
        let mut y = outer[0].y;
        for (spec, &h) in top_specs.iter().zip(top_heights.iter()) {
            let area = Rect {
                x: outer[0].x,
                y,
                width: outer[0].width,
                height: h,
            };
            self.registry.render_panel(
                &spec.name,
                f,
                area,
                state,
                theme,
                focused == Some(spec.name.as_str()),
            );
            y += h;
        }

        // Bottom panels - stacked downward
        let mut y = outer[2].y;
        for (spec, &h) in bottom_specs.iter().zip(bottom_heights.iter()) {
            let area = Rect {
                x: outer[2].x,
                y,
                width: outer[2].width,
                height: h,
            };
            self.registry.render_panel(
                &spec.name,
                f,
                area,
                state,
                theme,
                focused == Some(spec.name.as_str()),
            );
            y += h;
        }

        // Body - split into left / center / right columns
        if !body_specs.is_empty() {
            let left_specs: Vec<_> = body_specs
                .iter()
                .filter(|s| s.position == Position::Left)
                .collect();
            let right_specs: Vec<_> = body_specs
                .iter()
                .filter(|s| s.position == Position::Right)
                .collect();
            let center_specs: Vec<_> = body_specs
                .iter()
                .filter(|s| s.position == Position::Body)
                .collect();

            // Column width is determined by the FIRST panel in each column.
            let left_pct = left_specs.first().and_then(|s| s.width_pct).unwrap_or(0);
            let right_pct = right_specs.first().and_then(|s| s.width_pct).unwrap_or(0);

            let columns = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(left_pct),
                    Constraint::Min(0),
                    Constraint::Percentage(right_pct),
                ])
                .split(outer[1]);

            render_column(
                f,
                &left_specs,
                columns[0],
                state,
                &self.registry,
                theme,
                focused,
            );
            // Bond: a center column that is exactly [spectrum, waterfall] renders as
            // one instrument - the spectrum drops its bottom border + own freq axis,
            // the waterfall's top border becomes the shared frequency ruler, and a
            // `├`/`┤` junction overlay ties the seam into the continuous side borders.
            let is_bond_pair = center_specs.len() == 2
                && center_specs[0].name == "spectrum"
                && center_specs[1].name == "waterfall";
            if is_bond_pair {
                let halves = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(0), Constraint::Min(0)])
                    .split(columns[1]);
                spectrum::render(
                    f,
                    halves[0],
                    state,
                    theme,
                    focused == Some("spectrum"),
                    Bond::Below,
                );
                waterfall::render(
                    f,
                    halves[1],
                    state,
                    theme,
                    focused == Some("waterfall"),
                    Bond::Above,
                );
                let seam = if focused == Some("spectrum") || focused == Some("waterfall") {
                    theme.border_focused
                } else {
                    theme.border_accent
                };
                chrome::junction_caps(f, halves[1], seam);
            } else {
                render_column(
                    f,
                    &center_specs,
                    columns[1],
                    state,
                    &self.registry,
                    theme,
                    focused,
                );
            }
            render_column(
                f,
                &right_specs,
                columns[2],
                state,
                &self.registry,
                theme,
                focused,
            );
        }
    }
}

fn render_column(
    f: &mut Frame,
    specs: &[&&crate::config::PanelSpec],
    area: Rect,
    state: &SdrMetrics,
    registry: &PanelRegistry,
    theme: &crate::Theme,
    focused_panel: Option<&str>,
) {
    if specs.is_empty() {
        return;
    }
    let constraints: Vec<Constraint> = specs.iter().map(|_| Constraint::Min(0)).collect();
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);
    for (spec, area) in specs.iter().zip(areas.iter()) {
        let focused = focused_panel == Some(spec.name.as_str());
        registry.render_panel(&spec.name, f, *area, state, theme, focused);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{PanelSpec, PresetConfig};
    use crate::ui::panel::Panel;
    use std::collections::HashMap;

    /// A panel that exists only to be registered, so the engine's bookkeeping can
    /// be exercised without a frame, a theme or a metrics snapshot.
    struct Stub(&'static str, &'static [(&'static str, &'static str)]);
    impl Panel for Stub {
        fn name(&self) -> &'static str {
            self.0
        }
        fn min_size(&self) -> (u16, u16) {
            (1, 1)
        }
        fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
            self.1
        }
        fn render(&self, _: &mut Frame, _: Rect, _: &SdrMetrics, _: &crate::Theme, _: bool) {}
    }

    fn spec(name: &str) -> PanelSpec {
        PanelSpec {
            name: name.into(),
            position: Position::Body,
            height: None,
            width_pct: None,
        }
    }

    /// Two presets, `alpha` and `beta`, so ordering questions have an answer that
    /// does not depend on which built-ins happen to exist.
    fn engine() -> LayoutEngine {
        let mut presets = HashMap::new();
        presets.insert(
            "alpha".to_string(),
            PresetConfig {
                panels: vec![spec("one"), spec("two")],
                ..Default::default()
            },
        );
        presets.insert(
            "beta".to_string(),
            PresetConfig {
                panels: vec![spec("two")],
                ..Default::default()
            },
        );
        let mut cfg = LayoutConfig {
            active_preset: "alpha".into(),
            presets,
        };
        cfg.active_preset = "alpha".into();

        let mut registry = PanelRegistry::new();
        registry.register(Stub("one", &[("A", "do a thing")]));
        registry.register(Stub("two", &[]));
        LayoutEngine::new(cfg, registry)
    }

    /// A preset name that resolves to nothing must not become the active layout.
    ///
    /// `active_panels()` returns an empty slice for an unknown name, so accepting
    /// one would draw a blank screen with no error anywhere - the failure mode is
    /// silence, which is why the guard is checked rather than trusted.
    #[test]
    fn set_preset_refuses_a_name_that_is_not_defined() {
        let mut e = engine();
        e.set_preset("beta");
        assert_eq!(e.active_preset(), "beta");
        e.set_preset("nope");
        assert_eq!(
            e.active_preset(),
            "beta",
            "unknown name must not take effect"
        );
        assert!(!e.has_preset("nope"));
        assert!(e.has_preset("alpha"));
    }

    /// `[P]` walks every preset and comes back round.
    ///
    /// The order is the sorted name order, not the HashMap's, which is the only
    /// thing that makes the cycle stable between launches.
    #[test]
    fn cycle_preset_walks_sorted_and_wraps() {
        let mut e = engine();
        assert_eq!(e.active_preset(), "alpha");
        e.cycle_preset();
        assert_eq!(e.active_preset(), "beta");
        e.cycle_preset();
        assert_eq!(e.active_preset(), "alpha", "cycle must wrap, not stop");
    }

    /// A user preset joins the cycle with no list to register it in.
    #[test]
    fn a_preset_added_after_construction_joins_the_cycle() {
        let mut e = engine();
        e.config.presets.insert(
            "aardvark".to_string(),
            PresetConfig {
                panels: vec![spec("one")],
                ..Default::default()
            },
        );
        // Sorted order is aardvark, alpha, beta - so from alpha the next is beta,
        // and one more wrap brings the new name round.
        e.cycle_preset();
        e.cycle_preset();
        assert_eq!(e.active_preset(), "aardvark");
        assert_eq!(e.preset_names().len(), 3);
    }

    #[test]
    fn focus_is_a_round_trip() {
        let mut e = engine();
        assert_eq!(e.focused_panel_name(), None);
        e.focus("one");
        assert!(e.is_focused("one"));
        assert!(!e.is_focused("two"));
        assert_eq!(e.focused_panel_name(), Some("one"));
        e.clear_focus();
        assert_eq!(e.focused_panel_name(), None);
        assert!(!e.is_focused("one"));
    }

    /// Visible means *on screen*: named by the active preset and not hidden.
    ///
    /// `App::draw` gates the demodulator worker on this, so a panel that is hidden
    /// but still reported visible would leave a worker running for a panel nobody
    /// can see.
    #[test]
    fn a_hidden_panel_is_not_visible() {
        let mut e = engine();
        assert!(e.is_panel_visible("one"));
        e.set_panel_hidden("one", true);
        assert!(!e.is_panel_visible("one"));
        e.set_panel_hidden("one", false);
        assert!(e.is_panel_visible("one"));
    }

    /// Not in the active preset is also not visible, hidden flag or no.
    #[test]
    fn a_panel_the_preset_does_not_name_is_not_visible() {
        let mut e = engine();
        e.set_preset("beta"); // names only "two"
        assert!(!e.is_panel_visible("one"));
        assert!(e.is_panel_visible("two"));
    }

    /// The footer renders whatever `focus_bindings()` returns, so an unknown name
    /// has to give back an empty slice rather than panic: the name comes from the
    /// focus state, which outlives a preset switch.
    #[test]
    fn bindings_for_an_unregistered_panel_are_empty() {
        let e = engine();
        assert_eq!(e.get_panel_bindings("one").len(), 1);
        assert!(e.get_panel_bindings("ghost").is_empty());
    }

    // ── Scope ────────────────────────────────────────────────────────────────
    //
    // These run on the real built-in presets rather than the stub engine above,
    // because the thing under test is precisely how those presets are filed.

    fn real_engine(active: &str) -> LayoutEngine {
        let mut cfg = LayoutConfig::default_config();
        cfg.active_preset = active.to_string();
        LayoutEngine::new(cfg, PanelRegistry::new())
    }

    /// The scope is the section of whatever preset is active. Nothing stores it,
    /// so it cannot disagree with what is on screen.
    #[test]
    fn the_scope_follows_the_active_preset() {
        let mut e = real_engine("lab_timing");
        assert_eq!(e.scope().map(|s| s.id.as_str()), Some("lab"));
        e.set_preset("spectrum");
        assert_eq!(e.scope().map(|s| s.id.as_str()), Some("command_rail"));
        e.set_preset("micro_gain");
        assert_eq!(e.scope().map(|s| s.id.as_str()), Some("micro"));
    }

    /// A number key means "the nth view of the section I am in", so the same
    /// digit resolves to a different preset in a different scope. This is the
    /// whole feature in one assertion.
    #[test]
    fn the_same_digit_means_different_things_in_different_scopes() {
        let mut e = real_engine("lab_iq");
        assert_eq!(e.preset_in_scope(2), Some("lab_rf"));
        e.set_preset("command_rail");
        assert_eq!(e.preset_in_scope(2), Some("spectrum"));
        e.set_preset("micro_main");
        assert_eq!(e.preset_in_scope(2), Some("micro_signal"));
    }

    /// The Sweep section has two entries, so 3 to 9 name nothing there. A key
    /// that names nothing must be silent rather than wrap: a digit that quietly
    /// means "the last one" is a digit you cannot trust.
    #[test]
    fn a_digit_past_the_end_of_a_section_names_nothing() {
        let e = real_engine("lab_sweep");
        assert_eq!(e.preset_in_scope(1), Some("lab_sweep"));
        assert_eq!(e.preset_in_scope(2), Some("micro_sweep"));
        assert_eq!(e.preset_in_scope(3), None);
        assert_eq!(e.preset_in_scope(9), None);
    }

    /// `[P]` stops crossing section boundaries: it walks the current section and
    /// wraps at its end.
    #[test]
    fn cycle_in_scope_wraps_inside_the_section() {
        let mut e = real_engine("lab_timing");
        assert_eq!(e.next_in_scope().as_deref(), Some("lab_signal"));
        e.set_preset("lab_signal");
        assert_eq!(e.next_in_scope().as_deref(), Some("lab_iq"), "wraps");
    }

    /// Walking a section returns to where it started, having visited every
    /// entry. A cycle that skipped one would leave a layout unreachable by `[P]`.
    #[test]
    fn cycling_a_section_visits_every_entry_once() {
        let mut e = real_engine("command_rail");
        let mut seen = vec![e.active_preset().to_string()];
        for _ in 0..4 {
            let next = e.next_in_scope().expect("a next");
            e.set_preset(&next);
            seen.push(next);
        }
        assert_eq!(
            seen,
            [
                "command_rail",
                "spectrum",
                "waterfall",
                "spectrum_waterfall",
                "main",
            ]
        );
        assert_eq!(e.next_in_scope().as_deref(), Some("command_rail"), "wraps");
    }

    /// Observer mode is hidden from the menu, so it has no scope. Asking must
    /// give `None`, never a panic: `App::new` falls back to this preset whenever
    /// the device is busy, so it is a real path rather than a hypothetical.
    #[test]
    fn a_hidden_preset_has_no_scope() {
        let e = real_engine("observer");
        assert!(e.scope().is_none());
        assert_eq!(e.preset_in_scope(1), None);
        assert_eq!(e.next_in_scope(), None);
    }

    /// And so does a preset name that is not in the config at all, which is what
    /// a config naming a layout the user has since deleted looks like.
    #[test]
    fn an_unknown_preset_has_no_scope() {
        let e = real_engine("deleted_by_a_user");
        assert!(e.scope().is_none());
        assert_eq!(e.preset_in_scope(1), None);
        assert_eq!(e.next_in_scope(), None);
    }

    /// The built-ins file themselves cleanly, so a default install logs nothing
    /// at startup. A warning here would mean two built-in presets fight over a
    /// number key.
    #[test]
    fn the_builtin_presets_produce_no_warnings() {
        let e = real_engine("command_rail");
        assert!(e.menu_warnings().is_empty(), "{:?}", e.menu_warnings());
    }
}
