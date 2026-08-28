//! The closing line: the worst thing that is true, and how long it has been true.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::state::SdrMetrics;

use super::super::field::Field;

/// Process CPU (percent) above which the host is the thing to look at.
const CPU_HIGH_PCT: f32 = 70.0;

/// One-glance verdict: drops beat CPU beat all-clear; idle when not streaming.
///
/// One row, so the ordering is the whole design - the worst thing that is true
/// is the thing shown.
pub(super) fn line(state: &SdrMetrics, fd: &Field) -> Line<'static> {
    let theme = fd.theme;
    if fd.stale {
        return Line::from(vec![
            Span::raw(" "),
            Span::styled(
                "\u{25cb} IDLE \u{2014} RX stopped",
                Style::default().fg(theme.stale),
            ),
        ]);
    }

    let session = state
        .radio
        .rx_start_time
        .map(|t| crate::tasks::fmt_duration(t.elapsed().as_secs()))
        .unwrap_or_else(|| "\u{2014}".to_string());

    let (text, color) = if state.signal.drops_per_sec > 0 {
        ("\u{26a0} DROP DETECTED".to_string(), theme.status_crit)
    } else if state.system.process_cpu_pct > CPU_HIGH_PCT {
        ("\u{26a0} CPU HIGH".to_string(), theme.status_warn)
    } else {
        (
            format!("\u{2713} System OK \u{2014} session {session}"),
            theme.status_ok,
        )
    };
    Line::from(vec![
        Span::raw(" "),
        Span::styled(
            text,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
    ])
}
