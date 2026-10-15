// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `signal_characterization` - the left column of the `lab_signal` preset's
//! redesign (DSN-2026-07).
//!
//! An airy read-out of what the signal at centre *is* and how clean it is, built
//! as a single Line stack fitted with `chrome::fit_spacers` and grouped by the
//! shared `chrome::section` nameplates, exactly like `iq_diagnostics`,
//! `rf_chain`, and `timing_diagnostics`:
//!
//!   1. [`headline`] RADIO HEADLINE   - the peak/noise figure + a status lamp.
//!   2. [`metrics`]  SIGNAL METRICS   - channel power, peak (+freq), noise floor,
//!      occupied BW, peak hold.
//!   3. [`acpr`]     ADJACENT CHANNEL - ACPR L/R, a badness-fill bar per side plus
//!      the absolute level of the louder adjacent band.
//!   4. [`shape`]    SPECTRAL SHAPE   - C/N trend + crest.
//!   5. [`card`]     Verdict          - a rule-based, plain-language read of the
//!      same four zones.
//!
//! The verdict's *rule* lives apart from its card, in [`verdict`](fn@verdict): a pure function
//! of modulation / SNR / ACPR / OBW with no drawing in it at all, in the spirit of
//! `timing_diagnostics::verdict_copy`. That is what lets the `lab_signal` marker
//! bar quote the same severity this panel shows without depending on the panel.
//! [`row`] holds the label column and the annotation rule every zone writes into.
//!
//! The channel figures come from the latest coherent FFT frame
//! (`state.waterfall.last_fft`), so they agree with the bonded spectrum beside
//! it. The carrier figures (occupied bandwidth, the adjacent channels, the
//! verdict) come from `state.signal.carrier`: taken only from frames with a
//! carrier, held through a gap of up to a second, because in a gap between
//! bursts the same measurements read noise.

mod acpr;
mod card;
mod headline;
mod metrics;
mod row;
mod shape;
mod verdict;

#[cfg(test)]
pub(crate) use verdict::steady as steady_carrier;
pub(crate) use verdict::{verdict, VerdictLevel};

use ratatui::{layout::Rect, text::Line, widgets::Paragraph, Frame};

use crate::state::SdrMetrics;
use crate::ui::chrome::fit_spacers;
use crate::ui::panel::{Panel, PanelChrome, Staleness};
use crate::ui::widgets::micro_common::fft_stale;

pub struct SignalCharacterizationPanel;

impl Panel for SignalCharacterizationPanel {
    fn name(&self) -> &'static str {
        "signal_characterization"
    }
    fn min_size(&self) -> (u16, u16) {
        (30, 12)
    }
    fn focus_key(&self) -> Option<char> {
        Some('x')
    }
    fn focus_bindings(&self) -> &'static [(&'static str, &'static str)] {
        &[("C", "Snapshot to log")]
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        // No `_` marker: `x` is not a letter of the name, so the engine advertises
        // the key in brackets instead of lighting one up inline.
        PanelChrome::new("Signal Characterization").stale_when(Staleness::FftAge)
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
        let iw = inner.width as usize;

        let stale = fft_stale(state);
        // Every zone that reads the frame reads the *live* frame or none: an aged
        // one must never be printed as a current measurement. Resolved once here so
        // the zones cannot disagree about it, and so `stale` itself is only passed
        // to the two zones that have their own idle copy to print.
        let frame = state.waterfall.last_fft.as_ref().filter(|_| !stale);

        let mut lines: Vec<Line> = Vec::new();
        headline::lines(&mut lines, state, frame, iw, theme);
        lines.push(Line::raw(""));
        let carrier = state.signal.carrier.as_ref().filter(|_| !stale);
        metrics::lines(&mut lines, frame, carrier, iw, theme);
        lines.push(Line::raw(""));
        acpr::lines(&mut lines, state, frame, carrier, iw, theme);
        lines.push(Line::raw(""));
        shape::lines(&mut lines, state, stale, iw, theme);
        lines.push(Line::raw(""));
        card::lines(&mut lines, state, frame, iw, theme);

        fit_spacers(&mut lines, inner.height as usize);
        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::state::Modulation;

    /// A carrier heard 300 ms ago, and the frame in hand a gap between its
    /// bursts: noise against noise, about 0 dB a side, the whole span occupied.
    fn in_a_gap() -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming().with_carrier(0.0, 40.0);
        let mut c = verdict::steady(Modulation::Wfm, 38.0, -41.0, -43.0, 180_000);
        c.heard = std::time::Instant::now() - std::time::Duration::from_millis(300);
        m.signal.carrier = Some(c);
        m.signal.acpr_lower_db = -0.4;
        m.signal.acpr_upper_db = 0.3;
        m.signal.occupied_bw_hz = 1_900_000;
        if let Some(fr) = m.waterfall.last_fft.as_mut() {
            fr.occupied_bw_hz = 1_900_000;
        }
        m
    }

    fn row<'a>(lines: &'a [String], label: &str) -> &'a str {
        lines
            .iter()
            .find(|l| l.contains(label))
            .map_or("", |l| l.as_str())
    }

    /// **A gap is not read as the carrier.** The adjacent ratios, the
    /// occupied bandwidth and the verdict are the carrier's, not the gap's.
    #[test]
    fn the_panel_prints_the_carrier_not_the_gap() {
        let lines = crate::state::fixture::draw(SignalCharacterizationPanel, 48, 40, &in_a_gap());
        let text = lines.join("\n");
        assert!(row(&lines, "Occupied BW").contains("180.0 kHz"), "{text}");
        assert!(row(&lines, "L \u{2212}").contains("-41.0 dB"), "{text}");
        assert!(row(&lines, "R +").contains("-43.0 dB"), "{text}");
        assert!(text.contains("CLEAN WFM SIGNAL"), "{text}");
    }

    /// A second with no carrier and its figures are refused, whatever the
    /// frame in hand reads.
    #[test]
    fn without_a_carrier_its_figures_are_refused() {
        let mut m = in_a_gap();
        m.signal.carrier = None;
        let lines = crate::state::fixture::draw(SignalCharacterizationPanel, 48, 40, &m);
        let text = lines.join("\n");
        assert!(row(&lines, "Occupied BW").contains("---"), "{text}");
        assert!(row(&lines, "L \u{2212}").contains("---"), "{text}");
        assert!(text.contains("NO SIGNAL"), "{text}");
    }

    #[test]
    fn panel_name_is_stable() {
        assert_eq!(
            SignalCharacterizationPanel.name(),
            "signal_characterization"
        );
    }
}
