// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! LE Coded's own receive chain, apart from LE 1M's.
//!
//! LE Coded sends the same 1 Msym/s GFSK as LE 1M, but spends every bit as
//! two or eight symbols so a receiver can hear it further away. A receiver
//! that throws decibels away throws that range away, so this chain is not
//! LE 1M's with a second detector bolted on: it has a channel filter chosen
//! for Coded's sensitivity ([`FILTER`]) and reads each symbol as the mean of
//! the discriminator over the symbol's own period ([`symbol_means`]) rather
//! than one sample of it. LE 1M's front end cannot change without moving
//! every LE 1M figure, and does not have to: the two chains share nothing
//! but the samples.

use std::collections::VecDeque;

use num_complex::Complex;

use super::coded::{self, Coding};
use super::detect::ADVERTISING_ACCESS_ADDRESS;
use super::pdu::{self, Packet};
use super::receive::Funnel;
use crate::signal::dsp::correlate::ShapeMatcher;
use crate::signal::dsp::discriminate::instantaneous_freq_hz;
use crate::signal::dsp::fir::{design_lowpass_to_spec, StreamingDecimator};
use crate::signal::dsp::nco::Nco;

/// The rate the chain decimates to: four samples a symbol, as LE 1M's.
pub const WORKING_RATE_HZ: f64 = 4e6;

/// Working samples a symbol at [`WORKING_RATE_HZ`].
const WORKING_SPS: usize = 4;

/// The channel filter's stopband, as LE 1M's: real rejection of the
/// neighbouring channels a wide capture carries, without the taps a deeper
/// one would cost.
const STOPBAND_DB: f64 = 40.0;

/// A channel filter, Hz: `cutoff_hz` is its -6 dB point (the windowed
/// sinc's cutoff, `fir::design_lowpass_to_spec`'s `fc`), and the transition
/// is centred on it, so the passband ends `transition_hz / 2` below it and
/// the stopband starts as far above.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChannelFilter {
    pub cutoff_hz: f64,
    pub transition_hz: f64,
}

#[cfg(test)]
impl ChannelFilter {
    /// Where the passband ends, Hz.
    pub fn passband_hz(self) -> f64 {
        self.cutoff_hz - self.transition_hz / 2.0
    }

    /// Where the stopband starts, Hz.
    pub fn stopband_hz(self) -> f64 {
        self.cutoff_hz + self.transition_hz / 2.0
    }
}

/// The filters the sensitivity bench weighs, narrowest first, to LE 1M's
/// (-6 dB at 1.5 MHz, 0.5 MHz transition) as the reference. A narrower
/// passband needs a narrower transition to keep the stopband where the
/// neighbours are, and that costs taps: the bench's load column says how
/// many.
pub const CANDIDATES: [ChannelFilter; 7] = [
    ChannelFilter {
        cutoff_hz: 350e3,
        transition_hz: 300e3,
    },
    ChannelFilter {
        cutoff_hz: 400e3,
        transition_hz: 300e3,
    },
    ChannelFilter {
        cutoff_hz: 500e3,
        transition_hz: 300e3,
    },
    ChannelFilter {
        cutoff_hz: 650e3,
        transition_hz: 300e3,
    },
    ChannelFilter {
        cutoff_hz: 800e3,
        transition_hz: 300e3,
    },
    ChannelFilter {
        cutoff_hz: 1_000e3,
        transition_hz: 400e3,
    },
    ChannelFilter {
        cutoff_hz: 1_500e3,
        transition_hz: 500e3,
    },
];

/// The chain's channel filter: -6 dB at 500 kHz, a 300 kHz transition, so a
/// passband to 350 kHz and a stopband from 650 kHz.
///
/// **Chosen on the sensitivity bench** (`coded_bench::coded_filter_bench`: a
/// 50-octet PDU, 150 packets a point, crystal offsets to ±50 ppm, each
/// symbol's mean, soft decisions), where packet error rate reaches the
/// Core's 30.8 % at, in Eb/N0:
///
/// | -6 dB at | 350 kHz | 400 | **500** | 650 | 800 | 1000 | 1500, LE 1M's |
/// |---|---|---|---|---|---|---|---|
/// | S=8 | 12.8 dB | 12.0 | **11.9** | 12.6 | 13.7 | 14.5 | 16.1 |
/// | S=2 | never | 14.1 | **11.2** | 11.2 | 12.0 | 12.7 | 14.4 |
///
/// 4.2 dB better than LE 1M's filter at S=8 and 3.1 at S=2. Narrower is
/// worse: at 400 kHz S=2 already has a floor (a fifth of its packets lost
/// at any Eb/N0 the bench reached), the filter cutting into the signal, so
/// 500 kHz sits about 100 kHz from that edge. Its 151 taps cost about half a
/// millisecond per millisecond of 20 Msps signal on the i3, against 0.33 for
/// LE 1M's 91; the chain runs only on the LE Coded view.
pub const FILTER: ChannelFilter = CANDIDATES[2];

/// The chain's front end from `raw_rate` to [`WORKING_RATE_HZ`] through
/// `filter`, or why it cannot be built: a raw rate below the working rate,
/// or not a whole multiple of it, would scale every timing and deviation by
/// the mismatch with nothing on screen to say so, so it is refused as LE
/// 1M's front end refuses it.
pub fn front_end(raw_rate: f64, filter: ChannelFilter) -> Result<StreamingDecimator, String> {
    if raw_rate < WORKING_RATE_HZ {
        return Err(format!(
            "LE Coded decode needs at least {:.1} Msps; the radio is at {:.3} Msps",
            WORKING_RATE_HZ / 1e6,
            raw_rate / 1e6
        ));
    }
    let d = (raw_rate / WORKING_RATE_HZ).round().max(1.0) as usize;
    if (raw_rate / d as f64 - WORKING_RATE_HZ).abs() > WORKING_RATE_HZ * 0.01 {
        return Err(format!(
            "LE Coded decode needs a sample rate near a whole multiple of {:.1} Msps; {:.3} Msps is not one",
            WORKING_RATE_HZ / 1e6,
            raw_rate / 1e6
        ));
    }
    let taps = design_lowpass_to_spec(
        filter.cutoff_hz / raw_rate,
        filter.transition_hz / raw_rate,
        STOPBAND_DB,
    );
    Ok(StreamingDecimator::new(taps, d))
}

/// One reading a symbol: the mean of the discriminator `track` over the
/// four working samples of each symbol's own period, the first symbol
/// centred at `first_centre` (a sample index, fractional, as a phase search
/// gives it), up to `count` symbols or as many whole ones as `track` holds.
///
/// **A mean, not a sample.** The discriminator's noise is broadband next to
/// one symbol's worth of signal; averaging the symbol's own four samples
/// keeps the signal and halves the noise's amplitude, which the sensitivity
/// bench measured at 3 to 4 dB on packet error rate against reading the
/// centre alone.
pub fn symbol_means(track: &[f32], first_centre: f64, count: usize) -> Vec<f32> {
    let half = (WORKING_SPS as f64 - 1.0) / 2.0;
    (0..count)
        .map_while(|i| {
            let start = (first_centre + (i * WORKING_SPS) as f64 - half).round();
            if start < 0.0 {
                return None;
            }
            let start = start as usize;
            let period = track.get(start..start + WORKING_SPS)?;
            Some(period.iter().sum::<f32>() / WORKING_SPS as f32)
        })
        .collect()
}

/// The symbols the detector looks for: the preamble and the advertising
/// access address through FEC block 1 (`coded::sync_symbols`).
const SYNC_SYMBOLS: usize = coded::PREAMBLE_SYMBOLS + 256;

/// The detector statistic's effective sample count under noise alone: its
/// variance is `1 / SYNC_N_EFF`. 557.2, measured through this chain's front
/// end and discriminator on four million samples of complex Gaussian noise
/// at 4 Msps, against the 1344-sample template (336 symbols); far below the
/// template's length because the 500 kHz filter makes neighbouring readings
/// alike. `the_coded_detector_s_noise_statistics_are_the_ones_measured`
/// measures it again and holds it here. The threshold, `SYNC_Z` deviations,
/// is about 0.25.
const SYNC_N_EFF: f64 = 557.2;

/// Standard deviations above zero the statistic must reach to trigger, as
/// LE 1M's detector: about one false trigger in 10^9 working samples of noise.
const SYNC_Z: f64 = 6.0;

/// The second stage of detection: how well the symbols at the found timing
/// must agree with the sync sequence ([`sync_agreement`]) for the trigger to
/// stand. Six standard deviations of the agreement on another packet's S=8
/// data, the worst case: its 84 coded bits (20 of preamble, 64 of access
/// address) are random there, so the agreement scatters about zero with a
/// deviation of `1 / sqrt(84)`.
///
/// **Why a second stage.** The first stage correlates the discriminator's
/// whole track, within each symbol too, and every S=8 stream has the same
/// shape within its four-symbol patterns (`0011` or `1100`, a transition in
/// the middle of each), the coded access address's included. So the first
/// stage also fires inside S=8 packets it is not reading (one whose CI it
/// refused, say), several times a packet, measured. Each symbol's mean
/// carries only which pattern was sent, so there the structure is gone and
/// only the bits agree or do not.
const SYNC_AGREEMENT: f64 = 6.0 / 9.165_151_389_911_68; // 6 / sqrt(84)

/// How long after first crossing the threshold the detector keeps looking for
/// the statistic's peak, working samples: a whole preamble.
///
/// **Not LE 1M's two symbols.** The preamble repeats every eight symbols, so
/// the statistic has sidelobes every eight symbols before its peak, where
/// the template's preamble lines up with part of the packet's: about 0.30 at
/// one period early, measured on a clean packet, above the 0.25 threshold,
/// against 0.999 at the peak. A detector that settled within two symbols of
/// first crossing settled on a sidelobe, eight symbols early, and read
/// block 1 from the wrong place. Looking across a whole preamble reaches the
/// peak from any sidelobe.
const PEAK_SAMPLES: u64 = (coded::PREAMBLE_SYMBOLS * WORKING_SPS) as u64;

/// Random-looking symbols either side of the sync symbols when its template
/// is built, so the filters settle on signal before and after it.
const TEMPLATE_MARGIN_SYMBOLS: usize = 16;

/// The longest packet, symbols (Table 2.1: 17040 us at S=8), and a little:
/// a capture longer than this has gone wrong and is given up.
const LONGEST_SYMBOLS: usize = 17_040 + 64;

/// The nominal frequency deviation the readings are scaled by, Hz: the
/// scale does not change a soft decision, it only makes a reading of 1.0
/// mean a full symbol.
const DEVIATION_HZ: f64 = 250e3;

/// One LE Coded receiver for one channel: its own mixer and front end, the
/// detector over the sync symbols, and the capture under way.
pub struct CodedReceiver {
    mixer: Option<Nco>,
    tuned_centre_hz: f64,
    channel: u8,
    raw_rate: f64,
    decim: StreamingDecimator,
    matcher: ShapeMatcher,
    /// The template's length, working samples.
    template_len: usize,
    threshold: f64,
    /// The last working sample, so the discriminator runs across blocks.
    last_sample: Option<Complex<f32>>,
    /// The latest readings of the discriminator, enough to reach back to the
    /// start of a sync sequence once its end is detected.
    recent: VecDeque<f32>,
    /// Working samples produced since the stream (re)started.
    worked: u64,
    /// The stream pair the first sample since the (re)start sits at.
    origin_pair: Option<u64>,
    /// Where the next block should start; a block that starts elsewhere is a
    /// break in the stream.
    next_pair: Option<u64>,
    capture: Option<Capture>,
    funnel: Funnel,
}

/// A packet being read.
struct Capture {
    /// The working-sample index, counted as [`CodedReceiver::worked`] is, of
    /// `track[0]`.
    origin: u64,
    track: Vec<f32>,
    /// The strongest reading since the trigger, and where it fell.
    peak: (f64, u64),
    /// Until when the peak is still being looked for.
    peak_until: u64,
    stage: Stage,
    /// How long `track` must be before the stage can be read: checked every
    /// sample, so a packet is finished, and the detector free for the next
    /// one, as soon as its last symbol is in.
    due: usize,
}

/// How many samples of a capture hold `symbols` whole symbols, the first
/// centred at `centre`: [`symbol_means`] reads two samples either side of a
/// centre.
fn samples_for(centre: f64, symbols: usize) -> usize {
    (centre + (symbols * WORKING_SPS) as f64 + 1.0).ceil() as usize
}

enum Stage {
    /// Finding where the sync sequence ends.
    Peaking,
    /// The first symbol's centre (an index into `track`) is known; block 1
    /// is awaited.
    Block1 { centre: f64 },
    /// Block 1 read; the PDU header is awaited.
    Header {
        centre: f64,
        coding: Coding,
        repairs: u32,
    },
    /// The length is known; the whole of block 2 is awaited.
    Body {
        centre: f64,
        coding: Coding,
        repairs: u32,
        symbols: usize,
    },
}

impl CodedReceiver {
    /// A receiver for `channel` (its centre from `channel::centre_hz`) on a
    /// radio tuned to `tuned_centre_hz` at `raw_rate`, for the advertising
    /// access address and CRC init, primary or secondary channel alike.
    pub fn new(raw_rate: f64, channel: u8, tuned_centre_hz: f64) -> Result<Self, String> {
        let centre = super::channel::centre_hz(channel)
            .ok_or_else(|| format!("{channel} is not a BLE channel"))? as f64;
        let decim = front_end(raw_rate, FILTER)?;
        let template = sync_template(raw_rate)?;
        let offset_hz = centre - tuned_centre_hz;
        Ok(Self {
            mixer: (offset_hz.abs() >= 1.0).then(|| Nco::new(-offset_hz, raw_rate)),
            tuned_centre_hz,
            channel,
            raw_rate,
            decim,
            matcher: ShapeMatcher::new(&template),
            template_len: template.len(),
            threshold: SYNC_Z / SYNC_N_EFF.sqrt(),
            last_sample: None,
            recent: VecDeque::new(),
            worked: 0,
            origin_pair: None,
            next_pair: None,
            capture: None,
            funnel: Funnel::default(),
        })
    }

    /// Whether this receiver is the one for `channel` at `raw_rate` on a
    /// radio tuned to `tuned_centre_hz`; a change on any of them is a new
    /// receiver.
    pub fn matches(&self, channel: u8, raw_rate: f64, tuned_centre_hz: f64) -> bool {
        self.channel == channel
            && self.raw_rate == raw_rate
            && self.tuned_centre_hz == tuned_centre_hz
    }

    /// The trigger counts since the last call, and a fresh count started.
    pub fn take_funnel(&mut self) -> Funnel {
        std::mem::take(&mut self.funnel)
    }

    /// Put the receiver down: the trigger counts since the last
    /// [`Self::take_funnel`], with a capture under way counted as given up,
    /// so every trigger ends somewhere even when the receiver does not.
    pub fn finish(mut self) -> Funnel {
        if self.capture.is_some() {
            self.funnel.gave_up += 1;
        }
        self.funnel
    }

    /// Forget the stream: the next block is read as the first. A capture
    /// under way is abandoned without being counted; [`Self::push_iq_at`]
    /// counts one a break in the stream cuts short.
    pub fn reset(&mut self) {
        self.decim.reset();
        if let Some(m) = self.mixer.as_mut() {
            m.reset();
        }
        self.matcher.reset();
        self.last_sample = None;
        self.recent.clear();
        self.worked = 0;
        self.origin_pair = None;
        self.next_pair = None;
        self.capture = None;
    }

    /// Feed one block, `first_pair` the stream position of its first pair.
    /// Returns the packets whose capture ended in it, stamped on the stream
    /// clock: `at_pair` where the preamble started, `pdu_pair` the centre of
    /// the PDU's first symbol.
    ///
    /// A block that does not start where the last one ended is a break in
    /// the stream: whatever was being captured is given up (and counted),
    /// and the receiver starts again from this block, rather than reading a
    /// packet across a gap.
    pub fn push_iq_at(&mut self, iq: &[Complex<f32>], first_pair: u64) -> Vec<Packet> {
        if self.next_pair.is_some_and(|next| next != first_pair) {
            if self.capture.is_some() {
                self.funnel.gave_up += 1;
            }
            self.reset();
        }
        self.origin_pair.get_or_insert(first_pair);
        self.next_pair = Some(first_pair + iq.len() as u64);

        let mixed;
        let iq = match self.mixer.as_mut() {
            Some(mixer) => {
                let mut out = Vec::new();
                mixer.mix_into(iq, &mut out);
                mixed = out;
                &mixed[..]
            }
            None => iq,
        };
        let mut working = Vec::new();
        self.decim.process(iq, &mut working);
        let mut track = Vec::with_capacity(working.len());
        for &s in &working {
            track.push(match self.last_sample {
                Some(prev) => instantaneous_freq_hz(prev, s, WORKING_RATE_HZ),
                None => 0.0,
            });
            self.last_sample = Some(s);
        }
        let mut readings = Vec::new();
        self.matcher.process_block(&track, &mut readings);

        let keep = self.template_len + PEAK_SAMPLES as usize + 2 * WORKING_SPS;
        let mut found = Vec::new();
        for (&f, reading) in track.iter().zip(&readings) {
            let here = self.worked;
            self.worked += 1;
            self.recent.push_back(f);
            if self.recent.len() > keep {
                self.recent.pop_front();
            }
            match self.capture.as_mut() {
                Some(c) => {
                    c.track.push(f);
                    if matches!(c.stage, Stage::Peaking) {
                        if let Some(rho) = *reading {
                            if rho > c.peak.0 {
                                c.peak = (rho, here);
                            }
                        }
                    }
                }
                None => {
                    if let Some(rho) = *reading {
                        if rho > self.threshold {
                            self.capture = Some(Capture {
                                origin: here + 1 - self.recent.len() as u64,
                                track: self.recent.iter().copied().collect(),
                                peak: (rho, here),
                                peak_until: here + PEAK_SAMPLES,
                                stage: Stage::Peaking,
                                due: usize::MAX,
                            });
                        }
                    }
                }
            }
            if self
                .capture
                .as_ref()
                .is_some_and(|c| matches!(c.stage, Stage::Peaking) && here >= c.peak_until)
            {
                self.fix_timing();
            }
            if self
                .capture
                .as_ref()
                .is_some_and(|c| c.track.len() >= c.due)
            {
                while let Some(done) = self.advance() {
                    if let Some(p) = done {
                        found.push(p);
                    }
                }
            }
        }
        found
    }

    /// The peak is found: where the sync sequence started, the first
    /// symbol's centre to the quarter sample that best fits the known
    /// symbols, and whether the symbols there are the sync sequence at all.
    fn fix_timing(&mut self) {
        let Some(c) = self.capture.as_mut() else {
            return;
        };
        let start = c.peak.1 + 1 - self.template_len as u64;
        let nominal = start.saturating_sub(c.origin) as f64 + 1.5;
        let sync = coded::sync_symbols(ADVERTISING_ACCESS_ADDRESS);
        let fit = |centre: f64| -> f32 {
            symbol_means(&c.track, centre, SYNC_SYMBOLS)
                .iter()
                .zip(&sync)
                .map(|(m, &b)| if b { *m } else { -m })
                .sum()
        };
        let centre = (-8..=8)
            .map(|q| nominal + q as f64 * 0.25)
            .filter(|&x| x >= 1.5)
            .max_by(|&a, &b| fit(a).total_cmp(&fit(b)))
            .unwrap_or(nominal);
        if sync_agreement(&symbol_means(&c.track, centre, SYNC_SYMBOLS), &sync) < SYNC_AGREEMENT {
            // Not the sync sequence: another packet's S=8 data shaped like
            // it. Not a trigger, and not counted as one.
            self.capture = None;
            return;
        }
        self.funnel.triggered += 1;
        c.stage = Stage::Block1 { centre };
        c.due = samples_for(centre, coded::PREAMBLE_SYMBOLS + coded::BLOCK1_SYMBOLS);
    }

    /// One step of the capture's reading, if it can be taken with what has
    /// arrived: `Some(Some(packet))` when a packet ended, `Some(None)` when
    /// a step was taken or the capture was given up, `None` when it waits
    /// for more.
    fn advance(&mut self) -> Option<Option<Packet>> {
        let c = self.capture.as_mut()?;
        let centre = match c.stage {
            Stage::Peaking => return None,
            Stage::Block1 { centre }
            | Stage::Header { centre, .. }
            | Stage::Body { centre, .. } => centre,
        };
        let means = symbol_means(&c.track, centre, usize::MAX);
        if means.len() > LONGEST_SYMBOLS {
            self.funnel.gave_up += 1;
            self.capture = None;
            return Some(None);
        }
        if means.len() < coded::PREAMBLE_SYMBOLS {
            return None;
        }
        // The preamble's `00111100` is balanced, so its mean is the carrier
        // offset, which the discriminator turned into a constant.
        let offset =
            means[..coded::PREAMBLE_SYMBOLS].iter().sum::<f32>() / coded::PREAMBLE_SYMBOLS as f32;
        let scale = DEVIATION_HZ as f32;
        let readings: Vec<f32> = means.iter().map(|m| (m - offset) / scale).collect();
        let body = &readings[coded::PREAMBLE_SYMBOLS..];
        match c.stage {
            Stage::Peaking => None,
            Stage::Block1 { centre } => {
                let b1 = coded::read_block1(body)?;
                match b1.coding {
                    Some(coding) if b1.access_address == ADVERTISING_ACCESS_ADDRESS => {
                        c.stage = Stage::Header {
                            centre,
                            coding,
                            repairs: b1.repairs,
                        };
                        c.due = samples_for(
                            centre,
                            coded::PREAMBLE_SYMBOLS
                                + coded::BLOCK1_SYMBOLS
                                + (16 + coded::HEADER_LOOKAHEAD_BITS) * coding.symbols_per_bit(),
                        );
                    }
                    _ => {
                        self.funnel.gave_up += 1;
                        self.capture = None;
                    }
                }
                Some(None)
            }
            Stage::Header {
                centre,
                coding,
                repairs,
            } => {
                let block2 = &body[coded::BLOCK1_SYMBOLS.min(body.len())..];
                let header = coded::peek_header(block2, coding, self.channel)?;
                match pdu::length(&header) {
                    Some(length) if length > 0 => {
                        let symbols = coded::PREAMBLE_SYMBOLS
                            + coded::BLOCK1_SYMBOLS
                            + coded::block2_symbols(coding, 16 + 8 * length as usize);
                        c.stage = Stage::Body {
                            centre,
                            coding,
                            repairs,
                            symbols,
                        };
                        c.due = samples_for(centre, symbols);
                    }
                    _ => {
                        self.funnel.gave_up += 1;
                        self.capture = None;
                    }
                }
                Some(None)
            }
            Stage::Body {
                centre,
                coding,
                repairs,
                symbols,
            } => {
                if means.len() < symbols {
                    return None;
                }
                let block2 = &body[coded::BLOCK1_SYMBOLS..symbols - coded::PREAMBLE_SYMBOLS];
                let decoded =
                    coded::read_block2(block2, coding, self.channel).and_then(|(bits, more)| {
                        // The PDU and CRC as sent, whitened again: what the
                        // measurement path rebuilds the packet's symbols from.
                        let mut air = bits.clone();
                        crate::signal::dsp::code::lfsr::whiten(&mut air, self.channel);
                        pdu::decode(&bits).map(|mut p| {
                            p.air = air;
                            (p, more)
                        })
                    });
                let origin = c.origin;
                self.capture = None;
                let Some((mut packet, more)) = decoded else {
                    self.funnel.gave_up += 1;
                    return Some(None);
                };
                if packet.crc_ok {
                    self.funnel.decoded += 1;
                } else {
                    self.funnel.crc_failed += 1;
                }
                let first = coded::PREAMBLE_SYMBOLS + coded::BLOCK1_SYMBOLS;
                packet.coding = Some(coding);
                packet.fec_repairs = Some(repairs + more);
                // A symbol starts half a symbol before its centre.
                let half = WORKING_SPS as f64 / 2.0;
                packet.at_pair = Some(self.pair_at(origin as f64 + centre - half).round() as u64);
                packet.pdu_pair = Some(self.pair_at(
                    origin as f64
                        + centre
                        + (first * WORKING_SPS) as f64
                        + (coding.symbols_per_bit() as f64 - 1.0) * WORKING_SPS as f64 / 2.0,
                ));
                Some(Some(packet))
            }
        }
    }

    /// The stream position, pairs, of working sample `index` (fractional):
    /// output `j` of the decimator stands for input `delay + j * factor`.
    fn pair_at(&self, index: f64) -> f64 {
        self.origin_pair.unwrap_or(0) as f64
            + self.decim.delay()
            + index * self.decim.factor() as f64
    }

    /// The detector's readings on `iq`, for measuring its statistics.
    #[cfg(test)]
    fn noise_readings(&mut self, iq: &[Complex<f32>]) -> Vec<f64> {
        let mut working = Vec::new();
        self.decim.process(iq, &mut working);
        let mut track = Vec::with_capacity(working.len());
        for &s in &working {
            track.push(match self.last_sample {
                Some(prev) => instantaneous_freq_hz(prev, s, WORKING_RATE_HZ),
                None => 0.0,
            });
            self.last_sample = Some(s);
        }
        let mut readings = Vec::new();
        self.matcher.process_block(&track, &mut readings);
        readings.into_iter().flatten().collect()
    }
}

/// How well `means` (one reading a symbol) agree with the sync sequence
/// `sync`: Pearson's correlation with it as ±1, in `[-1, 1]`, blind to a
/// carrier offset (a constant) and to the readings' scale.
fn sync_agreement(means: &[f32], sync: &[bool]) -> f64 {
    let n = means.len().min(sync.len());
    if n == 0 {
        return 0.0;
    }
    let mean = means[..n].iter().map(|&m| m as f64).sum::<f64>() / n as f64;
    let (mut dot, mut energy) = (0.0, 0.0);
    for (&m, &b) in means[..n].iter().zip(sync) {
        let x = m as f64 - mean;
        dot += if b { x } else { -x };
        energy += x * x;
    }
    if energy == 0.0 {
        return 0.0;
    }
    dot / (energy.sqrt() * (n as f64).sqrt())
}

/// The sync symbols' frequency track as this chain receives them: modulated
/// at `raw_rate` between random-looking margins, through [`front_end`] and
/// the discriminator, then cut to the stretch the sync symbols themselves
/// occupy. Built through the chain rather than drawn ideal, because a
/// correlation against a template the filter never shaped is a correlation
/// against the wrong shape.
fn sync_template(raw_rate: f64) -> Result<Vec<f32>, String> {
    let sps = (raw_rate / 1e6).round().max(1.0) as usize;
    let margin: Vec<bool> = (0..TEMPLATE_MARGIN_SYMBOLS)
        .map(|i| (i * 7 + 3) % 5 < 2)
        .collect();
    let mut bits = margin.clone();
    bits.extend(coded::sync_symbols(ADVERTISING_ACCESS_ADDRESS));
    bits.extend(&margin);
    let iq = super::gfsk::modulate(&bits, sps, DEVIATION_HZ, sps as f64 * 1e6, 0.5);
    let mut decim = front_end(raw_rate, FILTER)?;
    let mut working = Vec::new();
    decim.process(&iq, &mut working);
    let track: Vec<f32> = std::iter::once(0.0)
        .chain(
            working
                .windows(2)
                .map(|w| instantaneous_freq_hz(w[0], w[1], WORKING_RATE_HZ)),
        )
        .collect();
    let first = ((TEMPLATE_MARGIN_SYMBOLS * sps) as f64 - decim.delay()) / decim.factor() as f64;
    let first = first.round().max(0.0) as usize;
    let len = SYNC_SYMBOLS * WORKING_SPS;
    track
        .get(first..first + len)
        .map(<[f32]>::to_vec)
        .ok_or_else(|| "the LE Coded sync template ran short".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use num_complex::Complex;

    /// Mean power of `x` once the filter has settled (its first and last
    /// tenth dropped).
    fn settled_power(x: &[Complex<f32>]) -> f64 {
        let skip = x.len() / 10;
        let mid = &x[skip..x.len() - skip];
        mid.iter().map(|s| s.norm_sqr() as f64).sum::<f64>() / mid.len() as f64
    }

    fn tone(freq_hz: f64, rate: f64, n: usize) -> Vec<Complex<f32>> {
        (0..n)
            .map(|i| {
                Complex::from_polar(
                    1.0,
                    (std::f64::consts::TAU * freq_hz * i as f64 / rate) as f32,
                )
            })
            .collect()
    }

    /// Every candidate passes a tone inside its passband and stops one in
    /// its stopband: the filters are what their numbers say.
    #[test]
    fn a_tone_inside_the_passband_passes_and_one_outside_is_stopped() {
        for filter in CANDIDATES {
            let inside = 0.9 * filter.passband_hz();
            let outside = filter.stopband_hz() + 50e3;
            let mut out = Vec::new();
            front_end(20e6, filter)
                .unwrap()
                .process(&tone(inside, 20e6, 200_000), &mut out);
            let pass_db = 10.0 * settled_power(&out).log10();
            assert!(
                pass_db.abs() < 1.0,
                "{filter:?}: {pass_db:.2} dB in the passband"
            );
            front_end(20e6, filter)
                .unwrap()
                .process(&tone(outside, 20e6, 200_000), &mut out);
            let stop_db = 10.0 * settled_power(&out).log10();
            assert!(
                stop_db < -30.0,
                "{filter:?}: {stop_db:.2} dB in the stopband"
            );
        }
    }

    /// Four samples a symbol, each symbol's own four averaged: on a track of
    /// four highs then four lows the readings alternate exactly; half a
    /// symbol late, each reading straddles two symbols and says so.
    #[test]
    fn symbol_means_average_each_symbols_own_period() {
        let track: Vec<f32> = (0..64)
            .map(|i| if (i / 4) % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        let on_time = symbol_means(&track, 1.5, 6);
        assert_eq!(on_time, vec![1.0, -1.0, 1.0, -1.0, 1.0, -1.0]);
        let late = symbol_means(&track, 2.5, 4);
        assert_eq!(late, vec![0.5, -0.5, 0.5, -0.5]);
    }

    /// As many readings as the track holds whole symbols for, no more.
    #[test]
    fn symbol_means_stop_at_the_end_of_the_track() {
        let track = vec![1.0f32; 10];
        assert_eq!(symbol_means(&track, 1.5, 100).len(), 2);
    }

    #[test]
    fn a_rate_that_is_not_a_working_rate_multiple_is_refused() {
        assert!(front_end(10.5e6, FILTER).is_err());
        assert!(front_end(2e6, FILTER).is_err());
        assert!(front_end(20e6, FILTER).is_ok());
    }

    use crate::signal::ble::coded::{self, Coding};
    use crate::signal::ble::{channel, detect, gfsk, pdu};
    use crate::signal::dsp::testkit::Rng;

    const AA: u32 = detect::ADVERTISING_ACCESS_ADDRESS;
    /// An ADV_EXT_IND as LE Coded sends one: ext header length 6, AdvMode 0,
    /// ADI and AuxPtr.
    const EXT_IND: [u8; 7] = [6, 0b0001_1000, 0x23, 0x31, 0x09, 0x64, 0x40];

    /// `symbols` at 1 Msym/s through GFSK at `raw_rate`, placed on `ch` as a
    /// radio tuned to `tuned_hz` sees it, `offset_hz` of crystal error added,
    /// with `lead` random symbols before and after. Returns the samples and
    /// the pair the first of `symbols` starts at.
    fn on_air(
        symbols: &[bool],
        raw_rate: f64,
        ch: u8,
        tuned_hz: f64,
        offset_hz: f64,
        lead: usize,
        rng: &mut Rng,
    ) -> (Vec<Complex<f32>>, u64) {
        let sps = (raw_rate / 1e6).round() as usize;
        let mut bits: Vec<bool> = (0..lead).map(|_| rng.next_u64() & 1 == 1).collect();
        bits.extend(symbols);
        bits.extend((0..lead).map(|_| rng.next_u64() & 1 == 1));
        let shift = channel::centre_hz(ch).unwrap() as f64 - tuned_hz + offset_hz;
        let step = std::f64::consts::TAU * shift / raw_rate;
        let iq = gfsk::modulate(&bits, sps, 250e3, raw_rate, 0.5)
            .into_iter()
            .enumerate()
            .map(|(n, s)| s * Complex::from_polar(1.0, (step * n as f64) as f32))
            .collect();
        (iq, (lead * sps) as u64)
    }

    fn coded_packet(coding: Coding, ch: u8, payload: &[u8]) -> Vec<bool> {
        coded::transmit(AA, coding, ch, 0x07, payload)
    }

    /// A packet built as `coded::transmit` builds one, but with the CI's two
    /// bits as given, reserved values included.
    fn with_ci(ci: [bool; 2], ch: u8, payload: &[u8]) -> Vec<bool> {
        let s8 = |bits: &[bool]| -> Vec<bool> {
            coded::encode(bits)
                .into_iter()
                .flat_map(coded::pattern_map_s8)
                .collect()
        };
        let mut out = coded::preamble_bits().to_vec();
        let mut block1 = detect::access_address_bits(AA).to_vec();
        block1.extend(ci);
        block1.extend([false; 3]);
        out.extend(s8(&block1));
        let mut block2 = pdu::encode(ch, 0x07, payload);
        block2.extend([false; 3]);
        out.extend(s8(&block2));
        out
    }

    fn receive(rx: &mut CodedReceiver, iq: &[Complex<f32>]) -> Vec<pdu::Packet> {
        // In blocks, as the worker hands them over.
        let mut out = Vec::new();
        for (k, block) in iq.chunks(131_072).enumerate() {
            out.extend(rx.push_iq_at(block, (k * 131_072) as u64));
        }
        out
    }

    /// The whole chain on a clean packet: one packet, its CRC passing, its
    /// payload and scheme right, nothing repaired, stamped within a symbol
    /// of where it started; on the advertising channel the radio is tuned
    /// to (38, 2426 MHz) and on data channels four megahertz either side.
    #[test]
    fn a_synthetic_coded_packet_is_received_whole() {
        let mut rng = Rng::new(1);
        for (coding, ch) in [
            (Coding::S8, 38),
            (Coding::S2, 38),
            (Coding::S8, 9),
            (Coding::S2, 12),
        ] {
            let (iq, start) = on_air(
                &coded_packet(coding, ch, &EXT_IND),
                20e6,
                ch,
                2_426e6,
                0.0,
                64,
                &mut rng,
            );
            let mut rx = CodedReceiver::new(20e6, ch, 2_426e6).unwrap();
            let got = receive(&mut rx, &iq);
            assert_eq!(got.len(), 1, "{coding:?} on {ch}");
            let p = &got[0];
            assert!(p.crc_ok, "{coding:?} on {ch}");
            assert_eq!(p.payload, EXT_IND);
            assert_eq!(p.coding, Some(coding));
            assert_eq!(p.fec_repairs, Some(0));
            let at = p.at_pair.unwrap() as f64;
            assert!(
                (at - start as f64).abs() < 20.0,
                "{coding:?}: at {at}, started {start}"
            );
        }
    }

    #[test]
    fn a_coded_packet_with_a_crystal_offset_is_still_heard() {
        let mut rng = Rng::new(2);
        for offset in [-100e3, 100e3] {
            let (iq, _) = on_air(
                &coded_packet(Coding::S8, 12, &EXT_IND),
                20e6,
                12,
                2_426e6,
                offset,
                64,
                &mut rng,
            );
            let mut rx = CodedReceiver::new(20e6, 12, 2_426e6).unwrap();
            let got = receive(&mut rx, &iq);
            assert!(got.len() == 1 && got[0].crc_ok, "{offset} Hz: {got:?}");
        }
    }

    #[test]
    fn noise_alone_triggers_nothing() {
        let mut rng = Rng::new(3);
        let noise = rng.noise(8_000_000, 1.0);
        let mut rx = CodedReceiver::new(4e6, 12, 2_426e6).unwrap();
        assert!(receive(&mut rx, &noise).is_empty());
        assert_eq!(rx.take_funnel().triggered, 0);
    }

    /// An LE 1M packet is not a Coded one: its sync word is not the Coded
    /// sequence, so nothing triggers.
    #[test]
    fn an_le_1m_packet_is_not_a_coded_one() {
        let mut rng = Rng::new(4);
        let mut le1m = detect::preamble_bits(AA, crate::signal::ble::Phy::OneM);
        le1m.extend(detect::access_address_bits(AA));
        le1m.extend(pdu::encode(12, 0x00, &[1, 2, 3, 4, 5, 6, 2, 1, 6]));
        let (iq, _) = on_air(&le1m, 20e6, 12, 2_426e6, 0.0, 64, &mut rng);
        let mut rx = CodedReceiver::new(20e6, 12, 2_426e6).unwrap();
        assert!(receive(&mut rx, &iq).is_empty());
        assert_eq!(rx.take_funnel().triggered, 0);
    }

    /// A reserved CI is refused: the trigger is counted as given up, and
    /// nothing is decoded as S=2 or S=8.
    #[test]
    fn a_reserved_ci_is_counted_not_decoded() {
        let mut rng = Rng::new(5);
        let (iq, _) = on_air(
            &with_ci([false, true], 12, &EXT_IND),
            20e6,
            12,
            2_426e6,
            0.0,
            64,
            &mut rng,
        );
        let mut rx = CodedReceiver::new(20e6, 12, 2_426e6).unwrap();
        assert!(receive(&mut rx, &iq).is_empty());
        let f = rx.take_funnel();
        assert_eq!((f.triggered, f.gave_up, f.decoded), (1, 1, 0));
    }

    #[test]
    fn two_coded_packets_back_to_back_are_both_heard() {
        let mut rng = Rng::new(6);
        let mut both = coded_packet(Coding::S2, 12, &EXT_IND);
        both.extend((0..300).map(|_| rng.next_u64() & 1 == 1));
        both.extend(coded_packet(
            Coding::S2,
            12,
            &[6, 0b0001_1000, 0x24, 0x31, 0x09, 0x64, 0x40],
        ));
        let (iq, _) = on_air(&both, 20e6, 12, 2_426e6, 0.0, 64, &mut rng);
        let mut rx = CodedReceiver::new(20e6, 12, 2_426e6).unwrap();
        let got = receive(&mut rx, &iq);
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|p| p.crc_ok));
        assert_ne!(got[0].payload, got[1].payload);
    }

    /// A stream that breaks in the middle of a packet is not stitched: the
    /// capture is abandoned and counted, and nothing is decoded across the
    /// gap.
    #[test]
    fn a_gap_inside_a_coded_capture_is_not_stitched() {
        let mut rng = Rng::new(7);
        let (iq, start) = on_air(
            &coded_packet(Coding::S8, 12, &EXT_IND),
            20e6,
            12,
            2_426e6,
            0.0,
            64,
            &mut rng,
        );
        let cut = start as usize + 600 * 20;
        let mut rx = CodedReceiver::new(20e6, 12, 2_426e6).unwrap();
        let mut got = rx.push_iq_at(&iq[..cut], 0);
        // A block went missing: the next one starts later than this one ended.
        got.extend(rx.push_iq_at(&iq[cut..], cut as u64 + 131_072));
        assert!(got.is_empty(), "{got:?}");
        // What is left of the packet after the gap is S=8 data, which the
        // second stage of detection does not take for a sync sequence.
        let f = rx.take_funnel();
        assert_eq!((f.triggered, f.gave_up), (1, 1));
    }

    /// A receiver put down in the middle of a capture, as the worker does at
    /// a break it sees itself or on a retune, still ends that trigger: it
    /// gave up, and the funnel it hands back says so.
    #[test]
    fn a_receiver_put_down_mid_capture_counts_it_given_up() {
        let mut rng = Rng::new(7);
        let (iq, start) = on_air(
            &coded_packet(Coding::S8, 12, &EXT_IND),
            20e6,
            12,
            2_426e6,
            0.0,
            64,
            &mut rng,
        );
        let cut = start as usize + 600 * 20;
        let mut rx = CodedReceiver::new(20e6, 12, 2_426e6).unwrap();
        assert!(rx.push_iq_at(&iq[..cut], 0).is_empty());
        let f = rx.finish();
        assert_eq!((f.triggered, f.gave_up), (1, 1));
        assert_eq!(f.triggered, f.decoded + f.crc_failed + f.gave_up);
    }

    /// After `reset`, a window far later is read as if it were the first.
    #[test]
    fn reset_forgets_the_stream() {
        let mut rng = Rng::new(8);
        let (iq, _) = on_air(
            &coded_packet(Coding::S2, 12, &EXT_IND),
            20e6,
            12,
            2_426e6,
            0.0,
            64,
            &mut rng,
        );
        let mut rx = CodedReceiver::new(20e6, 12, 2_426e6).unwrap();
        assert_eq!(rx.push_iq_at(&iq, 0).len(), 1);
        rx.reset();
        let again = rx.push_iq_at(&iq, 50_000_000);
        assert_eq!(again.len(), 1);
        assert!(again[0].at_pair.unwrap() > 50_000_000);
    }

    /// Every trigger ends one way and is counted once: a clean packet, one
    /// with a reserved CI, and one whose CRC fails (a CRC bit flipped before
    /// the FEC, so the code delivers it faithfully and the CRC catches it).
    #[test]
    fn the_funnel_counts_each_trigger_by_how_it_ended() {
        let mut rng = Rng::new(9);
        let s8 = |bits: &[bool]| -> Vec<bool> {
            coded::encode(bits)
                .into_iter()
                .flat_map(coded::pattern_map_s8)
                .collect()
        };
        let mut bad_crc = coded::preamble_bits().to_vec();
        let mut block1 = detect::access_address_bits(AA).to_vec();
        block1.extend([false, false, false, false, false]);
        bad_crc.extend(s8(&block1));
        let mut block2 = pdu::encode(12, 0x07, &EXT_IND);
        let last = block2.len() - 1;
        block2[last] = !block2[last];
        block2.extend([false; 3]);
        bad_crc.extend(s8(&block2));

        let gap =
            |rng: &mut Rng| -> Vec<bool> { (0..400).map(|_| rng.next_u64() & 1 == 1).collect() };
        let mut all = coded_packet(Coding::S8, 12, &EXT_IND);
        all.extend(gap(&mut rng));
        all.extend(with_ci([true, true], 12, &EXT_IND));
        all.extend(gap(&mut rng));
        all.extend(bad_crc);
        let (iq, _) = on_air(&all, 20e6, 12, 2_426e6, 0.0, 64, &mut rng);
        let mut rx = CodedReceiver::new(20e6, 12, 2_426e6).unwrap();
        let got = receive(&mut rx, &iq);
        assert_eq!(got.len(), 2, "the clean one and the failed CRC");
        assert_eq!(got.iter().filter(|p| p.crc_ok).count(), 1);
        let f = rx.take_funnel();
        assert_eq!(
            (f.triggered, f.decoded, f.crc_failed, f.gave_up),
            (3, 1, 1, 1)
        );
    }

    /// The detector's statistic under noise alone: Pearson's correlation of
    /// the discriminator through this chain's front end with the sync
    /// symbols' frequency track, close to normal about zero with variance
    /// `1 / N_eff`. Measured, and held to [`SYNC_N_EFF`] so a front-end
    /// change that moves it fails here rather than moving the threshold's
    /// meaning quietly.
    #[test]
    fn the_coded_detector_s_noise_statistics_are_the_ones_measured() {
        let mut rng = Rng::new(10);
        let mut rx = CodedReceiver::new(4e6, 12, 2_426e6).unwrap();
        let readings = rx.noise_readings(&rng.noise(4_000_000, 1.0));
        let n = readings.len() as f64;
        let mean = readings.iter().sum::<f64>() / n;
        let var = readings.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / n;
        let n_eff = 1.0 / var;
        assert!(mean.abs() < 0.01, "mean {mean}");
        assert!(
            (n_eff / SYNC_N_EFF - 1.0).abs() < 0.1,
            "measured N_eff {n_eff:.1}, pinned {SYNC_N_EFF}"
        );
    }

    /// S=8 data is shaped like the coded sync sequence within every pattern,
    /// so the first stage of detection can fire on it; the second, on each
    /// symbol's mean, does not take it for a sync sequence.
    #[test]
    fn s8_data_is_not_taken_for_a_sync_sequence() {
        let mut rng = Rng::new(11);
        let bits: Vec<bool> = (0..2000).map(|_| rng.next_u64() & 1 == 1).collect();
        let data: Vec<bool> = coded::encode(&bits)
            .into_iter()
            .flat_map(coded::pattern_map_s8)
            .collect();
        let (iq, _) = on_air(&data, 20e6, 38, 2_426e6, 0.0, 64, &mut rng);
        let mut rx = CodedReceiver::new(20e6, 38, 2_426e6).unwrap();
        assert!(receive(&mut rx, &iq).is_empty());
        assert_eq!(rx.take_funnel().triggered, 0);
    }
}
