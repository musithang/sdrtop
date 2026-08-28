//! Sample-rate accuracy: what was asked for against what the device delivers.
//!
//! A rate that is off shifts every frequency on the deck by the same fraction,
//! so this row says whether the other numbers can be trusted.

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::state::SdrMetrics;

use super::super::field::Field;
use super::rows::LABEL_W;

/// Offset (ppm) beyond which the clock is worth remarking on. Well inside what
/// would shift a reading visibly, so it warns before anything looks wrong.
const PPM_WARN: i64 = 100;

/// Sample-rate offset in parts per million, or `None` until the device reports
/// an actual rate. Pure, so the arithmetic can be checked without a frame.
pub(super) fn offset_ppm(configured_hz: f64, actual_hz: u32) -> Option<i64> {
    if actual_hz == 0 || configured_hz <= 0.0 {
        return None;
    }
    Some((((actual_hz as f64 - configured_hz) / configured_hz) * 1e6).round() as i64)
}

pub(super) fn line(state: &SdrMetrics, fd: &Field) -> Line<'static> {
    let theme = fd.theme;
    let configured = state.radio.config_sample_rate / 1e6;
    let mut spans = vec![Span::raw(" "), fd.padded("SR", LABEL_W)];

    // Until the device answers there is a configured rate but nothing to compare
    // it against, so the comparison is a dash rather than a fabricated match.
    match offset_ppm(
        state.radio.config_sample_rate,
        state.radio.actual_sample_rate,
    )
    .filter(|_| !fd.stale)
    {
        None => {
            spans.push(fd.value(format!("{configured:.3} MHz")));
            spans.push(Span::raw("  "));
            spans.push(fd.dash());
        }
        Some(ppm) => {
            let actual = state.radio.actual_sample_rate as f64 / 1e6;
            spans.push(fd.value(format!("{configured:.3} \u{2192} {actual:.3} MHz")));
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                format!("{ppm:+}ppm"),
                Style::default().fg(if ppm.abs() < PPM_WARN {
                    theme.status_ok
                } else {
                    theme.status_warn
                }),
            ));
        }
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_offset_is_signed_parts_per_million() {
        assert_eq!(offset_ppm(10_000_000.0, 10_000_000), Some(0));
        assert_eq!(offset_ppm(10_000_000.0, 10_000_100), Some(10));
        assert_eq!(offset_ppm(10_000_000.0, 9_999_900), Some(-10));
    }

    /// No reported rate is *unknown*, not zero offset — the difference between
    /// "the clock is perfect" and "the device has not said".
    #[test]
    fn an_unreported_rate_has_no_offset() {
        assert_eq!(offset_ppm(10_000_000.0, 0), None);
        assert_eq!(offset_ppm(0.0, 10_000_000), None);
    }
}
