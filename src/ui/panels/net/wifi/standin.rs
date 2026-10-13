// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetWifiStandinPanel` - what a Wi-Fi view can say before its receiver
//! is finished: where the radio is, whether its rate is the one the bench
//! needs, the latest frame of this channel it synchronised to, its format
//! and what its signalling said, and that nothing past that is decoded
//! yet.
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

use crate::signal::wifi::format::Format;
use crate::signal::wifi::htsig::{HtSig, SigFault};
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
    // What its signalling said. The L-SIG as the L-SIG's: a later format's
    // names 6 Mb/s whatever its own rate, and its airtime is the frame's
    // rounded up to the 4 us above; a non-HT frame's is exact.
    if let Some(f) = &state.net.wifi.last_frame {
        let quiet = Style::default().fg(theme.label);
        let warn = Style::default().fg(theme.status_warn);
        let format = f.heard.format;
        out.push(Line::from(match format {
            None => vec![
                label("Format"),
                Span::styled("not read: its L-SIG was refused", quiet),
            ],
            Some(format) => {
                let mut spans = vec![label("Format")];
                spans.extend(format_spans(format, value, quiet, warn));
                spans
            }
        }));
        let greenfield = matches!(format, Some(Format::Greenfield(Ok(_))));
        out.push(Line::from(match f.heard.lsig {
            None if greenfield => vec![label("L-SIG"), Span::styled("none in greenfield", quiet)],
            None => vec![label("L-SIG"), Span::styled("none read", quiet)],
            Some(Ok(l)) => vec![
                label("L-SIG"),
                Span::styled(
                    format!("{} Mb/s \u{b7} {} octets", l.rate.mbps(), l.length),
                    value,
                ),
                Span::styled(
                    if format == Some(Format::NonHt) {
                        format!(" \u{b7} {} \u{b5}s on the air", l.airtime_us())
                    } else {
                        format!(
                            " \u{b7} {} \u{b5}s on the air, to the 4 \u{b5}s above",
                            l.airtime_us()
                        )
                    },
                    quiet,
                ),
            ],
            Some(Err(why)) => vec![
                label("L-SIG"),
                Span::styled(format!("refused: {}", why.label()), warn),
            ],
        }));
    }
    out.push(Line::from(vec![
        label("Receiver"),
        Span::styled(
            "reads each frame's signalling; nothing past it is decoded yet.",
            Style::default().fg(theme.label),
        ),
    ]));
    out
}

/// A format named as its signalling said: HT's MCS and length from a good
/// HT-SIG, and greenfield's airtime from it; a refused SIG by its fault;
/// HE and EHT named only, their fields read elsewhere.
fn format_spans(format: Format, value: Style, quiet: Style, warn: Style) -> Vec<Span<'static>> {
    let ht = |name: &str, sig: Result<HtSig, SigFault>, airtime: bool| match sig {
        Ok(h) => {
            let mut spans = vec![Span::styled(
                format!("{name} \u{b7} MCS {} \u{b7} {} octets", h.mcs, h.length),
                value,
            )];
            if airtime {
                spans.push(match h.greenfield_airtime_ns() {
                    Ok(ns) => Span::styled(
                        format!(" \u{b7} {} \u{b5}s on the air", ns as f64 / 1e3),
                        quiet,
                    ),
                    Err(why) => Span::styled(format!(" \u{b7} airtime not computed: {why}"), warn),
                });
            }
            spans
        }
        Err(fault) => vec![
            Span::styled(format!("{name} \u{b7} "), value),
            Span::styled(format!("HT-SIG refused: {}", fault.label()), warn),
        ],
    };
    match format {
        Format::NonHt => vec![Span::styled("non-HT (802.11a/g)", value)],
        Format::HtMixed(sig) => ht("HT-mixed", sig, false),
        Format::Greenfield(Ok(sig)) => ht("HT-greenfield", Ok(sig), true),
        // Only the CRC says a greenfield frame was there; a rotation alone
        // noise can tip.
        Format::Greenfield(Err(fault)) => vec![
            Span::styled("unreadable \u{b7} QBPSK first, ", value),
            Span::styled(format!("HT-SIG refused: {}", fault.label()), warn),
        ],
        Format::Vht(Ok(())) => vec![Span::styled(
            "VHT (a 5 GHz format) \u{b7} VHT-SIG-A CRC good",
            value,
        )],
        Format::Vht(Err(fault)) => vec![
            Span::styled("VHT (a 5 GHz format) \u{b7} ", value),
            Span::styled(format!("VHT-SIG-A refused: {}", fault.label()), warn),
        ],
        Format::He { remainder: 1 } => vec![Span::styled("HE, SU or TB", value)],
        Format::He { .. } => vec![Span::styled("HE, MU or ER SU", value)],
        Format::EhtOrLater => vec![Span::styled("EHT or a later PHY", value)],
    }
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
    use crate::signal::wifi::format::{Format, Heard};
    use crate::signal::wifi::htsig::{HtSig, SigFault};
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
        draw(NetWifiStandinPanel, 80, 10, m).join("\n")
    }

    /// A frame of this channel that said `heard`.
    fn frame(heard: Heard) -> crate::state::WifiFrame {
        crate::state::WifiFrame {
            offset_hz: crate::signal::dsp::uncertainty::Uncertain::from_sigma(-12_430.0, 1_900.0),
            snr_db: 14.2,
            carrier_hz: 2_437_000_000.0,
            heard,
            when: std::time::Instant::now(),
        }
    }

    fn non_ht(rate: Rate, length: u16) -> Heard {
        Heard {
            lsig: Some(Ok(LSig { rate, length })),
            format: Some(Format::NonHt),
        }
    }

    fn ht() -> HtSig {
        HtSig {
            mcs: 7,
            cbw40: false,
            length: 1500,
            stbc: 0,
            ldpc: false,
            short_gi: true,
            ness: 0,
        }
    }

    /// The row whose label is `label`.
    fn row(out: &str, label: &str) -> String {
        out.lines()
            .find(|l| {
                l.trim_start_matches('\u{2502}')
                    .trim_start()
                    .starts_with(label)
            })
            .unwrap_or_else(|| panic!("no {label} row in\n{out}"))
            .to_string()
    }

    /// The three statements, and nothing measured among them.
    #[test]
    fn it_says_where_it_is_that_the_rate_fits_and_that_nothing_is_decoded() {
        let out = text(&on_channel_six());
        assert!(out.contains("channel 6"), "{out}");
        assert!(out.contains("2437.000 MHz"), "{out}");
        assert!(out.contains("20.000 Msps"), "{out}");
        assert!(out.contains("reads each frame's signalling"), "{out}");
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
        m.net.wifi.last_frame = Some(frame(non_ht(Rate::R24, 1_500)));
        let r = row(&text(&m), "Last frame");
        assert!(r.contains("kHz") && r.contains("ppm"), "{r}");
        assert!(r.contains("-12"), "{r}");
        assert!(r.contains("14 dB"), "{r}");
    }

    /// The latest frame's L-SIG, said as the L-SIG's: its rate and length
    /// and the airtime they give (1500 octets at 24 Mb/s: 126 symbols, 524
    /// us), exact for a non-HT frame; a refused one says which check
    /// refused it and names no format.
    #[test]
    fn the_latest_frames_l_sig_is_shown_as_the_l_sigs() {
        let mut m = on_channel_six();
        m.net.wifi.last_frame = Some(frame(non_ht(Rate::R24, 1_500)));
        let out = text(&m);
        let r = row(&out, "L-SIG");
        assert!(r.contains("24 Mb/s"), "{r}");
        assert!(r.contains("1500 octets"), "{r}");
        assert!(r.contains("524 \u{b5}s on the air"), "{r}");
        assert!(row(&out, "Format").contains("non-HT"), "{out}");

        m.net.wifi.last_frame = Some(frame(Heard {
            lsig: Some(Err(Refused::Parity)),
            format: None,
        }));
        let out = text(&m);
        let r = row(&out, "L-SIG");
        assert!(r.contains("refused") && r.contains("parity"), "{r}");
        assert!(!r.contains("Mb/s"), "{r}");
        assert!(row(&out, "Format").contains("not read"), "{out}");
    }

    /// Each format named as its signalling said, and nothing more: HT's
    /// MCS and length from its HT-SIG, a refused HT-SIG by its fault, VHT
    /// as the 5 GHz format, HE and EHT named only. A later format's L-SIG
    /// airtime is said to be rounded to the 4 us above.
    #[test]
    fn each_format_is_named_as_its_signalling_said() {
        let six = Some(Ok(LSig {
            rate: Rate::R6,
            length: 2_001,
        }));
        let cases = [
            (
                Format::HtMixed(Ok(ht())),
                "HT-mixed \u{b7} MCS 7 \u{b7} 1500 octets",
            ),
            (
                Format::HtMixed(Err(SigFault::Crc)),
                "HT-mixed \u{b7} HT-SIG refused: CRC",
            ),
            (
                Format::Vht(Ok(())),
                "VHT (a 5 GHz format) \u{b7} VHT-SIG-A CRC good",
            ),
            (Format::He { remainder: 1 }, "HE, SU or TB"),
            (Format::He { remainder: 2 }, "HE, MU or ER SU"),
            (Format::EhtOrLater, "EHT or a later PHY"),
        ];
        for (format, want) in cases {
            let mut m = on_channel_six();
            m.net.wifi.last_frame = Some(frame(Heard {
                lsig: six,
                format: Some(format),
            }));
            let out = text(&m);
            assert!(row(&out, "Format").contains(want), "{format:?}:\n{out}");
            let l = row(&out, "L-SIG");
            assert!(
                l.contains("6 Mb/s") && l.contains("to the 4 \u{b5}s above"),
                "{l}"
            );
        }
    }

    /// Greenfield has no L-SIG: the row says so, and the airtime is its
    /// HT-SIG's (MCS 7, 1500 octets, short guard: 193.2 us).
    #[test]
    fn greenfield_takes_its_airtime_from_its_ht_sig() {
        let mut m = on_channel_six();
        m.net.wifi.last_frame = Some(frame(Heard {
            lsig: None,
            format: Some(Format::Greenfield(Ok(ht()))),
        }));
        let out = text(&m);
        let f = row(&out, "Format");
        assert!(
            f.contains("HT-greenfield \u{b7} MCS 7 \u{b7} 1500 octets"),
            "{f}"
        );
        assert!(f.contains("193.2 \u{b5}s on the air"), "{f}");
        assert!(row(&out, "L-SIG").contains("none in greenfield"), "{out}");

        // A QBPSK first symbol whose HT-SIG failed is not called greenfield:
        // noise tips a rotation, and only the CRC says one was there.
        m.net.wifi.last_frame = Some(frame(Heard {
            lsig: None,
            format: Some(Format::Greenfield(Err(SigFault::Crc))),
        }));
        let out = text(&m);
        let f = row(&out, "Format");
        assert!(f.contains("unreadable"), "{f}");
        assert!(f.contains("HT-SIG refused: CRC"), "{f}");
        assert!(!f.contains("HT-greenfield"), "{f}");
        assert!(row(&out, "L-SIG").contains("none read"), "{out}");
    }

    /// While it shows an offset its frame says what that is worth; with no
    /// frame yet there is no offset to tag.
    #[test]
    fn it_declares_offsets_while_it_shows_one() {
        use crate::ui::panel::Panel;
        let mut m = on_channel_six();
        assert!(!NetWifiStandinPanel.chrome(&m).offsets);
        m.net.wifi.last_frame = Some(frame(non_ht(Rate::R6, 14)));
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
