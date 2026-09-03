// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

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
    /// Interleaved signed 16-bit little endian, two bytes per component, which
    /// is what SoapySDR calls `CS16`. The container is 16 bits wide; how many of
    /// them the converter actually fills is
    /// [`SampleGeometry::bits`], from the full scale the driver reports.
    ///
    Int16,
}

impl Default for SampleFormat {
    /// Only so [`SampleGeometry`] can derive `Default` for the RX accumulators.
    /// Nothing chooses a format this way: every device states its own.
    fn default() -> Self {
        SampleFormat::Int8
    }
}

/// How a device's raw bytes decode, and what "full scale" means in them.
///
/// The two questions travel together because every caller needs both: reading a
/// sample and knowing how loud it is allowed to get are the same question asked
/// twice. Keeping them apart is how a decoder ends up correct and a histogram
/// ends up drawn against the wrong ceiling.
///
/// Both shipped radios are 8-bit, so `full_scale` is 128.0 for each. That stays
/// true even for the RTL-SDR, whose true DC bias is 127.5: see
/// [`super::process`] on why centering by 128 is deliberate.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct SampleGeometry {
    pub format: SampleFormat,
    /// The sample count that means 0 dBFS.
    pub full_scale: f32,
}

impl SampleGeometry {
    /// Wire bytes per I/Q pair.
    ///
    /// Two for every 8-bit format, one byte each for I and Q. Anything wider
    /// costs more, and the USB link ceiling and the block-to-sample arithmetic
    /// both need to know: a 16-bit stream at the same sample rate is twice the
    /// traffic, and calling it the same would report a device as comfortably
    /// inside a budget it is actually at the edge of.
    pub fn bytes_per_pair(&self) -> usize {
        match self.format {
            SampleFormat::Int8 | SampleFormat::Uint8 => 2,
            SampleFormat::Int16 => 4,
        }
    }

    /// Significant bits in the converter.
    ///
    /// **Derived, not stored.** A converter's depth and its full scale are one
    /// fact written two ways, and two fields would eventually disagree. This is
    /// also the honest answer for a 12-bit ADC delivering 16-bit containers: a
    /// driver reporting a full scale of 2048 is telling us it has 12 bits, and
    /// the ADC bench would be wrong to call it 16.
    ///
    /// **Capped by the container**, because a driver can report a full scale
    /// that does not fit in the format it is sending. SoapySDR's `audio` module
    /// reports `CS16 [full-scale=65536]`, which derives to 17 bits, and there is
    /// no such thing as a 17-bit sample in a 16-bit container. Found by probing
    /// a real driver, not by imagining one.
    pub fn bits(&self) -> u8 {
        let derived = (self.full_scale.max(1.0).log2().round() as i32 + 1).clamp(1, 32) as u8;
        derived.min(self.container_bits())
    }

    /// How many bits the wire format can carry per component, whatever the
    /// converter behind it actually fills.
    fn container_bits(&self) -> u8 {
        match self.format {
            SampleFormat::Int8 | SampleFormat::Uint8 => 8,
            SampleFormat::Int16 => 16,
        }
    }
}

/// One amplification element: what it is called, and what values it accepts.
///
/// The shape half of the gain model. `caps` owns a list of these, built once at
/// open; the state owns the values in the same order. Nothing here knows what
/// any stage is currently set to.
///
/// **`f64`, not `u32`.** `SoapySDRRange` places no constraint on either bound,
/// so an element can have a fractional step or a negative minimum, the latter
/// describing an attenuator rather than an amplifier. The previous model rounded
/// both away and would have answered 0 dB for a stage sitting at -10.
#[derive(Clone, Debug, PartialEq)]
#[allow(dead_code)] // read from G3
pub struct StageSpec {
    /// The driver's own name for it: `LNA`, `IFGR`, `TUNER`, whatever it said.
    /// Never one of ours, because a name we chose would be a claim about a chain
    /// we have not seen.
    pub name: String,
    pub min_db: f64,
    pub max_db: f64,
    /// Spacing of the legal values, or zero for a continuous range.
    pub step_db: f64,
}

#[allow(dead_code)] // read from G3
impl StageSpec {
    /// Whether the driver said anything usable about this element.
    ///
    /// A maximum below the minimum, or a bound that is not a finite number, is
    /// not a range. Such an element is dropped with a named log line rather than
    /// silently kept, because a stage pinned at zero looks exactly like a stage
    /// the user turned down.
    pub fn is_usable(&self) -> bool {
        self.min_db.is_finite() && self.max_db.is_finite() && self.max_db >= self.min_db
    }

    /// How many settings this element has, or `None` when it is continuous.
    ///
    /// **This is what makes a switch detectable rather than assumed.** A HackRF
    /// through SoapyHackRF reports `AMP [0, 14, 14]`: the step spans the whole
    /// range, so there are exactly two settings and it is a boost, not a stage
    /// to distribute a figure across. That is the driver's own answer, not a
    /// table of what a HackRF has.
    pub fn positions(&self) -> Option<u32> {
        if !self.is_usable() || self.step_db <= 0.0 {
            return None;
        }
        let n = ((self.max_db - self.min_db) / self.step_db).floor();
        Some((n.max(0.0) as u32).saturating_add(1))
    }

    /// Two settings and nothing between: a boost, which sdrtop already has a
    /// concept and a key for.
    pub fn is_switch(&self) -> bool {
        self.positions() == Some(2)
    }

    /// One setting: the driver exposes the element but it cannot be moved.
    /// Not a fault, and not something to offer the user a control for.
    pub fn is_fixed(&self) -> bool {
        self.positions() == Some(1)
    }

    /// The nearest legal value to `db`.
    ///
    /// Total by construction: a driver that reports nonsense gets the one value
    /// it certainly accepts rather than a panic. `f64::clamp` panics on an
    /// inverted range or a NaN bound, and a value read over FFI is exactly where
    /// those come from.
    pub fn snap(&self, db: f64) -> f64 {
        if !self.is_usable() {
            return if self.min_db.is_finite() {
                self.min_db
            } else {
                0.0
            };
        }
        if !db.is_finite() {
            return self.min_db;
        }
        let v = db.clamp(self.min_db, self.max_db);
        if self.step_db <= 0.0 {
            return v; // continuous, or a step the driver declined to give
        }
        let snapped = self.min_db + ((v - self.min_db) / self.step_db).round() * self.step_db;
        // Rounding up can leave the grid: the top of the range is not always on
        // it. `[0, 10, 3]` allows 9, never 12.
        if snapped > self.max_db {
            (snapped - self.step_db).max(self.min_db)
        } else {
            snapped
        }
    }
}

/// The gain "shape" a device exposes - drives UI rendering and key bindings.
#[derive(Clone, Debug)]
pub enum GainModel {
    /// HackRF: RF amp (0 / +14 dB) → LNA (0..=40 step 8) → VGA (0..=62 step 2).
    HackRf,
    /// RTL-SDR: a single tuner gain restricted to a discrete table (whole dB),
    /// plus a tuner-AGC toggle.
    RtlSingle { gain_steps_db: Vec<u32> },
    /// A SoapySDR device: one overall gain across the driver's own range, and
    /// the element names for display only.
    ///
    /// Mapping named elements onto sdrtop's LNA / VGA / AMP is device-specific,
    /// unverifiable without the device, and a wrong guess silently drives the
    /// wrong stage. So 0.4.5 does not guess. See `dev_docs/soapy-design.md`,
    /// decision 3.
    ///
    /// `agc` is `hasGainMode`, and it is genuinely false on real hardware: a
    /// HackRF through SoapyHackRF reports `Supports AGC: NO`, so the boost key
    /// has nothing to toggle there.
    ///
    /// `elements` is display only for now: naming which stage is which is the
    /// follow-up this release deliberately does not guess at.
    Soapy {
        min_db: u32,
        max_db: u32,
        #[cfg_attr(not(test), allow(dead_code))]
        elements: Vec<String>,
        agc: bool,
    },
}

impl GainModel {
    /// True for a device with one gain control and no separate VGA stage.
    pub fn is_single(&self) -> bool {
        matches!(self, GainModel::RtlSingle { .. } | GainModel::Soapy { .. })
    }

    /// Label for the primary front-end gain stage.
    pub fn primary_label(&self) -> &'static str {
        match self {
            GainModel::HackRf => "LNA",
            GainModel::RtlSingle { .. } => "Tuner",
            // Soapy distributes one number across whatever elements the device
            // has, so there is no one stage to name. "RF" is the overall front
            // end gain, and it reads correctly in the two places this appears:
            // as a bar label on its own, and as "RF gain" in a heading. "Gain"
            // read as "GAIN gain" there.
            GainModel::Soapy { .. } => "RF",
        }
    }

    /// Full-scale value for the primary-gain bar/gauge (dB).
    pub fn primary_max_db(&self) -> u32 {
        match self {
            GainModel::HackRf => 40,
            GainModel::RtlSingle { gain_steps_db, .. } => {
                gain_steps_db.last().copied().unwrap_or(49)
            }
            GainModel::Soapy { max_db, .. } => *max_db,
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
            GainModel::RtlSingle { .. } | GainModel::Soapy { .. } => "AGC",
        }
    }

    /// The stages between antenna and converter, for the panels that draw a
    /// chain they cannot model.
    ///
    /// Only read where `friis_applicable` is false. An RTL-SDR really is one
    /// tuner, and a SoapySDR device is **whatever `listGains` named**, which is
    /// the driver's own answer rather than one of ours. A driver that names no
    /// elements gets a question mark, because that is the honest character for
    /// "it did not say".
    pub fn unmodelled_stages(&self) -> String {
        match self {
            GainModel::HackRf => "LNA\u{25b8}MIX\u{25b8}VGA".to_string(),
            GainModel::RtlSingle { .. } => "TUNER".to_string(),
            GainModel::Soapy { elements, .. } if !elements.is_empty() => elements.join("\u{25b8}"),
            GainModel::Soapy { .. } => "?".to_string(),
        }
    }

    /// Why the modelled cascade is not on offer, in a few words.
    ///
    /// Two devices land in that branch for **different reasons**, and the panels
    /// used to print one sentence for both: "single-tuner". That is true of an
    /// RTL-SDR and false of a HackRF reached through SoapySDR, which has three
    /// gain elements and a cascade we simply have not been told the noise
    /// figures for.
    pub fn no_cascade_reason(&self) -> &'static str {
        match self {
            // Never shown: this device has a cascade.
            GainModel::HackRf => "no cascade",
            GainModel::RtlSingle { .. } => "single tuner, no cascade",
            GainModel::Soapy { .. } => "chain not modelled",
        }
    }

    /// Whether there is a front-end boost to toggle at all.
    ///
    /// Both native radios have one, so this was previously not a question worth
    /// asking. A SoapySDR device often has neither an RF amp nor an automatic
    /// gain mode, and a key that toggles a flag meaning nothing is worse than a
    /// key that is not offered.
    pub fn has_boost(&self) -> bool {
        match self {
            GainModel::HackRf | GainModel::RtlSingle { .. } => true,
            GainModel::Soapy { agc, .. } => *agc,
        }
    }

    /// Snap stored gains into this model's legal values, returning `(lna, vga)`.
    /// A config saved on one device family must not apply or display an illegal
    /// gain on another - e.g. an RTL-SDR tuner's 49 dB on a HackRF LNA that maxes
    /// at 40, or a HackRF value shown unsnapped on an RTL tuner's discrete table.
    /// HackRF snaps to its 8 dB LNA / 2 dB VGA steps; a single-tuner device snaps
    /// the primary gain to the nearest table entry and leaves `vga` untouched.
    pub fn clamp_gains(&self, lna: u32, vga: u32) -> (u32, u32) {
        match self {
            GainModel::HackRf => (
                (lna.min(40) + 4) / 8 * 8,   // nearest 8 dB step within 0..=40
                vga.min(62).div_ceil(2) * 2, // nearest 2 dB step within 0..=62
            ),
            GainModel::RtlSingle { gain_steps_db } => {
                let snapped = gain_steps_db
                    .iter()
                    .copied()
                    .min_by_key(|&g| (g as i64 - lna as i64).abs())
                    .unwrap_or(lna);
                (snapped, vga)
            }
            // A continuous range, so clamping is the whole job. `vga` is left
            // alone because there is no second stage to put it in, and a config
            // carried over from a HackRF should not be silently rewritten.
            GainModel::Soapy { min_db, max_db, .. } => {
                (lna.clamp(*min_db, (*max_db).max(*min_db)), vga)
            }
        }
    }
}

/// How samples reach us, which decides what "on time" can even mean.
///
/// Not a property of the radio. A property of the **transport**, which is why it
/// lives here beside the other capability answers rather than in a `DeviceKind`
/// match: two backends could reach the same radio by different routes, and the
/// timing bench has to follow the route rather than the hardware.
///
/// The distinction is not cosmetic. It was found by measuring: on a HackRF
/// reached through `SoapyHackRF`, the timing bench reported permanent USB
/// distress on a link with zero drops, a correct sample rate and 50 per cent of
/// the bus spare. The gaps between reads were bimodal, median 1079 µs against an
/// expected 1638, p95 5933 and p99 7763. Nothing was wrong. A pull loop drains
/// whatever the driver has buffered and then blocks, so the interval between
/// reads is our own rhythm and not the link's, and a deadline measured over it
/// grades the wrong thing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryModel {
    /// The driver calls us and paces the calls. HackRF's `hackrf_start_rx` and
    /// RTL-SDR's `rtlsdr_read_async` both take a callback and drive it, so the
    /// interval between callbacks really is the link's cadence and the deadline
    /// budget means something.
    Push,
    /// We ask, and the call returns as soon as the driver has anything. A
    /// SoapySDR `readStream` loop. Health here is throughput against the
    /// configured rate, drops, and how much of the loop is spent waiting rather
    /// than working.
    Pull,
}

/// Static description of a device's limits and features - the single source of
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
    pub sample_geometry: SampleGeometry,
    pub gain: GainModel,
    /// IQ pairs per USB transfer - feeds the expected callback-period math in
    /// [`crate::state::TimingState`].
    pub samples_per_transfer: u64,
    /// Programmable baseband filter (HackRF yes, RTL-SDR no). Part of the device
    /// capability contract and asserted in the device tests; the live panels key off
    /// `bb_filter_hz` (0 ⇒ unknown) directly, so the binary never reads this flag.
    #[allow(dead_code)]
    pub has_bb_filter: bool,
    /// The Friis cascade NF / MDS panel applies (HackRF's known 3-stage chain).
    pub friis_applicable: bool,
    /// Whether the driver pushes samples at us or we pull them.
    ///
    /// Read by the capability tests only until T5, where the timing bench starts
    /// asking it. Declared now rather than then so that every backend has to
    /// answer the question as part of describing itself, including any added in
    /// between.
    #[cfg_attr(not(test), allow(dead_code))]
    pub delivery: DeliveryModel,
}

/// The software layer between sdrtop and a radio that has no firmware of its
/// own, for the header's firmware field.
///
/// `label` is padded to ten columns by the backend that sets it, because the
/// header's top band gap is computed from a fixed field width.
#[derive(Clone, Debug)]
pub struct SoftwareStack {
    pub label: &'static str,
    pub value: std::sync::Arc<str>,
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
    /// What to show where a HackRF shows its firmware version.
    ///
    /// `None` on a device with firmware of its own. The header used to work this
    /// out by asking whether the gain model was single-knob, which was a fine
    /// proxy while there were two backends and started calling a SoapySDR device
    /// an RTL-SDR the moment there were three. A backend says what it is.
    pub stack: Option<SoftwareStack>,
}

/// Shared RX plumbing handed to a backend's streaming start. The per-sample
/// accumulators write into `metrics`; raw byte blocks go out via `sample_tx` to
/// the FFT worker. `geometry` tells [`crate::hardware::process::process_block`]
/// how to decode the bytes and what full scale is worth in them.
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
    pub geometry: SampleGeometry,
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

    /// Primary front-end gain - HackRF's LNA, RTL-SDR's tuner gain. The other
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

    /// Cumulative microseconds the read loop has spent `(waiting, working)`,
    /// for a [`DeliveryModel::Pull`] backend.
    ///
    /// `None` is the honest answer for a push backend rather than a pair of
    /// zeroes: there is no read loop there, so there is nothing to divide. The
    /// same shape as the optional gain stages above, and for the same reason:
    /// a device that cannot answer declines rather than inventing a number.
    ///
    /// Cumulative, so the caller takes differences. Never reset, so a stream
    /// that stops and restarts does not step the counters backwards.
    fn read_loop_us(&self) -> Option<(u64, u64)> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── StageSpec ───────────────────────────────────────────────────────────
    //
    // The three fixtures are the real answers `SoapySDRUtil --probe` gives for a
    // HackRF through SoapyHackRF, transcribed rather than invented:
    //
    //     LNA gain range: [0, 40, 8] dB
    //     AMP gain range: [0, 14, 14] dB
    //     VGA gain range: [0, 62, 2] dB

    fn stage(name: &str, min: f64, max: f64, step: f64) -> StageSpec {
        StageSpec {
            name: name.into(),
            min_db: min,
            max_db: max,
            step_db: step,
        }
    }

    #[test]
    fn snapping_lands_on_the_drivers_own_grid() {
        let lna = stage("LNA", 0.0, 40.0, 8.0);
        assert_eq!(lna.snap(0.0), 0.0);
        assert_eq!(lna.snap(11.0), 8.0, "nearest, not nearest-below");
        assert_eq!(lna.snap(13.0), 16.0);
        assert_eq!(lna.snap(40.0), 40.0);

        let vga = stage("VGA", 0.0, 62.0, 2.0);
        assert_eq!(vga.snap(47.0), 48.0);
        assert_eq!(vga.snap(47.9), 48.0);
    }

    #[test]
    fn snapping_clamps_to_the_range() {
        let lna = stage("LNA", 0.0, 40.0, 8.0);
        assert_eq!(lna.snap(1000.0), 40.0);
        assert_eq!(lna.snap(-1000.0), 0.0);
    }

    /// The top of a range is not always on the grid, and rounding up must not
    /// leave it. `[0, 10, 3]` allows 9 and never 12.
    #[test]
    fn snapping_never_steps_above_the_maximum() {
        let odd = stage("ODD", 0.0, 10.0, 3.0);
        assert_eq!(odd.snap(10.0), 9.0);
        assert_eq!(odd.snap(11.0), 9.0);
        assert_eq!(odd.positions(), Some(4), "0, 3, 6, 9");
    }

    /// An attenuator. The old `u32` model rounded this away and would have
    /// reported a stage sitting at -10 dB as 0.
    #[test]
    fn a_negative_minimum_is_a_real_range() {
        let att = stage("ATT", -30.0, 0.0, 10.0);
        assert!(att.is_usable());
        assert_eq!(att.snap(-12.0), -10.0);
        assert_eq!(att.snap(-100.0), -30.0);
        assert_eq!(att.snap(5.0), 0.0);
        assert_eq!(att.positions(), Some(4));
    }

    /// A step of zero is the driver declining to quantise, not a zero-width
    /// grid to divide by.
    #[test]
    fn a_continuous_range_is_not_snapped() {
        let cont = stage("RF", 0.0, 50.0, 0.0);
        assert_eq!(cont.snap(17.3), 17.3);
        assert_eq!(cont.snap(60.0), 50.0);
        assert_eq!(cont.positions(), None);
        assert!(!cont.is_switch());
    }

    /// The G1 finding, and the reason `positions` exists: an element whose step
    /// spans its range has two settings and is a boost, not a stage.
    #[test]
    fn an_element_whose_step_spans_its_range_is_a_switch() {
        let amp = stage("AMP", 0.0, 14.0, 14.0);
        assert_eq!(amp.positions(), Some(2));
        assert!(amp.is_switch());
        assert_eq!(amp.snap(3.0), 0.0);
        assert_eq!(amp.snap(8.0), 14.0);

        // And the two real neighbours from the same probe are not switches.
        assert!(!stage("LNA", 0.0, 40.0, 8.0).is_switch());
        assert!(!stage("VGA", 0.0, 62.0, 2.0).is_switch());
    }

    /// A step wider than the range leaves one setting. Not a fault, but not a
    /// control to offer either.
    #[test]
    fn a_step_wider_than_the_range_leaves_one_setting() {
        let stuck = stage("STUCK", 0.0, 5.0, 20.0);
        assert_eq!(stuck.positions(), Some(1));
        assert!(stuck.is_fixed());
        assert!(!stuck.is_switch());
        assert_eq!(stuck.snap(5.0), 0.0);
    }

    /// What a broken driver can hand back over FFI. `f64::clamp` panics on an
    /// inverted range or a NaN bound, so every one of these has to be answered
    /// rather than passed through.
    #[test]
    fn nonsense_from_a_driver_is_refused_without_panicking() {
        let inverted = stage("BAD", 40.0, 0.0, 8.0);
        assert!(!inverted.is_usable());
        assert_eq!(inverted.snap(20.0), 40.0, "the one value it surely accepts");
        assert_eq!(inverted.positions(), None);

        let nan_max = stage("NAN", 0.0, f64::NAN, 1.0);
        assert!(!nan_max.is_usable());
        assert_eq!(nan_max.snap(5.0), 0.0);

        let nan_min = stage("NAN", f64::NAN, 10.0, 1.0);
        assert!(!nan_min.is_usable());
        assert_eq!(nan_min.snap(5.0), 0.0);

        // And a NaN asked for, against a range that is fine.
        assert_eq!(stage("LNA", 0.0, 40.0, 8.0).snap(f64::NAN), 0.0);
    }

    /// The depth follows the full scale, which is the number a SoapySDR driver
    /// actually reports. The 2048 case is the one that matters: a 12-bit ADC
    /// handing over 16-bit containers is 12 bits, and saying 16 would put a
    /// wrong ENOB on the RF bench for every such radio.
    #[test]
    fn bit_depth_follows_full_scale() {
        let wide = |fs| SampleGeometry {
            format: SampleFormat::Int16,
            full_scale: fs,
        };
        assert_eq!(
            SampleGeometry {
                format: SampleFormat::Int8,
                full_scale: 128.0,
            }
            .bits(),
            8,
            "both shipped radios"
        );
        assert_eq!(
            wide(2048.0).bits(),
            12,
            "a 12-bit ADC in a 16-bit container"
        );
        assert_eq!(wide(8192.0).bits(), 14);
        assert_eq!(wide(32768.0).bits(), 16);
    }

    /// Two real drivers, probed on this machine, that would each have been got
    /// wrong by a rule that only looked at one of format or full scale.
    #[test]
    fn the_container_caps_the_depth() {
        // SoapySDR's audio module: `CS16 [full-scale=65536]`. Deriving from the
        // full scale alone gives 17, and a 16-bit container cannot hold 17 bits.
        assert_eq!(
            SampleGeometry {
                format: SampleFormat::Int16,
                full_scale: 65536.0,
            }
            .bits(),
            16
        );
        // SoapyHackRF: `CS8 [full-scale=128]`, which is 8 either way.
        assert_eq!(
            SampleGeometry {
                format: SampleFormat::Int8,
                full_scale: 128.0,
            }
            .bits(),
            8
        );
        // And a driver claiming an absurd scale in a narrow container is still
        // capped rather than believed.
        assert_eq!(
            SampleGeometry {
                format: SampleFormat::Int8,
                full_scale: 1_000_000.0,
            }
            .bits(),
            8
        );
    }

    /// A driver is free to report nonsense. It must not produce a depth of zero,
    /// which would divide the ADC bench by zero, or a negative one, which would
    /// panic on the cast.
    #[test]
    fn a_nonsense_full_scale_still_gives_a_usable_depth() {
        for fs in [0.0, -1.0, 0.5, f32::NAN] {
            let b = SampleGeometry {
                format: SampleFormat::Int8,
                full_scale: fs,
            }
            .bits();
            assert!((1..=32).contains(&b), "full_scale {fs} gave {b} bits");
        }
    }

    #[test]
    fn hackrf_clamp_snaps_to_steps_and_caps() {
        let g = GainModel::HackRf;
        // In-range values already on a step are unchanged.
        assert_eq!(g.clamp_gains(16, 30), (16, 30));
        // An RTL tuner's 49 dB can't reach a HackRF LNA - caps to 40, a legal step.
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
