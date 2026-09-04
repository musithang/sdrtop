// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Device abstraction: the [`SdrDevice`] trait plus the capability and metadata
//! types that let HackRF, RTL-SDR, and future backends share one RX → FFT
//! pipeline, one UI, and one input handler. Concrete backends live in the
//! `native` and `soapy` submodules; everything device-generic keys off the
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
    /// The exact legal values, for a device that has a **table** rather than a
    /// grid. Empty means the bounds and step above describe it fully.
    ///
    /// An RTL-SDR tuner is the case: its gains are an irregular list the driver
    /// reads out of the device, not a span with a spacing. Rounding one to a
    /// nearest step would offer settings the tuner will refuse.
    pub table: Vec<f64>,
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
        !self.table.is_empty()
            || (self.min_db.is_finite() && self.max_db.is_finite() && self.max_db >= self.min_db)
    }

    /// A stage over a span, with a spacing. `step` of zero is continuous.
    pub fn ranged(name: &str, min_db: f64, max_db: f64, step_db: f64) -> Self {
        Self {
            name: name.to_string(),
            min_db,
            max_db,
            step_db,
            table: Vec::new(),
        }
    }

    /// A stage over an exact list of values, for a device that has one.
    pub fn tabled(name: &str, values: Vec<f64>) -> Self {
        let min_db = values.iter().copied().fold(f64::INFINITY, f64::min);
        let max_db = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        Self {
            name: name.to_string(),
            min_db,
            max_db,
            step_db: 0.0,
            table: values,
        }
    }

    /// How many settings this element has, or `None` when it is continuous.
    ///
    /// **This is what makes a switch detectable rather than assumed.** A HackRF
    /// through SoapyHackRF reports `AMP [0, 14, 14]`: the step spans the whole
    /// range, so there are exactly two settings and it is a boost, not a stage
    /// to distribute a figure across. That is the driver's own answer, not a
    /// table of what a HackRF has.
    pub fn positions(&self) -> Option<u32> {
        if !self.table.is_empty() {
            return Some(self.table.len() as u32);
        }
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

    /// The largest legal value **not above** `db`.
    ///
    /// The distribution needs this rather than [`Self::snap`]: rounding to the
    /// nearest can round up, and a stage that rounds up has spent gain the
    /// caller did not ask for. Flooring keeps the running total at or under the
    /// request, which is what makes the knob monotonic.
    pub fn snap_down(&self, db: f64) -> f64 {
        if !self.table.is_empty() {
            let want = if db.is_finite() {
                db
            } else {
                f64::NEG_INFINITY
            };
            // Largest entry at or below, or the smallest entry if none is.
            let mut best: Option<f64> = None;
            let mut lowest = f64::INFINITY;
            for &v in &self.table {
                lowest = lowest.min(v);
                if v <= want && best.is_none_or(|b| v > b) {
                    best = Some(v);
                }
            }
            return best.unwrap_or(lowest);
        }
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
            return v;
        }
        let steps = ((v - self.min_db) / self.step_db).floor();
        (self.min_db + steps * self.step_db).clamp(self.min_db, self.max_db)
    }

    /// The nearest legal value to `db`.
    ///
    /// Total by construction: a driver that reports nonsense gets the one value
    /// it certainly accepts rather than a panic. `f64::clamp` panics on an
    /// inverted range or a NaN bound, and a value read over FFI is exactly where
    /// those come from.
    pub fn snap(&self, db: f64) -> f64 {
        // A table is the exact set of values the device accepts, so it wins
        // over any grid the bounds might imply. Ties go to the first entry,
        // which is what the RTL-SDR path did before this existed.
        if !self.table.is_empty() {
            let want = if db.is_finite() {
                db
            } else {
                f64::NEG_INFINITY
            };
            return self
                .table
                .iter()
                .copied()
                .min_by(|a, b| {
                    (a - want)
                        .abs()
                        .partial_cmp(&(b - want).abs())
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .unwrap_or(0.0);
        }
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

/// How a device's front-end boost is reached, when it has one.
///
/// Two different mechanisms, and the distinction is the driver's, not ours. A
/// HackRF through `SoapyHackRF` reports `Supports AGC: NO` and yet has an `AMP`
/// element with exactly two positions, which is the same physical switch the
/// native backend drives. Treating "no gain mode" as "no boost" cost that device
/// its amp key.
///
/// **Not named after a backend.** Both mechanisms are ideas any driver could
/// present, and this was called `SoapyBoost` only because SoapySDR was the
/// first to need the distinction. All three backends build one now: a HackRF's
/// RF amp really is a two-position element, and an RTL-SDR's tuner AGC really
/// is a gain mode.
#[derive(Clone, Debug, PartialEq)]
pub enum Boost {
    /// An automatic gain mode, set as a flag. SoapySDR's `setGainMode`.
    GainMode,
    /// A two-position gain element, driven to one end or the other. SoapySDR's
    /// `setGainElement`.
    Element(StageSpec),
}

impl Boost {
    /// What to call this boost on screen, when the device named it.
    ///
    /// An element carries the driver's own word, which is the whole reason to
    /// prefer one: `SoapyHackRF` calls its switch `AMP` and so does the native
    /// backend, so the same radio reads the same either way round. A gain mode
    /// is a flag and has no name of its own.
    pub fn label(&self) -> Option<&str> {
        match self {
            Boost::GainMode => None,
            Boost::Element(s) => Some(&s.name),
        }
    }
}

/// The gain chain a device exposes: what stages it has, what to call them, and
/// which of the questions the UI asks it answers yes to.
///
/// **A description, not a taxonomy.** This was an enum with a variant per
/// backend, which put a fact about one radio's gain chain in the file the other
/// two share and answered every question about it with a match arm naming all
/// three. Each backend now builds one of these for itself, and the eleven
/// questions below are field reads. A driver that has to describe itself
/// differently changes the constructor in its own module and nothing else.
#[derive(Clone, Debug)]
pub struct GainModel {
    /// The adjustable stages, front to back, in the order the device presents
    /// them. The one source for the chain, as [`Self::stages`] always promised.
    stages: Vec<StageSpec>,
    /// The front-end boost, when there is one. **Not a stage**: it is a toggle
    /// with its own key, not a range to distribute a figure across.
    boost: Option<Boost>,
    primary_label: &'static str,
    primary_label_short: &'static str,
    /// Whether sdrtop offers a dedicated key for a second stage.
    ///
    /// **Declared, not counted.** A chain device can have three stages and
    /// still present one knob, because sdrtop distributes a single figure
    /// across them rather than giving each its own key.
    has_second_stage: bool,
    /// The signal path as a caption, for the panels that draw a chain they
    /// cannot model.
    ///
    /// Not built from `stages`, because it names parts that have no gain to
    /// set: a HackRF reads `LNA▸MIX▸VGA`, and the mixer is not a stage.
    chain_diagram: String,
    no_cascade_reason: &'static str,
    /// Full scale for the primary gain gauge when the stages describe no span.
    ///
    /// Reachable only for a device whose stages are absent or zero width: an
    /// RTL-SDR whose tuner named no gains at all, or a driver that reported a
    /// chain with no room in it. 49 dB is the historical RTL answer.
    gauge_fallback_db: u32,
}

impl GainModel {
    /// Describe a chain of stages, front to back.
    ///
    /// The defaults are the ones a device sdrtop knows nothing about should
    /// get: one knob, no boost, no modelled cascade, and a diagram built from
    /// the stage names. A backend that knows more says so with the `with_`
    /// methods below, which keeps each addition visible at the call site.
    pub fn new(
        stages: Vec<StageSpec>,
        primary_label: &'static str,
        primary_label_short: &'static str,
    ) -> Self {
        let chain_diagram = stages
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>()
            .join("\u{25b8}");
        Self {
            stages,
            boost: None,
            primary_label,
            primary_label_short,
            has_second_stage: false,
            // A driver that named nothing gets a question mark, because that is
            // the honest character for "it did not say".
            chain_diagram: if chain_diagram.is_empty() {
                "?".to_string()
            } else {
                chain_diagram
            },
            no_cascade_reason: "chain not modelled",
            gauge_fallback_db: 0,
        }
    }

    /// This device has a front-end boost, reached the given way.
    pub fn with_boost(mut self, boost: Boost) -> Self {
        self.boost = Some(boost);
        self
    }

    /// The second stage gets its own key, rather than a share of one figure.
    pub fn with_second_stage(mut self) -> Self {
        self.has_second_stage = true;
        self
    }

    /// The signal path in full, including parts that are not gain stages.
    pub fn with_chain_diagram(mut self, diagram: &str) -> Self {
        self.chain_diagram = diagram.to_string();
        self
    }

    /// Why the modelled Friis cascade is not on offer for this device.
    pub fn with_no_cascade_reason(mut self, reason: &'static str) -> Self {
        self.no_cascade_reason = reason;
        self
    }

    /// What the primary gauge reads full at when the stages describe no span.
    pub fn with_gauge_fallback(mut self, db: u32) -> Self {
        self.gauge_fallback_db = db;
        self
    }

    /// True for a device with one gain control and no separate second stage.
    ///
    /// Exactly the negation of [`Self::has_second_stage`]. Two names for one
    /// fact, kept because each reads correctly at its own call sites, but
    /// **one field**: they cannot drift apart.
    pub fn is_single(&self) -> bool {
        !self.has_second_stage
    }

    /// Label for the primary front-end gain stage.
    pub fn primary_label(&self) -> &'static str {
        self.primary_label
    }

    /// The primary stage's name in three columns, for the header's fixed field.
    ///
    /// A short form is a real need, not a duplicate: the header budgets four
    /// columns and `Tuner` does not fit. It lives beside the full name so the
    /// two cannot drift, which they did: the header hardcoded `TUN` for every
    /// single-knob device and so called a SoapySDR device's chain a tuner.
    pub fn primary_label_short(&self) -> &'static str {
        self.primary_label_short
    }

    /// Full-scale value for the primary-gain bar/gauge (dB).
    pub fn primary_max_db(&self) -> u32 {
        if self.has_second_stage {
            // The knob moves the front stage alone, so the gauge is that stage.
            if let Some(first) = self.stages.first() {
                return first.max_db.max(0.0).round() as u32;
            }
        } else {
            // One knob for the whole chain, so the gauge is everything the knob
            // can reach. The boost is not part of it and has its own key:
            // `getGainRange` says 116 dB on a HackRF because it counts the
            // 14 dB AMP, and a scale to 116 under a control that stops at 102
            // would read as broken at the top.
            let reachable: f64 = self.stages.iter().map(|s| s.max_db).sum();
            if reachable > 0.0 {
                return reachable.round() as u32;
            }
        }
        self.gauge_fallback_db
    }

    /// Whether a distinct second gain stage (HackRF's VGA) exists.
    pub fn has_second_stage(&self) -> bool {
        self.has_second_stage
    }

    /// Label for the front-end-boost toggle (`amp_enabled`): HackRF's RF amp vs
    /// RTL-SDR's tuner AGC vs whatever a driver called its own switch.
    ///
    /// "AGC" covers the two cases with no name of their own, and neither is a
    /// guess: a gain mode genuinely is an automatic gain control, and a device
    /// with no boost at all never reaches this line, because every one of the
    /// nine call sites is gated on [`Self::has_boost`] first.
    pub fn boost_label(&self) -> &str {
        self.boost.as_ref().and_then(Boost::label).unwrap_or("AGC")
    }

    /// How the boost is reached, for the backend that has to drive it.
    pub fn boost(&self) -> Option<&Boost> {
        self.boost.as_ref()
    }

    /// The stages between antenna and converter, for the panels that draw a
    /// chain they cannot model.
    ///
    /// Only read where `friis_applicable` is false. An RTL-SDR really is one
    /// tuner, and a SoapySDR device is **whatever `listGains` named**, which is
    /// the driver's own answer rather than one of ours.
    pub fn unmodelled_stages(&self) -> String {
        self.chain_diagram.clone()
    }

    /// Why the modelled cascade is not on offer, in a few words.
    ///
    /// Devices land in that branch for **different reasons**, and the panels
    /// used to print one sentence for all of them: "single-tuner". That is true
    /// of an RTL-SDR and false of a HackRF reached through SoapySDR, which has
    /// three gain elements and a cascade we simply have not been told the noise
    /// figures for.
    pub fn no_cascade_reason(&self) -> &'static str {
        self.no_cascade_reason
    }

    /// Whether there is a front-end boost to toggle at all.
    ///
    /// A device can decline: plenty of SoapySDR devices have neither an RF amp
    /// nor an automatic gain mode, and a key that toggles a flag meaning
    /// nothing is worse than a key that is not offered.
    pub fn has_boost(&self) -> bool {
        self.boost.is_some()
    }

    /// Every adjustable stage this device has, front to back.
    ///
    /// The boost is **not** here. It is a toggle, not a range, and every caller
    /// that wants it asks [`Self::has_boost`].
    pub fn stages(&self) -> Vec<StageSpec> {
        self.stages.clone()
    }

    /// Snap stored gains into this model's legal values, returning `(lna, vga)`.
    ///
    /// A config saved on one device family must not apply or display an illegal
    /// gain on another - e.g. an RTL-SDR tuner's 49 dB on a HackRF LNA that
    /// maxes at 40, or a HackRF value shown unsnapped on an RTL tuner's discrete
    /// table.
    ///
    /// **Every device answers from `stages()`**, so the 8 dB and 2 dB grids, the
    /// tuner's table and a driver's reported elements are each written once. A
    /// device with fewer stages than values leaves the extra ones alone rather
    /// than zeroing them: there is nothing to snap to, and saying so is not the
    /// same as saying the value is 0.
    pub fn clamp_gains(&self, lna: u32, vga: u32) -> (u32, u32) {
        let first = self
            .stages
            .first()
            .map(|st| st.snap(lna as f64).max(0.0).round() as u32)
            .unwrap_or(lna);
        let second = self
            .stages
            .get(1)
            .map(|st| st.snap(vga as f64).max(0.0).round() as u32)
            .unwrap_or(vga);
        (first, second)
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

    /// Set one stage by position, exactly.
    ///
    /// **One path for every backend.** The default maps position onto the two
    /// setters the native radios already have, so a HackRF and an RTL-SDR need
    /// no new code; a SoapySDR device overrides it and addresses the element by
    /// the name the driver gave it.
    ///
    /// `name` is passed rather than looked up because the caller already has the
    /// stage list it is iterating, and a second lookup here could disagree with
    /// it.
    fn set_stage_gain(&self, index: usize, _name: &str, db: f64) -> anyhow::Result<()> {
        let whole = db.max(0.0).round() as u32;
        match index {
            0 => self.set_lna_gain(whole),
            1 => self.set_vga_gain(whole),
            _ => Ok(()),
        }
    }

    /// Anything the backend refused or worked around while opening, for the
    /// startup log.
    ///
    /// Empty for a backend that had nothing to say, which is both native ones.
    /// It exists because a refusal made during `open` has no log to go to yet:
    /// the app is built afterwards. `App::assemble` drains this, the same way
    /// it drains the menu's warnings.
    fn open_notes(&self) -> &[String] {
        &[]
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
    use crate::hardware::native::{hackrf, rtlsdr};

    // ── The native stage lists ──────────────────────────────────────────────

    /// `clamp_gains` is what a config saved on one radio lands on when it is
    /// opened on the other, so rewriting it on top of `stages()` had to change
    /// nothing. This is the arithmetic it replaced, run against the new path for
    /// **every** value either stage can be handed.
    #[test]
    fn the_hackrf_stage_list_reproduces_the_old_clamp_exactly() {
        let g = hackrf::gain_model();
        for lna in 0..=200u32 {
            for vga in [0u32, 1, 2, 3, 31, 47, 61, 62, 63, 99, 200] {
                let was = ((lna.min(40) + 4) / 8 * 8, vga.min(62).div_ceil(2) * 2);
                assert_eq!(g.clamp_gains(lna, vga), was, "lna={lna} vga={vga}");
            }
        }
    }

    #[test]
    fn the_rtl_stage_list_reproduces_the_old_clamp_exactly() {
        // The shape `rtl_caps` produces: whole dB, deduped, irregular.
        let table: Vec<u32> = vec![0, 1, 3, 4, 8, 15, 24, 33, 40, 49];
        let g = rtlsdr::gain_model(&table.clone());
        for lna in 0..=120u32 {
            let was = table
                .iter()
                .copied()
                .min_by_key(|&x| (x as i64 - lna as i64).abs())
                .unwrap();
            assert_eq!(g.clamp_gains(lna, 30), (was, 30), "lna={lna}");
        }
    }

    /// The two native models describe themselves, and the boost is not among
    /// the stages: it is a toggle, not a range.
    #[test]
    fn the_native_models_name_their_own_stages() {
        let hack = hackrf::gain_model().stages();
        let names: Vec<&str> = hack.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            ["LNA", "VGA"],
            "the RF amp is the boost, not a stage"
        );
        assert_eq!(hack[0].positions(), Some(6), "0, 8, 16, 24, 32, 40");
        assert_eq!(hack[1].positions(), Some(32));

        let rtl = rtlsdr::gain_model(&[0, 1, 3, 49]).stages();
        assert_eq!(rtl.len(), 1);
        assert_eq!(rtl[0].name, "Tuner");
        assert_eq!(rtl[0].positions(), Some(4), "a table, not a grid");
        assert_eq!(rtl[0].snap(2.0), 1.0, "nearest entry, ties to the first");
        assert_eq!(rtl[0].snap(1000.0), 49.0);
    }

    /// A tuner that reported nothing still has to answer. The driver's own
    /// fallback is a single zero entry, and a stage over it must not divide by
    /// an empty table.
    #[test]
    fn a_single_entry_table_is_still_a_stage() {
        let g = rtlsdr::gain_model(&[0]);
        assert_eq!(g.stages()[0].positions(), Some(1));
        assert_eq!(g.clamp_gains(37, 12), (0, 12));
    }

    // ── StageSpec ───────────────────────────────────────────────────────────
    //
    // The three fixtures are the real answers `SoapySDRUtil --probe` gives for a
    // HackRF through SoapyHackRF, transcribed rather than invented:
    //
    //     LNA gain range: [0, 40, 8] dB
    //     AMP gain range: [0, 14, 14] dB
    //     VGA gain range: [0, 62, 2] dB

    fn stage(name: &str, min: f64, max: f64, step: f64) -> StageSpec {
        StageSpec::ranged(name, min, max, step)
    }

    /// A driver-described chain, the shape `soapy::caps` builds: stages the
    /// driver named, one knob for all of them, no modelled cascade.
    fn chain_with_boost(boost: Option<Boost>) -> GainModel {
        let gm = GainModel::new(vec![stage("LNA", 0.0, 40.0, 8.0)], "RF", "RF");
        match boost {
            Some(b) => gm.with_boost(b),
            None => gm,
        }
    }

    /// The four answers `boost_label` can give, pinned before anything moves
    /// where they come from.
    ///
    /// The two native ones are string constants in a match arm today. They are
    /// about to become fields of a `Boost` each backend builds for itself, and
    /// a refactor that quietly changes what the rail and the header print is
    /// exactly what a test naming the strings catches and a structural one does
    /// not.
    #[test]
    fn each_backend_names_its_own_boost() {
        assert_eq!(hackrf::gain_model().boost_label(), "AMP");
        assert_eq!(rtlsdr::gain_model(&[0, 15, 28]).boost_label(), "AGC");

        // A driver that named its switch: its own word wins. `SoapyHackRF` says
        // AMP for the same physical amplifier the native path calls AMP, so one
        // radio reads the same whichever way it was reached.
        let named = chain_with_boost(Some(Boost::Element(stage("AMP", 0.0, 14.0, 14.0))));
        assert_eq!(named.boost_label(), "AMP");

        // A gain mode is a flag with no name, and an automatic gain control is
        // what it is. Not a fallback standing in for something unknown.
        assert_eq!(chain_with_boost(Some(Boost::GainMode)).boost_label(), "AGC");
    }

    /// A device that declines a boost says so, and nothing asks it for a label.
    ///
    /// The label is still `&str` rather than `Option<&str>` because all nine
    /// call sites are gated on this method. If that ever stops being true, this
    /// test is the one that should start looking wrong.
    #[test]
    fn a_device_with_no_boost_reports_none_to_offer() {
        assert!(!chain_with_boost(None).has_boost());
        assert!(hackrf::gain_model().has_boost());
        assert!(rtlsdr::gain_model(&[0]).has_boost());
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

    /// The bug the struct conversion fixed, pinned so it cannot come back.
    ///
    /// A `SoapyHackRF` reports a whole-chain range of 0 to 116 dB, and the old
    /// chain arm of `clamp_gains` clamped the **primary** gain against it. The
    /// stage that value goes into stops at 40. Asking for 100 therefore stored
    /// 100, presented it as legal, and only the radio disagreed.
    ///
    /// Every device now answers from `stages()`, and the two native backends
    /// always did, which is why this was only ever wrong on one of the three.
    #[test]
    fn a_chain_clamps_each_gain_into_its_own_stage() {
        let chain = GainModel::new(
            vec![stage("LNA", 0.0, 40.0, 8.0), stage("VGA", 0.0, 62.0, 2.0)],
            "RF",
            "RF",
        )
        .with_gauge_fallback(116);

        assert_eq!(
            chain.clamp_gains(100, 0).0,
            40,
            "the front stage stops at 40, whatever the whole chain adds up to"
        );
        assert_eq!(chain.clamp_gains(200, 200), (40, 62), "both ends");
        assert_eq!(chain.clamp_gains(16, 20), (16, 20), "already on the grids");
        assert_eq!(chain.clamp_gains(13, 47), (16, 48), "snapped to them");

        // And the gauge reads what the knob can reach, not what `getGainRange`
        // said: 116 counts the AMP, which is the boost and has its own key.
        assert_eq!(chain.primary_max_db(), 102);
    }

    /// A device with fewer stages than values leaves the extra ones alone.
    ///
    /// Not the same as zeroing them. An RTL-SDR that named no gains has no
    /// table to snap to, and a config the operator recognises beats a
    /// fabricated legal value.
    #[test]
    fn a_gain_with_no_stage_to_snap_to_is_left_as_it_is() {
        let one = GainModel::new(vec![stage("RF", 0.0, 30.0, 0.0)], "RF", "RF");
        assert_eq!(
            one.clamp_gains(20, 44),
            (20, 44),
            "nothing holds the second"
        );
        let none = GainModel::new(Vec::new(), "RF", "RF").with_gauge_fallback(49);
        assert_eq!(none.clamp_gains(27, 13), (27, 13));
        assert_eq!(none.primary_max_db(), 49, "the gauge still needs a top");
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
        let g = hackrf::gain_model();
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
        let g = rtlsdr::gain_model(&[0, 9, 16, 24, 49]);
        // A HackRF LNA value snaps to the nearest tuner-table entry; vga is inert.
        assert_eq!(g.clamp_gains(20, 40), (16, 40));
        assert_eq!(g.clamp_gains(100, 0), (49, 0));
        // An empty table can't snap → the value passes through unchanged.
        let empty = rtlsdr::gain_model(&[]);
        assert_eq!(empty.clamp_gains(33, 7), (33, 7));
    }
}
