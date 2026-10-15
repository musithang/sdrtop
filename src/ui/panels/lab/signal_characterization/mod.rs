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
//! The channel figures come from the FFT frame as it stood at the last
//! quarter-second tick (`state.signal.readout`), so a number changes slowly
//! enough to be read; channel power is the rail S-meter's own readout, one
//! account for one quantity. The carrier figures (occupied bandwidth, the adjacent channels, the
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
        //
        // The frame is the one taken at the last quarter-second tick, not the
        // one in hand, so a number changes slowly enough to be read; staleness
        // is still the frame in hand's.
        let frame = state.signal.readout.as_ref().filter(|_| !stale);

        let mut lines: Vec<Line> = Vec::new();
        headline::lines(&mut lines, state, frame, iw, theme);
        lines.push(Line::raw(""));
        let carrier = state.signal.carrier.as_ref().filter(|_| !stale);
        let s_meter = state.signal.s_meter.filter(|_| !stale);
        metrics::lines(&mut lines, frame, carrier, s_meter, iw, theme);
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
        m.signal.acpr_needle = [Some(-41.0), Some(-43.0)];
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

    /// **The adjacent-channel rows keep one shape**: measured, released (the
    /// needle falling back with its digits refused) and at rest, the bar
    /// starts and the digits end in the same columns.
    #[test]
    fn the_acpr_rows_keep_one_shape() {
        let measured = in_a_gap();
        let mut released = in_a_gap();
        released.signal.carrier = None;
        released.signal.acpr_needle = [Some(-40.0), Some(-45.0)];
        let mut rest = released.clone();
        rest.signal.acpr_needle = [None, None];
        let l_row = |m: &SdrMetrics| {
            let lines = crate::state::fixture::draw(SignalCharacterizationPanel, 48, 40, m);
            row(&lines, "L \u{2212}").to_string()
        };
        let (a, b, c) = (l_row(&measured), l_row(&released), l_row(&rest));
        assert!(a.contains("-41.0 dB") && a.contains('\u{2588}'), "{a}");
        assert!(b.contains("---") && b.contains('\u{2588}'), "released: {b}");
        assert!(c.contains("---") && !c.contains('\u{2588}'), "at rest: {c}");
        let end = |r: &str| r.trim_end_matches(['\u{2502}', ' ']).chars().count();
        assert_eq!(end(&a), end(&b), "{a}\n{b}");
        assert_eq!(end(&b), end(&c), "{b}\n{c}");
    }

    /// The rows name the offset their needles were measured at, not the one
    /// the frame in hand would use: in a gap the classifier has no class.
    #[test]
    fn the_acpr_rows_name_the_measured_offset() {
        let mut m = in_a_gap();
        m.signal.acpr_offset_hz = 200_000.0;
        m.signal.acpr_measured_offset_hz = Some(9_000.0);
        let lines = crate::state::fixture::draw(SignalCharacterizationPanel, 48, 40, &m);
        let text = lines.join("\n");
        assert!(
            text.contains("L \u{2212}9k") && !text.contains("200k"),
            "{text}"
        );
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

    fn rows_of(m: &SdrMetrics) -> (Option<usize>, Option<usize>) {
        let lines = crate::state::fixture::draw(SignalCharacterizationPanel, 48, 40, m);
        (
            lines.iter().position(|l| l.contains("SPECTRAL SHAPE")),
            lines.iter().position(|l| l.contains("[C] snapshot")),
        )
    }

    /// **The card is one height.** Whatever it says, from RX stopped to a
    /// splatter verdict whose sentence wraps to four rows, the zones above it
    /// and its own last row stay where they are.
    #[test]
    fn the_card_keeps_one_height() {
        use verdict::steady;
        let base = SdrMetrics::fixture().streaming().with_carrier(0.0, 40.0);
        let with = |c: Option<crate::state::Carrier>| {
            let mut m = base.clone();
            m.signal.carrier = c;
            m
        };
        let mut judging = steady(Modulation::Wfm, 40.0, -40.0, -40.0, 180_000);
        judging.verdict = None;
        let mut held = steady(Modulation::Wfm, 43.0, -38.0, -41.0, 180_000);
        held.heard = std::time::Instant::now() - std::time::Duration::from_millis(800);
        let states = [
            with(None),
            with(Some(judging)),
            with(Some(steady(Modulation::Wfm, 15.0, -40.0, -40.0, 180_000))),
            with(Some(steady(Modulation::Wfm, 47.0, -10.0, -40.0, 1_250_000))),
            with(Some(steady(Modulation::Wfm, 43.0, -38.0, -41.0, 180_000))),
            with(Some(held)),
            base.clone().with_stale_fft(),
        ];
        let want = rows_of(&states[0]);
        assert!(want.0.is_some() && want.1.is_some());
        for (i, m) in states.iter().enumerate() {
            assert_eq!(rows_of(m), want, "state {i}");
        }
    }

    /// A verdict held through a gap says how long it has been held.
    #[test]
    fn a_held_verdict_says_how_long() {
        let text = |m: &SdrMetrics| {
            crate::state::fixture::draw(SignalCharacterizationPanel, 48, 40, m).join("\n")
        };
        let m = in_a_gap();
        assert!(!text(&m).contains("held"), "{}", text(&m));
        let mut held = in_a_gap();
        if let Some(c) = held.signal.carrier.as_mut() {
            c.heard = std::time::Instant::now() - std::time::Duration::from_millis(800);
        }
        assert!(text(&held).contains("held 0.8 s"), "{}", text(&held));
    }

    /// **The channel figures are the quarter-second frame's.** The frame in
    /// hand has moved on since the last tick; the panel prints the tick's,
    /// and channel power is the S-meter's own readout.
    #[test]
    fn the_channel_figures_are_the_quarter_second_frame() {
        let mut m = SdrMetrics::fixture().streaming().with_carrier(0.0, 40.0);
        if let Some(fr) = m.waterfall.last_fft.as_mut() {
            fr.peak_to_nf_db = 52.0;
            fr.noise_floor = -95.0;
        }
        if let Some(s) = m.signal.s_meter.as_mut() {
            s.readout = -61.2;
        }
        let lines = crate::state::fixture::draw(SignalCharacterizationPanel, 48, 40, &m);
        let text = lines.join("\n");
        assert!(text.contains("40.0 dB  peak / noise"), "{text}");
        assert!(row(&lines, "Noise floor").contains("-100.0 dBFS"), "{text}");
        assert!(
            row(&lines, "Channel power").contains("~-61.2 dBFS"),
            "{text}"
        );
    }

    /// The badge is the class that has held, as the demod beside it uses.
    #[test]
    fn the_badge_is_the_settled_class() {
        let mut m = SdrMetrics::fixture().streaming().with_carrier(0.0, 40.0);
        m.signal.modulation = Modulation::Nfm;
        m.signal.modulation_settled = Modulation::Wfm;
        let lines = crate::state::fixture::draw(SignalCharacterizationPanel, 48, 40, &m);
        let head = row(&lines, "peak / noise");
        assert!(head.contains("WFM") && !head.contains("NFM"), "{head}");
    }

    #[test]
    fn panel_name_is_stable() {
        assert_eq!(
            SignalCharacterizationPanel.name(),
            "signal_characterization"
        );
    }
}
