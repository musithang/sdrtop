// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

mod acc;
mod demod;
#[cfg(test)]
pub(crate) mod fixture;
pub mod ft8;
mod home;
mod iq;
mod lab;
mod micro;
mod net;
mod observer;
mod pager;
mod radio;
mod record;
mod selection;
mod signal;
mod spectrum;
mod sweep;
mod system;
mod timing;
mod ui;
mod waterfall;

pub(crate) use acc::Accumulators;
pub use demod::{
    deviation_limit_hz, strongest_offset_hz, AmMeasure, CtcssMeasure, DemodState, FmMeasure,
    MpxFrame, PilotState, RdsData, OFFSET_STEP_HZ, PTY_NAMES, RDS_AGED_AFTER, RDS_DROPPED_AFTER,
};
pub use iq::{IqCalState, IqState, CONSTELLATION_CAP};
pub use lab::{LabState, NoiseReading, RfFreeze};
pub use micro::MicroView;
// `NetExit` is not re-exported, for the reason `SweepExit` is not: it exists
// to be returned and destructured, and no call site has to name it.
pub use net::{
    who_with, AddressBook, AddressDisplay, BandOccupancy, BlePacket, BlePacketView, BtHop,
    CellReading, CodedFacts, ConnectionView, ExtInfo, ExtRole, FollowedConnection, LockTarget,
    NetMode, NetState, PacketsView, PduKind, ReceiverNow, RetuneRun, BLE_PACKET_LIMIT,
    BT_HOP_LIMIT, COLUMN_INTERVAL, FULL_ADDRESS_WIDTH, HISTORY_COLUMNS,
};
// The roster's sort keys, which the roster's columns are held to.
#[cfg(test)]
pub use net::ROSTER_SORT_KEYS;
// Named by the NET worker, which counts its arrivals into it, and Wi-Fi's
// funnel and latest frame with it.
pub use ft8::{verdict as ft8_verdict, CycleEntry, CycleOutcome, Ft8State, MapRange, Station};
pub use home::{Home, StationField};
#[cfg(test)]
pub use net::WifiSurvey;
pub use net::{AirColumn, Rhythm, Spender, READING_COLUMNS, RHYTHM_BINS};
pub use net::{Capture, FrameZoom, Lost};
pub use net::{Family, LeastBusy, NetDecodeHealth, RowKind, Visit, WifiClock, WifiFunnel, WifiRow};
#[cfg(test)]
pub use net::{FramesView, Holdoff, Trigger};
pub use observer::ObserverState;
pub use pager::{PagerState, PagerView};
pub use radio::{
    offset_ppm, FrequencyReference, OffsetBasis, Provenance, RadioState, REFERENCE_STALE_S,
};
pub use record::{RecordProgress, RecordState};
pub use selection::Selection;
pub use signal::{
    acpr_offset_hz, classify, judge, Carrier, Modulation, RadioDropAccount, SignalState, Verdict,
    ACPR_FLOOR_DB, ADC_COMFORT_DBFS, CLASSIFY_MIN_SNR_DB, SAT_CLIP_PCT, SAT_CRIT_PCT, SAT_WARN_PCT,
    WFM_MAX_BW_HZ,
};
pub use spectrum::{SpectrumMarker, SpectrumState, SpectrumStyle};
pub use sweep::{SweepConfig, SweepFrame, SweepState, SWEEP_SETTLING_MS};
pub use system::SystemState;
pub use timing::{TimingCause, TimingQuality, TimingState, HACKRF_SAMPLES_PER_TRANSFER};
pub use ui::{
    active_recall_slot, recall_from_hz, recall_to_hz, DeviceOptionUpdate, InputMode, LogEntry,
    LogLevel, MenuFocus, MenuPath, MenuState, MenuTile, RailMode, UiState, RECALL_SLOTS,
};
pub use waterfall::{BinAxis, BinWindow, FftFrame, WaterfallState, WATERFALL_MIN_ROWS};

pub const THROUGHPUT_HISTORY_LEN: usize = 64;
/// Depth of the per-callback gap ring feeding the `lab_timing` strip chart. ~256
/// keeps roughly the last 3 s at the HackRF/RTL callback cadence (~73–76 cb/s),
/// enough to fill the chart's ~160-column window with headroom.
pub const CB_GAP_HISTORY_LEN: usize = 256;
/// SNR/PWR/NF history depth - 120 samples at the 500 ms push cadence = 60 s window.
/// Must be ≥ 2 × max scope_top_w so the braille mini-scope fills completely.
pub const SNR_HISTORY_LEN: usize = 120;
pub const DEFAULT_LNA_GAIN: u32 = 16;
pub const DEFAULT_VGA_GAIN: u32 = 20;
pub const DEFAULT_FREQUENCY: u64 = 2_400_000_000;
pub const DEFAULT_SAMPLE_RATE: f64 = 10_000_000.0;

/// Where the gain knob sits on its vendor curve; see
/// [`SdrMetrics::curve_reading`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CurveReading {
    pub name: &'static str,
    pub short: &'static str,
    /// The step the chain is on, or the nearest one when `on` is false.
    pub step: usize,
    pub last: usize,
    /// Whether the chain is exactly on `step`, rather than moved off the
    /// curve by hand.
    pub on: bool,
    /// The whole chain in dB, where every stage's dB holds at the tuned
    /// frequency.
    pub total_db: Option<f64>,
}

impl CurveReading {
    /// `12/21`, or `≈12/21` for a chain off the curve: the nearest step,
    /// marked as not where the chain is.
    pub fn step_text(&self) -> String {
        let near = if self.on { "" } else { "\u{2248}" };
        format!("{near}{}/{}", self.step, self.last)
    }
}

#[derive(Clone)]
pub struct SdrMetrics {
    pub radio: RadioState,
    pub signal: SignalState,
    pub iq: IqState,
    pub observer: ObserverState,
    pub spectrum: SpectrumState,
    pub waterfall: WaterfallState,
    pub system: SystemState,
    pub timing: TimingState,
    pub sweep: SweepState,
    pub ui: UiState,
    /// Lab "instrument mode" measurement state (REF/AVG/CAL). See [`LabState`].
    pub lab: LabState,
    /// Demodulator measurement + gating for the `lab_signal` bench. See [`DemodState`].
    pub demod: DemodState,
    /// What the 2.4 GHz receiver is doing. See [`NetState`].
    pub net: NetState,
    /// What the pager receiver has heard. See [`PagerState`].
    pub pager: PagerState,
    /// What the FT8 receiver has heard. See [`Ft8State`].
    pub ft8: Ft8State,
    /// Where you listen from and your call. See [`Home`].
    pub home: Home,
    /// What the IQ recorder is doing. See [`RecordState`].
    pub record: RecordState,
    /// Active device's capability descriptor - drives capability-aware UI
    /// rendering (gain model, BB filter / Friis applicability, ranges). Shared
    /// (Arc) so the per-frame `SdrMetrics` clone stays cheap.
    pub caps: std::sync::Arc<crate::hardware::DeviceCapabilities>,
    pub device_options: std::sync::Arc<Vec<crate::hardware::DeviceOption>>,
    pub(crate) acc: Accumulators,
}

impl SdrMetrics {
    /// How stage `index` reads at the frequency the radio is tuned to:
    /// decibels where they were measured, its position elsewhere. `None`
    /// past the end of the chain.
    pub fn stage_reading(&self, index: usize) -> Option<crate::hardware::StageReading> {
        let spec = self.caps.gain.stages().into_iter().nth(index)?;
        if self.agc_sets(index) {
            return Some(crate::hardware::StageReading::Auto);
        }
        Some(spec.reading(self.radio.stage_gain(index), self.radio.frequency))
    }

    /// The highest level the samples can honestly reach on this radio where
    /// it is tuned, dBFS. See [`crate::hardware::GainModel::ceiling_dbfs_at`].
    pub fn ceiling_dbfs(&self) -> f64 {
        self.caps.gain.ceiling_dbfs_at(self.radio.frequency)
    }

    /// The ceiling where the radio is tuned as the screen writes it: `-3`, or
    /// `-14 to -12` for one measured over a span; `0` at the converter.
    pub fn ceiling_text(&self) -> String {
        self.ceiling()
            .map_or_else(|| "0".to_string(), |c| c.text_at(self.radio.frequency))
    }

    /// The headroom as the screen shows it: the ceiling and the peak each
    /// rounded as they are printed, then subtracted, so the figure always
    /// equals the difference of the two figures beside it.
    pub fn shown_headroom_db(&self) -> f64 {
        crate::ui::rf_calc::shown_headroom_db(self.ceiling_dbfs(), self.signal.adc_peak_dbfs as f64)
    }

    /// The part that stops the samples before the converter, when there is
    /// one, and what it does there.
    pub fn ceiling(&self) -> Option<&crate::hardware::Ceiling> {
        self.caps.gain.ceiling()
    }

    /// The ADC peak against that ceiling: 0 at it, negative below. What the
    /// staging verdicts, the headroom and auto-gain judge.
    pub fn peak_to_ceiling_db(&self) -> f32 {
        self.signal.adc_peak_dbfs - self.ceiling_dbfs() as f32
    }

    /// Whether stage `index` is being set by the device's own AGC now.
    pub fn agc_sets(&self, index: usize) -> bool {
        self.radio.amp_enabled && self.caps.gain.auto_governs(index)
    }

    /// Whether every stage's setting is a dB at the tuned frequency, so that
    /// adding them up gives a gain.
    pub fn chain_db_holds(&self) -> bool {
        self.caps
            .gain
            .stages()
            .iter()
            .enumerate()
            .all(|(i, s)| s.db_holds_at(self.radio.frequency) && !self.agc_sets(i))
    }

    /// Where the gain knob is on its vendor curve, when it walks one.
    ///
    /// The step the chain is exactly on, or, for a chain moved off the curve
    /// by hand, the step it would rejoin at, said as such; and the chain's
    /// total where every stage is a dB here.
    pub fn curve_reading(&self) -> Option<CurveReading> {
        let gm = &self.caps.gain;
        // Under the AGC the chain is not on any step the knob set.
        if (0..gm.stages().len()).any(|i| self.agc_sets(i)) {
            return None;
        }
        let curve = gm.curves().get(self.radio.gain_curve?)?;
        let stages = gm.stages();
        let exact = curve.step_of(&stages, &self.radio.gains);
        Some(CurveReading {
            name: curve.name,
            short: curve.short,
            step: exact.unwrap_or_else(|| curve.nearest_step(&stages, &self.radio.gains)),
            last: curve.last_step(),
            on: exact.is_some(),
            total_db: self.chain_db_holds().then(|| self.radio.total_gain()),
        })
    }

    /// Why the I/Q balance the moments measure is not the front end's, or
    /// `None` when it is.
    ///
    /// The gain and phase imbalance are read from the stream's second moments
    /// on the assumption that the front end's image mirrors about 0 Hz. A radio
    /// whose library turns the samples off the LO (`RadioState::lo_offset_hz`
    /// not zero) has its image mirror about the LO instead, and the turning
    /// spreads I into Q over every window, so the moments read balanced
    /// whatever the hardware does; such a library has corrected the balance
    /// itself besides. The readings, the image they predict, the trend and the
    /// auto-cal that would correct from them all stand down with this reason.
    pub fn iq_balance_refusal(&self) -> Option<String> {
        let offset = self.radio.lo_offset_hz;
        (offset != 0).then(|| {
            format!(
                "the samples are turned {:+.1} kHz off the LO, so their I/Q balance is not the front end's",
                offset as f64 / 1e3
            )
        })
    }

    /// Why a gain decision made in dB cannot be made at the tuned frequency,
    /// or `None` when it can: the first stage whose dB was not measured here,
    /// and the band it was.
    ///
    /// Auto-gain sets a target in dB and spreads it by each stage's dB; with
    /// a stage outside its measured band those dB are not what the stage
    /// gives, so the target would land somewhere nobody chose.
    pub fn chain_db_refusal(&self) -> Option<String> {
        let hz = self.radio.frequency;
        let stages = self.caps.gain.stages();
        let auto: Vec<&str> = stages
            .iter()
            .enumerate()
            .filter(|(i, _)| self.agc_sets(*i))
            .map(|(_, s)| s.name.as_str())
            .collect();
        if !auto.is_empty() {
            return Some(format!("the tuner AGC sets {}", auto.join(" and ")));
        }
        let stage = self
            .caps
            .gain
            .stages()
            .into_iter()
            .find(|s| !s.db_holds_at(hz))?;
        Some(match &stage.measured_in {
            Some(band) => format!(
                "{}'s gain in dB was measured from {:.0} to {:.0} MHz only",
                stage.name,
                *band.start() as f64 / 1e6,
                *band.end() as f64 / 1e6,
            ),
            None => format!("{}'s gain is not in dB here", stage.name),
        })
    }

    /// The gain figure a readout should show for the front-end knob.
    ///
    /// **The whole chain on a single-knob device, the front stage otherwise.**
    /// A HackRF presents LNA and VGA as separate controls, so its front stage is
    /// its own number. An RTL-SDR and a SoapySDR device present one knob, and on
    /// a SoapySDR device that knob's value is now spread across several stages
    /// by [`crate::hardware::gain::distribute`], so the front stage alone is
    /// only part of what the user asked for.
    ///
    /// Lives here rather than on `RadioState` because it needs `caps` to know
    /// which shape the device is, and duplicating the choice at each readout is
    /// how eight panels end up disagreeing.
    pub fn shown_gain(&self) -> u32 {
        if self.caps.gain.is_single() {
            self.radio.total_gain().max(0.0).round() as u32
        } else {
            self.radio.primary_gain()
        }
    }

    pub fn push_log(&mut self, msg: impl Into<String>) {
        self.ui.push_log(msg);
    }
}

#[cfg(test)]
mod stage_reading_tests {
    use super::SdrMetrics;
    use crate::hardware::StageReading;

    /// The R2's LNA reads in dB inside the band it was measured in and as a
    /// position outside it; the mixer and VGA, measured everywhere, read in
    /// dB at any frequency; and the chain adds up to a gain only where every
    /// stage does.
    #[test]
    fn a_stage_reads_by_where_the_radio_is_tuned() {
        let mut m = SdrMetrics::fixture().band_measured_chain();
        assert_eq!(
            m.stage_reading(0),
            Some(StageReading::Position { at: 14, of: 14 })
        );
        assert_eq!(m.stage_reading(1), Some(StageReading::Db(12.03)));
        assert_eq!(m.stage_reading(2), Some(StageReading::Db(26.79)));
        assert_eq!(m.stage_reading(3), None);
        assert!(!m.chain_db_holds());

        m.radio.frequency = 92_800_000;
        assert_eq!(m.stage_reading(0), Some(StageReading::Db(36.02)));
        assert!(m.chain_db_holds());
    }

    /// A decision in dB is refused, by the stage and its band, where that
    /// stage's dB was not measured; allowed inside the band and on any radio
    /// with no bands.
    #[test]
    fn a_decision_in_db_names_the_stage_it_cannot_trust() {
        let mut m = SdrMetrics::fixture().band_measured_chain();
        assert_eq!(
            m.chain_db_refusal().as_deref(),
            Some("LNA's gain in dB was measured from 88 to 170 MHz only")
        );
        m.radio.frequency = 92_800_000;
        assert_eq!(m.chain_db_refusal(), None);
        assert_eq!(SdrMetrics::fixture().chain_db_refusal(), None);
    }

    /// **Where the knob is on its curve**: the step and its total in dB in
    /// the LNA's band, the step and no total outside it; a chain moved off
    /// the curve by hand reads as the step it would rejoin at, marked; no
    /// curve, no reading.
    #[test]
    fn the_curve_reading_says_where_the_knob_is() {
        let mut m = SdrMetrics::fixture().band_measured_chain().on_curve(0, 10);
        m.radio.frequency = 92_800_000;
        let r = m.curve_reading().unwrap();
        assert_eq!((r.short, r.step, r.last, r.on), ("LIN", 10, 21, true));
        assert_eq!(r.step_text(), "10/21");
        assert!((r.total_db.unwrap() - 51.39).abs() < 1e-9);
        m.radio.frequency = 943_400_000;
        assert_eq!(m.curve_reading().unwrap().total_db, None);
        m.radio.gains = vec![36.02, 16.24, 49.96];
        let off = m.curve_reading().unwrap();
        assert_eq!((off.step, off.on), (21, false));
        assert_eq!(off.step_text(), "\u{2248}21/21");
        m.radio.gain_curve = None;
        assert_eq!(m.curve_reading(), None);
    }

    /// **What the tuner AGC sets reads as AGC**: on an R2 with its AGC on,
    /// the LNA and the mixer read `Auto` and the VGA keeps its dB; the chain
    /// has no total and no curve step, and a decision in dB says why. An
    /// RTL-SDR's tuner the same, where its last manual gain used to stay on
    /// screen.
    #[test]
    fn stages_under_the_agc_read_as_auto() {
        let mut m = SdrMetrics::fixture().band_measured_chain().on_curve(0, 10);
        m.radio.frequency = 92_800_000;
        m.radio.amp_enabled = true;
        assert_eq!(m.stage_reading(0), Some(StageReading::Auto));
        assert_eq!(m.stage_reading(1), Some(StageReading::Auto));
        assert_eq!(m.stage_reading(2), Some(StageReading::Db(33.51)));
        assert!(!m.chain_db_holds());
        assert_eq!(m.curve_reading(), None);
        assert_eq!(
            m.chain_db_refusal().as_deref(),
            Some("the tuner AGC sets LNA and MIX")
        );
        m.radio.amp_enabled = false;
        assert_eq!(m.stage_reading(0), Some(StageReading::Db(16.74)));
        let mut rtl = SdrMetrics::fixture().rtlsdr();
        rtl.radio.amp_enabled = true;
        assert_eq!(rtl.stage_reading(0), Some(StageReading::Auto));
        // A HackRF's amp is a switch, not an AGC: its stages keep their dB.
        let mut hackrf = SdrMetrics::fixture();
        hackrf.radio.amp_enabled = true;
        assert!(matches!(hackrf.stage_reading(0), Some(StageReading::Db(_))));
    }

    /// **The peak is judged against the radio's own ceiling**: on an R2 its
    /// tuner clips at -3 dBFS, so a peak there is at the limit; a HackRF's
    /// ceiling is the converter's 0 dBFS.
    /// **The HF+'s ceiling follows the band**: its digital AGC holds a peak
    /// from -13.5 dBFS on HF and from -14.8 on VHF, judged from the lower
    /// level, written as the span, and says it holds.
    #[test]
    fn the_hf_plus_ceiling_follows_the_band() {
        let mut m = SdrMetrics::fixture().airspyhf();
        m.signal.adc_peak_dbfs = -13.5;
        assert_eq!(m.ceiling_dbfs(), -13.5);
        assert!(m.peak_to_ceiling_db().abs() < 1e-5);
        assert_eq!(m.ceiling_text(), "-14 to -12");
        m.radio.frequency = 100_000_000;
        assert_eq!(m.ceiling_dbfs(), -14.8);
        assert!((m.peak_to_ceiling_db() - 1.3).abs() < 1e-5);
        assert_eq!(m.ceiling_text(), "-15 to -13");
        assert_eq!(
            m.ceiling().map(|c| c.kind),
            Some(crate::hardware::CeilingKind::Holds)
        );
    }

    #[test]
    fn the_peak_is_judged_against_the_radios_ceiling() {
        let mut r2 = SdrMetrics::fixture().band_measured_chain();
        r2.signal.adc_peak_dbfs = -3.0;
        assert_eq!(r2.ceiling_dbfs(), -3.0);
        assert_eq!(r2.peak_to_ceiling_db(), 0.0);
        let mut hackrf = SdrMetrics::fixture();
        hackrf.signal.adc_peak_dbfs = -3.0;
        assert_eq!(
            (hackrf.ceiling_dbfs(), hackrf.peak_to_ceiling_db()),
            (0.0, -3.0)
        );
    }

    /// Every other device reads as it always did.
    #[test]
    fn a_chain_with_no_bands_is_a_gain_everywhere() {
        let mut m = SdrMetrics::fixture();
        m.radio.frequency = 5_800_000_000;
        assert!(m.chain_db_holds());
        assert!(matches!(m.stage_reading(0), Some(StageReading::Db(_))));
    }
}
