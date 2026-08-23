use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::Style,
    text::{Line, Span},
    widgets::{
        canvas::{Canvas, Line as CanvasLine},
        Block, BorderType, Borders, Paragraph,
    },
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::panel::Panel;

pub struct IqHistogramPanel;

/// Estimated Peak-to-Average Power Ratio from the IQ amplitude histogram.
///
/// Bin mapping (from rx callback): bin i = max(|I|,|Q|) in range [4i, 4(i+1)) / 128.
/// Peak amplitude = top of the highest occupied bin.
/// RMS amplitude  = sqrt( Σ hist[i]·((4i+2)/128)² / total ).
/// Returns None when no samples or when all samples are in bin 0 (zero RMS).
fn estimate_papr_db(hist: &[u64; 32], total: u64) -> Option<f64> {
    if total == 0 { return None; }

    let peak_bin = hist.iter().enumerate().rev()
        .find(|(_, &c)| c > 0)
        .map(|(i, _)| i)?;
    let peak_amp = (peak_bin as f64 * 4.0 + 4.0) / 128.0;

    let mean_sq: f64 = hist.iter().enumerate()
        .map(|(i, &c)| {
            let amp = (4 * i + 2) as f64 / 128.0;  // bin centre, normalised
            c as f64 * amp * amp
        })
        .sum::<f64>()
        / total as f64;

    let rms = mean_sq.sqrt();
    if rms <= 0.0 { return None; }

    Some(20.0 * (peak_amp / rms).log10())
}

fn papr_hint(db: f64) -> &'static str {
    if db < 3.0       { "CW / FM"    }
    else if db < 8.0  { "AM / mixed" }
    else if db < 15.0 { "wideband"   }
    else              { "impulsive"  }
}

impl Panel for IqHistogramPanel {
    fn name(&self) -> &'static str { "iq_histogram" }
    fn min_size(&self) -> (u16, u16) { (36, 9) }

    fn render(&self, f: &mut Frame, area: ratatui::layout::Rect, state: &SdrMetrics, theme: &crate::Theme, _focused: bool) {
        let stale = !state.radio.hw_streaming;
        let title = if stale { " IQ Amplitude Distribution [STALE] " }
                    else     { " IQ Amplitude Distribution " };
        let border_color = if stale { theme.stale } else { theme.border_default };
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border_color));
        let inner = block.inner(area);
        f.render_widget(block, area);

        if inner.height < 5 || inner.width < 4 { return; }

        // chart | axis | numeric breakdown | PAPR | status label
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(inner);
        let chart_area     = layout[0];
        let axis_area      = layout[1];
        let breakdown_area = layout[2];
        let papr_area      = layout[3];
        let label_area     = layout[4];

        let hist = &state.iq.iq_amplitude_hist;
        let total: u64 = hist.iter().sum();
        let high_count: u64 = hist[24..].iter().sum();
        let low_count:  u64 = hist[..8].iter().sum();
        let mid_count:  u64 = hist[8..24].iter().sum();
        let max_count  = hist.iter().copied().max().unwrap_or(1).max(1);
        let n_bins     = 32usize.min(chart_area.width as usize);
        let log_max    = ((max_count + 1) as f64).log2();

        // Pre-compute per-bin heights and colors (can't borrow theme inside closure)
        let label_color = theme.label;
        let ok_color    = theme.status_ok;
        let crit_color  = theme.status_crit;

        let bin_data: Vec<(f64, f64)> = hist.iter().take(n_bins).enumerate()
            .map(|(i, &count)| {
                let h = if log_max > 0.0 { ((count + 1) as f64).log2() / log_max } else { 0.0 };
                (i as f64, h)
            })
            .collect();

        // Canvas — spectrum style: filled columns + outline connecting bin tops
        f.render_widget(
            Canvas::default()
                .x_bounds([0.0, n_bins as f64])
                .y_bounds([0.0, 1.0])
                .paint(move |ctx| {
                    // Filled columns
                    for &(x, h) in &bin_data {
                        let color = if x >= 24.0 { crit_color }
                                    else if x >= 8.0 { ok_color }
                                    else { label_color };
                        ctx.draw(&CanvasLine { x1: x + 0.5, y1: 0.0, x2: x + 0.5, y2: h, color });
                    }
                    // Outline
                    for i in 1..bin_data.len() {
                        let (x0, h0) = bin_data[i - 1];
                        let (x1, h1) = bin_data[i];
                        let color = if x1 >= 24.0 { crit_color }
                                    else if x1 >= 8.0 { ok_color }
                                    else { label_color };
                        ctx.draw(&CanvasLine { x1: x0 + 0.5, y1: h0, x2: x1 + 0.5, y2: h1, color });
                    }
                }),
            chart_area,
        );

        // X-axis zone labels aligned to bin boundaries
        let low_cols = (chart_area.width as usize * 8  / n_bins).max(1);
        let high_cols = (chart_area.width as usize * 8  / n_bins).max(1);
        let mid_cols  = (chart_area.width as usize).saturating_sub(low_cols + high_cols).max(1);

        let ax_layout = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(low_cols as u16),
                Constraint::Length(mid_cols as u16),
                Constraint::Min(0),
            ])
            .split(axis_area);

        let dim = Style::default().fg(theme.label);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("0", dim),
                Span::styled("─".repeat(low_cols.saturating_sub(1)), dim),
            ])),
            ax_layout[0],
        );
        let mid_label = "── OK ──";
        let pad = ax_layout[1].width as usize / 2;
        f.render_widget(
            Paragraph::new(Span::styled(
                // Use chars().count() — '─' is 3 bytes in UTF-8 so .len() would
                // over-pad by 8 bytes, pushing the label off-screen in narrow windows.
                format!("{:>width$}", mid_label, width = pad + mid_label.chars().count()),
                Style::default().fg(ok_color),
            )),
            ax_layout[1],
        );
        f.render_widget(
            Paragraph::new(Span::styled("clip", Style::default().fg(crit_color))),
            ax_layout[2],
        );

        // Numeric breakdown: Low / Mid / Clip percentages
        let lbl = Style::default().fg(theme.label);
        let (low_pct, mid_pct, high_pct) = if total > 0 {
            (low_count * 100 / total, mid_count * 100 / total, high_count * 100 / total)
        } else {
            (0, 0, 0)
        };
        let low_col  = if low_pct  > 90 { theme.status_crit }
                       else if low_pct  > 70 { theme.status_warn } else { theme.label };
        let mid_col  = if mid_pct  > 50 { theme.status_ok   }
                       else if mid_pct  > 20 { theme.status_warn } else { theme.label };
        let clip_col = if high_pct > 10 { theme.status_crit }
                       else if high_pct >  0 { theme.status_warn } else { theme.status_ok };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Low ", lbl),
                Span::styled(format!("{:3}%", low_pct),  Style::default().fg(low_col)),
                Span::styled("  Mid ", lbl),
                Span::styled(format!("{:3}%", mid_pct),  Style::default().fg(mid_col)),
                Span::styled("  Clip ", lbl),
                Span::styled(format!("{:3}%", high_pct), Style::default().fg(clip_col)),
            ])),
            breakdown_area,
        );

        // PAPR (Crest Factor) estimate
        let papr_line = match estimate_papr_db(hist, total) {
            Some(db) => Line::from(vec![
                Span::styled("PAPR ", lbl),
                Span::styled(format!("{:.1} dB", db), Style::default().fg(theme.value)),
                Span::styled(format!("  ({})", papr_hint(db)), Style::default().fg(theme.border_dim)),
            ]),
            None => Line::from(vec![
                Span::styled("PAPR ", lbl),
                Span::styled("---", Style::default().fg(theme.label)),
            ]),
        };
        f.render_widget(Paragraph::new(papr_line), papr_area);

        // Status label
        let label = if total == 0 {
            Span::styled("No samples yet", Style::default().fg(theme.label))
        } else if high_count > total / 10 {
            Span::styled("▲ clipping risk", Style::default().fg(theme.status_crit))
        } else if low_count > total * 9 / 10 {
            Span::styled("▼ weak signal — ADC under-utilised", Style::default().fg(theme.status_warn))
        } else {
            Span::styled("Dynamic range OK", Style::default().fg(theme.status_ok))
        };
        f.render_widget(Paragraph::new(label), label_area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn papr_none_when_empty() {
        assert!(estimate_papr_db(&[0u64; 32], 0).is_none());
    }

    #[test]
    fn papr_single_bin_is_near_zero_db() {
        // All samples in one mid/high bin → peak_top/bin_centre ≈ 1 → PAPR near 0 dB.
        // (Bin 0 is a special case: ratio = 4/2 = 2 → 6 dB, a quantisation artefact
        //  for noise-floor signals — the status label already flags those as "weak".)
        let mut hist = [0u64; 32];
        hist[20] = 1000;
        let papr = estimate_papr_db(&hist, 1000).unwrap();
        assert!(papr.abs() < 1.0, "single-bin PAPR (bin 20) should be ~0.2 dB, got {:.2}", papr);
    }

    #[test]
    fn papr_uniform_distribution_near_5db() {
        // Uniform distribution: peak is max bin, rms is ~sqrt(1/3) of peak
        // → PAPR ≈ 20*log10(sqrt(3)) ≈ 4.8 dB
        let hist = [100u64; 32];
        let papr = estimate_papr_db(&hist, 3200).unwrap();
        assert!(papr > 3.0 && papr < 7.0, "uniform PAPR should be ~4.8 dB, got {:.2}", papr);
    }

    #[test]
    fn papr_low_amplitude_signal_computable() {
        // All samples in lowest bins → PAPR should still compute without panic
        let mut hist = [0u64; 32];
        hist[0] = 500; hist[1] = 300; hist[2] = 100;
        let papr = estimate_papr_db(&hist, 900);
        assert!(papr.is_some(), "should return Some for low-amplitude signal");
    }

    #[test]
    fn papr_hint_coverage() {
        assert_eq!(papr_hint(1.0),  "CW / FM");
        assert_eq!(papr_hint(5.0),  "AM / mixed");
        assert_eq!(papr_hint(10.0), "wideband");
        assert_eq!(papr_hint(20.0), "impulsive");
    }

    #[test]
    fn histogram_row_split_does_not_panic_on_block_chars() {
        // '█' is 3 bytes in UTF-8; byte-slicing at column index 8 would panic
        // mid-character. chars().take/skip must be used instead.
        let row: String = (0..32).map(|_| '█').collect();
        let low_cols = 8usize;
        let mid_cols = 16usize;

        let low:  String = row.chars().take(low_cols).collect();
        let mid:  String = row.chars().skip(low_cols).take(mid_cols).collect();
        let high: String = row.chars().skip(low_cols + mid_cols).collect();

        assert_eq!(low.chars().count(),  8);
        assert_eq!(mid.chars().count(), 16);
        assert_eq!(high.chars().count(), 8);
    }
}
