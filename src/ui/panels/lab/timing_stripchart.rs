//! `timing_stripchart` — the centerpiece of the `lab_timing` redesign.
//!
//! A real-time strip chart of the per-callback interval deviation from the
//! expected period (`state.timing.cb_deviations_us`, newest last). Positive bars
//! are late deliveries, negative are early; the deadline budget is drawn as a
//! faint guide line and, more usefully, every bar is coloured the instant it
//! crosses the budget, so late callbacks redden in place. Spikes beyond the axis
//! clamp and are tagged in the direction they blew out: `▲` at the top for a late
//! (positive) overrun, `▼` at the bottom for an early (negative) one.
//!
//! The axis is anchored to the budget (full scale = 1.5 × budget) so the band sits
//! at a stable two-thirds out and the scale labels stay put across sample rates,
//! rather than jittering with an auto-scaled peak.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::widgets::charts::bipolar_braille_strip;
use crate::ui::panel::{Panel, PanelChrome, Staleness};
use crate::ui::widgets::timing_fmt::fmt_us;

pub struct TimingStripchartPanel;

/// Left axis gutter width (the `+0.45 ` labels).
const GUTTER_W: usize = 7;
/// Blank braille cell — an "empty" column with no plotted dots.
const BLANK: char = '\u{2800}';

/// Severity of a column's worst deviation against the budget: 0 in budget,
/// 1 over budget, 2 more than double over. Drives the bar colour.
fn dev_severity(max_abs_us: u64, budget_us: u64) -> u8 {
    if budget_us == 0 || max_abs_us <= budget_us { 0 }
    else if max_abs_us <= budget_us * 2 { 1 }
    else { 2 }
}

fn severity_color(sev: u8, theme: &crate::Theme) -> Color {
    match sev { 0 => theme.value, 1 => theme.status_warn, _ => theme.status_crit }
}

/// Direction of a column's over-range spike, from its two samples: `+1` if the
/// worst (largest-magnitude) sample is late (positive), `−1` if it is early
/// (negative). Decides whether the spike tag is drawn as `▲` at the top of the
/// chart (a late overrun) or `▼` at the bottom (an early one).
fn over_tag_sign(a: i32, b: i32) -> i8 {
    let worst = if a.unsigned_abs() >= b.unsigned_abs() { a } else { b };
    if worst < 0 { -1 } else { 1 }
}

/// Signed deviation (µs) at the top edge of text row `r` of an `rows`-tall chart,
/// where row 0 is `+full_scale` and the last row is `−full_scale`.
fn axis_value_us(r: usize, rows: usize, full_scale: i32) -> i32 {
    if rows <= 1 { return 0; }
    (full_scale as f64 * (1.0 - 2.0 * r as f64 / (rows - 1) as f64)).round() as i32
}

/// Right-aligned gutter label (`+0.45`, `0`, `−0.90`) in ms when the scale is
/// large, else µs. Plain spaces for an unlabelled row.
fn gutter_label(text: Option<String>) -> String {
    match text {
        Some(s) => format!("{s:>w$} ", w = GUTTER_W - 1),
        None    => " ".repeat(GUTTER_W),
    }
}

fn fmt_axis(us: i32) -> String {
    if us == 0 { "0".to_string() }
    else if us.unsigned_abs() >= 1000 {
        let sign = if us < 0 { "\u{2212}" } else { "+" };
        format!("{sign}{:.1}", us.unsigned_abs() as f64 / 1000.0)
    } else {
        let sign = if us < 0 { "\u{2212}" } else { "+" };
        format!("{sign}{}", us.unsigned_abs())
    }
}

impl Panel for TimingStripchartPanel {
    fn name(&self) -> &'static str { "timing_stripchart" }
    fn min_size(&self) -> (u16, u16) { (48, 12) }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("Callback Interval")
            .stale_when(Staleness::NotStreaming)
            .suffix(" \u{00b7} Real-Time Strip Chart")
    }

    fn render(&self, f: &mut Frame, inner: Rect, state: &SdrMetrics, theme: &crate::Theme, _focused: bool) {
        let stale = !state.radio.hw_streaming;
        if inner.width == 0 || inner.height == 0 { return; }
        let iw = inner.width as usize;
        let lbl = Style::default().fg(theme.label);
        let dim = Style::default().fg(theme.border_dim);

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // description
                Constraint::Length(1), // stats line
                Constraint::Length(1), // blank
                Constraint::Min(3),    // chart
                Constraint::Length(1), // legend 1
                Constraint::Length(1), // legend 2
            ])
            .split(inner);

        // ── Description ─────────────────────────────────────────────────────────
        let desc = if iw >= 78 {
            "each point is one RX callback \u{00b7} deviation of its interval from the expected period \u{00b7} bars past the band are late"
        } else if iw >= 40 {
            "per-callback interval deviation from the expected period"
        } else {
            "callback interval deviation"
        };
        f.render_widget(Paragraph::new(Line::from(vec![Span::raw(" "), Span::styled(desc, lbl)])), rows[0]);

        let t = &state.timing;
        let budget = t.deadline_budget_us;

        // ── Inset stats line ────────────────────────────────────────────────────
        let stats = if stale {
            Line::from(vec![Span::raw(" "), Span::styled("RX stopped", Style::default().fg(theme.stale))])
        } else {
            let q = t.timing_quality;
            let mark = if q.severity() == 0 { "\u{2713}" } else { "\u{26a0}" };
            let qcol = crate::ui::widgets::timing_fmt::quality_color(q, theme);
            Line::from(vec![
                Span::raw(" "),
                Span::styled(format!("jitter \u{00b1}{} \u{00b5}s", t.cb_jitter_us), Style::default().fg(theme.value)),
                Span::styled("   worst ", lbl),
                Span::styled(fmt_us(t.dev_peak_us), Style::default().fg(theme.value)),
                Span::styled("   over budget ", lbl),
                Span::styled(format!("{} / {}", t.late_callbacks, t.late_window),
                    Style::default().fg(if t.late_callbacks == 0 { theme.status_ok } else { theme.status_warn })),
                Span::styled("   ", lbl),
                Span::styled(format!("{mark} {}", q.label()), Style::default().fg(qcol).add_modifier(Modifier::BOLD)),
            ])
        };
        f.render_widget(Paragraph::new(stats), rows[1]);

        // ── Chart ───────────────────────────────────────────────────────────────
        let chart = rows[3];
        let chart_h = chart.height as usize;
        let cols = (chart.width as usize).saturating_sub(GUTTER_W);
        if stale || cols < 4 || chart_h == 0 || t.cb_deviations_us.is_empty() {
            let msg = if stale { "\u{25cb} IDLE \u{2014} RX stopped" } else { "waiting for callbacks\u{2026}" };
            let mid = chart.y + chart.height / 2;
            f.render_widget(
                Paragraph::new(Line::from(vec![Span::raw(" "), Span::styled(msg, Style::default().fg(theme.stale))])),
                Rect { x: chart.x, y: mid, width: chart.width, height: 1 });
        } else {
            let full_scale = ((budget as f64 * 1.5).round() as i32).max(1);
            let dev = &t.cb_deviations_us;
            let (strip, over) = bipolar_braille_strip(dev, cols, chart_h, full_scale);

            // The samples actually shown (last 2*cols), for per-column colouring.
            let n = cols * 2;
            let wstart = dev.len().saturating_sub(n);
            let window = &dev[wstart..];
            let col_sev = |c: usize| -> Option<u8> {
                let a = window.get(2 * c).map(|v| v.unsigned_abs() as u64);
                let b = window.get(2 * c + 1).map(|v| v.unsigned_abs() as u64);
                match (a, b) {
                    (None, None) => None,
                    _ => Some(dev_severity(a.unwrap_or(0).max(b.unwrap_or(0)), budget)),
                }
            };

            // Per-column over-range tag direction: +1 late (▲ top), −1 early
            // (▼ bottom), 0 in range. Precomputed so the row loop is a cheap lookup.
            let mut over_sign = vec![0i8; cols];
            for &c in &over {
                let a = window.get(2 * c).copied().unwrap_or(0);
                let b = window.get(2 * c + 1).copied().unwrap_or(0);
                if let Some(slot) = over_sign.get_mut(c) { *slot = over_tag_sign(a, b); }
            }
            let last_row = chart_h - 1;

            // Text rows nearest the ±budget band edges (for the faint guide line).
            let span = (chart_h * 4 - 1) as f64 / 2.0;
            let bf = budget as f64 / full_scale as f64;
            let band_top = ((span * (1.0 - bf)).round() as usize / 4).min(chart_h - 1);
            let band_bot = ((span * (1.0 + bf)).round() as usize / 4).min(chart_h - 1);

            // Rows that carry an axis label: top, quarter, mid, three-quarter, bottom.
            let label_rows = [0, (chart_h - 1) / 4, (chart_h - 1) / 2, (chart_h - 1) * 3 / 4, chart_h - 1];

            let mut out: Vec<Line> = Vec::with_capacity(chart_h);
            for (r, row_str) in strip.iter().enumerate() {
                let chars: Vec<char> = row_str.chars().collect();
                let label = if label_rows.contains(&r) {
                    Some(fmt_axis(axis_value_us(r, chart_h, full_scale)))
                } else { None };
                let mut spans = vec![Span::styled(gutter_label(label), lbl)];
                for (c, &ch) in chars.iter().enumerate() {
                    let osign = over_sign.get(c).copied().unwrap_or(0);
                    if r == 0 && osign > 0 {
                        spans.push(Span::styled("\u{25B2}".to_string(), Style::default().fg(theme.status_crit)));
                    } else if r == last_row && osign < 0 {
                        spans.push(Span::styled("\u{25BC}".to_string(), Style::default().fg(theme.status_crit)));
                    } else if ch == BLANK {
                        // Empty cell: draw the deadline guide on the band rows, else nothing.
                        if r == band_top || r == band_bot {
                            spans.push(Span::styled("\u{2504}".to_string(), dim));
                        } else {
                            spans.push(Span::styled(BLANK.to_string(), dim));
                        }
                    } else {
                        let col = col_sev(c).map(|s| severity_color(s, theme)).unwrap_or(theme.value);
                        spans.push(Span::styled(ch.to_string(), Style::default().fg(col)));
                    }
                }
                out.push(Line::from(spans));
            }
            f.render_widget(Paragraph::new(out), chart);
        }

        // ── Legend ──────────────────────────────────────────────────────────────
        let legend1 = Line::from(vec![
            Span::raw(" "),
            Span::styled("\u{25AC} in budget", Style::default().fg(theme.value)),
            Span::raw("   "),
            Span::styled("\u{25AC} over budget", Style::default().fg(theme.status_warn)),
            Span::raw("   "),
            Span::styled(format!("\u{2504} \u{00b1}{} \u{00b5}s deadline", budget), dim),
        ]);
        f.render_widget(Paragraph::new(legend1), rows[4]);

        // Time span of the samples actually on screen: the chart shows the last
        // 2×cols deviations, but never more than the snapshot holds.
        let window_s = {
            let shown = (2 * cols).min(t.cb_deviations_us.len());
            (shown as u64 * t.cb_period_us) as f64 / 1e6
        };
        let legend2 = if stale || t.cb_period_us == 0 {
            Line::from(vec![Span::raw(" "), Span::styled("one point per RX callback", lbl)])
        } else {
            Line::from(vec![
                Span::raw(" "),
                Span::styled(format!("\u{2190} {window_s:.1} s window \u{00b7} one point per RX callback \u{2192}"), lbl),
            ])
        };
        f.render_widget(Paragraph::new(legend2), rows[5]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_severity_thresholds() {
        assert_eq!(dev_severity(100, 600), 0, "well in budget");
        assert_eq!(dev_severity(600, 600), 0, "exactly at budget is not late");
        assert_eq!(dev_severity(900, 600), 1, "over budget");
        assert_eq!(dev_severity(1_300, 600), 2, "more than double");
        assert_eq!(dev_severity(999, 0), 0, "no budget → never late");
    }

    #[test]
    fn over_tag_sign_points_to_the_worst_samples_direction() {
        // The larger-magnitude sample decides: a late spike tags up, an early one down.
        assert_eq!(over_tag_sign(8_000, -50), 1, "late spike → ▲ top");
        assert_eq!(over_tag_sign(-9_000, 50), -1, "early spike → ▼ bottom");
        // Ties go to the first (positive) sample; a lone positive sample is late.
        assert_eq!(over_tag_sign(700, -700), 1);
        assert_eq!(over_tag_sign(0, -300), -1);
    }

    #[test]
    fn axis_value_spans_full_scale_top_to_bottom() {
        // Row 0 = +full_scale, last row = −full_scale, middle ≈ 0.
        assert_eq!(axis_value_us(0, 9, 900), 900);
        assert_eq!(axis_value_us(8, 9, 900), -900);
        assert_eq!(axis_value_us(4, 9, 900), 0);
    }

    #[test]
    fn fmt_axis_picks_units_and_sign() {
        assert_eq!(fmt_axis(0), "0");
        assert_eq!(fmt_axis(450), "+450");
        assert_eq!(fmt_axis(-450), "\u{2212}450");
        // Sub-millisecond stays in µs; only ≥ 1000 µs switches to ms.
        assert_eq!(fmt_axis(900), "+900");
        assert_eq!(fmt_axis(1_200), "+1.2");
        assert_eq!(fmt_axis(-1_500), "\u{2212}1.5");
    }

    #[test]
    fn gutter_label_is_fixed_width() {
        assert_eq!(gutter_label(None).chars().count(), GUTTER_W);
        assert_eq!(gutter_label(Some("+0.9".into())).chars().count(), GUTTER_W);
    }
}
