// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! LE Coded's own receive chain, apart from LE 1M's.
//!
//! LE Coded sends the same 1 Msym/s GFSK as LE 1M, but spends every bit as
//! two or eight symbols so a receiver can hear it further away. A receiver
//! that throws decibels away throws that range away, so this chain is not
//! LE 1M's with a second detector bolted on: it has a channel filter chosen
//! for Coded's sensitivity ([`FILTER`]) and reads each symbol as the mean of
//! the discriminator over the symbol's own period ([`symbol_means`]) rather
//! than one sample of it. LE 1M's front end cannot change without moving
//! every LE 1M figure, and does not have to: the two chains share nothing
//! but the samples.

use crate::signal::dsp::fir::{design_lowpass_to_spec, StreamingDecimator};

/// The rate the chain decimates to: four samples a symbol, as LE 1M's.
pub const WORKING_RATE_HZ: f64 = 4e6;

/// Working samples a symbol at [`WORKING_RATE_HZ`].
const WORKING_SPS: usize = 4;

/// The channel filter's stopband, as LE 1M's: real rejection of the
/// neighbouring channels a wide capture carries, without the taps a deeper
/// one would cost.
const STOPBAND_DB: f64 = 40.0;

/// A channel filter, Hz: `cutoff_hz` is its -6 dB point (the windowed
/// sinc's cutoff, `fir::design_lowpass_to_spec`'s `fc`), and the transition
/// is centred on it, so the passband ends `transition_hz / 2` below it and
/// the stopband starts as far above.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChannelFilter {
    pub cutoff_hz: f64,
    pub transition_hz: f64,
}

#[allow(dead_code)]
impl ChannelFilter {
    /// Where the passband ends, Hz.
    pub fn passband_hz(self) -> f64 {
        self.cutoff_hz - self.transition_hz / 2.0
    }

    /// Where the stopband starts, Hz.
    pub fn stopband_hz(self) -> f64 {
        self.cutoff_hz + self.transition_hz / 2.0
    }
}

/// The filters the sensitivity bench weighs, narrowest first, to LE 1M's
/// (-6 dB at 1.5 MHz, 0.5 MHz transition) as the reference. A narrower passband needs a narrower transition to keep the
/// stopband where the neighbours are, and that costs taps: the bench's load
/// column says how many.
#[allow(dead_code)]
pub const CANDIDATES: [ChannelFilter; 7] = [
    ChannelFilter {
        cutoff_hz: 350e3,
        transition_hz: 300e3,
    },
    ChannelFilter {
        cutoff_hz: 400e3,
        transition_hz: 300e3,
    },
    ChannelFilter {
        cutoff_hz: 500e3,
        transition_hz: 300e3,
    },
    ChannelFilter {
        cutoff_hz: 650e3,
        transition_hz: 300e3,
    },
    ChannelFilter {
        cutoff_hz: 800e3,
        transition_hz: 300e3,
    },
    ChannelFilter {
        cutoff_hz: 1_000e3,
        transition_hz: 400e3,
    },
    ChannelFilter {
        cutoff_hz: 1_500e3,
        transition_hz: 500e3,
    },
];

/// The chain's channel filter: -6 dB at 500 kHz, a 300 kHz transition, so a
/// passband to 350 kHz and a stopband from 650 kHz.
///
/// **Chosen on the sensitivity bench** (`coded_bench::coded_filter_bench`: a
/// 50-octet PDU, 150 packets a point, crystal offsets to ±50 ppm, each
/// symbol's mean, soft decisions), where packet error rate reaches the
/// Core's 30.8 % at, in Eb/N0:
///
/// | -6 dB at | 350 kHz | 400 | **500** | 650 | 800 | 1000 | 1500, LE 1M's |
/// |---|---|---|---|---|---|---|---|
/// | S=8 | 12.8 dB | 12.0 | **11.9** | 12.6 | 13.7 | 14.5 | 16.1 |
/// | S=2 | never | 14.1 | **11.2** | 11.2 | 12.0 | 12.7 | 14.4 |
///
/// 4.2 dB better than LE 1M's filter at S=8 and 3.1 at S=2. Narrower is
/// worse: at 400 kHz S=2 already has a floor (a fifth of its packets lost
/// at any Eb/N0 the bench reached), the filter cutting into the signal, so
/// 500 kHz sits about 100 kHz from that edge. Its 151 taps cost about half a
/// millisecond per millisecond of 20 Msps signal on the i3, against 0.33 for
/// LE 1M's 91; the chain runs only on the LE Coded view.
#[allow(dead_code)]
pub const FILTER: ChannelFilter = CANDIDATES[2];

/// The chain's front end from `raw_rate` to [`WORKING_RATE_HZ`] through
/// `filter`, or why it cannot be built: a raw rate below the working rate,
/// or not a whole multiple of it, would scale every timing and deviation by
/// the mismatch with nothing on screen to say so, so it is refused as LE
/// 1M's front end refuses it.
#[allow(dead_code)]
pub fn front_end(raw_rate: f64, filter: ChannelFilter) -> Result<StreamingDecimator, String> {
    if raw_rate < WORKING_RATE_HZ {
        return Err(format!(
            "LE Coded decode needs at least {:.1} Msps; the radio is at {:.3} Msps",
            WORKING_RATE_HZ / 1e6,
            raw_rate / 1e6
        ));
    }
    let d = (raw_rate / WORKING_RATE_HZ).round().max(1.0) as usize;
    if (raw_rate / d as f64 - WORKING_RATE_HZ).abs() > WORKING_RATE_HZ * 0.01 {
        return Err(format!(
            "LE Coded decode needs a sample rate near a whole multiple of {:.1} Msps; {:.3} Msps is not one",
            WORKING_RATE_HZ / 1e6,
            raw_rate / 1e6
        ));
    }
    let taps = design_lowpass_to_spec(
        filter.cutoff_hz / raw_rate,
        filter.transition_hz / raw_rate,
        STOPBAND_DB,
    );
    Ok(StreamingDecimator::new(taps, d))
}

/// One reading a symbol: the mean of the discriminator `track` over the
/// four working samples of each symbol's own period, the first symbol
/// centred at `first_centre` (a sample index, fractional, as a phase search
/// gives it), up to `count` symbols or as many whole ones as `track` holds.
///
/// **A mean, not a sample.** The discriminator's noise is broadband next to
/// one symbol's worth of signal; averaging the symbol's own four samples
/// keeps the signal and halves the noise's amplitude, which the sensitivity
/// bench measured at 3 to 4 dB on packet error rate against reading the
/// centre alone.
#[allow(dead_code)]
pub fn symbol_means(track: &[f32], first_centre: f64, count: usize) -> Vec<f32> {
    let half = (WORKING_SPS as f64 - 1.0) / 2.0;
    (0..count)
        .map_while(|i| {
            let start = (first_centre + (i * WORKING_SPS) as f64 - half).round();
            if start < 0.0 {
                return None;
            }
            let start = start as usize;
            let period = track.get(start..start + WORKING_SPS)?;
            Some(period.iter().sum::<f32>() / WORKING_SPS as f32)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use num_complex::Complex;

    /// Mean power of `x` once the filter has settled (its first and last
    /// tenth dropped).
    fn settled_power(x: &[Complex<f32>]) -> f64 {
        let skip = x.len() / 10;
        let mid = &x[skip..x.len() - skip];
        mid.iter().map(|s| s.norm_sqr() as f64).sum::<f64>() / mid.len() as f64
    }

    fn tone(freq_hz: f64, rate: f64, n: usize) -> Vec<Complex<f32>> {
        (0..n)
            .map(|i| {
                Complex::from_polar(
                    1.0,
                    (std::f64::consts::TAU * freq_hz * i as f64 / rate) as f32,
                )
            })
            .collect()
    }

    /// Every candidate passes a tone inside its passband and stops one in
    /// its stopband: the filters are what their numbers say.
    #[test]
    fn a_tone_inside_the_passband_passes_and_one_outside_is_stopped() {
        for filter in CANDIDATES {
            let inside = 0.9 * filter.passband_hz();
            let outside = filter.stopband_hz() + 50e3;
            let mut out = Vec::new();
            front_end(20e6, filter)
                .unwrap()
                .process(&tone(inside, 20e6, 200_000), &mut out);
            let pass_db = 10.0 * settled_power(&out).log10();
            assert!(
                pass_db.abs() < 1.0,
                "{filter:?}: {pass_db:.2} dB in the passband"
            );
            front_end(20e6, filter)
                .unwrap()
                .process(&tone(outside, 20e6, 200_000), &mut out);
            let stop_db = 10.0 * settled_power(&out).log10();
            assert!(
                stop_db < -30.0,
                "{filter:?}: {stop_db:.2} dB in the stopband"
            );
        }
    }

    /// Four samples a symbol, each symbol's own four averaged: on a track of
    /// four highs then four lows the readings alternate exactly; half a
    /// symbol late, each reading straddles two symbols and says so.
    #[test]
    fn symbol_means_average_each_symbols_own_period() {
        let track: Vec<f32> = (0..64)
            .map(|i| if (i / 4) % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        let on_time = symbol_means(&track, 1.5, 6);
        assert_eq!(on_time, vec![1.0, -1.0, 1.0, -1.0, 1.0, -1.0]);
        let late = symbol_means(&track, 2.5, 4);
        assert_eq!(late, vec![0.5, -0.5, 0.5, -0.5]);
    }

    /// As many readings as the track holds whole symbols for, no more.
    #[test]
    fn symbol_means_stop_at_the_end_of_the_track() {
        let track = vec![1.0f32; 10];
        assert_eq!(symbol_means(&track, 1.5, 100).len(), 2);
    }

    #[test]
    fn a_rate_that_is_not_a_working_rate_multiple_is_refused() {
        assert!(front_end(10.5e6, FILTER).is_err());
        assert!(front_end(2e6, FILTER).is_err());
        assert!(front_end(20e6, FILTER).is_ok());
    }
}
