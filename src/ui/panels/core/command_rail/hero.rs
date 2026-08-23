//! The frequency hero at the top of the rail: the tuned frequency in big
//! seven-segment digits with the active tuning digit lit, and the band / sample
//! rate line under it.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::state::SdrMetrics;
use crate::ui::panels::core::header::{active_digit_idx, vfo_spans, vfo_string};
use crate::ui::widgets::micro_common::fmt_rbw;
use crate::ui::widgets::band_plan::band_at;
use crate::ui::widgets::bigdigits;

/// The frequency hero: the big 3-row block readout, or a single bold line when
/// the rail is too narrow for the block font. The actively-tuned digit is lit in
/// `value_hi` (the same digit the small VFO underlines), the rest in `value`, the
/// decimal point dim — all dim in observer mode.
pub(super) fn freq_hero_lines(freq: u64, step: u64, observer: bool, inner_w: usize,
                   theme: &crate::Theme) -> Vec<Line<'static>> {
    let s = vfo_string(freq);

    // Narrow fallback: the existing single-line segmented VFO (+" MHz"). The
    // budget covers the leading space (1) + the gap (1) + the "MHz" suffix (3),
    // so the big readout shows whenever its widest (middle) row actually fits.
    if bigdigits::big_width(&s) + 5 > inner_w {
        let col = if observer { theme.label } else { theme.value_hi };
        let mut spans = vec![Span::raw(" ")];
        spans.extend(vfo_spans(freq, step, col, theme.label, theme.value_hi));
        spans.push(Span::raw(" "));
        spans.push(Span::styled("MHz", Style::default().fg(theme.label)));
        return vec![Line::from(spans)];
    }

    let active = active_digit_idx(freq, step);
    let chars: Vec<char> = s.chars().collect();
    let mut rows: [Vec<Span<'static>>; 3] =
        [vec![Span::raw(" ")], vec![Span::raw(" ")], vec![Span::raw(" ")]];
    for (i, &c) in chars.iter().enumerate() {
        let color = if observer { theme.label }
            else if Some(i) == active { theme.value_hi }
            else if c == '.' { theme.label }
            else { theme.value };
        let g = bigdigits::glyph(c);
        for (r, row) in rows.iter_mut().enumerate() {
            if i > 0 { row.push(Span::raw(" ")); }
            row.push(Span::styled(g[r].to_string(), Style::default().fg(color)));
        }
    }
    // "MHz" rides the middle row, just past the digits.
    rows[1].push(Span::raw(" "));
    rows[1].push(Span::styled("MHz", Style::default().fg(theme.label)));
    let [r0, r1, r2] = rows;
    vec![Line::from(r0), Line::from(r1), Line::from(r2)]
}

/// `[FM]  SR 2.0M · RBW 1.5 kHz` — the band chip plus sample-rate / resolution
/// context, sitting just under the frequency hero.
pub(super) fn band_sr_line(state: &SdrMetrics, iw: usize, theme: &crate::Theme) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    let mut used = 1;
    if let Some(b) = band_at(state.radio.frequency) {
        let chip = format!(" {b} ");
        used += chip.chars().count() + 2;
        spans.push(Span::styled(chip, Style::default()
            .fg(Color::Rgb(4, 6, 15)).bg(theme.value_hi).add_modifier(Modifier::BOLD)));
        spans.push(Span::raw("  "));
    }
    let sr = format!("SR {:.1}M", state.radio.config_sample_rate / 1_000_000.0);
    used += sr.chars().count();
    spans.push(Span::styled(sr, Style::default().fg(theme.label)));
    // RBW is the first thing to go on a narrow rail — drop it (and its separator)
    // rather than let it clip mid-word at the panel border.
    let rbw = match state.waterfall.last_fft.as_ref().filter(|fr| fr.enbw_hz > 0.0) {
        Some(fr) => fmt_rbw(fr.enbw_hz),
        None     => "—".to_string(),
    };
    let rbw_str = format!(" · RBW {rbw}");
    if used + rbw_str.chars().count() <= iw {
        spans.push(Span::styled(" · ", Style::default().fg(theme.border_dim)));
        spans.push(Span::styled(format!("RBW {rbw}"), Style::default().fg(theme.label)));
    }
    Line::from(spans)
}
