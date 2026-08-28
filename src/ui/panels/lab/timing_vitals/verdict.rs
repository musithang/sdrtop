//! The closing line: one word on the pipeline, and how long it has been running.
//!
//! [`decide`] draws nothing - it takes a severity and returns the mark, the
//! words and how loudly to say them, so the wording can be checked without a
//! frame.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::state::SdrMetrics;

use super::calc::fmt_uptime;
use super::rows::Rows;

/// The verdict for a [`TimingQuality`](crate::state::TimingQuality) severity.
///
/// Three bands from four severities on purpose: this panel is about the *host
/// pipeline*, and "good" and "marginal" timing both mean the same thing here -
/// it is coping, but working for it. `timing_diagnostics` is the panel that
/// distinguishes them, because that is where the numbers are.
pub(super) fn decide(severity: u8) -> (&'static str, &'static str, u8) {
    match severity {
        0 => ("\u{2713}", "all vitals nominal", 0),
        1 | 2 => ("\u{26a0}", "pipeline under load", 1),
        _ => ("\u{26a0}", "overrun logged", 2),
    }
}

pub(super) fn lines(state: &SdrMetrics, r: &Rows) -> Vec<Line<'static>> {
    let theme = r.theme;
    if r.stale {
        return vec![Line::from(vec![
            Span::raw(" "),
            Span::styled("\u{25cb} idle \u{2014} RX stopped", r.dim()),
        ])];
    }

    let (mark, text, level) = decide(state.timing.timing_quality.severity());
    let color = match level {
        0 => theme.status_ok,
        1 => theme.status_warn,
        _ => theme.status_crit,
    };
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(
            format!("{mark} {text}"),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(up) = state
        .radio
        .rx_start_time
        .map(|t| fmt_uptime(t.elapsed().as_secs()))
    {
        let used = 1 + mark.chars().count() + 1 + text.chars().count();
        let tail = format!("uptime {up}");
        let gap = r.iw.saturating_sub(used + tail.chars().count()).max(1);
        spans.push(Span::raw(" ".repeat(gap)));
        spans.push(Span::styled(tail, r.lbl()));
    }
    vec![Line::from(spans)]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every severity has a verdict, and they get worse in order.
    #[test]
    fn the_verdict_never_improves_as_severity_rises() {
        let mut last = 0u8;
        for sev in 0..=3u8 {
            let (mark, text, level) = decide(sev);
            assert!(
                !text.is_empty() && !mark.is_empty(),
                "sev {sev} has no words"
            );
            assert!(level >= last, "sev {sev} graded softer than {}", sev - 1);
            last = level;
        }
    }

    /// The two middle severities deliberately collapse: this panel says the
    /// pipeline is working for it, and leaves the distinction to the diagnostics
    /// column that has the numbers.
    #[test]
    fn good_and_marginal_read_the_same_here() {
        assert_eq!(decide(1), decide(2));
        assert_ne!(decide(0), decide(1));
        assert_ne!(decide(2), decide(3));
    }
}
