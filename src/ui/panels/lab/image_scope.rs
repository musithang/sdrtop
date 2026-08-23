//! `ImageScopePanel` — the LO-centred image-rejection scope for the Lab IQ preset.
//!
//! Reads the existing fftshifted FFT frame (DC/LO at the centre bin) and tells the
//! quadrature story directly: the strongest **carrier**, its **mirror image**
//! reflected about the LO, and the residual **DC spike** at the centre. The gap
//! between carrier and image is the measured image suppression — the empirical
//! counterpart to the computed IRR shown one panel left.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

use crate::state::{FftFrame, SdrMetrics};
use crate::ui::panel::Panel;

pub struct ImageScopePanel;

/// Carrier + mirror image read-out in absolute terms, for the Lab IQ marker bar.
pub(crate) struct CarrierImage {
    pub carrier_hz:     u64,
    pub image_hz:       u64,
    pub carrier_dbfs:   f32,
    pub image_dbfs:     f32,
    /// carrier − image, in dB (positive = image is below the carrier).
    pub suppression_db: f32,
}

/// Resolve the carrier and its LO-mirror image into absolute frequencies + levels,
/// honouring a placed marker / pin as the carrier (see [`carrier_hint_bin`]).
/// Shared by the scope panel and the marker bar so both tell the same story.
/// `None` when there is no frame yet or it is too small / silent.
pub(crate) fn carrier_image(state: &SdrMetrics) -> Option<CarrierImage> {
    let frame = state.waterfall.last_fft.as_ref()?;
    let hint = carrier_hint_bin(state, frame);
    let r = detect_image(&frame.bins_dbfs, frame.sample_rate, frame.noise_floor, hint)?;
    let center = frame.center_freq_hz as f64;
    Some(CarrierImage {
        carrier_hz:     (center + r.carrier_offset_hz).round() as u64,
        image_hz:       (center - r.carrier_offset_hz).round() as u64,
        carrier_dbfs:   r.carrier_dbfs,
        image_dbfs:     r.image_dbfs,
        suppression_db: r.suppression_db,
    })
}

/// Fixed dBFS window for the bar chart — a stable axis (0 at top, −120 at the
/// floor) reads better than an auto-ranging one when comparing two peaks.
const FLOOR_DBFS: f32 = -120.0;
const TOP_DBFS:   f32 = 0.0;

/// Partial-cell block ramp: index 0 = empty, 8 = full cell.
const BLOCKS: [char; 9] = [' ', '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}',
                           '\u{2585}', '\u{2586}', '\u{2587}', '\u{2588}'];

/// Dim an `Rgb` colour's brightness by `f`. Non-Rgb colours pass through.
fn dim(c: Color, f: f32) -> Color {
    match c {
        Color::Rgb(r, g, b) =>
            Color::Rgb((r as f32 * f) as u8, (g as f32 * f) as u8, (b as f32 * f) as u8),
        other => other,
    }
}

/// Carrier / image / DC read-out derived from one fftshifted FFT frame.
struct ImageReadout {
    carrier_idx:       usize,
    carrier_dbfs:      f32,
    image_dbfs:        f32,
    dc_dbfs:           f32,
    /// carrier − image, in dB (positive = image is below the carrier).
    suppression_db:    f32,
    /// Carrier offset from the LO, signed (Hz).
    carrier_offset_hz: f64,
}

/// Map an absolute frequency to a bin index in the fftshifted frame, or `None` if
/// it falls outside the captured span. Uses the canonical fftshift convention
/// (`bin = n/2 + (f − f_c)·n/rate`, bin `n/2` = DC), the exact inverse of the
/// per-bin frequency the scope and `carrier_offset_hz` use — so a frequency and
/// its bin round-trip without the off-by-one an `(n−1)` mapping introduces.
fn freq_to_bin(freq_hz: u64, center_freq_hz: u64, sample_rate: f64, n: usize) -> Option<usize> {
    if n == 0 || sample_rate <= 0.0 { return None; }
    let bin_hz = sample_rate / n as f64;
    let b = (n as f64 / 2.0 + (freq_hz as f64 - center_freq_hz as f64) / bin_hz).round();
    if b < 0.0 || b >= n as f64 { return None; }
    Some(b as usize)
}

/// How far above the noise floor (dB) the strongest bin must sit before the auto
/// path treats it as a carrier. Below this there is no signal to measure — only
/// noise — so the scope reports "no carrier" rather than a random noise peak.
/// A placed marker / pin bypasses this gate.
const CARRIER_MIN_SNR_DB: f32 = 10.0;

/// Locate the carrier, its mirror about the centre (LO) bin, and the DC-spike
/// level. The carrier is `carrier_hint` when supplied and valid (a placed marker
/// or pin); otherwise the strongest bin outside a small DC guard, when it clears
/// the noise floor by [`CARRIER_MIN_SNR_DB`]. `None` when the
/// frame is too small / silent. Pure + deterministic for unit testing.
fn detect_image(bins: &[f32], sample_rate: f64, noise_floor: f32, carrier_hint: Option<usize>)
    -> Option<ImageReadout>
{
    let n = bins.len();
    if n < 8 { return None; }
    let center = n / 2;
    let guard  = (n / 64).max(2);
    let in_band = |i: usize| i < n && (i as isize - center as isize).unsigned_abs() > guard;

    let carrier_idx = match carrier_hint {
        // Honour a hinted carrier (marker / pin) when it lands in a usable band,
        // regardless of strength — the operator may be probing a deliberately
        // weak signal, so explicit intent overrides the noise gate below.
        Some(h) if in_band(h) => h,
        // Auto path: the strongest bin outside the DC guard, but only when it
        // stands clear of the noise floor. With no real carrier present (just
        // noise), the loudest bin is a random noise peak — reporting it as a
        // "carrier" would show a meaningless, alarming image-suppression figure
        // and make the scope's frequency axis jitter frame to frame. Treat that
        // as "no signal" instead.
        _ => {
            let mut idx = center;
            let mut best = f32::NEG_INFINITY;
            for (i, &v) in bins.iter().enumerate() {
                if !in_band(i) { continue; }
                if v > best { best = v; idx = i; }
            }
            if best < noise_floor + CARRIER_MIN_SNR_DB { return None; }
            idx
        }
    };

    let image_idx = (2 * center).saturating_sub(carrier_idx).min(n - 1);
    let bin_hz = sample_rate / n as f64;
    Some(ImageReadout {
        carrier_idx,
        carrier_dbfs:      bins[carrier_idx],
        image_dbfs:        bins[image_idx],
        dc_dbfs:           bins[center],
        suppression_db:    bins[carrier_idx] - bins[image_idx],
        carrier_offset_hz: (carrier_idx as f64 - center as f64) * bin_hz,
    })
}

/// Resolve the carrier bin from operator intent, in priority order:
/// 1. an explicit `[M]` pin, 2. the strongest **placed spectrum marker**, else
/// `None` so [`detect_image`] auto-picks the strongest bin. This is what makes a
/// marker you set on the spectrum actually drive the image calculation.
fn carrier_hint_bin(state: &SdrMetrics, frame: &FftFrame) -> Option<usize> {
    let n = frame.bins_dbfs.len();
    let to_bin = |f: u64| freq_to_bin(f, frame.center_freq_hz, frame.sample_rate, n);

    if let Some((carrier_hz, _)) = state.lab.iq_marker_pin {
        if let Some(b) = to_bin(carrier_hz) { return Some(b); }
    }
    state.spectrum.markers.iter()
        .filter_map(|m| to_bin(m.freq_hz).map(|b| (b, frame.bins_dbfs[b])))
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(b, _)| b)
}

/// Colour for an image-suppression figure: deeper is better.
fn supp_color(supp_db: f32, theme: &crate::Theme) -> Color {
    if supp_db >= 40.0      { theme.status_ok   }
    else if supp_db >= 20.0 { theme.status_warn }
    else                    { theme.status_crit }
}

fn fmt_mhz2(hz: f64) -> String { format!("{:.2}M", hz / 1e6) }

impl Panel for ImageScopePanel {
    fn name(&self) -> &'static str { "image_scope" }
    fn min_size(&self) -> (u16, u16) { (28, 12) }

    fn render(&self, f: &mut Frame, area: Rect, state: &SdrMetrics, theme: &crate::Theme, focused: bool) {
        let stale = !state.radio.hw_streaming;
        let mut title_spans = vec![
            Span::raw(" "),
            Span::styled("Image-Rejection Scope",
                         Style::default().fg(theme.label).add_modifier(Modifier::BOLD)),
        ];
        if stale {
            title_spans.push(Span::styled(" [STALE]", Style::default().fg(theme.stale)));
        }
        title_spans.push(Span::raw(" "));
        // Match the other Lab IQ panels (border_default is the documented colour for
        // iq_* panels) so the [5] bench reads as one unit, not a spectrum offcut.
        let border_color = if focused { theme.border_focused }
            else if stale { theme.stale }
            else { theme.border_default };
        let block = Block::default()
            .title(Line::from(title_spans))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border_color));
        let inner = block.inner(area);
        f.render_widget(block, area);

        if stale {
            f.render_widget(
                Paragraph::new(Span::styled("\u{2014}\u{2014}\u{2014}", Style::default().fg(theme.label))),
                inner,
            );
            return;
        }

        let Some(frame) = state.waterfall.last_fft.as_ref() else {
            f.render_widget(
                Paragraph::new(Span::styled("Waiting for RX\u{2026}", Style::default().fg(theme.label))),
                inner,
            );
            return;
        };
        let hint   = carrier_hint_bin(state, frame);
        let bins   = &frame.bins_dbfs;
        let center = frame.center_freq_hz as f64;
        let rate   = frame.sample_rate;
        let Some(r) = detect_image(bins, rate, frame.noise_floor, hint) else {
            f.render_widget(
                Paragraph::new(Span::styled("No signal yet\u{2026}", Style::default().fg(theme.label))),
                inner,
            );
            return;
        };

        let dimc   = theme.border_dim;
        let lbl     = Style::default().fg(theme.label);
        let base_col = dim(theme.border_accent, 0.5);
        let car_col  = theme.value_hi;
        let img_col  = theme.status_warn;
        let dc_col_c = theme.status_crit;

        // LO-centred window wide enough to hold both carrier and its mirror.
        let off  = r.carrier_offset_hz.abs().max(rate * 0.04);
        let span = (off * 1.5).min(rate / 2.0).max(1.0);
        let left_hz  = center - span;
        let right_hz = center + span;
        let width_hz = (right_hz - left_hz).max(1.0);

        let iw = inner.width as usize;
        let ih = inner.height as usize;
        let n  = bins.len();
        let bin_hz = rate / n as f64;

        let carrier_f = center + r.carrier_offset_hz;
        let image_f   = center - r.carrier_offset_hz;

        // Readout block (always shown); the bar chart fills whatever height is left.
        // Image level relative to the carrier: normally negative (image below); a
        // positive value flags that the "carrier" is weaker than its mirror.
        let supp_c = supp_color(r.suppression_db, theme);
        let rel = r.image_dbfs - r.carrier_dbfs;
        let rel_str = if rel <= 0.0 { format!("\u{2212}{:.1} dB", -rel) }
                      else          { format!("+{rel:.1} dB") };
        // Width-aware readouts: the frequency/level is essential, the parenthetical
        // ("(mirror)", "(I/Q offset)") is decoration. On a narrow scope (the 32% slot
        // is often < 33 cols of inner width) drop the trailing decoration rather than
        // clipping a level mid-word. Each line keeps its first three spans.
        let line_w = |spans: &[Span]| spans.iter().map(|s| s.content.chars().count()).sum::<usize>();
        let fit = |mut spans: Vec<Span<'static>>| -> Line<'static> {
            while spans.len() > 3 && line_w(&spans) > iw { spans.pop(); }
            Line::from(spans)
        };
        let readouts: Vec<Line> = vec![
            fit(vec![
                Span::styled(" \u{25bc} ", Style::default().fg(car_col)),
                Span::styled("CARRIER ", lbl),
                Span::styled(format!("{:.3} MHz", carrier_f / 1e6),
                             Style::default().fg(car_col).add_modifier(Modifier::BOLD)),
                Span::styled(format!(" \u{00b7} {:.1} dBFS", r.carrier_dbfs), Style::default().fg(dimc)),
            ]),
            fit(vec![
                Span::styled(" \u{25bc} ", Style::default().fg(img_col)),
                Span::styled("IMAGE ", lbl),
                Span::styled(format!("\u{00b7} {:.1} dBFS", r.image_dbfs),
                             Style::default().fg(img_col)),
                Span::styled(" (mirror)", lbl),
            ]),
            fit(vec![
                Span::styled(" \u{25ae} ", Style::default().fg(dc_col_c)),
                Span::styled("DC spike ", lbl),
                Span::styled(format!("\u{00b7} {:.1} dBFS", r.dc_dbfs), Style::default().fg(dc_col_c)),
                Span::styled(" (I/Q offset)", Style::default().fg(dimc)),
            ]),
            Line::from(vec![
                Span::raw(" "),
                Span::styled("image supp. ", lbl),
                Span::styled(rel_str,
                             Style::default().fg(supp_c).add_modifier(Modifier::BOLD)),
            ]),
        ];

        let mut lines: Vec<Line> = Vec::new();

        // Bar chart, if there's room. Gutter holds dBFS tick labels; one marker row
        // sits above the bars; a freq-axis row sits below.
        let gutter = 4usize;                       // "−120"
        let chart_w = iw.saturating_sub(gutter + 1);
        let reserved = 1 /*marker*/ + 1 /*axis*/ + 1 /*gap*/ + readouts.len() + 1 /*caption*/;
        // Fill all the vertical room left after the chrome — a taller chart gives
        // finer dBFS resolution and matches the mockup's full-height scope.
        let chart_h = ih.saturating_sub(reserved);

        if chart_w >= 8 && chart_h >= 3 {
            // Aggregate bins into columns (peak per column), single O(n) pass.
            let mut col_level = vec![FLOOR_DBFS; chart_w];
            for (i, &v) in bins.iter().enumerate() {
                let bf = center + (i as f64 - n as f64 / 2.0) * bin_hz;
                if bf < left_hz || bf >= right_hz { continue; }
                let c = (((bf - left_hz) / width_hz) * chart_w as f64) as usize;
                let c = c.min(chart_w - 1);
                if v > col_level[c] { col_level[c] = v; }
            }

            let freq_to_col = |fhz: f64| -> Option<usize> {
                if fhz < left_hz || fhz >= right_hz { return None; }
                Some((((fhz - left_hz) / width_hz) * chart_w as f64) as usize).map(|c| c.min(chart_w - 1))
            };
            let carrier_col = freq_to_col(carrier_f);
            let image_col   = freq_to_col(image_f);
            let dc_col      = freq_to_col(center);
            let col_color = |c: usize| -> Color {
                if Some(c) == carrier_col      { car_col }
                else if Some(c) == image_col   { img_col }
                else if Some(c) == dc_col      { dc_col_c }
                else                           { base_col }
            };

            // Marker row: ▼ over carrier/image, ▮ over DC.
            let mut mk: Vec<Span> = vec![Span::raw(" ".repeat(gutter + 1))];
            let mut run = String::new();
            let mut run_col = base_col;
            let flush = |run: &mut String, col: Color, out: &mut Vec<Span>| {
                if !run.is_empty() { out.push(Span::styled(std::mem::take(run), Style::default().fg(col))); }
            };
            for c in 0..chart_w {
                let (ch, col) =
                    if Some(c) == carrier_col      { ('\u{25bc}', car_col) }
                    else if Some(c) == image_col   { ('\u{25bc}', img_col) }
                    else if Some(c) == dc_col      { ('\u{25ae}', dc_col_c) }
                    else                           { (' ', base_col) };
                if col != run_col { flush(&mut run, run_col, &mut mk); run_col = col; }
                run.push(ch);
            }
            flush(&mut run, run_col, &mut mk);
            lines.push(Line::from(mk));

            // dBFS gridline label rows.
            let tick_row = |db: f32| -> usize {
                (((TOP_DBFS - db) / (TOP_DBFS - FLOOR_DBFS)) * (chart_h - 1) as f32).round() as usize
            };
            let mut row_label = vec![String::new(); chart_h];
            for db in [0.0, -30.0, -60.0, -90.0, -120.0] {
                let row = tick_row(db).min(chart_h - 1);
                if row_label[row].is_empty() { row_label[row] = format!("{:>width$}", db as i32, width = gutter); }
            }

            // Bar rows, top → bottom.
            for row in 0..chart_h {
                let mut spans: Vec<Span> = Vec::new();
                let g = &row_label[row];
                if g.is_empty() {
                    spans.push(Span::styled(format!("{:>gutter$}\u{2502}", "", gutter = gutter), Style::default().fg(dimc)));
                } else {
                    spans.push(Span::styled(format!("{g}\u{2524}"), Style::default().fg(dimc)));
                }
                let from_bottom = (chart_h - 1 - row) as f32;
                let mut run = String::new();
                let mut run_col = base_col;
                let mut started = false;
                for c in 0..chart_w {
                    let frac = ((col_level[c] - FLOOR_DBFS) / (TOP_DBFS - FLOOR_DBFS)).clamp(0.0, 1.0);
                    let cell = frac * chart_h as f32 - from_bottom;
                    let ch = if cell >= 1.0 { '\u{2588}' }
                             else if cell <= 0.05 { ' ' }
                             else { BLOCKS[(cell * 8.0).round().clamp(1.0, 8.0) as usize] };
                    let col = if ch == ' ' { base_col } else { col_color(c) };
                    if !started { run_col = col; started = true; }
                    if col != run_col {
                        spans.push(Span::styled(std::mem::take(&mut run), Style::default().fg(run_col)));
                        run_col = col;
                    }
                    run.push(ch);
                }
                spans.push(Span::styled(run, Style::default().fg(run_col)));
                lines.push(Line::from(spans));
            }

            // Frequency axis row.
            let mut axis: Vec<char> = vec![' '; chart_w];
            let write = |buf: &mut Vec<char>, at: usize, s: &str| {
                for (k, ch) in s.chars().enumerate() {
                    if at + k < buf.len() { buf[at + k] = ch; }
                }
            };
            let lo_lbl = format!("{} LO", fmt_mhz2(center));
            write(&mut axis, 0, &fmt_mhz2(left_hz));
            let cen = chart_w.saturating_sub(lo_lbl.chars().count()) / 2;
            write(&mut axis, cen, &lo_lbl);
            let r_lbl = fmt_mhz2(right_hz);
            write(&mut axis, chart_w.saturating_sub(r_lbl.chars().count()), &r_lbl);
            lines.push(Line::from(vec![
                Span::raw(" ".repeat(gutter + 1)),
                Span::styled(axis.into_iter().collect::<String>(), Style::default().fg(dimc)),
            ]));

            lines.push(Line::raw(""));
        }

        lines.extend(readouts);
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled("image mirrors the carrier about the LO", Style::default().fg(dimc)),
        ]));

        // Self-adjusting density: drop only as many airy spacers as needed to fit,
        // spread evenly, so a short pane keeps balanced breathing room. (chrome)
        crate::ui::chrome::collapse_spacers(&mut lines, ih);
        let _ = r.carrier_idx; // (kept for tests / future cursor)
        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(n: usize) -> Vec<f32> { vec![-110.0; n] }

    #[test]
    fn detect_image_finds_carrier_and_mirror() {
        let mut b = frame(64);
        b[40] = -8.0;    // carrier, +8 bins from centre (32)
        b[24] = -64.0;   // its mirror, −8 bins
        b[32] = -20.0;   // DC spike
        let r = detect_image(&b, 64.0, -110.0, None).unwrap();
        assert_eq!(r.carrier_idx, 40);
        assert!((r.carrier_dbfs - (-8.0)).abs() < 1e-6);
        assert!((r.image_dbfs - (-64.0)).abs() < 1e-6);
        assert!((r.dc_dbfs - (-20.0)).abs() < 1e-6);
        assert!((r.suppression_db - 56.0).abs() < 1e-6);
        assert!((r.carrier_offset_hz - 8.0).abs() < 1e-6, "off {}", r.carrier_offset_hz);
    }

    #[test]
    fn detect_image_ignores_dc_spike_as_carrier() {
        // A tall DC spike inside the guard band must not be picked as the carrier.
        let mut b = frame(64);
        b[32] = 0.0;     // huge DC, at centre
        b[44] = -12.0;   // the real carrier
        let r = detect_image(&b, 64.0, -110.0, None).unwrap();
        assert_eq!(r.carrier_idx, 44, "carrier should skip the guarded DC bin");
    }

    #[test]
    fn detect_image_honours_carrier_hint() {
        // A weaker bin chosen by a marker must override the strongest-bin auto-pick.
        let mut b = frame(64);
        b[50] = -4.0;    // strongest peak (would auto-win)
        b[40] = -18.0;   // the bin the operator marked
        let r = detect_image(&b, 64.0, -110.0, Some(40)).unwrap();
        assert_eq!(r.carrier_idx, 40, "hint should drive the carrier");
        assert_eq!((2 * 32usize) - 40, 24);
        assert!((r.carrier_dbfs - (-18.0)).abs() < 1e-6);
    }

    #[test]
    fn detect_image_invalid_hint_falls_back_to_auto() {
        let mut b = frame(64);
        b[50] = -4.0;
        // A hint inside the DC guard is rejected → auto-pick the strongest bin.
        let r = detect_image(&b, 64.0, -110.0, Some(33)).unwrap();
        assert_eq!(r.carrier_idx, 50);
        // An out-of-range hint is likewise ignored.
        let r2 = detect_image(&b, 64.0, -110.0, Some(999)).unwrap();
        assert_eq!(r2.carrier_idx, 50);
    }

    #[test]
    fn freq_to_bin_maps_endpoints_and_centre() {
        // 64-bin frame, 64 Hz span centred on 1000 Hz → 1 Hz/bin, left edge = 968.
        assert_eq!(freq_to_bin(968, 1000, 64.0, 64), Some(0));
        assert_eq!(freq_to_bin(1000, 1000, 64.0, 64), Some(32)); // centre = n/2
        assert_eq!(freq_to_bin(2000, 1000, 64.0, 64), None);     // out of span
    }

    #[test]
    fn detect_image_too_small_is_none() {
        assert!(detect_image(&frame(4), 64.0, -110.0, None).is_none());
    }

    #[test]
    fn detect_image_gates_noise_when_no_carrier() {
        // Only noise: the loudest in-band bin sits a few dB over the floor, below
        // the SNR gate → auto-detection reports no carrier (not a noise peak).
        let mut b = frame(64);            // floor -110
        b[40] = -104.0;                   // a 6 dB noise bump, under the 10 dB gate
        assert!(detect_image(&b, 64.0, -110.0, None).is_none(),
            "a sub-gate noise peak must not be reported as a carrier");
        // The same weak bin is still measured when the operator marks it explicitly.
        let r = detect_image(&b, 64.0, -110.0, Some(40)).unwrap();
        assert_eq!(r.carrier_idx, 40, "an explicit hint bypasses the noise gate");
        // A real carrier well above the floor passes the auto gate.
        b[44] = -70.0;
        let r2 = detect_image(&b, 64.0, -110.0, None).unwrap();
        assert_eq!(r2.carrier_idx, 44);
    }

    #[test]
    fn supp_color_thresholds() {
        let t = crate::theme::Theme::sdr();
        assert_eq!(supp_color(50.0, &t), t.status_ok);
        assert_eq!(supp_color(30.0, &t), t.status_warn);
        assert_eq!(supp_color(10.0, &t), t.status_crit);
    }
}
