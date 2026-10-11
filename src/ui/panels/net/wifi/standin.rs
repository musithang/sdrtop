// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetWifiStandinPanel` - what a Wi-Fi view can say before its receiver
//! exists: where the radio is, whether its rate is the one the bench needs,
//! and that nothing is decoded yet.
//!
//! Three statements and no number that was not set or read off the radio:
//! a view whose receiver is still being built says so rather than drawing
//! an empty plot that would read as a quiet channel (rule 2).

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness};

pub struct NetWifiStandinPanel;

/// Width of the label column.
const LABEL: usize = 10;

fn lines(state: &SdrMetrics, theme: &crate::Theme) -> Vec<Line<'static>> {
    let label = |s: &str| Span::styled(format!(" {s:<LABEL$}"), Style::default().fg(theme.label));
    let value = Style::default().fg(theme.value);
    let mhz = state.radio.frequency as f64 / 1e6;
    let channel = match crate::signal::net::band::wifi_channel(state.radio.frequency) {
        Some(ch) => vec![
            Span::styled("\u{25cf} ", Style::default().fg(theme.net_wifi)),
            Span::styled(format!("channel {ch}"), value),
            Span::styled(
                format!(" \u{b7} {mhz:.3} MHz"),
                Style::default().fg(theme.label),
            ),
        ],
        None => vec![Span::styled(
            format!("not on a channel centre \u{b7} {mhz:.3} MHz"),
            Style::default().fg(theme.label),
        )],
    };
    let rate = match crate::signal::net::wifi::rate_refusal(state.radio.config_sample_rate) {
        None => Span::styled(
            format!(
                "{:.3} Msps, the rate this bench needs",
                state.radio.config_sample_rate / 1e6
            ),
            value,
        ),
        Some(why) => Span::styled(why, Style::default().fg(theme.status_warn)),
    };
    let mut out = vec![Line::from([vec![label("Channel")], channel].concat())];
    out.push(Line::from(vec![label("Rate"), rate]));
    out.push(Line::from(vec![
        label("Receiver"),
        Span::styled(
            "finds short-training-like repeats (feed health); nothing is decoded yet.",
            Style::default().fg(theme.label),
        ),
    ]));
    out
}

impl Panel for NetWifiStandinPanel {
    fn name(&self) -> &'static str {
        "net_wifi_standin"
    }

    fn min_size(&self) -> (u16, u16) {
        (40, 5)
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        // Where the radio is goes out of date when the samples stop, like
        // every reading in the section.
        PanelChrome::new("Wi-Fi")
            .stale_when(Staleness::NotStreaming)
            .tag_if(true, state.net.mode.tag())
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let mut text = vec![Line::raw("")];
        text.extend(lines(state, theme));
        f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
    }
}

#[cfg(test)]
mod tests {
    use crate::state::fixture::draw;
    use crate::state::SdrMetrics;

    use super::NetWifiStandinPanel;

    /// A Wi-Fi view on channel 6 at 20 Msps, streaming.
    fn on_channel_six() -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = "wifi_channel".to_string();
        m.ui.active_preset = "wifi_survey".to_string();
        m.radio.frequency = 2_437_000_000;
        m.radio.config_sample_rate = 20e6;
        m
    }

    fn text(m: &SdrMetrics) -> String {
        draw(NetWifiStandinPanel, 80, 8, m).join("\n")
    }

    /// The three statements, and nothing measured among them.
    #[test]
    fn it_says_where_it_is_that_the_rate_fits_and_that_nothing_is_decoded() {
        let out = text(&on_channel_six());
        assert!(out.contains("channel 6"), "{out}");
        assert!(out.contains("2437.000 MHz"), "{out}");
        assert!(out.contains("20.000 Msps"), "{out}");
        assert!(out.contains("short-training-like repeats"), "{out}");
        assert!(out.contains("nothing is decoded"), "{out}");
    }

    /// At another rate it says so in the rate rule's own words.
    #[test]
    fn a_wrong_rate_is_refused_in_words() {
        let mut m = on_channel_six();
        m.radio.config_sample_rate = 8e6;
        let out = text(&m);
        assert!(out.contains("needs 20.000 Msps"), "{out}");
        assert!(out.contains("8.000"), "{out}");
    }

    /// Between two channels it names none.
    #[test]
    fn off_a_centre_it_names_no_channel() {
        let mut m = on_channel_six();
        m.radio.frequency = 2_439_500_000;
        let out = text(&m);
        assert!(out.contains("not on a channel centre"), "{out}");
        assert!(!out.contains("channel 6"), "{out}");
    }
}
