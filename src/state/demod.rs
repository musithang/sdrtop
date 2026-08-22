//! `DemodState` — the measurement the demodulator produces, and the gating that
//! decides whether it runs at all. See `dev_docs/demod-plan.md`.
//!
//! The demod is a measurement instrument, not a receiver: it produces numbers
//! about the signal, never audio. Phase 2 carries the FM discriminator's output
//! only; the MPX / pilot / RDS / audio fields land in later phases.

use std::sync::Arc;
use std::time::{Duration, Instant};

pub use crate::signal::demod::{PilotMeasure, PilotState};
pub use crate::signal::rds::{RdsData, PTY_NAMES};

/// One MPX baseband spectrum. `mags_hz[k]` is the deviation contributed by the
/// component at `k · bin_hz`, so the display reads directly in Hz.
///
/// Shared behind an `Arc`: `SdrMetrics` is deep-cloned every frame, and this is
/// the one large field the demod adds.
#[derive(Clone, Debug)]
pub struct MpxFrame {
    pub bin_hz:  f64,
    pub mags_hz: Vec<f32>,
}

/// Demod channel offsets step by this much per key press — coarse enough to walk
/// a station off the DC spike in a couple of presses, fine enough to land inside
/// a WFM channel.
pub const OFFSET_STEP_HZ: i64 = 25_000;

/// A channel this close to the tuned centre is sitting on the front-end's DC
/// offset / LO leakage. Only worth flagging when no DC correction is active.
pub const DC_PROXIMITY_HZ: i64 = 30_000;

/// A measurement goes stale this long after the last worker update. At the
/// worker's 250 ms cadence, this tolerates a couple of missed updates before the
/// panel stops presenting the number as live.
pub const DEMOD_STALE_AFTER: Duration = Duration::from_millis(1_500);

/// One AM envelope measurement.
///
/// `positive_pct` and `negative_pct` are reported separately from `depth_pct`
/// because they fail differently: a negative depth approaching 100 % pinches the
/// carrier off and splatters, while a mismatched pair points at a modulator fault
/// rather than merely too much level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AmMeasure {
    pub depth_pct:    f32,
    pub positive_pct: f32,
    pub negative_pct: f32,
    pub carrier_dbfs: f32,
}

/// A detected CTCSS tone. `margin_db` is how far it stood above the best
/// non-adjacent candidate — the evidence for the identification, kept so the
/// panel can show a confident detection differently from a borderline one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CtcssMeasure {
    pub tone_hz:      f32,
    pub deviation_hz: f32,
    pub margin_db:    f32,
}

/// One FM discriminator measurement, all in Hz.
///
/// `peak_dev_hz` / `rms_dev_hz` are measured *about* `carrier_offset_hz`, so a
/// mistuned radio reports its tuning error as offset rather than as modulation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FmMeasure {
    pub peak_dev_hz:       f32,
    pub rms_dev_hz:        f32,
    pub carrier_offset_hz: f32,
}

#[derive(Clone, Debug)]
pub struct DemodState {
    /// User's on/off intent, toggled from the panel's focus mode. Survives preset
    /// switches, so leaving and returning to `lab_signal` restores the choice.
    pub user_on: bool,
    /// Whether blocks are actually being forwarded to the demod worker: the
    /// user's intent AND the demod preset being active. Recomputed each frame in
    /// `App::draw`, the same way `sweep.active` follows the active preset.
    pub enabled: bool,
    /// Decimation factor in use, and the channel rate it actually lands on —
    /// reported rather than assumed, since the factor is rounded.
    pub decimation:      usize,
    pub channel_rate_hz: f64,
    /// The current measurement, or `None` when there is nothing honest to show
    /// (no carrier, too weak, or a modulation an FM discriminator says nothing
    /// about). Never a stale number left behind.
    pub fm: Option<FmMeasure>,
    /// Channel centre relative to the tuned frequency. Non-zero demodulates a
    /// station the radio is not centred on — which is how the channel is kept off
    /// the DC spike.
    pub offset_hz: i64,
    /// Demodulator forced by the user, or `None` to follow the classifier.
    ///
    /// The classifier is a 99 %-occupied-bandwidth heuristic over the whole
    /// captured span, so on a wide span it reads ≥ 100 kHz — and therefore WFM —
    /// for almost anything, noise included. That is honest for a rough badge but
    /// far too coarse to pick a demodulator, so the bench can say what it is
    /// listening to.
    pub mode_override: Option<super::Modulation>,
    /// Recovered MPX baseband spectrum, and the 19 kHz pilot read from it.
    /// WFM only — neither concept exists for NFM or AM.
    pub mpx:   Option<Arc<MpxFrame>>,
    pub pilot: Option<PilotMeasure>,
    /// Everything decoded from the 57 kHz RDS subcarrier. WFM only.
    ///
    /// Unlike the other measurements this one *accumulates*: PS and RadioText are
    /// built up character by character over seconds, so the decoder keeps its own
    /// running state and publishes a snapshot. `None` means the RDS chain is not
    /// running at all, not that the station carries no RDS —
    /// [`RdsData::groups_ok`] is what separates those.
    pub rds: Option<Arc<RdsData>>,
    /// Whether the RDS block synchroniser is currently tracking the group
    /// structure. Distinguishes "receiving RDS" from "hoping to".
    pub rds_sync: bool,
    /// AM envelope measurement. Mutually exclusive with `fm` in practice: the
    /// classifier picks one demodulator, and the other's field stays `None`.
    pub am:    Option<AmMeasure>,
    /// CTCSS subaudible tone, NFM only. `None` covers both "no tone" and "not
    /// enough contiguous audio yet" — [`Self::ctcss_searching`] separates them.
    pub ctcss: Option<CtcssMeasure>,
    /// Fraction of the CTCSS observation window currently filled, 0..=1. Resets
    /// whenever a dropped block breaks the run, so the panel can honestly show
    /// "searching" instead of implying there is no tone.
    pub ctcss_fill: f32,
    /// Monotonic counter stamped on each forwarded block, so the worker can tell
    /// a contiguous run from one interrupted by a dropped block.
    pub block_seq: u64,
    pub last_update: Option<Instant>,
}

impl Default for DemodState {
    /// `user_on` starts **true**: arriving on the demod bench should demodulate,
    /// not wait to be switched on. It stays free everywhere else because
    /// `enabled` also requires the `lab_signal` preset to be active.
    fn default() -> Self {
        Self {
            user_on:         true,
            enabled:         false,
            decimation:      0,
            channel_rate_hz: 0.0,
            fm:              None,
            offset_hz:       0,
            mode_override:   None,
            mpx:             None,
            pilot:           None,
            rds:             None,
            rds_sync:        false,
            am:              None,
            ctcss:           None,
            ctcss_fill:      0.0,
            block_seq:       0,
            last_update:     None,
        }
    }
}

impl DemodState {
    /// Whether the last measurement is too old to present as live. Also true when
    /// there has never been one.
    pub fn is_stale(&self) -> bool {
        match self.last_update {
            Some(t) => t.elapsed() > DEMOD_STALE_AFTER,
            None    => true,
        }
    }

    /// The measurement to render, or `None` when it is missing or stale. The one
    /// call a panel should make — it cannot accidentally show an expired reading.
    pub fn live(&self) -> Option<FmMeasure> {
        if self.is_stale() { None } else { self.fm }
    }

    /// The MPX spectrum to render, subject to the same staleness rule.
    pub fn live_mpx(&self) -> Option<&Arc<MpxFrame>> {
        if self.is_stale() { None } else { self.mpx.as_ref() }
    }

    /// The pilot reading to render, subject to the same staleness rule.
    pub fn live_pilot(&self) -> Option<PilotMeasure> {
        if self.is_stale() { None } else { self.pilot }
    }

    /// The RDS snapshot to render, subject to the same staleness rule.
    pub fn live_rds(&self) -> Option<&Arc<RdsData>> {
        if self.is_stale() { None } else { self.rds.as_ref() }
    }

    /// The AM reading to render, subject to the same staleness rule.
    pub fn live_am(&self) -> Option<AmMeasure> {
        if self.is_stale() { None } else { self.am }
    }

    /// The CTCSS reading to render, subject to the same staleness rule.
    pub fn live_ctcss(&self) -> Option<CtcssMeasure> {
        if self.is_stale() { None } else { self.ctcss }
    }

    /// Whether the CTCSS detector is still filling its observation window rather
    /// than reporting the absence of a tone. The distinction matters: "no tone
    /// yet" and "no tone" look identical otherwise, and only one of them is a
    /// finding.
    pub fn ctcss_searching(&self) -> bool {
        !self.is_stale() && self.ctcss.is_none() && self.ctcss_fill < 1.0
    }

    /// Legal offset range for the current sample rate: the channel must stay
    /// wholly inside the captured span, so its centre can reach no further than
    /// half the span minus half the channel.
    pub fn offset_limit_hz(&self, sample_rate: f64) -> i64 {
        let half_span = sample_rate / 2.0;
        let half_chan = self.channel_rate_hz.max(0.0) / 2.0;
        (half_span - half_chan).max(0.0) as i64
    }

    /// The modulation the demodulator should actually use: the user's choice when
    /// they have made one, otherwise the classifier's.
    pub fn effective_modulation(&self, classified: super::Modulation) -> super::Modulation {
        self.mode_override.unwrap_or(classified)
    }

    /// Advance the mode selector: auto → WFM → NFM → AM → auto.
    pub fn cycle_mode(&mut self) -> Option<super::Modulation> {
        use super::Modulation::*;
        self.mode_override = match self.mode_override {
            None            => Some(Wfm),
            Some(Wfm)       => Some(Nfm),
            Some(Nfm)       => Some(Am),
            Some(Am) | Some(Unknown) => None,
        };
        self.mode_override
    }

    /// Whether the channel is close enough to the tuned centre to be competing
    /// with the front-end's DC artefact. `dc_corrected` suppresses it: the
    /// existing DC block already cleans that up.
    pub fn on_dc_spike(&self, dc_corrected: bool) -> bool {
        !dc_corrected && self.offset_hz.abs() < DC_PROXIMITY_HZ
    }
}

/// Bins this close to the tuned centre are skipped when hunting for a carrier.
///
/// The centre bin is where the front-end parks its DC offset and LO leakage, and
/// on a HackRF that artefact routinely out-peaks a real station a few hundred kHz
/// away. Without the guard, "snap to the strongest carrier" reliably snaps to the
/// artefact and lands the channel right back on DC — the opposite of the point.
pub const SNAP_DC_GUARD_HZ: f64 = 10_000.0;

/// Offset from the tuned centre, in Hz, of the strongest *carrier* in an
/// fftshifted spectrum — what "snap the demod onto the loudest carrier" resolves
/// to. Bins within [`SNAP_DC_GUARD_HZ`] of centre are excluded as artefact.
///
/// `None` for an empty spectrum, a nonsensical rate, or a span so narrow that the
/// guard swallows it — the caller then leaves the offset alone rather than
/// jumping to an invented frequency.
pub fn strongest_offset_hz(bins_dbfs: &[f32], sample_rate: f64) -> Option<i64> {
    if bins_dbfs.is_empty() || !sample_rate.is_finite() || sample_rate <= 0.0 { return None; }
    let n = bins_dbfs.len();
    let bin_hz = sample_rate / n as f64;
    let centre = n as f64 / 2.0;
    let guard_bins = (SNAP_DC_GUARD_HZ / bin_hz).ceil();

    let mut best: Option<(usize, f32)> = None;
    for (i, &v) in bins_dbfs.iter().enumerate() {
        if (i as f64 - centre).abs() <= guard_bins { continue; }
        if best.is_none_or(|(_, bv)| v > bv) { best = Some((i, v)); }
    }
    let (idx, _) = best?;
    Some(((idx as f64 - centre) * bin_hz).round() as i64)
}

/// Nominal peak-deviation limit (Hz) for a modulation — the full-scale reference
/// the deviation bar is drawn against. WFM broadcast is ±75 kHz; narrow-band FM
/// voice is ±5 kHz on 25 kHz channels.
pub fn deviation_limit_hz(modulation: super::Modulation) -> f32 {
    match modulation {
        super::Modulation::Nfm => 5_000.0,
        _                      => 75_000.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Modulation;

    fn measure() -> FmMeasure {
        FmMeasure { peak_dev_hz: 40_000.0, rms_dev_hz: 28_000.0, carrier_offset_hz: -1_200.0 }
    }

    #[test]
    fn fresh_state_is_stale_and_has_no_reading() {
        let d = DemodState::default();
        assert!(d.is_stale());
        assert!(d.live().is_none());
        // Intent defaults on, but nothing runs until the preset enables it.
        assert!(d.user_on);
        assert!(!d.enabled);
    }

    #[test]
    fn a_just_updated_measurement_is_live() {
        let d = DemodState { fm: Some(measure()), last_update: Some(Instant::now()), ..Default::default() };
        assert!(!d.is_stale());
        assert_eq!(d.live(), Some(measure()));
    }

    #[test]
    fn an_old_measurement_does_not_render() {
        let old = Instant::now().checked_sub(DEMOD_STALE_AFTER * 2);
        let d = DemodState { fm: Some(measure()), last_update: old, ..Default::default() };
        // A checked_sub failure would make this vacuous, so assert we really aged it.
        assert!(old.is_some());
        assert!(d.is_stale());
        assert!(d.live().is_none(), "a stale reading must never be presented as live");
    }

    #[test]
    fn cleared_measurement_reports_nothing_even_when_recent() {
        let d = DemodState { fm: None, last_update: Some(Instant::now()), ..Default::default() };
        assert!(d.live().is_none());
    }

    #[test]
    fn deviation_limits_follow_the_modulation() {
        assert_eq!(deviation_limit_hz(Modulation::Nfm), 5_000.0);
        assert_eq!(deviation_limit_hz(Modulation::Wfm), 75_000.0);
    }

    #[test]
    fn strongest_offset_reads_a_shifted_spectrum() {
        // 8 bins over 800 kHz → 100 kHz per bin, centre (DC) at index 4.
        let mut bins = vec![-90.0f32; 8];
        bins[6] = -10.0;
        assert_eq!(strongest_offset_hz(&bins, 800_000.0), Some(200_000));
        // A peak below centre reads negative.
        bins[6] = -90.0;
        bins[2] = -10.0;
        assert_eq!(strongest_offset_hz(&bins, 800_000.0), Some(-200_000));
    }

    #[test]
    fn strongest_offset_ignores_the_dc_artefact() {
        // A towering LO-leakage spike at centre with a real, weaker station at
        // +200 kHz. Snapping to the spike would put the channel back on DC, which
        // is exactly what the offset exists to escape.
        let mut bins = vec![-90.0f32; 8];
        bins[4] = 0.0;    // DC spike, by far the strongest bin
        bins[6] = -40.0;  // the actual carrier
        assert_eq!(strongest_offset_hz(&bins, 800_000.0), Some(200_000));
    }

    #[test]
    fn strongest_offset_declines_to_guess() {
        assert_eq!(strongest_offset_hz(&[], 2_000_000.0), None);
        assert_eq!(strongest_offset_hz(&[-10.0, -20.0], 0.0), None);
        // A span narrower than the guard leaves no candidate bins at all.
        assert_eq!(strongest_offset_hz(&[-10.0, -20.0, -30.0, -40.0], 8_000.0), None);
    }

    #[test]
    fn offset_limit_keeps_the_channel_inside_the_span() {
        let d = DemodState { channel_rate_hz: 333_000.0, ..Default::default() };
        // 2 Msps: half span 1 MHz, minus half a 333 kHz channel.
        assert_eq!(d.offset_limit_hz(2_000_000.0), 833_500);
    }

    #[test]
    fn mode_override_outranks_the_classifier() {
        let mut d = DemodState::default();
        // With no override the classifier's answer stands.
        assert_eq!(d.effective_modulation(Modulation::Wfm), Modulation::Wfm);
        d.mode_override = Some(Modulation::Nfm);
        assert_eq!(d.effective_modulation(Modulation::Wfm), Modulation::Nfm);
    }

    #[test]
    fn mode_cycle_returns_to_auto() {
        let mut d = DemodState::default();
        assert_eq!(d.cycle_mode(), Some(Modulation::Wfm));
        assert_eq!(d.cycle_mode(), Some(Modulation::Nfm));
        assert_eq!(d.cycle_mode(), Some(Modulation::Am));
        // Full circle: the user can always hand control back to the classifier.
        assert_eq!(d.cycle_mode(), None);
        assert_eq!(d.cycle_mode(), Some(Modulation::Wfm));
    }

    #[test]
    fn dc_spike_warning_only_without_correction() {
        let centred = DemodState { offset_hz: 0, ..Default::default() };
        assert!(centred.on_dc_spike(false));
        // The existing DC block already handles it — no need to nag.
        assert!(!centred.on_dc_spike(true));
        // Tuned well off centre, the artefact is out of the channel.
        let offset = DemodState { offset_hz: 200_000, ..Default::default() };
        assert!(!offset.on_dc_spike(false));
    }
}
