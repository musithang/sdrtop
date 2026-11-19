// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Shared formatting for the timing read-outs, so every panel that prints a
//! period, a drift or a verdict prints it the same way.
//!
//! These lived in `timing_panel` until that panel was retired: it was the
//! original single-panel `lab_timing` view, superseded by the three-panel bench
//! (`timing_diagnostics` · `timing_stripchart` · `timing_vitals`) and left
//! registered but unreachable. Its helpers had three callers by then, so they
//! outlived it and belong somewhere that isn't a panel.

use ratatui::{style::Style, text::Span};

use crate::state::TimingQuality;

/// A sample rate in Msps, without the unit: one decimal from 1 Msps up, as it
/// always was (`20.0`, ` 2.4`), and three below it, where one decimal reads
/// 768 and 912 kS/s alike as `0.8`; more where those would round a rate the
/// radio really runs at onto another, so 2.048 Msps is never `2.0` beside
/// 2.0 (an RTL-SDR's or an RSP1B's rate list) and 62.5 kS/s is `0.0625`.
pub(crate) fn fmt_msps(hz: f64) -> String {
    let msps = hz / 1e6;
    let least = if msps < 1.0 { 3 } else { 1 };
    let places = (least..=6)
        .find(|&d| {
            let scaled = msps * 10f64.powi(d);
            (scaled - scaled.round()).abs() < 1e-6
        })
        .unwrap_or(6) as usize;
    if msps < 1.0 || places > 1 {
        format!("{msps:.places$}")
    } else {
        format!("{msps:4.1}")
    }
}

/// Microseconds rendered as `ms` once they pass 1000 µs, else plain `µs`.
/// How long ago something was measured, at the resolution a reading's age is
/// read at: `3 s ago`, then `4 min ago` from ninety seconds on.
pub(crate) fn ago(elapsed: std::time::Duration) -> String {
    let secs = elapsed.as_secs();
    if secs < 90 {
        format!("{secs} s ago")
    } else {
        format!("{} min ago", secs / 60)
    }
}

/// A stretch of seconds given in milliseconds, at the resolution a plot's
/// time axis is read at: `0.5 s`, `2.0 s`, then whole seconds from ten on.
pub(crate) fn seconds_ms(ms: u64) -> String {
    if ms < 10_000 {
        format!("{:.1} s", ms as f64 / 1e3)
    } else {
        format!("{} s", ms / 1_000)
    }
}

pub(crate) fn fmt_us(us: u64) -> String {
    if us >= 1_000 {
        format!("{:.3} ms", us as f64 / 1_000.0)
    } else {
        format!("{} µs", us)
    }
}

/// Signed ppm value, colored by absolute magnitude (green / yellow / red).
///
/// A relayed stream's clock is read only to its scatter through the service,
/// and is not graded under [`crate::state::RELAYED_CLOCK_FLOOR_PPM`]; under
/// it the colour does not grade it either.
pub(crate) fn ppm_span(
    ppm: i64,
    delivery: crate::hardware::DeliveryModel,
    theme: &crate::Theme,
) -> Span<'static> {
    let mag = ppm.unsigned_abs();
    let unresolved = delivery == crate::hardware::DeliveryModel::Relayed
        && mag <= crate::state::RELAYED_CLOCK_FLOOR_PPM;
    let color = if mag < 50 || unresolved {
        theme.status_ok
    } else if mag < 200 {
        theme.status_warn
    } else {
        theme.status_crit
    };
    Span::styled(format!("{:+} ppm", ppm), Style::default().fg(color))
}

/// The verdict's colour, taken from its own severity ranking so the scale is
/// defined once in `TimingQuality` rather than per panel.
pub(crate) fn quality_color(q: TimingQuality, theme: &crate::Theme) -> ratatui::style::Color {
    match q.severity() {
        0 => theme.status_ok,
        1 => theme.value_hi,
        2 => theme.status_warn,
        _ => theme.status_crit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_us_switches_to_ms() {
        assert_eq!(fmt_us(450), "450 µs");
        assert_eq!(fmt_us(13_107), "13.107 ms");
    }

    #[test]
    fn quality_color_matches_severity() {
        let t = crate::theme::Theme::sdr();
        assert_eq!(quality_color(TimingQuality::Excellent, &t), t.status_ok);
        assert_eq!(quality_color(TimingQuality::Marginal, &t), t.status_warn);
        assert_eq!(quality_color(TimingQuality::Poor, &t), t.status_crit);
    }

    #[test]
    fn ppm_span_color_thresholds() {
        let t = crate::theme::Theme::sdr();
        let push = crate::hardware::DeliveryModel::Push;
        assert_eq!(ppm_span(10, push, &t).style.fg, Some(t.status_ok));
        assert_eq!(ppm_span(-120, push, &t).style.fg, Some(t.status_warn));
        assert_eq!(ppm_span(600, push, &t).style.fg, Some(t.status_crit));
    }

    /// Through a relaying service the clock is not resolved under 200 ppm,
    /// so it is not coloured as off there either; past it, as any other.
    #[test]
    fn a_relayed_clock_is_coloured_only_where_it_is_graded() {
        let t = crate::theme::Theme::sdr();
        let relayed = crate::hardware::DeliveryModel::Relayed;
        assert_eq!(ppm_span(-120, relayed, &t).style.fg, Some(t.status_ok));
        assert_eq!(ppm_span(200, relayed, &t).style.fg, Some(t.status_ok));
        assert_eq!(ppm_span(201, relayed, &t).style.fg, Some(t.status_crit));
    }

    /// Below 1 Msps three decimals, so 768 and 912 kS/s are two rates; from
    /// 1 Msps up the one decimal and the width it always had.
    #[test]
    fn a_rate_under_one_msps_keeps_its_kilosamples() {
        assert_eq!(fmt_msps(768_000.0), "0.768");
        assert_eq!(fmt_msps(912_000.0), "0.912");
        assert_eq!(fmt_msps(2_400_000.0), " 2.4");
        assert_eq!(fmt_msps(20_000_000.0), "20.0");
        assert_eq!(fmt_msps(2_048_000.0), "2.048", "not 2.0 beside 2.0");
        assert_eq!(fmt_msps(62_500.0), "0.0625");
        assert_eq!(fmt_msps(2_560_000.0), "2.56");
        assert_eq!(fmt_msps(10_000_000.0), "10.0");
    }
}
