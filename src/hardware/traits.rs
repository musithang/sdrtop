// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Device abstraction: the [`SdrDevice`] trait plus the capability and metadata
//! types that let HackRF, RTL-SDR, and future backends share one RX → FFT
//! pipeline, one UI, and one input handler. Concrete backends live in the
//! `native` and `soapy` submodules; everything device-generic keys off the
//! [`DeviceCapabilities`] descriptor rather than matching on the device type.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::state::SdrMetrics;

/// How a backend acquires spectrum data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AcquisitionKind {
    /// Complex time-domain samples feed the FFT and diagnostic workers.
    IqSamples,
    /// The backend publishes calibrated power-spectrum traces.
    PowerTrace,
}

/// Unit carried by spectral levels from a backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LevelUnit {
    Dbfs,
    Dbm,
}

pub const IQ_TRACE_STALE_MS: u128 = 500;

impl LevelUnit {
    pub fn label(self) -> &'static str {
        match self {
            Self::Dbfs => "dBFS",
            Self::Dbm => "dBm",
        }
    }
}

/// Where a direct power trace should be published.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerTraceTarget {
    Spectrum,
    Sweep,
}

/// One calibrated power-spectrum trace.
#[derive(Debug)]
pub struct PowerTrace {
    pub target: PowerTraceTarget,
    pub generation: u64,
    pub frequencies_hz: Vec<u64>,
    pub levels_dbm: Vec<f32>,
    pub rbw_hz: Option<u32>,
}

/// A native band sweep requested from a direct-power backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirectSweepConfig {
    pub start_hz: u64,
    pub stop_hz: u64,
    pub generation: u64,
}

/// One configurable choice exposed by a device.
///
/// Every snapshot must use unique IDs. Each option must advertise at least one
/// choice. `selected_choice` must be one of those choices. Choice strings are
/// stable values passed back to [`SdrDevice::set_option`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceOption {
    pub id: String,
    pub label: String,
    pub choices: Vec<String>,
    pub selected_choice: String,
    /// Editor bounds may contain gaps in the advertised canonical integer choices
    pub integer_range: Option<std::ops::RangeInclusive<i32>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IntegerChoiceError {
    InvalidInteger,
    OutOfRange { min: i32, max: i32 },
    Unadvertised { closest: Option<i32> },
    Unavailable,
}

impl std::fmt::Display for IntegerChoiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInteger => write!(f, "Enter a signed 32-bit integer"),
            Self::OutOfRange { min, max } => write!(f, "Out of range: {min} to {max}"),
            Self::Unadvertised {
                closest: Some(value),
            } => {
                write!(f, "Not advertised. Closest choice: {value}")
            }
            Self::Unadvertised { closest: None } => write!(f, "No advertised integer in range"),
            Self::Unavailable => write!(f, "Numeric entry is no longer available"),
        }
    }
}

impl DeviceOption {
    pub fn integer_choice(&self, input: &str) -> Result<&str, IntegerChoiceError> {
        let range = self
            .integer_range
            .as_ref()
            .ok_or(IntegerChoiceError::Unavailable)?;
        let digits = input.strip_prefix('-').unwrap_or(input);
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(IntegerChoiceError::InvalidInteger);
        }
        let value = input
            .parse::<i32>()
            .map_err(|_| IntegerChoiceError::InvalidInteger)?;
        if !range.contains(&value) {
            return Err(IntegerChoiceError::OutOfRange {
                min: *range.start(),
                max: *range.end(),
            });
        }
        let choice = value.to_string();
        self.choices
            .iter()
            .find(|advertised| **advertised == choice)
            .map(String::as_str)
            .ok_or_else(|| IntegerChoiceError::Unadvertised {
                closest: self
                    .choices
                    .iter()
                    .filter_map(|choice| {
                        let number = choice.parse::<i32>().ok()?;
                        (range.contains(&number) && number.to_string() == *choice).then_some(number)
                    })
                    .min_by_key(|number| (i64::from(*number) - i64::from(value)).abs()),
            })
    }
}

#[cfg(test)]
mod integer_choice_tests {
    use super::*;

    fn option() -> DeviceOption {
        DeviceOption {
            id: "level".into(),
            label: "Level".into(),
            choices: ["auto", "-100", "0", "10", "100"]
                .map(String::from)
                .to_vec(),
            selected_choice: "auto".into(),
            integer_range: Some(-100..=100),
        }
    }

    #[test]
    fn integer_validation_distinguishes_failure_reasons() {
        let mut option = option();
        for input in ["", "-", "+1", "1.0", "2147483648", "-2147483649"] {
            assert_eq!(
                option.integer_choice(input),
                Err(IntegerChoiceError::InvalidInteger)
            );
        }
        assert_eq!(
            option.integer_choice("101"),
            Err(IntegerChoiceError::OutOfRange {
                min: -100,
                max: 100
            })
        );
        assert_eq!(
            option.integer_choice("12"),
            Err(IntegerChoiceError::Unadvertised { closest: Some(10) })
        );
        assert_eq!(option.integer_choice("-00100"), Ok("-100"));
        assert_eq!(option.integer_choice("-0"), Ok("0"));
        option.integer_range = None;
        assert_eq!(
            option.integer_choice("12"),
            Err(IntegerChoiceError::Unavailable)
        );
    }

    #[test]
    fn closest_choice_excludes_noncanonical_and_out_of_range_values() {
        let mut option = option();
        option.choices = ["auto", "012", "+12", "12.0", "11", "31"]
            .map(String::from)
            .to_vec();
        option.integer_range = Some(12..=31);
        assert_eq!(
            option.integer_choice("12"),
            Err(IntegerChoiceError::Unadvertised { closest: Some(31) })
        );
        option.choices.pop();
        assert_eq!(
            option.integer_choice("12"),
            Err(IntegerChoiceError::Unadvertised { closest: None })
        );
    }

    #[test]
    fn closest_choice_distance_handles_i32_extremes() {
        let mut option = option();
        option.integer_range = Some(i32::MIN..=i32::MAX);
        option.choices = vec![i32::MIN.to_string(), i32::MAX.to_string()];
        assert_eq!(
            option.integer_choice("1"),
            Err(IntegerChoiceError::Unadvertised {
                closest: Some(i32::MAX)
            })
        );
        assert_eq!(
            option.integer_choice("-1"),
            Err(IntegerChoiceError::Unadvertised {
                closest: Some(i32::MIN)
            })
        );
    }
}

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
    /// Interleaved signed 32-bit little endian, four bytes per component, for
    /// a library that hands floats a radio's integer words became after its
    /// own processing: libairspyhf corrects and fine-tunes the HF+'s 16-bit
    /// USB words in floating point, and rounding them back to 16 bits lifted
    /// the measured noise floor by up to half a dB wherever the radio's own
    /// noise was under one word. The words are carried with fraction bits
    /// below them, so the full scale is the converter's times a power of two
    /// and [`SampleGeometry::converter_bits`] says how deep the converter is.
    ///
    /// A block's per-sample sums are `i64`: a sample's square is under
    /// 2^48 for anything within twice the full scale of a 24-bit container,
    /// so a block may hold 2^15 such pairs; the backend hands blocks well
    /// under that.
    Int32,
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
/// true even for the RTL-SDR, whose analogue zero is at 127.5: the decoder
/// centres on 128 because its accumulators are integers, and
/// [`SampleGeometry::centre_bias`] is what anything reporting an offset adds
/// back.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct SampleGeometry {
    pub format: SampleFormat,
    /// The sample count that means 0 dBFS.
    pub full_scale: f32,
    /// The converter's own depth, when a library has scaled its samples into
    /// a wider container than the full scale says. libairspy shifts the R2's
    /// 12-bit samples left by four and filters them into int16, so its full
    /// scale is 16384 counts while the converter behind it has 12 bits. `None`
    /// for every device whose full scale is its converter's (all the others).
    pub converter_bits: Option<u8>,
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
            SampleFormat::Int32 => 8,
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
    ///
    /// **Unless the converter's depth is stated**: a library that scales its
    /// samples up (libairspy puts 12 bits into 16384 counts) has made the
    /// full scale say more than the converter has.
    pub fn bits(&self) -> u8 {
        let derived = (self.full_scale.max(1.0).log2().round() as i32 + 1).clamp(1, 32) as u8;
        self.converter_bits
            .unwrap_or(derived)
            .min(self.container_bits())
    }

    /// Counts between the decoder's zero and the converter's own zero.
    ///
    /// [`crate::hardware::process`] centres `Uint8` by subtracting 128, because
    /// the accumulators are integers and 127.5 is not one. But the converter's
    /// analogue zero sits *between* code 127 and code 128, so a perfectly
    /// centred CU8 stream decodes to a mean of -0.5 counts. Anything reporting a
    /// DC offset has to add this back or it reports the decode convention as a
    /// fault in the radio.
    ///
    /// Half an LSB is 0.0039 of full scale on an 8-bit radio, which is 78 % of
    /// the offset the IQ bench warns at and puts a -45 dBFS floor under a spike
    /// it grades from -40 dBFS. Every other format's zero is a code, so there is
    /// nothing to add back.
    pub fn centre_bias(&self) -> f64 {
        match self.format {
            SampleFormat::Uint8 => 0.5,
            SampleFormat::Int8 | SampleFormat::Int16 | SampleFormat::Int32 => 0.0,
        }
    }

    /// Counts in one step of the converter's own words: where a library's
    /// processing left fractions of a word in a wider container, the
    /// converter's top word is that many counts below the full scale, and
    /// that, not one count below it, is where its samples rail.
    ///
    /// Only [`SampleFormat::Int32`] carries such fractions. Every other format
    /// holds whole words, one count each, and its rail stays a count below
    /// the full scale whatever the converter's depth: libairspy's 12-bit R2 in
    /// int16 is filtered, not scaled by a whole number of counts, so its words
    /// have no such step to find.
    pub fn counts_per_word(&self) -> i64 {
        match self.format {
            SampleFormat::Int32 => {
                let fs = self.full_scale.max(1.0) as i64;
                (fs >> (u32::from(self.bits()).saturating_sub(1))).max(1)
            }
            SampleFormat::Int8 | SampleFormat::Uint8 | SampleFormat::Int16 => 1,
        }
    }

    /// How many bits the wire format can carry per component, whatever the
    /// converter behind it actually fills.
    fn container_bits(&self) -> u8 {
        match self.format {
            SampleFormat::Int8 | SampleFormat::Uint8 => 8,
            SampleFormat::Int16 => 16,
            SampleFormat::Int32 => 32,
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
    /// Where these values are decibels: the frequencies, in Hz, the stage was
    /// measured over. `None` means everywhere the device tunes, which is what
    /// a datasheet or a driver's own answer is taken to say.
    ///
    /// **Outside it a setting is a position, not a gain.** The Airspy R2's LNA
    /// was measured on the bench across the VHF band only, and its steps give
    /// up to 5 dB less at 900 MHz; carrying the VHF figures there would show
    /// decibels nobody measured. So a stage tuned outside its band reads as
    /// "step 14 of 14", and anything that would add it into a dB total does
    /// not.
    pub measured_in: Option<std::ops::RangeInclusive<u64>>,
}

/// How a stage's setting reads at the frequency the radio is tuned to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StageReading {
    /// A gain in dB, as measured or as the device states it.
    Db(f64),
    /// Where the setting sits among the stage's positions, `at` counted from
    /// 0 and `of` the last one, because its dB was not measured here.
    Position { at: u32, of: u32 },
    /// Set by the device's own automatic gain, so sdrtop does not know it.
    Auto,
}

impl StageReading {
    /// Five columns: `36 dB`, `-12dB` for an attenuator, or `14/14` where
    /// the dB was not measured.
    pub fn compact(self) -> String {
        match self {
            // An attenuator's setting is a negative gain, and said as one:
            // flooring it at zero showed "0 dB" for 12 dB of attenuation.
            StageReading::Db(db) if db.round() < 0.0 => format!("{:3.0}dB", db),
            StageReading::Db(db) => format!("{:2.0} dB", db.max(0.0)),
            StageReading::Position { at, of } => format!("{at:2}/{of}"),
            StageReading::Auto => "  AGC".to_string(),
        }
    }
}

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
            measured_in: None,
        }
    }

    /// The same stage, its values decibels only between `lo_hz` and `hi_hz`.
    pub fn measured_between(mut self, lo_hz: u64, hi_hz: u64) -> Self {
        self.measured_in = Some(lo_hz..=hi_hz);
        self
    }

    /// Whether this stage's values are decibels at `hz`.
    pub fn db_holds_at(&self, hz: u64) -> bool {
        self.measured_in
            .as_ref()
            .is_none_or(|band| band.contains(&hz))
    }

    /// The position a value sits at: its index in the table, or its step
    /// count from the minimum. `None` for a continuous stage.
    pub fn position_of(&self, db: f64) -> Option<u32> {
        if !self.table.is_empty() {
            let nearest = self
                .table
                .iter()
                .enumerate()
                .min_by(|a, b| (a.1 - db).abs().total_cmp(&(b.1 - db).abs()))?;
            return u32::try_from(nearest.0).ok();
        }
        (self.step_db > 0.0).then(|| ((db - self.min_db) / self.step_db).round().max(0.0) as u32)
    }

    /// The value at a position: its table entry, or its step counted from the
    /// minimum, inside the range. The inverse of [`Self::position_of`].
    pub fn value_at(&self, position: u32) -> f64 {
        if !self.table.is_empty() {
            let last = self.table.len() - 1;
            return self.table[(position as usize).min(last)];
        }
        (self.min_db + f64::from(position) * self.step_db).clamp(self.min_db, self.max_db)
    }

    /// How a setting of `db` reads with the radio tuned to `hz`.
    pub fn reading(&self, db: f64, hz: u64) -> StageReading {
        if self.db_holds_at(hz) {
            return StageReading::Db(db);
        }
        match (self.position_of(db), self.positions()) {
            (Some(at), Some(n)) => StageReading::Position {
                at,
                of: n.saturating_sub(1),
            },
            // A continuous stage has no positions to name; its value is what
            // there is, and a continuous stage measured in a band is not a
            // shape anything builds.
            _ => StageReading::Db(db),
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
            measured_in: None,
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
    #[cfg(test)]
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

/// Where a radio's chain stops the samples before the converter's own rails,
/// measured on the radio, never assumed: what sets it, what happens there,
/// and how high it is at each frequency.
#[derive(Clone, Debug, PartialEq)]
pub struct Ceiling {
    /// The part that sets it, as the screen names it (`"tuner"`).
    pub part: &'static str,
    /// What happens to a signal that reaches it.
    pub kind: CeilingKind,
    /// From each frequency up to the next entry's, lowest first (the first
    /// from 0 Hz): the lowest and the highest level it was measured at, dBFS
    /// of the converter; the same for a ceiling that does not move.
    steps: Vec<(u64, f64, f64)>,
}

impl Ceiling {
    /// The measured span at `hz`, lowest first.
    fn span_at(&self, hz: u64) -> (f64, f64) {
        let &(_, low, high) = self
            .steps
            .iter()
            .rev()
            .find(|(from, _, _)| hz >= *from)
            .unwrap_or(&self.steps[0]);
        (low, high)
    }

    /// Where it starts at `hz`: the lowest level it was measured at, which
    /// is where every judgement of a peak starts, because past it the level
    /// may already not be the antenna's.
    pub fn dbfs_at(&self, hz: u64) -> f64 {
        self.span_at(hz).0
    }

    /// How it reads on screen at `hz`, rounded as every level beside it is:
    /// `-3`, or `-14 to -12` for one measured over a span.
    pub fn text_at(&self, hz: u64) -> String {
        let (low, high) = self.span_at(hz);
        let (low, high) = (low.round_ties_even() + 0.0, high.round_ties_even() + 0.0);
        if low == high {
            format!("{low:.0}")
        } else {
            format!("{low:.0} to {high:.0}")
        }
    }
}

/// What a [`Ceiling`] does to a signal that reaches it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CeilingKind {
    /// The part clips: what passes it is lost, and no converter rail counts
    /// it.
    Clips,
    /// The part turns its own gain down to keep the level there: nothing is
    /// lost, but past it the level is no longer the antenna's, and the radio
    /// does not report by how much.
    Holds,
}

impl CeilingKind {
    /// "clips" or "holds", for "the tuner clips at -3".
    pub fn verb(self) -> &'static str {
        match self {
            Self::Clips => "clips",
            Self::Holds => "holds",
        }
    }

    /// "clip" or "hold", for "the tuner's clip".
    pub fn noun(self) -> &'static str {
        match self {
            Self::Clips => "clip",
            Self::Holds => "hold",
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
/// A vendor's own way of spreading one knob across the stages: each step a
/// position on every stage, front to back.
///
/// **Positions, not decibels**, because that is what a vendor's table holds,
/// and a position means the same register setting at every frequency even
/// where its decibels were not measured.
#[derive(Clone, Debug, PartialEq)]
pub struct GainCurve {
    pub name: &'static str,
    /// Three letters, for the readouts that have no room for the name.
    pub short: &'static str,
    steps: Vec<Vec<u32>>,
}

impl GainCurve {
    pub fn new(name: &'static str, short: &'static str, steps: Vec<Vec<u32>>) -> Self {
        Self { name, short, steps }
    }

    /// The highest step; steps count from 0.
    pub fn last_step(&self) -> usize {
        self.steps.len().saturating_sub(1)
    }

    /// One value per stage at `step`, each stage's own value at the position
    /// the curve gives it.
    pub fn values(&self, step: usize, stages: &[StageSpec]) -> Vec<f64> {
        let positions = &self.steps[step.min(self.last_step())];
        stages
            .iter()
            .zip(positions)
            .map(|(stage, &at)| stage.value_at(at))
            .collect()
    }

    /// The step the chain is exactly on, if it is on one.
    pub fn step_of(&self, stages: &[StageSpec], gains: &[f64]) -> Option<usize> {
        let at = Self::positions_of(stages, gains)?;
        self.steps.iter().position(|step| *step == at)
    }

    /// The highest step whose chain adds up to no more than `total_db`, or the
    /// first step when every one is over it. Meaningful only where each
    /// stage's values are dB.
    pub fn step_for_total(&self, stages: &[StageSpec], total_db: f64) -> usize {
        (0..self.steps.len())
            .rev()
            .find(|&step| self.values(step, stages).iter().sum::<f64>() <= total_db + 1e-9)
            .unwrap_or(0)
    }

    /// The step closest to the chain, by how many positions each stage is
    /// away from it in all; the lower step on a tie. Where a hand-set chain
    /// rejoins the curve.
    pub fn nearest_step(&self, stages: &[StageSpec], gains: &[f64]) -> usize {
        let Some(at) = Self::positions_of(stages, gains) else {
            return 0;
        };
        self.steps
            .iter()
            .enumerate()
            .min_by_key(|(_, step)| {
                step.iter()
                    .zip(&at)
                    .map(|(&a, &b)| a.abs_diff(b))
                    .sum::<u32>()
            })
            .map_or(0, |(i, _)| i)
    }

    fn positions_of(stages: &[StageSpec], gains: &[f64]) -> Option<Vec<u32>> {
        stages
            .iter()
            .enumerate()
            .map(|(i, s)| s.position_of(gains.get(i).copied().unwrap_or(s.min_db)))
            .collect()
    }
}

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
    /// The stages are digital gain after the converter, not front-end gain,
    /// and this names what sits before the converter (`"AGC"`); `None` for a
    /// chain whose stages are ahead of it. See [`Self::stages_are_digital`].
    digital: Option<&'static str>,
    no_cascade_reason: &'static str,
    /// Full scale for the primary gain gauge when the stages describe no span.
    ///
    /// Reachable only for a device whose stages are absent or zero width: an
    /// RTL-SDR whose tuner named no gains at all, or a driver that reported a
    /// chain with no room in it. 49 dB is the historical RTL answer.
    gauge_fallback_db: u32,
    /// The vendor's curves, when the device has them; see [`GainCurve`].
    curves: Vec<GainCurve>,
    /// The curve and step a first run and `[R]` start from, when the device
    /// starts on a curve rather than on its stages' defaults.
    start: Option<(usize, usize)>,
    /// The stages an automatic gain mode sets, by position; empty for all of
    /// them. See [`Self::auto_governs`].
    auto_stages: Vec<usize>,
    /// Where the chain stops the samples before the converter does. See
    /// [`Self::ceiling_dbfs_at`].
    ceiling: Option<Ceiling>,
    /// The gains a first run and `[R]` give the stages when the knob is on no
    /// curve, where the device has its own; `None` for the historical rule
    /// (`hardware::gain::defaults`).
    start_values: Option<Vec<f64>>,
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
            digital: None,
            no_cascade_reason: "chain not modelled",
            gauge_fallback_db: 0,
            curves: Vec::new(),
            start: None,
            auto_stages: Vec::new(),
            ceiling: None,
            start_values: None,
        }
    }

    /// The stages start at `values`, one per stage, at a first run and on
    /// `[R]`, rather than at the historical front and second-stage defaults:
    /// for a device whose sensible start is not "some gain on the first two
    /// stages", an attenuator at none and a preamp off among them.
    pub fn with_start_gains(mut self, values: Vec<f64>) -> Self {
        debug_assert_eq!(values.len(), self.stages.len(), "one start value per stage");
        self.start_values = Some(values);
        self
    }

    /// The chain clips at `dbfs` of the converter, in `what` (`"tuner"`),
    /// before the converter's own rails, at every frequency: measured, not
    /// assumed.
    pub fn with_ceiling(mut self, dbfs: f64, what: &'static str) -> Self {
        self.ceiling = Some(Ceiling {
            part: what,
            kind: CeilingKind::Clips,
            steps: vec![(0, dbfs, dbfs)],
        });
        self
    }

    /// `what` (`"digital AGC"`) holds the level at a ceiling by turning its
    /// own gain down, somewhere between the two levels of `steps` (dBFS of
    /// the converter, lowest first) from each frequency up to the next one's:
    /// measured, not assumed.
    pub fn with_held_ceiling(mut self, what: &'static str, steps: Vec<(u64, f64, f64)>) -> Self {
        debug_assert!(
            steps.first().is_some_and(|&(from, _, _)| from == 0)
                && steps.windows(2).all(|w| w[0].0 < w[1].0)
                && steps.iter().all(|&(_, low, high)| low <= high),
            "steps from 0 Hz, lowest first, each span lowest first"
        );
        self.ceiling = Some(Ceiling {
            part: what,
            kind: CeilingKind::Holds,
            steps,
        });
        self
    }

    /// The highest level the samples can honestly reach at `hz`: the
    /// converter's 0 dBFS, or lower where the chain stops them first. Every
    /// judgement of a peak (headroom, the staging verdict, auto-gain's
    /// target) is made against this, because neither a clip nor a hold there
    /// touches a converter rail, and the clip counter cannot see it.
    pub fn ceiling_dbfs_at(&self, hz: u64) -> f64 {
        self.ceiling.as_ref().map_or(0.0, |c| c.dbfs_at(hz))
    }

    /// The ceiling, when it is not the converter's.
    pub fn ceiling(&self) -> Option<&Ceiling> {
        self.ceiling.as_ref()
    }

    /// The automatic gain mode sets these stages only; the rest stay where
    /// the keys put them.
    pub fn with_auto_stages(mut self, stages: &[usize]) -> Self {
        self.auto_stages = stages.to_vec();
        self
    }

    /// Whether stage `index` is the device's own to set while its automatic
    /// gain mode is on: every stage, unless the device named the ones it
    /// sets. A boost that is a switch rather than a gain mode sets none.
    pub fn auto_governs(&self, index: usize) -> bool {
        matches!(self.boost, Some(Boost::GainMode))
            && (self.auto_stages.is_empty() || self.auto_stages.contains(&index))
    }

    /// The vendor's curves, the knob starting on the one named `start` at
    /// `step`.
    pub fn with_curves(mut self, curves: Vec<GainCurve>, start: &str, step: usize) -> Self {
        let index = curves.iter().position(|c| c.name == start);
        debug_assert!(index.is_some(), "no curve named {start}");
        self.curves = curves;
        self.start = index.map(|i| (i, step));
        self
    }

    pub fn curves(&self) -> &[GainCurve] {
        &self.curves
    }

    /// The curve called `name`, by its index.
    pub fn curve_named(&self, name: &str) -> Option<usize> {
        self.curves
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(name))
    }

    /// The curve the knob starts on, if the device starts on one.
    pub fn default_curve(&self) -> Option<usize> {
        self.start.map(|(curve, _)| curve)
    }

    /// Where a first run and `[R]` put the stages with the knob on `curve`:
    /// the start step of that curve, or each stage's own default when the knob
    /// is on no curve.
    pub fn start_gains(&self, curve: Option<usize>) -> Vec<f64> {
        let stages = self.stages();
        match curve.and_then(|c| self.curves.get(c)) {
            Some(c) => c.values(self.start.map_or(0, |(_, step)| step), &stages),
            None => match &self.start_values {
                Some(values) => stages
                    .iter()
                    .zip(values)
                    .map(|(stage, &v)| stage.snap(v))
                    .collect(),
                None => super::gain::defaults(&stages),
            },
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

    /// The stages are digital gain applied after the converter, with
    /// `before` (`"AGC"`) ahead of it: they place the level in the samples,
    /// they do not set what the converter sees. The Airspy HF+'s ATT and LNA
    /// are such, as every measurement here shows.
    pub fn with_digital_stages(mut self, before: &'static str) -> Self {
        self.digital = Some(before);
        self
    }

    /// Whether the stages are digital gain after the converter.
    pub fn stages_are_digital(&self) -> bool {
        self.digital.is_some()
    }

    /// The receive chain as a caption, for the panels that draw a chain they
    /// cannot model: `ANT▸LNA▸MIX▸VGA▸ADC` where the stages are ahead of the
    /// converter, `ANT▸AGC▸ADC▸ATT▸LNA▸word` where they are digital gain after
    /// it and the samples are the word they produce. The stage names are the
    /// device's own: an RTL-SDR really is one tuner, and a SoapySDR device is
    /// **whatever `listGains` named**, the driver's answer rather than ours.
    pub fn chain_caption(&self) -> String {
        match self.digital {
            Some(before) => format!(
                "ANT\u{25B8}{before}\u{25B8}ADC\u{25B8}{}\u{25B8}word",
                self.chain_diagram
            ),
            None => format!("ANT\u{25B8}{}\u{25B8}ADC", self.chain_diagram),
        }
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
    pub acquisition: AcquisitionKind,
    /// `set_sample_rate` controls a swept span for this device.
    pub sample_rate_is_span: bool,
    /// Spectral levels and display bounds use this unit
    pub level_unit: LevelUnit,
    /// The finite display floor must be below `level_max_db`
    pub level_min_db: f32,
    /// The finite display ceiling must form a finite span with `level_min_db`
    pub level_max_db: f32,
    /// A trace becomes stale after this positive millisecond budget
    pub trace_stale_ms: u128,
    /// The lowest and highest frequency the radio tunes: the hull of its
    /// bands, which is what a display spans.
    pub freq_min_hz: u64,
    pub freq_max_hz: u64,
    /// Stretches inside the hull the radio does not tune, lowest first, none
    /// overlapping. Empty for a radio with one band, which is every radio
    /// but the Airspy HF+: its HF and VHF front ends leave 31 to 60 MHz to
    /// neither, and a frequency there is refused rather than tuned to
    /// whatever the radio then delivers. Ask [`Self::tunes`], never the hull,
    /// whether a frequency can be tuned.
    pub freq_gaps: Vec<std::ops::RangeInclusive<u64>>,
    pub sample_rate_min_hz: f64,
    pub sample_rate_max_hz: f64,
    /// The rates the radio offers, lowest first, when it takes only those
    /// (an Airspy's firmware lists its own); empty when any rate between the
    /// minimum and the maximum will do. `[S]` steps through a list instead of
    /// leaving the user to guess which figures the radio accepts.
    pub sample_rates: Vec<f64>,
    /// Startup freq/rate guaranteed legal for THIS device. Used as the fallback
    /// when a loaded config value is out of range (e.g. a HackRF config opened
    /// on an RTL-SDR), so the radio never boots to an illegal setting.
    pub default_frequency_hz: u64,
    pub default_sample_rate_hz: f64,
    pub sample_geometry: SampleGeometry,
    pub gain: GainModel,
    /// IQ pairs per USB transfer, as declared at open.
    ///
    /// Read through [`SdrDevice::samples_per_transfer`] rather than from here:
    /// a backend that only learns its real block size once a stream is running
    /// overrides that method, and this field is what it falls back to.
    pub samples_per_transfer: u64,
    /// Programmable baseband filter (HackRF yes, RTL-SDR no). Part of the device
    /// capability contract and asserted in the device tests; the live panels key off
    /// `bb_filter_hz` (0 ⇒ unknown) directly, so the binary never reads this flag.
    pub has_bb_filter: bool,
    /// The Friis cascade NF / MDS panel applies (HackRF's known 3-stage chain).
    pub friis_applicable: bool,
    /// Whether the driver pushes samples at us or we pull them. The timing
    /// bench grades a push stream on its callbacks' arrival and a pull stream
    /// on its read loop.
    pub delivery: DeliveryModel,
    /// Blocks a push driver holds between its USB transfers and our callback,
    /// when it holds any: libairspy keeps eight. With such a queue a callback
    /// may run that many blocks less one late before anything is lost, so
    /// that, and not a slice of one block, is the timing bench's budget.
    /// `None` where the callback runs on the transfer itself (libhackrf,
    /// librtlsdr) or the backend pulls.
    pub driver_queue_blocks: Option<u32>,
}

/// A frequency's unit for [`DeviceCapabilities::bands_text`]: Hz, kHz, MHz,
/// GHz as 0..3, the largest the number reaches.
fn unit_of(hz: u64) -> u32 {
    match hz {
        1_000_000_000.. => 3,
        1_000_000.. => 2,
        1_000.. => 1,
        _ => 0,
    }
}

/// `hz` in `unit`, with no more decimals than it needs (at most three).
fn number_in(hz: u64, unit: u32) -> String {
    let v = hz as f64 / 1000f64.powi(unit as i32);
    let text = format!("{v:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// `hz` in its own unit, named: `9 kHz`, `31 MHz`, `1.766 GHz`.
fn hz_text(hz: u64) -> String {
    let unit = unit_of(hz);
    let name = ["Hz", "kHz", "MHz", "GHz"][unit as usize];
    format!("{} {name}", number_in(hz, unit))
}

impl DeviceCapabilities {
    /// The listed rate one step up or down from `from`, or `None` for a radio
    /// that takes any rate in its range. From a rate between two listed
    /// ones, the next listed one that way; at either end of the list, that
    /// end, so holding a key stops there rather than wrapping round.
    pub fn rate_step(&self, from: f64, up: bool) -> Option<f64> {
        let first = *self.sample_rates.first()?;
        let last = *self.sample_rates.last()?;
        Some(if up {
            self.sample_rates
                .iter()
                .copied()
                .find(|&r| r > from + 0.5)
                .unwrap_or(last)
        } else {
            self.sample_rates
                .iter()
                .rev()
                .copied()
                .find(|&r| r < from - 0.5)
                .unwrap_or(first)
        })
    }

    /// Whether the radio tunes to `hz`: inside the hull and in no gap.
    pub fn tunes(&self, hz: u64) -> bool {
        (self.freq_min_hz..=self.freq_max_hz).contains(&hz) && !self.in_gap(hz)
    }

    /// Whether `hz` falls in a gap between bands: inside the hull, where a
    /// spectrum near a band's edge can still show it, but not a frequency the
    /// radio receives as itself.
    pub fn in_gap(&self, hz: u64) -> bool {
        self.freq_gaps.iter().any(|gap| gap.contains(&hz))
    }

    /// The bands the radio tunes, lowest first: the hull with the gaps cut
    /// out of it.
    pub fn bands(&self) -> Vec<std::ops::RangeInclusive<u64>> {
        let mut bands = Vec::with_capacity(self.freq_gaps.len() + 1);
        let mut from = self.freq_min_hz;
        for gap in &self.freq_gaps {
            if *gap.start() > from {
                bands.push(from..=gap.start() - 1);
            }
            from = gap.end().saturating_add(1);
        }
        if from <= self.freq_max_hz {
            bands.push(from..=self.freq_max_hz);
        }
        bands
    }

    /// One step of `step` up or down from `hz`, kept inside the band `hz` is
    /// in: the arrows stop at a band's edge, as they stop at the hull's, and
    /// a gap is crossed by typing a frequency on the other side of it. From a
    /// frequency the radio does not tune, the band nearest it in the step's
    /// direction, or its last band.
    pub fn step_within_band(&self, hz: u64, step: u64, up: bool) -> u64 {
        let bands = self.bands();
        let band = bands
            .iter()
            .find(|band| band.contains(&hz))
            .or_else(|| {
                if up {
                    bands.iter().find(|band| *band.start() > hz)
                } else {
                    bands.iter().rev().find(|band| *band.end() < hz)
                }
            })
            .or(bands.last());
        let Some(band) = band else {
            return hz;
        };
        let next = if up {
            hz.saturating_add(step)
        } else {
            hz.saturating_sub(step)
        };
        next.clamp(*band.start(), *band.end())
    }

    /// The bands in words, for a refusal: `9 kHz to 31 MHz, 60 to 260 MHz`.
    pub fn bands_text(&self) -> String {
        self.bands()
            .iter()
            .map(|band| {
                let (lo, hi) = (*band.start(), *band.end());
                if unit_of(lo) == unit_of(hi) {
                    format!("{} to {}", number_in(lo, unit_of(hi)), hz_text(hi))
                } else {
                    format!("{} to {}", hz_text(lo), hz_text(hi))
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Reject invalid display bounds or a zero trace-age budget before initializing views
    pub fn validate_display(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.level_min_db.is_finite()
                && self.level_max_db.is_finite()
                && self.level_min_db < self.level_max_db
                && (self.level_max_db - self.level_min_db).is_finite(),
            "Invalid display level bounds: {}..{} {}; expected finite ordered bounds with a finite span",
            self.level_min_db,
            self.level_max_db,
            self.level_unit.label(),
        );
        anyhow::ensure!(
            self.trace_stale_ms > 0,
            "Invalid trace_stale_ms: expected a positive millisecond budget",
        );
        Ok(())
    }
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
    /// `None` on a HackRF, whose own firmware the header names; a radio whose
    /// firmware is not a HackRF's names it here under its own label. The
    /// header used to work this out by asking whether the gain model was
    /// single-knob, which was a fine proxy while there were two backends and
    /// started calling a SoapySDR device an RTL-SDR the moment there were
    /// three. A backend says what it is.
    pub stack: Option<SoftwareStack>,
}

/// The radio's own account of what it dropped, cumulative since its stream
/// started. See [`SdrDevice::radio_drops`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RadioDrops {
    /// Times the radio's buffer had nowhere to go and samples were lost.
    pub events: u32,
    /// The longest of them, in bytes of the radio's own sample format.
    pub longest_bytes: u32,
    /// The most bytes that can be between the radio and the callback at
    /// once: how far past the stream position at a reading a drop the reading
    /// already counts can still surface.
    pub in_flight_bytes: u64,
}

/// Shared RX plumbing handed to a backend's streaming start. The per-sample
/// accumulators write into `metrics`; raw byte blocks go out via `sample_tx` to
/// the FFT worker. `geometry` tells [`crate::hardware::process::digest`]
/// how to decode the bytes and what full scale is worth in them.
pub struct RxContext {
    pub metrics: Arc<Mutex<SdrMetrics>>,
    pub sample_tx: crossbeam_channel::Sender<Vec<u8>>,
    /// What the FFT feed did with the blocks handed to it, for the poll task.
    pub fft_feed: FeedHealth,
    /// Second, independently lossy feed to the demod worker. Blocks are forwarded
    /// only while `demod.enabled` is set, so the extra copy is paid for solely on
    /// the bench that uses it.
    ///
    /// Carries a [`StreamBlock`] rather than bare bytes: this channel drops under
    /// load like the FFT one, and the CTCSS detector needs to know whether its
    /// half-second of audio is one unbroken run or two pieces either side of a
    /// gap. The FFT feed needs no such thing, which is why only this channel pays
    /// for it.
    pub demod_tx: crossbeam_channel::Sender<StreamBlock>,
    /// Third, independently lossy feed, to the 2.4 GHz worker. Forwarded only
    /// while the NET section is the one on screen, so a user who never opens it
    /// pays for none of it.
    ///
    /// It has a [`FeedHealth`] where the demod feed does not, and the
    /// difference is what the two workers do with a block they never receive.
    /// The demod notices: it duty-cycles to four updates a second and reads its
    /// own sequence numbers, so a hole shows up as a gap in the audio and is
    /// counted. This worker is meant to run flat out on every block, and what
    /// it missed is a displayed number rather than an inference - which needs
    /// the count taken at the point of refusal, because after `try_send`
    /// returns there is nothing left that knows the block existed.
    pub net_tx: crossbeam_channel::Sender<StreamBlock>,
    /// What the NET feed did with the blocks handed to it, for the poll task.
    pub net_feed: FeedHealth,
    /// Fourth, independently lossy feed, to the pager worker. Forwarded only
    /// while a pager view is on screen. Counted as the NET feed is, for the
    /// same reason: a block lost inside a page destroys its codewords, so a
    /// loss is a displayed number, taken where the block was refused.
    pub pager_tx: crossbeam_channel::Sender<StreamBlock>,
    /// What the pager feed did with the blocks handed to it, for the poll task.
    pub pager_feed: FeedHealth,
    /// Fifth, independently lossy feed, to the FT8 worker, while the FT8
    /// view is on screen. Its blocks carry the wall clock at their arrival
    /// ([`TimedBlock`]); counted as the pager's, for the pager's reason.
    pub ft8_tx: crossbeam_channel::Sender<TimedBlock>,
    /// What the FT8 feed did with the blocks handed to it, for the poll task.
    pub ft8_feed: FeedHealth,
    /// Direct power-spectrum traces from backends that do not publish IQ.
    pub power_tx: crossbeam_channel::Sender<PowerTrace>,
    pub geometry: SampleGeometry,
    /// Where the stream is: how many I/Q pairs the radio has delivered since
    /// this stream began, counting the ones the driver itself lost. Stamped
    /// onto every [`StreamBlock`] as its [`StreamBlock::first_pair`].
    ///
    /// **A position, not a count of what arrived.** A block the bounded feed
    /// refuses still advances it, and so do samples the driver reports as
    /// dropped, so a receiver downstream can tell from two positions alone
    /// whether the second block continues the first - and by how much it does
    /// not. Reset by [`RxContext::begin_stream`], so a restarted stream starts
    /// again from zero rather than appearing to continue across the pause.
    pub stream_pairs: AtomicU64,
    /// How many streams have begun on this context ([`RxContext::begin_stream`]).
    /// What the RX task watches to restart anything it measures over a run of
    /// one stream: a radio that changes rate by stopping and starting its
    /// stream leaves a silence no rate measured across it may count.
    pub streams_begun: AtomicU64,
    /// Fourth feed, to the IQ recorder: raw blocks while a recording runs,
    /// nothing otherwise. Shared, because the recorder that arms it outlives
    /// no stream and the stream outlives no recorder.
    pub record: Arc<super::RecordTap>,
    /// Between the driver's thread and the work. See [`super::intake`].
    pub intake: super::intake::Intake,
}

impl RxContext {
    /// A new stream is about to start: its positions count from zero again.
    ///
    /// Called just before `start_rx`, wherever a stream is started. Without
    /// it, stopping and restarting RX would carry the old position on, and the
    /// first block of the new stream would look like the continuation of the
    /// last block of the old one - seconds apart, joined as if contiguous.
    pub fn begin_stream(&self) {
        self.stream_pairs
            .store(0, std::sync::atomic::Ordering::Relaxed);
        self.streams_begun
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

/// What the FFT feed did with the blocks handed to it since the last poll.
///
/// **Atomics rather than the shared accumulator**, the same trade
/// [`crate::hardware::soapy::stream::ReadLoopClock`] makes and for the same
/// reason: both answers are known only *after* `process_block`'s lock block has
/// closed, and reopening the mutex to record two integers would put a second
/// acquisition on the hot path for figures nothing reads until the next poll.
///
/// Both are reset by the poll that reads them, so each one is "this window".
#[derive(Default)]
pub struct FeedHealth {
    peak_depth: AtomicU64,
    dropped: AtomicU64,
}

impl FeedHealth {
    /// Record one hand-off: how deep the queue was left, and whether it took the
    /// block at all.
    ///
    /// **A high-water mark, not the latest reading.** The queue fills and drains
    /// in microseconds while the poll runs every 200 ms, so a depth read at poll
    /// time is a point sample of something that is almost always empty: it
    /// reported a comfortable 0 % straight through backlogs it never happened to
    /// land in.
    ///
    /// A block the queue could not take is the event the depth is a proxy for,
    /// and it is the only place that event is visible: the channel is lossy by
    /// design and carries no sequence number, so nothing downstream can tell that
    /// a block ever existed.
    pub fn record(&self, depth: usize, taken: bool) {
        self.peak_depth.fetch_max(depth as u64, Ordering::Relaxed);
        if !taken {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// `(deepest the queue got, blocks it refused)` since the last call, and
    /// start counting again.
    pub fn take(&self) -> (u64, u64) {
        (
            self.peak_depth.swap(0, Ordering::Relaxed),
            self.dropped.swap(0, Ordering::Relaxed),
        )
    }
}

/// A block of samples, plus the continuity facts a consumer that spans blocks
/// needs.
///
/// Named rather than a tuple because the two scalars answer the same question
/// from opposite sides, and only one of them is visible from the consumer.
///
/// `seq` catches a block lost **after** this point: the channels that carry
/// these are bounded and deliberately lossy, so a gap in the numbers is the only
/// record that a block existed at all. `gap_before` carries the loss the
/// sequence number cannot see, because it happened **before** the block was ever
/// stamped - samples the driver itself lost, from a HackRF short transfer or a
/// SoapySDR overflow. Both mean the samples here do not continue the samples in
/// the block before them, and anything whose state spans blocks has to be told
/// so: a CTCSS detector needs half a second of unbroken audio to tell two tones
/// two hertz apart from each other, and a preamble correlator needs to know that
/// the run it is in the middle of is a run.
pub struct StreamBlock {
    /// Monotonic across every block stamped, forwarded or not.
    pub seq: u64,
    /// The driver reported lost samples immediately before this block.
    pub gap_before: bool,
    pub bytes: Vec<u8>,
    /// Where this block's first pair sits in the stream: every pair the
    /// radio has delivered since the stream began, lost ones included (see
    /// [`RxContext::stream_pairs`]). A block continues the one before it
    /// exactly when this equals the previous block's position plus its
    /// length; anything else is a gap, or - smaller than expected - a new
    /// stream.
    pub first_pair: u64,
    /// The tuning these samples were captured at.
    ///
    /// **Stamped at capture, not read at processing.** A worker behind the
    /// stream processes a block after the tuner may have moved on - a survey
    /// retunes every tenth of a second - and reading the tuning from the state
    /// at that point attributes the block to where the radio is now rather
    /// than where it was. On BLE that meant decoding one channel's samples
    /// with another channel's whitening.
    pub centre_hz: u64,
    /// The sample rate these samples were captured at, for the same reason.
    pub rate_hz: f64,
}

/// A [`StreamBlock`] with the wall clock at its arrival, for the one worker
/// that must know when its samples were taken: FT8's cycles start on the UTC
/// quarter minute. Stamped on the driver's thread, in
/// `process::arrive`, before any queue could hold the block.
pub struct TimedBlock {
    pub block: StreamBlock,
    /// Unix seconds when the block reached the intake: after its last sample
    /// was taken, by however long the transfer and the driver held it.
    pub arrived_unix: f64,
}

/// What a radio settled on after a rate change.
///
/// Two answers to one call, because they are two facts about the same act and
/// two trait methods would let a call site record one and forget the other.
///
/// Not re-exported from `hardware`: it exists to be returned and destructured,
/// and no call site has to name it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RateSet {
    /// The rate the radio is running at.
    ///
    /// **Read back from the driver wherever there is a getter to read.** A
    /// driver is free to round a request onto its own clock grid and say
    /// nothing about it: an RTL-SDR's rate is 28.8 MHz over an integer divider,
    /// and a SoapySDR device may only offer a fixed list. Every frequency on
    /// screen is derived from this figure, so recording the request as if it
    /// were the fact mislabels every bin, every marker and every demod offset
    /// by the ratio between them.
    ///
    /// libhackrf has no getter, so on a HackRF this is still what was asked
    /// for, and [`RateSet::new`] is told so.
    pub rate_hz: f64,
    /// Baseband filter bandwidth applied (Hz), or 0 where the device has none.
    pub bb_filter_hz: u32,
}

impl RateSet {
    /// What to record, given what was asked for and whatever the driver said
    /// when it was asked back.
    ///
    /// `None` is a backend whose driver has no getter at all. A `Some` that is
    /// not a rate - zero, negative, or not a number - is a driver that was asked
    /// and did not answer, which is librtlsdr's shape for a failure and the
    /// shape a SoapySDR driver's unimplemented `getSampleRate` returns. Both
    /// leave the request standing, because the alternative is a sample rate of
    /// zero dividing every frequency axis in the app.
    pub fn new(requested: f64, readback: Option<f64>, bb_filter_hz: u32) -> Self {
        let rate_hz = match readback {
            Some(hz) if hz.is_finite() && hz > 0.0 => hz,
            _ => requested,
        };
        Self {
            rate_hz,
            bb_filter_hz,
        }
    }
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

    /// Tune to `hz` and say where the radio is tuned: the frequency at the
    /// centre of the samples from now on. That is `hz` on a radio that tunes
    /// to the hertz, and the nearest step of a radio that cannot: an Airspy
    /// HF+ with its library's correction off tunes in whole kHz. Every caller
    /// keeps what comes back, never what it asked for, so no frequency on
    /// screen is one the samples are not centred on.
    fn set_frequency(&self, hz: u64) -> anyhow::Result<u64>;
    /// Set the sample rate and report what the radio settled on.
    ///
    /// An implementation asks the driver back where the driver can be asked,
    /// and builds its answer through [`RateSet::new`] so that a getter which
    /// declines is handled the same way everywhere. What comes back is what the
    /// app records as the rate, not what was passed in.
    fn set_sample_rate(&self, hz: f64) -> anyhow::Result<RateSet>;

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

    fn set_direct_sweep(&self, _config: Option<DirectSweepConfig>) -> anyhow::Result<()> {
        anyhow::bail!("this backend does not support direct power sweeps")
    }

    fn options(&self) -> Vec<DeviceOption> {
        Vec::new()
    }

    fn set_option(&self, _id: &str, _choice: &str) -> anyhow::Result<()> {
        anyhow::bail!("this backend has no device options")
    }

    /// Whether setting option `id` can move where the radio is tuned, so the
    /// app sets the frequency again after it and keeps what comes back. No
    /// option does on any radio but one: the Airspy HF+'s library DSP, off,
    /// puts the centre on the firmware's kHz grid.
    fn option_moves_tuning(&self, _id: &str) -> bool {
        false
    }

    /// Update backend-owned settings before the app saves its config snapshot.
    fn update_config(&self, _config: &mut crate::config::AppConfig) -> anyhow::Result<()> {
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

    /// What the radio says about samples it dropped itself, before any of
    /// them reached the host.
    ///
    /// Three answers, kept apart. `Ok(None)`: this radio keeps no such count,
    /// and there is nothing to show. `Err(why)`: it does, and it could not be
    /// asked, which is itself worth showing. `Ok(Some(..))`: the count.
    ///
    /// A drop inside the radio leaves no hole in the stream positions: the
    /// driver hands over whole buffers either side of it. This count is the
    /// only way sdrtop learns it happened.
    fn radio_drops(&self) -> Result<Option<RadioDrops>, String> {
        Ok(None)
    }

    /// IQ pairs in one delivery, feeding the expected callback period in
    /// [`crate::state::TimingState`].
    ///
    /// The default is the figure [`DeviceCapabilities`] declares, which is the
    /// whole answer for a backend that chose its own transfer size and knew it
    /// at open. A pull backend overrides it, because the block it reads is not
    /// settled until a stream exists: the driver's MTU may be smaller than the
    /// one we ask for, and every figure on the timing bench is measured against
    /// this number.
    ///
    /// One accessor rather than a field the caller may or may not remember to
    /// correct, so there is one account of how big a block is.
    fn samples_per_transfer(&self) -> u64 {
        self.capabilities().samples_per_transfer
    }

    /// Bytes the radio's link carries for each I/Q pair delivered, when the
    /// backend knows its wire format; `None` when it does not.
    ///
    /// What the timing bench calls bus throughput. The default is the pairs
    /// as delivered, which is the wire for a radio that hands over its own
    /// samples (a HackRF's int8, an RTL-SDR's uint8). A backend whose library
    /// converts on the host says what crossed the link, and one that cannot
    /// know says so, rather than calling a delivery figure a bus figure.
    fn wire_bytes_per_pair(&self) -> Option<f64> {
        Some(self.capabilities().sample_geometry.bytes_per_pair() as f64)
    }

    /// Where the radio's local oscillator sits in the delivered spectrum, Hz
    /// from its centre, at the tuning and rate it has now.
    ///
    /// Zero for a radio that mixes the tuned frequency straight to 0 Hz, which
    /// is every radio but one: libairspyhf puts the HF+'s LO 5 kHz off the
    /// asked frequency at its zero-IF rates and rotates the samples back, and
    /// below its lowest LO it holds the LO there and rotates further. What is
    /// measured "at the LO" (its leak, the axis an image mirrors about) is
    /// measured here, not at the centre.
    fn lo_offset_hz(&self) -> i64 {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::native::{hackrf, rtlsdr};

    // ── What the FFT feed did with a block ──────────────────────────────────

    /// The depth has to be the deepest the queue got, not the depth at some
    /// arbitrary later instant. A backlog that builds and drains between two
    /// polls is exactly the event worth reporting, and a point sample is what
    /// used to miss it.
    #[test]
    fn the_feed_reports_the_deepest_the_queue_got_and_not_the_last_reading() {
        let feed = FeedHealth::default();
        feed.record(1, true);
        feed.record(3, true);
        feed.record(0, true);
        assert_eq!(feed.take(), (3, 0));
    }

    /// A block the queue refused is counted, and reading the counters starts the
    /// next window rather than accumulating into it.
    #[test]
    fn a_refused_block_is_counted_and_the_window_starts_again() {
        let feed = FeedHealth::default();
        feed.record(4, false);
        feed.record(4, false);
        assert_eq!(feed.take(), (4, 2));
        assert_eq!(feed.take(), (0, 0), "the window it reported is over");
    }

    // ── The rate a radio settled on ─────────────────────────────────────────

    /// A driver that rounds a request onto its own grid is the authority on
    /// what it did. Asking for 2.5 Msps on an RTL-SDR gets a divider, and the
    /// divider is the rate every bin frequency is computed from.
    #[test]
    fn a_driver_that_reports_its_own_rate_is_believed_over_the_request() {
        let set = RateSet::new(2_500_000.0, Some(2_400_000.0), 0);
        assert_eq!(set.rate_hz, 2_400_000.0);
        assert_eq!(set.bb_filter_hz, 0);
    }

    /// libhackrf has no getter. Nothing was asked, so nothing overrides the
    /// request, and the baseband width the device did report still comes back.
    #[test]
    fn a_driver_with_no_getter_leaves_the_request_standing() {
        let set = RateSet::new(10_000_000.0, None, 8_000_000);
        assert_eq!(set.rate_hz, 10_000_000.0);
        assert_eq!(set.bb_filter_hz, 8_000_000);
    }

    /// The one that matters. `rtlsdr_get_sample_rate` returns 0 when it fails,
    /// and a driver is free to return anything at all. A rate of zero divides
    /// the FFT span, the markers and the demod offsets, so a non-answer must
    /// read as a non-answer rather than as a rate of nothing.
    #[test]
    fn a_readback_that_is_not_a_rate_is_not_an_answer() {
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let set = RateSet::new(2_400_000.0, Some(bad), 0);
            assert_eq!(set.rate_hz, 2_400_000.0, "readback {bad}");
        }
    }

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

    /// The reason `positions` exists: an element whose step
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
    /// wrong bit count on the RF bench for every such radio.
    /// Only an unsigned format has a zero that is not a code. Half a count, and
    /// half a count is 0.0039 of full scale on an 8-bit radio: enough to sit at
    /// 78 % of the DC offset the IQ bench warns at, which is why it is added
    /// back rather than shrugged off.
    #[test]
    fn only_an_unsigned_format_is_centred_off_a_code() {
        let g = |format| SampleGeometry {
            format,
            full_scale: 128.0,
            converter_bits: None,
        };
        assert_eq!(g(SampleFormat::Uint8).centre_bias(), 0.5);
        assert_eq!(g(SampleFormat::Int8).centre_bias(), 0.0);
        assert_eq!(g(SampleFormat::Int16).centre_bias(), 0.0);
        assert_eq!(g(SampleFormat::Int32).centre_bias(), 0.0);
    }

    /// A 16-bit converter's words with eight fraction bits under them: eight
    /// bytes a pair, the converter's 16 bits (not the 24 the full scale would
    /// derive), 256 counts to one of its words. Every other format's word is
    /// one count, whatever its converter, so nothing shipped moves.
    #[test]
    fn a_wide_container_carries_its_converters_words_with_fractions() {
        let wide = SampleGeometry {
            format: SampleFormat::Int32,
            full_scale: 8_388_608.0,
            converter_bits: Some(16),
        };
        assert_eq!(wide.bytes_per_pair(), 8);
        assert_eq!(wide.bits(), 16);
        assert_eq!(wide.counts_per_word(), 256);
        let undeclared = SampleGeometry {
            converter_bits: None,
            ..wide
        };
        assert_eq!(undeclared.bits(), 24, "the full scale's own depth");
        assert_eq!(undeclared.counts_per_word(), 1);
        let r2 = SampleGeometry {
            format: SampleFormat::Int16,
            full_scale: 16384.0,
            converter_bits: Some(12),
        };
        assert_eq!(r2.counts_per_word(), 1, "whole words, filtered, not scaled");
        for format in [SampleFormat::Int8, SampleFormat::Uint8] {
            let g = SampleGeometry {
                format,
                full_scale: 128.0,
                converter_bits: None,
            };
            assert_eq!(g.counts_per_word(), 1);
        }
    }

    #[test]
    fn bit_depth_follows_full_scale() {
        let wide = |fs| SampleGeometry {
            format: SampleFormat::Int16,
            full_scale: fs,
            converter_bits: None,
        };
        assert_eq!(
            SampleGeometry {
                format: SampleFormat::Int8,
                full_scale: 128.0,
                converter_bits: None,
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

    /// The Airspy R2's LNA as measured at 92.8 MHz, valid over the VHF band
    /// the bench checked it in.
    fn vhf_lna() -> StageSpec {
        StageSpec::tabled(
            "LNA",
            vec![
                0.0, 4.38, 9.09, 11.28, 11.75, 13.52, 16.74, 19.96, 23.40, 26.96, 27.76, 28.19,
                28.64, 32.34, 36.02,
            ],
        )
        .measured_between(88_000_000, 170_000_000)
    }

    /// **Inside its band a setting is a gain; outside, a position.** The
    /// edges belong to the band. A stage with no band is a gain everywhere.
    #[test]
    fn a_stage_reads_in_db_only_where_it_was_measured() {
        let lna = vhf_lna();
        assert_eq!(lna.reading(36.02, 92_800_000), StageReading::Db(36.02));
        assert_eq!(lna.reading(36.02, 88_000_000), StageReading::Db(36.02));
        assert_eq!(lna.reading(36.02, 170_000_000), StageReading::Db(36.02));
        assert_eq!(
            lna.reading(36.02, 943_400_000),
            StageReading::Position { at: 14, of: 14 }
        );
        assert_eq!(
            lna.reading(11.28, 465_000_000),
            StageReading::Position { at: 3, of: 14 }
        );
        assert!(!lna.db_holds_at(170_000_001));
        let vga = StageSpec::ranged("VGA", 0.0, 62.0, 2.0);
        assert_eq!(vga.reading(30.0, 2_400_000_000), StageReading::Db(30.0));
    }

    /// A position is counted the same way for a table and for a grid; a
    /// continuous stage has none.
    #[test]
    fn a_position_is_counted_in_tables_and_grids() {
        assert_eq!(vhf_lna().position_of(0.0), Some(0));
        assert_eq!(vhf_lna().position_of(23.40), Some(8));
        assert_eq!(
            StageSpec::ranged("VGA", 0.0, 62.0, 2.0).position_of(30.0),
            Some(15)
        );
        assert_eq!(
            StageSpec::ranged("IF", 0.0, 50.0, 0.0).position_of(10.0),
            None
        );
    }

    /// Five columns either way, so the header and the rail keep their
    /// layout whichever a stage reads as.
    #[test]
    fn a_reading_fits_five_columns() {
        assert_eq!(StageReading::Db(36.02).compact(), "36 dB");
        assert_eq!(StageReading::Db(4.38).compact(), " 4 dB");
        assert_eq!(StageReading::Position { at: 14, of: 14 }.compact(), "14/14");
        assert_eq!(StageReading::Position { at: 3, of: 14 }.compact(), " 3/14");
        // An attenuator reads as the negative gain it is, in the same five
        // columns.
        assert_eq!(StageReading::Db(-12.0).compact(), "-12dB");
        assert_eq!(StageReading::Db(-48.0).compact(), "-48dB");
        assert_eq!(StageReading::Db(-6.0).compact(), " -6dB");
        assert_eq!(StageReading::Db(0.0).compact(), " 0 dB");
    }

    /// **A converter scaled into a wider container says its own depth.**
    /// libairspy's R2 stream has a full scale of 16384 counts, which derives
    /// to 15 bits, from a 12-bit converter; the ADC bench must call it 12. A
    /// stated depth is still capped by the container.
    #[test]
    fn a_scaled_converter_says_its_own_depth() {
        let r2 = SampleGeometry {
            format: SampleFormat::Int16,
            full_scale: 16384.0,
            converter_bits: Some(12),
        };
        assert_eq!(r2.bits(), 12);
        let impossible = SampleGeometry {
            converter_bits: Some(20),
            ..r2
        };
        assert_eq!(impossible.bits(), 16, "no 20 bits in an int16");
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
                converter_bits: None,
            }
            .bits(),
            16
        );
        // SoapyHackRF: `CS8 [full-scale=128]`, which is 8 either way.
        assert_eq!(
            SampleGeometry {
                format: SampleFormat::Int8,
                full_scale: 128.0,
                converter_bits: None,
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
                converter_bits: None,
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
                converter_bits: None,
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

    /// The HF+'s two bands, on a capability set that is otherwise a HackRF's:
    /// 9 kHz to 260 MHz with 31 to 60 MHz cut out.
    fn two_bands() -> DeviceCapabilities {
        DeviceCapabilities {
            freq_min_hz: 9_000,
            freq_max_hz: 260_000_000,
            freq_gaps: vec![std::ops::RangeInclusive::new(31_000_001, 59_999_999)],
            ..crate::hardware::native::hackrf::caps()
        }
    }

    /// A frequency in the gap is not tuned, its edges are, and so is the
    /// hull's; outside the hull nothing is. A radio with no gap tunes its
    /// whole hull, as every radio did before gaps.
    #[test]
    fn a_gap_is_not_tuned_and_its_edges_are() {
        let c = two_bands();
        for hz in [9_000, 31_000_000, 60_000_000, 260_000_000, 7_074_000] {
            assert!(c.tunes(hz), "{hz}");
        }
        for hz in [8_999, 31_000_001, 45_000_000, 59_999_999, 260_000_001] {
            assert!(!c.tunes(hz), "{hz}");
        }
        assert!(c.in_gap(45_000_000) && !c.in_gap(31_000_000) && !c.in_gap(8_000));
        let one = crate::hardware::native::hackrf::caps();
        assert!(one.freq_gaps.is_empty());
        assert!(one.tunes(one.freq_min_hz) && one.tunes(one.freq_max_hz));
        assert!(!one.tunes(one.freq_max_hz + 1));
    }

    /// The bands are the hull with the gaps cut out; one band without them.
    #[test]
    fn the_bands_are_the_hull_less_its_gaps() {
        assert_eq!(
            two_bands().bands(),
            vec![9_000..=31_000_000, 60_000_000..=260_000_000]
        );
        let one = crate::hardware::native::hackrf::caps();
        assert_eq!(one.bands(), vec![one.freq_min_hz..=one.freq_max_hz]);
    }

    /// The arrows stop at a band's edge as at the hull's; they do not step
    /// into the gap nor jump over it.
    #[test]
    fn a_step_stops_at_its_bands_edge() {
        let c = two_bands();
        assert_eq!(c.step_within_band(30_950_000, 100_000, true), 31_000_000);
        assert_eq!(c.step_within_band(31_000_000, 100_000, true), 31_000_000);
        assert_eq!(c.step_within_band(60_050_000, 100_000, false), 60_000_000);
        assert_eq!(c.step_within_band(60_000_000, 100_000, true), 60_100_000);
        assert_eq!(c.step_within_band(9_000, 100_000, false), 9_000);
        // From inside the gap (a config written elsewhere), the band in the
        // step's direction.
        assert_eq!(c.step_within_band(45_000_000, 100_000, true), 60_000_000);
        assert_eq!(c.step_within_band(45_000_000, 100_000, false), 31_000_000);
        let one = crate::hardware::native::hackrf::caps();
        assert_eq!(
            one.step_within_band(one.freq_max_hz - 10, 100_000, true),
            one.freq_max_hz,
            "the hull's edge, as before"
        );
    }

    /// The bands in words, each edge in the unit it reaches.
    #[test]
    fn the_bands_are_said_in_words() {
        assert_eq!(two_bands().bands_text(), "9 kHz to 31 MHz, 60 to 260 MHz");
        assert_eq!(
            crate::hardware::native::hackrf::caps().bands_text(),
            "1 MHz to 6 GHz"
        );
        let rtl = DeviceCapabilities {
            freq_min_hz: 24_000_000,
            freq_max_hz: 1_766_000_000,
            ..crate::hardware::native::hackrf::caps()
        };
        assert_eq!(rtl.bands_text(), "24 MHz to 1.766 GHz");
    }

    /// A device's own start gains are where a first run and `[R]` put the
    /// stages, snapped onto their values, rather than the historical front
    /// and second-stage defaults (which would switch a 0/6 dB preamp on).
    #[test]
    fn a_device_can_start_its_stages_where_it_says() {
        let stages = vec![
            StageSpec::tabled("ATT", (0..=8).map(|i| f64::from(i) * 6.0 - 48.0).collect()),
            StageSpec::tabled("LNA", vec![0.0, 6.0]),
        ];
        let plain = GainModel::new(stages.clone(), "ATT", "ATT");
        assert_eq!(plain.start_gains(None), [0.0, 6.0], "the historical rule");
        let own = GainModel::new(stages, "ATT", "ATT").with_start_gains(vec![0.0, 0.0]);
        assert_eq!(own.start_gains(None), [0.0, 0.0]);
    }

    /// Each stream begun is counted, and its positions start from zero.
    #[test]
    fn beginning_a_stream_counts_it_and_restarts_its_positions() {
        let (ctx, _f, _d, _n) = crate::hardware::process::tests::rx_ctx();
        ctx.stream_pairs
            .store(4096, std::sync::atomic::Ordering::Relaxed);
        let before = ctx.streams_begun.load(std::sync::atomic::Ordering::Relaxed);
        ctx.begin_stream();
        assert_eq!(
            ctx.stream_pairs.load(std::sync::atomic::Ordering::Relaxed),
            0
        );
        assert_eq!(
            ctx.streams_begun.load(std::sync::atomic::Ordering::Relaxed),
            before + 1
        );
    }

    /// A radio with a list of rates steps through it: from a listed rate to
    /// the next, from between two to the nearer one that way, and stops at
    /// either end; a radio with no list has no steps.
    #[test]
    fn rates_step_through_a_list_and_stop_at_its_ends() {
        let hf = crate::hardware::native::airspyhf::caps(
            &[
                912_000, 768_000, 650_000, 456_000, 384_000, 228_000, 192_000,
            ],
            2048,
        );
        assert_eq!(
            hf.sample_rates,
            vec![192e3, 228e3, 384e3, 456e3, 650e3, 768e3, 912e3]
        );
        assert_eq!(hf.rate_step(768e3, true), Some(912e3));
        assert_eq!(hf.rate_step(768e3, false), Some(650e3));
        assert_eq!(hf.rate_step(912e3, true), Some(912e3));
        assert_eq!(hf.rate_step(192e3, false), Some(192e3));
        assert_eq!(hf.rate_step(700e3, true), Some(768e3));
        assert_eq!(hf.rate_step(700e3, false), Some(650e3));
        // A hair under a listed rate is that rate, not the one below it.
        assert_eq!(hf.rate_step(767_999.9, false), Some(650e3));
        let r2 = crate::hardware::native::airspy::caps();
        assert_eq!(r2.sample_rates, vec![2.5e6, 10e6]);
        assert_eq!(r2.rate_step(2.5e6, true), Some(10e6));
        assert_eq!(
            crate::hardware::native::hackrf::caps().rate_step(8e6, true),
            None
        );
    }
}
