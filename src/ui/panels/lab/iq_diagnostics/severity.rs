//! Threshold → colour for each reading.
//!
//! Separate from [`super::reading`] on purpose: that module is what the front end
//! *is*, this one is how strongly to say it. The verdict block reuses the same
//! boundaries as words, so they are named here once rather than repeated as
//! literals in two places.

use ratatui::style::Color;

/// DC offset per channel, in raw units. `±0.010` is the target printed on the
/// section nameplate; twice that is where it starts distorting the spectrum.
pub(super) const OFFSET_WARN: f32 = 0.005;
pub(super) const OFFSET_CRIT: f32 = 0.02;

/// Amplitude imbalance, dB.
pub(super) const AMP_WARN_DB: f32 = 1.0;
pub(super) const AMP_CRIT_DB: f32 = 3.0;

/// Phase imbalance, degrees.
pub(super) const PHASE_WARN_DEG: f32 = 2.0;
pub(super) const PHASE_CRIT_DEG: f32 = 5.0;

pub(super) fn offset_color(abs_val: f32, theme: &crate::Theme) -> Color {
    if abs_val > OFFSET_CRIT {
        theme.status_crit
    } else if abs_val > OFFSET_WARN {
        theme.status_warn
    } else {
        theme.status_ok
    }
}

pub(super) fn imbalance_color(abs_db: f32, theme: &crate::Theme) -> Color {
    if abs_db > AMP_CRIT_DB {
        theme.status_crit
    } else if abs_db > AMP_WARN_DB {
        theme.status_warn
    } else {
        theme.status_ok
    }
}

pub(super) fn phase_color(abs_deg: f32, theme: &crate::Theme) -> Color {
    if abs_deg > PHASE_CRIT_DEG {
        theme.status_crit
    } else if abs_deg > PHASE_WARN_DEG {
        theme.status_warn
    } else {
        theme.status_ok
    }
}

/// IRR is the one where **higher is better**, so the comparisons run the other
/// way round. Kept beside the others precisely so that asymmetry is visible.
pub(super) fn irr_color(irr_db: f64, theme: &crate::Theme) -> Color {
    if irr_db >= 30.0 {
        theme.status_ok
    } else if irr_db >= 20.0 {
        theme.status_warn
    } else {
        theme.status_crit
    }
}

/// Likewise: a *lower* (more negative) spike is better.
pub(super) fn spike_color(spike_dbfs: f64, theme: &crate::Theme) -> Color {
    if spike_dbfs < -40.0 {
        theme.status_ok
    } else if spike_dbfs < -20.0 {
        theme.status_warn
    } else {
        theme.status_crit
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Theme;

    #[test]
    fn the_deviation_scales_escalate_away_from_zero() {
        let t = Theme::sdr();
        assert_eq!(offset_color(0.0, &t), t.status_ok);
        assert_eq!(offset_color(OFFSET_WARN + 0.001, &t), t.status_warn);
        assert_eq!(offset_color(OFFSET_CRIT + 0.001, &t), t.status_crit);

        assert_eq!(imbalance_color(0.0, &t), t.status_ok);
        assert_eq!(imbalance_color(AMP_WARN_DB + 0.1, &t), t.status_warn);
        assert_eq!(imbalance_color(AMP_CRIT_DB + 0.1, &t), t.status_crit);

        assert_eq!(phase_color(0.0, &t), t.status_ok);
        assert_eq!(phase_color(PHASE_WARN_DEG + 0.1, &t), t.status_warn);
        assert_eq!(phase_color(PHASE_CRIT_DEG + 0.1, &t), t.status_crit);
    }

    /// The two "higher/lower is better" scales run the opposite way, which is the
    /// easiest thing in this file to get backwards.
    #[test]
    fn irr_and_spike_run_the_other_way() {
        let t = Theme::sdr();
        assert_eq!(irr_color(45.0, &t), t.status_ok, "more rejection is better");
        assert_eq!(irr_color(25.0, &t), t.status_warn);
        assert_eq!(irr_color(5.0, &t), t.status_crit);

        assert_eq!(spike_color(-60.0, &t), t.status_ok, "a lower spike is better");
        assert_eq!(spike_color(-30.0, &t), t.status_warn);
        assert_eq!(spike_color(-5.0, &t), t.status_crit);
    }
}
