use std::collections::HashSet;

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    Frame,
};

use crate::config::{LayoutConfig, Position};
use crate::state::SdrMetrics;
use crate::ui::chrome;
use crate::ui::panel::Bond;
use crate::ui::panels::core::{spectrum, waterfall};
use crate::ui::registry::PanelRegistry;

pub struct LayoutEngine {
    pub config: LayoutConfig,
    registry: PanelRegistry,
    focused_panel: Option<String>,
    hidden_panels: HashSet<String>,
}

impl LayoutEngine {
    pub fn new(config: LayoutConfig, registry: PanelRegistry) -> Self {
        Self {
            config,
            registry,
            focused_panel: None,
            hidden_panels: HashSet::new(),
        }
    }

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

    pub fn is_panel_visible(&self, name: &str) -> bool {
        self.config.active_panels().iter().any(|s| s.name == name)
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

        // Compute heights once — reused for both total-height sum and per-panel Rect.
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

        // Top panels — stacked downward
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

        // Bottom panels — stacked downward
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

        // Body — split into left / center / right columns
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
            // one instrument — the spectrum drops its bottom border + own freq axis,
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
