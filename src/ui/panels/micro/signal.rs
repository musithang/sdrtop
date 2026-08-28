//! `micro_signal` - the field signal-quality view (`[0]` cycle, 2nd step).
//!
//! Built for antenna aiming and level hunting: a large, immediately readable SNR
//! bar with a short-term trend arrow (the key feedback when sweeping an antenna),
//! plus channel power, noise floor, occupied bandwidth and RBW.

use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome};
use crate::ui::widgets::micro_common::{bar_spans, fft_stale, fmt_bw, fmt_rbw, snr_color};

pub struct MicroSignalPanel;

/// SNR bar spans a 0–40 dB range.
const SNR_FULL_SCALE: f32 = 40.0;

impl Panel for MicroSignalPanel {
    fn name(&self) -> &'static str {
        "micro_signal_panel"
    }
    fn min_size(&self) -> (u16, u16) {
        (40, 6)
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        PanelChrome::untitled()
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        let stale = fft_stale(state);
        let lbl = |s: &'static str| Span::styled(s, Style::default().fg(theme.label));
        let dash = || Span::styled("---".to_string(), Style::default().fg(theme.stale));

        // Header: status badge + frequency. Shared with the other field views -
        // it was written out identically in three of them.
        let header = super::field::header(state, theme);

        // SNR bar row.
        let snr = state.signal.peak_to_nf_db;
        let snr_col = if stale {
            theme.stale
        } else {
            snr_color(snr, theme)
        };
        let bar_w = (inner.width as usize).saturating_sub(24).clamp(8, 28);
        let mut snr_row = vec![Span::raw(" ")];
        if stale {
            snr_row.push(Span::styled(
                "░".repeat(bar_w),
                Style::default().fg(theme.border_dim),
            ));
            snr_row.push(Span::raw("  "));
            snr_row.push(dash());
        } else {
            let [filled, empty] = bar_spans((snr / SNR_FULL_SCALE) as f64, bar_w, snr_col, theme);
            snr_row.push(filled);
            snr_row.push(empty);
            snr_row.push(Span::raw("  "));
            snr_row.push(Span::styled(
                format!("{:.1} dB", snr),
                Style::default().fg(snr_col),
            ));
            if let Some(span) = delta_span(state.signal.snr_delta(), theme) {
                snr_row.push(Span::raw("   "));
                snr_row.push(span);
            }
        }

        // PWR / NF row.
        let pwr = state.signal.channel_power_dbfs;
        let pwr_span = if stale || !pwr.is_finite() {
            dash()
        } else {
            Span::styled(format!("{:.1} dBFS", pwr), Style::default().fg(theme.value))
        };
        let nf_span = match state.waterfall.last_fft.as_ref().filter(|_| !stale) {
            Some(fr) => Span::styled(
                format!("{:.1} dBFS", fr.noise_floor),
                Style::default().fg(theme.value),
            ),
            None => dash(),
        };
        let pwr_nf = Line::from(vec![
            Span::raw(" "),
            lbl("PWR  "),
            pwr_span,
            Span::raw("    "),
            lbl("NF  "),
            nf_span,
        ]);

        // OCC.BW / RBW row.
        let occ = state.signal.occupied_bw_hz;
        let occ_span = if stale || occ == 0 {
            dash()
        } else {
            Span::styled(fmt_bw(occ), Style::default().fg(theme.value))
        };
        let rbw_span = match state
            .waterfall
            .last_fft
            .as_ref()
            .filter(|fr| !stale && fr.enbw_hz > 0.0)
        {
            Some(fr) => Span::styled(fmt_rbw(fr.enbw_hz), Style::default().fg(theme.value)),
            None => dash(),
        };
        let occ_rbw = Line::from(vec![
            Span::raw(" "),
            lbl("OCC.BW  "),
            occ_span,
            Span::raw("    "),
            lbl("RBW  "),
            rbw_span,
        ]);

        let lines = vec![
            header,
            Line::raw(""),
            Line::from(vec![Span::raw("     "), lbl("SNR")]),
            Line::from(snr_row),
            Line::raw(""),
            pwr_nf,
            occ_rbw,
        ];
        f.render_widget(Paragraph::new(lines), inner);
    }
}

/// Trend arrow for the SNR delta: rising green, falling yellow, otherwise a dim
/// steady marker. `None` while there is not yet enough history.
fn delta_span(delta: Option<f32>, theme: &crate::Theme) -> Option<Span<'static>> {
    let d = delta?;
    let (text, color): (String, Color) = if d > 0.3 {
        (format!("↑ +{:.1} dB", d), theme.status_ok)
    } else if d < -0.3 {
        (format!("↓ {:.1} dB", d), theme.status_warn)
    } else {
        ("→ steady".to_string(), theme.stale)
    };
    Some(Span::styled(text, Style::default().fg(color)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    #[test]
    fn delta_span_directions() {
        let t = Theme::sdr();
        assert!(delta_span(None, &t).is_none());
        assert_eq!(
            delta_span(Some(2.3), &t).unwrap().style.fg,
            Some(t.status_ok)
        );
        assert_eq!(
            delta_span(Some(-1.8), &t).unwrap().style.fg,
            Some(t.status_warn)
        );
        assert_eq!(delta_span(Some(0.1), &t).unwrap().style.fg, Some(t.stale));
    }
}
