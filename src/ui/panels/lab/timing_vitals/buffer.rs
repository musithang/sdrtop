//! The ring buffer between the radio and the host: how full it gets, and how much
//! room is left before an overrun.
//!
//! The peak matters more than the instantaneous fill - the buffer only has to
//! reach the ceiling once to lose samples - which is why the margin is computed
//! from the session peak rather than from the current depth.

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::state::SdrMetrics;
use crate::ui::widgets::micro_common::buf_color;

use super::calc::overrun_margin_pct;
use super::rows::{load_color, Rows};

pub(super) fn lines(state: &SdrMetrics, r: &Rows) -> Vec<Line<'static>> {
    let theme = r.theme;
    let mut out = vec![crate::ui::chrome::section(
        "RING BUFFER",
        "overrun margin",
        r.iw,
        theme,
    )];

    let fill = state.iq.buf_fill_pct as f64;
    out.push(r.bar(
        "fill depth",
        fill / 100.0,
        format!("{fill:.0}%"),
        theme.status_ok,
        theme.status_crit,
        buf_color(state.iq.buf_fill_pct, theme),
    ));

    // The history is stored as percent × 10, so the peak is scaled back here.
    let peak = state.iq.buf_fill_history.iter().copied().max().unwrap_or(0) as f64 / 10.0;
    let (tag, tag_color) = if peak >= 100.0 {
        ("hit ceiling", theme.status_crit)
    } else {
        ("headroom ok", theme.status_ok)
    };
    out.push(Line::from(if r.stale {
        vec![
            Span::raw(" "),
            Span::styled("Peak fill ", r.lbl()),
            r.dash(),
        ]
    } else {
        vec![
            Span::raw(" "),
            Span::styled("Peak fill ", r.lbl()),
            Span::styled(
                format!("{peak:.0} %"),
                Style::default().fg(buf_color(peak as f32, theme)),
            ),
            Span::styled(format!("   {tag}"), Style::default().fg(tag_color)),
        ]
    }));

    let margin = overrun_margin_pct(peak);
    out.push(Line::from(if r.stale {
        vec![
            Span::raw(" "),
            Span::styled("Overrun margin ", r.lbl()),
            r.dash(),
        ]
    } else {
        vec![
            Span::raw(" "),
            Span::styled("Overrun margin ", r.lbl()),
            Span::styled(
                format!("{margin:.0}%"),
                // Graded on the *fill* the margin leaves, so a shrinking margin
                // reddens on the same scale CPU load does.
                Style::default().fg(load_color(100.0 - margin, theme)),
            ),
        ]
    }));
    out
}
