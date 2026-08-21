//! `DemodState` — the measurement the demodulator produces, and the gating that
//! decides whether it runs at all. See `dev_docs/demod-plan.md`.
//!
//! The demod is a measurement instrument, not a receiver: it produces numbers
//! about the signal, never audio. Phase 2 carries the FM discriminator's output
//! only; the MPX / pilot / RDS / audio fields land in later phases.

use std::time::{Duration, Instant};

/// A measurement goes stale this long after the last worker update. At the
/// worker's 250 ms cadence, this tolerates a couple of missed updates before the
/// panel stops presenting the number as live.
pub const DEMOD_STALE_AFTER: Duration = Duration::from_millis(1_500);

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
}
