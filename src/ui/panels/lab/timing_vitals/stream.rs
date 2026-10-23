// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! What the radio delivers: sample drops and ADC saturation, each as a 60 s trend.
//!
//! Both are counted in the hot path and both mean "the stream is not clean", but
//! for different reasons - drops are the host failing to keep up, saturation is
//! the front end being over-driven. Side by side because a reader is deciding
//! which of the two it is.

use ratatui::text::{Line, Span};

use crate::state::{RadioDropAccount, SdrMetrics};
use crate::ui::widgets::micro_common::{drop_color, sat_color};

use super::rows::Rows;

pub(super) fn lines(state: &SdrMetrics, r: &Rows) -> Vec<Line<'static>> {
    let mut out = Vec::new();

    let drops = &state.signal;
    out.extend(r.trend(
        r.heading(
            "Sample drops ",
            format!("{}/s", drops.drops_per_sec),
            drop_color(drops.drops_per_sec, r.theme),
            format!("   session {}", drops.total_drops_session),
        ),
        drops.drop_history.iter().map(|&v| v as f64).collect(),
    ));
    // Whose loss: the driver reported it before the block reached sdrtop, or
    // sdrtop's own intake had no room for the block.
    if drops.total_drops_session > 0 {
        let ours = drops
            .total_drops_session
            .saturating_sub(drops.driver_drops_session);
        out.push(Line::from(vec![
            Span::raw(" "),
            Span::styled("Lost in      ", r.lbl()),
            Span::styled(
                format!(
                    "driver {} \u{00b7} sdrtop {ours}",
                    drops.driver_drops_session
                ),
                r.dim(),
            ),
        ]));
    }
    // What the radio dropped before anything reached the host: no hole in
    // the stream shows these, so the count above cannot.
    match &drops.radio_drops {
        RadioDropAccount::NotKept => {}
        RadioDropAccount::Unreadable(why) => out.push(Line::from(vec![
            Span::raw(" "),
            Span::styled("In the radio ", r.lbl()),
            Span::styled(format!("not counted: {why}"), r.dim()),
        ])),
        RadioDropAccount::Counted {
            stream,
            longest_bytes,
            ..
        } => {
            let geometry = state.caps.sample_geometry;
            let rate = state.radio.config_sample_rate.max(1.0);
            let longest_ms = *longest_bytes as f64 / geometry.bytes_per_pair() as f64 / rate * 1e3;
            let tail = if *stream > 0 {
                format!("   this stream, the longest {longest_ms:.1} ms")
            } else {
                "   this stream".to_string()
            };
            let color = if *stream > 0 {
                r.theme.status_crit
            } else {
                r.theme.status_ok
            };
            out.push(Line::from(r.heading(
                "In the radio ",
                stream.to_string(),
                color,
                tail,
            )));
        }
    }
    out.push(Line::raw(""));

    out.extend(r.trend(
        r.heading(
            "ADC saturation ",
            format!("{:.1} %", drops.adc_saturation_pct),
            sat_color(drops.adc_saturation_pct, r.theme),
            format!("   peak {:.1}%", drops.adc_saturation_peak),
        ),
        drops.saturation_history.iter().map(|&v| v as f64).collect(),
    ));
    out
}
