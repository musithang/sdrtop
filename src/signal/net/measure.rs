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

use crate::signal::ble::measure::{drift, modulation_from, Drift, ModulationQuality};
use crate::signal::ble::Phy;
use crate::signal::bt::piconet::Deviation;
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
}

/// How finely [`timing`] searches, in steps a bit.
const TIMING_STEPS: i32 = 64;

/// Where the bits' centres are, as an offset in bits from `first` (the
/// estimated centre of `known[0]`), searched over one bit either way: the
/// offset at which the readings, less their mean, best agree with the known
/// bits' signs. A tester times a packet from its known start the same way
/// (the suites' "position of bit p0"); a lane never enters it.
fn timing(tester: &Tester, first: f64, bit: f64, known: &[bool]) -> f64 {
    let score = |tau: f64| {
        let hz: Vec<f64> = (0..known.len())
            .map(|k| tester.at(first + (k as f64 + tau) * bit) as f64)
            .collect();
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

/// The suites' readings (`dsp::deviation::suite_readings`) of `known`
/// bits and the `after` bits that follow them on the air, and each of the
/// `after` bits' reading at its centre: `first` is the estimated stream
/// position of `known[0]`'s centre, `rate` the stream's, `offset_hz` the
/// channel's distance from the tuning. The timing comes from `known`
/// ([`timing`]). `None` as [`Tester::new`] refuses, or with no carrier to
/// measure from.
type Readings = ((Vec<f32>, Vec<f32>), Vec<f32>);

/// What reading a burst came to.
enum Read {
    Readings(Readings),
    /// A neighbour was louder than [`NEIGHBOUR_LIMIT_DB`] allows.
    NeighbourBusy,
}

fn read_after_known(
    recent: &Recent,
    rate: f64,
    offset_hz: f64,
    known: &[bool],
    first: f64,
    after: &[bool],
    neighbours: Option<f64>,
) -> Option<Read> {
    let bit = rate / 1e6;
    let last = first + (known.len() + after.len()) as f64 * bit;
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
        return Some(Read::NeighbourBusy);
    }
    let tau = timing(&tester, first, bit, known);
    // Bit position `x` (bit `k` spans `k..k + 1`) as a stream position.
    let at = |x: f64| tester.at(first + (x - 0.5 + tau) * bit);
    let all: Vec<bool> = known.iter().chain(after).copied().collect();
    let readings = crate::signal::dsp::deviation::suite_readings(&all, at)?;
    let centres = (0..after.len())
        .map(|i| at((known.len() + i) as f64 + 0.5))
        .collect();
    Some(Read::Readings((readings, centres)))
}

/// A classic header's modulation readings, as a tester reads them: `lap`'s
/// sync word ended near stream position `sync_end_pair` on a channel
/// `offset_hz` from the tuning, and `air` is the trailer and header that
/// followed. `None` when the samples are no longer held or the rate cannot
/// be decimated to [`MEASURE_RATE_HZ`]; a header that is not measured adds
/// nothing, rather than a reading from the detection path.
pub fn classic(
    recent: &Recent,
    rate: f64,
    offset_hz: f64,
    lap: u32,
    sync_end_pair: f64,
    air: &[bool],
) -> Option<Deviation> {
    let sync = crate::signal::bt::access_code::access_code_bits(lap);
    let first = sync_end_pair - (sync.len() - 1) as f64 * rate / 1e6;
    match read_after_known(
        recent,
        rate,
        offset_hz,
        &sync,
        first,
        air,
        Some(CLASSIC_SPACING_HZ),
    )? {
        Read::Readings(((settled, alternating), _)) => {
            Some(Deviation::from_readings(&settled, &alternating))
        }
        Read::NeighbourBusy => Some(Deviation::neighbour_busy()),
    }
}

/// An LE 1M packet's modulation and drift, as a tester reads them: `air`
/// is its PDU as sent (header through CRC, whitened), whose first bit the
/// receiver's slicer centred at stream position `pdu_pair`, on a channel
/// `offset_hz` from the tuning. Timed by the preamble and the advertising
/// access address before it, the 40 bits every such packet starts with.
///
/// LE 1M only: the suites' filter is written for 1 Msym/s, and LE 2M, twice
/// the rate and the deviation, would lose half its signal in it. `None` as
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
    let mut known = preamble_bits(ADVERTISING_ACCESS_ADDRESS, Phy::OneM);
    known.extend(access_address_bits(ADVERTISING_ACCESS_ADDRESS));
    let first = pdu_pair - known.len() as f64 * rate / 1e6;
    // No guard: LE's neighbours are 2 MHz away, in the measurement filter's
    // stopband, and one 15 dB down moves the readings 0.3 %
    // (`conformance::a_neighbour_25_db_down_moves_the_readings_under_one_
    // percent`, which also runs LE at 15).
    let Read::Readings(((settled, alternating), centres)) =
        read_after_known(recent, rate, offset_hz, &known, first, air, None)?
    else {
        return None;
    };
    Some((
        modulation_from(&settled, &alternating, Phy::OneM),
        drift(&centres, Phy::OneM),
    ))
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
        bits.extend([!sync[0], sync[0], !sync[0], sync[0]]);
        bits.extend(sync);
        bits.extend([!sync[63], sync[63], !sync[63], sync[63]]);
        for _ in 0..18 {
            let b = rng.next_u64() & 1 == 1;
            bits.extend([b, b, b]);
        }
        bits.extend((0..40).map(|i| i % 2 == 0));
        let air_at = sync_at + 64;
        let air = &bits[air_at..air_at + 58];

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
            let got = classic(&recent, rate, offset, lap, true_end + wrong * bit, air)
                .expect("the window is held");
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
        assert!(classic(&short, rate, offset, lap, true_end, air).is_none());

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
        let got = classic(&held, rate, offset, lap, true_end, air).expect("held");
        assert_eq!(got.neighbour_busy, 1);
        assert_eq!((got.settled.n, got.alternating.n), (0, 0));
    }
}
