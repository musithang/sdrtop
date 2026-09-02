// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! RDS - the signal layer: recovering the bitstream from the 57 kHz subcarrier
//! in the demodulated MPX baseband. The protocol that consumes those bits lives
//! in [`super::rds`].
//!
//! The chain, and why each stage is shaped the way it is:
//!
//! 1. **Pilot PLL.** The RDS subcarrier is locked to the third harmonic of the
//!    19 kHz stereo pilot, and the 1187.5 bps symbol clock is that subcarrier
//!    divided by 48. Tracking the pilot therefore pins carrier *and* symbol
//!    frequency at once, and the pilot is the strongest, purest line in the
//!    baseband - far easier to lock than the suppressed-carrier RDS itself.
//! 2. **Mix and decimate.** Multiplying by three times the pilot phase brings the
//!    subcarrier to DC; a narrow low-pass then rejects the stereo difference
//!    signal, whose upper edge lands only ~4 kHz away once shifted.
//! 3. **Phase resolution.** RDS is suppressed-carrier BPSK, so the recovered
//!    constellation sits on an unknown axis. A squaring estimator finds it. The
//!    residual sign ambiguity needs no resolving at all: the data is
//!    differentially encoded, so inverting every symbol leaves the bits unchanged.
//! 4. **Biphase symbol recovery.** Each bit is Manchester-coded, so integrating
//!    the first half-symbol minus the second recovers it, and the guaranteed
//!    mid-symbol transition gives timing an unambiguous thing to lock to.

use num_complex::Complex;

use super::rds::RdsDecoder;

/// RDS subcarrier: exactly three times the stereo pilot.
pub const SUBCARRIER_HZ: f64 = 57_000.0;
/// Symbol rate: the subcarrier divided by 48.
pub const BITRATE: f64 = SUBCARRIER_HZ / 48.0;
/// Stereo pilot frequency, the reference everything is derived from.
pub const PILOT_HZ: f64 = 19_000.0;

/// Half-bandwidth kept around the subcarrier. RDS occupies ±2.4 kHz.
const RDS_BW_HZ: f64 = 2_400.0;
/// Baseband rate after decimation, targeted at ~17 samples per symbol - enough to
/// place the mid-symbol transition precisely without carrying needless samples.
const BASEBAND_TARGET_HZ: f64 = 20_000.0;
/// Channel-filter length. Long enough that the stereo difference signal, which
/// arrives ~4 kHz from DC after mixing, is well into the stopband.
const LP_TAPS: usize = 511;

/// PLL loop bandwidth. Narrow enough to ignore programme audio, wide enough to
/// follow a receiver's clock error (tens of ppm) without losing the pilot.
const PLL_LOOP_HZ: f64 = 40.0;
/// How hard the early-late gate steers the symbol clock, as a fraction of the
/// gate spacing per symbol. Small: the pilot already fixes the *rate*, so timing
/// only has to walk the *phase* into place once and then hold it.
const TIMING_GAIN: f64 = 0.1;

/// Second-order resonator isolating the pilot before the phase detector, so
/// programme audio cannot pull the loop.
struct Resonator {
    b0: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Resonator {
    /// Band-pass centred at `f0` with bandwidth `bw`, both in Hz.
    fn new(f0: f64, bw: f64, rate: f64) -> Self {
        let w = 2.0 * std::f64::consts::PI * f0 / rate;
        let r = (-std::f64::consts::PI * bw / rate).exp();
        Self {
            b0: 1.0 - r,
            a1: 2.0 * r * w.cos(),
            a2: -r * r,
            z1: 0.0,
            z2: 0.0,
        }
    }
    fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.a1 * self.z1 + self.a2 * self.z2;
        self.z2 = self.z1;
        self.z1 = y;
        y
    }
    fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}

/// Recovers RDS bits from MPX baseband samples.
pub struct RdsDemod {
    rate: f64,
    // Pilot PLL.
    band: Resonator,
    phase: f64,
    freq: f64,
    kp: f64,
    ki: f64,
    // Channel filter and decimation of the mixed-down subcarrier.
    lp: Vec<f32>,
    hist: Vec<Complex<f32>>,
    hist_pos: usize,
    dec_d: usize,
    dec_ctr: usize,
    // Symbol recovery.
    /// Baseband samples per symbol, generally fractional.
    sps: f64,
    /// Baseband samples awaiting symbol evaluation.
    buf: Vec<f32>,
    /// Fractional carry, so a non-integer symbol length does not drift.
    frac: f64,
    /// Squaring-estimator phase, smoothed across symbols.
    bpsk_phase: f32,
    prev_sym: Option<bool>,
}

impl RdsDemod {
    pub fn new(rate: f64) -> Self {
        let dec_d = ((rate / BASEBAND_TARGET_HZ).round().max(1.0)) as usize;
        let baseband = rate / dec_d as f64;
        let cutoff = RDS_BW_HZ / rate;
        // PLL gains for a critically-damped second-order loop.
        let wn = 2.0 * std::f64::consts::PI * PLL_LOOP_HZ / rate;
        Self {
            rate,
            band: Resonator::new(PILOT_HZ, 200.0, rate),
            phase: 0.0,
            freq: 2.0 * std::f64::consts::PI * PILOT_HZ / rate,
            kp: 2.0 * 0.707 * wn,
            ki: wn * wn,
            lp: super::demod::design_lowpass(LP_TAPS, cutoff),
            hist: vec![Complex { re: 0.0, im: 0.0 }; LP_TAPS],
            hist_pos: 0,
            dec_d,
            dec_ctr: 0,
            sps: baseband / BITRATE,
            buf: Vec::new(),
            frac: 0.0,
            bpsk_phase: 0.0,
            prev_sym: None,
        }
    }

    /// Rate this demod was built for - the caller rebuilds it when the channel
    /// rate changes.
    pub fn rate(&self) -> f64 {
        self.rate
    }

    /// Drop all filter and timing state. Used when the sample stream breaks: bits
    /// either side of a gap are not the same message.
    pub fn reset(&mut self) {
        self.band.reset();
        self.hist
            .iter_mut()
            .for_each(|z| *z = Complex { re: 0.0, im: 0.0 });
        self.hist_pos = 0;
        self.dec_ctr = 0;
        self.buf.clear();
        self.frac = 0.0;
        self.prev_sym = None;
    }

    /// Push a contiguous run of MPX samples, feeding any recovered bits to `dec`.
    pub fn process(&mut self, mpx: &[f32], dec: &mut RdsDecoder) {
        for &x in mpx {
            // ── Pilot PLL ──────────────────────────────────────────────────
            let p = self.band.process(x as f64);
            let (s, c) = self.phase.sin_cos();
            // Product detector against a *hard-limited* pilot. The limiter is not
            // optional: MPX samples are scaled in Hz of deviation, so a raw
            // product would hand the loop a phase error some thousands of times
            // too large and it would never settle. Limiting fixes the detector
            // gain at 2/π regardless of how strong the pilot happens to be.
            let err = -p.signum() * s;
            self.freq += self.ki * err;
            self.phase += self.freq + self.kp * err;
            if self.phase > std::f64::consts::TAU {
                self.phase -= std::f64::consts::TAU;
            }

            // ── Mix the subcarrier to DC using three times the pilot phase ──
            // Triple-angle identities rather than a second `sin_cos`: this runs on
            // every sample of the full channel rate, and the detector above has
            // already paid for sin/cos of the fundamental.
            let c3 = c * (4.0 * c * c - 3.0);
            let s3 = s * (3.0 - 4.0 * s * s);
            let mixed = Complex {
                re: (x as f64 * c3) as f32,
                im: (-(x as f64) * s3) as f32,
            };

            // ── Channel filter, evaluated only on decimation instants ──────
            self.hist[self.hist_pos] = mixed;
            self.hist_pos = (self.hist_pos + 1) % LP_TAPS;
            self.dec_ctr += 1;
            if self.dec_ctr < self.dec_d {
                continue;
            }
            self.dec_ctr = 0;

            let mut acc = Complex {
                re: 0.0f32,
                im: 0.0f32,
            };
            for (k, &h) in self.lp.iter().enumerate() {
                let idx = (self.hist_pos + k) % LP_TAPS;
                let z = self.hist[idx];
                acc.re += z.re * h;
                acc.im += z.im * h;
            }
            self.on_baseband(acc, dec);
        }
    }

    /// One decimated baseband sample: integrate it into the current symbol, and
    /// emit a bit when the symbol completes.
    fn on_baseband(&mut self, z: Complex<f32>, dec: &mut RdsDecoder) {
        // Track the BPSK axis with a squaring estimator: for a two-phase signal,
        // z² collects at twice the carrier phase, so half its argument is the axis.
        let sq = z * z;
        if sq.norm() > 1e-12 {
            let est = 0.5 * sq.im.atan2(sq.re);
            // Small, steady correction - the pilot already holds the frequency, so
            // this only has to follow slow phase wander.
            let diff = (est - self.bpsk_phase + std::f32::consts::PI)
                .rem_euclid(2.0 * std::f32::consts::PI)
                - std::f32::consts::PI;
            self.bpsk_phase += 0.01 * diff;
        }
        let (s, c) = self.bpsk_phase.sin_cos();
        let v = z.re * c + z.im * s;
        self.buf.push(v);

        // The window is evaluated at three offsets, so the buffer has to hold a
        // whole symbol plus both gates before any of them can be read.
        let delta = (self.sps / 4.0).max(1.0);
        let need = (self.sps + 2.0 * delta).ceil() as usize + 2;
        if self.buf.len() < need {
            return;
        }

        let early = biphase(&self.buf, 0.0, self.sps);
        let ontime = biphase(&self.buf, delta, self.sps);
        let late = biphase(&self.buf, 2.0 * delta, self.sps);

        let sym = ontime > 0.0;
        // Differential decoding: the bit is the *change* between symbols, which
        // is also why the earlier sign ambiguity never needs resolving.
        if let Some(prev) = self.prev_sym {
            dec.push_bit((sym != prev) as u8);
        }
        self.prev_sym = Some(sym);

        // Early-late gate. Whichever gate sees the wider eye is nearer the true
        // symbol centre, so walk the window that way. Magnitudes, not signed
        // values: the data flips sign symbol to symbol, the alignment does not.
        let err = ((late.abs() - early.abs()) / (ontime.abs() + 1e-9)).clamp(-1.0, 1.0) as f64;
        // Carry the fraction, or a non-integer symbol length would drift.
        let total = self.sps + TIMING_GAIN * err * delta + self.frac;
        let k = (total.floor().max(1.0) as usize).min(self.buf.len());
        self.frac = total - k as f64;
        self.buf.drain(..k);
    }
}

/// Biphase matched filter over one symbol of `buf` starting at `off`: the mean of
/// the first half-symbol minus the mean of the second. Manchester coding puts a
/// guaranteed transition at the midpoint, so this peaks exactly on alignment -
/// which is what makes it serve as both bit slicer and timing discriminator.
fn biphase(buf: &[f32], off: f64, sps: f64) -> f32 {
    let a = off.round() as usize;
    let m = (off + sps / 2.0).round() as usize;
    let b = (off + sps).round() as usize;
    if b > buf.len() || m <= a || b <= m {
        return 0.0;
    }
    let mean = |r: &[f32]| r.iter().sum::<f32>() / r.len() as f32;
    mean(&buf[a..m]) - mean(&buf[m..b])
}

#[cfg(test)]
mod tests {
    use super::super::rds::{encode_block, BlockOffset, RdsDecoder, BLOCK_BITS};
    use super::*;
    use std::f64::consts::TAU;

    /// Build an MPX waveform carrying a pilot and an RDS bitstream, the way a
    /// broadcast transmitter would.
    fn mpx_with_rds(rate: f64, bits: &[u8], pilot_dev: f64, rds_dev: f64) -> Vec<f32> {
        // Differential encoding, then biphase - the transmitter's order.
        let mut sym = false;
        let symbols: Vec<bool> = bits
            .iter()
            .map(|&b| {
                sym ^= b == 1;
                sym
            })
            .collect();

        let sps = rate / BITRATE;
        let n = (symbols.len() as f64 * sps) as usize;
        (0..n)
            .map(|i| {
                let t = i as f64 / rate;
                let si = (i as f64 / sps) as usize;
                let frac = (i as f64 / sps).fract();
                // Manchester: each symbol is a half-high, half-low pulse (or inverted).
                let level = if symbols.get(si).copied().unwrap_or(false) {
                    if frac < 0.5 {
                        1.0
                    } else {
                        -1.0
                    }
                } else if frac < 0.5 {
                    -1.0
                } else {
                    1.0
                };
                let pilot = pilot_dev * (TAU * PILOT_HZ * t).sin();
                // The subcarrier is locked to the pilot's third harmonic.
                let rds = rds_dev * level * (TAU * SUBCARRIER_HZ * t).sin();
                (pilot + rds) as f32
            })
            .collect()
    }

    /// Block B of a group 0A: type 0 and version A are both zero, TP is set, PTY
    /// is 3, and the low bits carry the PS segment index.
    fn block_b(segment: u16) -> u16 {
        (1 << 10) | (3 << 5) | segment
    }

    fn group_bits(info: [u16; 4]) -> Vec<u8> {
        let offs = [
            BlockOffset::A,
            BlockOffset::B,
            BlockOffset::C,
            BlockOffset::D,
        ];
        let mut bits = Vec::new();
        for (w, off) in info.iter().zip(offs.iter()) {
            let block = encode_block(*w, *off);
            for i in (0..BLOCK_BITS).rev() {
                bits.push(((block >> i) & 1) as u8);
            }
        }
        bits
    }

    #[test]
    fn resonator_passes_its_centre_and_rejects_far_tones() {
        let rate = 333_000.0;
        let mut r = Resonator::new(PILOT_HZ, 200.0, rate);
        let mut on = 0.0f64;
        for i in 0..20_000 {
            let v = r.process((TAU * PILOT_HZ * i as f64 / rate).sin());
            if i > 10_000 {
                on += v.abs();
            }
        }
        let mut r2 = Resonator::new(PILOT_HZ, 200.0, rate);
        let mut off = 0.0f64;
        for i in 0..20_000 {
            let v = r2.process((TAU * 5_000.0 * i as f64 / rate).sin());
            if i > 10_000 {
                off += v.abs();
            }
        }
        assert!(
            on > off * 10.0,
            "pilot {on:.3} should dominate 5 kHz audio {off:.3}"
        );
    }

    #[test]
    fn demod_recovers_a_group_from_a_synthetic_mpx() {
        // The end-to-end proof: a real transmitter's construction in, decoded
        // station identity out.
        let rate = 333_000.0;
        let mut bits = Vec::new();
        // Repeat so the PLL has time to settle before the groups that matter.
        for _ in 0..12 {
            bits.extend(group_bits([0xB201, block_b(0), 0, 0x4142]));
        }
        let mpx = mpx_with_rds(rate, &bits, 7_500.0, 2_000.0);

        let mut demod = RdsDemod::new(rate);
        let mut dec = RdsDecoder::new();
        demod.process(&mpx, &mut dec);

        assert_eq!(dec.data().pi, Some(0xB201), "PI not recovered");
        assert!(dec.data().groups_ok > 0, "no groups accepted");
    }

    #[test]
    fn demod_recovers_a_programme_service_name() {
        let rate = 333_000.0;
        let mut bits = Vec::new();
        let name = [b"SD", b"RT", b"OP", b"  "];
        // Several passes so the confirmation threshold is met.
        for _ in 0..4 {
            for (seg, chars) in name.iter().enumerate() {
                let b = block_b(seg as u16);
                let d = u16::from_be_bytes([chars[0], chars[1]]);
                bits.extend(group_bits([0xB201, b, 0, d]));
            }
        }
        let mpx = mpx_with_rds(rate, &bits, 7_500.0, 2_000.0);

        let mut demod = RdsDemod::new(rate);
        let mut dec = RdsDecoder::new();
        demod.process(&mpx, &mut dec);
        assert_eq!(dec.data().ps.as_deref(), Some("SDRTOP"));
    }

    #[test]
    fn inverted_subcarrier_decodes_identically() {
        // Differential encoding means the absolute polarity carries no
        // information - a 180° carrier error must change nothing.
        let rate = 333_000.0;
        let mut bits = Vec::new();
        for _ in 0..12 {
            bits.extend(group_bits([0x1234, block_b(0), 0, 0x4142]));
        }
        let mpx: Vec<f32> = mpx_with_rds(rate, &bits, 7_500.0, 2_000.0)
            .iter()
            .map(|v| -v)
            .collect();

        let mut demod = RdsDemod::new(rate);
        let mut dec = RdsDecoder::new();
        demod.process(&mpx, &mut dec);
        assert_eq!(dec.data().pi, Some(0x1234));
    }

    #[test]
    fn no_pilot_means_no_lock_and_no_invented_data() {
        // Programme audio alone must not produce a station identity.
        let rate = 333_000.0;
        let mpx: Vec<f32> = (0..200_000)
            .map(|i| (10_000.0 * (TAU * 1_000.0 * i as f64 / rate).sin()) as f32)
            .collect();
        let mut demod = RdsDemod::new(rate);
        let mut dec = RdsDecoder::new();
        demod.process(&mpx, &mut dec);
        assert_eq!(dec.data().pi, None, "decoded a station out of pure audio");
        assert_eq!(dec.data().groups_ok, 0);
    }

    #[test]
    fn samples_per_symbol_lands_near_the_target() {
        let d = RdsDemod::new(333_000.0);
        // ~17 samples per symbol: enough to place the mid-symbol transition well.
        assert!(d.sps > 12.0 && d.sps < 24.0, "sps = {}", d.sps);
        assert_eq!(d.rate(), 333_000.0);
    }
}
