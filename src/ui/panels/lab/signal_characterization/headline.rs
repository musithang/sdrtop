//! `RADIO HEADLINE` - the one figure that answers "is there anything here?":
//! peak over noise floor, with the classifier's modulation badge and a status
//! lamp in the same colour.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::state::{FftFrame, SdrMetrics};
use crate::ui::chrome::section;
use crate::ui::widgets::micro_common::snr_color;

use super::row::{dim, val};

pub(super) fn lines(
    out: &mut Vec<Line<'static>>,
    state: &SdrMetrics,
    frame: Option<&FftFrame>,
    iw: usize,
    theme: &crate::Theme,
) {
    out.push(section("RADIO HEADLINE", "", iw, theme));
    let Some(fr) = frame else {
        out.push(Line::from(vec![
            Span::raw(" "),
            Span::styled("\u{25cb} IDLE \u{2014} RX stopped", dim(theme)),
        ]));
        return;
    };

    // The same clean ≥ 20 dB / usable ≥ 10 dB grading the signal strip and the
    // micro views use, from the one definition in `micro_common`.
    let snr = fr.peak_to_nf_db;
    let col = snr_color(snr, theme);
    let mut hspans = vec![
        Span::raw(" "),
        Span::styled(
            format!("{snr:.1}"),
            Style::default().fg(col).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" dB", val(theme)),
        Span::styled("  peak / noise", dim(theme)),
    ];
    // MOD badge - the classifier's estimate of what's at centre.
    if state.signal.modulation.is_known() {
        hspans.push(Span::styled(
            format!("   {}", state.signal.modulation.label()),
            Style::default()
                .fg(theme.value_hi)
                .add_modifier(Modifier::BOLD),
        ));
    }
    hspans.push(Span::styled("   \u{25cf}", Style::default().fg(col)));
    out.push(Line::from(hspans));
}
