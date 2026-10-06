// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! One classic Bluetooth channel's live receiver: mix it to baseband,
//! decimate, discriminate, slice bits at a handful of candidate phases, and
//! hand every bit to its own [`super::detect::Detector`].
//!
//! **Free-running, not triggered.** `signal::ble::receive::Receiver`
//! correlates raw IQ against a *known* sync word to find where a packet
//! starts before it ever slices a bit. Classic Bluetooth has no such
//! reference to correlate against - the LAP, and so the access code, is
//! exactly what a passive receiver does not know - so
//! this receiver never waits for a trigger. It slices continuously, from the
//! moment it is built, and lets [`super::detect::Detector`] decide on every
//! new bit whether the last 64 form a real access code.
//!
//! **Multiple channels, one capture - capped, not unbounded.** A single
//! ~20 MHz capture already contains on the order of 20 of the 79 classic BT
//! channels at once, each only 1 MHz wide.
//! `signal::net::worker` builds one [`Receiver`] per channel it decides to
//! watch, each with its own [`crate::signal::dsp::nco::Nco`] mixing that one
//! channel down to its own baseband - unlike BLE's receiver, which only ever
//! runs when the radio is tuned exactly to the channel it decodes.
//!
//! **Measured, and found too expensive to run unbounded.** A channel-select
//! filter tight enough to matter at 1 MHz spacing needs a real tap count -
//! `front_end`'s own doc has the numbers - and paying that once per watched
//! channel is far over a budget of a few operations per sample for
//! always-on detection. `signal::net::worker` therefore caps
//! how many of the channels in view are actually given a receiver
//! (`[net].bt_channels` in the config, small by default), choosing the ones
//! nearest the tuned centre first, and says on screen how many of the total
//! it is watching: an honest, coarser step than watching every one.
//!
//! **No active symbol-timing recovery.** With no known preamble to anchor a
//! phase search on (the way `signal::ble::sync::slice` anchors one), this
//! runs [`PHASES`] independent free-running slicers per channel, one per
//! sample offset within a symbol period, and lets whichever one happens to
//! land close enough to the true phase find the access code on its own. A
//! crystal's drift over one access code's 64-bit, 64-microsecond duration is
//! negligible next to a quarter-symbol phase error, so a fixed offset chosen
//! once at start-up does not need to track anything for a window this
//! short - the same reasoning that lets `sync::slice` search its own phase
//! once and hold it, taken a step further because this receiver does not
//! even get a burst boundary to search from.
//!
//! **What the air has and has not confirmed.** Every test below is
//! synthetic. Live captures have since found real piconets and resolved
//! their UAPs, but the channel-select filter's real adjacent-channel
//! rejection, as opposed to what it measures against a synthetic
//! interferer, is unmeasured.

use num_complex::Complex;

#[cfg(test)]
use crate::hardware::SampleGeometry;
#[cfg(test)]
use crate::signal::demod::decode as decode_iq;
use crate::signal::dsp::discriminate::instantaneous_freq_hz;
use crate::signal::dsp::fir::{design_lowpass_to_spec, StreamingDecimator};
use crate::signal::dsp::nco::Nco;

use super::channel;
use super::detect::Detector;
use super::header;

/// Classic BT's own symbol rate: 1 Mb/s, fixed for the basic rate physical
/// layer: the same GFSK chain as BLE's.
const SYMBOL_RATE_HZ: f64 = 1_000_000.0;

/// One access code found: its LAP, and when its last bit was sliced, in µs
/// on the stream's own sample clock. To a quarter
/// symbol: the working-rate sample the lane sliced, not the symbol count,
/// so a slot grid can be fitted to it (`super::slots`). A constant delay
/// (the decimator's, the discriminator's first sample, the access code's
/// own length) sits in every hit alike and does not move a grid's
/// residuals.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AccessHit {
    pub lap: u32,
    pub at_us: f64,
}

/// One header captured and FEC-decoded after some lane's own access-code
/// hit - still whitened, not yet attributable to a UAP. `signal::net::
/// worker` owns the per-LAP `header::PiconetClock` that turns a run of
/// these into one, the same way it (not this receiver) owns `signal::net::
/// census`'s own per-address aggregation.
pub struct HeaderHit {
    pub lap: u32,
    pub whitened: [bool; header::HEADER_BITS],
    /// The raw, still-whitened bits captured immediately after the
    /// header, up to [`PAYLOAD_CAPTURE_BITS`] of them regardless of what
    /// packet type this header turns out to name - this lane has no way
    /// to know that without a confirmed UAP, which only `signal::net::
    /// worker`'s own `header::PiconetClock` computes. `payload::
    /// verify_crc`/`break_uap_tie`'s own `raw` parameter, unchanged.
    pub payload_raw: Vec<bool>,
    /// The channel it was heard on.
    pub ch: u8,
    /// Where on the stream the last sync-word bit was sliced, in raw sample
    /// pairs ([`crate::hardware::StreamBlock::first_pair`]'s clock),
    /// fractional: where the measurement finds the burst in the raw samples.
    /// Taken at the lane that fired, so within a symbol of the bit's centre,
    /// not at it; the measurement times the burst from its own sync word.
    pub sync_end_pair: f64,
    /// When the access code before it ended, dated as
    /// [`AccessHit::at_us`] is: what joins a header to its hit in the
    /// export.
    pub at_us: f64,
}

/// How many raw, still-whitened bits after a header this receiver keeps
/// capturing alongside it: DM5's worst case, 228 bytes of payload under the
/// rate 2/3 FEC, 183 codewords of 15 bits, one bit more than DH5's 343
/// bytes sent bare. `payload_capture_covers_every_supported_types_own_
/// worst_case` (below) holds this number to `payload::max_payload_length`
/// and `payload::raw_bits_for` for every type they read, rather than
/// trusting the figures to stay in step by hand. A real payload shorter than this
/// (DH1's own 30-byte ceiling, most of the time) simply leaves this
/// capture's own tail sitting past the payload's real end, which `verify_
/// crc` never reads: it trusts the payload header's own LENGTH field, not
/// how much this receiver happened to buffer.
const PAYLOAD_CAPTURE_BITS: usize = 183 * 15;

/// How many raw air bits a header itself occupies after the access
/// code - the 4-bit trailer, never read, plus FEC(1/3)'s own
/// 54 air bits for [`header::HEADER_BITS`] host bits.
const HEADER_CAPTURE_BITS: usize = header::TRAILER_BITS + header::HEADER_AIR_BITS;

/// How close two hits of one LAP are, in working samples, to be one packet:
/// four symbols. The lanes of one packet fire within a symbol of each
/// other; the next packet of the same piconet comes at least a whole access
/// code later.
const SAME_PACKET_SAMPLES: u64 = 4 * PHASES as u64;

/// Whether `lap` at working sample `at` is a packet `log` already holds.
fn same_packet(log: &[(u32, u64)], lap: u32, at: u64) -> bool {
    log.iter()
        .any(|&(l, s)| l == lap && at.saturating_sub(s) <= SAME_PACKET_SAMPLES)
}

/// One lane's own header-plus-payload capture in progress: the bits
/// collected so far after that lane's `Detector` last fired, and which
/// LAP it was for.
struct PendingHeader {
    lap: u32,
    /// Trailer bits first, header air bits next
    /// ([`HEADER_CAPTURE_BITS`] in total), the raw payload region after
    /// that ([`PAYLOAD_CAPTURE_BITS`] more, once [`Self::header_whitened`]
    /// is set).
    bits: Vec<bool>,
    /// Set the moment the header's own air bits complete and pass FEC -
    /// `None` while still capturing the header itself. A header that
    /// fails FEC never sets this; that capture is abandoned right there
    /// instead ([`Receiver::push`]'s own doc), since no amount of payload
    /// afterward can be attributed to a header that could not even be read.
    header_whitened: Option<[bool; header::HEADER_BITS]>,
    /// [`HeaderHit::sync_end_pair`].
    sync_end_pair: f64,
    /// The working sample its access code was found at, counted as the
    /// receiver counts them: which packet it is ([`SAME_PACKET_SAMPLES`]).
    start_sample: u64,
    /// When the access code that started it ended ([`AccessHit::at_us`]).
    at_us: f64,
}

/// Samples per symbol this receiver decimates to, and so also the number of
/// independent free-running phase lanes it runs - see the module doc's own
/// note on why there is no active timing recovery to pick one instead.
const PHASES: usize = 4;
const WORKING_RATE_HZ: f64 = SYMBOL_RATE_HZ * PHASES as f64;

/// The channel-select filter's passband edge and transition, in Hz, and the
/// stopband it is designed to. Chosen the way
/// `signal::ble::receive::front_end` chose its own, but deliberately
/// modest rather than matched to it - and chosen by measurement, not by
/// the first number that seemed reasonable.
///
/// **The first, narrower attempt (250 kHz plus a 250 kHz transition,
/// summing to half this channel's own 1 MHz spacing) measurably clipped
/// this receiver's *own wanted signal*, not just a neighbour's.** Classic
/// BT's own occupied bandwidth is roughly 1.3 MHz by Carson's rule for a
/// ~160 kHz peak deviation GFSK signal at 1 Mb/s - wider than the channel
/// spacing itself, so a filter narrow enough to leave real room before the
/// next channel's centre cuts into content this receiver needs. Measured
/// directly against a synthetic access code: the narrower filter recovered
/// roughly 75-80% of bits correctly *regardless of SNR*, from 25 dB up to
/// 60 dB - the signature of a deterministic distortion, not noise, and far
/// too lossy for [`super::detect::Detector`]'s exact-match-only rule to
/// ever complete a clean 64-bit window. Widening to the numbers below
/// recovered a clean, error-free bit sequence on at least one of
/// [`PHASES`] lanes at every SNR tried down to 25 dB.
///
/// **The honest cost of that fix: this filter now passes real energy from
/// the very next channel.** 500 kHz plus a 200 kHz transition reaches
/// 700 kHz either side of centre, on channels 1 MHz apart - there is
/// almost no stopband left before a neighbour's own centre frequency. This
/// receiver's own adjacent-channel rejection is therefore weak by
/// construction, not merely imperfect, and nothing here has measured how
/// weak against a real interferer; that is still unmeasured air, the same
/// honest gap the module doc's own closing paragraph names.
///
/// **The stopband is 25 dB, not `signal::ble::receive::front_end`'s 40, and
/// that is a measured trade-off, not an oversight.** A Kaiser filter's own tap
/// count is set by its transition width and its stopband depth together
/// (`dsp::fir::kaiser_taps`), and this filter runs once per *watched channel*,
/// continuously, at [`WORKING_RATE_HZ`] output samples a second each. At 20
/// Msps this design (121 taps) costs on the order of 484 million complex
/// multiply-adds a second *per channel* - measured directly, not estimated -
/// which for
/// [`super::super::net::worker::SAFE_BT_CHANNELS`]
/// simultaneous channels is still well past a budget of a few operations per
/// sample for always-on detection; a 40 dB stopband at the same transition would cost
/// close to three times that. `NetWorker` additionally caps *how many* channels
/// get a receiver at all rather than relying on the filter alone to make every
/// one of them cheap.
const CHANNEL_SELECT_CUTOFF_HZ: f64 = 500_000.0;
const CHANNEL_SELECT_TRANSITION_HZ: f64 = 200_000.0;
const CHANNEL_SELECT_STOPBAND_DB: f64 = 25.0;

/// How quickly the per-channel DC bias tracker follows a change - a leaky
/// integrator, not a hard reset, so a slow drift is followed and a single
/// sample's own noise is not. `1/256` settles within a few hundred symbols,
/// fast next to how long any one access code needs to sit still for (64
/// symbols) and slow next to a single symbol's own noise.
const BIAS_ALPHA: f32 = 1.0 / 256.0;

/// The tracker learns only from readings within this of zero, in Hz. A
/// transmitter's own readings stay inside it: 175 kHz of deviation at most,
/// 75 kHz of its carrier tolerance (Core 5.4 Vol 2 Part A 3.1.3) and some
/// 50 kHz of our own oscillator's. The discriminator on noise between
/// packets reads anywhere to the working rate's edge, and learning from
/// that walked the threshold 35 kHz rms at 20 Msps, where packets arriving
/// at 60 kHz or more were missed on every lane; clipped here, 24 kHz, and
/// none missed (48 packets at 40 and 25 dB, 4, 8 and 20 Msps).
const BIAS_CLIP_HZ: f32 = 400_000.0;

/// Build the decimator from `raw_rate` to [`WORKING_RATE_HZ`], or say why it
/// cannot be built - the same refusal shape
/// `signal::ble::receive::front_end` uses for the same reason: a decode
/// running against a rate it silently disagreed with about would be wrong in
/// a way nothing on screen would say.
fn front_end(raw_rate: f64) -> Result<StreamingDecimator, String> {
    if raw_rate < WORKING_RATE_HZ {
        return Err(format!(
            "classic Bluetooth decode needs at least {:.1} Msps; the radio is at {:.3} Msps",
            WORKING_RATE_HZ / 1e6,
            raw_rate / 1e6
        ));
    }
    let d = (raw_rate / WORKING_RATE_HZ).round().max(1.0) as usize;
    let achieved = raw_rate / d as f64;
    if (achieved - WORKING_RATE_HZ).abs() > WORKING_RATE_HZ * 0.01 {
        return Err(format!(
            "classic Bluetooth decode needs a sample rate near a whole multiple of {:.1} Msps; {:.3} Msps is not one",
            WORKING_RATE_HZ / 1e6,
            raw_rate / 1e6
        ));
    }
    let taps = design_lowpass_to_spec(
        CHANNEL_SELECT_CUTOFF_HZ / raw_rate,
        CHANNEL_SELECT_TRANSITION_HZ / raw_rate,
        CHANNEL_SELECT_STOPBAND_DB,
    );
    Ok(StreamingDecimator::new(taps, d))
}

/// One classic BT channel's live, free-running receiver.
pub struct Receiver {
    ch: u8,
    raw_rate: f64,
    tuned_centre_hz: f64,
    mixer: Nco,
    decim: StreamingDecimator,
    /// The last decimated sample of the previous block, so the discriminator
    /// carries across block boundaries rather than dropping or repeating one
    /// sample per block - `signal::dsp::discriminate::discriminate`'s own
    /// batch form has no memory of its own, so this receiver keeps it.
    last_sample: Option<Complex<f32>>,
    /// A running estimate of this channel's own DC bias, tracked from the
    /// discriminator output. A channel whose own centre lands near the
    /// radio's tuned frequency sees the front end's own LO leakage sitting
    /// at its baseband DC - here there is no captured burst to average over
    /// the way `signal::ble::sync::slice` has, so it is tracked continuously
    /// instead.
    bias: f32,
    /// Which of [`PHASES`] decimated samples is next for each lane - lane
    /// `p` slices the sample whose position (mod [`PHASES`]) equals `p`.
    lane: usize,
    detectors: [Detector; PHASES],
    /// How many symbols each lane has sliced since this receiver was built.
    /// Exact integer arithmetic, but only valid while the samples are
    /// unbroken - which is why the worker rebuilds the receiver at every
    /// break.
    lane_symbols: [u64; PHASES],
    /// Where on the stream's own clock this receiver's first sample sits, in
    /// µs: [`AccessHit::at_us`]'s origin, exact, so a hit's time is the
    /// stream's and not this receiver's.
    anchor_us: f64,
    /// The stream position of this receiver's first raw sample.
    first_pair: u64,
    /// The last few hits, as (LAP, working sample): what makes a lane's hit
    /// the same packet as another's ([`SAME_PACKET_SAMPLES`]).
    recent_hits: Vec<(u32, u64)>,
    /// The same for headers, by the working sample their capture began at.
    recent_headers: Vec<(u32, u64)>,
    /// One header capture in progress per lane, if any. A second hit on a
    /// lane that already has one pending does not restart it - finishing
    /// the older capture first is a small, honest simplification, not a
    /// claim that this could never lose a genuinely new packet to it.
    pending: [Option<PendingHeader>; PHASES],
}

impl Receiver {
    /// Build a receiver for `ch` at `raw_rate`, currently tuned to
    /// `tuned_centre_hz` - the offset the mixer needs, since a classic BT
    /// channel's own absolute frequency is fixed but its position inside
    /// *this* capture depends on where the radio is tuned right now.
    ///
    /// `first_pair` is the stream position of the first block this receiver
    /// will be given ([`crate::hardware::StreamBlock::first_pair`]); it
    /// anchors the receiver's times to the stream's own clock.
    pub fn new(
        raw_rate: f64,
        ch: u8,
        tuned_centre_hz: f64,
        first_pair: u64,
    ) -> Result<Self, String> {
        let channel_hz = channel::centre_hz(ch)
            .ok_or_else(|| format!("classic Bluetooth has no channel {ch}"))?;
        let decim = front_end(raw_rate)?;
        // Mixing a signal sitting at `+offset` down to baseband takes an
        // oscillator at `-offset` - `dsp::nco::Nco`'s own documented
        // convention.
        let offset_hz = channel_hz as f64 - tuned_centre_hz;
        let mixer = Nco::new(-offset_hz, raw_rate);
        Ok(Self {
            ch,
            raw_rate,
            tuned_centre_hz,
            mixer,
            decim,
            last_sample: None,
            bias: 0.0,
            lane: 0,
            detectors: [Detector::new(); PHASES],
            lane_symbols: [0; PHASES],
            anchor_us: first_pair as f64 * 1e6 / raw_rate,
            first_pair,
            recent_hits: Vec::new(),
            recent_headers: Vec::new(),
            pending: std::array::from_fn(|_| None),
        })
    }

    pub fn channel(&self) -> u8 {
        self.ch
    }

    /// Whether this receiver is still the right one for `ch` at `raw_rate`,
    /// tuned to `tuned_centre_hz` - any of the three changing invalidates
    /// the mixer's own offset or the decimator's own filter, so the caller
    /// rebuilds rather than reusing, the same shape
    /// `signal::ble::receive::Receiver::matches` uses for its own two.
    pub fn matches(&self, ch: u8, raw_rate: f64, tuned_centre_hz: f64) -> bool {
        self.ch == ch
            && (self.raw_rate - raw_rate).abs() < 1.0
            && (self.tuned_centre_hz - tuned_centre_hz).abs() < 1.0
    }

    /// Feed one block of raw device bytes. Returns every LAP found in it -
    /// almost always none.
    ///
    /// **One hit a packet, by time, not one a lane or one a block.** With no
    /// active timing recovery, more than one of the [`PHASES`] lanes
    /// routinely lands close enough to the true phase to decode the same
    /// real transmission cleanly - measured directly: a single synthetic
    /// packet was found on three of the four lanes at once. The lanes of one
    /// packet fire within a symbol of each other, and two packets of one
    /// piconet are at least an access code apart, so a LAP found again
    /// within [`SAME_PACKET_SAMPLES`] of its last hit is that packet, across
    /// block boundaries too. This used to be one hit per LAP per block,
    /// which counted a piconet sending every 625 us as one packet in each
    /// block: at the harness's block of 65 536 pairs, 3 of 12 packets at
    /// 4 Msps and 5 at 8, and the slot grid and the paging rhythm read from
    /// what was left.
    ///
    /// **Header capture rides alongside the same loop, and now runs past
    /// the header itself into the raw payload region.** A lane with a
    /// capture already in progress appends this bit to it before anything
    /// else happens this iteration; a lane whose `Detector` fires this bit
    /// starts a fresh capture, empty, so the access code's own last bit is
    /// never mistaken for the trailer's first one. The moment
    /// [`HEADER_CAPTURE_BITS`] completes, the header's own FEC(1/3) is
    /// decoded once and cached (a header that fails it abandons the
    /// capture right there - no payload capture is kept for a header this
    /// arc could not even read); once [`PAYLOAD_CAPTURE_BITS`] more
    /// arrive, a [`HeaderHit`] carrying both is emitted.
    #[cfg(test)]
    pub fn push(
        &mut self,
        bytes: &[u8],
        geometry: SampleGeometry,
    ) -> (Vec<AccessHit>, Vec<HeaderHit>) {
        let mut iq = Vec::new();
        decode_iq(bytes, geometry, usize::MAX, &mut iq);
        self.push_iq(&iq)
    }

    /// [`Self::push`] on a block already decoded, mixed into this channel's
    /// own buffer: the worker decodes each block once for every watched
    /// channel, where each channel used to decode its own copy.
    pub fn push_iq(&mut self, iq: &[Complex<f32>]) -> (Vec<AccessHit>, Vec<HeaderHit>) {
        let mut mixed = Vec::new();
        self.mixer.mix_into(iq, &mut mixed);
        let mut working = Vec::new();
        self.decim.process(&mixed, &mut working);

        let mut found = Vec::new();
        let mut headers = Vec::new();
        const TOTAL_CAPTURE_BITS: usize = HEADER_CAPTURE_BITS + PAYLOAD_CAPTURE_BITS;
        for &sample in &working {
            let prev = self.last_sample.replace(sample);
            let Some(prev) = prev else { continue };
            let freq = instantaneous_freq_hz(prev, sample, WORKING_RATE_HZ);
            self.bias += (freq.clamp(-BIAS_CLIP_HZ, BIAS_CLIP_HZ) - self.bias) * BIAS_ALPHA;
            let bit = freq > self.bias;

            if let Some(pending) = self.pending[self.lane].as_mut() {
                pending.bits.push(bit);
                if pending.header_whitened.is_none() && pending.bits.len() == HEADER_CAPTURE_BITS {
                    match header::unfec13(&pending.bits[header::TRAILER_BITS..]) {
                        Some(whitened) => pending.header_whitened = Some(whitened),
                        None => self.pending[self.lane] = None,
                    }
                } else if pending.bits.len() == TOTAL_CAPTURE_BITS {
                    let pending = self.pending[self.lane].take().unwrap();
                    let whitened = pending.header_whitened.expect(
                        "reaching the total capture length implies the header already decoded",
                    );
                    // One header a packet, by when its capture began, the
                    // same way hits are counted: more than one lane
                    // routinely captures the same real header cleanly.
                    let start = pending.start_sample;
                    self.recent_headers
                        .retain(|&(_, s)| start.saturating_sub(s) <= SAME_PACKET_SAMPLES);
                    if !same_packet(&self.recent_headers, pending.lap, start) {
                        self.recent_headers.push((pending.lap, start));
                        headers.push(HeaderHit {
                            lap: pending.lap,
                            whitened,
                            payload_raw: pending.bits[HEADER_CAPTURE_BITS..].to_vec(),
                            ch: self.ch,
                            sync_end_pair: pending.sync_end_pair,
                            at_us: pending.at_us,
                        });
                    }
                }
            }

            if let Some(lap) = self.detectors[self.lane].push(bit) {
                // The working-rate sample this lane just sliced.
                let sample = self.lane_symbols[self.lane] * PHASES as u64 + self.lane as u64;
                let at_us = self.anchor_us + sample as f64 * 1e6 / WORKING_RATE_HZ;
                // That reading sits between working samples `sample` and
                // `sample + 1` (the first sample only primes the
                // discriminator), and working sample `j` stands for raw
                // instant `delay + j * factor` from this receiver's first.
                let sync_end_pair = self.first_pair as f64
                    + self.decim.delay()
                    + (sample as f64 + 0.5) * self.decim.factor() as f64;
                self.recent_hits
                    .retain(|&(_, s)| sample.saturating_sub(s) <= SAME_PACKET_SAMPLES);
                if !same_packet(&self.recent_hits, lap, sample) {
                    self.recent_hits.push((lap, sample));
                    found.push(AccessHit { lap, at_us });
                }
                if self.pending[self.lane].is_none() {
                    self.pending[self.lane] = Some(PendingHeader {
                        lap,
                        bits: Vec::with_capacity(TOTAL_CAPTURE_BITS),
                        header_whitened: None,
                        sync_end_pair,
                        start_sample: sample,
                        at_us,
                    });
                }
            }

            self.lane_symbols[self.lane] += 1;
            self.lane = (self.lane + 1) % PHASES;
        }
        (found, headers)
    }
}

#[cfg(test)]
mod tests {
    use super::super::payload;
    use super::*;
    use crate::hardware::SampleFormat;
    use crate::signal::ble::gfsk::modulate;
    use crate::signal::bt::access_code::access_code_bits;
    use crate::signal::dsp::testkit::{at_snr, Rng};

    fn eight_bit() -> SampleGeometry {
        SampleGeometry {
            format: SampleFormat::Int8,
            full_scale: 128.0,
        }
    }

    fn to_bytes(iq: &[Complex<f32>], geometry: SampleGeometry) -> Vec<u8> {
        iq.iter()
            .flat_map(|s| {
                let re = (s.re * geometry.full_scale).clamp(-127.0, 127.0) as i8;
                let im = (s.im * geometry.full_scale).clamp(-127.0, 127.0) as i8;
                [re as u8, im as u8]
            })
            .collect()
    }

    /// How many settling symbols of real, alternating content sit either
    /// side of the access code under test - not a bare few bits. Two
    /// separate filters have their own edge transient to clear first: the
    /// GFSK modulator's own Gaussian pulse shaping, and this receiver's own
    /// channel-select decimator, whose 121-tap filter (`front_end`'s own
    /// doc) costs three symbols of group delay on its own, measured
    /// directly rather than assumed. `signal::ble::receive::
    /// matched_reference` pads by 16 symbols either side for the same
    /// reason, at a shorter filter; this is more generous because the
    /// margin costs nothing in a test built to have room for it.
    const SETTLE_SYMBOLS: usize = 40;

    /// Build a clean, GFSK-modulated access code (BR is the same GFSK chain
    /// BLE already models), already mixed
    /// to where `ch` sits relative to `tuned_centre_hz` inside a wideband
    /// capture.
    fn place_on_channel(
        raw_rate: f64,
        ch: u8,
        tuned_centre_hz: f64,
        lap: u32,
    ) -> Vec<Complex<f32>> {
        let channel_hz = channel::centre_hz(ch).unwrap() as f64;
        let mut bits: Vec<bool> = (0..SETTLE_SYMBOLS).map(|i| i % 2 == 0).collect();
        bits.extend(access_code_bits(lap));
        bits.extend((0..SETTLE_SYMBOLS).map(|i| i % 2 == 1));
        let sps = (raw_rate / SYMBOL_RATE_HZ) as usize;
        let clean = modulate(&bits, sps, 160_000.0, raw_rate, 0.5);
        let noisy = at_snr(&clean, 40.0, &mut Rng::new(1));
        let mut placed = noisy;
        Nco::new(channel_hz - tuned_centre_hz, raw_rate).mix(&mut placed);
        placed
    }

    /// End to end, with no worker or state
    /// involved: a synthetic classic-BT access code sitting on one channel
    /// of a wideband capture is found.
    #[test]
    fn a_clean_access_code_on_one_channel_of_a_wideband_capture_is_found() {
        const RAW_RATE: f64 = 20_000_000.0;
        const TUNED_CENTRE: f64 = 2_441_000_000.0; // channel 39
        let ch = 39u8;
        let lap = 0x0055_aa11;
        let placed = place_on_channel(RAW_RATE, ch, TUNED_CENTRE, lap);
        let geometry = eight_bit();
        let bytes = to_bytes(&placed, geometry);

        let mut rx = Receiver::new(RAW_RATE, ch, TUNED_CENTRE, 0).unwrap();
        let (found, _headers) = rx.push(&bytes, geometry);
        let found: Vec<u32> = found.iter().map(|h| h.lap).collect();
        assert_eq!(found, vec![lap], "{found:?}");
    }

    /// **Every packet is a hit, however many of one piconet's share a
    /// block.** Two access codes of one LAP 700 us apart in a single block
    /// are two hits, not one: a piconet sends every 625 us, and a radio's
    /// block holds several milliseconds. And each is still one hit, not one
    /// a lane, when the block is cut in the middle of the second's sync
    /// word and the lanes finish it on either side of the cut.
    #[test]
    fn two_packets_of_one_piconet_in_one_block_are_two_hits() {
        const RAW_RATE: f64 = 4_000_000.0;
        const TUNED_CENTRE: f64 = 2_441_000_000.0;
        let (ch, lap) = (39u8, 0x0055_aa11);
        let geometry = eight_bit();
        let one = place_on_channel(RAW_RATE, ch, TUNED_CENTRE, lap);
        let mut iq = one.clone();
        iq.extend(vec![Complex::new(0.0f32, 0.0); 700 * 4]);
        let second = iq.len();
        iq.extend(one.iter().copied());
        let bytes = to_bytes(&iq, geometry);

        let mut rx = Receiver::new(RAW_RATE, ch, TUNED_CENTRE, 0).unwrap();
        let (found, _) = rx.push(&bytes, geometry);
        assert_eq!(found.len(), 2, "{found:?}");
        let apart = found[1].at_us - found[0].at_us;
        let expected = second as f64 / 4.0;
        assert!((apart - expected).abs() < 1.0, "{apart} vs {expected}");

        // Cut where the second access code's last bits are being sliced.
        let cut = 2 * (second + (SETTLE_SYMBOLS + 63) * 4);
        let mut rx = Receiver::new(RAW_RATE, ch, TUNED_CENTRE, 0).unwrap();
        let (a, _) = rx.push(&bytes[..cut], geometry);
        let (b, _) = rx.push(&bytes[cut..], geometry);
        assert_eq!(a.len() + b.len(), 2, "{a:?} then {b:?}");
    }

    /// **Noise between packets does not walk the slicing threshold away.**
    /// The bias tracker learns the carrier from the discriminator, and on
    /// noise the discriminator reads anywhere to the working rate's edge; at
    /// 20 Msps, where the channel filter folds five bands of its stopband
    /// into the working band, that walked the threshold 35 kHz rms, and
    /// packets arriving at 60 kHz or more were missed on every lane. Here:
    /// 50 ms of noise alone, the threshold checked after every block.
    #[test]
    fn noise_between_packets_does_not_walk_the_threshold_away() {
        const RAW_RATE: f64 = 20_000_000.0;
        const TUNED_CENTRE: f64 = 2_441_000_000.0;
        let geometry = eight_bit();
        let mut rng = Rng::new(3);
        let mut rx = Receiver::new(RAW_RATE, 39, TUNED_CENTRE, 0).unwrap();
        let mut worst = 0.0f32;
        for _ in 0..50 {
            // Noise at the level a 20 dB packet would stand over, in 1 MHz.
            let block = rng.noise(20_000, 0.25 / 100.0 * 20.0);
            rx.push(&to_bytes(&block, geometry), geometry);
            worst = worst.max(rx.bias.abs());
        }
        assert!(worst < 60_000.0, "the threshold walked to {worst} Hz");
    }

    /// **A hit is dated to a quarter symbol on the stream's clock** (6.5):
    /// the same access code placed 250 µs later in the capture is found
    /// 250 µs later, and a receiver built at a later stream position dates
    /// the same sample alike, since the anchor is exact rather than rounded
    /// to a symbol.
    #[test]
    fn a_hit_is_dated_on_the_streams_own_clock() {
        const RAW_RATE: f64 = 20_000_000.0;
        const TUNED_CENTRE: f64 = 2_441_000_000.0;
        let (ch, lap) = (39u8, 0x0055_aa11);
        let geometry = eight_bit();
        let at = |lead_us: usize, first_pair: u64| {
            let mut iq = vec![Complex::new(0.0f32, 0.0); lead_us * 20];
            iq.extend(place_on_channel(RAW_RATE, ch, TUNED_CENTRE, lap));
            let mut rx = Receiver::new(RAW_RATE, ch, TUNED_CENTRE, first_pair).unwrap();
            let (found, _) = rx.push(&to_bytes(&iq, geometry), geometry);
            found.first().expect("found").at_us
        };
        let base = at(0, 0);
        assert!(
            (at(250, 0) - base - 250.0).abs() < 0.3,
            "{} vs {base}",
            at(250, 0)
        );
        // Built 7 samples (0.35 us) into the stream: dated 0.35 us later.
        assert!((at(0, 7) - base - 0.35).abs() < 1e-9);
    }

    /// The same signal is found regardless of which of the 79 classic BT
    /// channels it happens to be on, and regardless of whether that channel
    /// is the one the radio is tuned to or one offset from it inside the
    /// capture.
    #[test]
    fn the_channel_and_the_tuning_can_both_vary() {
        const RAW_RATE: f64 = 20_000_000.0;
        for (ch, tuned_centre) in [
            (10u8, 2_441_000_000.0),
            (39u8, 2_441_000_000.0), // this channel IS the tuned centre
            (60u8, 2_450_000_000.0),
        ] {
            let lap = 0x0033_7799;
            let placed = place_on_channel(RAW_RATE, ch, tuned_centre, lap);
            let geometry = eight_bit();
            let bytes = to_bytes(&placed, geometry);
            let mut rx = Receiver::new(RAW_RATE, ch, tuned_centre, 0).unwrap();
            let (found, _headers) = rx.push(&bytes, geometry);
            let found: Vec<u32> = found.iter().map(|h| h.lap).collect();
            assert_eq!(
                found,
                vec![lap],
                "channel {ch} at tuning {tuned_centre}: {found:?}"
            );
        }
    }

    /// Wired end to end: a synthetic classic-BT packet -
    /// access code, a 4-bit trailer, and a real FEC(1/3)-encoded, whitened
    /// header, followed by enough further content to complete the
    /// payload capture window too - produces exactly one `HeaderHit`,
    /// whose own `whitened` bits, dewhitened with the header's real
    /// CLK1-6 and the UAP that HEC implies, give back exactly the fields
    /// it was built from. The payload content here is arbitrary filler,
    /// not a real DH1 payload - `a_real_dh1_payload_lets_break_uap_tie_
    /// resolve_the_floor` (below) is the test that exercises `payload_
    /// raw` for real.
    #[test]
    fn a_full_packet_produces_a_decodable_header_hit() {
        const RAW_RATE: f64 = 20_000_000.0;
        const TUNED_CENTRE: f64 = 2_441_000_000.0;
        let ch = 39u8;
        let lap = 0x0055_aa11u32;
        let clk6 = 17u8;
        let lt_addr = 0b101u8;
        let packet_type = 0b0100u8; // DH1
        let flags = 0b011u8;
        let data10 = (lt_addr as u16) | ((packet_type as u16) << 3) | ((flags as u16) << 7);
        let hec = 0x5au8; // arbitrary; the UAP is whatever this and data10 imply
        let uap = header::uap_from_hec(data10, hec);

        let mut host = [false; header::HEADER_BITS];
        for (i, slot) in host[0..10].iter_mut().enumerate() {
            *slot = (data10 >> i) & 1 != 0;
        }
        for (i, slot) in host[10..18].iter_mut().enumerate() {
            *slot = (hec >> i) & 1 != 0;
        }
        let whitened_header = header::unwhiten_header(&host, clk6);

        let mut bits: Vec<bool> = (0..SETTLE_SYMBOLS).map(|i| i % 2 == 0).collect();
        bits.extend(access_code_bits(lap));
        bits.extend([true, false, true, false]); // 4-bit trailer, content unread
        for &b in &whitened_header {
            bits.extend([b, b, b]); // FEC(1/3): each bit sent three times
        }
        // Arbitrary filler, exactly `PAYLOAD_CAPTURE_BITS` long, so the
        // capture window completes and a `HeaderHit` is actually emitted -
        // this test does not care what the payload says.
        bits.extend((0..PAYLOAD_CAPTURE_BITS).map(|i| i % 2 == 0));
        bits.extend((0..SETTLE_SYMBOLS).map(|i| i % 2 == 1));

        let sps = (RAW_RATE / SYMBOL_RATE_HZ) as usize;
        let clean = modulate(&bits, sps, 160_000.0, RAW_RATE, 0.5);
        let noisy = at_snr(&clean, 40.0, &mut Rng::new(1));
        let mut placed = noisy;
        let channel_hz = channel::centre_hz(ch).unwrap() as f64;
        Nco::new(channel_hz - TUNED_CENTRE, RAW_RATE).mix(&mut placed);

        let geometry = eight_bit();
        let bytes = to_bytes(&placed, geometry);

        let mut rx = Receiver::new(RAW_RATE, ch, TUNED_CENTRE, 0).unwrap();
        let (found, headers) = rx.push(&bytes, geometry);
        let found: Vec<u32> = found.iter().map(|h| h.lap).collect();
        assert_eq!(found, vec![lap], "{found:?}");
        assert_eq!(
            headers.len(),
            1,
            "{:?}",
            headers.iter().map(|h| h.lap).collect::<Vec<_>>()
        );
        let hit = &headers[0];
        assert_eq!(hit.lap, lap);

        let decoded = header::decode_with_uap(&hit.whitened, uap).expect("should decode");
        assert_eq!(decoded.lt_addr, lt_addr);
        assert_eq!(decoded.packet_type, header::PacketType::Dh1);
        assert_eq!(decoded.flags, flags);

        // **The time is on the stream's clock, not the receiver's.** The
        // same samples, handed to a receiver built one second into the
        // stream (20 million pairs at 20 Msps), put the same header exactly
        // one second later. A receiver-local count put it at the same time
        // whenever the receiver happened to be rebuilt, and a piconet's
        // clock counts its slots across receivers.
        let mut later = Receiver::new(RAW_RATE, ch, TUNED_CENTRE, 20_000_000).unwrap();
        let (_, later_headers) = later.push(&bytes, geometry);
        assert_eq!(later_headers.len(), 1);
        assert!(
            (later_headers[0].at_us - hit.at_us - 1e6).abs() < 1e-6,
            "{} us",
            later_headers[0].at_us - hit.at_us
        );
    }

    /// [`PAYLOAD_CAPTURE_BITS`]'s own doc claims it covers every packet
    /// type the `payload` module supports - checked directly rather
    /// than trusted from the arithmetic in the comment alone, the same
    /// discipline every other cited-but-hand-computed constant in this
    /// arc is held to.
    #[test]
    fn payload_capture_covers_every_supported_types_own_worst_case() {
        let mut largest = 0;
        for code in 0..16 {
            let pt = header::PacketType::from_code(code);
            let Some(max_bytes) = payload::max_payload_length(pt) else {
                continue;
            };
            let needed = payload::raw_bits_for(pt, max_bytes * 8);
            largest = largest.max(needed);
            assert!(
                needed <= PAYLOAD_CAPTURE_BITS,
                "{pt:?}: {max_bytes} bytes needs {needed} raw bits, capture only holds {PAYLOAD_CAPTURE_BITS}"
            );
        }
        assert_eq!(
            largest, PAYLOAD_CAPTURE_BITS,
            "no bigger than the largest need"
        );
    }

    /// The reason this wiring exists, proven end to end through the real
    /// GFSK/decimate/slice chain rather than built directly as bits: a
    /// `HeaderHit`'s own `payload_raw`, captured immediately after the
    /// header the same call decoded, is exactly what `payload::
    /// break_uap_tie` needs. Fed two candidate UAPs - the true one and a
    /// genuine decoy this same header's own 64-candidate set actually
    /// produces under some other CLK1-6 - it picks the true one, which is
    /// `header::PiconetClock`'s own measured two-candidate floor actually
    /// broken, not just the primitive that can break it landing untested.
    #[test]
    fn a_real_dh1_payload_lets_break_uap_tie_resolve_the_floor() {
        const RAW_RATE: f64 = 20_000_000.0;
        const TUNED_CENTRE: f64 = 2_441_000_000.0;
        let ch = 39u8;
        let lap = 0x0044_bb22u32;
        let clk6 = 9u8;
        let true_uap = 0x5cu8;

        // A real DH1 header - fields chosen freely, the UAP derived from
        // them by `uap_from_hec` itself, the same discipline `header.rs`'s
        // own tests hold to rather than a second, uncited forward
        // algorithm.
        let lt_addr = 0b011u8;
        let flags = 0b101u8;
        let type_bits = 0b0100u8; // DH1
        let data10 = (lt_addr as u16) | ((type_bits as u16) << 3) | ((flags as u16) << 7);
        let hec = (0u16..256)
            .map(|h| h as u8)
            .find(|&h| header::uap_from_hec(data10, h) == true_uap)
            .expect("uap_from_hec is a bijection in hec for a fixed data10");
        let mut header_host = [false; header::HEADER_BITS];
        for (i, slot) in header_host[0..10].iter_mut().enumerate() {
            *slot = (data10 >> i) & 1 != 0;
        }
        for (i, slot) in header_host[10..18].iter_mut().enumerate() {
            *slot = (hec >> i) & 1 != 0;
        }
        let whitened_header = header::unwhiten_header(&header_host, clk6);

        // A real DH1 payload - one-byte payload header (LLID/FLOW/LENGTH),
        // three bytes of body, a genuine CRC-16 trailer - built in host
        // (dewhitened) form, then whitened the same way the header
        // already is: XOR with the same LFSR, continued from bit 18
        // (`header::unwhiten_at`'s own doc).
        let body = [0x11u8, 0x22, 0x33];
        let mut payload_host = vec![false, true, false]; // LLID, FLOW
        for i in 0..5 {
            payload_host.push((body.len() as u8 >> i) & 1 != 0); // LENGTH
        }
        for &byte in &body {
            for i in 0..8 {
                payload_host.push((byte >> i) & 1 != 0);
            }
        }
        let crc = payload::crcgen(&payload_host, true_uap);
        for i in 0..16 {
            payload_host.push((crc >> i) & 1 != 0);
        }
        let payload_whitened = header::unwhiten_at(&payload_host, clk6, header::HEADER_BITS);

        let mut bits: Vec<bool> = (0..SETTLE_SYMBOLS).map(|i| i % 2 == 0).collect();
        bits.extend(access_code_bits(lap));
        bits.extend([true, false, true, false]); // 4-bit trailer, content unread
        for &b in &whitened_header {
            bits.extend([b, b, b]); // FEC(1/3): each bit sent three times
        }
        bits.extend(payload_whitened.iter().copied());
        // Pad out to the full capture window with arbitrary filler -
        // `PAYLOAD_CAPTURE_BITS`'s own doc explains why a real payload
        // shorter than the window is safe - then a settle tail.
        bits.extend((0..(PAYLOAD_CAPTURE_BITS - payload_whitened.len())).map(|i| i % 2 == 1));
        bits.extend((0..SETTLE_SYMBOLS).map(|i| i % 2 == 0));

        let sps = (RAW_RATE / SYMBOL_RATE_HZ) as usize;
        let clean = modulate(&bits, sps, 160_000.0, RAW_RATE, 0.5);
        let noisy = at_snr(&clean, 40.0, &mut Rng::new(2));
        let mut placed = noisy;
        let channel_hz = channel::centre_hz(ch).unwrap() as f64;
        Nco::new(channel_hz - TUNED_CENTRE, RAW_RATE).mix(&mut placed);

        let geometry = eight_bit();
        let bytes = to_bytes(&placed, geometry);

        let mut rx = Receiver::new(RAW_RATE, ch, TUNED_CENTRE, 0).unwrap();
        let (_found, headers) = rx.push(&bytes, geometry);
        assert_eq!(
            headers.len(),
            1,
            "{:?}",
            headers.iter().map(|h| h.lap).collect::<Vec<_>>()
        );
        let hit = &headers[0];

        let all_candidates = header::candidate_uaps(&hit.whitened);
        let decoy_uap = all_candidates
            .iter()
            .copied()
            .find(|&u| u != true_uap)
            .expect("64 candidates for one header must include more than one distinct value");

        let tries = header::pairs_for(&hit.whitened, &[true_uap, decoy_uap]);
        let winner = payload::break_uap_tie(&tries, &hit.whitened, &hit.payload_raw);
        assert_eq!(winner.map(|(uap, _)| uap), Some(true_uap));
    }

    /// A single real access code, found on more
    /// than one phase lane in the same call, is reported once - not once
    /// per lane. Directly measured to actually happen on this synthetic
    /// signal, not a hypothetical.
    #[test]
    fn a_hit_on_multiple_lanes_at_once_is_reported_once() {
        const RAW_RATE: f64 = 20_000_000.0;
        const TUNED_CENTRE: f64 = 2_441_000_000.0;
        let ch = 39u8;
        let lap = 0x0055_aa11;
        let placed = place_on_channel(RAW_RATE, ch, TUNED_CENTRE, lap);
        let geometry = eight_bit();
        let bytes = to_bytes(&placed, geometry);

        let mut rx = Receiver::new(RAW_RATE, ch, TUNED_CENTRE, 0).unwrap();
        let (found, _headers) = rx.push(&bytes, geometry);
        assert_eq!(found.len(), 1, "{found:?}");
    }

    /// A quiet channel, with only noise on it, does not manufacture a hit -
    /// the same discipline `signal::bt::detect`'s own
    /// `noise_alone_does_not_manufacture_a_hit` holds the bit-domain
    /// detector to, carried through the whole front end.
    #[test]
    fn a_quiet_channel_does_not_manufacture_a_hit() {
        const RAW_RATE: f64 = 20_000_000.0;
        let geometry = eight_bit();
        let mut rng = Rng::new(3);
        let n = 4000;
        let bytes: Vec<u8> = (0..n * 2)
            .map(|_| {
                let (a, _) = rng.normal_pair();
                (a * 20.0).clamp(-127.0, 127.0) as i8 as u8
            })
            .collect();
        let mut rx = Receiver::new(RAW_RATE, 20, 2_441_000_000.0, 0).unwrap();
        let (found, _headers) = rx.push(&bytes, geometry);
        assert!(found.is_empty(), "{found:?}");
    }

    /// A rate under the working rate is refused rather than decoded wrongly.
    #[test]
    fn a_rate_too_low_is_refused() {
        assert!(Receiver::new(1_000_000.0, 10, 2_440_000_000.0, 0).is_err());
    }

    /// A channel that does not exist is refused.
    #[test]
    fn an_unknown_channel_is_refused() {
        assert!(Receiver::new(20_000_000.0, 200, 2_440_000_000.0, 0).is_err());
    }

    /// Retuning, or a channel or rate change, is a different receiver - the
    /// caller rebuilds rather than reusing one whose mixer offset or filter
    /// no longer matches.
    #[test]
    fn matches_is_false_after_any_of_the_three_change() {
        let rx = Receiver::new(20_000_000.0, 10, 2_440_000_000.0, 0).unwrap();
        assert!(rx.matches(10, 20_000_000.0, 2_440_000_000.0));
        assert!(!rx.matches(11, 20_000_000.0, 2_440_000_000.0));
        assert!(!rx.matches(10, 8_000_000.0, 2_440_000_000.0));
        assert!(!rx.matches(10, 20_000_000.0, 2_441_000_000.0));
    }
}
