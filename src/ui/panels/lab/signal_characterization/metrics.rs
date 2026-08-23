//! `SIGNAL METRICS` — the five scalars, all read off the same coherent FFT frame
//! so they agree with the bonded spectrum beside the panel.

use ratatui::text::{Line, Span};

use crate::state::FftFrame;
use crate::ui::chrome::section;
use crate::ui::widgets::micro_common::fmt_bw;

use super::row::{annotated, dash, fmt_freq, metric, val};

/// The five rows, in order. The idle branch prints the same five names against
/// dashes, so the stack below does not jump when the measurement comes and goes.
pub(super) fn lines(
    out: &mut Vec<Line<'static>>, frame: Option<&FftFrame>, iw: usize, theme: &crate::Theme,
) {
    out.push(section("SIGNAL METRICS", "", iw, theme));
    let Some(fr) = frame else {
        for name in ["Channel power", "Peak", "Noise floor", "Occupied BW", "Peak hold"] {
            out.push(metric(name, vec![dash(theme)], theme));
        }
        return;
    };

    out.push(metric("Channel power", if fr.channel_power_dbfs.is_finite() {
        vec![Span::styled(format!("{:.1} dBFS", fr.channel_power_dbfs), val(theme))]
    } else { vec![dash(theme)] }, theme));

    out.push(metric("Peak", match peak_bin(fr) {
        Some((lvl, hz)) => annotated(format!("{lvl:.1} dBFS"), fmt_freq(hz), iw, theme),
        None => vec![dash(theme)],
    }, theme));

    let nf = format!("{:.1} dBFS", fr.noise_floor);
    out.push(metric("Noise floor", match noise_density_dbfs_hz(fr.noise_floor, fr.enbw_hz) {
        Some(d) => annotated(nf, format!("{d:.1} dBFS/Hz"), iw, theme),
        None    => vec![Span::styled(nf, val(theme))],
    }, theme));

    out.push(metric("Occupied BW", if fr.occupied_bw_hz > 0 {
        annotated(fmt_bw(fr.occupied_bw_hz), "99% power".to_string(), iw, theme)
    } else { vec![dash(theme)] }, theme));

    // Same scope as `peak_bin`: a peak hold sitting on the DC artefact
    // since the session started, or on a station the radio is not tuned to,
    // is the least useful number on the panel.
    let ph = crate::signal::fft::strongest_real_bin(
        &fr.peak_hold,
        Some(crate::signal::fft::centre_radius_bins(fr.peak_hold.len(), fr.sample_rate)),
    ).map(|(_, v)| v);
    out.push(metric("Peak hold", match ph {
        Some(v) if v.is_finite() => vec![Span::styled(format!("{v:.1} dBFS"), val(theme))],
        _ => vec![dash(theme)],
    }, theme));
}

/// Noise floor as a power spectral density, `dBFS/Hz`, or `None` when the frame
/// carries no usable resolution bandwidth.
///
/// The one figure on this panel that is genuinely meaningless on its own. Every
/// other level here is a power — a peak, a channel, an adjacent band — and a power
/// is a power whatever the analyser's resolution. A noise floor is not: it is the
/// power that happened to land in one bin, so it rises with the bin width and says
/// as much about the sample rate as about the radio.
///
/// Measured on the same station at two rates: the floor read -81.1 dBFS at 2 Msps
/// and -73.8 dBFS at 10 Msps, a 7.3 dB difference that is entirely the 5× wider bin
/// (theory says 7.0). As a density the same two readings are -112.8 and
/// -112.5 dBFS/Hz — the same radio, correctly reported as the same radio.
///
/// Shown *beside* the per-bin figure rather than instead of it, because the per-bin
/// number is the one that matches where the noise visually sits on the trace next
/// to this panel.
fn noise_density_dbfs_hz(noise_floor_dbfs: f32, enbw_hz: f64) -> Option<f32> {
    if !enbw_hz.is_finite() || enbw_hz <= 0.0 || !noise_floor_dbfs.is_finite() { return None; }
    Some(noise_floor_dbfs - 10.0 * enbw_hz.log10() as f32)
}

/// The strongest live bin as `(level_dbfs, freq_hz)`, mapping the bin index back to
/// frequency across the captured span. `None` for an empty frame.
///
/// Skips the DC artefact and looks only near centre, via the same helper and radius
/// the FFT worker's own peak search uses. Without the guard this row names the tuned
/// frequency and the front end's LO leakage every time the channel is quiet, which
/// is exactly when the reading matters; without the radius it can name a station the
/// radio is not tuned to, and then disagree with the headline computed above it.
fn peak_bin(fr: &FftFrame) -> Option<(f32, u64)> {
    let bins = &fr.bins_dbfs;
    let n = bins.len();
    let radius = crate::signal::fft::centre_radius_bins(n, fr.sample_rate);
    let (idx, best) = crate::signal::fft::strongest_real_bin(bins, Some(radius))?;
    let left = fr.center_freq_hz as f64 - fr.sample_rate / 2.0;
    let span_frac = if n > 1 { idx as f64 / (n - 1) as f64 } else { 0.0 };
    let freq = (left + span_frac * fr.sample_rate).max(0.0).round() as u64;
    Some((best, freq))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Instant;

    fn frame(bins: Vec<f32>, center: u64, sr: f64) -> FftFrame {
        FftFrame {
            peak_hold: Arc::new(bins.clone()),
            bins_dbfs: Arc::new(bins),
            noise_floor: -90.0,
            center_freq_hz: center,
            sample_rate: sr,
            timestamp: Instant::now(),
            peak_to_nf_db: 40.0,
            channel_power_dbfs: -22.0,
            occupied_bw_hz: 180_000,
            enbw_hz: 1_000.0,
        }
    }

    /// 101 bins across 400 kHz centred at 100 MHz → span 99.8..100.2 MHz, 3.96 kHz
    /// a bin, centre bin at index 50. At that resolution the ±150 kHz centre radius
    /// is 38 bins, so there is room either side of it to test both edges of the rule.
    fn peaked_at(idx: usize) -> FftFrame {
        let mut bins = vec![-80.0f32; 101];
        bins[idx] = -10.0;
        frame(bins, 100_000_000, 400_000.0)
    }

    #[test]
    fn noise_density_is_the_same_radio_at_any_resolution() {
        // The two readings that started this, measured on 92.8 MHz at 2 and 10 Msps.
        // Per bin they differ by 7.3 dB and describe the sample rate; as densities
        // they agree, and describe the radio.
        let two  = noise_density_dbfs_hz(-81.1, 976.5625 * 1.5).unwrap();
        let ten  = noise_density_dbfs_hz(-73.8, 4882.8125 * 1.5).unwrap();
        assert!((two - (-112.8)).abs() < 0.2, "2 Msps: {two:.1}");
        assert!((ten - (-112.5)).abs() < 0.2, "10 Msps: {ten:.1}");
        assert!((two - ten).abs() < 0.5, "densities must agree: {two:.1} vs {ten:.1}");
    }

    #[test]
    fn noise_density_declines_without_a_resolution_bandwidth() {
        assert!(noise_density_dbfs_hz(-81.0, 0.0).is_none());
        assert!(noise_density_dbfs_hz(-81.0, f64::NAN).is_none());
        assert!(noise_density_dbfs_hz(f32::NEG_INFINITY, 1_465.0).is_none());
    }

    #[test]
    fn peak_bin_maps_index_to_frequency() {
        let (lvl, hz) = peak_bin(&peaked_at(75)).unwrap();
        assert!((lvl + 10.0).abs() < 1e-6, "peak level is the max bin");
        assert_eq!(hz, 100_100_000, "three quarters across the span");
    }

    #[test]
    fn peak_bin_stays_inside_the_tuned_channel() {
        // A louder signal further out is somebody else's station. Naming it here
        // would also put this row at odds with the headline above it, which is
        // scoped the same way.
        let mut bins = vec![-80.0f32; 101];
        bins[75] = -30.0; // inside the radius
        bins[5]  = 0.0;   // far outside it, and much louder
        let (lvl, hz) = peak_bin(&frame(bins, 100_000_000, 400_000.0)).unwrap();
        assert!((lvl + 30.0).abs() < 1e-6, "reported a station out of channel: {lvl}");
        assert_eq!(hz, 100_100_000);
    }

    #[test]
    fn peak_bin_skips_the_dc_artefact() {
        // The whole point of the guard: on a quiet channel the LO leakage is the
        // tallest bin, and naming it would put the tuned frequency in this row as
        // though a station were there. The next real bin is the honest answer.
        let mut bins = vec![-80.0f32; 101];
        bins[50] = 0.0;   // artefact at centre, by far the strongest
        bins[60] = -30.0; // a real, weaker carrier
        let (lvl, hz) = peak_bin(&frame(bins, 100_000_000, 400_000.0)).unwrap();
        assert!((lvl + 30.0).abs() < 1e-6, "reported the artefact: {lvl}");
        assert_eq!(hz, 100_040_000);
    }

    #[test]
    fn peak_bin_empty_frame_is_none() {
        let fr = frame(vec![], 100_000_000, 2_000_000.0);
        assert!(peak_bin(&fr).is_none());
    }
}
