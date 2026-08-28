//! Finding the carrier at centre, and measuring it.
//!
//! The single carrier analysis in the app: `carrier_window` resolves the bin
//! range once and both the occupied bandwidth and the channel power are taken
//! from it, so the two cannot describe different slices of the same spectrum.
//!
//! Everything here is pure - a slice of linear power in, numbers out - which is
//! why it carries most of this module's tests.

/// How far a bin must stand above the noise floor to be counted as part of the
/// carrier when the occupied-bandwidth window is drawn.
///
/// The 99 % cumulative method answers "where does the power sit", and handed the
/// whole capture it answers "spread across the span" - a noise floor several MHz
/// wide carries real power, so the 0.5 % / 99.5 % cut-offs land near the span edges
/// and the result describes the sample rate rather than the signal. Bounding the
/// method to the carrier first is what makes the number a property of the signal.
///
/// 10 dB is the same judgement `spectrum::PEAK_PROMINENCE_DB` already makes about
/// what separates a solid carrier from FFT ripple.
const OBW_CARRIER_THRESHOLD_DB: f32 = 10.0;

/// How far the carrier window steps over bins below the threshold before it gives
/// up. A broadcast signal's own spectrum is ragged, and a momentary null inside the
/// channel must not amputate the measurement at that bin. In Hz, so it means the
/// same thing at 2 Msps as at 20.
const OBW_GAP_TOLERANCE_HZ: f64 = 10_000.0;

/// Bins either side of centre that are treated as the DC artefact rather than as
/// signal.
///
/// Both front-ends park their DC offset and LO leakage on the centre bin, and on an
/// otherwise empty channel that artefact is the strongest thing in the spectrum. It
/// is a single spectral line, which the Hann window spreads over about three bins -
/// measured live at 447 MHz with nothing on air it stands 46 dB above the floor, and
/// unguarded it becomes the reported peak, the SNR, and a 2 kHz "carrier".
///
/// In bins rather than Hz because the width being excluded is a property of the
/// transform's resolution, not of any frequency. Two bins either side covers the
/// Hann main lobe; the sidelobes beyond it fall below the carrier threshold, as the
/// live capture confirms (`-78 dB` at ±2 against a `-74.8 dB` threshold).
///
/// Deliberately narrow, because it is also the width that is *not* excluded from a
/// real narrow signal: the 6–8 kHz data bursts at 447 MHz span about seven bins at
/// 2 Msps, so they still seed from their own skirts. A wider guard would swallow
/// them whole, which is why widening it is not the answer to a stubborn artefact.
///
/// `strongest_offset_hz` refuses the same artefact for the same reason.
/// (Module-private: it was `pub` before the split and nothing outside used it.)
pub(super) const DC_GUARD_BINS: usize = 2;

/// The strongest bin that is not part of the DC artefact, as `(index, value)`.
///
/// `radius_bins` optionally confines the search to that many bins either side of
/// centre - what "the signal at centre" needs, as opposed to "the loudest thing in
/// the capture". `None` searches the whole spectrum.
///
/// Returns `None` for an empty spectrum, or one where the guard and the radius
/// between them leave no candidate.
pub fn strongest_real_bin(bins: &[f32], radius_bins: Option<usize>) -> Option<(usize, f32)> {
    let n = bins.len();
    if n == 0 {
        return None;
    }
    let centre = n / 2;
    bins.iter()
        .enumerate()
        .filter(|(i, _)| {
            let d = i.abs_diff(centre);
            d > DC_GUARD_BINS && radius_bins.is_none_or(|r| d <= r)
        })
        .fold(None, |best: Option<(usize, f32)>, (i, &v)| match best {
            Some((_, bv)) if bv >= v => best,
            _ => Some((i, v)),
        })
}

/// How far from the tuned centre a measurement may look for its subject.
///
/// The panels this feeds read out *the signal at centre*, so the measurements have
/// to be about the thing the user tuned to. Unbounded they are about whatever
/// happens to be loudest in the capture: on an empty channel at 447 MHz the carrier
/// seed landed on a spur 863 kHz away and reported its 2 kHz width as the tuned
/// signal, and the SNR reported that spur's height as signal-to-noise.
///
/// It bounds the SNR peak as much as the carrier window, and that is the point:
/// with one radius there is a single notion of "at centre" on the page, so the
/// headline, the peak row and the occupancy cannot end up describing different
/// signals. It also keeps a *trend* meaningful - the rail's 60-second SNR trace and
/// the aiming view's trend arrow are only readable while their subject stays put,
/// and an unbounded peak lets the subject jump between stations mid-trace with
/// nothing on screen to say so.
///
/// About one broadcast channel wide, so a station is still found when the radio is
/// parked a channel off it, while a different station further out is left to be
/// tuned rather than silently measured in its place.
const CENTRE_RADIUS_HZ: f64 = 150_000.0;

/// [`CENTRE_RADIUS_HZ`] in bins for a given spectrum, clamped to the span for the
/// rates where it would otherwise reach past the capture.
pub fn centre_radius_bins(n: usize, sample_rate: f64) -> usize {
    if n == 0 || sample_rate <= 0.0 {
        return 0;
    }
    let bin_hz = sample_rate / n as f64;
    ((CENTRE_RADIUS_HZ / bin_hz).ceil() as usize).min(n)
}

/// The carrier at centre, as the inclusive bin range holding 99 % of its power.
/// The single carrier analysis in the app: both the occupied bandwidth and the
/// in-channel power come from this one window, so they cannot end up describing
/// different slices of the same spectrum.
///
/// Two steps:
///
/// 1. **Bound the carrier.** Seed at the strongest bin within
///    [`CENTRE_RADIUS_HZ`] of centre and outside the DC guard, then walk outward
///    while bins stay above `noise_floor + OBW_CARRIER_THRESHOLD_DB`, stepping over
///    dips shorter than [`OBW_GAP_TOLERANCE_HZ`] and trimming back to the last bin
///    that actually cleared the threshold. The walk itself is unbounded - only the
///    seed has to be near centre, so a wide carrier keeps its skirts.
/// 2. **Apply ITU-R SM.328 inside that window.** Exclude the bottom 0.5 % and the
///    top 0.5 % of the window's power; the span between the cut-offs is the answer.
///
/// `None` when no bin clears the threshold, or when all the occupancy turns out to
/// be the DC artefact. Callers render that as `---` rather than guessing off the
/// noise floor.
///
/// Note what the bandwidth this yields is *not*: Carson's rule. A WFM broadcast is
/// allocated 200 kHz and designed around 180 kHz, but the 99 % occupied bandwidth of
/// one carrying real programme material measures nearer 85 kHz, because the
/// time-averaged spectrum of FM is strongly peaked at the carrier. Measured, not
/// assumed - which is the point of the field.
pub(super) fn carrier_window(
    linear: &[f32],
    sample_rate: f64,
    noise_floor_db: f32,
) -> Option<(usize, usize)> {
    let n = linear.len();
    if n == 0 || sample_rate <= 0.0 {
        return None;
    }
    let bin_hz = sample_rate / n as f64;

    let centre = n / 2;
    let radius = centre_radius_bins(n, sample_rate);
    let (peak_idx, peak_lin) = strongest_real_bin(linear, Some(radius))?;
    let threshold = 10f32.powf((noise_floor_db + OBW_CARRIER_THRESHOLD_DB) / 10.0);
    if !peak_lin.is_finite() || peak_lin <= threshold {
        return None;
    }

    // Bins of slack, at least one, so a single ragged bin never ends the walk.
    let gap = (OBW_GAP_TOLERANCE_HZ / bin_hz).ceil().max(1.0) as usize;
    let mut lo = peak_idx;
    let mut miss = 0usize;
    for i in (0..peak_idx).rev() {
        if linear[i] > threshold {
            lo = i;
            miss = 0;
        } else {
            miss += 1;
            if miss > gap {
                break;
            }
        }
    }
    let mut hi = peak_idx;
    miss = 0;
    for (i, &v) in linear.iter().enumerate().skip(peak_idx + 1) {
        if v > threshold {
            hi = i;
            miss = 0;
        } else {
            miss += 1;
            if miss > gap {
                break;
            }
        }
    }

    let window = &linear[lo..=hi];
    let total: f32 = window.iter().sum();
    if !total.is_finite() || total <= 0.0 {
        return None;
    }
    let lo_thresh = total * 0.005;
    let hi_thresh = total * 0.995;
    let mut acc = 0f32;
    let mut lo_b = 0usize;
    let mut hi_b = window.len() - 1;
    for (i, &v) in window.iter().enumerate() {
        acc += v;
        if acc < lo_thresh {
            lo_b = i;
        }
        if acc < hi_thresh {
            hi_b = i;
        }
    }
    // Where the measured occupancy actually ended up, in absolute bins.
    let (occ_lo, occ_hi) = (lo + lo_b, lo + hi_b);

    // If all of it sits where the artefact lives, it is the artefact. A strong DC
    // line seeds a window from its own skirt - outside the guard, so the seed is
    // allowed - and the 99 % cut-offs then collapse back onto the centre bins that
    // dominate the window's power. Measured live at 447 MHz with nothing on air,
    // that reported "976 Hz" as a channel.
    //
    // Position, not width, is the test. A width threshold cannot separate the
    // artefact from the 6–8 kHz bursts on that same frequency, which are only a
    // couple of bins wider; but those bursts put power *outside* the guard, and the
    // artefact by definition does not.
    //
    // The cost is honest and small: a carrier too narrow to resolve at this bin size
    // reads as nothing, which is the truthful answer at 4.9 kHz bins.
    if occ_lo.abs_diff(centre) <= DC_GUARD_BINS && occ_hi.abs_diff(centre) <= DC_GUARD_BINS {
        return None;
    }
    Some((occ_lo, occ_hi))
}

#[cfg(test)]
mod tests {
    use super::super::analysis::carrier;
    use super::*;

    /// `level_db` is the carrier's **total** power, spread evenly across the bins it
    /// covers - not a per-bin level. That distinction is the whole point: a fixed
    /// per-bin level would make the carrier's power scale with how many bins the
    /// sample rate happens to divide it into, so the same station would carry five
    /// times the power at 2 Msps as at 10, and any test comparing the two would be
    /// measuring the helper rather than the code.
    fn spectrum(
        n: usize,
        sample_rate: f64,
        nf_db: f32,
        bw_hz: f64,
        level_db: f32,
        offset_hz: f64,
    ) -> Vec<f32> {
        let lin = |db: f32| 10f32.powf(db / 10.0);
        let mut bins = vec![lin(nf_db); n];
        let bin_hz = sample_rate / n as f64;
        let half = ((bw_hz / bin_hz) / 2.0).round() as i64;
        let c = n as i64 / 2 + (offset_hz / bin_hz).round() as i64;
        let count = (2 * half).max(1) as f32;
        for i in (c - half)..(c + half) {
            if (0..n as i64).contains(&i) {
                bins[i as usize] = lin(level_db) / count;
            }
        }
        bins
    }

    #[test]
    fn occupied_bw_measures_the_carrier_not_the_span() {
        // The bug this replaces: the 99 % method over the whole capture reported
        // 7.49 MHz for this signal at 10 Msps, because a noise floor that wide
        // carries enough power to push the cut-offs out to the span edges.
        let bins = spectrum(2048, 2_000_000.0, -90.0, 180_000.0, -40.0, 0.0);
        let bw = carrier(&bins, 2_000_000.0, -90.0).occupied_bw_hz;
        assert!(
            (170_000..=190_000).contains(&bw),
            "expected ~180 kHz, got {bw}"
        );
    }

    #[test]
    fn occupied_bw_is_a_property_of_the_signal_not_the_sample_rate() {
        // The same 180 kHz carrier seen through a 2 MHz and a 10 MHz window must
        // measure the same, to within the coarser window's bin size.
        let narrow = carrier(
            &spectrum(2048, 2_000_000.0, -90.0, 180_000.0, -40.0, 0.0),
            2_000_000.0,
            -90.0,
        )
        .occupied_bw_hz;
        let wide = carrier(
            &spectrum(2048, 10_000_000.0, -90.0, 180_000.0, -40.0, 0.0),
            10_000_000.0,
            -90.0,
        )
        .occupied_bw_hz;
        let bin_hz = 10_000_000.0 / 2048.0;
        assert!(
            (narrow as f64 - wide as f64).abs() < 3.0 * bin_hz,
            "narrow={narrow} wide={wide}, bin={bin_hz:.0} Hz"
        );
    }

    #[test]
    fn occupied_bw_finds_a_carrier_the_radio_is_parked_beside() {
        // Tuned a little off the station: still the signal at centre for any
        // practical purpose, and it must report its own bandwidth rather than its
        // distance from centre.
        let bins = spectrum(2048, 2_000_000.0, -90.0, 180_000.0, -40.0, 100_000.0);
        let bw = carrier(&bins, 2_000_000.0, -90.0).occupied_bw_hz;
        assert!(
            (170_000..=190_000).contains(&bw),
            "expected ~180 kHz, got {bw}"
        );
    }

    #[test]
    fn occupied_bw_ignores_a_station_that_is_not_the_tuned_one() {
        // A strong station most of a megahertz away is somebody else's channel. The
        // panel says "the signal at centre", so an empty centre reads as empty -
        // measured live at 447 MHz, where an unbounded seed latched onto a spur
        // 863 kHz out and reported its width as the tuned signal.
        let bins = spectrum(2048, 2_000_000.0, -90.0, 180_000.0, -40.0, 800_000.0);
        assert_eq!(carrier(&bins, 2_000_000.0, -90.0).occupied_bw_hz, 0);
    }

    #[test]
    fn occupied_bw_reads_a_narrow_carrier_as_narrow() {
        // The wide/narrow split is what `classify` turns into the MOD badge, so a
        // 15 kHz channel must not come back looking like broadcast FM.
        let bins = spectrum(2048, 2_000_000.0, -90.0, 15_000.0, -40.0, 0.0);
        let bw = carrier(&bins, 2_000_000.0, -90.0).occupied_bw_hz;
        assert!(
            (10_000..=20_000).contains(&bw),
            "expected ~15 kHz, got {bw}"
        );
        assert_eq!(
            crate::state::classify(50.0, bw),
            crate::state::Modulation::Nfm
        );
    }

    #[test]
    fn occupied_bw_steps_over_a_null_inside_the_channel() {
        // A broadcast signal's own spectrum is ragged. A momentary null a few bins
        // wide must not amputate the window at that bin.
        let mut bins = spectrum(2048, 2_000_000.0, -90.0, 180_000.0, -40.0, 0.0);
        let notch = 2048 / 2 - 40;
        for b in bins.iter_mut().skip(notch).take(5) {
            *b = 10f32.powf(-90.0 / 10.0);
        }
        let bw = carrier(&bins, 2_000_000.0, -90.0).occupied_bw_hz;
        assert!(
            (170_000..=190_000).contains(&bw),
            "null truncated the window: {bw}"
        );
    }

    #[test]
    fn occupied_bw_is_zero_without_a_carrier() {
        // Noise with a few dB of ripple is still noise: no window, no measurement,
        // and `---` on screen rather than a bandwidth invented from the floor.
        let mut bins = vec![10f32.powf(-90.0 / 10.0); 2048];
        for (i, b) in bins.iter_mut().enumerate() {
            *b = 10f32.powf((-90.0 + (i % 7) as f32 - 3.0) / 10.0);
        }
        assert_eq!(carrier(&bins, 2_000_000.0, -90.0).occupied_bw_hz, 0);
    }

    #[test]
    fn occupied_bw_refuses_the_dc_artefact() {
        // Measured live at 447.137 MHz with nothing on air: the LO leakage is a
        // three-bin line at the centre, 46 dB above the floor. Without the guard it
        // reports as a 2 kHz carrier and `classify` names it AM - a station
        // invented out of the front end's own artefact.
        let lin = |db: f32| 10f32.powf(db / 10.0);
        let mut bins = vec![lin(-85.0); 2048];
        for b in bins[1023..=1025].iter_mut() {
            *b = lin(-39.0);
        }
        assert_eq!(carrier(&bins, 2_000_000.0, -85.0).occupied_bw_hz, 0);
    }

    #[test]
    fn occupied_bw_still_sees_a_carrier_sitting_on_dc() {
        // The guard excludes the seed, not the window: a real carrier wider than
        // the artefact seeds from its own skirts, and the walk grows back across
        // the centre. A 7 kHz burst centred on DC - the shape of the 447 MHz data
        // bursts - must survive intact.
        let bins = spectrum(2048, 2_000_000.0, -85.0, 7_000.0, -39.0, 0.0);
        let bw = carrier(&bins, 2_000_000.0, -85.0).occupied_bw_hz;
        assert!((5_000..=9_000).contains(&bw), "expected ~7 kHz, got {bw}");
    }

    #[test]
    fn occupied_bw_prefers_a_real_carrier_over_the_dc_line() {
        // Both present: the artefact at centre, a station off to one side whose own
        // skirts stop well short of it. The window must be drawn around the station,
        // even though the artefact is 20 dB louder.
        let lin = |db: f32| 10f32.powf(db / 10.0);
        let mut bins = spectrum(2048, 2_000_000.0, -90.0, 60_000.0, -40.0, 100_000.0);
        for b in bins[1023..=1025].iter_mut() {
            *b = lin(-20.0);
        } // artefact louder
        let bw = carrier(&bins, 2_000_000.0, -90.0).occupied_bw_hz;
        assert!(
            (50_000..=70_000).contains(&bw),
            "expected ~60 kHz, got {bw}"
        );
    }

    #[test]
    fn strongest_real_bin_skips_the_dc_line() {
        // The artefact towers over a real, weaker carrier off to one side. Report
        // the carrier - the artefact is not signal, however loud it is.
        let mut bins = vec![-90.0f32; 2048];
        for b in bins[1023..=1025].iter_mut() {
            *b = -20.0;
        }
        bins[1300] = -50.0;
        assert_eq!(strongest_real_bin(&bins, None), Some((1300, -50.0)));
    }

    #[test]
    fn strongest_real_bin_honours_the_search_radius() {
        let mut bins = vec![-90.0f32; 2048];
        bins[1100] = -40.0; // 76 bins out
        bins[1800] = -20.0; // 776 bins out, louder
        assert_eq!(strongest_real_bin(&bins, Some(100)), Some((1100, -40.0)));
        assert_eq!(strongest_real_bin(&bins, None), Some((1800, -20.0)));
    }

    #[test]
    fn centre_radius_covers_a_broadcast_channel_and_clamps_to_the_span() {
        // 977 Hz bins: 150 kHz is about 154 of them.
        assert!((centre_radius_bins(2048, 2_000_000.0) as i64 - 154).abs() <= 1);
        // 4.9 kHz bins at 10 Msps: the same 150 kHz is far fewer.
        assert!((centre_radius_bins(2048, 10_000_000.0) as i64 - 31).abs() <= 1);
        // Never wider than the spectrum it is applied to.
        assert!(centre_radius_bins(64, 2_000.0) <= 64);
        assert_eq!(centre_radius_bins(0, 2_000_000.0), 0);
        assert_eq!(centre_radius_bins(2048, 0.0), 0);
    }

    #[test]
    fn strongest_real_bin_declines_when_nothing_is_left() {
        assert_eq!(strongest_real_bin(&[], None), None);
        // A span so narrow the guard covers all of it.
        assert_eq!(strongest_real_bin(&[-10.0, -20.0, -30.0], None), None);
        // A radius inside the guard leaves no candidate either.
        assert_eq!(strongest_real_bin(&vec![-10.0f32; 2048], Some(1)), None);
    }

    #[test]
    fn occupied_bw_rejects_occupancy_that_is_all_dc_artefact() {
        // Measured live at 447 MHz with nothing on air: the artefact's skirt seeds a
        // window outside the guard, then the 99 % cut-offs collapse back onto the
        // centre bins that dominate it, and the panel reported "976 Hz" as a channel.
        // The levels here are the ones the live dump recorded.
        let lin = |db: f32| 10f32.powf(db / 10.0);
        let mut bins = vec![lin(-85.0); 2048];
        bins[1023] = lin(-44.9);
        bins[1024] = lin(-39.0);
        bins[1025] = lin(-45.0);
        bins[1021] = lin(-70.0); // skirt, outside the guard, above the threshold
        bins[1027] = lin(-70.0);
        assert_eq!(carrier(&bins, 2_000_000.0, -85.0).occupied_bw_hz, 0);
    }

    #[test]
    fn occupied_bw_keeps_a_narrow_burst_the_artefact_could_be_mistaken_for() {
        // The other half of the same rule. These bursts are only a couple of bins
        // wider than the artefact, so no width threshold separates them - but they
        // put real power outside the guard, and the artefact never does.
        let bins = spectrum(2048, 2_000_000.0, -85.0, 7_000.0, -39.0, 0.0);
        let bw = carrier(&bins, 2_000_000.0, -85.0).occupied_bw_hz;
        assert!(
            bw >= 4_000,
            "a real 7 kHz burst must survive the artefact rule, got {bw}"
        );
    }

    #[test]
    fn occupied_bw_declines_degenerate_input() {
        assert_eq!(carrier(&[], 2_000_000.0, -90.0).occupied_bw_hz, 0);
        assert_eq!(carrier(&[1.0, 1.0, 1.0], 0.0, -90.0).occupied_bw_hz, 0);
    }
}
