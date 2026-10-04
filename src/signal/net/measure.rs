// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The measurement path: a burst that detection found, read again from the
//! raw samples, through a Bluetooth tester's own filter, at the centres of
//! its bits.
//!
//! **Detection and measurement want different things, so they are two
//! paths.** Detection runs continuously on every watched channel and wants
//! whatever finds packets: filters narrow enough to keep a neighbour out,
//! four lanes because it does not know where a bit's centre is. A figure
//! wants what a tester reads. The test suites say what that is (RF.TS.p35
//! for BR, RF-PHY.TS.4.2.1 for LE, both read for `conformance`): a filter
//! flat to 550 kHz and the frequency at the bit, timed from the packet's own
//! known bits. Measured against the reference, the detection path read an
//! ideal BR transmitter's df2/df1 as 0.28 to 0.54 where it sends 0.88: its
//! lane sits early in the bit, and its filter takes a fifth of df2 even at
//! the centre.
//!
//! **What this does for a burst:**
//! 1. The raw samples around it are taken from the blocks the worker still
//!    holds ([`Recent`]), by stream position, without copying a block.
//! 2. They are mixed to baseband, put through [`tester_filter`] and
//!    decimated to [`MEASURE_RATE_HZ`].
//! 3. They are read between samples as the band-limited signal they are
//!    ([`Oversampled`]).
//! 4. The timing is fitted to the packet's known bits (the classic sync
//!    word), not taken from a lane.
//!
//! All of it runs per detected burst, never on the continuous stream, and
//! is plain data in and out: `dev_docs/measurement-path-design.md` has the
//! reasoning and Viktor's choices.

use num_complex::Complex;

use crate::signal::ble::measure::{
    drift_from, modulation_from, CodedInitial, CodedModulation, Drift, ModulationQuality,
    CODED_DELTA_F1_MAX_LIMIT_HZ,
};
use crate::signal::ble::Phy;
use crate::signal::bt::piconet::Deviation;
use crate::signal::dsp::deviation::{suite_readings_from, BitReadings};
use crate::signal::dsp::discriminate::Oversampled;
use crate::signal::dsp::fir::{design_lowpass_to_spec, StreamingDecimator};
use crate::signal::dsp::nco::Nco;

/// The measurement filter: flat to here...
const MEASURE_PASS_HZ: f64 = 1_250_000.0;
/// ...and at least 40 dB down from here on: the BLE receiver's own front
/// end's edges. The test suites' recommended filter (flat to 550 kHz, steep
/// by 650 kHz) is right for a tester reading its own patterns and wrong for
/// traffic: every filter it allows cuts into BR's spectrum, which lifts
/// df2 3 % on a continuous `1010` and lowers it 3 to 4 % on whitened
/// traffic. Through this one an ideal transmitter's traffic reads within
/// 0.5 % of the suites' own figures on their own patterns
/// (`conformance::traffic_reads_as_the_suites_test_patterns`); chosen by
/// Viktor, 2026-09-26. What it gives up, a BR neighbour 1 MHz away, moves
/// the figures under 1 % at 25 dB down (`conformance::a_neighbour_25_db_
/// down_moves_the_readings_under_one_percent`).
const MEASURE_STOP_HZ: f64 = 1_750_000.0;
/// Designed 2 dB past the 40 the test holds it to: at 4 Msps the filter is
/// short enough that Kaiser's rule lands a stopband peak at 39.3 dB.
const MEASURE_STOPBAND_DB: f64 = 42.0;

/// A classic header is not measured when one side of the channel carries
/// this much more power, a channel away, than the other side does, over
/// the channel's own power in the same bandwidth, in dB. What a neighbour
/// does to the readings through the wide filter was measured
/// (`conformance::a_neighbour_25_db_down_moves_the_readings_under_one_
/// percent`): under 1 % to 20 dB down, 4 % at 15. The other side is the
/// yardstick because the transmitter's own spectrum a channel away, 40 to
/// 44 dB down depending on its deviation, and the noise are the same on both
/// sides, where a neighbour is on one.
const NEIGHBOUR_LIMIT_DB: f64 = -20.0;

/// Classic channels are this far apart, so a neighbour's centre is here.
const CLASSIC_SPACING_HZ: f64 = 1_000_000.0;

/// The bandwidth a neighbour's power is read in, either side of its centre:
/// its main lobe, and little of the transmitter's own spectrum.
const GUARD_PASS_HZ: f64 = 100_000.0;
const GUARD_STOP_HZ: f64 = 250_000.0;
const GUARD_STOPBAND_DB: f64 = 50.0;

/// How much more power one side of the channel carries, `spacing` away,
/// than the other, over the channel's own power in the same bandwidth, in
/// dB: `iq` at `rate`, the channel at baseband. Minus infinity when the two
/// sides are equal. [`NEIGHBOUR_LIMIT_DB`] has why one side against the
/// other.
pub fn neighbour_excess_db(iq: &[Complex<f32>], rate: f64, spacing: f64) -> f64 {
    let taps = design_lowpass_to_spec(
        (GUARD_PASS_HZ + GUARD_STOP_HZ) / 2.0 / rate,
        (GUARD_STOP_HZ - GUARD_PASS_HZ) / rate,
        GUARD_STOPBAND_DB,
    );
    // A power needs no more than the band's own rate.
    let factor = (rate / (4.0 * GUARD_STOP_HZ)).floor().max(1.0) as usize;
    let band = |offset: f64| {
        let mut shifted = iq.to_vec();
        Nco::new(-offset, rate).mix(&mut shifted);
        let mut filter = StreamingDecimator::new(taps.clone(), factor);
        let mut out = Vec::new();
        filter.process(&shifted, &mut out);
        out.iter().map(|z| z.norm_sqr() as f64).sum::<f64>() / out.len().max(1) as f64
    };
    let (own, up, down) = (band(0.0), band(spacing), band(-spacing));
    10.0 * ((up - down).abs() / own).log10()
}

/// The rate the measurement filter decimates to: four samples a symbol at
/// 1 Msym/s, read between by [`Oversampled`].
const MEASURE_RATE_HZ: f64 = 4_000_000.0;

/// How much of the recent stream the worker holds for measuring, in seconds.
/// A classic header is handed over when its payload capture ends, 2 744 bits
/// after it: the header then lies about 3 ms back.
pub const HELD_S: f64 = 0.008;

/// The measurement filter at `rate`, built to the edges above;
/// `tests::the_measurement_filter_is_flat_to_its_edge_and_down_past_it`
/// measures it at each rate the chain runs at.
pub fn measurement_filter(rate: f64) -> Vec<f32> {
    design_lowpass_to_spec(
        (MEASURE_PASS_HZ + MEASURE_STOP_HZ) / 2.0 / rate,
        (MEASURE_STOP_HZ - MEASURE_PASS_HZ) / rate,
        MEASURE_STOPBAND_DB,
    )
}

/// The recent decoded blocks, each with its stream position, oldest first.
pub struct Recent<'a> {
    blocks: Vec<(u64, &'a [Complex<f32>])>,
}

impl<'a> Recent<'a> {
    pub fn new(blocks: impl IntoIterator<Item = (u64, &'a [Complex<f32>])>) -> Self {
        Self {
            blocks: blocks.into_iter().collect(),
        }
    }

    /// `len` samples from stream position `from`, or `None` unless every one
    /// of them is held: a window across a gap, or older than what is kept,
    /// is refused rather than stitched.
    pub fn slice(&self, from: u64, len: usize) -> Option<Vec<Complex<f32>>> {
        let end = from.checked_add(len as u64)?;
        let mut out = Vec::with_capacity(len);
        let mut at = from;
        for &(first, block) in &self.blocks {
            let last = first + block.len() as u64;
            if at == end {
                break;
            }
            if at < first || at >= last {
                continue;
            }
            let upto = end.min(last);
            out.extend_from_slice(&block[(at - first) as usize..(upto - first) as usize]);
            at = upto;
        }
        (at == end).then_some(out)
    }
}

/// A burst's frequency as the tester reads it, at any stream position
/// inside the window it was built over.
struct Tester {
    fine: Oversampled,
    /// The window through the tester's filter, at [`MEASURE_RATE_HZ`]: what
    /// [`Self::snr_db`] reads the envelope of.
    iq: Vec<Complex<f32>>,
    /// The stream position of the window's first raw sample.
    start: f64,
    /// The filter's delay, in raw samples.
    delay: f64,
    /// The decimation factor.
    factor: f64,
    /// [`neighbour_excess_db`] over `from..to`, when asked for.
    neighbour_db: Option<f64>,
}

impl Tester {
    /// The tester over stream positions `from` to `to` of a channel
    /// `offset_hz` from the tuning, at `rate`; `None` when the rate is not a
    /// whole multiple of [`MEASURE_RATE_HZ`] or the samples are not held.
    ///
    /// `neighbours`, when given, is the channel spacing whose neighbours'
    /// power is read over `from..to` ([`neighbour_excess_db`]).
    fn new(
        recent: &Recent,
        rate: f64,
        offset_hz: f64,
        from: f64,
        to: f64,
        neighbours: Option<f64>,
    ) -> Option<Self> {
        let factor = (rate / MEASURE_RATE_HZ).round().max(1.0);
        if (rate / factor - MEASURE_RATE_HZ).abs() > MEASURE_RATE_HZ * 0.01 {
            return None;
        }
        let taps = measurement_filter(rate);
        // The filter's whole span either side, and the rebuilt waveform's
        // own few samples at each end.
        let reach = taps.len() as f64 + 16.0 * factor;
        let start = (from - reach).floor();
        if start < 0.0 {
            return None;
        }
        let len = ((to + reach).ceil() - start) as usize;
        let mut iq = recent.slice(start as u64, len)?;
        // Frequency is measured, not phase, so the oscillator may start at
        // any phase: fresh for every burst.
        Nco::new(-offset_hz, rate).mix(&mut iq);
        let neighbour_db = neighbours.map(|spacing| {
            let burst = (from - start) as usize..((to - start) as usize).min(iq.len());
            neighbour_excess_db(&iq[burst], rate, spacing)
        });
        let mut filter = StreamingDecimator::new(taps, factor as usize);
        let delay = filter.delay();
        let mut out = Vec::new();
        filter.process(&iq, &mut out);
        Some(Self {
            fine: Oversampled::new(&out, rate / factor),
            iq: out,
            start,
            delay,
            factor,
            neighbour_db,
        })
    }

    /// The frequency at stream position `pos`, in Hz.
    fn at(&self, pos: f64) -> f32 {
        self.fine.at((pos - self.start - self.delay) / self.factor)
    }

    /// The SNR over stream positions `from` to `to`, in dB, from the
    /// envelope through the tester's filter (`estimate::snr_m2m4`). The
    /// filter has the BLE receiver's front end's edges, so this is read in
    /// the band LE 1M's SNR is.
    fn snr_db(&self, from: f64, to: f64) -> Option<f64> {
        let index = |pos: f64| ((pos - self.start - self.delay) / self.factor).round();
        let (a, b) = (index(from), index(to));
        if a < 0.0 || b <= a {
            return None;
        }
        let window = self.iq.get(a as usize..(b as usize).min(self.iq.len()))?;
        crate::signal::dsp::estimate::snr_m2m4(window).map(|snr| 10.0 * snr.log10())
    }
}

/// How finely [`timing`] searches, in steps a bit.
const TIMING_STEPS: i32 = 64;

/// Where the bits' centres are, as an offset in bits from `first` (the
/// estimated centre of `known[0]`), searched over one bit either way: the
/// offset at which the readings, less their mean, best agree with the known
/// bits' signs. A tester times a packet from its known start the same way
/// (the suites' "position of bit p0"); a lane never enters it.
fn timing(tester: &Tester, first: f64, bit: f64, known: &[bool]) -> f64 {
    // Every step's readings into one buffer, reused: the same numbers, in
    // the same order, without a fresh allocation for each of 129 steps.
    let hz = std::cell::RefCell::new(Vec::with_capacity(known.len()));
    let score = |tau: f64| {
        let mut hz = hz.borrow_mut();
        hz.clear();
        hz.extend((0..known.len()).map(|k| tester.at(first + (k as f64 + tau) * bit) as f64));
        let mean = hz.iter().sum::<f64>() / hz.len() as f64;
        hz.iter()
            .zip(known)
            .map(|(f, &b)| if b { f - mean } else { mean - f })
            .sum::<f64>()
    };
    let steps: Vec<(f64, f64)> = (-TIMING_STEPS..=TIMING_STEPS)
        .map(|s| {
            let tau = s as f64 / TIMING_STEPS as f64;
            (tau, score(tau))
        })
        .collect();
    let best = (0..steps.len())
        .max_by(|&a, &b| steps[a].1.total_cmp(&steps[b].1))
        .unwrap_or(TIMING_STEPS as usize);
    // A parabola through the best step and its neighbours places the peak
    // between steps.
    if best == 0 || best + 1 == steps.len() {
        return steps[best].0;
    }
    let (l, c, r) = (steps[best - 1].1, steps[best].1, steps[best + 1].1);
    let curve = l - 2.0 * c + r;
    let shift = if curve < 0.0 {
        (0.5 * (l - r) / curve).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    steps[best].0 + shift / TIMING_STEPS as f64
}

/// A burst lined up on its own bits: the tester over it, and where bit `x`
/// (bit `k` spans `k..k + 1`, `known[0]` being bit 0) sits in the stream.
struct Aligned {
    tester: Tester,
    first: f64,
    bit: f64,
    tau: f64,
}

impl Aligned {
    /// The frequency at bit position `x`, in Hz.
    fn at(&self, x: f64) -> f32 {
        self.tester.at(self.first + (x - 0.5 + self.tau) * self.bit)
    }
}

/// What lining a burst up came to.
enum Lined {
    Up(Aligned),
    /// A neighbour was louder than [`NEIGHBOUR_LIMIT_DB`] allows.
    NeighbourBusy,
}

/// The burst whose `known` bits start with the one centred near stream
/// position `first`, followed by `after` more bits, on a channel `offset_hz`
/// from the tuning at `rate`, timed from `known` ([`timing`]). `neighbours`,
/// when given, is the channel spacing to guard. `None` as [`Tester::new`]
/// refuses.
fn align(
    recent: &Recent,
    rate: f64,
    offset_hz: f64,
    known: &[bool],
    first: f64,
    after: usize,
    neighbours: Option<f64>,
) -> Option<Lined> {
    let bit = rate / 1e6;
    let last = first + (known.len() + after) as f64 * bit;
    let tester = Tester::new(
        recent,
        rate,
        offset_hz,
        first - 2.0 * bit,
        last + 2.0 * bit,
        neighbours,
    )?;
    if tester
        .neighbour_db
        .is_some_and(|db| db >= NEIGHBOUR_LIMIT_DB)
    {
        return Some(Lined::NeighbourBusy);
    }
    let tau = timing(&tester, first, bit, known);
    Some(Lined::Up(Aligned {
        tester,
        first,
        bit,
        tau,
    }))
}

/// One classic header as the test suites define its readings: the
/// modulation, and the carrier (f0 in Hz from the channel's nominal centre,
/// and its drift) where there were enough blocks to read one.
pub struct ClassicReading {
    pub deviation: Deviation,
    pub carrier: Option<(f64, Drift)>,
}

/// A classic header's readings: `lap`'s sync word ended near stream
/// position `sync_end_pair` on a channel `offset_hz` from the tuning.
///
/// **Every bit it reads is known or re-read here, none taken from the
/// detector.** The access code is known whole: the preamble alternates into
/// the sync word (Core 5.4 Vol 2 Part B 6.3.2) and the trailer out of it
/// (6.3.4), 72 bits that also time the burst. The header's 54 bits are
/// decided again from this measurement's own readings: its rate 1/3 code
/// sends every bit three times (7.4), so each triple is one decision, on
/// the three bits' mean readings together against the midpoint the known
/// bits give. The detector's own bits come from whichever lane fired,
/// which can sit at a bit's edge: one wrong bit among them put a whole
/// ten-bit block 32 kHz off, a drift of 25 kHz read where none was sent.
/// Deciding on the reading at each bit's centre instead, three by
/// majority, still lost a whole triple in about one header in four at
/// 20 dB (one reading there scatters 100 kHz, a bit's mean 27): 96 kHz of
/// false drift.
///
/// f0 is the preamble's four bits (RF.TS.p35 RF/TRM/CA/BV-08-C); the drift
/// blocks run from the second bit after it to the end of the header: the
/// access code and header, not the payload the suite reads, which is what
/// a header hit holds.
///
/// `None` when the samples are no longer held or the rate cannot be
/// decimated to [`MEASURE_RATE_HZ`]; a header that is not measured adds
/// nothing, rather than a reading from the detection path. A busy neighbour
/// gives a reading that says so and nothing else.
pub fn classic(
    recent: &Recent,
    rate: f64,
    offset_hz: f64,
    lap: u32,
    sync_end_pair: f64,
) -> Option<ClassicReading> {
    use crate::signal::bt::header::{HEADER_AIR_BITS, TRAILER_BITS};
    use crate::signal::dsp::carrier::{by_bit_from, initial, ten_bit_blocks};
    const PREAMBLE: usize = 4;
    let sync = crate::signal::bt::access_code::access_code_bits(lap);
    let (s0, s63) = (sync[0], sync[63]);
    let mut known = vec![s0, !s0, s0, !s0];
    known.extend(sync);
    known.extend([!s63, s63, !s63, s63]);
    debug_assert_eq!(known.len(), PREAMBLE + 64 + TRAILER_BITS);
    let first = sync_end_pair - (PREAMBLE + 63) as f64 * rate / 1e6;
    let aligned = match align(
        recent,
        rate,
        offset_hz,
        &known,
        first,
        HEADER_AIR_BITS,
        Some(CLASSIC_SPACING_HZ),
    )? {
        Lined::Up(aligned) => aligned,
        Lined::NeighbourBusy => {
            return Some(ClassicReading {
                deviation: Deviation::neighbour_busy(),
                carrier: None,
            })
        }
    };
    let at = |x: f64| aligned.at(x);
    // Every bit read once, before any is decided: a reading does not depend
    // on what the bit turns out to be, and the header's decisions, the
    // modulation and the carrier all read the same ones.
    let readings = BitReadings::read(known.len() + HEADER_AIR_BITS, at);
    let per_bit = |k: usize| readings.mean(k);
    // The header: each triple decided by its three bits' means together
    // against the known bits' midpoint.
    let side = |one: bool| {
        let v: Vec<f64> = (0..known.len())
            .filter(|&k| known[k] == one)
            .map(per_bit)
            .collect();
        v.iter().sum::<f64>() / v.len().max(1) as f64
    };
    let threshold = (side(true) + side(false)) / 2.0;
    let mut header = Vec::with_capacity(HEADER_AIR_BITS);
    for triple in 0..HEADER_AIR_BITS / 3 {
        let k = known.len() + 3 * triple;
        let sum: f64 = (k..k + 3).map(|j| per_bit(j) - threshold).sum();
        header.extend([sum > 0.0; 3]);
    }
    let all: Vec<bool> = known.iter().chain(&header).copied().collect();
    let deviation = suite_readings_from(&all[PREAMBLE..], &readings.tail_from(PREAMBLE))
        .map(|(settled, alternating)| Deviation::from_readings(&settled, &alternating))?;
    let f0 = initial(at, 0, PREAMBLE);
    let blocks = ten_bit_blocks(&by_bit_from(&all, &readings), PREAMBLE + 1, all.len() - 1);
    let carrier = crate::signal::dsp::carrier::drift_from(Some((f0, PREAMBLE)), &blocks, 1e6)
        .map(|drift| (f0, drift));
    Some(ClassicReading { deviation, carrier })
}

/// An LE 1M packet's modulation and carrier, as the test suites define
/// them: `air` is its PDU as sent (header through CRC, whitened), whose
/// first bit the receiver's slicer centred at stream position `pdu_pair`, on
/// a channel `offset_hz` from the tuning. Timed by the preamble and the
/// advertising access address before it, the 40 bits every such packet
/// starts with; f0 from the preamble's 8 bits, the drift blocks from the
/// PDU's second bit to its CRC (RF-PHY.TS.4.2.1 TP/TRM-LE/CA/BV-06-C).
///
/// LE 1M only: LE 2M is read by the receiver (`ble::receive`), whose
/// capture does not hold the samples 2M's rate needs from here. `None` as
/// [`classic`] refuses.
pub fn le_1m(
    recent: &Recent,
    rate: f64,
    offset_hz: f64,
    pdu_pair: f64,
    air: &[bool],
) -> Option<(Option<ModulationQuality>, Option<Drift>)> {
    use crate::signal::ble::detect::{
        access_address_bits, preamble_bits, ADVERTISING_ACCESS_ADDRESS,
    };
    use crate::signal::dsp::carrier::{by_bit_from, initial, ten_bit_blocks};
    let preamble = preamble_bits(ADVERTISING_ACCESS_ADDRESS, Phy::OneM);
    let mut known = preamble.clone();
    known.extend(access_address_bits(ADVERTISING_ACCESS_ADDRESS));
    let first = pdu_pair - known.len() as f64 * rate / 1e6;
    // No guard: LE's neighbours are 2 MHz away, in the measurement filter's
    // stopband, and one 15 dB down moves the readings 0.3 %
    // (`conformance::a_neighbour_25_db_down_moves_the_readings_under_one_
    // percent`, which also runs LE at 15).
    let Lined::Up(aligned) = align(recent, rate, offset_hz, &known, first, air.len(), None)? else {
        return None;
    };
    let all: Vec<bool> = known.iter().chain(air).copied().collect();
    // Every bit read once, for the modulation and the carrier both.
    let readings = BitReadings::read(all.len(), |x| aligned.at(x));
    let modulation = suite_readings_from(&all, &readings)
        .and_then(|(settled, alternating)| modulation_from(&settled, &alternating, Phy::OneM));
    let f0 = initial(|x| aligned.at(x), 0, preamble.len());
    let carrier = by_bit_from(&all, &readings);
    let pdu = known.len();
    let crc = pdu + air.len().saturating_sub(crate::signal::ble::pdu::CRC_BITS);
    let blocks = ten_bit_blocks(&carrier, pdu + 1, crc);
    Some((
        modulation,
        drift_from(Some((f0, preamble.len())), &blocks, Phy::OneM),
    ))
}

/// What the measurement path reads of an LE Coded packet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CodedReading {
    /// Over the whole packet, in dB, in LE 1M's band ([`Tester::snr_db`]).
    pub snr_db: Option<f64>,
    /// RFPHY/TRM/BV-13-C; S=8 only.
    pub modulation: Option<CodedModulation>,
    /// RFPHY/TRM/BV-14-C's preamble groups; S=8 only.
    pub initial: Option<CodedInitial>,
    /// RFPHY/TRM/BV-14-C's carrier through the payload, in LE 1M's terms
    /// (`dsp::carrier::coded_drift_from`); S=8 only, and `None` on a payload
    /// too short for a 48 us step.
    pub drift: Option<Drift>,
}

/// An LE Coded packet's readings: `symbols` is the whole packet as sent,
/// preamble through TERM2 (the receiver's decode encoded again), whose
/// first symbol starts near stream position `start_pair`, on a channel
/// `offset_hz` from the tuning. Timed from the sync symbols (the preamble
/// and the coded access address, the 336 every advertising packet starts
/// with), through the tester's filter, as LE 1M's are.
///
/// **S=8 only for the suites' figures.** RFPHY.TS defines LE Coded's
/// modulation (BV-13-C) and carrier (BV-14-C) for S=8 and nothing for S=2,
/// so an S=2 packet gets its SNR and no figure a suite has not defined.
///
/// **The test packets' patterns, read from traffic.** BV-13-C reads Δf1 on
/// a payload of `00111100` symbols; every `00001111` in a packet, preamble
/// or data, is the same pattern, and its middle symbols are the ones whose
/// neighbours both equal them (`dsp::deviation::suite_readings_from`, as
/// LE 1M's traffic is read). BV-14-C's preamble groups need nothing but the
/// preamble, which every Coded packet has. `None` as [`le_1m`] refuses.
pub fn le_coded(
    recent: &Recent,
    rate: f64,
    offset_hz: f64,
    start_pair: f64,
    symbols: &[bool],
    coding: crate::signal::ble::coded::Coding,
) -> Option<CodedReading> {
    use crate::signal::ble::coded::{self, Coding};
    use crate::signal::dsp::uncertainty::mean_with_uncertainty;
    let sync = coded::PREAMBLE_SYMBOLS + 256;
    let known = symbols.get(..sync)?;
    let symbol = rate / 1e6;
    let first = start_pair + 0.5 * symbol;
    let Lined::Up(aligned) = align(
        recent,
        rate,
        offset_hz,
        known,
        first,
        symbols.len() - sync,
        None,
    )?
    else {
        return None;
    };
    let start = aligned.first + (aligned.tau - 0.5) * symbol;
    let snr_db = aligned
        .tester
        .snr_db(start, start + symbols.len() as f64 * symbol);
    if coding == Coding::S2 {
        return Some(CodedReading {
            snr_db,
            modulation: None,
            initial: None,
            drift: None,
        });
    }
    let readings = BitReadings::read(symbols.len(), |x| aligned.at(x));
    // BV-14-C: four groups of 16 from the preamble's third symbol.
    let group = |g: usize| {
        (2 + 16 * g..18 + 16 * g)
            .map(|k| readings.mean(k))
            .sum::<f64>()
            / 16.0
    };
    let initial = CodedInitial {
        groups_hz: [group(0), group(1), group(2), group(3)],
    };
    // BV-14-C's payload groups: 16 symbols each, from the PDU payload to the
    // CRC, each the mean of the carrier under its symbols. The suite
    // integrates the frequency itself, on a test payload of `00111100`
    // repeated, whose every 16 symbols average to the carrier. On traffic
    // they do not: the Gaussian filter spills each group's edge symbols into
    // its neighbours, and a plain mean moved by about 3 kHz with the data,
    // which the steepest 48 us change then found, reading 200 Hz/us as 400.
    // So the carrier is read under each symbol, its own and its neighbours'
    // pull taken out (`dsp::carrier::by_bit_from`, as LE 1M's blocks are), and
    // averaged; on the suite's payload that is its integral. The groups start
    // on the 4-symbol pattern boundaries, at the payload's 29th symbol, two
    // after the suite's 27th.
    let carrier = crate::signal::dsp::carrier::by_bit_from(symbols, &readings);
    let block2 = coded::PREAMBLE_SYMBOLS + coded::BLOCK1_SYMBOLS;
    let header = 16 * Coding::S8.symbols_per_bit();
    let trailer = (24 + 3) * Coding::S8.symbols_per_bit();
    let crc = symbols.len().saturating_sub(trailer);
    let payload: Vec<f64> = (block2 + header + 28..)
        .step_by(16)
        .take_while(|&k| k + 16 <= crc)
        // A group with a symbol whose carrier could not be read ends the
        // groups there: the steps between them are taken as 16 us each.
        .map_while(|k| {
            let group: Option<Vec<f64>> = carrier[k..k + 16].iter().copied().collect();
            group.map(|g| g.iter().sum::<f64>() / 16.0)
        })
        .collect();
    let drift = crate::signal::dsp::carrier::coded_drift_from(initial.groups_hz, &payload);
    let modulation = suite_readings_from(symbols, &readings).and_then(|(settled, _)| {
        (!settled.is_empty()).then(|| CodedModulation {
            delta_f1_avg_hz: mean_with_uncertainty(&settled),
            share_f1max_above_limit: settled
                .iter()
                .filter(|&&v| v as f64 > CODED_DELTA_F1_MAX_LIMIT_HZ)
                .count() as f64
                / settled.len() as f64,
        })
    });
    Some(CodedReading {
        snr_db,
        modulation,
        initial: Some(initial),
        drift,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::dsp::testkit::Rng;
    use crate::signal::net::conformance::{self as reference, Burst, Gfsk, Trace};

    /// The production filter against its edges, at every rate the chain runs
    /// at: within 0.15 dB to 1.25 MHz (a 40 dB Kaiser ripples about 0.09 dB,
    /// 0.10 at the edge), 40 dB down from 1.75 MHz to the Nyquist frequency.
    /// Measured, not trusted to the design formula.
    #[test]
    fn the_measurement_filter_is_flat_to_its_edge_and_down_past_it() {
        for rate in [4e6, 8e6, 20e6] {
            let taps: Vec<f64> = measurement_filter(rate).iter().map(|&t| t as f64).collect();
            for i in 0..=125 {
                let db = reference::response_db(&taps, i as f64 * 10_000.0, rate);
                assert!(db.abs() < 0.15, "{rate}: {db} dB at {} kHz", i * 10);
            }
            let mut hz = 1.75e6;
            while hz <= rate / 2.0 {
                let db = reference::response_db(&taps, hz, rate);
                assert!(db <= -40.0, "{rate}: {db} dB at {hz}");
                hz += 25e3;
            }
        }
    }

    /// An LE 1M packet off the tuned centre, handed over with its PDU's
    /// start misplaced by up to 0.3 of a bit: timed back from its preamble
    /// and access address, it reads the suites' figures as the reference
    /// reads the same bits, and its carrier as sent.
    #[test]
    fn an_le_packet_reads_as_the_suites_define() {
        use crate::signal::ble::detect::{
            access_address_bits, preamble_bits, ADVERTISING_ACCESS_ADDRESS,
        };
        let rate = 8e6;
        let offset = 1e6;
        let mut rng = Rng::new(12);
        let mut bits = preamble_bits(ADVERTISING_ACCESS_ADDRESS, Phy::OneM);
        bits.extend(access_address_bits(ADVERTISING_ACCESS_ADDRESS));
        let pdu_at = bits.len();
        let air: Vec<bool> = (0..312).map(|_| rng.next_u64() & 1 == 1).collect();
        bits.extend(&air);
        let lead = 20;
        let mut all: Vec<bool> = (0..lead).map(|_| rng.next_u64() & 1 == 1).collect();
        all.extend(&bits);
        all.extend((0..20).map(|_| rng.next_u64() & 1 == 1));

        let tx = Gfsk::new(1e6, 250_000.0, 0.5).with_cfo(offset + 12_000.0);
        let burst = Burst::new(tx, &all);
        let n = burst.len_at(rate);
        let iq: Vec<Complex<f32>> = burst
            .iq(rate, n)
            .iter()
            .map(|z| Complex::new(z.re as f32 * 0.5, z.im as f32 * 0.5))
            .collect();
        let base = 500_000u64;
        let recent = Recent::new([(base, &iq[..])]);

        // The reference: the suites' readings of the same bits, ideally.
        let ideal = Burst::new(Gfsk::new(1e6, 250_000.0, 0.5), &all);
        let from = lead;
        let (settled, alternating) = crate::signal::dsp::deviation::suite_readings(&bits, |x| {
            ideal.frequency((from as f64 + x) * 1e-6) as f32
        })
        .unwrap();
        let want = modulation_from(&settled, &alternating, Phy::OneM).unwrap();

        let bit = rate / 1e6;
        let pdu_centre = base as f64 + ((lead + pdu_at) as f64 + 0.5) * bit;
        for wrong in [-0.3, 0.0, 0.3] {
            let (modulation, drift) =
                le_1m(&recent, rate, offset, pdu_centre + wrong * bit, &air).expect("held");
            let got = modulation.expect("both kinds of bit");
            for (name, g, w) in [
                (
                    "df1",
                    got.delta_f1_avg_hz.value(),
                    want.delta_f1_avg_hz.value(),
                ),
                (
                    "df2",
                    got.delta_f2_avg_hz.value(),
                    want.delta_f2_avg_hz.value(),
                ),
            ] {
                assert!((g - w).abs() < 0.01 * w, "{wrong}: {name} {g} vs {w}");
            }
            let drift = drift.expect("blocks enough");
            let f0 = drift.initial_hz.value();
            assert!((f0 - 12_000.0).abs() < 1_000.0, "{wrong}: f0 {f0}");
            assert!(drift.drift_hz.value().abs() < 3_000.0, "{wrong}: {drift:?}");
        }
    }

    /// The guard against the channel's own spectrum, noise, and neighbours
    /// either side of its limit, over a header's worth of bits (130) at
    /// 8 Msps, for transmitters across BR's deviation band.
    #[test]
    fn the_neighbour_guard_sees_the_next_channel_and_not_the_channel_itself() {
        let rate = 8e6;
        let mut rng = Rng::new(60);
        let bits: Vec<bool> = (0..130).map(|_| rng.next_u64() & 1 == 1).collect();
        let other_bits: Vec<bool> = (0..130).map(|_| rng.next_u64() & 1 == 1).collect();
        let to_f32 = |v: Vec<Complex<f64>>| -> Vec<Complex<f32>> {
            v.iter()
                .map(|z| Complex::new(z.re as f32, z.im as f32))
                .collect()
        };
        for deviation in [140e3, 160e3, 175e3] {
            let own = Burst::new(Gfsk::new(1e6, deviation, 0.5), &bits);
            let n = own.len_at(rate);
            let own = to_f32(own.iq(rate, n));
            for side in [1e6, -1e6] {
                let other = to_f32(
                    Burst::new(Gfsk::new(1e6, 160e3, 0.5).with_cfo(side), &other_bits).iq(rate, n),
                );
                // Measured: the channel alone -54 to -56 dB, with noise at
                // 20 dB in 1 MHz -30 to -31, a neighbour 25 dB down -23 to
                // -30, one 15 dB down -14 to -16.
                // (neighbour, SNR, the range the guard must read in).
                let (low, limit, high) = (f64::NEG_INFINITY, NEIGHBOUR_LIMIT_DB, f64::INFINITY);
                let cases = [
                    (None, f64::INFINITY, low, -45.0),
                    (None, 20.0, low, -26.0),
                    (Some(-25.0), 20.0, low, limit),
                    (Some(-15.0), 20.0, limit, high),
                    (Some(-15.0), f64::INFINITY, limit, high),
                ];
                for (neighbour_db, snr_db, from, below) in cases {
                    let a = neighbour_db.map_or(0.0, |db: f64| 10f32.powf(db as f32 / 20.0));
                    let noise = if snr_db.is_finite() {
                        10f64.powf(-snr_db / 10.0) * rate / 1e6
                    } else {
                        0.0
                    };
                    let z = Rng::new(61).noise(n, noise);
                    let iq: Vec<Complex<f32>> = own
                        .iter()
                        .zip(&other)
                        .zip(&z)
                        .map(|((x, y), w)| x + y * a + w)
                        .collect();
                    let db = neighbour_excess_db(&iq, rate, 1e6);
                    assert!(
                        from <= db && db < below,
                        "{deviation} Hz, side {side}, neighbour {neighbour_db:?}, SNR {snr_db}: {db} dB"
                    );
                }
            }
        }
    }

    /// A window is cut across blocks by stream position, and refused where
    /// a sample is missing.
    #[test]
    fn a_window_spans_blocks_and_refuses_a_gap() {
        let a: Vec<Complex<f32>> = (0..10).map(|i| Complex::new(i as f32, 0.0)).collect();
        let b: Vec<Complex<f32>> = (10..20).map(|i| Complex::new(i as f32, 0.0)).collect();
        let recent = Recent::new([(100, &a[..]), (110, &b[..])]);
        let got = recent.slice(107, 6).unwrap();
        let re: Vec<f32> = got.iter().map(|z| z.re).collect();
        assert_eq!(re, vec![7.0, 8.0, 9.0, 10.0, 11.0, 12.0]);
        assert!(recent.slice(99, 3).is_none(), "before what is held");
        assert!(recent.slice(118, 3).is_none(), "past what is held");
        let gap = Recent::new([(100, &a[..]), (111, &b[..])]);
        assert!(gap.slice(108, 4).is_none(), "across a gap");
    }

    /// A classic burst off the tuned centre, handed over with its sync end
    /// misplaced by up to 0.4 of a bit either way, as a lane can: the
    /// timing comes back from the sync word, and the readings are the
    /// suites' own, as the reference reads the same bits through the same
    /// filter.
    #[test]
    fn a_classic_header_reads_as_the_suites_define() {
        let rate = 20e6;
        let offset = 2e6;
        let lap = 0x0044_5566;
        let sync = crate::signal::bt::access_code::access_code_bits(lap);
        let mut rng = Rng::new(9);
        let mut bits: Vec<bool> = (0..40).map(|i| i % 2 == 0).collect();
        let sync_at = bits.len() + 4;
        // The preamble alternates into the sync word (Core 5.4 Vol 2 Part B
        // 6.3.2), the trailer out of it (6.3.4).
        bits.extend([sync[0], !sync[0], sync[0], !sync[0]]);
        bits.extend(sync);
        bits.extend([!sync[63], sync[63], !sync[63], sync[63]]);
        for _ in 0..18 {
            let b = rng.next_u64() & 1 == 1;
            bits.extend([b, b, b]);
        }
        bits.extend((0..40).map(|i| i % 2 == 0));
        let air_at = sync_at + 64;

        // On the air at `offset` from the tuning, as the radio sees it.
        let tx = Gfsk::new(1e6, 160_000.0, 0.5).with_cfo(offset);
        let burst = Burst::new(tx, &bits);
        let n = burst.len_at(rate);
        let iq: Vec<Complex<f32>> = burst
            .iq(rate, n)
            .iter()
            .map(|z| Complex::new(z.re as f32 * 0.5, z.im as f32 * 0.5))
            .collect();
        // Held in two blocks, starting at an arbitrary stream position.
        let base = 1_000_000u64;
        let cut = n / 3;
        let recent = Recent::new([(base, &iq[..cut]), (base + cut as u64, &iq[cut..])]);

        // The reference: the suites' readings of the same bits (sync word,
        // trailer, header), through its own copy of the measurement filter.
        let ref_tx = Gfsk::new(1e6, 160_000.0, 0.5);
        let ref_burst = Burst::new(ref_tx, &bits);
        let fine = 32e6;
        let ref_iq = reference::filter(
            &ref_burst.iq(fine, ref_burst.len_at(fine)),
            &reference::wide_filter(fine),
        );
        let trace = Trace::from_iq(&ref_iq, fine, 1e6);
        let (settled, alternating) =
            crate::signal::dsp::deviation::suite_readings(&bits[sync_at..air_at + 58], |x| {
                trace.at(sync_at as f64 + x).unwrap() as f32
            })
            .unwrap();
        let want = Deviation::from_readings(&settled, &alternating);

        let bit = rate / 1e6;
        let true_end = base as f64 + (sync_at as f64 + 63.5) * bit;
        for wrong in [-0.4, -0.15, 0.0, 0.2, 0.4] {
            let reading = classic(&recent, rate, offset, lap, true_end + wrong * bit)
                .expect("the window is held");
            // On the channel's own centre, with no drift: f0 and the drift
            // read nothing, to a fraction of a kHz.
            let (f0, drift) = reading.carrier.expect("blocks enough for a carrier");
            assert!(f0.abs() < 1_000.0, "{wrong}: f0 {f0}");
            assert!(drift.drift_hz.value().abs() < 3_000.0, "{wrong}: {drift:?}");
            let got = reading.deviation;
            let (g1, w1) = (got.settled.mean().unwrap(), want.settled.mean().unwrap());
            assert!(
                (g1.value() - w1.value()).abs() < 0.005 * w1.value(),
                "{wrong}: df1 {g1:?} vs {w1:?}"
            );
            assert_eq!(got.alternating.n, want.alternating.n);
            let (g2, w2) = (
                got.alternating.mean().unwrap(),
                want.alternating.mean().unwrap(),
            );
            assert!(
                (g2.value() - w2.value()).abs() < 0.01 * w2.value(),
                "{wrong}: df2 {g2:?} vs {w2:?}"
            );
        }
        // Not held: refused, not read from somewhere else.
        let short = Recent::new([(base, &iq[..cut])]);
        assert!(classic(&short, rate, offset, lap, true_end).is_none());

        // A neighbour a channel above, 15 dB down: counted, not read.
        let mut rng = Rng::new(10);
        let other_bits: Vec<bool> = (0..bits.len()).map(|_| rng.next_u64() & 1 == 1).collect();
        let other = Burst::new(
            Gfsk::new(1e6, 160_000.0, 0.5).with_cfo(offset + 1e6),
            &other_bits,
        )
        .iq(rate, n);
        let a = 0.5 * 10f64.powf(-15.0 / 20.0);
        let loud: Vec<Complex<f32>> = iq
            .iter()
            .zip(&other)
            .map(|(x, y)| x + Complex::new((y.re * a) as f32, (y.im * a) as f32))
            .collect();
        let held = Recent::new([(base, &loud[..])]);
        let got = classic(&held, rate, offset, lap, true_end)
            .expect("held")
            .deviation;
        assert_eq!(got.neighbour_busy, 1);
        assert_eq!((got.settled.n, got.alternating.n), (0, 0));
    }

    /// A Coded packet's symbols at `rate` from a reference transmitter, with
    /// `lead` symbols of settling either side, unit amplitude, plus noise of
    /// `noise_power` (none at zero): the samples, and the stream position of
    /// the packet's first symbol when they are held from `base`.
    fn coded_burst(
        tx: Gfsk,
        symbols: &[bool],
        rate: f64,
        lead: usize,
        noise_power: f64,
        base: u64,
        rng: &mut Rng,
    ) -> (Vec<Complex<f32>>, f64) {
        let mut all: Vec<bool> = (0..lead).map(|_| rng.next_u64() & 1 == 1).collect();
        all.extend(symbols);
        all.extend((0..lead).map(|_| rng.next_u64() & 1 == 1));
        let burst = Burst::new(tx, &all);
        let n = burst.len_at(rate);
        let noise = if noise_power > 0.0 {
            rng.noise(n, noise_power)
        } else {
            vec![Complex::new(0.0, 0.0); n]
        };
        let iq = burst
            .iq(rate, n)
            .iter()
            .zip(&noise)
            .map(|(z, w)| Complex::new(z.re as f32, z.im as f32) + w)
            .collect();
        (iq, base as f64 + lead as f64 * rate / 1e6)
    }

    /// An S=8 packet off the tuned centre: its four preamble groups read the
    /// carrier as sent (RFPHY/TRM/BV-14-C), and its Δf1 reads as the suites'
    /// readings of the same symbols do, ideally (BV-13-C), every Δf1max above
    /// 185 kHz. Handed over a third of a symbol early or late, it times itself
    /// from the sync symbols.
    #[test]
    fn a_coded_packet_reads_as_the_suites_define() {
        use crate::signal::ble::coded::{self, Coding};
        use crate::signal::ble::detect::ADVERTISING_ACCESS_ADDRESS;
        let (rate, offset, base) = (8e6, 1e6, 400_000u64);
        let mut rng = Rng::new(70);
        let payload: Vec<u8> = (0..30).map(|_| rng.next_u64() as u8).collect();
        let symbols = coded::transmit(ADVERTISING_ACCESS_ADDRESS, Coding::S8, 38, 0x07, &payload);
        let tx = Gfsk::new(1e6, 250_000.0, 0.5).with_cfo(offset + 12_000.0);
        let (iq, start) = coded_burst(tx, &symbols, rate, 20, 0.0, base, &mut rng);
        let recent = Recent::new([(base, &iq[..])]);

        let ideal = Burst::new(Gfsk::new(1e6, 250_000.0, 0.5), &symbols);
        let (settled, _) = crate::signal::dsp::deviation::suite_readings(&symbols, |x| {
            ideal.frequency(x * 1e-6) as f32
        })
        .unwrap();
        let want = settled.iter().map(|&v| v as f64).sum::<f64>() / settled.len() as f64;

        let symbol = rate / 1e6;
        for wrong in [-0.3, 0.0, 0.3] {
            let got = le_coded(
                &recent,
                rate,
                offset,
                start + wrong * symbol,
                &symbols,
                Coding::S8,
            )
            .expect("held");
            let initial = got.initial.expect("S=8 has a preamble to read");
            for (k, f) in initial.groups_hz.iter().enumerate() {
                assert!((f - 12_000.0).abs() < 300.0, "{wrong}: f{k} {f}");
            }
            let m = got.modulation.expect("S=8 has settled symbols");
            let df1 = m.delta_f1_avg_hz.value();
            assert!(
                (df1 - want).abs() < 0.01 * want,
                "{wrong}: df1 {df1} vs {want}"
            );
            assert_eq!(m.share_f1max_above_limit, 1.0);
        }
    }

    /// A carrier drifting 200 Hz a microsecond: the preamble's f0 and f3,
    /// 48 us apart, differ by 9.6 kHz.
    #[test]
    fn the_preamble_reads_the_drift_between_its_groups() {
        use crate::signal::ble::coded::{self, Coding};
        use crate::signal::ble::detect::ADVERTISING_ACCESS_ADDRESS;
        let (rate, base) = (8e6, 100_000u64);
        let mut rng = Rng::new(71);
        let symbols = coded::transmit(
            ADVERTISING_ACCESS_ADDRESS,
            Coding::S8,
            38,
            0x07,
            &[1, 2, 3, 4],
        );
        let tx = Gfsk::new(1e6, 250_000.0, 0.5).with_drift(200e6);
        let (iq, start) = coded_burst(tx, &symbols, rate, 20, 0.0, base, &mut rng);
        let recent = Recent::new([(base, &iq[..])]);
        let got = le_coded(&recent, rate, 0.0, start, &symbols, Coding::S8).expect("held");
        let g = got.initial.expect("S=8").groups_hz;
        let drift = g[3] - g[0];
        assert!((drift - 9_600.0).abs() < 300.0, "f3 - f0 = {drift}");
    }

    /// S=2 has neither suite (both are defined for S=8 alone), so neither is
    /// read; its SNR is.
    #[test]
    fn s2_has_no_suite_readings_but_an_snr() {
        use crate::signal::ble::coded::{self, Coding};
        use crate::signal::ble::detect::ADVERTISING_ACCESS_ADDRESS;
        let (rate, base) = (8e6, 100_000u64);
        let mut rng = Rng::new(72);
        let symbols = coded::transmit(
            ADVERTISING_ACCESS_ADDRESS,
            Coding::S2,
            38,
            0x07,
            &[1, 2, 3, 4],
        );
        let tx = Gfsk::new(1e6, 250_000.0, 0.5);
        let (iq, start) = coded_burst(tx, &symbols, rate, 20, 0.01, base, &mut rng);
        let recent = Recent::new([(base, &iq[..])]);
        let got = le_coded(&recent, rate, 0.0, start, &symbols, Coding::S2).expect("held");
        assert!(
            got.modulation.is_none() && got.initial.is_none() && got.drift.is_none(),
            "{got:?}"
        );
        assert!(got.snr_db.is_some());
    }

    /// The SNR is the one in the band the samples are measured in: the
    /// signal's power over the noise's through the tester's filter, whose
    /// equivalent noise bandwidth fixes how much of the added noise gets in.
    /// Within a decibel of that at three noise levels.
    #[test]
    fn the_coded_snr_is_the_snr_in_the_measurement_band() {
        use crate::signal::ble::coded::{self, Coding};
        use crate::signal::ble::detect::ADVERTISING_ACCESS_ADDRESS;
        let rate = 8e6;
        let taps = measurement_filter(rate);
        let (sum, sq) = taps.iter().fold((0.0f64, 0.0f64), |(a, b), &t| {
            (a + t as f64, b + (t as f64).powi(2))
        });
        // The share of the added noise's power the filter lets through.
        let passed = sq / (sum * sum);
        for (k, noise) in [0.3, 0.1, 0.03].into_iter().enumerate() {
            let mut rng = Rng::new(80 + k as u64);
            let payload: Vec<u8> = (0..30).map(|_| rng.next_u64() as u8).collect();
            let symbols =
                coded::transmit(ADVERTISING_ACCESS_ADDRESS, Coding::S8, 38, 0x07, &payload);
            let tx = Gfsk::new(1e6, 250_000.0, 0.5);
            let (iq, start) = coded_burst(tx, &symbols, rate, 40, noise, 0, &mut rng);
            let recent = Recent::new([(0, &iq[..])]);
            let got = le_coded(&recent, rate, 0.0, start, &symbols, Coding::S8)
                .and_then(|r| r.snr_db)
                .expect("an SNR");
            let want = -10.0 * (noise * passed).log10();
            assert!(
                (got - want).abs() < 1.0,
                "noise {noise}: {got:.2} dB, in band {want:.2} dB"
            );
        }
    }

    /// A carrier drifting 200 Hz a microsecond through a whole S=8 packet:
    /// the payload's groups read that rate, and the furthest from f0 is the
    /// last, on the side it drifted to.
    #[test]
    fn a_drifting_coded_packet_reads_its_drift_through_the_payload() {
        use crate::signal::ble::coded::{self, Coding};
        use crate::signal::ble::detect::ADVERTISING_ACCESS_ADDRESS;
        let (rate, base) = (8e6, 100_000u64);
        let mut rng = Rng::new(73);
        let payload: Vec<u8> = (0..30).map(|_| rng.next_u64() as u8).collect();
        let symbols = coded::transmit(ADVERTISING_ACCESS_ADDRESS, Coding::S8, 38, 0x07, &payload);
        let tx = Gfsk::new(1e6, 250_000.0, 0.5).with_drift(200e6);
        let (iq, start) = coded_burst(tx, &symbols, rate, 20, 0.0, base, &mut rng);
        let recent = Recent::new([(base, &iq[..])]);
        let got = le_coded(&recent, rate, 0.0, start, &symbols, Coding::S8).expect("held");
        let drift = got.drift.expect("a payload long enough");
        let rate_read = drift.drift_rate_hz_per_us.value();
        assert!((rate_read - 200.0).abs() < 10.0, "{rate_read} Hz/us");
        assert!(drift.drift_hz.value() > 0.0);
        assert_eq!(
            drift.drift_hz.value(),
            drift.final_hz.value() - drift.initial_hz.value()
        );
    }

    /// No drift, on traffic: the steepest 48 us change is the noise's, near
    /// zero. A plain mean of 16 symbols moves about 3 kHz with the data at
    /// the group's edges, and its steepest change would read over 100 Hz/us.
    #[test]
    fn a_steady_coded_carrier_reads_no_drift_on_traffic() {
        use crate::signal::ble::coded::{self, Coding};
        use crate::signal::ble::detect::ADVERTISING_ACCESS_ADDRESS;
        let (rate, base) = (8e6, 100_000u64);
        let mut rng = Rng::new(74);
        let payload: Vec<u8> = (0..60).map(|_| rng.next_u64() as u8).collect();
        let symbols = coded::transmit(ADVERTISING_ACCESS_ADDRESS, Coding::S8, 38, 0x07, &payload);
        let tx = Gfsk::new(1e6, 250_000.0, 0.5).with_cfo(-20_000.0);
        let (iq, start) = coded_burst(tx, &symbols, rate, 20, 0.0, base, &mut rng);
        let recent = Recent::new([(base, &iq[..])]);
        let drift = le_coded(&recent, rate, 0.0, start, &symbols, Coding::S8)
            .and_then(|r| r.drift)
            .expect("a payload long enough");
        assert!(drift.drift_rate_hz_per_us.value().abs() < 10.0, "{drift:?}");
        assert!(drift.drift_hz.value().abs() < 500.0, "{drift:?}");
        assert!(
            (drift.initial_hz.value() + 20_000.0).abs() < 300.0,
            "{drift:?}"
        );
    }
}
