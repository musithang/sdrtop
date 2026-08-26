//! Shared building blocks for the micro ecosystem panels (`micro_panel`,
//! `micro_signal`, `micro_gain`, `micro_health`). Keeps the status colors,
//! formatters, the status badge, and the character bar in one place so the four
//! panels stay visually consistent.

use ratatui::{
    style::{Color, Style},
    text::Span,
};

use crate::state::SdrMetrics;

// ── Status colors ───────────────────────────────────────────────────────────

pub fn snr_color(db: f32, theme: &crate::Theme) -> Color {
    if db >= 20.0 {
        theme.status_ok
    } else if db >= 10.0 {
        theme.status_warn
    } else {
        theme.status_crit
    }
}
pub fn sat_color(pct: f32, theme: &crate::Theme) -> Color {
    if pct < 1.0 {
        theme.status_ok
    } else if pct < 5.0 {
        theme.status_warn
    } else {
        theme.status_crit
    }
}
pub fn drop_color(drops: u64, theme: &crate::Theme) -> Color {
    if drops == 0 {
        theme.status_ok
    } else if drops < 10 {
        theme.status_warn
    } else {
        theme.status_crit
    }
}
pub fn buf_color(pct: f32, theme: &crate::Theme) -> Color {
    if pct < 50.0 {
        theme.status_ok
    } else if pct < 80.0 {
        theme.status_warn
    } else {
        theme.status_crit
    }
}

// ── Formatters ──────────────────────────────────────────────────────────────

pub fn fmt_rbw(hz: f64) -> String {
    if hz >= 1_000.0 {
        format!("{:.1} kHz", hz / 1_000.0)
    } else {
        format!("{:.0} Hz", hz)
    }
}

pub fn fmt_freq_mhz(hz: u64) -> String {
    format!("{:.3} MHz", hz as f64 / 1_000_000.0)
}

/// `1.500 MHz` / `152.0 kHz` / `400 Hz` — the one bandwidth readout.
///
/// There were five of these, in `input.rs` (twice, once inline), `signal_metrics`,
/// `signal_characterization` and here, and they disagreed: 180 kHz of occupied
/// bandwidth read as `180 kHz` on the micro views and `180.0 kHz` on the lab panel,
/// and the characterization panel and its own `[C]` snapshot printed the same number
/// two different ways.
///
/// This precision is the lab one, kept because it is the honest one: roughly four
/// significant figures at every scale, so the readout resolves to about 1 kHz on a
/// megahertz-wide signal and 100 Hz on a kilohertz-wide one. An FFT bin is ~488 Hz
/// at 2 Msps, so those digits are measured, not invented — while `{} kHz` integer
/// truncation was throwing away resolution the analyser actually has.
pub fn fmt_bw(hz: u64) -> String {
    if hz >= 1_000_000 {
        format!("{:.3} MHz", hz as f64 / 1e6)
    } else if hz >= 1_000 {
        format!("{:.1} kHz", hz as f64 / 1e3)
    } else {
        format!("{hz} Hz")
    }
}

// ── Shared spans ────────────────────────────────────────────────────────────

/// `● RX` (green) when streaming, `○ IDLE` (yellow) otherwise. Two spans: the
/// dot and the word, both carrying the status color so they read on monochrome.
pub fn status_badge(state: &SdrMetrics, theme: &crate::Theme) -> [Span<'static>; 2] {
    let (dot, col, word) = if state.radio.rx_enabled {
        ("●", theme.status_ok, " RX")
    } else {
        ("○", theme.status_warn, " IDLE")
    };
    [
        Span::styled(dot, Style::default().fg(col)),
        Span::styled(word, Style::default().fg(col)),
    ]
}

/// Whether FFT-derived signal data (SNR, PWR, NF, RBW) is stale — no frame, or
/// the last one older than the shared threshold.
///
/// The same rule the engine tags a title `[STALE]` with, so a panel's contents
/// and its nameplate can never disagree about whether the numbers are live.
pub fn fft_stale(state: &SdrMetrics) -> bool {
    crate::ui::panel::Staleness::FftAge.resolve(state)
}

/// A character bar `████░░░░` of `width` cells: filled (in `color`) for `ratio`,
/// empty (dim) for the rest. Two spans.
pub fn bar_spans(
    ratio: f64,
    width: usize,
    color: Color,
    theme: &crate::Theme,
) -> [Span<'static>; 2] {
    let ratio = ratio.clamp(0.0, 1.0);
    let filled = (ratio * width as f64).round() as usize;
    let empty = width.saturating_sub(filled);
    [
        Span::styled("█".repeat(filled), Style::default().fg(color)),
        Span::styled("░".repeat(empty), Style::default().fg(theme.border_dim)),
    ]
}

/// Block-character ticks for inline sparklines, low → high.
const SPARK_TICKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Inline sparkline string from the most recent `width` samples, auto-scaled to
/// the window maximum. An all-zero (or empty-window) input renders as the
/// lowest tick so the row still reads as a flat baseline.
pub fn sparkline(samples: &[f64], width: usize) -> String {
    if samples.is_empty() || width == 0 {
        return String::new();
    }
    let start = samples.len().saturating_sub(width);
    let slice = &samples[start..];
    let max = slice.iter().cloned().fold(0.0f64, f64::max).max(1e-9);
    slice
        .iter()
        .map(|&v| {
            let idx = ((v / max) * (SPARK_TICKS.len() - 1) as f64)
                .round()
                .clamp(0.0, 7.0) as usize;
            SPARK_TICKS[idx]
        })
        .collect()
}

/// Block-sparkline of the most recent `width` samples, **auto-scaled to the window's
/// own min..max** (not 0..max) so a flat-but-jittery trend — IRR hovering at 56 dB,
/// a noise floor wandering near −48 dBFS — still shows its wiggle. Returns the glyph
/// string and the peak-to-peak spread of the visible window (for a `±x` annotation).
/// Shared by the Lab IQ IRR trend and the Lab RF sensitivity floor trend.
pub fn spark_minmax(samples: &[f32], width: usize) -> (String, f64) {
    if samples.is_empty() || width == 0 {
        return (String::new(), 0.0);
    }
    let start = samples.len().saturating_sub(width);
    let slice = &samples[start..];
    let lo = slice.iter().cloned().fold(f32::INFINITY, f32::min);
    let hi = slice.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let span = (hi - lo).max(1e-6);
    let s = slice
        .iter()
        .map(|&v| SPARK_TICKS[(((v - lo) / span) * 7.0).round().clamp(0.0, 7.0) as usize])
        .collect();
    (s, (hi - lo) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    #[test]
    fn spark_minmax_autoscales_to_window() {
        // A flat-but-high series with a small wiggle still spans the glyph range,
        // and the reported peak-to-peak is the true window span.
        let (s, p2p) = spark_minmax(&[56.0, 56.2, 55.8, 56.4, 56.0], 8);
        assert_eq!(s.chars().count(), 5);
        assert!(s.contains('\u{2588}'), "max sample → full block: {s}");
        assert!(s.contains('\u{2581}'), "min sample → low block: {s}");
        assert!((p2p - 0.6).abs() < 1e-4, "p2p {p2p}");
    }

    #[test]
    fn spark_minmax_empty_is_empty() {
        let (s, p2p) = spark_minmax(&[], 8);
        assert!(s.is_empty() && p2p == 0.0);
    }

    #[test]
    fn spark_minmax_respects_width() {
        let data: Vec<f32> = (0..30).map(|i| i as f32).collect();
        let (s, _) = spark_minmax(&data, 10);
        assert_eq!(s.chars().count(), 10);
    }

    #[test]
    fn snr_color_thresholds() {
        let t = Theme::sdr();
        assert_eq!(snr_color(25.0, &t), t.status_ok);
        assert_eq!(snr_color(15.0, &t), t.status_warn);
        assert_eq!(snr_color(5.0, &t), t.status_crit);
    }

    #[test]
    fn sat_and_drop_and_buf_thresholds() {
        let t = Theme::sdr();
        assert_eq!(sat_color(0.5, &t), t.status_ok);
        assert_eq!(sat_color(8.0, &t), t.status_crit);
        assert_eq!(drop_color(0, &t), t.status_ok);
        assert_eq!(drop_color(15, &t), t.status_crit);
        assert_eq!(buf_color(10.0, &t), t.status_ok);
        assert_eq!(buf_color(90.0, &t), t.status_crit);
    }

    #[test]
    fn fmt_helpers() {
        assert_eq!(fmt_rbw(9_800.0), "9.8 kHz");
        assert_eq!(fmt_rbw(800.0), "800 Hz");
        assert_eq!(fmt_freq_mhz(433_920_000), "433.920 MHz");
        assert_eq!(fmt_bw(152_000), "152.0 kHz");
        assert_eq!(fmt_bw(1_200_000), "1.200 MHz");
        assert_eq!(fmt_bw(400), "400 Hz");
    }

    #[test]
    fn fmt_bw_reads_the_same_everywhere() {
        // C1: five copies of this used to disagree, which meant the lab panel and
        // its own `[C]` snapshot printed one measurement two ways. There is one
        // function now, so the only thing left to pin is that it keeps the
        // resolution the analyser has rather than truncating to whole kHz.
        assert_eq!(fmt_bw(180_500), "180.5 kHz", "sub-kHz resolution survives");
        assert_eq!(
            fmt_bw(1_250_000),
            "1.250 MHz",
            "and sub-10-kHz at MHz scale"
        );
    }

    #[test]
    fn sparkline_scales_and_handles_edges() {
        assert_eq!(sparkline(&[], 8), "");
        // All-zero → flat baseline of lowest ticks.
        assert_eq!(sparkline(&[0.0, 0.0, 0.0], 8), "▁▁▁");
        // Max sample maps to the top tick; only the last `width` are used.
        let s = sparkline(&[0.0, 10.0], 2);
        assert_eq!(s.chars().count(), 2);
        assert_eq!(s.chars().nth(1), Some('█'));
        // Window keeps only the most recent `width` samples.
        assert_eq!(sparkline(&[5.0, 5.0, 5.0, 5.0], 2).chars().count(), 2);
    }

    #[test]
    fn bar_fills_proportionally() {
        let t = Theme::sdr();
        let [filled, empty] = bar_spans(0.5, 10, t.status_ok, &t);
        assert_eq!(filled.content.chars().count(), 5);
        assert_eq!(empty.content.chars().count(), 5);
        // Clamps above 1.0.
        let [filled, _] = bar_spans(2.0, 8, t.status_ok, &t);
        assert_eq!(filled.content.chars().count(), 8);
    }
}
