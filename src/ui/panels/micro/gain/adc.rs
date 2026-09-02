// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! What the gain is actually doing to the converter: how much of its range is in
//! use, and how often it is being driven past the end of it.
//!
//! The two are a pair. Utilisation alone would say "more gain"; saturation alone
//! would say "less". Read together they bracket the right answer, which is what
//! [`super::advisor`] turns into a sentence.

use ratatui::{
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::rf_calc::adc_utilisation_ratio;
use crate::ui::widgets::charts::draw_hbar;
use crate::ui::widgets::micro_common::sat_color;

use super::super::field::Field;
use super::READOUT_W;

/// How much of the converter's range should be in use before the level is
/// comfortable. Below the lower figure the front end is being wasted; the bar is
/// green above the upper one.
const UTIL_GOOD: f64 = 0.5;
const UTIL_LOW: f64 = 0.2;

pub(super) fn draw(f: &mut Frame, util: Rect, sat: Rect, state: &SdrMetrics, fd: &Field) {
    let theme = fd.theme;

    if fd.stale {
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw(" "),
                fd.padded("ADC util", READOUT_W),
                fd.dash(),
            ])),
            util,
        );
    } else {
        let ratio = adc_utilisation_ratio(&state.iq.iq_amplitude_hist);
        let color = if ratio > UTIL_GOOD {
            theme.status_ok
        } else if ratio > UTIL_LOW {
            theme.status_warn
        } else {
            theme.status_crit
        };
        draw_hbar(
            f,
            util,
            ratio,
            &format!(" {:<READOUT_W$}", "ADC util"),
            &format!("{:.0}%", ratio * 100.0),
            color,
            theme,
        );
    }

    let pct = state.signal.adc_saturation_pct;
    let value = if fd.stale {
        fd.dash()
    } else {
        Span::styled(
            format!("{pct:.1}%"),
            ratatui::style::Style::default().fg(sat_color(pct, theme)),
        )
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" "),
            fd.padded("SAT", READOUT_W),
            value,
        ])),
        sat,
    );
}
