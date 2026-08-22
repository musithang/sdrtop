use std::collections::VecDeque;

/// ADC saturation at or above this percent records a clip event for the Command
/// Rail's alert-memory. Aligned with the SAT "warn" colour — real clipping that's
/// worth remembering, not measurement noise.
pub const SAT_CLIP_PCT: f32 = 10.0;

/// Adjacent-channel offset from centre for the ACPR measurement, by modulation.
///
/// One fixed spacing cannot serve every band: ±200 kHz is broadcast FM's channel
/// step, and applying it to a 12.5 kHz narrow-FM channel measures a band eight
/// channels away — a number that is arithmetically correct and answers nobody's
/// question. So the offset follows what the carrier turns out to be.
///
/// These are the channel steps of the services the classifier can name, not
/// regulatory ACPR masks (sdrtop asserts no mask; see the panel's own
/// `ACPR_BAR_FLOOR_DB`). An unclassified carrier keeps the broadcast spacing,
/// since that is the state the page rests in.
///
/// The value actually used is recorded in [`SignalState::acpr_offset_hz`] rather
/// than re-derived by the panel, so the label can never name a different offset
/// than the one measured.
pub fn acpr_offset_hz(modulation: Modulation) -> f64 {
    match modulation {
        Modulation::Nfm => 25_000.0,
        Modulation::Am  => 9_000.0,
        _               => 200_000.0,
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
            Modulation::Wfm     => "WFM",
            Modulation::Nfm     => "NFM",
            Modulation::Am      => "AM",
            Modulation::Unknown => "\u{2014}",
        }
    }

    /// Whether a modulation was confidently classified (not the no-signal fallback).
    pub fn is_known(self) -> bool { !matches!(self, Modulation::Unknown) }
}

/// Minimum peak-to-noise (dB) for [`classify`] to commit to a modulation. Below
/// this there is no clear carrier at centre, so it reports [`Modulation::Unknown`]
/// rather than labelling noise.
pub const CLASSIFY_MIN_SNR_DB: f32 = 10.0;

/// Occupied bandwidth at or above which the carrier reads as broadcast FM.
///
/// Not the 180 kHz of Carson's rule, and not the 200 kHz channel allocation. The
/// 99 % occupied bandwidth of a real WFM broadcast measures far less than either,
/// because the time-averaged spectrum of FM is strongly peaked at the carrier —
/// 92.8 MHz on a stub antenna measures ~85 kHz, with 91 % of its power inside
/// ±25 kHz. A weaker signal reads narrower still, since its skirts sink under the
/// noise floor before the threshold that bounds the measurement window can see them.
///
/// So the boundary sits well below the observed figure, and still far above narrow
/// FM, which tops out near 16 kHz. There is a factor of five of empty space between
/// the two populations; this is the middle of it.
pub const WFM_MIN_BW_HZ: u64 = 50_000;

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
        bw if bw >= WFM_MIN_BW_HZ => Modulation::Wfm,
        bw if bw >= NFM_MIN_BW_HZ => Modulation::Nfm,
        _                         => Modulation::Am,
    }
}

#[derive(Clone)]
pub struct SignalState {
    pub drops_per_sec:       u64,
    pub total_drops_session: u64,
    pub drop_history:        VecDeque<u64>,
    pub adc_saturation_pct:  f32,
    pub adc_saturation_peak: f32,
    pub saturation_history:  VecDeque<f32>,
    pub peak_to_nf_db:       f32,
    /// Power integrated over the occupied channel at centre, dBFS.
    ///
    /// The *channel*, not the capture: summing every bin made this a function of the
    /// sample rate, so widening the span raised the "channel power" of a station
    /// that had not changed. `f32::NEG_INFINITY` when there is no carrier to
    /// integrate — every reader already renders that as a dash.
    ///
    /// Distinct from [`Self::adc_rms_dbfs`], which is the full-bandwidth figure and
    /// belongs to ADC loading rather than to the signal.
    pub channel_power_dbfs:  f32,
    pub occupied_bw_hz:      u64,
    /// Adjacent-channel power ratio, dB relative to the in-channel power, at
    /// ±[`Self::acpr_offset_hz`]. `f32::NEG_INFINITY` when there is nothing to
    /// compare against yet (no occupied bandwidth), a band falls outside the
    /// captured span, or the bands would overlap the channel — never a guessed
    /// number. The two are written together: both finite, or neither.
    pub acpr_lower_db:       f32,
    pub acpr_upper_db:       f32,
    /// Absolute level (dBFS) of the louder — worse — of the two adjacent bands.
    /// Paired with `acpr_lower_db` / `acpr_upper_db`; same undefined sentinel,
    /// which also covers a genuinely silent adjacent band.
    pub adj_carrier_dbfs:    f32,
    /// The offset the ratio above was actually measured at, from
    /// [`acpr_offset_hz`]. Recorded rather than re-derived so the panel's labels
    /// and the adjacent-band frequency it prints can never disagree with the
    /// measurement — the modulation could change between the two reads otherwise.
    pub acpr_offset_hz:      f64,
    pub usb_errors_session:   u64,
    pub usb_errors_last_poll: u64,
    pub usb_error_history:    std::collections::VecDeque<u64>,
    /// Recent SNR (peak/noise-floor) samples, pushed by the rx poll task roughly
    /// every 500 ms while streaming. Powers the micro_signal trend arrow.
    pub snr_history:          VecDeque<f32>,
    /// Recent channel-power (dBFS) samples — pushed alongside `snr_history` at the
    /// same ~500 ms cadence. Powers the command rail's PWR sparkline + trend.
    pub pwr_history:          VecDeque<f32>,
    /// Recent noise-floor (dBFS) samples — pushed alongside `snr_history`. Powers
    /// the command rail's NF sparkline + trend.
    pub nf_history:           VecDeque<f32>,
    /// Recent ADC-saturation (%) samples — pushed alongside `snr_history` at the
    /// same ~500 ms / [`crate::state::SNR_HISTORY_LEN`] depth so the command rail's
    /// SAT trace fills like the other three. Distinct from [`Self::saturation_history`],
    /// which feeds the health panels' mini-graph at the 200 ms / 64-deep cadence.
    pub sat_history:          VecDeque<f32>,
    /// Unix-epoch second of the most recent ADC clip (saturation ≥ [`SAT_CLIP_PCT`]),
    /// for the rail's fading "last clip Xs" alert-memory. `None` = none this session.
    pub last_clip_at:         Option<u64>,
    /// Rough modulation estimate for the signal at centre, refreshed each display
    /// frame by the FFT worker via [`classify`]. Drives the lab_signal headline /
    /// banner and, later, the demod panel's mode-adaptive view.
    pub modulation:           Modulation,
    /// ADC loading for the Lab RF bench, refreshed each ~200 ms window: the loudest
    /// sample (`adc_peak_dbfs`), the full-bandwidth RMS level (`adc_rms_dbfs`, total
    /// I/Q power vs full scale — distinct from the in-channel `channel_power_dbfs`),
    /// and the clipped-sample count in the last window (`adc_clip_events`).
    pub adc_peak_dbfs:        f32,
    pub adc_rms_dbfs:         f32,
    pub adc_clip_events:      u64,
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
            drops_per_sec: 0, total_drops_session: 0, drop_history: VecDeque::new(),
            adc_saturation_pct: 0.0, adc_saturation_peak: 0.0,
            saturation_history: VecDeque::new(),
            peak_to_nf_db: 0.0,
            channel_power_dbfs: f32::NEG_INFINITY,
            occupied_bw_hz: 0,
            acpr_lower_db: f32::NEG_INFINITY,
            acpr_upper_db: f32::NEG_INFINITY,
            adj_carrier_dbfs: f32::NEG_INFINITY,
            acpr_offset_hz: acpr_offset_hz(Modulation::Unknown),
            usb_errors_session: 0, usb_errors_last_poll: 0,
            usb_error_history: VecDeque::new(),
            snr_history: hist(), pwr_history: hist(), nf_history: hist(), sat_history: hist(),
            last_clip_at: None,
            modulation: Modulation::Unknown,
            adc_peak_dbfs: 0.0, adc_rms_dbfs: 0.0, adc_clip_events: 0,
        }
    }
}

impl SignalState {
    /// Short-term SNR trend in dB: mean of the most recent half of
    /// `snr_history` minus the mean of the older half. Positive means the
    /// signal is strengthening. `None` until there are enough samples.
    pub fn snr_delta(&self) -> Option<f32> {
        let n = self.snr_history.len();
        if n < 4 { return None; }
        let half = n / 2;
        let older:  f32 = self.snr_history.iter().take(half).sum::<f32>() / half as f32;
        let recent: f32 = self.snr_history.iter().skip(n - half).sum::<f32>() / half as f32;
        Some(recent - older)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_history(samples: &[f32]) -> SignalState {
        let mut s = SignalState {
            drops_per_sec: 0, total_drops_session: 0, drop_history: VecDeque::new(),
            adc_saturation_pct: 0.0, adc_saturation_peak: 0.0, saturation_history: VecDeque::new(),
            peak_to_nf_db: 0.0, channel_power_dbfs: 0.0, occupied_bw_hz: 0,
            acpr_lower_db: f32::NEG_INFINITY, acpr_upper_db: f32::NEG_INFINITY,
            adj_carrier_dbfs: f32::NEG_INFINITY,
            acpr_offset_hz: acpr_offset_hz(Modulation::Unknown),
            usb_errors_session: 0, usb_errors_last_poll: 0, usb_error_history: VecDeque::new(),
            snr_history: VecDeque::new(), pwr_history: VecDeque::new(), nf_history: VecDeque::new(),
            sat_history: VecDeque::new(),
            last_clip_at: None,
            modulation: Modulation::Unknown,
            adc_peak_dbfs: 0.0, adc_rms_dbfs: 0.0, adc_clip_events: 0,
        };
        s.snr_history.extend(samples.iter().copied());
        s
    }

    #[test]
    fn classify_gates_on_signal_presence() {
        // Weak carrier or no occupancy → no guess.
        assert_eq!(classify(5.0, 180_000), Modulation::Unknown);
        assert_eq!(classify(40.0, 0),      Modulation::Unknown);
    }

    #[test]
    fn classify_bands_by_occupied_bandwidth() {
        assert_eq!(classify(40.0, 180_000), Modulation::Wfm);
        assert_eq!(classify(40.0, WFM_MIN_BW_HZ), Modulation::Wfm); // wide boundary
        assert_eq!(classify(40.0, 15_000),  Modulation::Nfm);
        assert_eq!(classify(40.0, NFM_MIN_BW_HZ), Modulation::Nfm); // narrow-FM boundary
        assert_eq!(classify(40.0, 8_000),   Modulation::Am);
    }

    #[test]
    fn acpr_offset_follows_the_channel_step_of_the_service() {
        // Broadcast FM's 200 kHz step applied to a 12.5 kHz narrow-FM channel would
        // measure a band eight channels away.
        assert_eq!(acpr_offset_hz(Modulation::Wfm), 200_000.0);
        assert_eq!(acpr_offset_hz(Modulation::Nfm), 25_000.0);
        assert_eq!(acpr_offset_hz(Modulation::Am),   9_000.0);
        // An unclassified carrier rests in the broadcast shape, like the panel does.
        assert_eq!(acpr_offset_hz(Modulation::Unknown), acpr_offset_hz(Modulation::Wfm));
    }

    #[test]
    fn acpr_offset_clears_the_bandwidth_of_its_own_service() {
        // The measurement is only meaningful while the adjacent bands sit clear of
        // the channel, and each band is as wide as the occupied bandwidth. So every
        // offset has to exceed what its own modulation typically occupies, or the
        // overlap guard in `acpr_bands` would suppress the reading for good.
        for (m, typical_bw) in [(Modulation::Wfm, 120_000.0),
                                (Modulation::Nfm, 15_000.0),
                                (Modulation::Am,   8_000.0)] {
            assert!(acpr_offset_hz(m) > typical_bw,
                    "{m:?}: offset {} does not clear {typical_bw}", acpr_offset_hz(m));
        }
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
}
