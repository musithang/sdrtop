// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! From a repeat found to a frame of this channel: the offset, the timing,
//! and the long training field's verdict on whose frame it is.
//!
//! **The gate has a law, as the detector does.** The long training field's
//! two symbols, 128 samples, are compared with a reference, normalised by
//! both energies: for noise alone the squared cosine is Beta(1, 127)
//! whatever the reference, so its threshold is a stated false-alarm rate
//! ([`GATE_FALSE_ALARM`]), not a level tuned on a recording.
//!
//! **A neighbour is judged on what of it we can hear.** A frame 5, 10 or 15
//! MHz away has its long training tones 16, 32 or 48 subcarriers along; the
//! ones that land outside our own +-26 are beyond the capture or cut by our
//! filter. So each shift's reference holds only the tones that land inside,
//! and the shift whose reference matches best names the neighbour. Without
//! that, a partly heard neighbour would be no frame at all.
//!
//! **The offset comes twice, coarse then fine** (Moose, cited in
//! `dsp::estimate`): over the short field's 16-sample repeat, which reaches
//! +-625 kHz, then over the long field's two 64-sample symbols for what is
//! left, which reaches +-156 kHz. Its uncertainty is the long field's own:
//! `estimate::moose_variance` at the SNR that field's coherence gives.

use num_complex::Complex;
use rustfft::FftPlanner;

use crate::signal::dsp::correlate::threshold_for_false_alarm;
use crate::signal::dsp::estimate::{moose_offset, moose_variance, snr_from_metric};
use crate::signal::dsp::uncertainty::Uncertain;

use super::ofdm::LONG;

/// The sample rate everything here is at (a 20 MHz channel, 17.3.2.4).
const FS: f64 = 20e6;

/// The chance that noise alone passes the gate at one shift and one timing.
pub const GATE_FALSE_ALARM: f64 = 1e-8;

/// The neighbours judged: 5, 10 and 15 MHz either side, as subcarriers of
/// 312.5 kHz (16 to a channel), and ourselves.
const SHIFTS: [i32; 7] = [0, -1, 1, -2, 2, -3, 3];

/// Where the long training field's first symbol is searched for, against
/// the detection's peak, which lands from about 110 to 170 samples into the
/// frame; the symbol starts at 192.
const SEARCH: std::ops::Range<i64> = -10..130;

/// A frame of this channel, synchronised.
#[derive(Clone, Copy, Debug)]
pub struct Sync {
    /// The stream pair the long training field's first symbol starts on.
    pub ltf_start: u64,
    /// The transmitter's offset from our tuning, in hertz, with its
    /// uncertainty. Our own oscillator's error is in it.
    pub offset: Uncertain,
    /// The per-sample signal-to-noise ratio the long field's coherence
    /// gives, as a power ratio.
    pub snr: f64,
}

/// What the long training field says the repeat was.
#[derive(Clone, Copy, Debug)]
pub enum Verdict {
    ThisChannel(Sync),
    /// A frame on the channel `shift_mhz` away (5, 10 or 15 MHz either
    /// side), which the Frames list names.
    Adjacent {
        shift_mhz: i32,
    },
    /// Nothing matched: a repeat, but no frame of ours or a neighbour's.
    NotPreamble,
    /// A repeat that correlates more 0.4 us apart than 0.8 us apart: no
    /// short training field does, whatever channel it came through.
    NotShortField,
}

/// The two-symbol reference for a neighbour `shift` channels' worth of
/// subcarriers away (16 each): the long field's tones that land inside our
/// own +-26, through the 64-point inverse transform, twice.
fn reference(shift: i32) -> Vec<Complex<f64>> {
    let mut spectrum = vec![Complex::new(0.0, 0.0); 64];
    for (i, &v) in LONG.iter().enumerate() {
        let q = i as i32 - 26 + 16 * shift;
        if v != 0 && (-26..=26).contains(&q) {
            spectrum[q.rem_euclid(64) as usize] = Complex::new(v as f64, 0.0);
        }
    }
    FftPlanner::new()
        .plan_fft_inverse(64)
        .process(&mut spectrum);
    spectrum.iter().chain(spectrum.iter()).copied().collect()
}

/// Every shift's reference, built once.
fn references() -> &'static [(i32, Vec<Complex<f64>>)] {
    static REFS: std::sync::OnceLock<Vec<(i32, Vec<Complex<f64>>)>> = std::sync::OnceLock::new();
    REFS.get_or_init(|| SHIFTS.iter().map(|&s| (s, reference(s))).collect())
}

/// The squared cosine between `w` and `r`, both 128 samples.
fn fit(r: &[Complex<f64>], w: &[Complex<f64>]) -> f64 {
    let (mut dot, mut er, mut ew) = (Complex::new(0.0, 0.0), 0.0, 0.0);
    for (a, b) in r.iter().zip(w) {
        dot += a.conj() * b;
        er += a.norm_sqr();
        ew += b.norm_sqr();
    }
    if er > 0.0 && ew > 0.0 {
        dot.norm_sqr() / (er * ew)
    } else {
        0.0
    }
}

/// The stretch of the stream a detection that peaked at `peak` is read
/// from: from 127 samples before it to the end of the SIGNAL symbol after
/// the latest long field the search could find, so the verdict and the
/// L-SIG ([`super::lsig`]) are read from one stretch. `None` too early in
/// the stream to hold.
pub fn needs(peak: u64) -> Option<(u64, usize)> {
    let from = peak.checked_sub(127)?;
    let len = 127 + SEARCH.end as usize + super::sig::FIRST_AT + 80 * (super::sig::HELD - 1) + 64;
    Some((from, len))
}

/// Judge the repeat whose detection peaked at stream pair `peak`, from
/// `samples` starting at `first_pair`. `None` when what it needs is not all
/// held yet (or was never held): 127 samples before the peak, and the long
/// field's search beyond it.
pub fn examine(samples: &[Complex<f32>], first_pair: u64, peak: u64) -> Option<Verdict> {
    let p = peak.checked_sub(first_pair)? as i64;
    let first = p + SEARCH.start;
    let last = p + SEARCH.end + 128;
    if p < 127 || first < 0 || last > samples.len() as i64 {
        return None;
    }
    let at = |n: i64| {
        let z = samples[n as usize];
        Complex::new(z.re as f64, z.im as f64)
    };

    // A short field repeats every 16 samples (0.8 us) and cancels 8 apart:
    // its tones sit every 1.25 MHz, alternately in and out of phase 0.4 us
    // apart. A carrier or any narrowband signal correlates with itself at
    // both. So a repeat that correlates more at 8 than at 16 is no short
    // field, through whatever channel it came (an echo 0.4 us late, the
    // worst there is, never lifts 8 above 16 unless it is as strong as the
    // frame), and it is refused before the match that costs the most here.
    // Both the squared cosine between two 64-sample windows, in the samples
    // the detector compared.
    let repeat = |lag: i64| {
        let (mut c, mut e0, mut e1) = (Complex::new(0.0, 0.0), 0.0, 0.0);
        for n in p - 127..p - 127 + 64 {
            let (a, b) = (at(n), at(n + lag));
            c += a * b.conj();
            e0 += a.norm_sqr();
            e1 += b.norm_sqr();
        }
        if e0 > 0.0 && e1 > 0.0 {
            c.norm_sqr() / (e0 * e1)
        } else {
            0.0
        }
    };
    if repeat(8) > repeat(16) {
        return Some(Verdict::NotShortField);
    }

    // Coarse: the short field's 16-sample repeat over the 112 samples
    // ending at the peak.
    let mut p16 = Complex::new(0.0, 0.0);
    for n in p - 111..=p {
        p16 += at(n - 16).conj() * at(n);
    }
    let coarse = moose_offset(p16, 16);

    // Derotated by it, from the search's first sample.
    let y: Vec<Complex<f64>> = (first..last)
        .map(|n| at(n) * Complex::from_polar(1.0, -std::f64::consts::TAU * coarse * (n - p) as f64))
        .collect();

    // The best timing and fit for each shift.
    let gate = threshold_for_false_alarm(128, GATE_FALSE_ALARM);
    let mut best: Option<(f64, i32, usize)> = None;
    for (shift, r) in references() {
        for t in 0..(SEARCH.end - SEARCH.start) as usize {
            let f = fit(r, &y[t..t + 128]);
            if best.is_none_or(|(b, _, _)| f > b) {
                best = Some((f, *shift, t));
            }
        }
    }
    let (fit_best, shift, t) = best?;
    if fit_best < gate {
        return Some(Verdict::NotPreamble);
    }
    if shift != 0 {
        return Some(Verdict::Adjacent {
            shift_mhz: 5 * shift,
        });
    }

    // Fine: the long field's two symbols against each other.
    let (mut p64, mut e1, mut e2) = (Complex::new(0.0, 0.0), 0.0, 0.0);
    for k in 0..64 {
        p64 += y[t + k].conj() * y[t + 64 + k];
        e1 += y[t + k].norm_sqr();
        e2 += y[t + 64 + k].norm_sqr();
    }
    let fine = moose_offset(p64, 64);
    let metric = if e1 > 0.0 && e2 > 0.0 {
        p64.norm_sqr() / (e1 * e2)
    } else {
        0.0
    };
    let snr = snr_from_metric(metric, 64).unwrap_or(f64::INFINITY);
    let variance = moose_variance(snr, 64, 64) * FS * FS;
    Some(Verdict::ThisChannel(Sync {
        ltf_start: (first_pair as i64 + first + t as i64) as u64,
        offset: Uncertain::from_variance((coarse + fine) * FS, variance),
        snr,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::dsp::testkit::Rng;
    use crate::signal::wifi::annex::MESSAGE;
    use crate::signal::wifi::detect::StfDetector;
    use crate::signal::wifi::ppdu::ppdu;
    use crate::signal::wifi::signal_field::Rate;

    const FS: f64 = 20e6;

    /// The annex's frame `lead` samples into noise `snr_db` below it,
    /// shifted by `shift_hz`.
    fn on_air(lead: usize, snr_db: f64, shift_hz: f64, rng: &mut Rng) -> Vec<Complex<f32>> {
        let f = ppdu(&MESSAGE, Rate::R36, 54);
        let power = f[..320].iter().map(|z| z.norm_sqr()).sum::<f64>() / 320.0;
        let mut x = vec![Complex::new(0.0f32, 0.0); lead + f.len() + 600];
        for (o, z) in x[lead..].iter_mut().zip(&f) {
            let z = z / power.sqrt();
            *o = Complex::new(z.re as f32, z.im as f32);
        }
        // A neighbour channels away through the band-limited model; an
        // offset of a few kilohertz exactly, by a mixer, which cannot wrap.
        if shift_hz.abs() >= 1e6 {
            x = heard_shifted(&x, shift_hz);
        } else if shift_hz != 0.0 {
            crate::signal::dsp::nco::Nco::new(shift_hz, FS).mix(&mut x);
        }
        let noise = rng.noise(x.len(), 10f64.powf(-snr_db / 10.0));
        x.iter().zip(noise).map(|(s, n)| s + n).collect()
    }

    /// `x` moved `shift_hz` up, as a receiver at 20 Msample/s hears it: at
    /// twice the rate (the spectrum zero-padded), shifted without wrapping,
    /// and cut back to +-10 MHz, an ideal filter in place of ours. A plain
    /// mix at 20 Msample/s would wrap a 15 MHz neighbour round to -5 MHz,
    /// which no radio with a filter does.
    fn heard_shifted(x: &[Complex<f32>], shift_hz: f64) -> Vec<Complex<f32>> {
        let n = x.len();
        let mut spec: Vec<Complex<f64>> = x
            .iter()
            .map(|z| Complex::new(z.re as f64, z.im as f64))
            .collect();
        FftPlanner::new().plan_fft_forward(n).process(&mut spec);
        let bin = |k: usize| {
            if k < n / 2 {
                k as i64
            } else {
                k as i64 - n as i64
            }
        };
        let move_by = (shift_hz / FS * n as f64).round() as i64;
        let mut out = vec![Complex::new(0.0, 0.0); n];
        for (k, v) in spec.iter().enumerate() {
            let to = bin(k) + move_by;
            if to >= -(n as i64) / 2 && to < n as i64 / 2 {
                out[to.rem_euclid(n as i64) as usize] = *v;
            }
        }
        FftPlanner::new().plan_fft_inverse(n).process(&mut out);
        out.iter()
            .map(|z| Complex::new((z.re / n as f64) as f32, (z.im / n as f64) as f32))
            .collect()
    }

    /// The detector's peak on `x`, then the verdict there.
    fn judge(x: &[Complex<f32>]) -> Option<Verdict> {
        let found = StfDetector::new().push(0, x);
        let d = found.first()?;
        examine(x, 0, d.peak)
    }

    /// At 10 dB the long training field's first symbol is placed to the
    /// sample: 192 samples after the frame starts (8 us of short field and
    /// the 1.6 us guard, 17.3.3).
    #[test]
    fn the_timing_is_exact_at_ten_db() {
        let mut rng = Rng::new(10);
        let mut exact = 0;
        for i in 0..50 {
            let lead = 500 + 37 * i;
            match judge(&on_air(lead, 10.0, 0.0, &mut rng)) {
                Some(Verdict::ThisChannel(s)) if s.ltf_start == (lead + 192) as u64 => {
                    // The long field's coherence says about the SNR it was
                    // made at.
                    assert!((5.0..20.0).contains(&s.snr), "snr {}", s.snr);
                    exact += 1
                }
                other => eprintln!("lead {lead}: {other:?}"),
            }
        }
        assert_eq!(exact, 50);
    }

    /// The offset across the 2.4 GHz band's +-25 ppm (+-60 kHz) and beyond
    /// it: unbiased, and spread as its own stated uncertainty says, at
    /// 10 dB over many frames.
    #[test]
    fn the_offset_is_unbiased_and_as_uncertain_as_it_says() {
        let mut rng = Rng::new(11);
        for offset in [-150e3, -60e3, 0.0, 60e3, 150e3] {
            let mut errors = Vec::new();
            let mut stated = Vec::new();
            for _ in 0..80 {
                if let Some(Verdict::ThisChannel(s)) = judge(&on_air(700, 10.0, offset, &mut rng)) {
                    errors.push(s.offset.value() - offset);
                    stated.push(s.offset.sigma());
                }
            }
            assert!(errors.len() >= 78, "{offset}: {} judged", errors.len());
            let n = errors.len() as f64;
            let mean = errors.iter().sum::<f64>() / n;
            let spread =
                (errors.iter().map(|e| (e - mean).powi(2)).sum::<f64>() / (n - 1.0)).sqrt();
            let sigma = stated.iter().sum::<f64>() / n;
            eprintln!(
                "{:+.0} kHz: bias {mean:+.0} Hz, spread {spread:.0} Hz, stated {sigma:.0} Hz, ratio {:.2}",
                offset / 1e3,
                spread / sigma
            );
            assert!(
                mean.abs() < 3.0 * spread / n.sqrt() + 1.0,
                "{offset}: bias {mean} Hz, spread {spread}"
            );
            assert!(
                (0.7..1.5).contains(&(spread / sigma)),
                "{offset}: spread {spread:.0} Hz against stated {sigma:.0}"
            );
        }
    }

    /// A frame on a channel 5, 10 or 15 MHz away is judged adjacent, by the
    /// shift it is at, and never as this channel's.
    #[test]
    fn a_neighbours_frame_is_judged_adjacent_by_its_shift() {
        let mut rng = Rng::new(12);
        for mhz in [-15, -10, -5, 5, 10, 15] {
            for _ in 0..10 {
                let x = on_air(800, 15.0, mhz as f64 * 1e6, &mut rng);
                match judge(&x) {
                    Some(Verdict::Adjacent { shift_mhz }) => assert_eq!(shift_mhz, mhz),
                    // A neighbour this far out may not pass the short field
                    // at all; it must never pass as ours.
                    None => {}
                    other => panic!("{mhz} MHz: {other:?}"),
                }
            }
        }
    }

    /// A burst like Bluetooth's at `hz` from our centre: 1 Mb/s, the
    /// frequency 250 kHz either side of it, 50 us long, 20 dB over the noise.
    fn fsk_burst(hz: f64, rng: &mut Rng) -> Vec<Complex<f32>> {
        let mut x = vec![Complex::new(0.0f32, 0.0); 3000];
        let mut phase = 0.0f64;
        let mut f = hz;
        for (n, z) in x.iter_mut().enumerate().skip(500).take(1000) {
            if n % 20 == 0 {
                f = hz
                    + if rng.next_u64() & 1 == 1 {
                        250e3
                    } else {
                        -250e3
                    };
            }
            phase += std::f64::consts::TAU * f / FS;
            *z = Complex::new(phase.cos() as f32, phase.sin() as f32);
        }
        let noise = rng.noise(x.len(), 0.01);
        x.iter().zip(noise).map(|(s, n)| s + n).collect()
    }

    /// **A narrowband burst is no short field.** Bluetooth's kind of signal
    /// passes the detector's 3.2 us repeat, and correlates with itself more
    /// 0.4 us apart than 0.8 us apart; a short field cancels at 0.4 us.
    /// Refused before the long field's match eight times in ten at least
    /// (measured, 46 to 50 in 50),
    /// wherever it sits: a burst whose bits run alike for most of the
    /// window is a tone there, which the match refuses instead. Never a
    /// frame of this channel.
    #[test]
    fn a_narrowband_burst_is_no_short_field() {
        let mut rng = Rng::new(16);
        for hz in [0.3e6, 2.1e6, -4.7e6, 7.9e6] {
            let mut refused = 0;
            for _ in 0..50 {
                let v = judge(&fsk_burst(hz, &mut rng));
                assert!(!matches!(v, Some(Verdict::ThisChannel(_))), "{hz}: {v:?}");
                refused += matches!(v, Some(Verdict::NotShortField)) as usize;
            }
            assert!(refused >= 40, "{hz}: {refused} of 50");
        }
    }

    /// A pure tone correlates alike at both spacings, so the test cannot
    /// refuse it; the long field's match does. It is never a frame of this
    /// channel.
    #[test]
    fn a_pure_tone_is_never_this_channels() {
        let mut rng = Rng::new(19);
        for hz in [0.3e6, 2.1e6, -4.7e6, 7.9e6] {
            let mut x = vec![Complex::new(0.0f32, 0.0); 3000];
            for (n, z) in x.iter_mut().enumerate().skip(500).take(1000) {
                let ph = std::f64::consts::TAU * hz / FS * n as f64;
                *z = Complex::new(ph.cos() as f32, ph.sin() as f32);
            }
            let noise = rng.noise(x.len(), 0.01);
            let x: Vec<Complex<f32>> = x.iter().zip(noise).map(|(s, n)| s + n).collect();
            let v = judge(&x);
            assert!(!matches!(v, Some(Verdict::ThisChannel(_))), "{hz}: {v:?}");
        }
    }

    /// **An echo does not make a frame no short field, up to half its
    /// strength.** The worst a channel can do to the test is an echo 0.4 us
    /// late, which raises every other tone of the short field and lowers the
    /// rest. At 0.3 and 0.5 of the frame's amplitude, at 8 and 15 dB, the
    /// frame is this channel's every time. (Measured beyond: at 0.7, 8 dB,
    /// 31 frames in 40 are refused, and at 0.9, 15 dB, all; 1.2 us late, 4 in
    /// 40 at 0.9; other delays none. Such an echo, 120 m of extra path at
    /// two thirds of the frame's strength, is the case the test gives up.)
    #[test]
    fn a_frame_through_an_echo_is_still_a_short_field() {
        let mut rng = Rng::new(17);
        for a in [0.3f32, 0.5] {
            for snr in [8.0, 15.0] {
                for _ in 0..20 {
                    let x = on_air(800, snr, 0.0, &mut rng);
                    let echoed: Vec<Complex<f32>> = (0..x.len())
                        .map(|n| {
                            x[n] + if n >= 8 {
                                x[n - 8] * a
                            } else {
                                Complex::new(0.0, 0.0)
                            }
                        })
                        .collect();
                    match judge(&echoed) {
                        Some(Verdict::ThisChannel(_)) => {}
                        other => panic!("echo {a} at {snr} dB: {other:?}"),
                    }
                }
            }
        }
    }

    /// Where there is only noise, wherever the gate is asked, there is no
    /// preamble: refused by the period test or by the match.
    #[test]
    fn noise_is_not_a_preamble() {
        let mut rng = Rng::new(13);
        let x = rng.noise(20_000, 1.0);
        for peak in (200..19_000).step_by(997) {
            match examine(&x, 0, peak as u64) {
                Some(Verdict::NotPreamble | Verdict::NotShortField) => {}
                other => panic!("{peak}: {other:?}"),
            }
        }
    }

    /// **What a verdict costs, measured.** Not a check: by hand, in release
    /// (`cargo test --release what_a_verdict_costs -- --ignored
    /// --nocapture`).
    #[test]
    #[ignore]
    fn what_a_verdict_costs() {
        let mut rng = Rng::new(15);
        let x = on_air(700, 10.0, 0.0, &mut rng);
        let peak = StfDetector::new().push(0, &x)[0].peak;
        let start = std::time::Instant::now();
        for _ in 0..2000 {
            std::hint::black_box(examine(std::hint::black_box(&x), 0, peak));
        }
        let each = start.elapsed().as_secs_f64() / 2000.0;
        eprintln!(
            "{:.0} us a verdict; at 200 repeats a second, {:.1} % of a core",
            each * 1e6,
            each * 200.0 * 100.0
        );
    }

    /// What a repeat needs held is what `examine` reads and the symbols
    /// a format is told from after the latest long field it could find:
    /// given that stretch, it judges and the L-SIG is read; without those
    /// symbols' worth, `examine` still judges.
    #[test]
    fn needs_holds_the_verdict_and_the_signal_field() {
        let mut rng = Rng::new(16);
        let x = on_air(900, 12.0, 0.0, &mut rng);
        let peak = StfDetector::new().push(0, &x)[0].peak;
        let (from, len) = needs(peak).unwrap();
        let cut = &x[from as usize..from as usize + len];
        let Some(Verdict::ThisChannel(s)) = examine(cut, from, peak) else {
            panic!("not judged this channel's");
        };
        assert!(matches!(
            crate::signal::wifi::lsig::read(cut, from, &s),
            Some(Ok(_))
        ));
        let after = 80 * crate::signal::wifi::sig::HELD;
        assert!(examine(&cut[..len - after], from, peak).is_some());
        assert!(examine(&cut[..len - after - 1], from, peak).is_none());
        assert_eq!(needs(100), None);
    }

    /// Too near either end of what is held, it waits rather than guessing.
    #[test]
    fn too_near_the_edge_it_does_not_judge() {
        let mut rng = Rng::new(14);
        let x = rng.noise(600, 1.0);
        assert!(examine(&x, 0, 50).is_none());
        assert!(examine(&x, 0, 500).is_none());
    }
}
