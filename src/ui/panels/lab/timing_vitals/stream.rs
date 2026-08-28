//! What the radio delivers: sample drops and ADC saturation, each as a 60 s trend.
//!
//! Both are counted in the hot path and both mean "the stream is not clean", but
//! for different reasons — drops are the host failing to keep up, saturation is
//! the front end being over-driven. Side by side because a reader is deciding
//! which of the two it is.

use ratatui::text::Line;

use crate::state::SdrMetrics;
use crate::ui::widgets::micro_common::{drop_color, sat_color};

use super::rows::Rows;

pub(super) fn lines(state: &SdrMetrics, r: &Rows) -> Vec<Line<'static>> {
    let mut out = Vec::new();

    let drops = &state.signal;
    out.extend(r.trend(
        r.heading(
            "Sample drops ",
            format!("{}/s", drops.drops_per_sec),
            drop_color(drops.drops_per_sec, r.theme),
            format!("   session {}", drops.total_drops_session),
        ),
        drops.drop_history.iter().map(|&v| v as f64).collect(),
    ));
    out.push(Line::raw(""));

    out.extend(r.trend(
        r.heading(
            "ADC saturation ",
            format!("{:.1} %", drops.adc_saturation_pct),
            sat_color(drops.adc_saturation_pct, r.theme),
            format!("   peak {:.1}%", drops.adc_saturation_peak),
        ),
        drops.saturation_history.iter().map(|&v| v as f64).collect(),
    ));
    out
}
