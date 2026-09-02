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
//! The verdict's *rule* lives apart from its card, in [`verdict`]: a pure function
//! of modulation / SNR / ACPR / OBW with no drawing in it at all, in the spirit of
//! `timing_diagnostics::verdict_copy`. That is what lets the `lab_signal` marker
//! bar quote the same severity this panel shows without depending on the panel.
//! [`row`] holds the label column and the annotation rule every zone writes into.
//!
//! Every scalar comes from the latest coherent FFT frame (`state.waterfall.last_fft`),
//! so the numbers agree with the bonded spectrum beside it.

mod acpr;
mod card;
mod headline;
mod metrics;
mod row;
mod shape;
mod verdict;

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
        metrics::lines(&mut lines, frame, iw, theme);
        lines.push(Line::raw(""));
        acpr::lines(&mut lines, state, frame, stale, iw, theme);
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

    #[test]
    fn panel_name_is_stable() {
        assert_eq!(
            SignalCharacterizationPanel.name(),
            "signal_characterization"
        );
    }
}
