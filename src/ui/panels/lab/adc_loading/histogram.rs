//! The signed-sample bell: the panel's hero visual, plus the caliper guides
//! overlaid on it and the axis and legend that tie it to numbers.
//!
//! The bell is drawn from the 32-bin signed histogram; the calipers are placed
//! from the measured levels, symmetrically about true 0. That is deliberate and
//! is what makes the picture a measurement rather than a decoration: the bell
//! shows where the samples actually are, the calipers show where full scale is,
//! and a DC offset shows up as the bell sitting off-centre between them.

use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

/// Vertical ⅛-block ramp for the histogram bell, 0 = blank … 8 = full cell.
const VBLOCKS: [char; 9] = [
    ' ', '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}',
    '\u{2585}', '\u{2586}', '\u{2587}', '\u{2588}',
];

/// Everything the bell needs that is not the histogram itself.
pub(super) struct Levels {
    /// Loudest per-axis sample as a fraction of full scale.
    pub peak_frac:  f64,
    /// Per-axis 1σ of the distribution, as a fraction of full scale.
    pub sigma_frac: f64,
    /// Whether the ADC is clipping — reddens the rail-adjacent columns.
    pub clipping:   bool,
}

/// Push the bell, its x-axis and the caliper legend.
pub(super) fn draw(
    out: &mut Vec<Line<'static>>, hist: &[u64; 32], levels: &Levels,
    chart_w: usize, chart_h: usize, theme: &crate::Theme,
) {
    let dim = theme.border_dim;
    let lbl = Style::default().fg(theme.label);
    let cols = rebin(hist, chart_w);
    let maxc = cols.iter().copied().max().unwrap_or(0);

    // Caliper columns from the live levels. Peak is the bright outer caliper at
    // the loudest per-axis sample (peak_frac is already a single-component max, so
    // ±a_peak is its true envelope); the inner band marks the per-axis 1σ of the
    // distribution. Both use cool colours so they stay legible over the warm fill.
    let (peak_lo, peak_hi) = caliper_cols(levels.peak_frac,  chart_w);
    let (sig_lo,  sig_hi)  = caliper_cols(levels.sigma_frac, chart_w);
    let peak_col = theme.border_accent;
    let sig_col  = theme.label;

    let col_color = |c: usize| -> Color {
        let p = (c as f64 + 0.5) / chart_w as f64;
        let d = (p - 0.5).abs() * 2.0; // 0 = centre, 1 = rail
        if d > 0.88 { if levels.clipping { theme.status_crit } else { theme.status_warn } }
        else if d > 0.62 { theme.value }
        else { theme.value_hi }
    };

    if maxc == 0 {
        out.push(Line::from(Span::styled(" no samples yet", lbl)));
    } else {
        let heights: Vec<f64> = cols.iter().map(|&c| c as f64 / maxc as f64).collect();
        for r in 0..chart_h {
            let rb = chart_h - 1 - r; // rows fill from the bottom up
            let mut spans: Vec<Span> = vec![Span::raw(" ")];
            for (c, &h) in heights.iter().enumerate() {
                // The caliper overlay wins over the bell fill at its column, so the
                // guides read as a full-height measuring caliper (peak before rms).
                if c == peak_lo || c == peak_hi {
                    spans.push(Span::styled("\u{254e}".to_string(), Style::default().fg(peak_col)));
                } else if c == sig_lo || c == sig_hi {
                    spans.push(Span::styled("\u{2506}".to_string(), Style::default().fg(sig_col)));
                } else {
                    let he = (h * chart_h as f64 * 8.0).round() as usize;
                    let cell = he.saturating_sub(rb * 8).min(8);
                    let ch = VBLOCKS[cell];
                    let color = if cell == 0 { dim } else { col_color(c) };
                    spans.push(Span::styled(ch.to_string(), Style::default().fg(color)));
                }
            }
            out.push(Line::from(spans));
        }
    }

    // x-axis: −FS … 0 … +FS under the bell.
    let mut axis: Vec<char> = vec![' '; chart_w];
    let place = |axis: &mut Vec<char>, at: usize, s: &str| {
        let start = at.min(chart_w.saturating_sub(s.chars().count()));
        for (k, ch) in s.chars().enumerate() {
            if start + k < chart_w { axis[start + k] = ch; }
        }
    };
    place(&mut axis, 0, "\u{2212}FS");
    place(&mut axis, chart_w / 2, "0");
    place(&mut axis, chart_w.saturating_sub(3), "+FS");
    out.push(Line::from(vec![
        Span::raw(" "),
        Span::styled(axis.into_iter().collect::<String>(), Style::default().fg(dim)),
    ]));

    // Caliper legend: the glyph keys coloured to match the guides — peak (loudest
    // sample) and the per-axis 1σ, each as a percent of full scale, so the line
    // positions tie to numbers. The combined-magnitude rms lives in LOADING below.
    if maxc == 0 {
        out.push(Line::raw(""));
    } else {
        out.push(Line::from(vec![
            Span::raw(" "),
            Span::styled("\u{254e}".to_string(), Style::default().fg(peak_col)),
            Span::styled(format!(" peak \u{00b1}{:.0}%", (levels.peak_frac * 100.0).min(100.0)), lbl),
            Span::styled("   \u{2506}".to_string(), Style::default().fg(sig_col)),
            Span::styled(format!(" 1\u{03c3} \u{00b1}{:.0}%", (levels.sigma_frac * 100.0).min(100.0)), lbl),
        ]));
    }
}

/// Fold the 32-bin signed histogram down to `w` display columns (sum into buckets),
/// preserving the centre-heavy bell shape at any panel width.
fn rebin(hist: &[u64; 32], w: usize) -> Vec<u64> {
    if w == 0 { return Vec::new(); }
    let mut cols = vec![0u64; w];
    for (i, &c) in hist.iter().enumerate() {
        let col = (i * w / 32).min(w - 1);
        cols[col] += c;
    }
    cols
}

/// Per-axis 1σ amplitude (fraction of full scale) from the combined-magnitude RMS
/// in dBFS. `adc_rms_dbfs` measures √(var_i+var_q) — the magnitude RMS — but the
/// signed histogram's axis is a single I/Q component, whose standard deviation is
/// that divided by √2 (balanced I/Q). Placing the inner caliper here brackets the
/// bell's actual spread instead of sitting √2 wider than it.
pub(super) fn per_axis_sigma_frac(rms_dbfs: f64) -> f64 {
    10f64.powf(rms_dbfs / 20.0) / std::f64::consts::SQRT_2
}

/// Caliper column pair for an amplitude fraction `a` (0..1 of full scale), placed
/// symmetrically about the centre of a `w`-wide axis that runs −FS … 0 … +FS.
/// Returns `(lo, hi)` display columns, clamped into range. `a = 1` lands on the
/// rails, `a = 0` collapses to the centre — so the guides measure how close the
/// signal comes to clipping, independent of where the bell's mass actually sits.
fn caliper_cols(a: f64, w: usize) -> (usize, usize) {
    if w == 0 { return (0, 0); }
    let a = a.clamp(0.0, 1.0);
    let col = |p: f64| ((p * w as f64).round() as isize).clamp(0, w as isize - 1) as usize;
    (col(0.5 - a / 2.0), col(0.5 + a / 2.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebin_preserves_total() {
        let mut hist = [0u64; 32];
        hist[16] = 100; hist[15] = 50; hist[0] = 7; hist[31] = 3;
        let cols = rebin(&hist, 10);
        assert_eq!(cols.iter().sum::<u64>(), 160, "rebin must not lose counts");
        assert_eq!(cols.len(), 10);
    }

    #[test]
    fn rebin_centre_heavy_input_peaks_in_middle() {
        let mut hist = [0u64; 32];
        hist[16] = 1000; // mid-scale (v ≈ 0)
        let cols = rebin(&hist, 8);
        let peak = cols.iter().enumerate().max_by_key(|(_, &c)| c).unwrap().0;
        assert!(peak >= 3 && peak <= 4, "centre bin should land mid-bell, got col {peak}");
    }

    #[test]
    fn rebin_zero_width_is_empty() {
        assert!(rebin(&[1u64; 32], 0).is_empty());
    }

    #[test]
    fn caliper_full_scale_lands_on_the_rails() {
        let (lo, hi) = caliper_cols(1.0, 20);
        assert_eq!((lo, hi), (0, 19), "a=1 (0 dBFS) sits on both rails");
    }

    #[test]
    fn caliper_zero_collapses_to_centre() {
        let (lo, hi) = caliper_cols(0.0, 20);
        assert_eq!(lo, hi, "a=0 collapses to a single centre column");
        assert_eq!(lo, 10);
    }

    #[test]
    fn caliper_is_symmetric_about_centre() {
        let w = 24;
        // −8 dBFS ≈ 0.398 of full scale.
        let a = 10f64.powf(-8.0 / 20.0);
        let (lo, hi) = caliper_cols(a, w);
        // Equal distance from the centre on both sides.
        let centre = w as f64 / 2.0;
        assert!(((centre - lo as f64) - (hi as f64 - centre)).abs() <= 1.0,
                "caliper columns symmetric about centre: lo={lo} hi={hi}");
        assert!(lo < w / 2 && hi > w / 2, "peak guides straddle the centre");
    }

    #[test]
    fn caliper_zero_width_is_safe() {
        assert_eq!(caliper_cols(0.5, 0), (0, 0));
    }

    #[test]
    fn per_axis_sigma_is_magnitude_rms_over_sqrt2() {
        // −18 dBFS magnitude RMS ≈ 0.1259 of FS; the per-axis 1σ is that / √2.
        let a = per_axis_sigma_frac(-18.0);
        let mag = 10f64.powf(-18.0 / 20.0);
        assert!((a - mag / std::f64::consts::SQRT_2).abs() < 1e-9, "got {a}");
        // The inner caliper sits strictly inside the old (√2-too-wide) placement.
        assert!(a < mag, "per-axis σ must be narrower than the magnitude RMS");
    }

    #[test]
    fn per_axis_sigma_inside_peak_for_typical_signal() {
        // A signal whose peak is 8 dB above its rms → the 1σ band sits inside the
        // peak envelope (the inner caliper never crosses the outer one).
        let a_peak  = 10f64.powf(-8.0 / 20.0);
        let a_sigma = per_axis_sigma_frac(-16.0);
        assert!(a_sigma < a_peak, "1σ {a_sigma} should be inside peak {a_peak}");
    }
}
