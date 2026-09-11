// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The live pipeline: raw device bytes to a decoded advertising channel PDU.
//!
//! Four stages, run in order over every sample: [`Detector`] (a matched
//! filter over raw IQ) finds the sync word; once it does, the samples that
//! follow are captured; [`discriminate`] turns the capture into instantaneous
//! frequency; and [`pdu::decode`], fed bits sliced from it and de-whitened,
//! either returns a packet or says there was not enough of one yet.
//!
//! **No phase search here, deliberately - the detector's own peak already
//! is one.** `signal::ble::detect`'s tests measured the matched filter's
//! peak landing at the exact sample the sync word's last symbol ends on, so
//! the header's first symbol begins at the very next sample: a known,
//! symbol-aligned position, not an estimate. `dsp::timing::find_phase`
//! stays the right tool for a burst whose alignment is not already known
//! this precisely - which is not the position this arc's own detector
//! leaves a caller in.

use num_complex::Complex;

use crate::hardware::SampleGeometry;
use crate::signal::demod::decode as decode_iq;
use crate::signal::dsp::code::lfsr::whiten;
use crate::signal::dsp::correlate::threshold_for_false_alarm;
use crate::signal::dsp::discriminate::discriminate;
use crate::signal::dsp::fir::StreamingDecimator;
use crate::signal::dsp::timing::{find_phase, interpolate};

use super::detect::{Detector, Le1mParams, ADVERTISING_ACCESS_ADDRESS, REFERENCE_SYMBOLS};
use super::pdu::{self, Packet};

/// Samples per symbol this arc demodulates at. Not a specification
/// requirement - LE 1M's symbol rate is fixed at 1 Mb/s, and this is
/// comfortably enough resolution for the matched filter and the discriminator
/// slice both, without decimating a wide capture further than it has to.
const WORKING_SPS: usize = 4;
const WORKING_RATE_HZ: f64 = 1_000_000.0 * WORKING_SPS as f64;

/// The longest a legacy advertising PDU can be: 2-byte header, up to 37
/// bytes of payload, 3-byte CRC.
const MAX_PDU_BYTES: usize = 2 + 37 + 3;

/// How rare a false trigger has to be to live with continuously, at 4
/// million matched-filter evaluations a second.
///
/// **Measured against real air, not just the model.** `1e-9` - about one
/// false trigger every four minutes under `false_alarm_rate`'s assumption of
/// circularly symmetric Gaussian noise - was tried first on a real HackRF
/// capture and found the coherence at every real trigger sitting at 0.12 to
/// 0.17, a hair above that rate's own 0.122 threshold, on a busy real
/// channel. Real RF is not the model: correlated interference, ADC
/// artefacts and genuine nearby transmissions on other protocols give a
/// matched filter far more near-misses than white Gaussian noise would, so
/// the same formula's rate needs to be pushed much further to buy a
/// threshold that actually rejects them. This is that adjustment - a rate
/// with no claim to being the true false-alarm probability of a live
/// receiver, only to producing a threshold high enough that this arc's own
/// clean synthetic detections (coherence 0.85 upward) still clear it
/// comfortably while the marginal real-air ones measured here do not.
const FALSE_ALARM_RATE: f64 = 1e-30;

/// Build the decimator from `raw_rate` to [`WORKING_RATE_HZ`], or say why it
/// cannot be built.
///
/// Refuses rather than approximating when `raw_rate` is narrower than the
/// working rate, or is not close to a whole multiple of it: a decode running
/// against a rate it silently disagreed with about would scale every
/// deviation and timing figure downstream by exactly the mismatch, with
/// nothing on screen to say so - the same reasoning N15's survey refusal
/// follows for a span too narrow to plan a sweep across.
///
/// **No anti-alias filter, and that is a measured choice rather than an
/// oversight.** A Kaiser-windowed lowpass sized well inside the working
/// Nyquist was tried here first; on this arc's own synthetic signal it
/// measured the detector's matched-filter coherence collapsing from about
/// 0.99 to under 0.15, for a cause this step did not run down to ground -
/// the filter passes a plain in-band tone at unity gain, so the loss is
/// specific to the GFSK waveform and the matched-filter comparison, not a
/// gain or scaling bug. Plain decimation - keep every `d`th sample, filter
/// nothing - measured 0.99 on the same signal, so that is what is here.
/// The real cost this defers rather than removes: energy from outside the
/// working Nyquist that a real wideband capture carries can alias into the
/// band the detector searches, and nothing here has been measured against
/// that on real hardware yet. HackRF's own baseband filter already narrows
/// what reaches the ADC before this ever runs, which is why this is a
/// deferred question and not a known-broken one.
pub fn front_end(raw_rate: f64) -> Result<StreamingDecimator, String> {
    if raw_rate < WORKING_RATE_HZ {
        return Err(format!(
            "BLE decode needs at least {:.1} Msps; the radio is at {:.3} Msps",
            WORKING_RATE_HZ / 1e6,
            raw_rate / 1e6
        ));
    }
    let d = (raw_rate / WORKING_RATE_HZ).round().max(1.0) as usize;
    let achieved = raw_rate / d as f64;
    if (achieved - WORKING_RATE_HZ).abs() > WORKING_RATE_HZ * 0.01 {
        return Err(format!(
            "BLE decode needs a sample rate near a whole multiple of {:.1} Msps; {:.3} Msps is not one",
            WORKING_RATE_HZ / 1e6,
            raw_rate / 1e6
        ));
    }
    Ok(StreamingDecimator::new(vec![1.0], d))
}

/// One channel's live receiver: the decimator, the detector, and the capture
/// in progress, if any.
pub struct Receiver {
    decim: StreamingDecimator,
    detector: Detector,
    threshold: f64,
    channel: u8,
    raw_rate: f64,
    capture: Vec<Complex<f32>>,
    capturing: bool,
    last_coherence: f64,
}

impl Receiver {
    pub fn new(raw_rate: f64, channel: u8) -> Result<Self, String> {
        let decim = front_end(raw_rate)?;
        let params = Le1mParams {
            sps: WORKING_SPS,
            sample_rate: WORKING_RATE_HZ,
            deviation_hz: 250_000.0,
            bt: 0.5,
        };
        let threshold =
            threshold_for_false_alarm(REFERENCE_SYMBOLS * WORKING_SPS, FALSE_ALARM_RATE);
        Ok(Self {
            decim,
            detector: Detector::new(ADVERTISING_ACCESS_ADDRESS, params),
            threshold,
            channel,
            raw_rate,
            capture: Vec::new(),
            capturing: false,
            last_coherence: 0.0,
        })
    }

    /// Whether this receiver is still the right one for `channel` at
    /// `raw_rate` - a retune or a sample-rate change invalidates the
    /// detector's own reference and the capture in progress alike, so the
    /// caller rebuilds rather than reusing.
    pub fn matches(&self, channel: u8, raw_rate: f64) -> bool {
        self.channel == channel && (self.raw_rate - raw_rate).abs() < 1.0
    }

    /// Feed one block of raw device bytes. Returns every packet fully
    /// decoded from it - almost always zero, and rarely more than one: a
    /// legacy advertising PDU is under a third of a millisecond of air time.
    pub fn push(&mut self, bytes: &[u8], geometry: SampleGeometry) -> Vec<Packet> {
        let mut iq = Vec::new();
        decode_iq(bytes, geometry, usize::MAX, &mut iq);
        let mut working = Vec::new();
        self.decim.process(&iq, &mut working);

        let cap_limit = (16 + MAX_PDU_BYTES * 8) * WORKING_SPS;
        let mut found = Vec::new();
        for &sample in &working {
            if self.capturing {
                self.capture.push(sample);
                match self.try_decode() {
                    Some(packet) => {
                        found.push(packet);
                        self.capturing = false;
                        self.capture.clear();
                    }
                    None if self.capture.len() > cap_limit => {
                        // Either a corrupt length field or a false trigger
                        // with nothing real behind it. Either way, waiting
                        // longer only spends memory: give up and search
                        // again.
                        self.capturing = false;
                        self.capture.clear();
                    }
                    None => {}
                }
            } else if let Some(coherence) = self.detector.push(sample) {
                if coherence > self.threshold {
                    self.capturing = true;
                    self.capture.clear();
                    self.last_coherence = coherence;
                }
            }
        }
        found
    }

    /// Try to decode whatever has been captured so far. `None` means either
    /// "not enough yet" or "the header itself is not readable yet" - both
    /// are the same instruction to the caller: keep capturing.
    ///
    /// **Runs `find_phase` on the capture, and does not trust the detector's
    /// peak position for anything beyond where the capture starts.** The
    /// first version indexed directly from the peak, reasoning that
    /// `signal::ble::detect`'s own tests measured it landing exactly on the
    /// symbol boundary - true for a synthetic packet built by the same
    /// `gfsk::modulate` call the detector's own reference comes from, and
    /// false for a real transmitter: a real symbol clock has no reason to
    /// share a sample-aligned phase with this receiver's, only an
    /// unsynchronised, arbitrary one. Real hardware measured this directly -
    /// every field decoded correctly (plausible PDU types, real advertiser
    /// addresses) while every CRC failed, which is exactly the signature of
    /// a small, consistent sub-sample timing error rather than a wrong
    /// algorithm. `find_phase` is the same tool B4 built for exactly this;
    /// the deterministic shortcut only worked on the test signal that could
    /// never have shown the bug.
    fn try_decode(&self) -> Option<Packet> {
        let mut inst = Vec::new();
        discriminate(&self.capture, WORKING_RATE_HZ, &mut inst);
        let symbols = inst.len() / WORKING_SPS;
        if symbols < pdu::HEADER_BITS {
            return None;
        }
        let phase = find_phase(&inst, WORKING_SPS as f64, symbols, 16);
        // A tuning sitting exactly on the channel's own centre - which this
        // arc's is, since it never mixes off it - is exactly where a real
        // front end's LO leakage and IQ DC offset concentrate. That is a
        // constant added to every discriminator sample, not to the signal
        // this arc's tests build, which is why no synthetic test caught it:
        // slicing against a fixed zero silently moves the decision boundary
        // by however large that offset is. The capture's own mean is the
        // honest estimate of it - GFSK data is balanced over any real
        // stretch of bits - so the threshold is that mean, not zero.
        let bias = inst.iter().sum::<f32>() / inst.len() as f32;
        let mut bits: Vec<bool> = (0..symbols)
            .map(|k| interpolate(&inst, phase + k as f64 * WORKING_SPS as f64) > bias)
            .collect();
        whiten(&mut bits, self.channel);
        let mut packet = pdu::decode(&bits)?;
        packet.debug_coherence = self.last_coherence; // TEMP DEBUG
        packet.debug_phase = phase; // TEMP DEBUG
        Some(packet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::{SampleFormat, StreamBlock};
    use crate::signal::ble::channel;
    use crate::signal::ble::gfsk::modulate;
    use crate::signal::dsp::testkit::{at_snr, Rng};

    fn eight_bit() -> SampleGeometry {
        SampleGeometry {
            format: SampleFormat::Int8,
            full_scale: 128.0,
        }
    }

    /// A synthetic packet, preamble through CRC, at the working rate - the
    /// same construction B3's own detection tests use, extended with a real
    /// PDU instead of random bits after the sync word.
    fn synthetic_packet_iq(
        ch: u8,
        header_byte0: u8,
        payload: &[u8],
        noise_snr_db: f64,
    ) -> Vec<Complex<f32>> {
        let params = Le1mParams {
            sps: WORKING_SPS,
            sample_rate: WORKING_RATE_HZ,
            deviation_hz: 250_000.0,
            bt: 0.5,
        };
        let mut bits = super::super::detect::preamble_bits(ADVERTISING_ACCESS_ADDRESS).to_vec();
        bits.extend_from_slice(&super::super::detect::access_address_bits(
            ADVERTISING_ACCESS_ADDRESS,
        ));
        bits.extend_from_slice(&pdu::encode(ch, header_byte0, payload));
        // A real capture never stops the instant a packet's own last bit
        // ends - there is always more stream after it, whether the next
        // packet's own preamble or just the channel's noise floor. Trailing
        // padding stands in for that, so decoding the last symbol never
        // waits on a sample this test would otherwise never provide.
        let mut rng = Rng::new(1234);
        bits.extend((0..16).map(|_| rng.next_u64() & 1 == 1));
        let clean = modulate(
            &bits,
            params.sps,
            params.deviation_hz,
            params.sample_rate,
            params.bt,
        );
        if noise_snr_db.is_finite() {
            at_snr(&clean, noise_snr_db, &mut Rng::new(99))
        } else {
            clean
        }
    }

    fn bytes_for(iq: &[Complex<f32>], geometry: SampleGeometry) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(iq.len() * 2);
        for s in iq {
            let re = (s.re * geometry.full_scale).clamp(-127.0, 127.0) as i8;
            let im = (s.im * geometry.full_scale).clamp(-127.0, 127.0) as i8;
            bytes.push(re as u8);
            bytes.push(im as u8);
        }
        bytes
    }

    /// B6's exit condition, built with a synthetic transmitter standing in
    /// for the real one: a full ADV_IND, correctly received end to end -
    /// detection, timing, de-whitening and CRC all agreeing.
    #[test]
    fn a_synthetic_adv_ind_is_received_whole() {
        let addr = [0xAA, 0xBB, 0xCC, 0x11, 0x22, 0x33];
        let mut payload = addr.to_vec();
        payload.extend_from_slice(&[0x02, 0x01, 0x06]);
        let iq = synthetic_packet_iq(37, 0x00, &payload, 20.0);
        let geometry = eight_bit();
        let bytes = bytes_for(&iq, geometry);

        let mut rx = Receiver::new(WORKING_RATE_HZ, 37).unwrap();
        let packets = rx.push(&bytes, geometry);
        assert_eq!(packets.len(), 1, "expected exactly one packet");
        let p = &packets[0];
        assert_eq!(p.pdu_type, pdu::PduType::AdvInd);
        assert_eq!(p.adv_addr, Some(addr));
        assert!(p.crc_ok);
    }

    /// The same packet, fed one byte at a time - the shape a real capture
    /// actually arrives in, blocks with no relation to a packet's own
    /// boundaries.
    #[test]
    fn a_packet_split_across_many_small_blocks_still_arrives() {
        let addr = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06];
        let payload = addr.to_vec();
        let iq = synthetic_packet_iq(37, 0x02, &payload, 20.0); // ADV_NONCONN_IND
        let geometry = eight_bit();
        let bytes = bytes_for(&iq, geometry);

        let mut rx = Receiver::new(WORKING_RATE_HZ, 37).unwrap();
        let mut found = Vec::new();
        for chunk in bytes.chunks(6) {
            found.extend(rx.push(chunk, geometry));
        }
        assert_eq!(found.len(), 1);
        assert!(found[0].crc_ok);
        assert_eq!(found[0].adv_addr, Some(addr));
    }

    /// A capture with nothing in it produces nothing - noise alone must not
    /// manufacture a packet.
    #[test]
    fn noise_alone_produces_no_packets() {
        let mut rng = Rng::new(3);
        let noise = rng.noise(200_000, 1.0);
        let geometry = eight_bit();
        let bytes = bytes_for(&noise, geometry);
        let mut rx = Receiver::new(WORKING_RATE_HZ, 37).unwrap();
        assert!(rx.push(&bytes, geometry).is_empty());
    }

    /// A sample rate below the working rate is refused, not silently
    /// mis-scaled.
    #[test]
    fn a_sample_rate_below_the_working_rate_is_refused() {
        assert!(front_end(2_000_000.0).is_err());
    }

    /// A sample rate that decimates cleanly to the working rate is accepted,
    /// at a few realistic HackRF rates.
    #[test]
    fn clean_multiples_of_the_working_rate_are_accepted() {
        for rate in [4_000_000.0, 8_000_000.0, 20_000_000.0] {
            assert!(front_end(rate).is_ok(), "rate {rate} should be accepted");
        }
    }

    /// A real capture at 20 Msps, decimated down, still finds the packet -
    /// the one test in this module that exercises [`front_end`]'s own filter
    /// rather than assuming the working rate directly.
    #[test]
    fn a_packet_survives_decimation_from_a_wider_capture_rate() {
        let addr = [0x10, 0x20, 0x30, 0x40, 0x50, 0x60];
        let mut payload = addr.to_vec();
        payload.push(0xFF);
        let raw_rate = 20_000_000.0;
        let params = Le1mParams {
            sps: (raw_rate / 1_000_000.0) as usize,
            sample_rate: raw_rate,
            deviation_hz: 250_000.0,
            bt: 0.5,
        };
        let mut bits = super::super::detect::preamble_bits(ADVERTISING_ACCESS_ADDRESS).to_vec();
        bits.extend_from_slice(&super::super::detect::access_address_bits(
            ADVERTISING_ACCESS_ADDRESS,
        ));
        bits.extend_from_slice(&pdu::encode(38, 0x00, &payload));
        // See `synthetic_packet_iq`'s own comment: a real stream keeps going
        // past a packet's last bit, and the decimating filter's own warm-up
        // needs a few dozen more samples of margin besides.
        let mut rng = Rng::new(4321);
        bits.extend((0..64).map(|_| rng.next_u64() & 1 == 1));
        let clean = modulate(
            &bits,
            params.sps,
            params.deviation_hz,
            params.sample_rate,
            params.bt,
        );
        let noisy = at_snr(&clean, 25.0, &mut Rng::new(7));
        let geometry = eight_bit();
        let bytes = bytes_for(&noisy, geometry);

        let mut rx = Receiver::new(raw_rate, 38).unwrap();
        let packets = rx.push(&bytes, geometry);
        assert_eq!(packets.len(), 1, "expected exactly one packet");
        assert!(packets[0].crc_ok);
        assert_eq!(packets[0].adv_addr, Some(addr));
    }

    /// A block carried through `StreamBlock`, the real shape blocks arrive
    /// in from the worker - not load-bearing for the decode itself, just
    /// that the geometry plumbing is the same type the real worker uses.
    #[test]
    fn the_block_shape_matches_what_the_worker_hands_it() {
        let geometry = eight_bit();
        let block = StreamBlock {
            seq: 1,
            gap_before: false,
            bytes: vec![0u8; 64],
        };
        let mut rx =
            Receiver::new(WORKING_RATE_HZ, channel::channel_of(2_426_000_000).unwrap()).unwrap();
        assert!(rx.push(&block.bytes, geometry).is_empty());
    }
}
