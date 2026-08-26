//! The mode strip and its lead card.
//!
//! The rail's mode auto-follows what the user is doing — tuning switches to
//! Hunt, touching gain switches to Bench, and it decays back to Monitor — so the
//! card under the tabs shows whatever is most useful right now. Everything below
//! the card is fixed, which is what keeps the rail readable while the top of it
//! adapts.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::state::{RailMode, SdrMetrics};
use crate::ui::panels::core::spectrum::detect_peaks;
use crate::ui::widgets::band_plan::band_at;

/// Columns the full `HUNT·MONITOR·BENCH` strip needs: a leading space, then each
/// mode as ` LABEL ` (label+2) plus a one-column gap. Pure, for the width check.
fn mode_tabs_full_w() -> usize {
    1 + RailMode::ALL
        .iter()
        .map(|m| m.label().len() + 3)
        .sum::<usize>()
}

/// The `HUNT·MONITOR·BENCH` mode strip — every mode a filled ` LABEL ` chip: the
/// active one lit bright (`value_hi` bg, bold), the inactive ones the same chip in a
/// muted "inactive" fill (`border_dim` bg) so they read as selected-but-off rather
/// than plain text. Both use dark text on the fill. Falls back to 3-letter codes
/// when the rail is too narrow for the full labels, so the strip never clips
/// mid-word.
pub(super) fn mode_tabs_line(active: RailMode, iw: usize, theme: &crate::Theme) -> Line<'static> {
    let compact = mode_tabs_full_w() > iw;
    let ink = Color::Rgb(4, 6, 15); // dark text on the chip fill
    let mut spans = vec![Span::raw(" ")];
    for m in RailMode::ALL {
        let label = if compact { &m.label()[..3] } else { m.label() };
        let style = if m == active {
            Style::default()
                .fg(ink)
                .bg(theme.value_hi)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(ink).bg(theme.border_dim)
        };
        spans.push(Span::styled(format!(" {label} "), style));
        spans.push(Span::raw(" "));
    }
    Line::from(spans)
}

/// The rail's signal list: the strongest distinct spectral peaks mapped to
/// `(freq_hz, dbfs)`, strongest-first. A thin wrapper over the spectrum panel's
/// [`detect_peaks`] (shared prominence ≥ NF+10 dB + min-separation logic) plus
/// the bin→Hz map, so HUNT and the MONITOR activity count agree with the markers.
pub(super) fn rail_peaks(
    bins: &[f32],
    noise_floor: f32,
    center_hz: u64,
    sample_rate: f64,
    n: usize,
) -> Vec<(u64, f32)> {
    if sample_rate <= 0.0 || bins.is_empty() {
        return Vec::new();
    }
    let len = bins.len();
    let sep = (len / 48).max(2);
    let left_hz = center_hz as f64 - sample_rate / 2.0;
    detect_peaks(bins, noise_floor, n, sep)
        .into_iter()
        .map(|i| {
            let hz = (left_hz + i as f64 / len as f64 * sample_rate).max(0.0) as u64;
            (hz, bins[i])
        })
        .collect()
}

/// BENCH gain-health verdict from ADC saturation and clip headroom. Returns the
/// word plus severity (2=crit, 1=warn, 0=ok), so the caller picks the colour.
/// Pure for testability. `headroom_db` is `-adc_peak_dbfs`.
/// Severity mirrors [`super::signal::sat_color`]: ≥50% → crit, ≥10% → warn.
fn chain_verdict(sat_pct: f32, headroom_db: f32) -> (&'static str, i8) {
    if sat_pct >= 50.0 {
        ("clipping", 2)
    }
    // rail hits → back off now
    else if sat_pct >= 10.0 {
        ("hot", 1)
    }
    // high level → nudge down
    else if headroom_db > 45.0 {
        ("low", 1)
    }
    // lots of room → add gain
    else {
        ("optimal", 0)
    }
}

/// The mode-adaptive lead card that sits between the mode strip and the SIGNAL
/// zone. Only this block changes with the mode; everything below is fixed.
pub(super) fn mode_card_lines(
    mode: RailMode,
    state: &SdrMetrics,
    stale: bool,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    let dim = |s: String| {
        Line::from(vec![
            Span::raw(" "),
            Span::styled(s, Style::default().fg(theme.stale)),
        ])
    };

    match mode {
        // HUNT — the three strongest signals on screen, with band tags.
        RailMode::Hunt => {
            let fft = state.waterfall.last_fft.as_ref().filter(|_| !stale);
            let Some(fr) = fft else {
                return vec![dim("scanning…".into())];
            };
            let peaks = rail_peaks(
                &fr.bins_dbfs,
                fr.noise_floor,
                state.radio.frequency,
                fr.sample_rate,
                3,
            );
            if peaks.is_empty() {
                return vec![dim("no peaks".into())];
            }
            peaks
                .into_iter()
                .enumerate()
                .map(|(i, (hz, db))| {
                    let mark = if i == 0 { "▸" } else { " " };
                    let mut spans = vec![
                        Span::styled(mark.to_string(), Style::default().fg(theme.value_hi)),
                        Span::styled(
                            format!("{:7.2}", hz as f64 / 1e6),
                            Style::default().fg(if i == 0 { theme.value_hi } else { theme.value }),
                        ),
                        Span::styled(format!(" {db:4.0}"), Style::default().fg(theme.label)),
                    ];
                    if let Some(b) = band_at(hz) {
                        spans.push(Span::styled(
                            format!("  {b}"),
                            Style::default().fg(theme.border_accent),
                        ));
                    }
                    Line::from(spans)
                })
                .collect()
        }

        // MONITOR — a calm watch headline: signal quality + how many signals are up.
        RailMode::Monitor => {
            let snr = state.signal.peak_to_nf_db;
            let (word, col) = if stale {
                ("—", theme.stale)
            } else if snr >= 20.0 {
                ("strong", theme.status_ok)
            } else if snr >= 10.0 {
                ("fair", theme.value)
            } else {
                ("quiet", theme.label)
            };
            // detect_peaks already gates on NF+10 dB, so its count is the activity.
            let n_active = state
                .waterfall
                .last_fft
                .as_ref()
                .filter(|_| !stale)
                .map_or(0, |fr| {
                    rail_peaks(
                        &fr.bins_dbfs,
                        fr.noise_floor,
                        state.radio.frequency,
                        fr.sample_rate,
                        8,
                    )
                    .len()
                });
            vec![
                Line::from(vec![
                    Span::raw(" "),
                    Span::styled("WATCH ", Style::default().fg(theme.label)),
                    Span::styled(
                        word.to_string(),
                        Style::default().fg(col).add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(vec![
                    Span::raw(" "),
                    Span::styled(
                        format!("{n_active}"),
                        Style::default()
                            .fg(theme.value)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" active", Style::default().fg(theme.label)),
                ]),
            ]
        }

        // BENCH — gain-chain health: clip headroom + a one-word verdict.
        RailMode::Bench => {
            let streaming = state.radio.hw_streaming;
            let power = state.signal.adc_peak_dbfs;
            let headroom = if streaming {
                (-power).max(0.0)
            } else {
                f32::NAN
            };
            let sat = state.signal.adc_saturation_pct;
            let (verdict_str, vcol) = if streaming {
                let (v, sev) = chain_verdict(sat, headroom);
                let col = match sev {
                    2 => theme.status_crit,
                    1 => theme.status_warn,
                    _ => theme.status_ok,
                };
                (v.to_string(), col)
            } else {
                ("\u{2014}".to_string(), theme.stale)
            };
            let hstr = if headroom.is_finite() {
                format!("{headroom:.0} dB")
            } else {
                "\u{2014}".into()
            };
            vec![
                Line::from(vec![
                    Span::raw(" "),
                    Span::styled("HEADROOM ", Style::default().fg(theme.label)),
                    Span::styled(
                        hstr,
                        Style::default()
                            .fg(theme.value)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(vec![
                    Span::raw(" "),
                    Span::styled("CHAIN ", Style::default().fg(theme.label)),
                    Span::styled(
                        verdict_str,
                        Style::default().fg(vcol).add_modifier(Modifier::BOLD),
                    ),
                ]),
            ]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rail_peaks_maps_bins_to_frequency_strongest_first() {
        // Two lobes above the −80 dB noise floor: a tall one left of centre
        // (bin 2), a shorter one right (bin 6). 10 Msps @ 100 MHz → 95..105 MHz.
        let bins = [
            -90.0, -40.0, -10.0, -40.0, -80.0, -50.0, -25.0, -50.0, -90.0,
        ];
        let peaks = rail_peaks(&bins, -80.0, 100_000_000, 10_000_000.0, 3);
        assert_eq!(peaks.len(), 2, "two distinct lobes above NF+10");
        assert!(peaks[0].1 > peaks[1].1, "strongest first");
        assert!((peaks[0].1 - (-10.0)).abs() < 1e-3);
        // Bin 2 of 9 maps below centre; bin 6 above it.
        assert!(peaks[0].0 < 100_000_000 && peaks[1].0 > 100_000_000);
    }

    #[test]
    fn rail_peaks_empty_without_signal_or_rate() {
        // All near the floor → nothing clears NF+10 dB.
        assert!(rail_peaks(&[-90.0, -88.0, -90.0], -90.0, 100_000_000, 6_000_000.0, 3).is_empty());
        // No sample rate → no usable frequency map.
        assert!(rail_peaks(&[-90.0, -10.0, -90.0], -90.0, 100_000_000, 0.0, 3).is_empty());
    }

    #[test]
    fn mode_tabs_full_width_is_the_label_budget() {
        // " HUNT " + gap + " MONITOR " + gap + " BENCH " + gap, plus leading space.
        // Each chip is label+2 (pad spaces) + 1 gap = label+3:
        // (4+3) + (7+3) + (5+3) + 1 = 26.
        assert_eq!(mode_tabs_full_w(), 26);
        // Compact kicks in below that — the strip then uses 3-letter codes.
        assert!(mode_tabs_full_w() > 20, "narrow rail must compact");
        assert!(mode_tabs_full_w() <= 28, "wide rail shows full labels");
    }

    #[test]
    fn chain_verdict_reads_saturation_and_headroom() {
        assert_eq!(chain_verdict(60.0, 10.0), ("clipping", 2)); // ≥50% → crit
        assert_eq!(chain_verdict(20.0, 10.0), ("hot", 1)); // 10-50% → warn
        assert_eq!(chain_verdict(0.0, 60.0), ("low", 1)); // lots of headroom
        assert_eq!(chain_verdict(0.0, 20.0), ("optimal", 0));
    }
}
