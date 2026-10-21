// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

mod acc;
mod demod;
#[cfg(test)]
pub(crate) mod fixture;
mod iq;
mod lab;
mod micro;
mod net;
mod observer;
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
#[cfg(test)]
pub use net::WifiSurvey;
pub use net::{AirColumn, Rhythm, Spender, READING_COLUMNS, RHYTHM_BINS};
pub use net::{Capture, FrameZoom, Lost};
pub use net::{Family, LeastBusy, NetDecodeHealth, RowKind, Visit, WifiClock, WifiFunnel, WifiRow};
#[cfg(test)]
pub use net::{FramesView, Holdoff, Trigger};
pub use observer::ObserverState;
pub use radio::{
    offset_ppm, FrequencyReference, OffsetBasis, Provenance, RadioState, REFERENCE_STALE_S,
};
pub use record::{RecordProgress, RecordState};
pub use selection::Selection;
pub use signal::{
    acpr_offset_hz, classify, judge, Carrier, Modulation, RadioDropAccount, SignalState, Verdict,
    ACPR_FLOOR_DB, ADC_COMFORT_DBFS, CLASSIFY_MIN_SNR_DB, SAT_CLIP_PCT, SAT_CRIT_PCT, SAT_WARN_PCT,
};
pub use spectrum::{SpectrumMarker, SpectrumState, SpectrumStyle};
pub use sweep::{SweepConfig, SweepFrame, SweepState, SWEEP_SETTLING_MS};
pub use system::SystemState;
pub use timing::{TimingCause, TimingQuality, TimingState, HACKRF_SAMPLES_PER_TRANSFER};
pub use ui::{
    active_recall_slot, recall_from_hz, recall_to_hz, DeviceOptionUpdate, InputMode, LogEntry,
    LogLevel, MenuPane, MenuState, RailMode, UiState, RECALL_SLOTS,
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
        Some(spec.reading(self.radio.stage_gain(index), self.radio.frequency))
    }

    /// Whether every stage's setting is a dB at the tuned frequency, so that
    /// adding them up gives a gain.
    pub fn chain_db_holds(&self) -> bool {
        self.caps
            .gain
            .stages()
            .iter()
            .all(|s| s.db_holds_at(self.radio.frequency))
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

    /// Every other device reads as it always did.
    #[test]
    fn a_chain_with_no_bands_is_a_gain_everywhere() {
        let mut m = SdrMetrics::fixture();
        m.radio.frequency = 5_800_000_000;
        assert!(m.chain_db_holds());
        assert!(matches!(m.stage_reading(0), Some(StageReading::Db(_))));
    }
}
