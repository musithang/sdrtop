// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! One carrier's samples to dated rows, each with its strongest line.
//!
//! **The chain.** A mixer puts the carrier at 0 Hz; an average divides the
//! rate by most of the factor and a filter by the last [`LAST_STAGE`], its
//! passband the span and its stopband starting where an alias would fold
//! back into the span (`narrow rate - span`), at [`STOPBAND_DB`]; a ring
//! holds the last window of narrow samples. At every hop boundary the ring
//! is Hann-windowed and transformed, zero-padded [`PAD`] times so the line
//! can be found between bins; the row is the transform's power over the
//! span, one value per resolution bin, in dB over the row's median.
//!
//! **The line, and its uncertainty.** The strongest padded bin is a first
//! guess; the line is the maximum of the windowed transform's magnitude near
//! it, found by golden-section search of the transform evaluated directly,
//! because interpolating three padded bins biases a Hann peak by more than
//! the uncertainty at good SNR. Its sigma is the Cramér–Rao bound for a tone
//! in noise at the SNR the row measures over the window; [`HANN_VARIANCE`]
//! says why that, and not more, is the honest figure.
//!
//! **Refused, never guessed.** A peak under a detection threshold set by a
//! false-alarm rate is "no line"; a peak too wide at -6 dB, or with a second
//! maximum within two resolution bins, is "a blend": two lines the window
//! cannot tell apart, whose blended position belongs to neither.

use std::collections::VecDeque;
use std::f64::consts::{LN_2, TAU};
use std::sync::Arc;

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use super::settings::{derive, placement, CarrierSetting, Derived, LAST_STAGE};
use crate::signal::dsp::fir::{design_lowpass_to_spec, StreamingDecimator};
use crate::signal::dsp::uncertainty::crlb_frequency;

/// How many times the transform is zero-padded: four points per resolution
/// bin, enough to find the peak's bin and to walk its -6 dB width. Drawing
/// only; the resolution claimed stays one over the window.
pub const PAD: usize = 4;

/// The rate at which noise alone may be called a line, per row. One row in a
/// thousand: a row every five seconds makes that one false line in about an
/// hour and a half, which a spectrogram shows as the lone dot it is.
pub const FALSE_ALARM: f64 = 1e-3;

/// The variance the line's estimate is reported with, over the Cramér–Rao
/// bound at the SNR the row measures. **Measured end to end, and one.** Five
/// minutes of a steady tone through the whole chain (mixer, average, filter,
/// Hann window, the search), at 10, 20 and 30 dB: each row's error over its
/// sigma spreads by 0.96 to 0.97 with this at one. A bare Hann estimator
/// handed the true SNR scatters about 2.3 times the bound; the chain's own
/// SNR, measured off the Hann peak, already carries that cost, and inflating
/// it again made the sigma half again too wide. `a_reported_sigma_is_honest`
/// holds it.
pub const HANN_VARIANCE: f64 = 1.0;

/// The widest a single line may be at -6 dB, in resolution bins. Hann's own
/// main lobe is 2.0 there; two lines a bin and a half apart read about 3.5,
/// and a line that moves a resolution bin inside its window about 2.2.
pub const BLEND_WIDTH: f64 = 2.6;

/// How far below the line a second maximum still makes it a blend: 25 dB.
pub const BLEND_DB: f64 = 25.0;

/// The anti-alias filter's stopband: 60 dB puts a neighbour as loud as a
/// broadcaster's carrier is above the floor here back under it.
pub const STOPBAND_DB: f64 = 60.0;

/// The strongest line in a row's span: its offset from the carrier's nominal
/// frequency as received through our oscillator, Hz, and the standard
/// uncertainty of that offset (the Cramér–Rao bound at the row's own SNR,
/// which the chain meets, measured). It is the strongest line in the span, which is the carrier only when
/// nothing louder is inside it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    pub hz: f64,
    pub sigma_hz: f64,
}

/// One window of one carrier.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// The window's number: its end pair over the hop. Carriers of one
    /// resolution share it, which is what pairs them.
    pub seq: u64,
    /// The input pair the window ends at.
    pub end_pair: u64,
    pub window_s: f64,
    /// The width of one value of `bins_db`: the resolution.
    pub bin_hz: f64,
    /// The offset from the carrier the first value starts at.
    pub start_hz: f64,
    /// The power in each resolution bin, dB over the row's median.
    pub bins_db: Vec<f32>,
    /// The strongest line, or why there is none to give.
    pub line: Result<Line, String>,
    /// The strongest padded bin over the noise per bin, dB.
    pub snr_db: Option<f64>,
    /// Whether the feed lost samples inside this window.
    pub feed_loss: bool,
}

/// One carrier, followed through a stream at one tuning and rate.
pub struct Tracker {
    setting: CarrierSetting,
    d: Derived,
    phase: f64,
    step: f64,
    average: usize,
    sum: Complex<f64>,
    summed: usize,
    filter: StreamingDecimator,
    stage_in: Vec<Complex<f32>>,
    stage_out: Vec<Complex<f32>>,
    ring: VecDeque<(Complex<f32>, bool)>,
    lost_pending: bool,
    next_pair: Option<u64>,
    epoch_pair: u64,
    produced: u64,
    next_boundary: u64,
    hann: Vec<f32>,
    fft: Arc<dyn Fft<f32>>,
}

impl Tracker {
    /// A tracker for `s` in a stream at `input_rate_hz` centred on
    /// `centre_hz`, or why that stream cannot hold it.
    pub fn new(s: CarrierSetting, input_rate_hz: f64, centre_hz: u64) -> Result<Tracker, String> {
        let d = derive(&s, input_rate_hz)?;
        let offset = placement(&s, centre_hz, input_rate_hz)?;
        let (average, last) = if d.factor >= 2 * LAST_STAGE {
            (d.factor / LAST_STAGE, LAST_STAGE)
        } else {
            (1, d.factor)
        };
        let r1 = input_rate_hz / average as f64;
        // Passband to the span; stopband from where an alias would fold back
        // into it.
        let fc = d.narrow_rate_hz / 2.0 / r1;
        let transition = (d.narrow_rate_hz - 2.0 * s.span_hz) / r1;
        let taps = design_lowpass_to_spec(fc, transition, STOPBAND_DB);
        let n = d.window;
        let hann = (0..n)
            .map(|i| (0.5 - 0.5 * (TAU * i as f64 / n as f64).cos()) as f32)
            .collect();
        Ok(Tracker {
            setting: s,
            d,
            phase: 0.0,
            step: -TAU * offset / input_rate_hz,
            average,
            sum: Complex::new(0.0, 0.0),
            summed: 0,
            filter: StreamingDecimator::new(taps, last),
            stage_in: Vec::new(),
            stage_out: Vec::new(),
            ring: VecDeque::with_capacity(n + 1),
            lost_pending: false,
            next_pair: None,
            epoch_pair: 0,
            produced: 0,
            next_boundary: 0,
            hann,
            fft: FftPlanner::<f32>::new().plan_fft_forward(n * PAD),
        })
    }

    /// The setting followed.
    pub fn setting(&self) -> &CarrierSetting {
        &self.setting
    }

    /// What the setting came to at this stream's rate.
    pub fn derived(&self) -> &Derived {
        &self.d
    }

    /// Take one block starting at stream pair `first_pair`; `gap_before`
    /// when the feed lost samples just before it. Returns the rows of every
    /// window that ended inside it.
    pub fn push(&mut self, block: &[Complex<f32>], first_pair: u64, gap_before: bool) -> Vec<Row> {
        match self.next_pair {
            None => {
                self.epoch_pair = first_pair;
                self.next_boundary = (first_pair / self.d.hop_pairs + 1) * self.d.hop_pairs;
            }
            Some(expected) if first_pair > expected || gap_before => {
                self.bridge(first_pair.saturating_sub(expected));
            }
            _ => {}
        }
        self.next_pair = Some(first_pair + block.len() as u64);

        // Mix and average.
        self.stage_in.clear();
        for x in block {
            let (s, c) = self.phase.sin_cos();
            let y = Complex::new(x.re as f64, x.im as f64) * Complex::new(c, s);
            self.phase += self.step;
            if self.average == 1 {
                self.stage_in.push(Complex::new(y.re as f32, y.im as f32));
                continue;
            }
            self.sum += y;
            self.summed += 1;
            if self.summed == self.average {
                let m = self.sum / self.average as f64;
                self.stage_in.push(Complex::new(m.re as f32, m.im as f32));
                self.sum = Complex::new(0.0, 0.0);
                self.summed = 0;
            }
        }
        self.phase = self.phase.rem_euclid(TAU);
        let mut out = std::mem::take(&mut self.stage_out);
        self.filter.process(&self.stage_in, &mut out);
        let mut rows = Vec::new();
        for &z in &out {
            if let Some(row) = self.take(z, false) {
                rows.push(row);
            }
        }
        self.stage_out = out;
        rows
    }

    /// Samples went missing: the filters forget what they held, the mixer
    /// steps over the missing time, and the narrow samples that would have
    /// come in it are zeros marked lost, so time in the ring stays true.
    fn bridge(&mut self, missing_pairs: u64) {
        self.filter.reset();
        self.sum = Complex::new(0.0, 0.0);
        self.summed = 0;
        self.phase = (self.phase + self.step * missing_pairs as f64).rem_euclid(TAU);
        let zeros = (missing_pairs / self.d.factor as u64).min(self.d.window as u64 + 1);
        for _ in 0..zeros {
            self.take(Complex::new(0.0, 0.0), true);
        }
        self.lost_pending = true;
    }

    /// One narrow sample into the ring; a row when it closes a window.
    fn take(&mut self, z: Complex<f32>, lost: bool) -> Option<Row> {
        let lost = lost || std::mem::take(&mut self.lost_pending);
        self.ring.push_back((z, lost));
        if self.ring.len() > self.d.window {
            self.ring.pop_front();
        }
        self.produced += 1;
        let at = self.epoch_pair + self.produced * self.d.factor as u64;
        if at < self.next_boundary {
            return None;
        }
        let boundary = self.next_boundary;
        while self.next_boundary <= at {
            self.next_boundary += self.d.hop_pairs;
        }
        (self.ring.len() == self.d.window).then(|| self.row(boundary))
    }

    fn row(&self, end_pair: u64) -> Row {
        let n = self.d.window;
        let m = n * PAD;
        let rate = self.d.narrow_rate_hz;
        let (span, res) = (self.setting.span_hz, self.setting.resolution_hz);
        let samples: Vec<Complex<f32>> = self
            .ring
            .iter()
            .zip(&self.hann)
            .map(|(s, w)| s.0 * *w)
            .collect();
        let mut x = samples.clone();
        x.resize(m, Complex::new(0.0, 0.0));
        self.fft.process(&mut x);
        let hz = |k: usize| {
            (if k < m / 2 {
                k as f64
            } else {
                k as f64 - m as f64
            }) * rate
                / m as f64
        };
        // The padded bins inside the span, in frequency order.
        let mut span_bins: Vec<(f64, f64)> = (0..m)
            .map(|k| (hz(k), x[k].norm_sqr() as f64))
            .filter(|(f, _)| f.abs() < span)
            .collect();
        span_bins.sort_by(|a, b| a.0.total_cmp(&b.0));
        let median = |mut v: Vec<f64>| {
            v.sort_by(f64::total_cmp);
            v[v.len() / 2].max(f64::MIN_POSITIVE)
        };
        let noise = median(span_bins.iter().map(|b| b.1).collect()) / LN_2;
        // One value per resolution bin.
        let bins = self.d.bins;
        let mut per = vec![(0.0, 0usize); bins];
        for &(f, p) in &span_bins {
            let i = ((f + span) / res).floor() as usize;
            if let Some(slot) = per.get_mut(i) {
                slot.0 += p;
                slot.1 += 1;
            }
        }
        let rb: Vec<f64> = per
            .iter()
            .map(|(s, c)| if *c > 0 { s / *c as f64 } else { 0.0 })
            .collect();
        let floor = median(rb.clone());
        let bins_db = rb
            .iter()
            .map(|v| (10.0 * (v / floor).max(1e-10).log10()) as f32)
            .collect();
        let feed_loss = self.ring.iter().any(|s| s.1);

        let (peak_i, &(peak_f, peak_p)) = span_bins
            .iter()
            .enumerate()
            .max_by(|a, b| a.1 .1.total_cmp(&b.1 .1))
            .expect("a span holds bins");
        let threshold = noise * (span_bins.len() as f64 / FALSE_ALARM).ln();
        let db = |p: f64| 10.0 * (p / noise).log10();
        let snr_db = Some(db(peak_p));
        let line = if peak_p < threshold {
            Err(format!(
                "no line above the noise: the strongest stands {:.1} dB over it, under the {:.1} dB a line needs",
                db(peak_p),
                db(threshold)
            ))
        } else if let Some(why) = blend(&span_bins, peak_i, threshold, res) {
            Err(why)
        } else {
            let f = refine(&samples, rate, peak_f, rate / m as f64);
            let snr = 1.5 * (peak_p - noise).max(f64::MIN_POSITIVE) / (n as f64 * noise);
            let sigma = (HANN_VARIANCE * crlb_frequency(snr, n)).sqrt() * rate;
            Ok(Line {
                hz: f,
                sigma_hz: sigma,
            })
        };
        Row {
            seq: end_pair / self.d.hop_pairs,
            end_pair,
            window_s: 1.0 / res,
            bin_hz: res,
            start_hz: -span,
            bins_db,
            line,
            snr_db,
            feed_loss,
        }
    }
}

/// Why the peak at `i` is a blend of lines rather than one, if it is.
fn blend(bins: &[(f64, f64)], i: usize, threshold: f64, res: f64) -> Option<String> {
    let (f0, p0) = bins[i];
    let half = p0 / 4.0;
    let edge = |dir: isize| -> f64 {
        let mut j = i as isize;
        loop {
            let k = j + dir;
            if k < 0 || k as usize >= bins.len() {
                return bins[j as usize].0;
            }
            let (fk, pk) = bins[k as usize];
            if pk < half {
                let (fj, pj) = bins[j as usize];
                return fj + (fk - fj) * (pj - half) / (pj - pk);
            }
            j = k;
        }
    };
    let width = (edge(1) - edge(-1)) / res;
    if width > BLEND_WIDTH {
        return Some(format!(
            "a blend: the line is {width:.1} resolution bins wide at -6 dB, where one line is 2.0; two lines closer than two bins"
        ));
    }
    let floor = (p0 / 10f64.powf(BLEND_DB / 10.0)).max(threshold);
    for k in 1..bins.len().saturating_sub(1) {
        let (f, p) = bins[k];
        let d = (f - f0).abs();
        if k != i
            && d >= res
            && d <= 2.0 * res
            && p >= floor
            && p > bins[k - 1].1
            && p >= bins[k + 1].1
        {
            return Some(format!(
                "a blend: a second line {:.2} Hz away, within two resolution bins",
                f - f0
            ));
        }
    }
    None
}

/// The maximum of the windowed transform's magnitude within `±step` of
/// `guess`, by golden-section search over the transform evaluated directly.
fn refine(x: &[Complex<f32>], rate: f64, guess: f64, step: f64) -> f64 {
    let mag = |f: f64| {
        let w = -TAU * f / rate;
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (n, s) in x.iter().enumerate() {
            let (sn, cs) = (w * n as f64).sin_cos();
            re += s.re as f64 * cs - s.im as f64 * sn;
            im += s.re as f64 * sn + s.im as f64 * cs;
        }
        re * re + im * im
    };
    let g = (5f64.sqrt() - 1.0) / 2.0;
    let (mut a, mut b) = (guess - step, guess + step);
    let (mut c, mut d) = (b - g * (b - a), a + g * (b - a));
    let (mut fc, mut fd) = (mag(c), mag(d));
    for _ in 0..40 {
        if fc > fd {
            b = d;
            d = c;
            fd = fc;
            c = b - g * (b - a);
            fc = mag(c);
        } else {
            a = c;
            c = d;
            fc = fd;
            d = a + g * (b - a);
            fd = mag(d);
        }
    }
    (a + b) / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::doppler::testing::{add, carrier, keyed, noise};
    use crate::signal::doppler::{derive, CarrierSetting};
    use rustfft::num_complex::Complex;

    const CENTRE: u64 = 10_000_000;
    const OFFSET: f64 = 20_000.0;
    const SIGMA_IN: f32 = 0.1;

    fn ionosphere() -> CarrierSetting {
        CarrierSetting {
            hz: CENTRE + OFFSET as u64,
            span_hz: 25.0,
            resolution_hz: 0.1,
        }
    }

    /// The amplitude that gives `snr_db` per narrow sample against
    /// `SIGMA_IN` noise at the input.
    fn amp_for(s: &CarrierSetting, rate: f64, snr_db: f64) -> f32 {
        let f = derive(s, rate).unwrap().factor as f64;
        ((10f64.powf(snr_db / 10.0)).sqrt() * SIGMA_IN as f64 / f.sqrt()) as f32
    }

    /// Push `seconds` of `make(start_pair, n)` through a tracker in radio-sized
    /// blocks; `gap` drops the block holding that pair and flags the next.
    fn run(
        s: CarrierSetting,
        rate: f64,
        seconds: f64,
        mut make: impl FnMut(u64, usize) -> Vec<Complex<f32>>,
        gap: Option<u64>,
    ) -> Vec<Row> {
        let mut t = Tracker::new(s, rate, CENTRE).unwrap();
        let total = (seconds * rate) as u64;
        let block = 32_768usize;
        let (mut at, mut rows, mut lost) = (0u64, Vec::new(), false);
        while at < total {
            let n = block.min((total - at) as usize);
            let samples = make(at, n);
            if gap.is_some_and(|g| (at..at + n as u64).contains(&g)) {
                lost = true;
            } else {
                rows.extend(t.push(&samples, at, std::mem::take(&mut lost)));
            }
            at += n as u64;
        }
        rows
    }

    /// A tone at `doppler(t)` from the carrier, at `snr_db`, in noise.
    fn tone(
        s: CarrierSetting,
        rate: f64,
        snr_db: f64,
        doppler: impl Fn(f64) -> f64 + 'static,
    ) -> impl FnMut(u64, usize) -> Vec<Complex<f32>> {
        let amp = amp_for(&s, rate, snr_db);
        let (mut phase, mut seed) = (0.3, 0x5eed_u64);
        move |start, n| {
            let f = |t: f64| OFFSET + doppler(t);
            add(
                carrier(rate, &f, amp, start, n, &mut phase),
                &noise(n, SIGMA_IN, &mut seed),
            )
        }
    }

    fn line(r: &Row) -> &Line {
        r.line
            .as_ref()
            .unwrap_or_else(|why| panic!("row {}: {why}", r.seq))
    }

    #[test]
    fn a_steady_tone_is_found_within_its_sigma() {
        let rate = 768_000.0;
        let rows = run(
            ionosphere(),
            rate,
            25.0,
            tone(ionosphere(), rate, 20.0, |_| 0.37),
            None,
        );
        assert!(rows.len() >= 3, "{} rows", rows.len());
        for r in &rows {
            let l = line(r);
            assert!(
                (l.hz - 0.37).abs() <= 3.0 * l.sigma_hz,
                "row {}: {} ± {}",
                r.seq,
                l.hz,
                l.sigma_hz
            );
            assert!(l.sigma_hz < 0.01, "{}", l.sigma_hz);
            assert_eq!(r.bins_db.len(), 500);
            assert_eq!((r.start_hz, r.bin_hz, r.window_s), (-25.0, 0.1, 10.0));
        }
    }

    #[test]
    fn the_sign_follows_the_reference_convention() {
        let rate = 96_000.0;
        let up = run(
            ionosphere(),
            rate,
            15.0,
            tone(ionosphere(), rate, 20.0, |_| 0.5),
            None,
        );
        let down = run(
            ionosphere(),
            rate,
            15.0,
            tone(ionosphere(), rate, 20.0, |_| -0.5),
            None,
        );
        assert!((line(&up[0]).hz - 0.5).abs() < 0.01);
        assert!((line(&down[0]).hz + 0.5).abs() < 0.01);
    }

    #[test]
    fn a_slow_ramp_is_followed() {
        let rate = 96_000.0;
        let ramp = |t: f64| 0.2 + 0.01 * t;
        let rows = run(
            ionosphere(),
            rate,
            60.0,
            tone(ionosphere(), rate, 20.0, ramp),
            None,
        );
        assert!(rows.len() >= 9);
        for r in &rows {
            let centre_t = r.end_pair as f64 / rate - r.window_s / 2.0;
            assert!(
                (line(r).hz - ramp(centre_t)).abs() < 0.05,
                "row {}: {} vs {}",
                r.seq,
                line(r).hz,
                ramp(centre_t)
            );
        }
    }

    #[test]
    fn a_keyed_carrier_is_found_at_its_carrier_not_its_sidebands() {
        let rate = 96_000.0;
        let s = ionosphere();
        let amp = amp_for(&s, rate, 20.0);
        let (mut phase, mut seed) = (0.0, 7u64);
        let make = move |start, n| {
            let f = |_t: f64| OFFSET - 0.94;
            add(
                keyed(rate, &f, amp, start, n, &mut phase),
                &noise(n, SIGMA_IN, &mut seed),
            )
        };
        for r in run(s, rate, 25.0, make, None) {
            assert!(
                (line(&r).hz + 0.94).abs() < 0.05,
                "row {}: {}",
                r.seq,
                line(&r).hz
            );
        }
    }

    #[test]
    fn noise_alone_is_refused_not_guessed() {
        let rate = 96_000.0;
        let mut seed = 11u64;
        let rows = run(
            ionosphere(),
            rate,
            30.0,
            move |_, n| noise(n, SIGMA_IN, &mut seed),
            None,
        );
        assert!(!rows.is_empty());
        for r in rows {
            let why = r.line.unwrap_err();
            assert!(why.contains("no line above"), "{why}");
        }
    }

    #[test]
    fn two_lines_closer_than_two_bins_are_a_blend() {
        let rate = 96_000.0;
        let s = ionosphere();
        let two = |apart: f64| {
            let amp = amp_for(&s, rate, 25.0);
            let (mut p1, mut p2, mut seed) = (0.0, 1.0, 3u64);
            move |start, n| {
                let a = carrier(rate, &|_| OFFSET, amp, start, n, &mut p1);
                let b = carrier(rate, &|_| OFFSET + apart, amp * 0.9, start, n, &mut p2);
                add(add(a, &b), &noise(n, SIGMA_IN, &mut seed))
            }
        };
        let close = run(s, rate, 15.0, two(0.15), None);
        let why = close[0].line.as_ref().unwrap_err();
        assert!(why.contains("blend"), "{why}");
        let apart = run(s, rate, 15.0, two(1.0), None);
        assert!(
            line(&apart[0]).hz.abs() < 0.02,
            "the stronger one: {}",
            line(&apart[0]).hz
        );
    }

    /// The row names the strongest line in the span; a broadcaster louder than
    /// the carrier inside it is that line, and nothing claims it is the carrier.
    #[test]
    fn the_strongest_line_is_named_as_such() {
        let rate = 96_000.0;
        let s = ionosphere();
        let amp = amp_for(&s, rate, 15.0);
        let (mut p1, mut p2, mut seed) = (0.0, 1.0, 5u64);
        let make = move |start, n| {
            let c = carrier(rate, &|_| OFFSET, amp, start, n, &mut p1);
            let loud = carrier(rate, &|_| OFFSET + 18.0, amp * 31.6, start, n, &mut p2);
            add(add(c, &loud), &noise(n, SIGMA_IN, &mut seed))
        };
        let rows = run(s, rate, 15.0, make, None);
        assert!((line(&rows[0]).hz - 18.0).abs() < 0.05);
    }

    #[test]
    fn a_gap_marks_its_window_and_resets_the_filter() {
        let rate = 96_000.0;
        let gap_at = (12.5 * rate) as u64;
        let rows = run(
            ionosphere(),
            rate,
            30.0,
            tone(ionosphere(), rate, 20.0, |_| 0.2),
            Some(gap_at),
        );
        let hit: Vec<&Row> = rows
            .iter()
            .filter(|r| {
                r.end_pair as f64 / rate >= 12.5 && r.end_pair as f64 / rate - r.window_s <= 12.9
            })
            .collect();
        assert!(!hit.is_empty());
        for r in &hit {
            assert!(r.feed_loss, "row {} spans the gap", r.seq);
            assert!(
                (line(r).hz - 0.2).abs() <= 3.0 * line(r).sigma_hz + 0.01,
                "{}",
                line(r).hz
            );
        }
        let clean = rows
            .iter()
            .find(|r| r.end_pair as f64 / rate - r.window_s > 13.0)
            .unwrap();
        assert!(!clean.feed_loss);
    }

    /// A low-orbit pass at 145 MHz: ±3.5 kHz along a tanh, steepest at
    /// 87.5 Hz/s, followed at the satellite setting.
    #[test]
    fn the_satellite_setting_follows_an_s_curve() {
        let rate = 128_000.0;
        let s = CarrierSetting {
            hz: CENTRE + 40_000,
            span_hz: 10_000.0,
            resolution_hz: 10.0,
        };
        let curve = |t: f64| 3_500.0 * ((t - 60.0) / 40.0).tanh();
        let amp = amp_for(&s, rate, 20.0);
        let (mut phase, mut seed) = (0.0, 9u64);
        let make = move |start, n| {
            let f = |t: f64| 40_000.0 + curve(t);
            add(
                carrier(rate, &f, amp, start, n, &mut phase),
                &noise(n, SIGMA_IN, &mut seed),
            )
        };
        let rows = run(s, rate, 120.0, make, None);
        assert!(rows.len() > 2000, "{}", rows.len());
        for r in &rows {
            let t = r.end_pair as f64 / rate - r.window_s / 2.0;
            assert!(
                (line(r).hz - curve(t)).abs() < 15.0,
                "t {t}: {} vs {}",
                line(r).hz,
                curve(t)
            );
        }
    }

    /// A transmitter warming up: +10 Hz over a minute, at the drift setting.
    #[test]
    fn the_drift_setting_follows_a_warm_up() {
        let rate = 96_000.0;
        let s = CarrierSetting {
            hz: CENTRE + OFFSET as u64,
            span_hz: 500.0,
            resolution_hz: 0.5,
        };
        let warm = |t: f64| 10.0 * (1.0 - (-t / 60.0).exp());
        let rows = run(s, rate, 60.0, tone(s, rate, 20.0, warm), None);
        assert!(rows.len() > 50);
        for r in &rows {
            let t = r.end_pair as f64 / rate - r.window_s / 2.0;
            assert!(
                (line(r).hz - warm(t)).abs() < 1.0,
                "t {t}: {} vs {}",
                line(r).hz,
                warm(t)
            );
        }
    }

    /// **The sigma a row reports is not optimistic.** Over five minutes of a
    /// steady tone, each row's error over its own sigma spreads by about one.
    #[test]
    fn a_reported_sigma_is_honest() {
        let rate = 96_000.0;
        let rows = run(
            ionosphere(),
            rate,
            300.0,
            tone(ionosphere(), rate, 20.0, |_| 0.1),
            None,
        );
        let z: Vec<f64> = rows
            .iter()
            .map(|r| (line(r).hz - 0.1) / line(r).sigma_hz)
            .collect();
        let mean = z.iter().sum::<f64>() / z.len() as f64;
        let spread =
            (z.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (z.len() - 1) as f64).sqrt();
        assert!(z.len() >= 50, "{}", z.len());
        // 0.8 to 1.25: a sigma half again too wide (0.62) fails, and so does
        // one that is optimistic by a third.
        assert!(
            (0.8..=1.25).contains(&spread),
            "error/sigma spreads by {spread:.2} over {} rows",
            z.len()
        );
        assert!(mean.abs() < 0.6, "biased by {mean:.2} sigma");
    }
}
