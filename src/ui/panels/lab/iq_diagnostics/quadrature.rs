//! QUADRATURE section: how far the two channels are from balance.
//!
//! Both readings are deviations from zero that can err either way, so both are
//! null-meters. What they cost in image rejection is [`super::image`]'s job.

use ratatui::text::Line;

use crate::ui::chrome::section;

use super::reading::Reading;
use super::rows::Rows;
use super::severity::{imbalance_color, phase_color};

/// Meter full-scales. Wider than the crit thresholds in [`super::severity`], so a
/// front end that is genuinely bad still moves the needle instead of pinning it
/// and looking the same as one that is merely poor.
const AMP_FULL_SCALE_DB: f64 = 4.0;
const PHASE_FULL_SCALE_DEG: f64 = 6.0;

pub(super) fn lines(r: &Reading, rows: &Rows) -> Vec<Line<'static>> {
    let theme = rows.theme;
    vec![
        section("Quadrature", "gain \u{00b7} phase balance", rows.iw, theme),
        rows.meter(
            "AMP",
            r.amp_db as f64,
            AMP_FULL_SCALE_DB,
            imbalance_color(r.amp_db.abs(), theme),
            format!("{:+.2} dB", r.amp_db),
        ),
        Line::raw(""),
        rows.meter(
            "PHA",
            r.phase_deg as f64,
            PHASE_FULL_SCALE_DEG,
            phase_color(r.phase_deg.abs(), theme),
            format!("{:+.2}\u{b0}", r.phase_deg),
        ),
    ]
}
