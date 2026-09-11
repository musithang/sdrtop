// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Two ways to find a signal's own symbol phase, offered as alternatives
//! rather than a pipeline of both: [`recover`] (Gardner's adaptive detector)
//! and [`find_phase`] (exhaustive search over one symbol period). Design
//! section 1.1 names both and says to use whichever the test vectors say is
//! enough - and for `signal::ble`'s actual signal, they said something
//! specific enough to record here rather than only in the arc's own plan.
//!
//! **[`recover`] is validated on its own terms and used by neither arc yet.**
//! Gardner's detector is derived for a raised-cosine-shaped baseband PAM/PSK
//! signal, where the "S-curve" - the error's own value as a function of
//! phase - has a single zero exactly at the correct sampling instant. Fed a
//! Gaussian-filtered GFSK discriminator output instead, this module's own
//! development measured the loop converging every time, but to a stable
//! phase consistently off by around an eighth of a symbol rather than to
//! zero - a real S-curve bias from feeding it a pulse shape it was not
//! derived for, not a bug in the loop, and confirmed as much by
//! `a_correct_starting_phase_is_held` and `a_wrong_starting_phase_is_corrected`
//! passing cleanly on the PAM-style signal Gardner *is* derived for. That
//! bias was small - well under a sample at 4 samples per symbol - but large
//! enough to cost real bit errors even with no noise added at all, which
//! [`find_phase`] does not. `signal::ble::sync` uses [`find_phase`]
//! because of this, measured rather than assumed; see its own module doc.
//!
//! **[`recover`] tracks a static phase, not a drifting clock**, which is the
//! other reason it stays a validated-but-unused primitive rather than the
//! arc's choice: BLE's own capture is short bursts, exactly what
//! [`find_phase`]'s "search once, hold it" approach fits, while a loop that
//! tracks a phase changing over time is what a long capture with real sample-
//! clock skew would need - a real thing a real capture could show, which is
//! why the primitive is kept rather than deleted.

/// Linear interpolation of `x` at a fractional index `pos`. Clamped to the
/// array's own range: a probe earlier than the buffer starts, or later than
/// it ends, is asking for a sample this buffer does not have, and the honest
/// answer is the nearest one it does, not a sample invented past the edge.
pub fn interpolate(x: &[f32], pos: f64) -> f32 {
    if x.is_empty() {
        return 0.0;
    }
    let clamped = pos.clamp(0.0, (x.len() - 1) as f64);
    let i = clamped.floor() as usize;
    let frac = (clamped - i as f64) as f32;
    if i + 1 < x.len() {
        x[i] + (x[i + 1] - x[i]) * frac
    } else {
        x[i]
    }
}

/// Recover one sample per symbol from `x`, tracking the symbol phase with
/// Gardner's detector starting from an initial guess `start` that need not be
/// exact.
///
/// `sps` is the nominal samples per symbol. `gain` is the loop's proportional
/// gain: too small and the loop never reaches the true phase in the symbols
/// available; too large and it chases the noise instead of the phase.
/// `0 < gain < 1` is the useful range for every gain this module's own tests
/// exercise, the same way `dsp::fir`'s Kaiser beta has a range with nothing
/// useful past it - and, critically, that range only means the same thing at
/// every call site because the error below is normalised by the signal's own
/// power first. Gardner's raw error is bilinear in the signal's amplitude, so
/// an unnormalised `gain` tuned against a unit-amplitude test signal would be
/// six orders of magnitude too large against a discriminator's output in Hz
/// and the loop would diverge on its first symbol - measured, not guessed:
/// this is exactly the failure `dsp::correlate`'s own doc gives as the reason
/// its detectors report a normalised coherence rather than a raw level.
///
/// The detector, per symbol `k` at the loop's current phase estimate `mu`:
/// `e_k = y(mu - sps/2) * (y(mu) - y(mu - sps)) / P` - the sample halfway
/// through the symbol just finished, times how much the signal changed
/// across it, divided by `P`, the signal's mean squared value over the whole
/// buffer. Zero when `mu` sits exactly on the symbol boundary, because the
/// halfway sample is then the peak of a boundary-to-boundary transition and
/// perpendicular to the direction the phase would need to move; the sign
/// away from zero says which way `mu` is off. `mu` for the next symbol is
/// `mu + sps + gain * e_k`.
///
/// No consumer outside this module's own tests - see the module doc for why
/// `signal::ble::sync` uses [`find_phase`] instead.
#[allow(dead_code)]
pub fn recover(x: &[f32], start: f64, sps: f64, gain: f64, symbols: usize) -> Vec<f32> {
    let power = if x.is_empty() {
        1.0
    } else {
        (x.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / x.len() as f64).max(1e-12)
    };
    let mut mu = start;
    let mut prev_on_time = interpolate(x, mu - sps);
    let mut out = Vec::with_capacity(symbols);
    for _ in 0..symbols {
        let on_time = interpolate(x, mu);
        let mid = interpolate(x, mu - sps / 2.0);
        let error = mid as f64 * (on_time - prev_on_time) as f64 / power;
        out.push(on_time);
        mu += sps + gain * error;
        prev_on_time = on_time;
    }
    out
}

/// The samples-per-symbol-periodic phase, in `[0, sps)`, at which sampling
/// `x` every `sps` samples reads the loudest.
///
/// Every candidate phase, `resolution` of them spread evenly across one
/// symbol period, is scored by the sum of `|x|` at that phase and every
/// `sps` samples after it for `symbols` symbols; the candidate with the
/// highest score wins. This is the "peak-picking" design section 1.1 names
/// as Gardner's alternative: no loop, no gain to tune, and correct exactly
/// when the true sampling instant really is where the signal is loudest -
/// true for a symbol whose deviation is at its full value at the correct
/// instant and passes through a transition, and therefore near zero, at the
/// wrong one, which is what a GFSK discriminator's output actually looks
/// like.
///
/// Static, not adaptive: it looks once, at the whole span given, and returns
/// one phase for all of it. A signal whose timing drifts across `symbols`
/// symbols needs [`recover`] instead, or a shorter span here repeated.
///
/// Called for real by `signal::ble::sync::slice` (B4) - but that function's
/// own caller has no path from `main` yet, the same position `dsp::correlate`
/// and `ble::gfsk` were in until B6 wires a live capture into this arc.
///
/// **Open lead from B6's real-hardware testing, not yet acted on here.**
/// Every phase this function returned on a real HackRF capture was an exact
/// integer number of samples - never one of the fractional candidates
/// `resolution` is supposed to also try. The likely cause: interpolating
/// between two independent noisy samples has less variance than either
/// sample alone (half, at the midpoint), so scoring interpolated amplitude
/// is biased toward whichever candidates land on a real, unblended sample -
/// an effect proportional to noise, which is why no synthetic test here
/// caught it. A first fix (score the nearest raw sample instead of an
/// interpolated one) removed that bias but cost `a_noiseless_packet_slices_
/// to_exactly_its_own_bits` its exact match, because the coarser, integer-
/// only resolution loses real precision this arc's own clean signal needs.
/// Reverted rather than landed half-verified: the right fix likely scores a
/// small window around each candidate rather than a single point, trading
/// neither noise robustness nor precision, but that is untested and B6's
/// own plan is where this is recorded rather than guessed at further here.
#[allow(dead_code)]
pub fn find_phase(x: &[f32], sps: f64, symbols: usize, resolution: usize) -> f64 {
    let resolution = resolution.max(1);
    let mut best_phase = 0.0;
    let mut best_score = f64::NEG_INFINITY;
    for step in 0..resolution {
        let phase = sps * step as f64 / resolution as f64;
        let score: f64 = (0..symbols)
            .map(|k| interpolate(x, phase + k as f64 * sps).abs() as f64)
            .sum();
        if score > best_score {
            best_score = score;
            best_phase = phase;
        }
    }
    best_phase
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A straight line interpolates exactly at any fractional position - the
    /// definition of linear interpolation, checked so a later refactor of
    /// [`interpolate`] cannot silently change what "linear" means here.
    #[test]
    fn interpolation_is_exact_on_a_straight_line() {
        let x: Vec<f32> = (0..20).map(|i| i as f32 * 0.5).collect();
        for p in [0.0, 3.25, 7.5, 18.9] {
            let got = interpolate(&x, p);
            assert!((got - p as f32 * 0.5).abs() < 1e-5, "at {p}: got {got}");
        }
    }

    /// Past either edge, the answer is the nearest real sample - not zero,
    /// not an extrapolation, since neither is a sample this buffer has.
    #[test]
    fn interpolation_clamps_at_the_edges() {
        let x = [1.0f32, 2.0, 3.0];
        assert_eq!(interpolate(&x, -5.0), 1.0);
        assert_eq!(interpolate(&x, 50.0), 3.0);
    }

    /// A continuous stand-in for a shaped pulse train: symbol `k`'s value
    /// sits exactly at index `k * sps + sps / 2`, and everywhere else is the
    /// straight line between neighbouring symbols. This is what makes the
    /// test signal a fair one for Gardner's detector: a true rectangular
    /// hold has an instantaneous jump exactly at the sample a correct `mu`
    /// probes for the "mid" reading, which is ill-defined on an integer
    /// grid, and a real Gaussian-filtered signal never has that discontinuity
    /// in the first place.
    fn ramp(symbols: &[f32], sps: f64, len: usize) -> Vec<f32> {
        (0..len)
            .map(|i| interpolate(symbols, (i as f64 - sps / 2.0) / sps))
            .collect()
    }

    /// A clean symbol stream, exactly on its own grid, recovers the symbols
    /// unchanged and settles at the timing it started at - the loop must not
    /// wander away from a phase that was already correct.
    #[test]
    fn a_correct_starting_phase_is_held() {
        const SPS: f64 = 8.0;
        let symbols = [1.0f32, -1.0, -1.0, 1.0, 1.0, 1.0, -1.0, -1.0, 1.0, -1.0];
        let x = ramp(&symbols, SPS, symbols.len() * SPS as usize);
        let start = 1.0 * SPS + SPS / 2.0;
        let got = recover(&x, start, SPS, 0.05, symbols.len() - 2);
        for (i, (&g, &want)) in got.iter().zip(symbols.iter().skip(1)).enumerate() {
            assert!((g - want).abs() < 0.05, "symbol {i}: got {g}, want {want}");
        }
    }

    /// **The property that makes this a timing recovery rather than a fixed
    /// slice.** Starting from a phase that is wrong by a third of a symbol,
    /// the loop pulls itself onto the transitions and recovers the same
    /// symbols correctly - the thing a fixed `k * sps + sps / 2` slice, which
    /// is what B2's own tests use, cannot do without already knowing the
    /// answer.
    #[test]
    fn a_wrong_starting_phase_is_corrected() {
        const SPS: f64 = 8.0;
        let mut rng_state = 1u64;
        let mut next = || {
            rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1);
            (rng_state >> 32) & 1 == 1
        };
        let symbols: Vec<f32> = (0..200).map(|_| if next() { 1.0 } else { -1.0 }).collect();
        let x = ramp(&symbols, SPS, symbols.len() * SPS as usize);
        // A third of a symbol off, and on the wrong side of it than
        // `a_correct_starting_phase_is_held` starts from, so this is not
        // just the same case with a smaller number.
        let wrong_start = SPS + SPS / 2.0 + SPS / 3.0;
        let got = recover(&x, wrong_start, SPS, 0.05, symbols.len() - 20);
        // The first several symbols are the loop still acquiring; only the
        // settled tail is what this test is about.
        let settle = 30;
        for (i, (&g, &want)) in got
            .iter()
            .skip(settle)
            .zip(symbols.iter().skip(1 + settle))
            .enumerate()
        {
            assert!(
                (g.signum() - want.signum()).abs() < 0.01,
                "symbol {}: got {g}, want sign {want}",
                i + settle
            );
        }
    }

    /// The search finds a phase close to the one a signal was actually built
    /// with, on the same ramp signal Gardner's own tests use.
    #[test]
    fn the_search_finds_the_phase_the_signal_was_built_with() {
        const SPS: f64 = 8.0;
        let symbols = [1.0f32, -1.0, -1.0, 1.0, 1.0, 1.0, -1.0, -1.0, 1.0, -1.0];
        let true_phase = 3.0;
        let x: Vec<f32> = (0..symbols.len() * SPS as usize)
            .map(|i| interpolate(&symbols, (i as f64 - true_phase) / SPS))
            .collect();
        let found = find_phase(&x, SPS, symbols.len() - 1, 32);
        assert!(
            (found - true_phase).abs() < SPS / 32.0 + 1e-9,
            "found {found}, true phase {true_phase}"
        );
    }

    /// The property this exists for: on a signal where Gardner's own loop
    /// measurably settles away from the true centre (see the module doc),
    /// the search still lands close to it, because it never assumed the
    /// pulse shape Gardner's derivation does.
    #[test]
    fn the_search_does_not_share_gardners_bias_on_a_gfsk_like_signal() {
        // A Gaussian-ish smoothing, built the same way `ble::gfsk` builds
        // its shaping filter, so this signal has the same kind of transition
        // shape without depending on that module.
        const SPS: f64 = 4.0;
        let taps = crate::signal::dsp::fir::gaussian_taps(0.5, SPS as usize, 4);
        let half = (taps.len() as isize - 1) / 2;
        let mut rng_state = 7u64;
        let mut next_bit = || {
            rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1);
            if (rng_state >> 32) & 1 == 1 {
                1.0f32
            } else {
                -1.0f32
            }
        };
        let symbols: Vec<f32> = (0..300).map(|_| next_bit()).collect();
        let true_phase = SPS / 2.0 + 1.3;
        let n = (symbols.len() as f64 * SPS) as usize;
        let mut nrz = vec![0.0f32; n];
        for (i, &s) in symbols.iter().enumerate() {
            let start = ((i as f64) * SPS + true_phase - SPS / 2.0).round() as isize;
            for j in 0..SPS as isize {
                let idx = start + j;
                if idx >= 0 && (idx as usize) < n {
                    nrz[idx as usize] = s;
                }
            }
        }
        let x: Vec<f32> = (0..n)
            .map(|i| {
                taps.iter()
                    .enumerate()
                    .map(|(k, &h)| {
                        let j = i as isize - half + k as isize;
                        let v = if j >= 0 && (j as usize) < n {
                            nrz[j as usize]
                        } else {
                            0.0
                        };
                        h * v
                    })
                    .sum()
            })
            .collect();

        let found = find_phase(&x, SPS, 100, 16);
        assert!(
            (found - true_phase).abs() < 1.0,
            "found {found}, true phase {true_phase} - within a sample is enough at this resolution"
        );
    }
}
