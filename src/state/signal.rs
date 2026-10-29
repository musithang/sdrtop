// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use std::collections::VecDeque;

/// The ADC-saturation scale, in percent of samples pinned to the converter's
/// rails: calm below [`SAT_WARN_PCT`], amber to [`SAT_CRIT_PCT`], red above.
///
/// **One scale, defined once.** Every SAT readout in the app colours from this
/// pair through `micro_common::sat_color`, and the clip alert-memory below is
/// anchored to it. The Command Rail used to carry a second, laxer pair (10 % /
/// 50 %) on the argument that a rail sitting beside a gain control should not cry
/// wolf at the saturation a well-driven front end normally runs at - which was
/// true of the *advice*, but not of the *number*: the same measurement then read
/// green in the rail and red in the micro views at the same instant, and nothing
/// told the user which to believe. The advice now comes from clip headroom
/// instead (see `command_rail::modes::chain_verdict`), which is the thing a gain
/// control actually wants, and the number means one thing everywhere.
///
/// These live in `state` rather than beside the colour helper so that
/// [`SAT_CLIP_PCT`] and the RX poll can share them without the hot path
/// depending on the UI.
pub const SAT_WARN_PCT: f32 = 1.0;
/// Upper end of the [`SAT_WARN_PCT`] scale: at or above this, samples are pinned
/// often enough that the reading is clipping rather than peaks grazing the rails.
pub const SAT_CRIT_PCT: f32 = 5.0;

/// ADC saturation at or above this percent records a clip event for the Command
/// Rail's alert-memory - real clipping that's worth remembering, not measurement
/// noise.
///
/// The crit point of the scale above rather than a number of its own, so the
/// remembered event and the red SAT reading are the same threshold. (It was 10 %
/// while the rail had its own scale to be "aligned with the warn colour" of; that
/// scale is gone, and anchoring to *warn* instead would remember every amber
/// graze, which is not what an alert-memory is for.)
pub const SAT_CLIP_PCT: f32 = SAT_CRIT_PCT;

/// The ADC peak window a front end is well driven in, in dBFS.
///
/// Shared on purpose by the two things that judge gain staging: the auto-gain
/// latch re-centres the level when the peak leaves this window and does nothing
/// at all inside it, and the Command Rail's CHAIN verdict calls anything above
/// the top of it "hot". Two constants would let the rail advise backing off a
/// level the auto-gain is content to hold.
pub const ADC_COMFORT_DBFS: std::ops::RangeInclusive<f32> = -12.0..=-4.0;

/// Adjacent-channel offset from centre for the ACPR measurement, by modulation.
///
/// One fixed spacing cannot serve every band: ±200 kHz is broadcast FM's channel
/// step, and applying it to a 12.5 kHz narrow-FM channel measures a band eight
/// channels away - a number that is arithmetically correct and answers nobody's
/// question. So the offset follows what the carrier turns out to be.
///
/// These are the channel steps of the services the classifier can name, not
/// regulatory ACPR masks (sdrtop asserts no mask; see [`ACPR_FLOOR_DB`]). An unclassified carrier keeps the broadcast spacing,
/// since that is the state the page rests in.
///
/// The value actually used is recorded in [`SignalState::acpr_offset_hz`] rather
/// than re-derived by the panel, so the label can never name a different offset
/// than the one measured.
pub fn acpr_offset_hz(modulation: Modulation) -> f64 {
    match modulation {
        Modulation::Nfm => 25_000.0,
        Modulation::Am => 9_000.0,
        _ => 200_000.0,
    }
}

/// A rough modulation estimate for the signal at centre. Honest by design: a
/// bandwidth heuristic (see [`classify`]), not a demodulating classifier. The
/// demod phase refines it (e.g. WFM confirmed by a 19 kHz pilot lock).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Modulation {
    /// No clear carrier to characterize (weak signal), or a shape that does not
    /// fit the known bands.
    #[default]
    Unknown,
    /// Wide-band FM broadcast. Allocated a 200 kHz channel, but see
    /// [`WFM_MIN_BW_HZ`] for what it actually *measures*.
    Wfm,
    /// Narrow-band FM voice / data (~11–30 kHz).
    Nfm,
    /// Amplitude modulation / narrow voice (< 11 kHz).
    Am,
}

impl Modulation {
    /// Short badge label for the banner / headline: `WFM` / `NFM` / `AM` / `—`.
    pub fn label(self) -> &'static str {
        match self {
            Modulation::Wfm => "WFM",
            Modulation::Nfm => "NFM",
            Modulation::Am => "AM",
            Modulation::Unknown => "\u{2014}",
        }
    }

    /// Whether a modulation was confidently classified (not the no-signal fallback).
    pub fn is_known(self) -> bool {
        !matches!(self, Modulation::Unknown)
    }
}

/// Minimum peak-to-noise (dB) for [`classify`] to commit to a modulation. Below
/// this there is no clear carrier at centre, so it reports [`Modulation::Unknown`]
/// rather than labelling noise.
pub const CLASSIFY_MIN_SNR_DB: f32 = 10.0;

/// Occupied bandwidth at or above which the carrier reads as broadcast FM.
///
/// Not the 180 kHz of Carson's rule, and not the 200 kHz channel allocation. The
/// 99 % occupied bandwidth of a real WFM broadcast measures far less than either,
/// because the time-averaged spectrum of FM is strongly peaked at the carrier -
/// 92.8 MHz on a stub antenna measures ~85 kHz, with 91 % of its power inside
/// ±25 kHz. A weaker signal reads narrower still, since its skirts sink under the
/// noise floor before the threshold that bounds the measurement window can see them.
///
/// So the boundary sits well below the observed figure, and still far above narrow
/// FM, which tops out near 16 kHz. There is a factor of five of empty space between
/// the two populations; this is the middle of it.
pub const WFM_MIN_BW_HZ: u64 = 50_000;

/// Occupied bandwidth above which the energy at centre is not one signal at all.
///
/// Carson's rule for broadcast FM with its widest subcarrier, the 92 kHz SCA:
/// 2 x (75 + 92) = 334 kHz. No single broadcast reaches past it, and its 99 %
/// occupied bandwidth sits far inside it (~85 kHz measured). Wider than this,
/// the carrier window has walked through station after station: on the 40 m
/// band a 768 kHz slice of SSB, CW and broadcast read as one 657 kHz "clean
/// WFM signal". That is many signals, and no modulation is named for them.
pub const WFM_MAX_BW_HZ: u64 = 334_000;

/// Occupied bandwidth at or above which the carrier reads as narrow-band FM rather
/// than AM. The honest weak point of the heuristic: narrow FM data bursts and AM
/// voice genuinely overlap here (a 447 MHz data burst measures 6–8 kHz, AM
/// broadcast 9 kHz), and bandwidth alone cannot separate them. `[T]` on the demod
/// bench is the override, and a demodulator that confirms the mode is the real fix.
pub const NFM_MIN_BW_HZ: u64 = 11_000;

/// Estimate the modulation of the signal at centre from its 99% occupied
/// bandwidth, gated on signal presence. Deliberately conservative: a bandwidth
/// heuristic, so the wide/narrow split is trustworthy while the AM vs NFM boundary
/// is a best guess the demod phase can sharpen.
pub fn classify(snr_db: f32, occupied_bw_hz: u64) -> Modulation {
    if snr_db < CLASSIFY_MIN_SNR_DB || occupied_bw_hz == 0 {
        return Modulation::Unknown;
    }
    match occupied_bw_hz {
        bw if bw > WFM_MAX_BW_HZ => Modulation::Unknown,
        bw if bw >= WFM_MIN_BW_HZ => Modulation::Wfm,
        bw if bw >= NFM_MIN_BW_HZ => Modulation::Nfm,
        _ => Modulation::Am,
    }
}

/// Peak-to-noise (dB) at or above which a carrier is judged clean rather than
/// weak: the same "clean" threshold `snr_color` grades with everywhere.
pub const VERDICT_CLEAN_SNR_DB: f32 = 20.0;

/// An adjacent-channel ratio worse (less negative) than this is judged
/// splatter worth a note. sdrtop's own instrument reading, not an asserted
/// regulatory mask.
pub const VERDICT_ACPR_CONCERN_DB: f32 = -20.0;

/// The bottom of the adjacent-channel scale: an ACPR this far down or further
/// reads as a clean, empty bar, and the bar's needle comes to rest here. The
/// gauge's own display range, not a regulatory mask.
pub const ACPR_FLOOR_DB: f32 = -80.0;

/// What the Lab's verdict says of the carrier at centre: its kind. The words
/// are the Signal Characterization panel's; the kind lives here so it can be
/// held from frame to frame where the frames arrive, and every reader of it
/// shows the same one. The class is not part of it: it is held on its own,
/// longer (`SignalState::modulation_settled`), so a class that scatters from
/// burst to burst does not tear a settled verdict down.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict {
    /// Above the presence gate, under [`VERDICT_CLEAN_SNR_DB`].
    Weak,
    /// Strong, with an adjacent-channel ratio past [`VERDICT_ACPR_CONCERN_DB`].
    Splatter,
    /// Strong and well separated.
    Clean,
}

/// Judge one frame's carrier, or `None` when there is none to judge: no class,
/// or under the classifier's own presence gate. The class only gates; the kind
/// is the same whatever it is. The adjacent-channel ratio counts only when
/// both sides were measured.
pub fn judge(
    modulation: Modulation,
    snr_db: f32,
    acpr_lower_db: f32,
    acpr_upper_db: f32,
) -> Option<Verdict> {
    if !modulation.is_known() || snr_db < CLASSIFY_MIN_SNR_DB {
        return None;
    }
    if snr_db < VERDICT_CLEAN_SNR_DB {
        return Some(Verdict::Weak);
    }
    let worst = (acpr_lower_db.is_finite() && acpr_upper_db.is_finite())
        .then(|| acpr_lower_db.max(acpr_upper_db));
    if worst.is_some_and(|w| w > VERDICT_ACPR_CONCERN_DB) {
        return Some(Verdict::Splatter);
    }
    Some(Verdict::Clean)
}

/// The carrier figures as the Lab shows them. Taken only from frames with a
/// carrier in them, because in a gap between bursts the same measurements read
/// noise: the adjacent-channel ratio of noise is about 0 dB, its occupied
/// bandwidth the whole span. Held through a gap of up to a second
/// (`signal::ballistics::SETTLE`), then refused. Display only: every
/// measurement keeps the raw per-frame figures on [`SignalState`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Carrier {
    /// Peak over the noise floor, dB, as at the last quarter-second tick.
    pub snr_db: f32,
    /// The 99 % occupied bandwidth, as at the same tick.
    pub occupied_bw_hz: u64,
    /// The louder adjacent band's level, dBFS, as at the same tick.
    pub adj_carrier_dbfs: f32,
    /// The adjacent-channel ratios on the instrument ballistic, so splatter
    /// shows at once and its bar falls slowly; `None` for a side not measured.
    pub acpr_lower: Option<crate::signal::ballistics::Shown>,
    pub acpr_upper: Option<crate::signal::ballistics::Shown>,
    /// The verdict, changed only once a new kind has held for a second;
    /// `None` until one has.
    pub verdict: Option<Verdict>,
    /// The class the verdict's words name: the held one, as on the badge.
    pub modulation: Modulation,
    /// When a carrier was last heard: in a gap, how long it has been held.
    pub heard: std::time::Instant,
}

/// What the radio itself says about samples it dropped before any reached
/// the host. See `hardware::SdrDevice::radio_drops`.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum RadioDropAccount {
    /// This radio keeps no such count. Nothing to show.
    #[default]
    NotKept,
    /// It keeps one, and it could not be read, for this reason.
    Unreadable(String),
    /// The count, since the stream started.
    Counted {
        /// Drops since the stream started.
        stream: u32,
        /// Drops in the last poll window.
        window: u32,
        /// The longest, in bytes of the radio's sample format.
        longest_bytes: u32,
    },
}

#[derive(Clone)]
pub struct SignalState {
    pub drops_per_sec: u64,
    pub total_drops_session: u64,
    /// The part of `total_drops_session` the driver reported itself, before
    /// the block reached sdrtop (libairspy's queue, a HackRF's short transfer,
    /// a SoapySDR overflow); the rest is sdrtop's intake refusing a block.
    pub driver_drops_session: u64,
    /// The radio's own count of what it dropped: drops that leave no hole in
    /// the stream positions, so `drops_per_sec` can never see them.
    pub radio_drops: RadioDropAccount,
    pub drop_history: VecDeque<u64>,
    pub adc_saturation_pct: f32,
    pub adc_saturation_peak: f32,
    pub saturation_history: VecDeque<f32>,
    pub peak_to_nf_db: f32,
    /// Power integrated over the occupied channel at centre, dBFS.
    ///
    /// The *channel*, not the capture: summing every bin made this a function of the
    /// sample rate, so widening the span raised the "channel power" of a station
    /// that had not changed. `f32::NEG_INFINITY` when there is no carrier to
    /// integrate - every reader already renders that as a dash.
    ///
    /// Distinct from [`Self::adc_rms_dbfs`], which is the full-bandwidth figure and
    /// belongs to ADC loading rather than to the signal.
    pub channel_power_dbfs: f32,
    /// The Command Rail S-meter: [`Self::channel_power_dbfs`] through an
    /// instrument's ballistics (`signal::ballistics`), its bar, true peak and
    /// digits. Display only: never read as a measurement by another panel or
    /// calculation. `None` before a reading.
    pub s_meter: Option<crate::signal::ballistics::Shown>,
    pub occupied_bw_hz: u64,
    /// Adjacent-channel power ratio, dB relative to the in-channel power, at
    /// ±[`Self::acpr_offset_hz`]. `f32::NEG_INFINITY` when there is nothing to
    /// compare against yet (no occupied bandwidth), a band falls outside the
    /// captured span, or the bands would overlap the channel - never a guessed
    /// number. The two are written together: both finite, or neither.
    pub acpr_lower_db: f32,
    pub acpr_upper_db: f32,
    /// Absolute level (dBFS) of the louder - worse - of the two adjacent bands.
    /// Paired with `acpr_lower_db` / `acpr_upper_db`; same undefined sentinel,
    /// which also covers a genuinely silent adjacent band.
    pub adj_carrier_dbfs: f32,
    /// The offset the ratio above was actually measured at, from
    /// [`acpr_offset_hz`]. Recorded rather than re-derived so the panel's labels
    /// and the adjacent-band frequency it prints can never disagree with the
    /// measurement - the modulation could change between the two reads otherwise.
    pub acpr_offset_hz: f64,
    pub usb_errors_session: u64,
    pub usb_errors_last_poll: u64,
    pub usb_error_history: std::collections::VecDeque<u64>,
    /// Recent SNR (peak/noise-floor) samples, pushed by the rx poll task roughly
    /// every 500 ms while streaming. Powers the micro_signal trend arrow.
    pub snr_history: VecDeque<f32>,
    /// The FFT frame the trends last took their figures from, by its time:
    /// a frame is pushed once, so a paused FFT leaves a gap, not its last
    /// level repeated.
    pub trended_fft: Option<std::time::Instant>,
    /// Recent channel-power (dBFS) samples - pushed alongside `snr_history` at the
    /// same ~500 ms cadence. Powers the command rail's PWR sparkline + trend.
    pub pwr_history: VecDeque<f32>,
    /// Recent noise-floor (dBFS) samples - pushed alongside `snr_history`. Powers
    /// the command rail's NF sparkline + trend.
    pub nf_history: VecDeque<f32>,
    /// Recent ADC-saturation (%) samples - pushed alongside `snr_history` at the
    /// same ~500 ms / [`crate::state::SNR_HISTORY_LEN`] depth so the command rail's
    /// SAT trace fills like the other three. Distinct from [`Self::saturation_history`],
    /// which feeds the health panels' mini-graph at the 200 ms / 64-deep cadence.
    pub sat_history: VecDeque<f32>,
    /// Unix-epoch second of the most recent ADC clip (saturation ≥ [`SAT_CLIP_PCT`]),
    /// for the rail's fading "last clip Xs" alert-memory. `None` = none this session.
    pub last_clip_at: Option<u64>,
    /// Rough modulation estimate for the signal at centre, refreshed each display
    /// frame by the FFT worker via [`classify`]. Drives the lab_signal headline /
    /// banner and, later, the demod panel's mode-adaptive view.
    pub modulation: Modulation,
    /// The same, changed only once a new class has held for two seconds
    /// (`signal::ballistics::CLASS_HOLD`): on a bursty signal the classifier
    /// flips between a class and `Unknown` with every burst and gap, and
    /// between classes as the bandwidth scatters; a gap neither changes this
    /// nor restarts its hold, and two seconds with no class make it `Unknown`. What picks the demodulator and the demod panel's
    /// sections, so neither is torn down and rebuilt by a gap.
    pub modulation_settled: Modulation,
    /// The spectrum frame as it stood at the last quarter-second tick: what
    /// the Lab prints its channel figures from (peak over noise, peak, noise
    /// floor, peak hold), so a number changes at most four times a second and
    /// can be read. Display only.
    pub readout: Option<crate::state::FftFrame>,
    /// The carrier figures (see [`Carrier`]); `None` when no carrier has been
    /// heard for a second.
    pub carrier: Option<Carrier>,
    /// The adjacent-channel bars' needles, lower and upper: the carrier's
    /// ratios on the averaging meter while it is heard and held, then falling
    /// back to [`ACPR_FLOOR_DB`] rather than vanishing; `None` at rest. A
    /// falling needle is not a reading: its digits are refused (the carrier
    /// is gone) and it is drawn as stale.
    pub acpr_needle: [Option<f32>; 2],
    /// The adjacent-channel offset the needles were measured at: the last
    /// carrier frame's, kept through gaps (where the classifier has no class
    /// and [`Self::acpr_offset_hz`] falls back to the broadcast spacing) so
    /// the rows name what was measured. `None` until a carrier has been heard.
    pub acpr_measured_offset_hz: Option<f64>,
    /// ADC loading for the Lab RF bench, refreshed each ~200 ms window: the loudest
    /// sample (`adc_peak_dbfs`), the full-bandwidth RMS level (`adc_rms_dbfs`, total
    /// I/Q power vs full scale - distinct from the in-channel `channel_power_dbfs`),
    /// and the clipped-sample count in the last window (`adc_clip_events`).
    pub adc_peak_dbfs: f32,
    pub adc_rms_dbfs: f32,
    pub adc_clip_events: u64,
}

impl Default for SignalState {
    /// The no-measurement state the app starts in, and the one the tests build on.
    ///
    /// Not `#[derive(Default)]`: the "nothing measured yet" value for a level or a
    /// ratio is the undefined sentinel, not zero. Zero dBFS is full scale, and a
    /// panel that renders it reads as a signal pinning the ADC.
    fn default() -> Self {
        let hist = || VecDeque::with_capacity(crate::state::SNR_HISTORY_LEN);
        Self {
            drops_per_sec: 0,
            total_drops_session: 0,
            driver_drops_session: 0,
            radio_drops: RadioDropAccount::NotKept,
            drop_history: VecDeque::new(),
            adc_saturation_pct: 0.0,
            adc_saturation_peak: 0.0,
            saturation_history: VecDeque::new(),
            peak_to_nf_db: 0.0,
            channel_power_dbfs: f32::NEG_INFINITY,
            s_meter: None,
            occupied_bw_hz: 0,
            acpr_lower_db: f32::NEG_INFINITY,
            acpr_upper_db: f32::NEG_INFINITY,
            adj_carrier_dbfs: f32::NEG_INFINITY,
            acpr_offset_hz: acpr_offset_hz(Modulation::Unknown),
            usb_errors_session: 0,
            usb_errors_last_poll: 0,
            usb_error_history: VecDeque::new(),
            snr_history: hist(),
            trended_fft: None,
            pwr_history: hist(),
            nf_history: hist(),
            sat_history: hist(),
            last_clip_at: None,
            modulation: Modulation::Unknown,
            modulation_settled: Modulation::Unknown,
            readout: None,
            carrier: None,
            acpr_needle: [None, None],
            acpr_measured_offset_hz: None,
            adc_peak_dbfs: 0.0,
            adc_rms_dbfs: 0.0,
            adc_clip_events: 0,
        }
    }
}

impl SignalState {
    /// Short-term SNR trend in dB: mean of the most recent half of
    /// `snr_history` minus the mean of the older half. Positive means the
    /// signal is strengthening. `None` until there are enough samples.
    pub fn snr_delta(&self) -> Option<f32> {
        let n = self.snr_history.len();
        if n < 4 {
            return None;
        }
        let half = n / 2;
        let older: f32 = self.snr_history.iter().take(half).sum::<f32>() / half as f32;
        let recent: f32 = self.snr_history.iter().skip(n - half).sum::<f32>() / half as f32;
        Some(recent - older)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_judged_without_a_carrier() {
        assert_eq!(judge(Modulation::Unknown, 40.0, -40.0, -40.0), None);
        assert_eq!(judge(Modulation::Wfm, 5.0, -40.0, -40.0), None);
    }

    #[test]
    fn a_carrier_under_the_clean_threshold_is_weak() {
        let w = Modulation::Wfm;
        assert_eq!(judge(w, 15.0, -40.0, -40.0), Some(Verdict::Weak));
    }

    #[test]
    fn a_strong_carrier_with_a_poor_adjacent_ratio_splatters() {
        let w = Modulation::Wfm;
        assert_eq!(judge(w, 40.0, -10.0, -40.0), Some(Verdict::Splatter));
        assert_eq!(judge(w, 43.7, -38.0, -41.0), Some(Verdict::Clean));
    }

    #[test]
    fn half_an_adjacent_pair_is_no_ratio() {
        let n = Modulation::Nfm;
        for (lo, hi) in [(f32::NEG_INFINITY, -10.0), (-10.0, f32::NEG_INFINITY)] {
            assert_eq!(judge(n, 30.0, lo, hi), Some(Verdict::Clean));
        }
    }

    fn with_history(samples: &[f32]) -> SignalState {
        let mut s = SignalState {
            drops_per_sec: 0,
            total_drops_session: 0,
            driver_drops_session: 0,
            radio_drops: RadioDropAccount::NotKept,
            drop_history: VecDeque::new(),
            adc_saturation_pct: 0.0,
            adc_saturation_peak: 0.0,
            saturation_history: VecDeque::new(),
            peak_to_nf_db: 0.0,
            channel_power_dbfs: 0.0,
            s_meter: None,
            occupied_bw_hz: 0,
            acpr_lower_db: f32::NEG_INFINITY,
            acpr_upper_db: f32::NEG_INFINITY,
            adj_carrier_dbfs: f32::NEG_INFINITY,
            acpr_offset_hz: acpr_offset_hz(Modulation::Unknown),
            usb_errors_session: 0,
            usb_errors_last_poll: 0,
            usb_error_history: VecDeque::new(),
            snr_history: VecDeque::new(),
            trended_fft: None,
            pwr_history: VecDeque::new(),
            nf_history: VecDeque::new(),
            sat_history: VecDeque::new(),
            last_clip_at: None,
            modulation: Modulation::Unknown,
            modulation_settled: Modulation::Unknown,
            readout: None,
            carrier: None,
            acpr_needle: [None, None],
            acpr_measured_offset_hz: None,
            adc_peak_dbfs: 0.0,
            adc_rms_dbfs: 0.0,
            adc_clip_events: 0,
        };
        s.snr_history.extend(samples.iter().copied());
        s
    }

    #[test]
    fn classify_gates_on_signal_presence() {
        // Weak carrier or no occupancy → no guess.
        assert_eq!(classify(5.0, 180_000), Modulation::Unknown);
        assert_eq!(classify(40.0, 0), Modulation::Unknown);
    }

    #[test]
    fn classify_bands_by_occupied_bandwidth() {
        assert_eq!(classify(40.0, 180_000), Modulation::Wfm);
        assert_eq!(classify(40.0, WFM_MIN_BW_HZ), Modulation::Wfm); // wide boundary
        assert_eq!(classify(40.0, 15_000), Modulation::Nfm);
        assert_eq!(classify(40.0, NFM_MIN_BW_HZ), Modulation::Nfm); // narrow-FM boundary
        assert_eq!(classify(40.0, 8_000), Modulation::Am);
    }

    #[test]
    fn acpr_offset_follows_the_channel_step_of_the_service() {
        // Broadcast FM's 200 kHz step applied to a 12.5 kHz narrow-FM channel would
        // measure a band eight channels away.
        assert_eq!(acpr_offset_hz(Modulation::Wfm), 200_000.0);
        assert_eq!(acpr_offset_hz(Modulation::Nfm), 25_000.0);
        assert_eq!(acpr_offset_hz(Modulation::Am), 9_000.0);
        // An unclassified carrier rests in the broadcast shape, like the panel does.
        assert_eq!(
            acpr_offset_hz(Modulation::Unknown),
            acpr_offset_hz(Modulation::Wfm)
        );
    }

    #[test]
    fn acpr_offset_clears_the_bandwidth_of_its_own_service() {
        // The measurement is only meaningful while the adjacent bands sit clear of
        // the channel, and each band is as wide as the occupied bandwidth. So every
        // offset has to exceed what its own modulation typically occupies, or the
        // overlap guard in `acpr_bands` would suppress the reading for good.
        for (m, typical_bw) in [
            (Modulation::Wfm, 120_000.0),
            (Modulation::Nfm, 15_000.0),
            (Modulation::Am, 8_000.0),
        ] {
            assert!(
                acpr_offset_hz(m) > typical_bw,
                "{m:?}: offset {} does not clear {typical_bw}",
                acpr_offset_hz(m)
            );
        }
    }

    /// **Wider than any one broadcast is not one signal**: a band of stations
    /// walked into one window (657 kHz on 40 m) names no modulation; the widest
    /// FM broadcast Carson's rule allows still reads as WFM.
    #[test]
    fn classify_names_no_modulation_for_a_band_of_stations() {
        assert_eq!(classify(53.0, 657_000), Modulation::Unknown);
        assert_eq!(classify(53.0, WFM_MAX_BW_HZ + 1), Modulation::Unknown);
        assert_eq!(classify(53.0, WFM_MAX_BW_HZ), Modulation::Wfm);
    }

    #[test]
    fn classify_reads_a_real_broadcast_as_wfm() {
        // Measured on air at 92.8 MHz: the 99 % occupied bandwidth of a WFM
        // broadcast is ~85 kHz, not the 180 kHz of Carson's rule. The boundary that
        // was calibrated against the old whole-span measure put this in NFM, and
        // the demod then opened a 25 kHz channel on a broadcast station.
        assert_eq!(classify(41.0, 85_000), Modulation::Wfm);
        // Quiet programme material, or a weaker signal whose skirts sink under the
        // floor, still has to land on the same side of the line.
        assert_eq!(classify(41.0, 60_000), Modulation::Wfm);
    }

    #[test]
    fn classify_keeps_narrow_fm_clear_of_the_wide_boundary() {
        // The other side of the same gap: narrow FM tops out around 16 kHz, so
        // there is no width at which the two populations can be confused.
        assert_eq!(classify(40.0, 16_000), Modulation::Nfm);
        assert_eq!(classify(40.0, 30_000), Modulation::Nfm);
    }

    #[test]
    fn modulation_labels_and_known_flag() {
        assert_eq!(Modulation::Wfm.label(), "WFM");
        assert_eq!(Modulation::Unknown.label(), "\u{2014}");
        assert!(Modulation::Nfm.is_known());
        assert!(!Modulation::Unknown.is_known());
    }

    #[test]
    fn snr_delta_none_with_too_few_samples() {
        assert_eq!(with_history(&[10.0, 12.0, 14.0]).snr_delta(), None);
    }

    #[test]
    fn snr_delta_positive_when_rising() {
        // older half avg = 10, recent half avg = 20 → +10
        let d = with_history(&[10.0, 10.0, 20.0, 20.0]).snr_delta().unwrap();
        assert!((d - 10.0).abs() < 1e-6, "got {d}");
    }

    #[test]
    fn snr_delta_negative_when_falling() {
        let d = with_history(&[20.0, 20.0, 12.0, 12.0]).snr_delta().unwrap();
        assert!((d + 8.0).abs() < 1e-6, "got {d}");
    }

    /// The saturation scale is a scale: the thresholds are ordered, and the clip
    /// alert-memory sits on one of them rather than beside it.
    ///
    /// Two of these numbers used to disagree by a factor of ten between the rail
    /// and the micro views (D1). The relationships are asserted rather than
    /// commented so a future edit to one of them has to face the others.
    #[test]
    fn the_saturation_scale_is_ordered_and_the_clip_memory_sits_on_it() {
        // `const` blocks, so breaking the scale fails the build rather than a
        // test run: these are facts about four literals, and there is no reason
        // to wait for `cargo test` to hear about it.
        const { assert!(SAT_WARN_PCT > 0.0, "0 % saturation must read calm") };
        const {
            assert!(
                SAT_WARN_PCT < SAT_CRIT_PCT,
                "warn must come before crit, or the scale has no amber band"
            )
        };
        const { assert!(SAT_CRIT_PCT < 100.0) };
        // Deliberately breaking one of these fails `cargo build`, not `cargo test`.
        const {
            assert!(
                SAT_CLIP_PCT == SAT_CRIT_PCT,
                "a remembered clip and a red SAT reading must be the same event"
            )
        };
    }

    /// The comfort window has to be a window, and it has to sit below full scale
    /// with room to spare - the rail derives its "hot" edge from the top of it.
    #[test]
    fn the_adc_comfort_window_is_below_full_scale() {
        assert!(ADC_COMFORT_DBFS.start() < ADC_COMFORT_DBFS.end());
        assert!(
            *ADC_COMFORT_DBFS.end() < 0.0,
            "a comfortable peak is never at full scale"
        );
    }
}
