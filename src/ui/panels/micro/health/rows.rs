//! The `LABEL  value  sparkline  OK/⚠` row the stream block is built from.
//!
//! Was a seven-parameter free function whose call sites each repeated the same
//! `if stale { … } else { … }` twice — once for the value and once for the
//! colour. As a struct the staleness is asked once, and a caller supplies only
//! what actually differs between the three rows.

use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

use crate::ui::widgets::micro_common::sparkline;

use super::super::field::Field;

/// Label column, wide enough for `DROP`, `CPU`, `USB` and their neighbours.
pub(super) const LABEL_W: usize = 6;
/// Value column, so the sparklines all start at the same place.
pub(super) const VALUE_W: usize = 7;
/// Inline trend width.
pub(super) const SPARK_W: usize = 14;

/// One health row's live content. `None` is a reading that is not being taken.
pub(super) struct Row<'a> {
    pub label: &'static str,
    pub value: Option<String>,
    pub hist: &'a [f64],
    /// Colour of the value and its trend when live.
    pub color: Color,
    /// Whether this reading is within tolerance — drives the `OK` / `⚠` mark.
    pub ok: bool,
}

pub(super) fn row(r: Row<'_>, fd: &Field) -> Line<'static> {
    let color = if fd.stale { fd.theme.stale } else { r.color };
    let mut spans = vec![Span::raw(" "), fd.padded(r.label, LABEL_W)];
    match r.value.filter(|_| !fd.stale) {
        Some(v) => spans.push(Span::styled(
            format!("{v:<VALUE_W$}"),
            Style::default().fg(color),
        )),
        None => spans.push(Span::styled(
            format!("{:<VALUE_W$}", "---"),
            Style::default().fg(fd.theme.stale),
        )),
    }
    spans.push(Span::styled(
        sparkline(r.hist, SPARK_W),
        Style::default().fg(color),
    ));
    // No verdict mark while stopped: `OK` next to a dash would be claiming a
    // reading the panel is not taking.
    if !fd.stale {
        spans.push(Span::raw("  "));
        let (mark, mark_color) = if r.ok {
            ("OK", fd.theme.status_ok)
        } else {
            ("\u{26a0}", fd.theme.status_warn)
        };
        spans.push(Span::styled(mark, Style::default().fg(mark_color)));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::SdrMetrics;

    fn text(l: &Line<'static>) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn field(stale: bool, t: &crate::Theme) -> Field<'_> {
        Field { theme: t, stale }
    }

    /// A stopped radio shows no value, no trend colour and — crucially — no
    /// verdict mark, because it is not taking the reading.
    #[test]
    fn a_stale_row_makes_no_claim() {
        let t = crate::Theme::sdr();
        let hist = [1.0, 2.0, 3.0, 4.0];
        let line = row(
            Row {
                label: "DROP",
                value: Some("14/s".into()),
                hist: &hist,
                color: t.status_crit,
                ok: false,
            },
            &field(true, &t),
        );
        let s = text(&line);
        assert!(s.contains("---"), "{s:?}");
        assert!(!s.contains("14/s"), "{s:?}");
        assert!(!s.contains("OK") && !s.contains('\u{26a0}'), "{s:?}");
    }

    /// Live, the row carries its value and the mark that matches `ok`.
    #[test]
    fn a_live_row_marks_its_verdict() {
        let t = crate::Theme::sdr();
        let hist = [0.0; 4];
        let good = row(
            Row {
                label: "DROP",
                value: Some("0/s".into()),
                hist: &hist,
                color: t.status_ok,
                ok: true,
            },
            &field(false, &t),
        );
        assert!(text(&good).contains("OK"), "{:?}", text(&good));

        let bad = row(
            Row {
                label: "DROP",
                value: Some("14/s".into()),
                hist: &hist,
                color: t.status_crit,
                ok: false,
            },
            &field(false, &t),
        );
        assert!(text(&bad).contains('\u{26a0}'), "{:?}", text(&bad));
        assert!(!text(&bad).contains("OK"));
    }

    /// The three rows are read as a column, so their sparklines must start in the
    /// same place however long the values are.
    #[test]
    fn every_row_starts_its_trend_in_the_same_column() {
        let t = crate::Theme::sdr();
        let hist = [1.0; 4];
        let widths: Vec<usize> = [("DROP", "0/s"), ("SAT", "12.5%"), ("BUF", "100%")]
            .iter()
            .map(|(label, value)| {
                let line = row(
                    Row {
                        label,
                        value: Some((*value).to_string()),
                        hist: &hist,
                        color: t.value,
                        ok: true,
                    },
                    &field(false, &t),
                );
                // Everything before the sparkline span.
                line.spans[..3]
                    .iter()
                    .map(|s| s.content.chars().count())
                    .sum()
            })
            .collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "trends start at different columns: {widths:?}"
        );
    }

    /// The fixture is a real state, so a row built from it renders.
    #[test]
    fn a_row_built_from_the_fixture_renders() {
        let t = crate::Theme::sdr();
        let m = SdrMetrics::fixture().streaming();
        let fd = Field::new(&m, &t);
        assert!(!fd.stale);
        let line = row(
            Row {
                label: "BUF",
                value: Some("0%".into()),
                hist: &[],
                color: t.value,
                ok: true,
            },
            &fd,
        );
        assert!(!text(&line).is_empty());
    }
}
