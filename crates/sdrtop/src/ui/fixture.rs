// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Panels drawn from a fixture state into a fixed-size buffer and read back
//! as text, for the panels' tests: the screen's half of the state's fixture
//! (`SdrMetrics::fixture()` and its chainable methods).

use crate::state::SdrMetrics;

/// Render one panel into a `width × height` buffer and return it as text lines,
/// trailing spaces trimmed.
///
/// Goes through [`PanelRegistry::render_panel`] rather than calling
/// `Panel::render` directly, so the frame, the nameplate and the `[STALE]` tag
/// are part of what is compared - those are as much the panel's output as its
/// body is, and R3 moved them out of the panel precisely so they could not drift.
pub(crate) fn draw(
    panel: impl crate::ui::panel::Panel + 'static,
    width: u16,
    height: u16,
    state: &SdrMetrics,
) -> Vec<String> {
    draw_view(panel, width, height, &crate::ui::View::of(state))
}

/// [`draw`] with the screen's own state given too: for a test of what a
/// panel shows with a layout, a focus or a field being typed.
pub(crate) fn draw_view(
    panel: impl crate::ui::panel::Panel + 'static,
    width: u16,
    height: u16,
    view: &crate::ui::View<'_>,
) -> Vec<String> {
    let buffer = draw_buffer_view(panel, width, height, view);
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

/// [`draw`]'s buffer itself, cells and their colours, for a test that has
/// to say which ink a mark was drawn in. The sdr theme, as [`draw`] uses.
pub(crate) fn draw_buffer(
    panel: impl crate::ui::panel::Panel + 'static,
    width: u16,
    height: u16,
    state: &SdrMetrics,
) -> ratatui::buffer::Buffer {
    draw_buffer_view(panel, width, height, &crate::ui::View::of(state))
}

fn draw_buffer_view(
    panel: impl crate::ui::panel::Panel + 'static,
    width: u16,
    height: u16,
    view: &crate::ui::View<'_>,
) -> ratatui::buffer::Buffer {
    let name = panel.name();
    let mut registry = crate::ui::PanelRegistry::new();
    registry.register(panel);
    let theme = crate::Theme::sdr();

    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = ratatui::Terminal::new(backend).expect("test backend never fails");
    terminal
        .draw(|f| {
            let area = f.area();
            registry.render_panel(name, f, area, view, &theme, false);
        })
        .expect("test backend never fails");
    terminal.backend().buffer().clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `with_stale_fft` has to actually reach the rule the panels use, not merely
    /// set a number that looks old.
    #[test]
    fn with_stale_fft_reaches_the_declared_staleness_rule() {
        use crate::ui::panel::Staleness;
        let fresh = SdrMetrics::fixture().streaming().with_carrier(0.0, 40.0);
        assert!(!Staleness::FftAge.resolve(&fresh));
        assert!(Staleness::FftAge.resolve(&fresh.with_stale_fft()));
    }

    /// The harness must return something before any test believes an empty diff.
    /// (R4's verification recipe learned this the hard way, twice.)
    #[test]
    fn draw_returns_a_frame_with_the_panel_in_it() {
        let lines = draw(
            crate::ui::SignalMetricsPanel,
            60,
            10,
            &SdrMetrics::fixture().streaming().with_carrier(0.0, 40.0),
        );
        assert_eq!(lines.len(), 10, "one string per row");
        let joined = lines.join("\n");
        assert!(
            joined.contains("Signal") || joined.contains("SIGNAL"),
            "the nameplate should be in the buffer:\n{joined}"
        );
        assert!(
            joined.chars().any(|c| c.is_ascii_digit()),
            "a rendered panel with a carrier should show some number:\n{joined}"
        );
    }

    /// The point of the harness: the same state renders the same buffer, so a
    /// difference in a split is a real difference and not frame-to-frame noise.
    #[test]
    fn the_same_state_renders_identically_twice() {
        let m = SdrMetrics::fixture().streaming().with_carrier(0.0, 40.0);
        assert_eq!(
            draw(crate::ui::SignalMetricsPanel, 60, 10, &m),
            draw(crate::ui::SignalMetricsPanel, 60, 10, &m)
        );
    }
}
