//! `AdcLoadingPanel` — the ADC Loading column of the Lab RF bench ([6]).
//!
//! Shows how hard the 8-bit ADC is actually driven: the **signed sample histogram**
//! (a centred bell whose tails light up as they approach the rails), the **clip
//! headroom** bar, the **loading** read-out (peak / rms / crest / effective bits /
//! clip events), and a **modeled linearity** card (P1dB / IIP3 / IMD3 / SFDR). The
//! thesis: fill the ADC window without hitting the rails — that is what positions the
//! signal/noise gap the other two panels draw.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::widgets::charts::gain_bar_colored;
use crate::ui::panel::Panel;
use crate::ui::rf_calc::{adc_loading, linearity, staging_verdict, OPT_PEAK_DBFS};

pub struct AdcLoadingPanel;

/// Vertical ⅛-block ramp for the histogram bell, 0 = blank … 8 = full cell.
const VBLOCKS: [char; 9] = [
    ' ', '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}',
    '\u{2585}', '\u{2586}', '\u{2587}', '\u{2588}',
];

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
fn per_axis_sigma_frac(rms_dbfs: f64) -> f64 {
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

impl Panel for AdcLoadingPanel {
    fn name(&self) -> &'static str { "adc_loading" }
    fn min_size(&self) -> (u16, u16) { (30, 18) }

    fn render(&self, f: &mut Frame, area: Rect, state: &SdrMetrics, theme: &crate::Theme, focused: bool) {
        let stale = !state.radio.hw_streaming;
        let mut title = vec![
            Span::raw(" "),
            Span::styled("ADC Loading",
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

        let iw  = inner.width as usize;
        let ih  = inner.height as usize;
        let dim = theme.border_dim;
        let lbl = Style::default().fg(theme.label);

        if stale {
            f.render_widget(
                Paragraph::new(Span::styled("\u{2014}\u{2014}\u{2014}", lbl)), inner);
            return;
        }

        // --- model (frozen snapshot when held, else live) ----------------------
        let fz = state.lab.rf_freeze.as_ref();
        let hist = fz.map(|f| &f.signed_hist).unwrap_or(&state.iq.adc_signed_hist);
        let n: u64 = hist.iter().sum();
        let peak = fz.map(|f| f.peak_dbfs).unwrap_or(state.signal.adc_peak_dbfs) as f64;
        let rms  = fz.map(|f| f.rms_dbfs).unwrap_or(state.signal.adc_rms_dbfs) as f64;
        let clip = fz.map(|f| f.clip_events).unwrap_or(state.signal.adc_clip_events);
        let (lna_g, vga_g) = fz.map(|f| (f.lna_gain, f.vga_gain))
            .unwrap_or((state.radio.lna_gain, state.radio.vga_gain));
        let load = adc_loading(peak, rms, clip, n);
        let (verdict, sev) = staging_verdict(peak);
        let sev_col = match sev {
            2 => theme.status_crit, 1 => theme.status_warn, _ => theme.status_ok,
        };
        let clipping = load.clip_events > 0 || peak >= -1.0;

        // --- helpers (shared visual language with the RF Diagnostics panel) -----
        let section = |name: &str, hint: &str| -> Line<'static> {
            let label = name.to_uppercase();
            let left = label.chars().count() + 5;
            let hint_w = if hint.is_empty() { 0 } else { hint.chars().count() + 1 };
            let dashes = iw.saturating_sub(left + hint_w);
            let mut spans = vec![
                Span::styled("\u{251c}\u{2574} ".to_string(), Style::default().fg(dim)),
                Span::styled(label, Style::default().fg(theme.label).add_modifier(Modifier::BOLD)),
                Span::styled(" \u{2576}".to_string(), Style::default().fg(dim)),
                Span::styled("\u{2500}".repeat(dashes), Style::default().fg(dim)),
            ];
            if !hint.is_empty() {
                spans.push(Span::styled(format!(" {hint}"), Style::default().fg(dim)));
            }
            Line::from(spans)
        };
        // " LBL  mid............ right" — label + a mid column + a right-aligned value.
        let row3 = |label: &str, mid: String, mid_col: Color, right: String, right_col: Color| -> Line<'static> {
            let pad = iw.saturating_sub(1 + 4 + 1 + mid.chars().count() + right.chars().count());
            Line::from(vec![
                Span::raw(" "),
                Span::styled(format!("{label:<4}"), lbl),
                Span::raw(" "),
                Span::styled(mid, Style::default().fg(mid_col)),
                Span::raw(" ".repeat(pad.max(1))),
                Span::styled(right, Style::default().fg(right_col).add_modifier(Modifier::BOLD)),
            ])
        };

        let mut lines: Vec<Line> = Vec::new();

        // --- HISTOGRAM BELL (signed-sample distribution) -----------------------
        // The panel's hero visual: it grows to fill all the height the fixed read-out
        // blocks below leave free, instead of capping low and stranding empty rows.
        // Caliper guides overlay the loudest-sample (peak) and bulk (rms) reach,
        // mapped symmetrically about true 0 — so the bell reads as a measured
        // instrument, and a DC offset shows up as the bell sitting off the calipers.
        let chart_w = iw.saturating_sub(1).max(8);
        // Rows reserved below the bell: axis + caliper legend + the 3 read-out
        // sections (HEADROOM / LOADING / LINEARITY, with their spacers and caption).
        const BELOW: usize = 17;
        let chart_h = ih.saturating_sub(BELOW).clamp(3, 28);
        let cols = rebin(hist, chart_w);
        let maxc  = cols.iter().copied().max().unwrap_or(0);

        let col_color = |c: usize| -> Color {
            let p = (c as f64 + 0.5) / chart_w as f64;
            let d = (p - 0.5).abs() * 2.0; // 0 = centre, 1 = rail
            if d > 0.88 { if clipping { theme.status_crit } else { theme.status_warn } }
            else if d > 0.62 { theme.value }
            else { theme.value_hi }
        };

        // Caliper columns from the live levels. Peak is the bright outer caliper at
        // the loudest per-axis sample (peak_amp is already a single-component max, so
        // ±a_peak is its true envelope); the inner band marks the per-axis 1σ of the
        // distribution. Both use cool colours so they stay legible over the warm fill.
        let a_peak  = 10f64.powf(peak / 20.0);
        let a_sigma = per_axis_sigma_frac(rms);
        let (peak_lo, peak_hi) = caliper_cols(a_peak,  chart_w);
        let (sig_lo,  sig_hi)  = caliper_cols(a_sigma, chart_w);
        let peak_col = theme.border_accent;
        let sig_col  = theme.label;

        if maxc == 0 {
            lines.push(Line::from(Span::styled(" no samples yet", lbl)));
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
                lines.push(Line::from(spans));
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
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(axis.into_iter().collect::<String>(), Style::default().fg(dim)),
        ]));
        // Caliper legend: the glyph keys coloured to match the guides — peak (loudest
        // sample) and the per-axis 1σ, each as a percent of full scale, so the line
        // positions tie to numbers. The combined-magnitude rms lives in LOADING below.
        if maxc == 0 {
            lines.push(Line::raw(""));
        } else {
            lines.push(Line::from(vec![
                Span::raw(" "),
                Span::styled("\u{254e}".to_string(), Style::default().fg(peak_col)),
                Span::styled(format!(" peak \u{00b1}{:.0}%", (a_peak * 100.0).min(100.0)), lbl),
                Span::styled("   \u{2506}".to_string(), Style::default().fg(sig_col)),
                Span::styled(format!(" 1\u{03c3} \u{00b1}{:.0}%", (a_sigma * 100.0).min(100.0)), lbl),
            ]));
        }
        lines.push(Line::raw(""));

        // --- CLIP HEADROOM -----------------------------------------------------
        // Headroom = how far the peak sits below 0 dBFS; the tick marks the optimal
        // landing (−OPT_PEAK_DBFS, i.e. 8 dB of headroom).
        let headroom = -peak;
        const HDRM_MAX: f64 = 24.0;
        const VALW: usize = 11;
        let bar_w = iw.saturating_sub(1 + 4 + 1 + 1 + VALW).max(6);
        lines.push(section("Headroom", "\u{2502} = optimal"));
        let mut bar = gain_bar_colored(headroom.clamp(0.0, HDRM_MAX) as u32, HDRM_MAX as u32,
                                       bar_w, theme.status_warn, theme.status_ok, dim);
        let tick = ((-OPT_PEAK_DBFS / HDRM_MAX).clamp(0.0, 1.0) * bar_w as f64).round() as usize;
        let tick = tick.min(bar_w.saturating_sub(1));
        if tick < bar.len() {
            bar[tick] = Span::styled("\u{250a}".to_string(), Style::default().fg(theme.value_hi));
        }
        let mut hdrm_spans = vec![Span::raw(" "), Span::styled(format!("{:<4}", "HDRM"), lbl), Span::raw(" ")];
        hdrm_spans.extend(bar);
        hdrm_spans.push(Span::raw(" "));
        hdrm_spans.push(Span::styled(format!("{headroom:+.0} dB"),
                        Style::default().fg(sev_col).add_modifier(Modifier::BOLD)));
        lines.push(Line::from(hdrm_spans));
        lines.push(Line::raw(""));

        // --- LOADING -----------------------------------------------------------
        lines.push(section("Loading", "peak / rms"));
        lines.push(row3("peak", format!("{:.0} dBFS", load.peak_dbfs), sev_col,
                        format!("{}/127 cts", load.peak_counts), theme.value));
        lines.push(row3("rms", format!("{:.0} dBFS", load.rms_dbfs), theme.value,
                        format!("crest {:.1} dB", load.crest_db), theme.value));
        lines.push(row3("bits", format!("{:.1} / 8 eff", load.enob), theme.value_hi,
                        "ENOB".to_string(), dim));
        let (clip_txt, clip_col) = if load.clip_events == 0 {
            ("none".to_string(), theme.status_ok)
        } else {
            (format!("{} hits", load.clip_events), theme.status_crit)
        };
        let n_txt = if load.n >= 1000 { format!("{}k", load.n / 1000) } else { format!("{}", load.n) };
        lines.push(row3("clip", format!("{clip_txt} / {n_txt}"), clip_col,
                        verdict.to_string(), sev_col));
        lines.push(Line::raw(""));

        // --- LINEARITY (modeled) -----------------------------------------------
        let lin = linearity(lna_g, vga_g);
        lines.push(section("Linearity", "modeled"));
        lines.push(row3("P1dB", format!("{:.0} dB hdrm", lin.p1db_headroom_db), theme.value,
                        "compression".to_string(), dim));
        lines.push(row3("IIP3", format!("{:+.0} dBm", lin.iip3_dbm), theme.value,
                        format!("IMD3 {:.0} dBc", lin.imd3_dbc), theme.value));
        lines.push(row3("SFDR", format!("{:.0} dB", lin.sfdr_db), theme.value_hi,
                        format!("8-bit \u{2264}{:.0}", lin.sfdr_limit_db), dim));

        // Teaching caption.
        lines.push(Line::from(Span::styled(
            " fill the range without hitting the rails", Style::default().fg(dim))));

        // Self-adjusting density: collapse spacers when short, grow them to fill when
        // tall (chrome::fit_spacers), so the pane breathes the same at every height —
        // consistent with the other lab side panels.
        crate::ui::chrome::fit_spacers(&mut lines, ih);
        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_name_and_min_size() {
        let p = AdcLoadingPanel;
        assert_eq!(p.name(), "adc_loading");
        let (w, h) = p.min_size();
        assert!(w >= 16 && h >= 8);
    }

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
