// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `CLOCK ERROR` - the room's crystals, worst first, one null meter each.
//!
//! Bluetooth design measurement 18: "every Bluetooth device in range, sorted by
//! how bad its clock is", plotted as a distribution (net-ux-polish-plan 4.3).
//! Rendering only: the offsets are the census's, corrected exactly as the CFO
//! column corrects them (`RadioState::corrected_ppm`), so a meter and the
//! table cannot disagree about a clock.
//!
//! **The Lab's own null meter, one per device, on one scale.** `┃` is zero,
//! the needle is the reading and the fill runs between them, so the longer the
//! bar the worse the clock; every row shares the scale, so lengths compare.
//! The first attempt at this picture was a histogram over stacked error bars
//! and was removed from the live screen as unreadable; a row that is labelled
//! with its device, drawn in an idiom the Lab panels already taught, and has
//! its number beside it needs no explaining.
//!
//! **The specification's limit, only where it can be judged.** Core 5.4 Vol 6
//! Part A 3.3 holds a transmitter's centre frequency to ±150 kHz. With a
//! frequency reference the offsets are the devices' own errors, the limit is
//! marked on every track (`╎`) and each meter is coloured by where its reading
//! sits against it. Without one, every offset still includes our own
//! oscillator's error, so a red "outside" would be a verdict nobody measured:
//! no marks, one neutral ink, and the section rule says why.
//!
//! **±150 kHz is two ppm figures, and the colour respects both.** The census
//! combines a device's readings from all three advertising channels in ppm
//! (`census::Device::crystal_offset_ppm`), and 150 kHz is 62.4 ppm of
//! channel 37 but 60.5 ppm of channel 39. A clock inside the smaller figure is
//! inside on every channel and one outside the larger is outside on every
//! channel; between them it depends on which channel, and it is amber, as is
//! anything the reading's own uncertainty cannot place (idiom B's rule, at the
//! same two sigma).

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::signal::dsp::uncertainty::Uncertain;
use crate::signal::net::census::Device;
use crate::state::{Provenance, SdrMetrics};
use crate::ui::widgets::charts::{null_meter_column, null_meter_marked};
use crate::ui::widgets::reading::Reading;

/// Core 5.4 Vol 6 Part A 3.3: "The deviation of the center frequency during
/// the packet shall not exceed ±150 kHz, including both the initial frequency
/// offset and drift."
const CENTRE_TOLERANCE_HZ: f64 = 150e3;

/// The coverage the colour is decided at: two sigma, idiom B's own
/// (`widgets::limit`), because the question is the same one - can this
/// instrument tell which side of the line the clock is on.
const COVERAGE_K: f64 = 2.0;

/// Columns the address takes: a full address, the narrowest the table draws.
const LABEL_W: usize = crate::state::FULL_ADDRESS_WIDTH;

/// The narrowest track worth drawing. Below it the needle has too few places
/// to stand for a length to mean anything, and the block is not drawn.
const MIN_TRACK: usize = 15;

/// The limit in ppm, as `(inside on every channel, outside on every channel)`:
/// ±150 kHz of the highest advertising channel and of the lowest.
fn limit_ppm() -> (f64, f64) {
    let ppm = |ch: u8| {
        crate::signal::ble::channel::centre_hz(ch)
            .map(|hz| CENTRE_TOLERANCE_HZ / hz as f64 * 1e6)
            .unwrap_or(f64::NAN)
    };
    (ppm(39), ppm(37))
}

/// Where a clock sits against the limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    /// Inside on every channel, by more than the reading's uncertainty.
    Inside,
    /// Too close to call: within the uncertainty of the line, or between the
    /// two channels' figures.
    Close,
    /// Outside on every channel, by more than the uncertainty.
    Outside,
}

fn verdict(u: Uncertain) -> Verdict {
    let (inside, outside) = limit_ppm();
    let (v, k) = (u.value().abs(), u.expanded(COVERAGE_K));
    if v + k <= inside {
        Verdict::Inside
    } else if v - k >= outside {
        Verdict::Outside
    } else {
        Verdict::Close
    }
}

/// The smallest of a short list of round full scales that holds `worst`.
fn full_scale(worst: f64) -> f64 {
    [5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0]
        .into_iter()
        .find(|s| *s >= worst)
        .unwrap_or(worst.ceil())
}

/// The block, worst clock first, at most `max_lines` tall and `iw` wide, or
/// nothing when that is too little for a section rule, one meter and its
/// scale.
pub(super) fn lines(
    devices: &[Device],
    state: &SdrMetrics,
    now: std::time::Instant,
    iw: usize,
    max_lines: usize,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    use crate::ui::chrome::{section, selection_gutter};

    let mut clocks: Vec<(&Device, Uncertain)> = devices
        .iter()
        .filter_map(|d| Some((d, state.radio.corrected_ppm(d.crystal_offset_ppm?, now).0)))
        .filter(|(_, u)| u.value().is_finite())
        .collect();
    // Worst first; the address breaks a tie so rows do not trade places.
    clocks.sort_by(|a, b| {
        b.1.value()
            .abs()
            .total_cmp(&a.1.value().abs())
            .then_with(|| a.0.address.cmp(&b.0.address))
    });

    if clocks.is_empty() {
        if max_lines < 2 {
            return Vec::new();
        }
        return vec![
            section("clock error", "", iw, theme),
            Line::from(Span::styled(
                " no packet has reported an offset yet".to_string(),
                Style::default().fg(theme.label),
            )),
        ];
    }

    let values: Vec<String> = clocks
        .iter()
        .map(|(_, u)| Reading::new(*u, "ppm", f64::INFINITY).text())
        .collect();
    let value_w = values.iter().map(|v| v.chars().count()).max().unwrap_or(0);
    // Gutter, address, a space, the arrows either side of the track, a space,
    // the value.
    let track = iw.saturating_sub(1 + LABEL_W + 1 + 2 + 1 + value_w);
    if max_lines < 3 || track < MIN_TRACK {
        return Vec::new();
    }

    let judged = state.radio.offset_basis(now).provenance != Provenance::Unreferenced;
    let (inside, _) = limit_ppm();
    let worst = clocks[0].1.value().abs();
    // With a limit to show, the scale holds it, so a room of good clocks is
    // seen to be well inside rather than filling the track at a zoom that
    // hides where the line is.
    let scale = full_scale(if judged { worst.max(inside) } else { worst });
    let marks: Vec<f64> = if judged {
        vec![-inside, inside]
    } else {
        Vec::new()
    };

    let unmeasured = devices.len() - clocks.len();
    let basis = if judged {
        "spec ±150 kHz"
    } else {
        "relative, no limit without a reference"
    };
    let hint = [
        format!("worst first · {basis}"),
        basis.to_string(),
        "worst first".to_string(),
    ]
    .into_iter()
    .find(|h| h.chars().count() + 17 <= iw)
    .unwrap_or_default();
    let mut out = vec![section("clock error", &hint, iw, theme)];

    // The rule, the scale, and a line for what did not fit if anything
    // does not.
    let room = max_lines - 2;
    // One meter and no room to say how many more there are would drop the
    // rest in silence; the block waits for the room to do both.
    if clocks.len() > room && room < 2 {
        return Vec::new();
    }
    let shown = if clocks.len() <= room {
        clocks.len()
    } else {
        room.saturating_sub(1).max(1)
    };
    for ((d, u), value) in clocks.iter().zip(&values).take(shown) {
        let selected = state.net.census.selection.selected == Some(d.address);
        let ink = meter_ink(*u, judged, theme);
        let text = if selected {
            Style::default()
                .fg(theme.value_hi)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.label)
        };
        let mut spans = vec![
            selection_gutter(selected, theme),
            Span::styled(
                format!("{:<LABEL_W$}", d.address_text(&state.net, Some(LABEL_W))),
                text,
            ),
            Span::raw(" "),
        ];
        spans.extend(null_meter_marked(
            u.value(),
            scale,
            track,
            ink,
            theme.border_dim,
            &marks,
        ));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            format!("{value:>value_w$}"),
            if selected {
                text
            } else {
                Style::default().fg(theme.value)
            },
        ));
        out.push(Line::from(spans));
    }
    if shown < clocks.len() {
        // Worst first, so what is left off is the better clocks, and the
        // line says both how many and that.
        out.push(Line::from(Span::styled(
            format!(" +{} more, better clocks", clocks.len() - shown),
            Style::default().fg(theme.label),
        )));
    }
    if unmeasured > 0 && out.len() < max_lines - 1 {
        out.push(Line::from(Span::styled(
            format!(" {unmeasured} not measured yet"),
            Style::default().fg(theme.label),
        )));
    }
    out.push(ruler(
        scale,
        track,
        if judged { Some(inside) } else { None },
        theme,
    ));
    out
}

/// The meter's colour: the verdict's where there is a limit to judge by, the
/// accent where there is not.
fn meter_ink(u: Uncertain, judged: bool, theme: &crate::Theme) -> Color {
    if !judged {
        return theme.border_accent;
    }
    match verdict(u) {
        Verdict::Inside => theme.status_ok,
        Verdict::Close => theme.status_warn,
        Verdict::Outside => theme.status_crit,
    }
}

/// The scale under the tracks: the ends, zero, and either the limit (under
/// its marks) or the half-scale, each label centred on the column the meter
/// itself puts that value on (`null_meter_column`).
fn ruler(scale: f64, track: usize, limit: Option<f64>, theme: &crate::Theme) -> Line<'static> {
    let mut row = vec![' '; track];
    let inner = limit.unwrap_or(scale / 2.0);
    let fmt = |v: f64| -> String {
        if v == 0.0 {
            "0".to_string()
        } else {
            format!("{v:+.0}")
        }
    };
    // Zero and the inner pair first: when labels collide on a narrow track,
    // the ones that explain the marks win over the ends.
    for v in [0.0, -inner, inner, -scale, scale] {
        let label = fmt(v);
        let n = label.chars().count();
        let at = null_meter_column(v, scale, track);
        let start = at.saturating_sub(n / 2).min(track.saturating_sub(n));
        let clear = row[start.saturating_sub(1)..(start + n + 1).min(track)]
            .iter()
            .all(|c| *c == ' ');
        if clear {
            for (i, ch) in label.chars().enumerate() {
                row[start + i] = ch;
            }
        }
    }
    // Gutter, address and its space, and the `◄`.
    let pad = " ".repeat(1 + LABEL_W + 1 + 1);
    Line::from(Span::styled(
        format!("{pad}{} ppm", row.into_iter().collect::<String>()),
        Style::default().fg(theme.label),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 150 kHz is 60.5 ppm of channel 39 and 62.4 ppm of channel 37.
    #[test]
    fn the_limit_is_150_khz_of_each_end_of_the_advertising_band() {
        let (inside, outside) = limit_ppm();
        assert!((inside - 60.48).abs() < 0.01, "{inside}");
        assert!((outside - 62.45).abs() < 0.01, "{outside}");
    }

    /// **Amber wherever the instrument cannot say which side**: close to the
    /// line by less than two sigma, or between the two channels' figures.
    #[test]
    fn the_verdict_is_amber_where_it_cannot_be_called() {
        let u = |v, s| Uncertain::from_sigma(v, s);
        assert_eq!(verdict(u(-9.3, 0.2)), Verdict::Inside);
        assert_eq!(verdict(u(-95.6, 0.1)), Verdict::Outside);
        assert_eq!(
            verdict(u(61.5, 0.1)),
            Verdict::Close,
            "between the channels"
        );
        assert_eq!(verdict(u(59.9, 0.4)), Verdict::Close, "within 2 sigma");
        assert_eq!(verdict(u(59.9, 0.2)), Verdict::Inside);
    }

    /// Relative readings get one neutral ink whatever their size: no verdict
    /// nobody measured.
    #[test]
    fn without_a_reference_every_meter_is_the_accent() {
        let theme = crate::Theme::sdr();
        for v in [-0.3, -95.6] {
            let c = meter_ink(Uncertain::from_sigma(v, 0.1), false, &theme);
            assert_eq!(c, theme.border_accent);
        }
        assert_eq!(
            meter_ink(Uncertain::from_sigma(-95.6, 0.1), true, &theme),
            theme.status_crit
        );
    }

    #[test]
    fn the_scale_is_the_first_round_figure_that_holds_the_worst() {
        assert_eq!(full_scale(95.6), 100.0);
        assert_eq!(full_scale(60.48), 100.0);
        assert_eq!(full_scale(3.0), 5.0);
        assert_eq!(full_scale(1500.0), 1500.0);
    }

    /// **Every label sits under the column its value is drawn on.** A scale
    /// a column off reads a different number off every meter above it.
    #[test]
    fn the_ruler_labels_sit_under_their_columns() {
        let theme = crate::Theme::sdr();
        for track in [15usize, 21, 40, 57, 80] {
            let line: String = ruler(100.0, track, Some(60.48), &theme)
                .spans
                .iter()
                .map(|s| s.content.to_string())
                .collect();
            let pad = 1 + LABEL_W + 1 + 1;
            let row: Vec<char> = line.chars().skip(pad).take(track).collect();
            let zero = null_meter_column(0.0, 100.0, track);
            assert_eq!(row[zero], '0', "{track}: {line:?}");
            if let Some(i) = row.iter().position(|c| *c == '+') {
                // `+60`, centred: its middle digit on the mark's column.
                let mark = null_meter_column(60.48, 100.0, track);
                if row.get(i + 1..i + 3) == Some(&['6', '0']) {
                    assert_eq!(i + 1, mark, "{track}: {line:?}");
                }
            }
            assert!(line.ends_with(" ppm"), "{line:?}");
        }
    }
}
