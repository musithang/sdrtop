//! SAMPLE RATE: whether the clock is delivering what was asked for.
//!
//! Configured against actual, the drift in ppm, and the throughput it produces.
//! A rate that is off shifts every frequency reading on the deck, so this is the
//! zone that says whether the rest of the numbers can be trusted.

use ratatui::text::{Line, Span};

use crate::state::SdrMetrics;
use crate::ui::widgets::timing_fmt::ppm_span;

use super::rows::Rows;

pub(super) fn lines(state: &SdrMetrics, r: &Rows) -> Vec<Line<'static>> {
    let t = &state.timing;
    let theme = r.theme;
    let configured = state.radio.config_sample_rate / 1_000_000.0;
    // Until the device reports back, there is a configured rate but no measured
    // one - so the configured value is shown and the comparison is a dash.
    let no_actual = r.stale || state.radio.actual_sample_rate == 0;

    let rate = if no_actual {
        Line::from(vec![
            r.field("Rate"),
            Span::styled(format!("{configured:.3} MHz"), r.val()),
            Span::raw("  "),
            r.dash(),
        ])
    } else {
        let actual = state.radio.actual_sample_rate as f64 / 1_000_000.0;
        Line::from(vec![
            r.field("Rate"),
            Span::styled(format!("{configured:.3} \u{2192} {actual:.3} MHz"), r.val()),
        ])
    };
    let drift = if no_actual {
        Line::from(vec![r.field("SR drift"), r.dash()])
    } else {
        Line::from(vec![r.field("SR drift"), ppm_span(t.sr_delta_ppm, theme)])
    };

    vec![
        crate::ui::chrome::section("SAMPLE RATE", "clock integrity", r.iw, theme),
        rate,
        drift,
        r.row(
            "Throughput",
            vec![
                Span::styled(format!("{:.1} MB/s", t.throughput_mean_mbps), r.val()),
                Span::styled(format!("   \u{03c3} {:.2}", t.throughput_std_mbps), r.dim()),
            ],
        ),
        r.trend(
            "flow",
            &state
                .radio
                .throughput_history
                .iter()
                .map(|&v| v as f64)
                .collect::<Vec<f64>>(),
            "",
        ),
    ]
}
