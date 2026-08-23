//! Shared formatting for the timing read-outs, so every panel that prints a
//! period, a drift or a verdict prints it the same way.
//!
//! These lived in `timing_panel` until that panel was retired: it was the
//! original single-panel `lab_timing` view, superseded by the three-panel bench
//! (`timing_diagnostics` · `timing_stripchart` · `timing_vitals`) and left
//! registered but unreachable. Its helpers had three callers by then, so they
//! outlived it and belong somewhere that isn't a panel.

use ratatui::{
    style::Style,
    text::Span,
};

use crate::state::TimingQuality;

/// Microseconds rendered as `ms` once they pass 1000 µs, else plain `µs`.
pub(crate) fn fmt_us(us: u64) -> String {
    if us >= 1_000 { format!("{:.3} ms", us as f64 / 1_000.0) } else { format!("{} µs", us) }
}

/// Signed ppm value, colored by absolute magnitude (green / yellow / red).
pub(crate) fn ppm_span(ppm: i64, theme: &crate::Theme) -> Span<'static> {
    let mag = ppm.unsigned_abs();
    let color = if mag < 50 { theme.status_ok } else if mag < 200 { theme.status_warn } else { theme.status_crit };
    Span::styled(format!("{:+} ppm", ppm), Style::default().fg(color))
}

/// The verdict's colour, taken from its own severity ranking so the scale is
/// defined once in `TimingQuality` rather than per panel.
pub(crate) fn quality_color(q: TimingQuality, theme: &crate::Theme) -> ratatui::style::Color {
    match q.severity() {
        0 => theme.status_ok,
        1 => theme.value_hi,
        2 => theme.status_warn,
        _ => theme.status_crit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_us_switches_to_ms() {
        assert_eq!(fmt_us(450), "450 µs");
        assert_eq!(fmt_us(13_107), "13.107 ms");
    }

    #[test]
    fn quality_color_matches_severity() {
        let t = crate::theme::Theme::sdr();
        assert_eq!(quality_color(TimingQuality::Excellent, &t), t.status_ok);
        assert_eq!(quality_color(TimingQuality::Marginal, &t), t.status_warn);
        assert_eq!(quality_color(TimingQuality::Poor, &t), t.status_crit);
    }

    #[test]
    fn ppm_span_color_thresholds() {
        let t = crate::theme::Theme::sdr();
        assert_eq!(ppm_span(10, &t).style.fg, Some(t.status_ok));
        assert_eq!(ppm_span(-120, &t).style.fg, Some(t.status_warn));
        assert_eq!(ppm_span(600, &t).style.fg, Some(t.status_crit));
    }
}
