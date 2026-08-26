//! Device abstraction: the [`SdrDevice`] trait plus the capability and metadata
//! types that let HackRF, RTL-SDR, and future backends share one RX → FFT
//! pipeline, one UI, and one input handler. Concrete backends live in the
//! `hackrf` / `rtlsdr` submodules; everything device-generic keys off the
//! [`DeviceCapabilities`] descriptor rather than matching on the device type.

use std::sync::{Arc, Mutex};

use crate::state::SdrMetrics;

/// How raw USB bytes encode each I/Q component.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleFormat {
    /// Interleaved signed 8-bit (HackRF). Decode `b as i8 as f32 / 128.0`.
    Int8,
    /// Interleaved unsigned 8-bit, DC bias 127.5 (RTL-SDR).
    /// Decode `(b as f32 - 127.5) / 127.5`.
    Uint8,
}

/// The gain "shape" a device exposes — drives UI rendering and key bindings.
#[derive(Clone, Debug)]
pub enum GainModel {
    /// HackRF: RF amp (0 / +14 dB) → LNA (0..=40 step 8) → VGA (0..=62 step 2).
    HackRf,
    /// RTL-SDR: a single tuner gain restricted to a discrete table (whole dB),
    /// plus a tuner-AGC toggle.
    RtlSingle { gain_steps_db: Vec<u32> },
}

impl GainModel {
    /// True for a single-tuner device (RTL-SDR) — no separate VGA stage.
    pub fn is_single(&self) -> bool {
        matches!(self, GainModel::RtlSingle { .. })
    }

    /// Label for the primary front-end gain stage.
    pub fn primary_label(&self) -> &'static str {
        match self {
            GainModel::HackRf => "LNA",
            GainModel::RtlSingle { .. } => "Tuner",
        }
    }

    /// Full-scale value for the primary-gain bar/gauge (dB).
    pub fn primary_max_db(&self) -> u32 {
        match self {
            GainModel::HackRf => 40,
            GainModel::RtlSingle { gain_steps_db, .. } => {
                gain_steps_db.last().copied().unwrap_or(49)
            }
        }
    }

    /// Whether a distinct second gain stage (HackRF's VGA) exists.
    pub fn has_second_stage(&self) -> bool {
        matches!(self, GainModel::HackRf)
    }

    /// Label for the front-end-boost toggle (`amp_enabled`): HackRF's RF amp vs
    /// RTL-SDR's tuner AGC.
    pub fn boost_label(&self) -> &'static str {
        match self {
            GainModel::HackRf => "AMP",
            GainModel::RtlSingle { .. } => "AGC",
        }
    }

    /// Snap stored gains into this model's legal values, returning `(lna, vga)`.
    /// A config saved on one device family must not apply or display an illegal
    /// gain on another — e.g. an RTL-SDR tuner's 49 dB on a HackRF LNA that maxes
    /// at 40, or a HackRF value shown unsnapped on an RTL tuner's discrete table.
    /// HackRF snaps to its 8 dB LNA / 2 dB VGA steps; a single-tuner device snaps
    /// the primary gain to the nearest table entry and leaves `vga` untouched.
    pub fn clamp_gains(&self, lna: u32, vga: u32) -> (u32, u32) {
        match self {
            GainModel::HackRf => (
                (lna.min(40) + 4) / 8 * 8, // nearest 8 dB step within 0..=40
                (vga.min(62) + 1) / 2 * 2, // nearest 2 dB step within 0..=62
            ),
            GainModel::RtlSingle { gain_steps_db } => {
                let snapped = gain_steps_db
                    .iter()
                    .copied()
                    .min_by_key(|&g| (g as i64 - lna as i64).abs())
                    .unwrap_or(lna);
                (snapped, vga)
            }
        }
    }
}

/// Static description of a device's limits and features — the single source of
/// truth for every clamp, default, and UI capability check. Built once at open.
#[derive(Clone, Debug)]
pub struct DeviceCapabilities {
    pub freq_min_hz: u64,
    pub freq_max_hz: u64,
    pub sample_rate_min_hz: f64,
    pub sample_rate_max_hz: f64,
    /// Startup freq/rate guaranteed legal for THIS device. Used as the fallback
    /// when a loaded config value is out of range (e.g. a HackRF config opened
    /// on an RTL-SDR), so the radio never boots to an illegal setting.
    pub default_frequency_hz: u64,
    pub default_sample_rate_hz: f64,
    pub sample_format: SampleFormat,
    pub gain: GainModel,
    /// IQ pairs per USB transfer — feeds the expected callback-period math in
    /// [`crate::state::TimingState`].
    pub samples_per_transfer: u64,
    /// Programmable baseband filter (HackRF yes, RTL-SDR no). Part of the device
    /// capability contract and asserted in the device tests; the live panels key off
    /// `bb_filter_hz` (0 ⇒ unknown) directly, so the binary never reads this flag.
    #[allow(dead_code)]
    pub has_bb_filter: bool,
    /// The Friis cascade NF / MDS panel applies (HackRF's known 3-stage chain).
    pub friis_applicable: bool,
}

/// Identity / metadata shown in the header, telemetry, and RF-chain panels.
/// Fields a given device can't report are `None`.
#[derive(Clone, Debug, Default)]
pub struct DeviceInfo {
    pub board_name: String,
    pub serial: String,
    pub fw_version: Option<String>,
    pub board_rev: Option<u8>,
    pub usb_api_version: Option<u16>,
    pub tuner_name: Option<String>,
}

/// Shared RX plumbing handed to a backend's streaming start. The per-sample
/// accumulators write into `metrics`; raw byte blocks go out via `sample_tx` to
/// the FFT worker. `format` tells [`crate::hardware::process::process_block`]
/// how to decode the bytes.
pub struct RxContext {
    pub metrics: Arc<Mutex<SdrMetrics>>,
    pub sample_tx: crossbeam_channel::Sender<Vec<u8>>,
    /// Second, independently lossy feed to the demod worker. Blocks are forwarded
    /// only while `demod.enabled` is set, so the extra copy is paid for solely on
    /// the bench that uses it.
    ///
    /// Carries a monotonic sequence number alongside the bytes: this channel drops
    /// under load like the FFT one, and the CTCSS detector needs to know whether
    /// its half-second of audio is one unbroken run or two pieces either side of a
    /// gap. The FFT feed needs no such thing, which is why only this channel pays
    /// for it.
    pub demod_tx: crossbeam_channel::Sender<(u64, Vec<u8>)>,
    pub format: SampleFormat,
}

/// A tuned SDR receiver. Object-safe so it can be stored as `Arc<dyn SdrDevice>`
/// and shared across the input handler, the RX task, and the sweep task.
pub trait SdrDevice: Send + Sync {
    fn capabilities(&self) -> &DeviceCapabilities;
    fn info(&self) -> DeviceInfo;

    /// Begin streaming. The backend keeps `ctx` alive for the session and
    /// delivers sample blocks to it (HackRF via a lib-owned callback thread,
    /// RTL-SDR via an owned read thread).
    fn start_rx(&self, ctx: Arc<RxContext>) -> anyhow::Result<()>;
    fn stop_rx(&self) -> anyhow::Result<()>;
    fn is_streaming(&self) -> bool;

    fn set_frequency(&self, hz: u64) -> anyhow::Result<()>;
    /// Returns the baseband-filter bandwidth applied (Hz), or 0 when the device
    /// has none.
    fn set_sample_rate(&self, hz: f64) -> anyhow::Result<u32>;

    /// Primary front-end gain — HackRF's LNA, RTL-SDR's tuner gain. The other
    /// stages default to no-ops so call sites stay unconditional; capability
    /// flags decide what to render and bind.
    fn set_lna_gain(&self, db: u32) -> anyhow::Result<()>;
    fn set_vga_gain(&self, _db: u32) -> anyhow::Result<()> {
        Ok(())
    }
    fn set_amp_enable(&self, _on: bool) -> anyhow::Result<()> {
        Ok(())
    }
    fn set_tuner_agc(&self, _on: bool) -> anyhow::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hackrf_clamp_snaps_to_steps_and_caps() {
        let g = GainModel::HackRf;
        // In-range values already on a step are unchanged.
        assert_eq!(g.clamp_gains(16, 30), (16, 30));
        // An RTL tuner's 49 dB can't reach a HackRF LNA — caps to 40, a legal step.
        assert_eq!(g.clamp_gains(49, 100), (40, 62));
        // Off-step values snap to the nearest 8 dB / 2 dB step.
        assert_eq!(g.clamp_gains(20, 31), (24, 32));
        assert_eq!(g.clamp_gains(0, 0), (0, 0));
    }

    #[test]
    fn rtl_clamp_snaps_primary_to_table_keeps_vga() {
        let g = GainModel::RtlSingle {
            gain_steps_db: vec![0, 9, 16, 24, 49],
        };
        // A HackRF LNA value snaps to the nearest tuner-table entry; vga is inert.
        assert_eq!(g.clamp_gains(20, 40), (16, 40));
        assert_eq!(g.clamp_gains(100, 0), (49, 0));
        // An empty table can't snap → the value passes through unchanged.
        let empty = GainModel::RtlSingle {
            gain_steps_db: vec![],
        };
        assert_eq!(empty.clamp_gains(33, 7), (33, 7));
    }
}
