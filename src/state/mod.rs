mod acc;
mod demod;
mod iq;
mod lab;
mod micro;
mod observer;
mod radio;
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
pub use lab::{LabState, RfFreeze};
pub use micro::MicroView;
pub use observer::ObserverState;
pub use radio::RadioState;
pub use signal::{
    acpr_offset_hz, classify, Modulation, SignalState, CLASSIFY_MIN_SNR_DB, SAT_CLIP_PCT,
};
pub use spectrum::{SpectrumMarker, SpectrumState, SpectrumStyle};
pub use sweep::{SweepConfig, SweepFrame, SweepState, SWEEP_SETTLING_MS};
pub use system::SystemState;
pub use timing::{TimingQuality, TimingState, HACKRF_SAMPLES_PER_TRANSFER};
pub use ui::{
    active_recall_slot, recall_from_hz, recall_to_hz, InputMode, LogEntry, LogLevel, RailMode,
    UiState, RECALL_SLOTS,
};
pub use waterfall::{FftFrame, WaterfallState};

pub const THROUGHPUT_HISTORY_LEN: usize = 64;
/// Depth of the per-callback gap ring feeding the `lab_timing` strip chart. ~256
/// keeps roughly the last 3 s at the HackRF/RTL callback cadence (~73–76 cb/s),
/// enough to fill the chart's ~160-column window with headroom.
pub const CB_GAP_HISTORY_LEN: usize = 256;
/// SNR/PWR/NF history depth — 120 samples at the 500 ms push cadence = 60 s window.
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
    /// Active device's capability descriptor — drives capability-aware UI
    /// rendering (gain model, BB filter / Friis applicability, ranges). Shared
    /// (Arc) so the per-frame `SdrMetrics` clone stays cheap.
    pub caps: std::sync::Arc<crate::hardware::DeviceCapabilities>,
    pub(crate) acc: Accumulators,
}

impl SdrMetrics {
    pub fn push_log(&mut self, msg: impl Into<String>) {
        self.ui.push_log(msg);
    }
}
