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
        let dim = Style::default().fg(theme.border_dim);
        let x = area.x + 1;
        let xv = x + 11;
        let mut y = area.y;
        let mut row = |name: &str, text: String, style: Style, y: &mut u16| {
            if *y < area.bottom() {
                if !name.is_empty() {
                    put(buf, area, x, *y, name, label);
                }
                put(buf, area, xv, *y, &text, style);
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
        row(
            "placement",
            format!(
                "peak {:.0} dBFS · ceiling {} dBFS{part}",
                state.signal.adc_peak_dbfs,
                state.ceiling_text()
            ),
            value,
            &mut y,
        );
        let headroom = state.shown_headroom_db();
        let (_, sev) = staging_verdict_for(
            f64::from(state.peak_to_ceiling_db()),
            state.ceiling().map(|c| c.kind),
        );
        let ink = match sev {
            2 => theme.status_crit,
            1 => theme.status_warn,
            _ => theme.value,
        };
        let side = if headroom >= 0.0 { "under" } else { "over" };
        row(
            "",
            format!(
                "{:.0} dB {side} it · {held} of the hour's {hour} cycles reached it",
                headroom.abs()
            ),
            Style::default().fg(ink),
            &mut y,
        );
        if state.caps.gain.stages_are_digital() {
            row(
                "",
                "the gain stages place the level, not the SNR".into(),
                dim,
                &mut y,
            );
        }

        // The slice.
        match strongest(state) {
            Some((hz, db)) => row(
                "the slice",
                format!(
                    "strongest outside FT8: {} kHz, {db:.0} dBFS",
                    signed(hz / 1000.0, 1)
                ),
                value,
                &mut y,
            ),
            None => row("the slice", "no spectrum frame yet".into(), dim, &mut y),
        }

        // The reference.
        let per_ppm = state.radio.frequency as f64 * 1e-6;
        let reference = match &state.radio.reference {
            Some(r) => format!(
                "{} ppm ± {:.2}, by {}",
                signed(r.ppm, 2),
                r.sigma_ppm,
                r.source
            ),
            None => "none established".to_string(),
        };
        row("reference", reference, value, &mut y);
        row(
            "",
            format!("1 ppm is {per_ppm:.1} Hz here; no FT8 measurement can check it"),
            dim,
            &mut y,
        );

        // Timing: the timing bench's figures, and the FT8 clock's lateness.
        let t = &state.timing;
        let clock = if t.sr_known {
            format!("sample clock {} ppm", signed(t.sr_delta_ppm as f64, 0))
        } else {
            "sample clock not known yet".to_string()
        };
        row(
            "timing",
            format!("{clock} · jitter {} us", t.cb_jitter_us),
            value,
            &mut y,
        );
        let late = state
            .ft8
            .cycles
            .back()
            .and_then(|c| c.lateness)
            .map_or_else(
                || "lateness not known yet".to_string(),
                |l| {
                    format!(
                        "deliveries late {:.1} ms median, {:.1} p90",
                        l.median_ms, l.p90_ms
                    )
                },
            );
        row("", late, value, &mut y);
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
        assert!(all.contains("no FT8 measurement can check it"), "{all}");
        assert!(all.contains("place the level, not the SNR"), "{all}");
        assert!(all.contains("cycles reached it"), "{all}");
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
}
