//! How bad a column is, and which way it blew out.
//!
//! Note the level-0 colour: [`theme.value`](crate::Theme), not `status_ok`. This
//! grades a **chart trace**, not a verdict — a plot in which every in-budget
//! sample is bright green reads as a wall of green, and the eye stops finding the
//! few that are not. `rf_bench::severity_color` has the same name and signature
//! and uses `status_ok`, because it grades a verdict. The two are not
//! interchangeable.

use ratatui::style::Color;

/// Severity of a column's worst deviation against the budget: 0 in budget,
/// 1 over budget, 2 more than double over. Drives the bar colour.
pub(super) fn dev_severity(max_abs_us: u64, budget_us: u64) -> u8 {
    if budget_us == 0 || max_abs_us <= budget_us {
        0
    } else if max_abs_us <= budget_us * 2 {
        1
    } else {
        2
    }
}

pub(super) fn severity_color(sev: u8, theme: &crate::Theme) -> Color {
    match sev {
        0 => theme.value,
        1 => theme.status_warn,
        _ => theme.status_crit,
    }
}

/// Direction of a column's over-range spike, from its two samples: `+1` if the
/// worst (largest-magnitude) sample is late (positive), `−1` if it is early
/// (negative). Decides whether the spike tag is drawn as `▲` at the top of the
/// chart (a late overrun) or `▼` at the bottom (an early one).
pub(super) fn over_tag_sign(a: i32, b: i32) -> i8 {
    let worst = if a.unsigned_abs() >= b.unsigned_abs() {
        a
    } else {
        b
    };
    if worst < 0 {
        -1
    } else {
        1
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

    /// This scale is a trace tint, so "fine" is the neutral value colour rather
    /// than the green a verdict would use. Pinned because the two functions share
    /// a name across the lab.
    #[test]
    fn in_budget_is_neutral_not_green() {
        let t = crate::Theme::sdr();
        assert_eq!(severity_color(0, &t), t.value);
        assert_ne!(severity_color(0, &t), t.status_ok);
        assert_eq!(severity_color(1, &t), t.status_warn);
        assert_eq!(severity_color(2, &t), t.status_crit);
    }
}
