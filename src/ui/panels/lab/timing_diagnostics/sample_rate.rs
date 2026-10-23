// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! SAMPLE CLOCK: whether the radio delivers the rate that was asked for.
//!
//! The delivered rate with its offset in ppm, the one clock figure on the
//! bench, taken over the long baseline; then the throughput it produces. A
//! rate that is off shifts every frequency reading on the deck, so this is
//! the zone that says whether the rest of the numbers can be trusted.

use ratatui::text::{Line, Span};

use crate::state::SdrMetrics;
use crate::ui::widgets::timing_fmt::ppm_span;

use super::rows::Rows;

pub(super) fn lines(state: &SdrMetrics, r: &Rows) -> Vec<Line<'static>> {
    let t = &state.timing;
    let theme = r.theme;
    let configured = state.radio.config_sample_rate / 1_000_000.0;
    // Until the device reports back, there is a configured rate but no
    // measured one: the configured value is shown and the offset is a dash.
    let rate = if r.stale || state.radio.actual_sample_rate == 0 {
        Line::from(vec![
            r.field("Rate"),
            r.dash(),
            Span::styled(format!("   of {configured:.3} MHz"), r.dim()),
        ])
    } else {
        let actual = state.radio.actual_sample_rate as f64 / 1_000_000.0;
        let mut ppm = ppm_span(t.sr_delta_ppm, theme);
        ppm.content = format!("   {}", ppm.content).into();
        Line::from(vec![
            r.field("Rate"),
            Span::styled(format!("{actual:.3} MHz"), r.val()),
            ppm,
            Span::styled(format!(" of {configured:.3}"), r.dim()),
        ])
    };
    vec![
        crate::ui::chrome::section("Sample clock", "rate delivered", r.iw, theme),
        rate,
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
            "  60 s",
        ),
    ]
}
