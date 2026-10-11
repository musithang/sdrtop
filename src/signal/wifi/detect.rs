// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Finding a frame: the short training field's repeat, in the stream.
//!
//! **A repeat, read with a law behind its threshold.** The short training
//! field repeats every 16 samples at 20 Msample/s (0.8 us, IEEE 802.11-2024,
//! 17.3.3), so every 64 as well, and so does the long training field after
//! it (its 32-sample guard is the end of its own symbol). The detector
//! compares two disjoint windows of 64 samples, 64 apart, and normalises by
//! both windows' energies: `|P|^2 / (E_early E_late)`, the squared cosine of
//! the angle between them. For noise alone that is Beta(1, 63) distributed,
//! whatever the gain, so the threshold is the one whose false-alarm rate is
//! stated ([`FALSE_ALARM`]), not a level that worked on one recording. The
//! textbook detector (a 16-sample lag over overlapping windows) is more
//! sensitive in deep noise and has no such law.
//!
//! **What it finds is a repeat, not a frame of this channel.** A neighbour 5,
//! 10 or 15 MHz away repeats with the same period and passes as well; only
//! the long training field's known sequence tells them apart. Everything
//! found here is therefore pending.
//!
//! One preamble gives two runs over the threshold, the short training
//! field's and the long one's about 160 samples later; the second falls in
//! the hold-off after the first and is taken as the same preamble.

use num_complex::Complex;

use crate::signal::dsp::correlate::threshold_for_false_alarm;
use crate::signal::dsp::estimate::schmidl_cox_plateau;

/// The lag and each window, in samples: four of the short training field's
/// periods, so the two windows are disjoint and noise in one says nothing
/// about the other.
const LAG: usize = 64;
const WINDOW: usize = 64;

/// The chance a reading of noise alone crosses the threshold: about one
/// false trigger in five seconds at 20 Msample/s.
pub const FALSE_ALARM: f64 = 1e-8;

/// After a detection, how long a second run is taken as the same preamble's:
/// the long training field follows the short one 160 samples later.
const HOLD_OFF: u64 = 160;

/// The shortest run that is a detection, in readings. A whole short training
/// field holds the two windows full for 33 readings and partly for many more
/// either side; in deep noise its rising edge can flick over the threshold
/// for a few readings first, which must not stand for it.
const MIN_RUN: usize = 16;

/// How many readings in a row under the threshold end a run. Shorter dips,
/// which noise puts into a real run, are bridged.
const BRIDGE: usize = 8;

/// A short training field found, pending what it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Detection {
    /// Where the run over the threshold began: the stream pair the later
    /// window ended on.
    pub start: u64,
    /// Its strongest reading's pair, and that reading, the squared cosine.
    pub peak: u64,
    pub metric: f64,
}

/// Where the reading stands against the threshold.
enum Run {
    /// Below it.
    Below,
    /// Over it: a run that may be a detection. `under` counts the readings
    /// since it last was over, a dip it may still bridge.
    Open {
        start: u64,
        metrics: Vec<f64>,
        under: usize,
    },
    /// Over it, but begun in the hold-off: the same preamble's second run,
    /// swallowed whole however long it lasts.
    Swallowed,
}

/// The short training field detector, carried from block to block.
///
/// **Its sums slide, and are rebuilt every block.** The two windows'
/// correlation and energies move a sample at a time, the new term in and
/// the old one out, the old ones read from rings of the last products and
/// energies rather than computed again; at every block they are summed
/// whole from those rings, so they carry one block's rounding and never
/// more. One pass, a product and an energy a sample, and a division only
/// where a run needs its value: the sample-at-a-time
/// `dsp::correlate::DelayedAutocorrelator` cost more than the stream's own
/// pace on the old laptop sdrtop is tuned on. The tests hold the readings to
/// it.
pub struct StfDetector {
    /// The last `LAG` samples, each the earlier half of a product to come.
    samples: [Complex<f32>; LAG],
    /// The last `WINDOW` pair products.
    products: [Complex<f64>; WINDOW],
    /// The last `LAG + WINDOW` energies: the later window's and the earlier
    /// one's.
    energies: [f64; LAG + WINDOW],
    corr: Complex<f64>,
    late: f64,
    early: f64,
    /// Samples seen since the start or the last cut.
    seen: u64,
    threshold: f64,
    run: Run,
    /// No run starts before this pair.
    quiet_until: u64,
}

impl Default for StfDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl StfDetector {
    pub fn new() -> Self {
        Self {
            samples: [Complex::new(0.0, 0.0); LAG],
            products: [Complex::new(0.0, 0.0); WINDOW],
            energies: [0.0; LAG + WINDOW],
            corr: Complex::new(0.0, 0.0),
            late: 0.0,
            early: 0.0,
            seen: 0,
            threshold: threshold_for_false_alarm(WINDOW, FALSE_ALARM),
            run: Run::Below,
            quiet_until: 0,
        }
    }

    /// The three sums, whole, from the rings: what the sliding has been
    /// approximating, exactly.
    fn rebuild(&mut self) {
        let n = self.seen as usize;
        self.corr = self.products.iter().sum();
        self.late = (0..WINDOW)
            .map(|k| self.energies[(n + LAG + WINDOW - 1 - k) % (LAG + WINDOW)])
            .sum();
        self.early = (WINDOW..LAG + WINDOW)
            .map(|k| self.energies[(n + LAG + WINDOW - 1 - k) % (LAG + WINDOW)])
            .sum();
    }

    /// Slide over a block, handing each sample's position and its reading
    /// as (squared correlation, energy product) once both windows are full:
    /// the squared cosine is their ratio.
    fn scan(
        &mut self,
        first_pair: u64,
        block: &[Complex<f32>],
        mut each: impl FnMut(&mut Self, u64, f64, f64),
    ) {
        self.rebuild();
        for (j, &x) in block.iter().enumerate() {
            let n = self.seen as usize;
            let back = self.samples[n % LAG];
            let now = Complex::new(x.re as f64, x.im as f64);
            let product = Complex::new(back.re as f64, -back.im as f64) * now;
            let energy = now.norm_sqr();
            let e_lag = self.energies[(n + WINDOW) % (LAG + WINDOW)];
            let e_span = self.energies[n % (LAG + WINDOW)];
            self.corr += product - self.products[n % WINDOW];
            self.late += energy - e_lag;
            self.early += e_lag - e_span;
            self.samples[n % LAG] = x;
            self.products[n % WINDOW] = product;
            self.energies[n % (LAG + WINDOW)] = energy;
            self.seen += 1;
            if self.seen >= (LAG + WINDOW) as u64 && self.early > 0.0 && self.late > 0.0 {
                let (num, den) = (self.corr.norm_sqr(), self.early * self.late);
                each(self, first_pair + j as u64, num, den);
            }
        }
    }

    /// Feed a block that starts at stream pair `first_pair`; what was found
    /// in it. A block that does not follow the last one is a break the
    /// caller reports with [`Self::cut`] first.
    pub fn push(&mut self, first_pair: u64, block: &[Complex<f32>]) -> Vec<Detection> {
        let mut found = Vec::new();
        self.scan(first_pair, block, |d, at, num, den| {
            let over = num >= d.threshold * den;
            if !over && matches!(d.run, Run::Below | Run::Swallowed) {
                if matches!(d.run, Run::Swallowed) {
                    d.run = Run::Below;
                }
                return;
            }
            let c = (num / den).min(1.0);
            d.step(at, c, over, &mut found);
        });
        found
    }

    /// One reading against the run: start, grow, bridge or end it.
    fn step(&mut self, at: u64, c: f64, over: bool, found: &mut Vec<Detection>) {
        if over {
            match &mut self.run {
                Run::Open { metrics, under, .. } => {
                    metrics.push(c);
                    *under = 0;
                }
                Run::Swallowed => {}
                Run::Below if at >= self.quiet_until => {
                    self.run = Run::Open {
                        start: at,
                        metrics: vec![c],
                        under: 0,
                    }
                }
                Run::Below => self.run = Run::Swallowed,
            }
            return;
        }
        match &mut self.run {
            Run::Open { metrics, under, .. } if *under + 1 < BRIDGE => {
                metrics.push(c);
                *under += 1;
            }
            _ => match std::mem::replace(&mut self.run, Run::Below) {
                Run::Open {
                    start,
                    mut metrics,
                    under,
                } => {
                    // The dip that ended it is not part of it.
                    metrics.truncate(metrics.len() - under);
                    if metrics.len() >= MIN_RUN {
                        if let Some(p) = schmidl_cox_plateau(&metrics, 0.9) {
                            found.push(Detection {
                                start,
                                peak: start + p.peak as u64,
                                metric: p.peak_metric,
                            });
                        }
                        self.quiet_until = at + HOLD_OFF;
                    }
                }
                Run::Swallowed | Run::Below => {}
            },
        }
    }

    /// Each sample's squared cosine, negative before both windows are full:
    /// the readings `push` decides on, for the tests to hold to the law and
    /// to the running sums.
    #[cfg(test)]
    fn coherences(&mut self, block: &[Complex<f32>]) -> Vec<f64> {
        let first = self.seen;
        let mut out = vec![-1.0; block.len()];
        self.scan(first, block, |_, at, num, den| {
            out[(at - first) as usize] = (num / den).min(1.0);
        });
        out
    }

    /// The stream broke or the radio moved: start again from nothing.
    /// Whether a run was open, which the break cut short.
    pub fn cut(&mut self) -> bool {
        let open = matches!(self.run, Run::Open { .. });
        *self = Self::new();
        open
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::dsp::nco::Nco;
    use crate::signal::dsp::testkit::Rng;
    use crate::signal::wifi::annex::MESSAGE;
    use crate::signal::wifi::ppdu::ppdu;
    use crate::signal::wifi::signal_field::Rate;

    /// The annex's frame, at unit average power over its first 16 us.
    fn frame() -> Vec<Complex<f32>> {
        let f = ppdu(&MESSAGE, Rate::R36, 54);
        let power = f[..320].iter().map(|z| z.norm_sqr()).sum::<f64>() / 320.0;
        f.iter()
            .map(|z| {
                let z = z / power.sqrt();
                Complex::new(z.re as f32, z.im as f32)
            })
            .collect()
    }

    /// The frame `lead` samples into noise `snr_db` below it, shifted by
    /// `shift_hz` at 20 Msample/s, with noise after it too.
    fn on_air(lead: usize, snr_db: f64, shift_hz: f64, rng: &mut Rng) -> Vec<Complex<f32>> {
        let f = frame();
        let mut x = vec![Complex::new(0.0, 0.0); lead + f.len() + 400];
        x[lead..lead + f.len()].copy_from_slice(&f);
        if shift_hz != 0.0 {
            Nco::new(shift_hz, 20e6).mix(&mut x);
        }
        let noise = rng.noise(x.len(), 10f64.powf(-snr_db / 10.0));
        x.iter().zip(noise).map(|(s, n)| s + n).collect()
    }

    /// Detections in a stream fed in blocks of 4096, from pair 0.
    fn detect(x: &[Complex<f32>]) -> Vec<Detection> {
        let mut d = StfDetector::new();
        x.chunks(4096)
            .enumerate()
            .flat_map(|(i, c)| d.push((i * 4096) as u64, c))
            .collect()
    }

    /// One clean frame, one detection, its peak inside the training fields
    /// (the first 16 us, 320 samples, after the frame starts).
    #[test]
    fn one_preamble_is_one_detection() {
        let mut rng = Rng::new(1);
        let x = on_air(1000, 40.0, 0.0, &mut rng);
        let found = detect(&x);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!((1000..1320).contains(&found[0].peak), "{found:?}");
    }

    /// Detection at three SNRs over many frames: every frame at 10 and 6 dB,
    /// nearly every one at 3 dB.
    #[test]
    fn frames_are_found_at_three_snrs() {
        let mut rng = Rng::new(2);
        for (snr, need) in [(10.0, 1.0), (6.0, 1.0), (3.0, 0.95)] {
            let trials = 60;
            let hits = (0..trials)
                .filter(|_| {
                    let found = detect(&on_air(700, snr, 0.0, &mut rng));
                    found.len() == 1 && (700..1020).contains(&found[0].peak)
                })
                .count();
            let rate = hits as f64 / trials as f64;
            eprintln!("{snr} dB: {hits} of {trials} found");
            assert!(rate >= need, "{snr} dB: {hits} of {trials}");
        }
    }

    /// Noise alone crosses a coherence threshold as often as the model says
    /// it should: Beta(1, 63) for two disjoint 64-sample windows. Readings
    /// 128 samples apart share no sample, so they are independent trials,
    /// and only those are counted: a crossing lasts for many readings in a
    /// row, and counting them all would measure the clusters, not the law.
    #[test]
    fn noise_alone_obeys_the_false_alarm_model() {
        let mut rng = Rng::new(3);
        let mut d = StfDetector::new();
        for rate in [0.2, 0.05] {
            let t = crate::signal::dsp::correlate::threshold_for_false_alarm(WINDOW, rate);
            let (mut above, mut trials, mut n) = (0usize, 0usize, 0usize);
            while trials < 8_000 {
                for c in d
                    .coherences(&rng.noise(4096, 1.0))
                    .into_iter()
                    .filter(|&c| c >= 0.0)
                {
                    n += 1;
                    if n % 128 == 0 {
                        trials += 1;
                        above += usize::from(c >= t);
                    }
                }
            }
            let measured = above as f64 / trials as f64;
            eprintln!("false alarms at {rate}: {measured:.4} over {trials} independent readings");
            assert!(
                (measured - rate).abs() < 0.15 * rate,
                "{rate}: {above} of {trials}, {measured}"
            );
        }
    }

    /// At the operating threshold, a second and a half of noise at 20 Msps
    /// sets nothing off... checked here over two million samples.
    #[test]
    fn noise_alone_is_not_a_frame() {
        let mut rng = Rng::new(4);
        let mut d = StfDetector::new();
        let found: usize = (0..500)
            .map(|i| d.push(i * 4096, &rng.noise(4096, 1.0)).len())
            .sum();
        assert_eq!(found, 0);
    }

    /// A frame on the channel next door, 5 or 10 MHz away either side,
    /// passes the short training field too: it is found, and pending.
    #[test]
    fn a_neighbours_frame_is_found_and_pending() {
        let mut rng = Rng::new(5);
        for shift in [-10e6, -5e6, 5e6, 10e6] {
            let found = detect(&on_air(900, 15.0, shift, &mut rng));
            assert!(!found.is_empty(), "{shift}");
        }
    }

    /// **What the detector costs, measured.** Not a check: by hand, in
    /// release, on the machine the question is about (`cargo test --release
    /// what_the_detector_costs -- --ignored --nocapture`): two seconds of a
    /// 20 Msps stream, as a share of real time.
    #[test]
    #[ignore]
    fn what_the_detector_costs() {
        let mut rng = Rng::new(8);
        let block = rng.noise(131_072, 1.0);
        let blocks = (2.0 * 20e6 / 131_072.0) as usize;
        let mut d = StfDetector::new();
        let start = std::time::Instant::now();
        for i in 0..blocks {
            std::hint::black_box(d.push((i * 131_072) as u64, std::hint::black_box(&block)));
        }
        let wall = start.elapsed().as_secs_f64();
        let stream = (blocks * 131_072) as f64 / 20e6;
        eprintln!(
            "{stream:.2} s of 20 Msps in {wall:.3} s: {:.0} % of real time, {:.1} ns a sample",
            wall / stream * 100.0,
            wall / (blocks * 131_072) as f64 * 1e9
        );
    }

    /// **The block sums read what the running sums read.** Over a stream
    /// fed in uneven blocks, every reading equals the sample-at-a-time
    /// `DelayedAutocorrelator`'s, its earlier window's energy taken from
    /// `LAG` readings back, to within the rounding of either.
    #[test]
    fn the_block_sums_read_what_the_running_sums_read() {
        use crate::signal::dsp::correlate::DelayedAutocorrelator;
        let mut rng = Rng::new(9);
        let x = on_air(3000, 8.0, 0.0, &mut rng);
        let mut running = DelayedAutocorrelator::new(WINDOW, LAG);
        let mut late = [0.0f64; LAG];
        let mut n = 0usize;
        let want: Vec<Option<f64>> = x
            .iter()
            .map(|&z| {
                let r = running.push(z)?;
                let early = late[n % LAG];
                late[n % LAG] = r.energy;
                n += 1;
                (n > LAG).then(|| (r.p.norm_sqr() / (early * r.energy)).min(1.0))
            })
            .collect();
        let mut d = StfDetector::new();
        let mut got = Vec::new();
        let mut at = 0;
        for size in [1, 7, 300, 64, 4096].iter().cycle() {
            if at >= x.len() {
                break;
            }
            let end = (at + size).min(x.len());
            got.extend(d.coherences(&x[at..end]));
            at = end;
        }
        let mut compared = 0;
        for (i, (w, g)) in want.iter().zip(&got).enumerate() {
            if let Some(w) = w {
                assert!((w - g).abs() < 1e-9, "{i}: {w} against {g}");
                compared += 1;
            }
        }
        assert!(compared > x.len() - 200, "{compared}");
    }

    /// A break in the middle of a preamble cuts the run: it is reported as
    /// cut once, and nothing is detected from its remains.
    #[test]
    fn a_break_cuts_a_run_once() {
        let mut rng = Rng::new(6);
        let x = on_air(1000, 30.0, 0.0, &mut rng);
        let mut d = StfDetector::new();
        assert!(d.push(0, &x[..1150]).is_empty());
        assert!(d.cut(), "a run was open");
        assert!(!d.cut(), "and only once");
        assert!(d.push(5000, &x[1150..1200]).is_empty());
    }
}
