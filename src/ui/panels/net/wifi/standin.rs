// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetWifiStandinPanel` - what a Wi-Fi view can say before its receiver
//! is finished: where the radio is, whether its rate is the one the bench
//! needs, the latest frame of this channel it synchronised to and what its
//! L-SIG said, and that nothing past the L-SIG is decoded yet.
//!
//! The one measurement on it is that frame's offset, with its error bar
//! and dated, never a number without them: a view whose receiver is still
//! being built says what it has and no more, rather than drawing an empty
//! plot that would read as a quiet channel (rule 2).

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness};
use crate::ui::widgets::reading::Reading;

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
    let now = std::time::Instant::now();
    out.push(Line::from(match &state.net.wifi.last_frame {
        Some(f) => {
            let t = state
                .radio
                .transmitter_offset(f.offset_hz, f.carrier_hz, now);
            let ago = now.saturating_duration_since(f.when).as_secs();
            vec![
                label("Last frame"),
                Span::styled(
                    format!(
                        "{} · {}",
                        Reading::new(t.khz, "kHz", f64::INFINITY).text(),
                        Reading::new(t.ppm, "ppm", f64::INFINITY).text()
                    ),
                    value,
                ),
                Span::styled(
                    format!(" · SNR {:.0} dB · {ago} s ago", f.snr_db),
                    Style::default().fg(theme.label),
                ),
            ]
        }
        None => vec![
            label("Last frame"),
            Span::styled(
                "no frame of this channel yet",
                Style::default().fg(theme.label),
            ),
        ],
    }));
    // What its L-SIG said, as the L-SIG's: a later format's L-SIG names
    // 6 Mb/s whatever its own rate, while the airtime holds for every one.
    if let Some(f) = &state.net.wifi.last_frame {
        out.push(Line::from(match f.lsig {
            Ok(l) => vec![
                label("L-SIG"),
                Span::styled(
                    format!("{} Mb/s \u{b7} {} octets", l.rate.mbps(), l.length),
                    value,
                ),
                Span::styled(
                    format!(" \u{b7} {} \u{b5}s on the air", l.airtime_us()),
                    Style::default().fg(theme.label),
                ),
            ],
            Err(why) => vec![
                label("L-SIG"),
                Span::styled(
                    format!("refused: {}", why.label()),
                    Style::default().fg(theme.status_warn),
                ),
            ],
        }));
    }
    out.push(Line::from(vec![
        label("Receiver"),
        Span::styled(
            "reads each frame's L-SIG; nothing past it is decoded yet.",
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
        // Its offset's worth is tagged only while one is on screen: a tag
        // about a number that is not there would be about nothing.
        let chrome = PanelChrome::new("Wi-Fi")
            .stale_when(Staleness::NotStreaming)
            .tag_if(true, state.net.mode.tag());
        if state.net.wifi.last_frame.is_some() {
            chrome.shows_offsets()
        } else {
            chrome
        }
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
    use crate::signal::wifi::lsig::{LSig, Refused};
    use crate::signal::wifi::signal_field::Rate;

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
        assert!(out.contains("reads each frame's L-SIG"), "{out}");
        assert!(out.contains("nothing past it is decoded"), "{out}");
    }

    /// The latest frame of this channel, its offset in kHz and ppm with
    /// their uncertainties and its SNR; before any, it says none yet.
    #[test]
    fn the_latest_frame_is_shown_with_its_error_bar() {
        let m = on_channel_six();
        let out = text(&m);
        assert!(out.contains("no frame of this channel yet"), "{out}");

        let mut m = on_channel_six();
        m.net.wifi.last_frame = Some(crate::state::WifiFrame {
            offset_hz: crate::signal::dsp::uncertainty::Uncertain::from_sigma(-12_430.0, 1_900.0),
            snr_db: 14.2,
            carrier_hz: 2_437_000_000.0,
            lsig: Ok(LSig {
                rate: Rate::R24,
                length: 1_500,
            }),
            when: std::time::Instant::now(),
        });
        let out = text(&m);
        let row = out.lines().find(|l| l.contains("Last frame")).expect("row");
        assert!(row.contains("kHz") && row.contains("ppm"), "{row}");
        assert!(row.contains("-12"), "{row}");
        assert!(row.contains("14 dB"), "{row}");
    }

    /// The latest frame's L-SIG, said as the L-SIG's: its rate and length
    /// and the airtime they give (1500 octets at 24 Mb/s: 126 symbols, 524
    /// us); a refused one says which check refused it.
    #[test]
    fn the_latest_frames_l_sig_is_shown_as_the_l_sigs() {
        let mut m = on_channel_six();
        let frame = |lsig| crate::state::WifiFrame {
            offset_hz: crate::signal::dsp::uncertainty::Uncertain::from_sigma(0.0, 1_000.0),
            snr_db: 20.0,
            carrier_hz: 2_437_000_000.0,
            lsig,
            when: std::time::Instant::now(),
        };
        m.net.wifi.last_frame = Some(frame(Ok(LSig {
            rate: Rate::R24,
            length: 1_500,
        })));
        let out = text(&m);
        let row = out.lines().find(|l| l.contains("L-SIG")).expect("row");
        assert!(row.contains("24 Mb/s"), "{row}");
        assert!(row.contains("1500 octets"), "{row}");
        assert!(row.contains("524 \u{b5}s"), "{row}");

        m.net.wifi.last_frame = Some(frame(Err(Refused::Parity)));
        let out = text(&m);
        let row = out.lines().find(|l| l.contains("L-SIG")).expect("row");
        assert!(row.contains("refused") && row.contains("parity"), "{row}");
        assert!(!row.contains("Mb/s"), "{row}");
    }

    /// While it shows an offset its frame says what that is worth; with no
    /// frame yet there is no offset to tag.
    #[test]
    fn it_declares_offsets_while_it_shows_one() {
        use crate::ui::panel::Panel;
        let mut m = on_channel_six();
        assert!(!NetWifiStandinPanel.chrome(&m).offsets);
        m.net.wifi.last_frame = Some(crate::state::WifiFrame {
            offset_hz: crate::signal::dsp::uncertainty::Uncertain::from_sigma(0.0, 1.0),
            snr_db: 10.0,
            carrier_hz: 2_437_000_000.0,
            lsig: Err(Refused::Tail),
            when: std::time::Instant::now(),
        });
        assert!(NetWifiStandinPanel.chrome(&m).offsets);
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
