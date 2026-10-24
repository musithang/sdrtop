// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The channel's samples to pages, with what each transmission measured.
//!
//! **Three receivers, one discriminator.** The rates are 512, 1200 and 2400
//! baud, and nothing on the air says which before the sync word does, so all
//! three listen to the same frequency trace, each with its own bit clock and
//! decision level, and whichever finds a sync word reads the batches after it.
//!
//! **Locking takes a preamble, or a perfect sync word.** Annex 1 §1.1 sends at
//! least 576 bits of alternating reversals first. A sync word found with up
//! to two wrong bits right after a run of them is taken; one found in noise
//! alone, where 32 random bits pass a two-error match about once in eight
//! million tries and three receivers in both polarities make several thousand
//! tries a second, must be exact. That keeps noise from being read as a
//! batch: a batch read from noise passes the BCH check in about a quarter of
//! its codewords and would print addresses nobody sent. A pager joining
//! mid-transmission (§2.3) still locks, on the next exact sync word.
//!
//! **The polarity is found, not assumed.** The sync word is matched as sent
//! and inverted; [`Link::one_is_higher`] says which frequency carried a 1.
//! Nothing here names one of them "normal".
//!
//! **What a transmission measured** ([`Link`]) is rule 3's part of a page:
//! the rate it actually ran at against this radio's clock, its shift and
//! offset from the two decision levels, how far the bits stood out from their
//! own spread, its level, how long its preamble was and how clean its sync.

use num_complex::Complex;

use super::bch::SYNC;
use super::clock::BitClock;
use super::page::{Assembler, Page};
use crate::signal::dsp::discriminate::instantaneous_freq_hz;

/// The three rates POCSAG runs at.
pub const RATES: [u32; 3] = [512, 1200, 2400];

/// A run of reversals this long before a sync word lets the sync word be
/// taken with up to [`SYNC_ERRORS`] wrong bits.
const PREAMBLE_MIN: u32 = 32;

/// Wrong bits a sync word may have when a preamble came first, or inside a
/// transmission already locked.
const SYNC_ERRORS: u32 = 2;

/// What one transmission measured, from its first sync word to the page.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    /// The rate it was received at: 512, 1200 or 2400.
    pub baud: u32,
    /// The rate it ran at, by this radio's clock.
    pub measured_baud: f64,
    /// How far `measured_baud` is from `baud`, in parts per million.
    pub clock_ppm: f64,
    /// Half the distance between the two frequencies.
    pub shift_hz: f64,
    /// Where between them the carrier sat, from the channel's centre.
    pub offset_hz: f64,
    /// The bits' separation over their own spread: `20 log10(shift / sigma)`
    /// of the decided bit values. `None` until both levels have bits.
    pub decision_snr_db: Option<f64>,
    /// The channel's mean power over the transmission, dB full scale.
    pub level_dbfs: f64,
    /// Reversals counted before the first sync word.
    pub preamble_bits: u32,
    /// Wrong bits in the first sync word.
    pub sync_errors: u32,
    pub one_is_higher: bool,
    /// Batches read so far.
    pub batches: u32,
}

/// A page and the transmission it came in.
#[derive(Clone, Debug, PartialEq)]
pub struct Received {
    pub page: Page,
    pub link: Link,
}

/// The receiver for all three rates.
pub struct Decoder {
    rate: f64,
    last: Option<Complex<f32>>,
    receivers: Vec<Rx>,
    freq: Vec<f32>,
    power: Vec<f32>,
}

impl Decoder {
    /// A decoder for a channel at `channel_rate` samples a second.
    pub fn new(channel_rate: f64) -> Self {
        Self {
            rate: channel_rate,
            last: None,
            receivers: RATES.iter().map(|&b| Rx::new(b, channel_rate)).collect(),
            freq: Vec::new(),
            power: Vec::new(),
        }
    }

    /// The rate a receiver is locked at now, if any.
    pub fn locked(&self) -> Option<u32> {
        self.receivers
            .iter()
            .find(|r| r.lock.is_some())
            .map(|r| r.baud)
    }

    /// The channel's next samples. Returns the pages they completed.
    pub fn push(&mut self, iq: &[Complex<f32>]) -> Vec<Received> {
        self.freq.clear();
        self.power.clear();
        for &z in iq {
            if let Some(prev) = self.last {
                self.freq.push(instantaneous_freq_hz(prev, z, self.rate));
                self.power.push(z.norm_sqr());
            }
            self.last = Some(z);
        }
        let mut out = Vec::new();
        for rx in &mut self.receivers {
            for (&f, &p) in self.freq.iter().zip(&self.power) {
                rx.push(f, p, &mut out);
            }
        }
        out
    }

    /// A break in the stream: every transmission in progress ends here, its
    /// open page with it.
    pub fn reset(&mut self) -> Vec<Received> {
        self.last = None;
        let mut out = Vec::new();
        for rx in &mut self.receivers {
            rx.drop_lock(&mut out);
            rx.hunt_reset();
        }
        out
    }
}

/// One rate's receiver.
struct Rx {
    baud: u32,
    clock: BitClock,
    /// The decision level, in Hz: a slow mean while hunting, the midpoint of
    /// the two levels once locked.
    threshold: f64,
    hunt_alpha: f64,
    reg: u32,
    last_bit: bool,
    /// Reversals in a row, ending at the newest bit, and the same count as
    /// it stood 32 bits ago: the run that came before a sync word.
    run: u32,
    runs: std::collections::VecDeque<u32>,
    lock: Option<Lock>,
    assembler: Assembler,
}

struct Lock {
    inverted: bool,
    word: u32,
    bits: u32,
    words: Vec<u32>,
    /// Reading the sync word between batches rather than a codeword.
    expecting_sync: bool,
    stats: Stats,
}

/// What a bit inside a transmission completed.
enum Next {
    Nothing,
    Batch([u32; 16]),
    /// The sync word that should follow a batch did not: the transmission
    /// is over, or lost.
    Lost,
}

/// The transmission's running sums.
#[derive(Default)]
struct Stats {
    preamble: u32,
    sync_errors: u32,
    batches: u32,
    /// The transitions' times against their boundaries: the clock's rate.
    fit: Line,
    // The decided bits' values in Hz, by level: sums for the mean and
    // the spread.
    hi: (f64, f64, u64),
    lo: (f64, f64, u64),
    power_sum: f64,
    power_n: u64,
}

impl Rx {
    fn new(baud: u32, rate: f64) -> Self {
        let sps = rate / f64::from(baud);
        Self {
            baud,
            clock: BitClock::new(sps),
            threshold: 0.0,
            // About eight bits' memory: settles inside the preamble, and a
            // long run of one level does not drag it far.
            hunt_alpha: 1.0 / (8.0 * sps),
            reg: 0,
            last_bit: false,
            run: 0,
            runs: std::collections::VecDeque::with_capacity(33),
            lock: None,
            assembler: Assembler::default(),
        }
    }

    fn hunt_reset(&mut self) {
        self.clock.reset();
        self.reg = 0;
        self.run = 0;
        self.runs.clear();
    }

    fn push(&mut self, f: f32, p: f32, out: &mut Vec<Received>) {
        if self.lock.is_none() {
            self.threshold += self.hunt_alpha * (f64::from(f) - self.threshold);
        } else if let Some(lock) = &mut self.lock {
            lock.stats.power_sum += f64::from(p);
            lock.stats.power_n += 1;
        }
        let pushed = self.clock.push((f64::from(f) - self.threshold) as f32);
        // The transitions the clock found: a point each on the line whose
        // slope is the transmitter's bit length, while locked.
        let locked = self.lock.as_mut();
        match locked {
            Some(lock) => {
                for c in self.clock.take_crossings() {
                    lock.stats.fit.add(c.boundary as f64, c.at);
                }
            }
            None => {
                self.clock.take_crossings();
            }
        }
        let Some(bit) = pushed else {
            return;
        };
        let value = f64::from(bit.centre) + self.threshold;
        let high = bit.mean > 0.0;
        self.run = if high != self.last_bit {
            self.run + 1
        } else {
            1
        };
        self.last_bit = high;
        if self.runs.len() == 33 {
            self.runs.pop_front();
        }
        self.runs.push_back(self.run);
        self.reg = (self.reg << 1) | u32::from(high);

        if self.lock.is_none() {
            self.hunt();
            return;
        }
        let next = self.read(high, value);
        match next {
            Next::Nothing => {}
            Next::Batch(batch) => {
                let pages = self.assembler.push_batch(&batch);
                let link = self.link();
                out.extend(pages.into_iter().map(|page| Received {
                    page,
                    link: link.clone(),
                }));
            }
            Next::Lost => {
                self.drop_lock(out);
                self.hunt_reset();
            }
        }
    }

    /// One decided bit inside a transmission: counted, and added to the
    /// codeword or sync word being read.
    fn read(&mut self, high: bool, value: f64) -> Next {
        let Some(lock) = &mut self.lock else {
            return Next::Nothing;
        };
        let s = &mut lock.stats;
        let level = if high { &mut s.hi } else { &mut s.lo };
        level.0 += value;
        level.1 += value * value;
        level.2 += 1;
        if let (Some(h), Some(l)) = (mean(s.hi), mean(s.lo)) {
            self.threshold = (h + l) / 2.0;
        }
        let bit = high != lock.inverted;
        lock.word = (lock.word << 1) | u32::from(bit);
        lock.bits += 1;
        if lock.bits < 32 {
            return Next::Nothing;
        }
        lock.bits = 0;
        let word = lock.word;
        if lock.expecting_sync {
            if (word ^ SYNC).count_ones() <= SYNC_ERRORS {
                lock.expecting_sync = false;
                return Next::Nothing;
            }
            return Next::Lost;
        }
        lock.words.push(word);
        if lock.words.len() < 16 {
            return Next::Nothing;
        }
        let mut batch = [0u32; 16];
        batch.copy_from_slice(&lock.words);
        lock.words.clear();
        lock.expecting_sync = true;
        lock.stats.batches += 1;
        Next::Batch(batch)
    }

    /// Look for a sync word at the newest bit.
    fn hunt(&mut self) {
        let preamble = if self.runs.len() == 33 {
            self.runs[0]
        } else {
            0
        };
        for (inverted, want) in [(false, SYNC), (true, !SYNC)] {
            let errors = (self.reg ^ want).count_ones();
            let allowed = if preamble >= PREAMBLE_MIN {
                SYNC_ERRORS
            } else {
                0
            };
            if errors <= allowed {
                self.lock = Some(Lock {
                    inverted,
                    word: 0,
                    bits: 0,
                    words: Vec::with_capacity(16),
                    expecting_sync: false,
                    stats: Stats {
                        preamble,
                        sync_errors: errors,
                        ..Stats::default()
                    },
                });
                return;
            }
        }
    }

    /// The transmission ended: its open page ends with it.
    fn drop_lock(&mut self, out: &mut Vec<Received>) {
        if self.lock.is_none() {
            return;
        }
        if let Some(page) = self.assembler.finish() {
            let link = self.link();
            out.push(Received { page, link });
        }
        self.lock = None;
    }

    fn link(&self) -> Link {
        let Some(lock) = &self.lock else {
            return Link {
                baud: self.baud,
                measured_baud: f64::NAN,
                clock_ppm: f64::NAN,
                shift_hz: f64::NAN,
                offset_hz: f64::NAN,
                decision_snr_db: None,
                level_dbfs: f64::NAN,
                preamble_bits: 0,
                sync_errors: 0,
                one_is_higher: true,
                batches: 0,
            };
        };
        let s = &lock.stats;
        let sps = s.fit.slope().unwrap_or(f64::NAN);
        let rate = self.clock.nominal() * f64::from(self.baud);
        let measured = rate / sps;
        let (h, l) = (mean(s.hi), mean(s.lo));
        let (shift, offset) = match (h, l) {
            (Some(h), Some(l)) => ((h - l) / 2.0, (h + l) / 2.0),
            _ => (f64::NAN, f64::NAN),
        };
        let spread = match (spread(s.hi), spread(s.lo)) {
            (Some(a), Some(b)) => Some(((a * a + b * b) / 2.0).sqrt()),
            _ => None,
        };
        let power = if s.power_n > 0 {
            s.power_sum / s.power_n as f64
        } else {
            0.0
        };
        Link {
            baud: self.baud,
            measured_baud: measured,
            clock_ppm: (measured / f64::from(self.baud) - 1.0) * 1e6,
            shift_hz: shift,
            offset_hz: offset,
            decision_snr_db: spread
                .filter(|s| *s > 0.0 && shift.is_finite())
                .map(|s| 20.0 * (shift / s).log10()),
            level_dbfs: 10.0 * power.max(1e-20).log10(),
            preamble_bits: s.preamble,
            sync_errors: s.sync_errors,
            one_is_higher: !lock.inverted,
            batches: s.batches,
        }
    }
}

/// A least-squares straight line through points added one at a time.
#[derive(Default)]
struct Line {
    n: f64,
    sx: f64,
    sy: f64,
    sxx: f64,
    sxy: f64,
}

impl Line {
    fn add(&mut self, x: f64, y: f64) {
        self.n += 1.0;
        self.sx += x;
        self.sy += y;
        self.sxx += x * x;
        self.sxy += x * y;
    }

    /// `None` until there are two distinct points to draw a line through.
    fn slope(&self) -> Option<f64> {
        let d = self.n * self.sxx - self.sx * self.sx;
        (self.n >= 2.0 && d > 0.0).then(|| (self.n * self.sxy - self.sx * self.sy) / d)
    }
}

fn mean((sum, _, n): (f64, f64, u64)) -> Option<f64> {
    (n > 0).then(|| sum / n as f64)
}

fn spread((sum, sq, n): (f64, f64, u64)) -> Option<f64> {
    (n > 1).then(|| {
        let m = sum / n as f64;
        (sq / n as f64 - m * m).max(0.0).sqrt()
    })
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::signal::dsp::testkit::{at_snr, Rng};
    use crate::signal::pocsag::bch::SYNC;
    use crate::signal::pocsag::page::tests::{bits_of, transmit};
    use crate::signal::pocsag::page::Function;
    use std::f64::consts::TAU;

    /// How a test transmitter sends.
    #[derive(Clone, Copy, Debug)]
    pub struct Air {
        pub baud: f64,
        pub shift_hz: f64,
        pub offset_hz: f64,
        pub ppm: f64,
        pub one_is_higher: bool,
        pub preamble: usize,
        pub amplitude: f64,
    }

    impl Air {
        pub fn at(baud: u32) -> Self {
            Self {
                baud: f64::from(baud),
                shift_hz: 4_500.0,
                offset_hz: 0.0,
                ppm: 0.0,
                one_is_higher: false,
                preamble: 576,
                amplitude: 0.5,
            }
        }
    }

    /// The bits a transmission sends: the preamble, then each batch's sync
    /// word and sixteen codewords, most significant bit first.
    pub fn air_bits(batches: &[[u32; 16]], preamble: usize) -> Vec<bool> {
        let mut bits: Vec<bool> = (0..preamble).map(|i| i % 2 == 0).collect();
        for batch in batches {
            for w in std::iter::once(&SYNC).chain(batch.iter()) {
                bits.extend((0..32).rev().map(|b| (w >> b) & 1 == 1));
            }
        }
        bits
    }

    /// Phase-continuous two-level FSK at `rate`, in closed form: each bit a
    /// constant frequency for exactly its duration, the phase carried across.
    /// Silence of `lead` and `tail` samples either side.
    pub fn fsk(bits: &[bool], air: Air, rate: f64, lead: usize, tail: usize) -> Vec<Complex<f32>> {
        let bit_s = 1.0 / (air.baud * (1.0 + air.ppm * 1e-6));
        let total = lead + (bits.len() as f64 * bit_s * rate).ceil() as usize + tail;
        let mut phase = 0.0f64;
        let mut out = Vec::with_capacity(total);
        for i in 0..total {
            if i < lead || i >= total - tail {
                out.push(Complex::new(0.0, 0.0));
                continue;
            }
            let t = (i - lead) as f64 / rate;
            let k = ((t / bit_s) as usize).min(bits.len() - 1);
            let high = bits[k] == air.one_is_higher;
            let f = air.offset_hz + if high { air.shift_hz } else { -air.shift_hz };
            phase = (phase + TAU * f / rate) % TAU;
            out.push(Complex::new(
                (air.amplitude * phase.cos()) as f32,
                (air.amplitude * phase.sin()) as f32,
            ));
        }
        out
    }

    pub fn alpha(text: &str) -> Vec<bool> {
        let mut bits = bits_of(text.as_bytes(), 7);
        bits.extend(bits_of(&[0x04], 7));
        bits
    }

    fn decode(iq: &[Complex<f32>], rate: f64) -> Vec<Received> {
        let mut d = Decoder::new(rate);
        let mut out: Vec<Received> = iq.chunks(4_096).flat_map(|b| d.push(b)).collect();
        out.extend(d.reset());
        out
    }

    const RATE: f64 = 38_400.0;

    fn page_air(air: Air, text: &str) -> Vec<Complex<f32>> {
        let batches = transmit(&[(200_001, Function::D, alpha(text))]);
        let mut iq = fsk(&air_bits(&batches, air.preamble), air, RATE, 2_000, 2_000);
        // A little noise everywhere, so the silence either side is not a
        // perfect zero the discriminator could not read.
        let mut rng = Rng::new(7);
        let n = rng.noise(iq.len(), 1e-6);
        for (s, z) in iq.iter_mut().zip(n) {
            *s += z;
        }
        iq
    }

    /// Every rate, received clean: the page, and the link it came on.
    #[test]
    fn each_rate_reads_its_page_and_says_how() {
        for baud in RATES {
            let air = Air::at(baud);
            let got = decode(&page_air(air, "WX BUDAPEST 14C"), RATE);
            assert_eq!(got.len(), 1, "{baud}: {got:#?}");
            let r = &got[0];
            assert_eq!(r.page.text(), "WX BUDAPEST 14C", "{baud}");
            assert_eq!(r.page.ric, 200_001);
            assert_eq!(r.link.baud, baud);
            assert!(
                (r.link.shift_hz - 4_500.0).abs() < 100.0,
                "{baud}: {}",
                r.link.shift_hz
            );
            assert!(
                r.link.offset_hz.abs() < 50.0,
                "{baud}: {}",
                r.link.offset_hz
            );
            assert!(
                r.link.clock_ppm.abs() < 200.0,
                "{baud}: {} ppm",
                r.link.clock_ppm
            );
            assert!(!r.link.one_is_higher);
            // 576 sent. The noise before it may happen to alternate for a
            // few bits more, and that run is counted too: it was there.
            assert!(
                (570..=584).contains(&r.link.preamble_bits),
                "{baud}: {}",
                r.link.preamble_bits
            );
            assert_eq!(r.page.repairs(), 0);
        }
    }

    /// Either polarity reads, and says which it was.
    #[test]
    fn the_polarity_is_found() {
        for one_is_higher in [false, true] {
            let air = Air {
                one_is_higher,
                ..Air::at(1200)
            };
            let got = decode(&page_air(air, "POLARITY"), RATE);
            assert_eq!(got[0].page.text(), "POLARITY");
            assert_eq!(got[0].link.one_is_higher, one_is_higher);
        }
    }

    /// A transmitter off frequency and off clock is followed, and both are
    /// measured.
    #[test]
    fn offset_and_clock_error_are_measured() {
        for (offset, ppm) in [(2_000.0, 400.0), (-3_000.0, -400.0)] {
            let air = Air {
                offset_hz: offset,
                ppm,
                ..Air::at(1200)
            };
            let got = decode(&page_air(air, "OFFSET AND CLOCK"), RATE);
            let l = &got[0].link;
            assert_eq!(got[0].page.text(), "OFFSET AND CLOCK");
            assert!(
                (l.offset_hz - offset).abs() < 60.0,
                "{} vs {offset}",
                l.offset_hz
            );
            assert!(
                (l.clock_ppm - ppm).abs() < 100.0,
                "{} vs {ppm}",
                l.clock_ppm
            );
        }
    }

    /// Noise alone, a minute of it at every rate, reads no page.
    #[test]
    fn noise_alone_reads_nothing() {
        let mut rng = Rng::new(3);
        let iq = rng.noise((RATE * 60.0) as usize, 0.01);
        assert!(decode(&iq, RATE).is_empty());
    }

    /// A page through noise: still read, and the decision SNR says the
    /// noise was there.
    #[test]
    fn a_noisy_page_is_read_and_the_noise_is_measured() {
        let air = Air::at(1200);
        let clean = page_air(air, "THROUGH THE NOISE");
        let mut rng = Rng::new(11);
        let noisy = at_snr(&clean, 12.0, &mut rng);
        let a = &decode(&clean, RATE)[0];
        let b = &decode(&noisy, RATE)[0];
        assert_eq!(b.page.text(), "THROUGH THE NOISE");
        let (sa, sb) = (
            a.link.decision_snr_db.unwrap(),
            b.link.decision_snr_db.unwrap(),
        );
        assert!(sb < sa - 3.0, "clean {sa:.1} dB, noisy {sb:.1} dB");
    }

    /// Locked while a transmission is read, at its rate, and not after.
    #[test]
    fn it_says_which_rate_it_is_locked_at() {
        let iq = page_air(Air::at(2400), "LOCKED AT 2400 BAUD AND READING ON");
        let mut d = Decoder::new(RATE);
        let mut seen = None;
        for block in iq.chunks(512) {
            d.push(block);
            seen = seen.or(d.locked());
        }
        assert_eq!(seen, Some(2400));
        d.reset();
        assert_eq!(d.locked(), None);
    }

    /// A break in the stream mid-page ends the page there, as cut.
    #[test]
    fn a_break_cuts_the_page() {
        let air = Air::at(1200);
        let iq = page_air(
            air,
            "A MESSAGE LONG ENOUGH TO RUN INTO THE SECOND BATCH OF CODEWORDS",
        );
        let mut d = Decoder::new(RATE);
        let half = iq.len() * 2 / 3;
        let mut got: Vec<Received> = iq[..half].chunks(4_096).flat_map(|b| d.push(b)).collect();
        got.extend(d.reset());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].page.end, crate::signal::pocsag::page::End::Cut);
    }

    /// Through the whole chain from a radio's stream: 2.4 Msps, the channel
    /// 250 kHz off the tuned centre, read in uneven blocks.
    #[test]
    fn from_the_radios_stream() {
        use crate::signal::pocsag::channel::Channel;
        let input = 2.4e6;
        let air = Air {
            offset_hz: 250e3,
            ..Air::at(1200)
        };
        let batches = transmit(&[(8, Function::D, alpha("FROM THE STREAM"))]);
        let iq = fsk(&air_bits(&batches, 576), air, input, 50_000, 50_000);
        let mut c = Channel::new(input, 250e3);
        let mut d = Decoder::new(c.rate());
        let mut ch = Vec::new();
        let mut got = Vec::new();
        for block in iq.chunks(65_531) {
            c.push(block, &mut ch);
            got.extend(d.push(&ch));
        }
        got.extend(d.reset());
        assert_eq!(got.len(), 1, "{got:#?}");
        assert_eq!(got[0].page.text(), "FROM THE STREAM");
        assert!(
            got[0].link.offset_hz.abs() < 60.0,
            "{}",
            got[0].link.offset_hz
        );
    }
}
