use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::widgets::micro_common::fft_stale;
use crate::ui::panel::{Panel, PanelChrome};

pub struct SignalStripPanel;

fn snr_color(db: f32, theme: &crate::Theme) -> Color {
    if db >= 20.0 { theme.status_ok } else if db >= 10.0 { theme.status_warn } else { theme.status_crit }
}

fn sat_color(pct: f32, theme: &crate::Theme) -> Color {
    if pct < 1.0 { theme.status_ok } else if pct < 5.0 { theme.status_warn } else { theme.status_crit }
}

fn drop_color(drops: u64, theme: &crate::Theme) -> Color {
    if drops == 0 { theme.status_ok } else if drops < 10 { theme.status_warn } else { theme.status_crit }
}

fn buf_color(pct: f32, theme: &crate::Theme) -> Color {
    if pct < 50.0 { theme.status_ok } else if pct < 80.0 { theme.status_warn } else { theme.status_crit }
}

fn iq_color(db: f32, theme: &crate::Theme) -> Color {
    if db.abs() < 1.0 { theme.status_ok } else if db.abs() < 3.0 { theme.status_warn } else { theme.status_crit }
}

fn fmt_rbw(hz: f64) -> String {
    if hz >= 1_000.0 { format!("{:.1} kHz", hz / 1_000.0) }
    else { format!("{:.0} Hz", hz) }
}

/// Width (cells) of a mini-bar gauge.
const BAR_W: usize = 7;

/// Eight vertical block glyphs for a one-row sparkline (▁ low … █ high).
const SPARK: [char; 8] = ['\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}',
                          '\u{2585}', '\u{2586}', '\u{2587}', '\u{2588}'];

/// Render a value series as a one-row block sparkline of `width` chars, newest at
/// the right. Auto-scales to the series' own min/max so small movements stay
/// visible. Shorter series are left-padded with spaces. Empty → empty string.
fn sparkline(series: &[f32], width: usize) -> String {
    if series.is_empty() || width == 0 { return String::new(); }
    let take  = series.len().min(width);
    let slice = &series[series.len() - take..];
    let lo = slice.iter().copied().fold(f32::INFINITY, f32::min);
    let hi = slice.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let range = (hi - lo).max(1e-6);
    let body: String = slice.iter()
        .map(|&v| SPARK[(((v - lo) / range) * 7.0).round().clamp(0.0, 7.0) as usize])
        .collect();
    if take < width { format!("{}{}", " ".repeat(width - take), body) } else { body }
}

/// One metric in the strip: a label, a formatted value, and an optional gauge
/// fill ratio (0–1). `fill = None` marks an info value with no bar (e.g. RBW).
struct Cell {
    label:  &'static str,
    value:  String,
    vcolor: Color,
    fill:   Option<f32>,
    bcolor: Color,
    /// Recent value history (oldest→newest) for a trailing sparkline. Empty for
    /// metrics with no time-series (PWR / NF / RBW / IQ have no ring buffer).
    trend:  Vec<f32>,
}

impl Cell {
    fn gauge(label: &'static str, value: String, color: Color, fill: f32) -> Self {
        Cell { label, value, vcolor: color, fill: Some(fill.clamp(0.0, 1.0)), bcolor: color, trend: Vec::new() }
    }
    fn info(label: &'static str, value: String, color: Color) -> Self {
        Cell { label, value, vcolor: color, fill: None, bcolor: color, trend: Vec::new() }
    }
    fn dash(label: &'static str, theme: &crate::Theme, bar: bool) -> Self {
        Cell {
            label, value: "---".into(), vcolor: theme.stale,
            fill: if bar { Some(0.0) } else { None }, bcolor: theme.stale, trend: Vec::new(),
        }
    }
    /// Attach a value history for the trailing sparkline.
    fn with_trend(mut self, trend: Vec<f32>) -> Self { self.trend = trend; self }
}

/// Build all eight metric cells, ordered: signal row (P/NF, PWR, NF, RBW) then
/// hardware row (SAT, DROP, BUF, IQ). Stale / not-streaming metrics dash out.
fn build_cells(state: &SdrMetrics, theme: &crate::Theme, stale: bool, hw_stale: bool) -> Vec<Cell> {
    // ── Signal row (FFT-derived) ─────────────────────────────────────────
    let snr = state.signal.peak_to_nf_db;
    let pnf = if stale {
        Cell::dash("P/NF", theme, true)
    } else {
        Cell::gauge("P/NF", format!("{:.1} dB", snr), snr_color(snr, theme), snr / 40.0)
            .with_trend(state.signal.snr_history.iter().copied().collect())
    };

    let pwr_finite = state.signal.channel_power_dbfs.is_finite();
    let pwr = if stale || !pwr_finite {
        Cell::dash("PWR", theme, true)
    } else {
        let p = state.signal.channel_power_dbfs;
        Cell::gauge("PWR", format!("{:.1} dBFS", p), theme.value, (p + 100.0) / 100.0)
    };

    let nf = match state.waterfall.last_fft.as_ref().filter(|_| !stale) {
        Some(fr) => Cell::gauge("NF", format!("{:.1} dBFS", fr.noise_floor), theme.value,
                                (fr.noise_floor + 120.0) / 80.0),
        None => Cell::dash("NF", theme, true),
    };

    let rbw = match state.waterfall.last_fft.as_ref().filter(|_| !stale) {
        Some(fr) if fr.enbw_hz > 0.0 => Cell::info("RBW", fmt_rbw(fr.enbw_hz), theme.value),
        _ => Cell::dash("RBW", theme, false),
    };

    // ── Hardware row (rx-accumulator-derived) ────────────────────────────
    let sat = if hw_stale {
        Cell::dash("SAT", theme, true)
    } else {
        let s = state.signal.adc_saturation_pct;
        Cell::gauge("SAT", format!("{:.1}%", s), sat_color(s, theme), s / 10.0)
            .with_trend(state.signal.saturation_history.iter().copied().collect())
    };
    let drop = if hw_stale {
        Cell::dash("DROP", theme, true)
    } else {
        let d = state.signal.drops_per_sec;
        Cell::gauge("DROP", format!("{}/s", d), drop_color(d, theme), d as f32 / 20.0)
            .with_trend(state.signal.drop_history.iter().map(|&v| v as f32).collect())
    };
    let buf = if hw_stale {
        Cell::dash("BUF", theme, true)
    } else {
        let b = state.iq.buf_fill_pct;
        Cell::gauge("BUF", format!("{:.0}%", b), buf_color(b, theme), b / 100.0)
            .with_trend(state.iq.buf_fill_history.iter().map(|&v| v as f32).collect())
    };
    let iq = if hw_stale {
        Cell::dash("IQ", theme, true)
    } else {
        let v = state.iq.iq_imbalance_db;
        Cell::gauge("IQ", format!("{:+.1} dB", v), iq_color(v, theme), v.abs() / 6.0)
    };

    vec![pnf, pwr, nf, rbw, sat, drop, buf, iq]
}

/// Mini-bar gauge spans: filled `▰` in the metric color, empty `▱` dim. An
/// info cell (`fill = None`) renders a dim `·····` placeholder to keep columns
/// aligned across rows.
fn bar_spans(fill: Option<f32>, bcolor: Color, dim: Color) -> Vec<Span<'static>> {
    match fill {
        Some(f) => {
            let filled = (f.clamp(0.0, 1.0) * BAR_W as f32).round() as usize;
            vec![
                Span::styled("▮".repeat(filled), Style::default().fg(bcolor)),
                Span::styled("▯".repeat(BAR_W - filled), Style::default().fg(dim)),
            ]
        }
        None => vec![Span::styled("·".repeat(BAR_W), Style::default().fg(dim))],
    }
}

/// Status-lamp glyph for a metric, by its threshold colour: `●` nominal/ok,
/// `▲` warn or crit (the colour already carries the severity), `·` stale, and
/// `◦` for a neutral level metric (PWR/NF/RBW) that has no pass/fail state.
fn cell_sigil(color: Color, theme: &crate::Theme) -> &'static str {
    if color == theme.status_ok { "\u{25CF}" }                              // ●
    else if color == theme.status_warn || color == theme.status_crit { "\u{25B2}" } // ▲
    else if color == theme.stale { "\u{00B7}" }                            // ·
    else { "\u{25E6}" }                                                    // ◦
}

/// ` ● LABEL ▰▰▰▱▱ value ▁▂▃▅` — one gauge cell with a leading status lamp and,
/// when `spark_w` columns are free and the cell has history, a trailing
/// sparkline. Laid out left-aligned in its column.
fn cell_spans(c: &Cell, theme: &crate::Theme, spark_w: usize) -> Vec<Span<'static>> {
    let mut spans = vec![
        Span::styled(format!(" {} ", cell_sigil(c.vcolor, theme)), Style::default().fg(c.vcolor)),
        Span::styled(format!("{:<4} ", c.label), Style::default().fg(theme.label)),
    ];
    spans.extend(bar_spans(c.fill, c.bcolor, theme.border_dim));
    spans.push(Span::styled(format!(" {:<11}", c.value), Style::default().fg(c.vcolor)));
    if spark_w >= 3 && c.trend.len() >= 2 {
        spans.push(Span::styled(
            format!(" {}", sparkline(&c.trend, spark_w)),
            Style::default().fg(theme.border_dim),
        ));
    }
    spans
}

impl Panel for SignalStripPanel {
    fn name(&self) -> &'static str { "signal_strip" }
    fn min_size(&self) -> (u16, u16) { (60, 3) }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome { PanelChrome::deck("Signal") }

    fn render(&self, f: &mut Frame, inner: Rect, state: &SdrMetrics, theme: &crate::Theme, _focused: bool) {
        let stale = fft_stale(state);
        let hw_stale = !state.radio.hw_streaming;

        let cells = build_cells(state, theme, stale, hw_stale);

        // Rich 2×4 gauge grid when there is vertical room and width; otherwise a
        // single compact line (keeps height-3 / narrow presets working). Cells
        // are spread across four even columns so the cluster fills the panel.
        if inner.height >= 2 && inner.width >= 108 {
            let ncol: u16 = 4;
            let col_w = inner.width / ncol;
            // The base cell (sigil+label+bar+value) is 27 cols; anything past that,
            // up to 8, becomes the trailing sparkline. Narrow terminals simply omit
            // it (spark_w < 3) — no truncation, graceful degradation.
            let spark_w = (col_w as usize).saturating_sub(28).min(8);
            for (ri, chunk) in cells.chunks(4).enumerate().take(inner.height as usize) {
                for (ci, c) in chunk.iter().enumerate() {
                    let x = inner.x + ci as u16 * col_w;
                    let w = if ci as u16 == ncol - 1 { inner.width - col_w * (ncol - 1) } else { col_w };
                    let rect = Rect { x, y: inner.y + ri as u16, width: w, height: 1 };
                    f.render_widget(Paragraph::new(Line::from(cell_spans(c, theme, spark_w))), rect);
                }
            }
        } else {
            let sep = Span::styled("  ·  ", Style::default().fg(theme.border_dim));
            let mut spans = vec![Span::raw(" ")];
            for (i, c) in cells.iter().enumerate() {
                if i > 0 { spans.push(sep.clone()); }
                spans.push(Span::styled(format!("{} ", c.label), Style::default().fg(theme.label)));
                spans.push(Span::styled(c.value.clone(), Style::default().fg(c.vcolor)));
            }
            f.render_widget(Paragraph::new(Line::from(spans)), inner);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    #[test]
    fn snr_color_thresholds() {
        let t = Theme::sdr();
        assert_eq!(snr_color(25.0, &t), t.status_ok);
        assert_eq!(snr_color(15.0, &t), t.status_warn);
        assert_eq!(snr_color(5.0,  &t), t.status_crit);
    }

    #[test]
    fn sat_color_thresholds() {
        let t = Theme::sdr();
        assert_eq!(sat_color(0.5, &t), t.status_ok);
        assert_eq!(sat_color(2.0, &t), t.status_warn);
        assert_eq!(sat_color(8.0, &t), t.status_crit);
    }

    #[test]
    fn drop_color_thresholds() {
        let t = Theme::sdr();
        assert_eq!(drop_color(0,  &t), t.status_ok);
        assert_eq!(drop_color(5,  &t), t.status_warn);
        assert_eq!(drop_color(15, &t), t.status_crit);
    }

    #[test]
    fn sparkline_maps_extremes_to_low_and_high_glyphs() {
        let s = sparkline(&[0.0, 10.0], 2);
        let chars: Vec<char> = s.chars().collect();
        assert_eq!(chars[0], '\u{2581}'); // min → ▁
        assert_eq!(chars[1], '\u{2588}'); // max → █
    }

    #[test]
    fn sparkline_left_pads_short_series() {
        let s = sparkline(&[5.0], 4);
        assert_eq!(s.chars().count(), 4);
        assert!(s.starts_with("   "), "padded with leading spaces: {s:?}");
    }

    #[test]
    fn sparkline_takes_newest_when_longer_than_width() {
        // 5 samples into width 3 → only the last 3 are shown
        let s = sparkline(&[0.0, 0.0, 0.0, 0.0, 10.0], 3);
        assert_eq!(s.chars().count(), 3);
        assert_eq!(s.chars().last().unwrap(), '\u{2588}'); // newest (max) at the right
    }

    #[test]
    fn sparkline_flat_series_is_stable() {
        // constant series must not divide-by-zero or panic; all same glyph
        let s = sparkline(&[3.0, 3.0, 3.0], 3);
        assert_eq!(s.chars().count(), 3);
    }

    #[test]
    fn sparkline_empty_or_zero_width_is_empty() {
        assert_eq!(sparkline(&[], 4), "");
        assert_eq!(sparkline(&[1.0, 2.0], 0), "");
    }

    #[test]
    fn fmt_rbw_formats_correctly() {
        assert_eq!(fmt_rbw(800.0),       "800 Hz");
        assert_eq!(fmt_rbw(1_500.0),     "1.5 kHz");
        assert_eq!(fmt_rbw(15_000.0),    "15.0 kHz");
        assert_eq!(fmt_rbw(4_882.8),     "4.9 kHz");
    }
}
