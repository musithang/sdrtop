//! The three text rows under the axis: the zone breakdown, the PAPR estimate,
//! and the one-line verdict.

use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::papr::{estimate_papr_db, papr_hint};
use super::zones::{Zones, BINS};

/// Share of samples that is "too much" for each zone, in percent.
///
/// Deliberately asymmetric. Low and Mid are *distribution* judgements - a signal
/// mostly in the bottom eighth of the range is under-driven - while Clip is a
/// *fault*: any sample at all near the rails is worth an amber, because unlike
/// the other two it destroys information rather than merely wasting range.
const LOW_CRIT_PCT: u64 = 90;
const LOW_WARN_PCT: u64 = 70;
const MID_GOOD_PCT: u64 = 50;
const MID_WARN_PCT: u64 = 20;
const CLIP_CRIT_PCT: u64 = 10;

pub(super) fn breakdown(f: &mut Frame, area: Rect, z: &Zones, theme: &crate::Theme) {
    let lbl = Style::default().fg(theme.label);
    let (low, mid, clip) = (z.pct(z.low), z.pct(z.mid), z.pct(z.clip));

    let low_col = threshold_down(low, LOW_WARN_PCT, LOW_CRIT_PCT, theme);
    let mid_col = if mid > MID_GOOD_PCT {
        theme.status_ok
    } else if mid > MID_WARN_PCT {
        theme.status_warn
    } else {
        theme.label
    };
    let clip_col = if clip > CLIP_CRIT_PCT {
        theme.status_crit
    } else if clip > 0 {
        theme.status_warn
    } else {
        theme.status_ok
    };

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Low ", lbl),
            Span::styled(format!("{low:3}%"), Style::default().fg(low_col)),
            Span::styled("  Mid ", lbl),
            Span::styled(format!("{mid:3}%"), Style::default().fg(mid_col)),
            Span::styled("  Clip ", lbl),
            Span::styled(format!("{clip:3}%"), Style::default().fg(clip_col)),
        ])),
        area,
    );
}

/// "More of this is worse" - neutral, then amber, then red.
fn threshold_down(pct: u64, warn: u64, crit: u64, theme: &crate::Theme) -> Color {
    if pct > crit {
        theme.status_crit
    } else if pct > warn {
        theme.status_warn
    } else {
        theme.label
    }
}

pub(super) fn papr(
    f: &mut Frame,
    area: Rect,
    hist: &[u64; BINS],
    total: u64,
    theme: &crate::Theme,
) {
    let lbl = Style::default().fg(theme.label);
    let line = match estimate_papr_db(hist, total) {
        Some(db) => Line::from(vec![
            Span::styled("PAPR ", lbl),
            Span::styled(format!("{db:.1} dB"), Style::default().fg(theme.value)),
            Span::styled(
                format!("  ({})", papr_hint(db)),
                Style::default().fg(theme.border_dim),
            ),
        ]),
        None => Line::from(vec![
            Span::styled("PAPR ", lbl),
            Span::styled("---", Style::default().fg(theme.label)),
        ]),
    };
    f.render_widget(Paragraph::new(line), area);
}

/// The one-line verdict, worst first. Counted against the totals rather than the
/// rounded percentages above, so the words cannot disagree with themselves at a
/// boundary.
pub(super) fn status(f: &mut Frame, area: Rect, z: &Zones, theme: &crate::Theme) {
    let label = if z.total == 0 {
        Span::styled("No samples yet", Style::default().fg(theme.label))
    } else if z.clip > z.total / 10 {
        Span::styled(
            "\u{25b2} clipping risk",
            Style::default().fg(theme.status_crit),
        )
    } else if z.low > z.total * 9 / 10 {
        Span::styled(
            "\u{25bc} weak signal \u{2014} ADC under-utilised",
            Style::default().fg(theme.status_warn),
        )
    } else {
        Span::styled("Dynamic range OK", Style::default().fg(theme.status_ok))
    };
    f.render_widget(Paragraph::new(label), area);
}
