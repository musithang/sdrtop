//! The action chips and the status foot: what the corrections are doing now.
//!
//! Separate from [`super::verdict`] because it reports state rather than judging
//! it — the chips light from `IqCalState` directly, and say nothing about whether
//! the readings are good.

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::state::IqCalState;

use super::rows::Rows;

/// Inner width at which the full chip labels fit. Below it they fall back to
/// single letters, or the freeze chip is clipped off the right edge.
const FULL_LABEL_MIN_W: usize = 37;

pub(super) fn lines(cal: &IqCalState, rows: &Rows) -> Vec<Line<'static>> {
    let (d, c, f) = if rows.iw >= FULL_LABEL_MIN_W {
        ("D DC-block", "C auto-cal", "F freeze")
    } else {
        ("D", "C", "F")
    };
    vec![
        Line::from(vec![
            Span::raw(" "),
            rows.chip(d, cal.dc_block_on),
            Span::raw(" "),
            rows.chip(c, cal.cal_applied),
            Span::raw(" "),
            rows.chip(f, cal.frozen),
        ]),
        Line::from(vec![
            Span::raw(" "),
            Span::styled(foot(cal, now_secs()), Style::default().fg(rows.dim())),
        ]),
    ]
}

/// The status foot, as text. `now` is passed in so the "last cal Xm ago" branch
/// can be tested without waiting or mocking a clock.
fn foot(cal: &IqCalState, now: u64) -> String {
    let dc = if cal.dc_block_on {
        "DC-block ON"
    } else {
        "DC-block OFF"
    };
    let state = if cal.cal_applied {
        "auto-cal applied"
    } else if cal.cal_pending {
        "auto-cal capturing\u{2026}"
    } else {
        "auto-cal idle"
    };
    let mut out = format!("{dc} \u{00b7} {state}");
    if let Some(t) = cal.last_cal_at {
        let ago = now.max(t).saturating_sub(t);
        let ago_str = if ago < 60 {
            format!("{ago}s")
        } else {
            format!("{}m", ago / 60)
        };
        out.push_str(&format!(" \u{00b7} last cal {ago_str} ago"));
    }
    out
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_foot_reports_all_three_states_of_auto_cal() {
        let idle = IqCalState::default();
        assert!(foot(&idle, 100).contains("auto-cal idle"));

        let pending = IqCalState {
            cal_pending: true,
            ..Default::default()
        };
        assert!(foot(&pending, 100).contains("capturing"));

        let applied = IqCalState {
            cal_applied: true,
            cal_pending: true,
            ..Default::default()
        };
        assert!(
            foot(&applied, 100).contains("auto-cal applied"),
            "applied outranks pending"
        );
    }

    #[test]
    fn the_age_switches_from_seconds_to_minutes() {
        let cal = |t| IqCalState {
            last_cal_at: Some(t),
            ..Default::default()
        };
        assert!(foot(&cal(100), 130).contains("last cal 30s ago"));
        assert!(foot(&cal(100), 160).contains("last cal 1m ago"));
        assert!(!foot(&IqCalState::default(), 160).contains("last cal"));
    }

    /// A clock that steps backwards (NTP, suspend) must not produce a huge age
    /// from a subtraction that wrapped.
    #[test]
    fn a_backwards_clock_reads_zero_rather_than_forever() {
        let cal = IqCalState {
            last_cal_at: Some(500),
            ..Default::default()
        };
        assert!(foot(&cal, 100).contains("last cal 0s ago"), "{}", foot(&cal, 100));
    }
}
