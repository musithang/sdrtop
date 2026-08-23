//! `LevelDiagramPanel` — the Gain-Staging Level Diagram, centre of the Lab RF bench.
//!
//! Plots the modeled signal level and noise floor climbing stage-by-stage
//! (ANT▸LNA▸MIX▸VGA▸ADC). The vertical gap between the two traces is the SNR: it is
//! set at the antenna and only repositioned — not improved — by gain. Reading it left
//! to right tells the whole front-end story the left panel quantifies.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::panel::Panel;
use crate::ui::rf_calc::{cascade, level_lineup, StageLevel};

pub struct LevelDiagramPanel;

/// dBm window of the y-axis (modeled / relative). Wide enough to hold the antenna
/// thermal floor at the bottom and the ADC clip line near the top.
const TOP_DBM: f64 = 10.0;
const BOT_DBM: f64 = -110.0;

/// Interpolate a node series at fractional node position `p` (piecewise linear).
fn lerp_at(vals: &[f64], p: f64) -> f64 {
    if vals.is_empty() { return 0.0; }
    let lo = (p.floor() as usize).min(vals.len() - 1);
    let hi = (lo + 1).min(vals.len() - 1);
    let frac = p - lo as f64;
    vals[lo] + (vals[hi] - vals[lo]) * frac
}

/// Map a dBm level to a chart row (0 = top, `h-1` = bottom). Out-of-range clamps.
fn dbm_row(dbm: f64, h: usize) -> usize {
    let t = ((TOP_DBM - dbm) / (TOP_DBM - BOT_DBM)).clamp(0.0, 1.0);
    (t * (h.saturating_sub(1)) as f64).round() as usize
}

impl Panel for LevelDiagramPanel {
    fn name(&self) -> &'static str { "level_diagram" }
    fn min_size(&self) -> (u16, u16) { (40, 14) }

    fn render(&self, f: &mut Frame, area: Rect, state: &SdrMetrics, theme: &crate::Theme, focused: bool) {
        let stale = !state.radio.hw_streaming;
        let mut title = vec![
            Span::raw(" "),
            Span::styled("Gain-Staging Level Diagram",
                         Style::default().fg(theme.label).add_modifier(Modifier::BOLD)),
        ];
        if stale { title.push(Span::styled(" [STALE]", Style::default().fg(theme.stale))); }
        else if state.lab.rf_freeze.is_some() {
            title.push(Span::styled(" [FRZ]", Style::default().fg(theme.status_warn)));
        }
        title.push(Span::raw(" "));
        let border = if focused { theme.border_focused }
            else if stale { theme.stale } else { theme.border_default };
        let block = Block::default()
            .title(Line::from(title))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border));
        let inner = block.inner(area);
        f.render_widget(block, area);
        if inner.width == 0 || inner.height == 0 { return; }

        let dim = theme.border_dim;
        if stale || !state.caps.friis_applicable {
            let msg = if stale { "\u{2014}\u{2014}\u{2014}" } else { "single-tuner \u{2014} no cascade" };
            f.render_widget(Paragraph::new(Span::styled(msg, Style::default().fg(dim))), inner);
            return;
        }

        // --- model: 5 nodes ANT, LNA, MIX, VGA, ADC (ADC = VGA output) ----------
        // Frozen snapshot when held, else the live gain/level.
        let fz = state.lab.rf_freeze.as_ref();
        let (amp, lna, vga) = fz.map(|f| (f.amp_enabled, f.lna_gain, f.vga_gain))
            .unwrap_or((state.radio.amp_enabled, state.radio.lna_gain, state.radio.vga_gain));
        let stages = cascade(amp, lna, vga);
        let adc_peak = fz.map(|f| f.peak_dbfs).unwrap_or(state.signal.adc_peak_dbfs) as f64;
        let snr = fz.map(|f| f.snr_db).unwrap_or(state.signal.peak_to_nf_db) as f64;
        let mut nodes: Vec<StageLevel> = level_lineup(adc_peak, snr, &stages);
        if let Some(last) = nodes.last().copied() {
            nodes.push(StageLevel { label: "ADC", ..last });
        }
        let n = nodes.len();
        if n < 2 { return; }
        let sig: Vec<f64>   = nodes.iter().map(|s| s.signal_dbm).collect();
        let noise: Vec<f64> = nodes.iter().map(|s| s.noise_dbm).collect();

        let sig_col   = theme.value_hi;
        let noise_col = theme.border_accent;
        let gap_col   = dim;

        // --- layout ------------------------------------------------------------
        let iw = inner.width as usize;
        let ih = inner.height as usize;
        let gutter = 4usize;                         // "−110"
        let chart_w = iw.saturating_sub(gutter + 1);
        // Reserve: caption(1) + x-labels(1) + legend(1).
        let chart_h = ih.saturating_sub(3).clamp(4, 28);
        if chart_w < 8 || chart_h < 4 {
            f.render_widget(Paragraph::new(Span::styled(" level diagram", Style::default().fg(dim))), inner);
            return;
        }

        let mut lines: Vec<Line> = Vec::new();

        // Caption.
        lines.push(Line::from(Span::styled(
            " level climbs stage-by-stage \u{00b7} the gap between signal and noise is the SNR",
            Style::default().fg(dim))));

        // dBm gridline rows.
        let ticks = [10.0, -20.0, -50.0, -80.0, -110.0];
        let mut row_label = vec![String::new(); chart_h];
        for d in ticks {
            let r = dbm_row(d, chart_h);
            if row_label[r].is_empty() { row_label[r] = format!("{:>w$}", d as i32, w = gutter); }
        }
        // Reference lines: ADC clip @ 0 dBm, ADC 8-bit floor @ −50 dBm.
        let clip_row  = dbm_row(0.0, chart_h);
        let floor_row = dbm_row(-50.0, chart_h);

        // Per-column interpolated signal/noise rows.
        let node_at = |col: usize| (col as f64 / (chart_w - 1) as f64) * (n - 1) as f64;
        let sig_row: Vec<usize>   = (0..chart_w).map(|c| dbm_row(lerp_at(&sig,   node_at(c)), chart_h)).collect();
        let noise_row: Vec<usize> = (0..chart_w).map(|c| dbm_row(lerp_at(&noise, node_at(c)), chart_h)).collect();

        for row in 0..chart_h {
            // y gutter
            let g = &row_label[row];
            let mut spans: Vec<Span> = Vec::new();
            if g.is_empty() {
                spans.push(Span::styled(format!("{:>gutter$}\u{2502}", "", gutter = gutter), Style::default().fg(dim)));
            } else {
                spans.push(Span::styled(format!("{g}\u{2524}"), Style::default().fg(dim)));
            }
            // plot row
            let mut cells: Vec<(char, Color)> = Vec::with_capacity(chart_w);
            for c in 0..chart_w {
                let (ch, col) = match classify(row, sig_row[c], noise_row[c]) {
                    Cell::Signal   => ('\u{2588}', sig_col),                       // █ signal
                    Cell::Noise    => ('\u{25ac}', noise_col),                     // ▬ noise
                    Cell::UsableDr => ('\u{2591}', gap_col),                       // ░ usable DR
                    Cell::Buried   => ('\u{2592}', theme.status_warn),             // ▒ signal buried
                    Cell::Empty if row == clip_row  => ('\u{00b7}', theme.status_crit),
                    Cell::Empty if row == floor_row => ('\u{00b7}', theme.status_warn),
                    Cell::Empty    => (' ', dim),
                };
                cells.push((ch, col));
            }
            spans.extend(coalesce(cells));
            lines.push(Line::from(spans));
        }

        // x-axis stage labels, centred under each node column.
        let mut axis: Vec<char> = vec![' '; chart_w];
        for (i, node) in nodes.iter().enumerate() {
            let col = ((i as f64 / (n - 1) as f64) * (chart_w - 1) as f64).round() as usize;
            let lbl = node.label;
            let start = col.saturating_sub(lbl.len() / 2).min(chart_w.saturating_sub(lbl.len()));
            for (k, ch) in lbl.chars().enumerate() {
                if start + k < chart_w { axis[start + k] = ch; }
            }
        }
        lines.push(Line::from(vec![
            Span::raw(" ".repeat(gutter + 1)),
            Span::styled(axis.into_iter().collect::<String>(), Style::default().fg(theme.label)),
        ]));

        // Legend + key figures.
        let headroom = -adc_peak;
        let mut legend = vec![
            Span::raw(" "),
            Span::styled("\u{2588} signal", Style::default().fg(sig_col)),
            Span::styled("  \u{25ac} noise", Style::default().fg(noise_col)),
        ];
        if snr < 0.0 {
            // Signal sits below the noise at the ADC — flag the buried band.
            legend.push(Span::styled("  \u{2592} buried", Style::default().fg(theme.status_warn)));
        } else {
            legend.push(Span::styled("  \u{2591} usable DR", Style::default().fg(gap_col)));
        }
        legend.push(Span::styled(
            format!("  \u{00b7} SNR {snr:.0} dB \u{00b7} headroom {headroom:.0} dB"),
            Style::default().fg(dim)));
        lines.push(Line::from(legend));

        // Self-adjusting density: drop only as many airy spacers as needed to fit,
        // spread evenly, so a short pane keeps balanced breathing room. (chrome)
        crate::ui::chrome::collapse_spacers(&mut lines, ih);
        f.render_widget(Paragraph::new(lines), inner);
    }
}

/// What a chart row holds for a column, from the signal-trace row `sr` and the
/// noise-trace row `nr` (rows grow downward, so a *smaller* row is a *higher* level).
/// The `Buried` case keeps the diagram honest when the noise climbs above the signal
/// (negative SNR): the band between them is flagged instead of left blank.
#[derive(PartialEq, Debug)]
enum Cell { Signal, Noise, UsableDr, Buried, Empty }

fn classify(row: usize, sr: usize, nr: usize) -> Cell {
    if row == sr { Cell::Signal }            // signal wins ties (SNR ≈ 0)
    else if row == nr { Cell::Noise }
    else if row > sr.min(nr) && row < sr.max(nr) {
        if sr < nr { Cell::UsableDr } else { Cell::Buried }
    } else { Cell::Empty }
}

/// Coalesce a per-cell `(char, colour)` row into runs of same-coloured spans.
fn coalesce(cells: Vec<(char, Color)>) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut run = String::new();
    let mut run_col: Option<Color> = None;
    for (ch, col) in cells {
        if run_col != Some(col) {
            if let Some(c) = run_col {
                spans.push(Span::styled(std::mem::take(&mut run), Style::default().fg(c)));
            }
            run_col = Some(col);
        }
        run.push(ch);
    }
    if let Some(c) = run_col {
        spans.push(Span::styled(run, Style::default().fg(c)));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dbm_row_maps_top_and_bottom() {
        assert_eq!(dbm_row(TOP_DBM, 10), 0, "top of range → row 0");
        assert_eq!(dbm_row(BOT_DBM, 10), 9, "bottom → last row");
        // Clamps beyond the window.
        assert_eq!(dbm_row(100.0, 10), 0);
        assert_eq!(dbm_row(-500.0, 10), 9);
    }

    #[test]
    fn lerp_at_interpolates_between_nodes() {
        let v = [0.0, 10.0, 20.0];
        assert!((lerp_at(&v, 0.0) - 0.0).abs() < 1e-9);
        assert!((lerp_at(&v, 0.5) - 5.0).abs() < 1e-9);
        assert!((lerp_at(&v, 2.0) - 20.0).abs() < 1e-9);
        assert!((lerp_at(&v, 9.0) - 20.0).abs() < 1e-9, "past the end clamps to last");
    }

    #[test]
    fn panel_name() {
        assert_eq!(LevelDiagramPanel.name(), "level_diagram");
    }

    #[test]
    fn classify_normal_signal_above_noise() {
        // signal at row 2 (higher level), noise at row 6 (lower level)
        assert_eq!(classify(2, 2, 6), Cell::Signal);
        assert_eq!(classify(6, 2, 6), Cell::Noise);
        assert_eq!(classify(4, 2, 6), Cell::UsableDr, "between them = usable DR");
        assert_eq!(classify(0, 2, 6), Cell::Empty);
        assert_eq!(classify(9, 2, 6), Cell::Empty);
    }

    #[test]
    fn classify_inverted_noise_above_signal() {
        // Noise louder than signal: noise at row 2, signal at row 6.
        assert_eq!(classify(2, 6, 2), Cell::Noise);
        assert_eq!(classify(6, 6, 2), Cell::Signal);
        assert_eq!(classify(4, 6, 2), Cell::Buried, "buried band, not blank");
    }

    #[test]
    fn classify_coincident_is_signal() {
        // SNR ≈ 0: the two traces land on the same row → signal drawn, no blank gap.
        assert_eq!(classify(3, 3, 3), Cell::Signal);
    }
}
