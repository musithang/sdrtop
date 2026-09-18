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
//! exactly what a passive receiver does not know (design section 1.4) - so
//! this receiver never waits for a trigger. It slices continuously, from the
//! moment it is built, and lets [`super::detect::Detector`] decide on every
//! new bit whether the last 64 form a real access code.
//!
//! **Multiple channels, one capture - capped, not unbounded.** Design
//! section 1.4: a single ~20 MHz capture already contains on the order of 20
//! of the 79 classic BT channels at once, each only 1 MHz wide.
//! `signal::net::worker` builds one [`Receiver`] per channel it decides to
//! watch, each with its own [`crate::signal::dsp::nco::Nco`] mixing that one
//! channel down to its own baseband - unlike BLE's receiver, which only ever
//! runs when the radio is tuned exactly to the channel it decodes.
//!
//! **Measured, and found too expensive to run unbounded.** A channel-select
//! filter tight enough to matter at 1 MHz spacing needs a real tap count -
//! `front_end`'s own doc has the numbers - and paying that once per watched
//! channel is not the "a few operations per sample" budget design section
//! 12.4 sets for always-on detection. `signal::net::worker` therefore caps
//! how many of the channels in view are actually given a receiver
//! (`[net].bt_channels` in the config, small by default), choosing the ones
//! nearest the tuned centre first, and says on screen how many of the total
//! it is watching - an honest, coarser first step, the same shape B14's own
//! exact-match-only scope-cut already established for this arc.
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
//! **Not yet verified against real hardware.** Every test below is
//! synthetic, the same honest position B14 documented for
//! `signal::bt::access_code` itself: there is no live capture of real
//! classic Bluetooth traffic behind any of these numbers yet, and the
//! channel-select filter's own real adjacent-channel rejection - as opposed
//! to what it measures against a synthetic interferer - is unmeasured air.

use num_complex::Complex;

use crate::hardware::SampleGeometry;
use crate::signal::demod::decode as decode_iq;
use crate::signal::dsp::discriminate::instantaneous_freq_hz;
use crate::signal::dsp::fir::{design_lowpass_to_spec, StreamingDecimator};
use crate::signal::dsp::nco::Nco;

use super::channel;
use super::detect::Detector;
use super::header;

/// Classic BT's own symbol rate: 1 Mb/s, fixed for the basic rate physical
/// layer this arc's GFSK chain targets (design section 1.1's "BR payload is
/// the same GFSK chain as BLE").
const SYMBOL_RATE_HZ: f64 = 1_000_000.0;

/// Convert a count of this receiver's own symbols into CLK1-6 ticks
/// (`header::CLOCK_HZ`, 3200 Hz) - exact integer arithmetic rather than a
/// floating-point ratio that would drift: `SYMBOL_RATE_HZ / header::
/// CLOCK_HZ == 312.5` symbols per tick, so 625 symbols is exactly 2 ticks,
/// the smallest whole-symbol multiple, and every real symbol count this
/// receiver ever produces is measured in exactly that unit.
fn ticks_from_symbols(symbols: u64) -> i64 {
    (symbols * 2 / 625) as i64
}

/// One header captured and FEC-decoded after some lane's own access-code
/// hit - still whitened, not yet attributable to a UAP. `signal::net::
/// worker` owns the per-LAP `header::PiconetClock` that turns a run of
/// these into one, the same way it (not this receiver) owns `signal::net::
/// census`'s own per-address aggregation.
pub struct HeaderHit {
    pub lap: u32,
    pub whitened: [bool; header::HEADER_BITS],
    /// CLK1-6 ticks since this receiver was built - not since any
    /// particular header - `header::PiconetClock::observe` only ever
    /// needs differences between these, and finds its own reference the
    /// first time it is called for a given LAP.
    pub tick: i64,
}

/// One lane's own header capture in progress: the bits collected so far
/// after that lane's `Detector` last fired, and which LAP it was for.
struct PendingHeader {
    lap: u32,
    /// This lane's own symbol count at the moment the access code that
    /// triggered this capture completed - the trailer and header follow
    /// starting at the very next symbol, so this is also the tick
    /// [`HeaderHit::tick`] is measured from.
    start_symbol: u64,
    /// Trailer bits first (discarded once the capture is complete),
    /// header air bits after - `header::TRAILER_BITS` plus `header::
    /// HEADER_AIR_BITS` in total once done.
    bits: Vec<bool>,
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
/// **The stopband is 25 dB, not `signal::ble::receive::front_end`'s 40,
/// and that is a measured trade-off, not an oversight.** A Kaiser filter's
/// own tap count is set by its transition width and its stopband depth
/// together (`dsp::fir::kaiser_taps`), and this filter runs once per
/// *watched channel*, continuously, at [`WORKING_RATE_HZ`] output samples a
/// second each. At 20 Msps this design (121 taps) costs on the order of
/// 484 million complex multiply-adds a second *per channel* - measured
/// directly, not estimated - which for [`super::super::net::worker::
/// SAFE_BT_CHANNELS`] simultaneous channels is still well past design
/// section 12.4's "a few operations per sample" budget for always-on
/// detection; a 40 dB stopband at the same transition would cost close to
/// three times that. `NetWorker` additionally caps *how many* channels get
/// a receiver at all rather than relying on the filter alone to make every
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
    /// How many symbols each lane has ever sliced - this receiver's own
    /// clock, in the only unit `header::PiconetClock` needs: a count that
    /// never resets and never drifts, since it is exact integer arithmetic
    /// the whole way from raw samples down to here.
    lane_symbols: [u64; PHASES],
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
    pub fn new(raw_rate: f64, ch: u8, tuned_centre_hz: f64) -> Result<Self, String> {
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
    /// **Deduplicated within the call, not per lane.** With no active timing
    /// recovery, more than one of the [`PHASES`] lanes routinely lands close
    /// enough to the true phase to decode the same real transmission
    /// cleanly - measured directly: a single synthetic packet was found on
    /// three of the four lanes at once. Reporting each lane's own hit
    /// separately would count one real access code as three, which is
    /// exactly the invented reading rule 2 refuses in the other direction -
    /// a real event, over-counted. The trade-off, said plainly: two
    /// genuinely different packets sharing the same LAP (the ordinary case
    /// within one piconet) landing in the same block would also collapse to
    /// one. A block is a few milliseconds at most and classic BT's own slot
    /// timing is 625 microseconds, so it can happen; B15's own exit
    /// condition is a scatter that makes a piconet's *rhythm* visible, not
    /// an exact packet count, and a coarser count in exchange for not
    /// tripling every real one is the trade worth making here.
    ///
    /// **Header capture rides alongside the same loop.** A lane with a
    /// capture already in progress appends this bit to it before anything
    /// else happens this iteration; a lane whose `Detector` fires this bit
    /// starts a fresh capture, empty, so the access code's own last bit is
    /// never mistaken for the trailer's first one.
    pub fn push(&mut self, bytes: &[u8], geometry: SampleGeometry) -> (Vec<u32>, Vec<HeaderHit>) {
        let mut iq = Vec::new();
        decode_iq(bytes, geometry, usize::MAX, &mut iq);
        self.mixer.mix(&mut iq);
        let mut working = Vec::new();
        self.decim.process(&iq, &mut working);

        let mut found = Vec::new();
        let mut headers = Vec::new();
        const CAPTURE_LEN: usize = header::TRAILER_BITS + header::HEADER_AIR_BITS;
        for &sample in &working {
            let prev = self.last_sample.replace(sample);
            let Some(prev) = prev else { continue };
            let freq = instantaneous_freq_hz(prev, sample, WORKING_RATE_HZ);
            self.bias += (freq - self.bias) * BIAS_ALPHA;
            let bit = freq > self.bias;

            if let Some(pending) = &mut self.pending[self.lane] {
                pending.bits.push(bit);
                if pending.bits.len() == CAPTURE_LEN {
                    let pending = self.pending[self.lane].take().unwrap();
                    if let Some(whitened) = header::unfec13(&pending.bits[header::TRAILER_BITS..]) {
                        // Deduplicated by LAP within the call, the same
                        // reasoning `found`'s own doc gives: more than one
                        // lane routinely captures the same real header
                        // cleanly, and reporting each separately would
                        // over-count one real packet as several.
                        if !headers.iter().any(|h: &HeaderHit| h.lap == pending.lap) {
                            headers.push(HeaderHit {
                                lap: pending.lap,
                                whitened,
                                tick: ticks_from_symbols(pending.start_symbol),
                            });
                        }
                    }
                }
            }

            if let Some(lap) = self.detectors[self.lane].push(bit) {
                if !found.contains(&lap) {
                    found.push(lap);
                }
                if self.pending[self.lane].is_none() {
                    self.pending[self.lane] = Some(PendingHeader {
                        lap,
                        start_symbol: self.lane_symbols[self.lane],
                        bits: Vec::with_capacity(CAPTURE_LEN),
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

    /// Build a clean, GFSK-modulated access code (design section 1.1: "BR
    /// payload is the same GFSK chain" BLE already models), already mixed
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

    /// B15's own exit condition, run end to end with no worker or state
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

        let mut rx = Receiver::new(RAW_RATE, ch, TUNED_CENTRE).unwrap();
        let (found, _headers) = rx.push(&bytes, geometry);
        assert_eq!(found, vec![lap], "{found:?}");
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
            let mut rx = Receiver::new(RAW_RATE, ch, tuned_centre).unwrap();
            let (found, _headers) = rx.push(&bytes, geometry);
            assert_eq!(
                found,
                vec![lap],
                "channel {ch} at tuning {tuned_centre}: {found:?}"
            );
        }
    }

    /// B16's own exit condition, wired: a synthetic classic-BT packet -
    /// access code, a 4-bit trailer, and a real FEC(1/3)-encoded, whitened
    /// header - produces exactly one `HeaderHit`, whose own `whitened`
    /// bits, dewhitened with the header's real CLK1-6 and the UAP that HEC
    /// implies, give back exactly the fields it was built from.
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
        bits.extend((0..SETTLE_SYMBOLS).map(|i| i % 2 == 1));

        let sps = (RAW_RATE / SYMBOL_RATE_HZ) as usize;
        let clean = modulate(&bits, sps, 160_000.0, RAW_RATE, 0.5);
        let noisy = at_snr(&clean, 40.0, &mut Rng::new(1));
        let mut placed = noisy;
        let channel_hz = channel::centre_hz(ch).unwrap() as f64;
        Nco::new(channel_hz - TUNED_CENTRE, RAW_RATE).mix(&mut placed);

        let geometry = eight_bit();
        let bytes = to_bytes(&placed, geometry);

        let mut rx = Receiver::new(RAW_RATE, ch, TUNED_CENTRE).unwrap();
        let (found, headers) = rx.push(&bytes, geometry);
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
    }

    /// `push`'s own exit condition: a single real access code, found on more
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

        let mut rx = Receiver::new(RAW_RATE, ch, TUNED_CENTRE).unwrap();
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
        let mut rx = Receiver::new(RAW_RATE, 20, 2_441_000_000.0).unwrap();
        let (found, _headers) = rx.push(&bytes, geometry);
        assert!(found.is_empty(), "{found:?}");
    }

    /// A rate under the working rate is refused rather than decoded wrongly.
    #[test]
    fn a_rate_too_low_is_refused() {
        assert!(Receiver::new(1_000_000.0, 10, 2_440_000_000.0).is_err());
    }

    /// A channel that does not exist is refused.
    #[test]
    fn an_unknown_channel_is_refused() {
        assert!(Receiver::new(20_000_000.0, 200, 2_440_000_000.0).is_err());
    }

    /// Retuning, or a channel or rate change, is a different receiver - the
    /// caller rebuilds rather than reusing one whose mixer offset or filter
    /// no longer matches.
    #[test]
    fn matches_is_false_after_any_of_the_three_change() {
        let rx = Receiver::new(20_000_000.0, 10, 2_440_000_000.0).unwrap();
        assert!(rx.matches(10, 20_000_000.0, 2_440_000_000.0));
        assert!(!rx.matches(11, 20_000_000.0, 2_440_000_000.0));
        assert!(!rx.matches(10, 8_000_000.0, 2_440_000_000.0));
        assert!(!rx.matches(10, 20_000_000.0, 2_441_000_000.0));
    }
}
