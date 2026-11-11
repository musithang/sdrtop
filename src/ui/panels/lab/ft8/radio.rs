// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The radio under the decoder, as far as FT8 can be told about it: where
//! the level sits in the samples, what else is in the capture, the
//! frequency reference, and the timing.
//!
//! - **Placement**: the peak against the radio's stated ceiling, in the RF
//!   bench's own figures ([`SdrMetrics::shown_headroom_db`],
//!   [`SdrMetrics::ceiling_text`]), and how many of the hour's cycles
//!   reached it, coloured by the bench's own staging verdict. On a radio
//!   whose gain stages are digital, after the converter, the panel says
//!   they place the level and do not set the SNR.
//! - **The slice**: the strongest signal in the whole capture outside the
//!   FT8 band, from the FFT worker's latest frame. It is what pushes a
//!   radio's own gain control, and the FT8 channel never sees it. The panel
//!   reads the spectrum so the worker runs while it is on screen; its frame
//!   ages as the spectrum's does.
//! - **The reference**: the correction in force and its provenance, or
//!   none; what a ppm is at this dial; and that no FT8 measurement can check
//!   it, since every station picks its own tone.
//! - **Timing**: the timing bench's own figures (one witness), and the
//!   deliveries' lateness the FT8 clock saw.
//!
//! Each section opens with a dot: filled where there is something to say
//! (the placement's in the bench's own verdict colour, a reference in force
//! in the theme's "good"), hollow where there is nothing yet. The figures are
//! bright and the words around them plain, so the numbers read first.

use ratatui::{layout::Rect, style::Style, Frame};

use super::say;
use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness};
use crate::ui::panels::radio::ft8::{put, signed};
use crate::ui::rf_calc::staging_verdict_for;

pub struct Ft8RadioPanel;

/// The FT8 band above the dial, Hz, kept out of "the slice": its own
/// signals are not what the rest of the capture holds.
const FT8_BAND_HZ: (f64, f64) = (-200.0, 3_200.0);

/// The strongest bin outside the FT8 band in the latest frame: its offset
/// from the dial, Hz, and its level, dBFS.
pub fn strongest(m: &SdrMetrics) -> Option<(f64, f32)> {
    let frame = m.waterfall.last_fft.as_ref()?;
    let n = frame.bins_dbfs.len();
    if n == 0 {
        return None;
    }
    let bin = frame.sample_rate / n as f64;
    let dial = m.radio.frequency as f64;
    frame
        .bins_dbfs
        .iter()
        .enumerate()
        .map(|(k, &db)| (frame.axis_start_hz + (k as f64 + 0.5) * bin - dial, db))
        .filter(|(hz, _)| !(FT8_BAND_HZ.0..FT8_BAND_HZ.1).contains(hz))
        .max_by(|a, b| a.1.total_cmp(&b.1))
}

impl Panel for Ft8RadioPanel {
    fn name(&self) -> &'static str {
        "ft8_radio"
    }

    /// The slice row reads the FFT worker's frame.
    fn reads_spectrum(&self) -> bool {
        true
    }

    fn min_size(&self) -> (u16, u16) {
        (40, 6)
    }

    /// Two placement rows, the digital note, the slice, two of the reference
    /// and two of timing.
    fn column_height(&self, _column: u16) -> Option<u16> {
        Some(10)
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("The radio").stale_when(Staleness::FftAge)
    }

    fn render(
        &self,
        f: &mut Frame,
        area: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        let buf = f.buffer_mut();
        if area.height < 3 {
            say(buf, area, "too short", theme);
            return;
        }
        let label = Style::default().fg(theme.label);
        let value = Style::default().fg(theme.value);
        let hi = Style::default().fg(theme.value_hi);
        let dim = Style::default().fg(theme.border_dim);
        let ok = Style::default().fg(theme.status_ok);
        let x = area.x + 1;
        let xv = x + 13;
        let mut y = area.y;
        // A row: a dot saying how the section stands (filled when there is
        // something to say, hollow when there is not), its name, and the
        // text in pieces so the figures can stand out from the words.
        let mut row =
            |dot: Option<(&str, Style)>, name: &str, parts: &[(String, Style)], y: &mut u16| {
                if *y < area.bottom() {
                    if let Some((d, st)) = dot {
                        put(buf, area, x, *y, d, st);
                    }
                    if !name.is_empty() {
                        put(buf, area, x + 2, *y, name, label);
                    }
                    let mut xx = xv;
                    for (t, st) in parts {
                        xx = put(buf, area, xx, *y, t, *st);
                    }
                }
                *y += 1;
            };

        // Placement, in the RF bench's words and colours.
        let held = state.ft8.cycle_hour.iter().filter(|c| c.held).count();
        let hour = state.ft8.cycle_hour.len();
        let part = state
            .ceiling()
            .map(|c| format!(" ({})", c.part))
            .unwrap_or_default();
        let headroom = state.shown_headroom_db();
        let (_, sev) = staging_verdict_for(
            f64::from(state.peak_to_ceiling_db()),
            state.ceiling().map(|c| c.kind),
        );
        let ink = Style::default().fg(match sev {
            2 => theme.status_crit,
            1 => theme.status_warn,
            _ => theme.status_ok,
        });
        row(
            Some(("●", ink)),
            "placement",
            &[
                ("peak ".into(), label),
                (format!("{:.0} dBFS", state.signal.adc_peak_dbfs), hi),
                (" · ceiling ".into(), label),
                (format!("{} dBFS", state.ceiling_text()), hi),
                (part, dim),
            ],
            &mut y,
        );
        let side = if headroom >= 0.0 { "under" } else { "over" };
        row(
            None,
            "",
            &[
                (format!("{:.0} dB {side} it", headroom.abs()), ink),
                (" · ".into(), label),
                (
                    format!("{held}"),
                    if held > 0 {
                        Style::default().fg(theme.status_warn)
                    } else {
                        hi
                    },
                ),
                (format!(" of {hour} cycles this hour reached it"), label),
            ],
            &mut y,
        );
        if state.caps.gain.stages_are_digital() {
            row(
                None,
                "",
                &[("the gain stages place the level, not the SNR".into(), dim)],
                &mut y,
            );
        }

        // The slice.
        match strongest(state) {
            Some((hz, db)) => row(
                Some(("●", hi)),
                "the slice",
                &[
                    ("strongest outside FT8 ".into(), label),
                    (format!("{} kHz", signed(hz / 1000.0, 1)), hi),
                    (", ".into(), label),
                    (format!("{db:.0} dBFS"), hi),
                ],
                &mut y,
            ),
            None => row(
                Some(("○", dim)),
                "the slice",
                &[("no spectrum frame yet".into(), dim)],
                &mut y,
            ),
        }

        // The reference.
        let per_ppm = state.radio.frequency as f64 * 1e-6;
        match &state.radio.reference {
            Some(r) => row(
                Some(("●", ok)),
                "reference",
                &[
                    (format!("{} ppm", signed(r.ppm, 2)), hi),
                    (format!(" ± {:.2}, by ", r.sigma_ppm), label),
                    (r.source.to_string(), value),
                ],
                &mut y,
            ),
            None => row(
                Some(("○", dim)),
                "reference",
                &[("none established".into(), value)],
                &mut y,
            ),
        }
        row(
            None,
            "",
            &[(
                format!("1 ppm is {per_ppm:.1} Hz here; no FT8 decode can check it"),
                dim,
            )],
            &mut y,
        );

        // Timing: the timing bench's figures, and the FT8 clock's lateness.
        let t = &state.timing;
        let mut parts: Vec<(String, Style)> = if t.sr_known {
            vec![
                ("sample clock ".into(), label),
                (format!("{} ppm", signed(t.sr_delta_ppm as f64, 0)), hi),
            ]
        } else {
            vec![("sample clock not known yet".into(), dim)]
        };
        parts.push((" · jitter ".into(), label));
        parts.push((format!("{} us", t.cb_jitter_us), hi));
        let lateness = state.ft8.cycles.back().and_then(|c| c.lateness);
        row(
            Some(if t.sr_known {
                ("●", ok)
            } else {
                ("○", dim)
            }),
            "timing",
            &parts,
            &mut y,
        );
        let late: Vec<(String, Style)> = match lateness {
            Some(l) => vec![
                ("deliveries late ".into(), label),
                (format!("{:.1} ms", l.median_ms), hi),
                (" median, ".into(), label),
                (format!("{:.1}", l.p90_ms), hi),
                (" p90".into(), label),
            ],
            None => vec![("lateness not known yet".into(), dim)],
        };
        row(None, "", &late, &mut y);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::fixture::draw;

    #[test]
    fn the_slice_leaves_out_the_ft8_band() {
        let mut m = SdrMetrics::fixture()
            .streaming()
            .with_carrier(1_000.0, 40.0);
        // The fixture's carrier sits 1 kHz above the centre: inside the FT8
        // band when the dial is the centre.
        m.radio.frequency = m.waterfall.last_fft.as_ref().unwrap().center_freq_hz;
        if let Some((hz, _)) = strongest(&m) {
            assert!(!(-200.0..3_200.0).contains(&hz), "{hz}");
        }
    }

    #[test]
    fn it_names_the_ceiling_the_reference_and_what_ft8_cannot_check() {
        let m = SdrMetrics::fixture().airspyhf().streaming().ft8_hour();
        let all = draw(Ft8RadioPanel, 86, 12, &m).join("\n");
        assert!(
            all.contains(&format!("ceiling {} dBFS", m.ceiling_text())),
            "{all}"
        );
        assert!(all.contains("none established"), "{all}");
        assert!(all.contains("no FT8 decode can check it"), "{all}");
        assert!(all.contains("place the level, not the SNR"), "{all}");
        assert!(all.contains("cycles this hour reached it"), "{all}");
    }

    /// A radio with no part ahead of the converter is measured against the
    /// converter's own 0 dBFS, and without an FFT frame the slice says so
    /// rather than guessing.
    #[test]
    fn on_an_rtl_sdr_the_ceiling_is_the_converter_and_the_slice_waits() {
        let m = SdrMetrics::fixture().rtlsdr().streaming();
        let all = draw(Ft8RadioPanel, 86, 12, &m).join("\n");
        assert!(all.contains("ceiling 0 dBFS"), "{all}");
        assert!(all.contains("no spectrum frame yet"), "{all}");
        assert!(all.contains("none established"), "{all}");
    }

    /// A radio whose gain is ahead of the converter is not told its stages
    /// only place the level.
    #[test]
    fn an_analogue_chain_is_not_called_digital() {
        let m = SdrMetrics::fixture().streaming();
        let all = draw(Ft8RadioPanel, 86, 12, &m).join("\n");
        assert!(!all.contains("not the SNR"), "{all}");
    }

    /// Each section opens with a dot: filled where there is something to
    /// say, hollow where there is not; a reference in force is a filled dot
    /// in the theme's "good".
    #[test]
    fn each_section_opens_with_a_dot_that_says_how_it_stands() {
        let mut m = SdrMetrics::fixture().rtlsdr().streaming();
        m.timing.sr_known = false;
        let lines = draw(Ft8RadioPanel, 86, 12, &m);
        let line = |name: &str| lines.iter().find(|l| l.contains(name)).unwrap().clone();
        assert!(line("placement").contains("● placement"));
        assert!(line("the slice").contains("○ the slice"));
        assert!(line("reference").contains("○ reference"));
        assert!(line("timing").contains("○ timing"));

        m.radio.reference = Some(crate::state::FrequencyReference {
            ppm: 1.25,
            sigma_ppm: 0.04,
            provenance: crate::state::Provenance::Traceable,
            source: "DCF77".into(),
            at: std::time::Instant::now(),
            efficiency: None,
            trusted: None,
        });
        let theme = crate::Theme::sdr();
        let buf = crate::state::fixture::draw_buffer(Ft8RadioPanel, 86, 12, &m);
        let y = (0..12u16)
            .find(|&y| {
                (0..86u16)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .contains("reference")
            })
            .unwrap();
        let dot = (0..86u16)
            .find(|&x| buf[(x, y)].symbol() == "●")
            .expect("a filled dot");
        assert_eq!(buf[(dot, y)].fg, theme.status_ok);
        let all = draw(Ft8RadioPanel, 86, 12, &m).join("\n");
        assert!(all.contains("+1.25 ppm ± 0.04, by DCF77"), "{all}");
    }
}
