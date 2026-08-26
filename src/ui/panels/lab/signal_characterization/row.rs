//! The row vocabulary every zone of this panel writes into: the label column,
//! the rule for a trailing annotation, and the two styles a value wears.

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::ui::chrome::field;

/// Label-column width — clears the longest label ("Channel power" = 13) plus a gap.
const FIELD_W: usize = 14;

/// Gap between a metric's value and its dim annotation.
const ANN_GAP: usize = 3;

/// Value colour: the measurements themselves.
pub(super) fn val(theme: &crate::Theme) -> Style {
    Style::default().fg(theme.value)
}

/// Dim colour: annotations, units and anything that is context rather than a
/// reading.
pub(super) fn dim(theme: &crate::Theme) -> Style {
    Style::default().fg(theme.stale)
}

/// The placeholder for a reading that is not available. Dim, and never a number:
/// an unmeasured field must not look like a measured one.
pub(super) fn dash(theme: &crate::Theme) -> Span<'static> {
    Span::styled("---".to_string(), dim(theme))
}

/// One `label … value` row on the shared label column.
pub(super) fn metric(name: &str, body: Vec<Span<'static>>, theme: &crate::Theme) -> Line<'static> {
    let mut spans = vec![field(name, FIELD_W, theme)];
    spans.extend(body);
    Line::from(spans)
}

/// A value with its trailing dim annotation, the annotation dropped whole when
/// the column cannot hold both — see [`annotation_fits`].
pub(super) fn annotated(
    value: String,
    ann: String,
    iw: usize,
    theme: &crate::Theme,
) -> Vec<Span<'static>> {
    let fits = annotation_fits(value.chars().count(), ann.chars().count(), iw);
    let mut spans = vec![Span::styled(value, val(theme))];
    if fits {
        spans.push(Span::styled(
            format!("{}{ann}", " ".repeat(ANN_GAP)),
            dim(theme),
        ));
    }
    spans
}

/// `92.800 MHz` / `1.234500 GHz` — the same precise readout the lab marker bar uses.
pub(super) fn fmt_freq(hz: u64) -> String {
    if hz >= 1_000_000_000 {
        format!("{:.6} GHz", hz as f64 / 1e9)
    } else {
        format!("{:.3} MHz", hz as f64 / 1e6)
    }
}

/// Whether a metric row's dim annotation fits the panel's inner width `iw`, given
/// the value it trails.
///
/// The annotations on this panel — the peak's frequency, `99% power`, the noise
/// density, the adjacent band's frequency — are context, not measurements, and a
/// clipped one is worse than none at all. At 120 columns the panel's inner width
/// is 29 and the paragraph simply chopped them mid-token, so the Peak row read
///
/// ```text
/// Peak          -35.4 dBFS   9
/// ```
///
/// where `9` is the first character of `92.807 MHz`: a truncated frequency that
/// reads as a value. Dropping the annotation costs the reader a detail they can
/// get from the row above; truncating it tells them something untrue.
fn annotation_fits(value_w: usize, ann_w: usize, iw: usize) -> bool {
    1 + FIELD_W + value_w + ANN_GAP + ann_w <= iw
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annotation_dropped_rather_than_clipped_at_a_narrow_column() {
        // The B2 row, measured: 120-column terminal → 29 inner. Lead 1 + label 14
        // + "-35.4 dBFS" 10 + gap 3 leaves 1 column, and "92.807 MHz" needs 10 —
        // so the frequency goes, instead of arriving as a lone "9".
        assert!(!annotation_fits(
            "-35.4 dBFS".chars().count(),
            "92.807 MHz".chars().count(),
            29
        ));
        // Widen the panel past the exact total (1+14+10+3+10 = 38) and it returns.
        assert!(annotation_fits(
            "-35.4 dBFS".chars().count(),
            "92.807 MHz".chars().count(),
            38
        ));
        assert!(!annotation_fits(
            "-35.4 dBFS".chars().count(),
            "92.807 MHz".chars().count(),
            37
        ));
    }

    #[test]
    fn noise_density_annotation_is_the_first_to_go() {
        // The widest annotation on the panel; it should survive a full-width lab
        // column (the left column at 200 wide is ~46 inner) and drop below it.
        let (v, a) = (
            "-81.1 dBFS".chars().count(),
            "-112.8 dBFS/Hz".chars().count(),
        );
        assert!(annotation_fits(v, a, 46));
        assert!(!annotation_fits(v, a, 29));
    }
}
