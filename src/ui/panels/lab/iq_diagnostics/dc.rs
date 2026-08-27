//! DC OFFSET section: where the I/Q centroid sits, and what that costs at the LO.
//!
//! I and Q are deviations from zero, so they get null-meters; the magnitude is a
//! quality that runs one way, so it gets a gradient bar; the spike is a level, so
//! it is a plain readout.

use ratatui::text::Line;

use crate::ui::chrome::section;

use super::reading::Reading;
use super::rows::Rows;
use super::severity::{offset_color, spike_color};

/// Full-scale of the I and Q null-meters, in raw offset units. Five times the
/// `±0.010` target on the nameplate, so a reading at target sits visibly inside
/// the meter rather than pinning it.
const OFFSET_FULL_SCALE: f64 = 0.05;

pub(super) fn lines(r: &Reading, rows: &Rows) -> Vec<Line<'static>> {
    let theme = rows.theme;
    let mut out = vec![section("DC offset", "target \u{00b1}0.010", rows.iw, theme)];

    out.push(rows.meter(
        "I",
        r.dc_i as f64,
        OFFSET_FULL_SCALE,
        offset_color(r.dc_i.abs(), theme),
        format!("{:+.4}", r.dc_i),
    ));
    out.push(Line::raw(""));
    out.push(rows.meter(
        "Q",
        r.dc_q as f64,
        OFFSET_FULL_SCALE,
        offset_color(r.dc_q.abs(), theme),
        format!("{:+.4}", r.dc_q),
    ));
    out.push(Line::raw(""));

    out.push(rows.bar(
        "MAG",
        r.dc_mag / OFFSET_FULL_SCALE,
        theme.status_ok,
        theme.status_crit,
        offset_color(r.dc_mag as f32, theme),
        format!("{:.4}", r.dc_mag),
    ));
    out.push(Line::raw(""));

    let (spike_str, spike_col) = match r.spike_dbfs {
        Some(s) => (format!("{s:.1} dBFS"), spike_color(s, theme)),
        None => ("\u{2014}".to_string(), theme.label),
    };
    out.push(rows.readout("DC spike @ LO", spike_str, spike_col));
    out
}
