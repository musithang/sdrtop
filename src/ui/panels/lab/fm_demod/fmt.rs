//! The value vocabulary every section shares: how a frequency is written, and
//! the two styles a reading wears.
//!
//! Sections print into one shape — [`chrome::field`](crate::ui::chrome::field)
//! for the label, one of these styles for the number beside it — so the styles
//! live here rather than being re-derived at the top of each section module.

use ratatui::style::Style;

/// Label colour: hints, units, and anything that is not itself a reading.
pub(super) fn lbl(theme: &crate::Theme) -> Style {
    Style::default().fg(theme.label)
}

/// Value colour: the numbers, where the number is not being graded.
pub(super) fn val(theme: &crate::Theme) -> Style {
    Style::default().fg(theme.value)
}

/// Frequency in the most readable unit for a deviation / offset figure. Keeps
/// three significant figures across the WFM (tens of kHz) and NFM (single kHz)
/// ranges without ever padding a number with false precision.
pub(super) fn fmt_hz(hz: f32) -> String {
    let a = hz.abs();
    if a >= 10_000.0 {
        format!("{:.1} kHz", hz / 1000.0)
    } else if a >= 1_000.0 {
        format!("{:.2} kHz", hz / 1000.0)
    } else {
        format!("{:.0} Hz", hz)
    }
}

/// Signed variant for the carrier offset, where the sign is the point.
pub(super) fn fmt_offset(hz: f32) -> String {
    let sign = if hz >= 0.0 { "+" } else { "\u{2212}" };
    format!("{}{}", sign, fmt_hz(hz.abs()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_hz_picks_a_readable_unit() {
        assert_eq!(fmt_hz(42_300.0), "42.3 kHz");
        assert_eq!(fmt_hz(4_230.0), "4.23 kHz");
        assert_eq!(fmt_hz(420.0), "420 Hz");
    }

    #[test]
    fn fmt_offset_always_carries_a_sign() {
        assert!(fmt_offset(1_200.0).starts_with('+'));
        assert!(fmt_offset(-1_200.0).starts_with('\u{2212}'));
        // Zero reads as a positive zero rather than a bare number.
        assert!(fmt_offset(0.0).starts_with('+'));
    }
}
