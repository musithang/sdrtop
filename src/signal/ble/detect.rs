// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Detection: "is a BLE packet here", from the preamble and the fixed
//! advertising access address alone. Detection only - no timing recovery
//! (B4) and no decode (B5 onward).
//!
//! **One matched filter, not two.** Design section 1.1 lists the preamble
//! correlate and the access address correlate as separate items, and for a
//! receiver that does not yet know which access address to expect that is
//! the right split: a cheap preamble-only correlator finds *something*, and
//! a second stage decides what. On the advertising channels the address is
//! not unknown - it is the fixed constant [`ADVERTISING_ACCESS_ADDRESS`] - so
//! there is nothing the two-stage split buys here that concatenating the
//! preamble and the address into one forty-symbol reference does not also
//! give, more simply. A connection-following step that must search for an
//! address it has not yet learned (B14's classic Bluetooth LAP search is the
//! closer case) is where the two-stage form earns its keep, and that is where
//! it should be built, not here on spec.

use num_complex::Complex;

use super::gfsk;
use crate::signal::dsp::correlate::MatchedFilter;

/// The fixed access address every advertising channel PDU begins with.
///
/// Source: the Bluetooth SIG's own public Link Layer Specification page
/// (Core Specification, Vol 6, Part B) states this constant directly for the
/// advertising physical channel. Not independently re-derived, but not from
/// memory alone either - Rule 1's spirit, satisfied by the primary source
/// rather than a secondary account of it, unlike [`access_address_bits`]'s
/// transmission-order rule below, which rests on the same page read through a
/// search summary rather than the page itself.
///
/// No path from `main` yet - see [`Detector`]'s own doc for why this whole
/// file is presently dead together.
#[allow(dead_code)]
pub const ADVERTISING_ACCESS_ADDRESS: u32 = 0x8E89_BED6;

/// How many symbols the combined reference below is: 8 for the preamble, 32
/// for the access address.
#[allow(dead_code)]
pub const REFERENCE_SYMBOLS: usize = 8 + 32;

/// `access_address`'s 32 bits, in the order the specification transmits
/// them: least significant octet first, and least significant bit first
/// within each octet.
///
/// Source: the Bluetooth SIG's public Link Layer Specification page states
/// that multi-octet fields other than the CRC are sent least-significant-
/// octet-first, each octet least-significant-bit-first, and gives
/// `0x8E89BED6` transmitted as the octets D6, BE, 89, 8E in that order as its
/// own worked example - which is exactly what this function computes, and
/// `the_advertising_address_matches_its_own_worked_example` pins it against
/// that example rather than trusting the rule restated in prose.
#[allow(dead_code)]
pub fn access_address_bits(access_address: u32) -> [bool; 32] {
    let mut out = [false; 32];
    for octet in 0..4usize {
        let byte = (access_address >> (8 * octet)) as u8;
        for bit in 0..8usize {
            out[octet * 8 + bit] = (byte >> bit) & 1 != 0;
        }
    }
    out
}

/// The 8-bit preamble that precedes every LE 1M packet, as the bit sequence
/// in transmission order.
///
/// Its first transmitted bit equals the access address's own least
/// significant bit - equivalently, [`access_address_bits`]'s element 0 - and
/// then strictly alternates. Stated this way round deliberately: the
/// specification names the rule by the bit relationship, not by a byte value,
/// and naming it "0xAA" or "0x55" would silently commit to a bit order this
/// function does not need to take a position on.
#[allow(dead_code)]
pub fn preamble_bits(access_address: u32) -> [bool; 8] {
    let mut bit = access_address & 1 != 0;
    let mut out = [false; 8];
    for slot in out.iter_mut() {
        *slot = bit;
        bit = !bit;
    }
    out
}

/// GFSK parameters for the LE 1M PHY, gathered so a caller states the working
/// rate once. Design section 2.1's nominal modulation index `h = 0.5` at a
/// 1 Mb/s symbol rate is 250 kHz of peak deviation (`h = 2 * deviation /
/// symbol_rate`); the Gaussian filter's own bandwidth-time product is the
/// same 0.5.
#[derive(Clone, Copy)]
#[allow(dead_code)]
pub struct Le1mParams {
    pub sps: usize,
    pub sample_rate: f64,
    pub deviation_hz: f64,
    pub bt: f64,
}

#[allow(dead_code)]
impl Le1mParams {
    /// LE 1M at the given samples per symbol. The symbol rate is always
    /// 1 Mb/s on this PHY, so `sample_rate` follows from `sps` rather than
    /// being a second number that could disagree with it.
    pub fn at(sps: usize) -> Self {
        const SYMBOL_RATE: f64 = 1_000_000.0;
        Self {
            sps,
            sample_rate: SYMBOL_RATE * sps as f64,
            deviation_hz: 250_000.0,
            bt: 0.5,
        }
    }
}

/// A detector for one access address on the LE 1M PHY: the preamble and the
/// address, correlated as a single known reference.
///
/// No consumer yet in this arc's own code: nothing in `tasks` or `ui` feeds
/// this a live stream, because there is no capture pipeline wired to `ble`
/// until B6 puts a real packet on screen. Until then this is reached only
/// from this module's own tests, exactly the position `dsp::correlate`'s
/// `MatchedFilter` itself was in before this step gave it a real caller.
#[allow(dead_code)]
pub struct Detector {
    filter: MatchedFilter,
}

#[allow(dead_code)]
impl Detector {
    pub fn new(access_address: u32, params: Le1mParams) -> Self {
        let mut bits = Vec::with_capacity(REFERENCE_SYMBOLS);
        bits.extend_from_slice(&preamble_bits(access_address));
        bits.extend_from_slice(&access_address_bits(access_address));
        let reference = gfsk::modulate(
            &bits,
            params.sps,
            params.deviation_hz,
            params.sample_rate,
            params.bt,
        );
        Self {
            filter: MatchedFilter::new(&reference),
        }
    }

    /// How many samples the reference is - `REFERENCE_SYMBOLS * sps` - which
    /// is the figure [`crate::signal::dsp::correlate::threshold_for_false_alarm`]
    /// needs to turn a stated false-alarm rate into a threshold for this
    /// detector specifically.
    pub fn len(&self) -> usize {
        self.filter.len()
    }

    pub fn is_empty(&self) -> bool {
        self.filter.is_empty()
    }

    /// Feed one sample. `Some` with the coherence in `[0, 1]` once a full
    /// forty-symbol window has been seen; `None` before that and for the
    /// zero-energy window `Match::coherence` refuses to invent a number for.
    pub fn push(&mut self, x: Complex<f32>) -> Option<f64> {
        self.filter.push(x).and_then(|m| m.coherence())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::dsp::correlate::threshold_for_false_alarm;
    use crate::signal::dsp::testkit::{at_snr, Rng};

    /// The worked example the source citation names: `0x8E89BED6` is sent as
    /// the octets D6, BE, 89, 8E, each octet least-significant-bit-first.
    #[test]
    fn the_advertising_address_matches_its_own_worked_example() {
        let bits = access_address_bits(ADVERTISING_ACCESS_ADDRESS);
        let octet = |byte: u8| -> Vec<bool> { (0..8).map(|b| (byte >> b) & 1 != 0).collect() };
        let mut want = Vec::new();
        want.extend(octet(0xD6));
        want.extend(octet(0xBE));
        want.extend(octet(0x89));
        want.extend(octet(0x8E));
        assert_eq!(bits.to_vec(), want);
    }

    /// The preamble alternates starting from the address's own LSB. For the
    /// fixed advertising address that LSB is 0 (0xD6's low bit), so the
    /// preamble here is 0, 1, 0, 1, 0, 1, 0, 1.
    #[test]
    fn the_advertising_preamble_alternates_from_the_address_lsb() {
        let p = preamble_bits(ADVERTISING_ACCESS_ADDRESS);
        assert_eq!(p, [false, true, false, true, false, true, false, true]);
    }

    /// The reference is exactly `REFERENCE_SYMBOLS * sps` samples long, which
    /// is what every threshold and every peak-position check below assumes.
    #[test]
    fn the_reference_is_the_stated_length() {
        let sps = 4;
        let det = Detector::new(ADVERTISING_ACCESS_ADDRESS, Le1mParams::at(sps));
        assert_eq!(det.len(), REFERENCE_SYMBOLS * sps);
        assert!(!det.is_empty());
    }

    /// B3's exit condition: measured detection rate against a generated
    /// packet, at three SNRs, and where in the stream it was found.
    #[test]
    fn the_packet_is_found_at_three_snrs_at_the_right_position() {
        let params = Le1mParams::at(4);
        let threshold = threshold_for_false_alarm(REFERENCE_SYMBOLS * params.sps, 1e-6);
        let prefix_symbols = 500usize;

        for snr_db in [0.0, 10.0, 20.0] {
            let mut rng = Rng::new(1);
            let mut bits: Vec<bool> = (0..prefix_symbols)
                .map(|_| rng.next_u64() & 1 == 1)
                .collect();
            let sync_start = bits.len();
            bits.extend_from_slice(&preamble_bits(ADVERTISING_ACCESS_ADDRESS));
            bits.extend_from_slice(&access_address_bits(ADVERTISING_ACCESS_ADDRESS));
            bits.extend((0..200).map(|_| rng.next_u64() & 1 == 1));

            let clean = gfsk::modulate(
                &bits,
                params.sps,
                params.deviation_hz,
                params.sample_rate,
                params.bt,
            );
            let noisy = at_snr(&clean, snr_db, &mut Rng::new(2));

            let mut det = Detector::new(ADVERTISING_ACCESS_ADDRESS, params);
            let scores: Vec<(usize, f64)> = noisy
                .iter()
                .enumerate()
                .filter_map(|(i, &s)| det.push(s).map(|c| (i, c)))
                .collect();

            let (peak_idx, peak) =
                scores
                    .iter()
                    .cloned()
                    .fold((0, 0.0), |a, b| if b.1 > a.1 { b } else { a });
            assert!(
                peak > threshold,
                "snr={snr_db}: peak coherence {peak} did not clear threshold {threshold}"
            );

            // A reading is for the window ending at the sample just fed, so
            // the peak lands at the end of the sync word: the prefix, plus
            // the forty-symbol reference, minus one.
            let expected = (sync_start + REFERENCE_SYMBOLS) * params.sps - 1;
            assert_eq!(
                peak_idx, expected,
                "snr={snr_db}: peak at {peak_idx}, expected {expected}"
            );
        }
    }

    /// The other half of B3's exit condition: noise alone crosses a
    /// threshold chosen for a stated false-alarm rate about as often as that
    /// rate says it should, the same law N5's `dsp::correlate` tests already
    /// hold a generic reference sequence to.
    #[test]
    fn noise_alone_crosses_the_threshold_at_about_the_stated_rate() {
        let params = Le1mParams::at(4);
        let rate = 1e-3;
        let threshold = threshold_for_false_alarm(REFERENCE_SYMBOLS * params.sps, rate);
        let mut rng = Rng::new(21);
        let noise = rng.noise(400_000, 1.0);
        let mut det = Detector::new(ADVERTISING_ACCESS_ADDRESS, params);
        let crossings = noise
            .iter()
            .filter_map(|&s| det.push(s))
            .filter(|&c| c > threshold)
            .count();
        let got = crossings as f64 / noise.len() as f64;
        assert!(
            got > rate / 4.0 && got < rate * 4.0,
            "measured false-alarm rate {got:e}, expected about {rate:e}"
        );
    }
}
