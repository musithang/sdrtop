// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Wi-Fi's receiver, as the worker last left it: where it runs, or why not.

/// The latest frame of this channel the receiver synchronised to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WifiFrame {
    /// Its offset from our tuning, with its uncertainty; our own
    /// oscillator's error is in it until a reference takes it out.
    pub offset_hz: crate::signal::dsp::uncertainty::Uncertain,
    /// The per-sample SNR its long training field gave, in dB.
    pub snr_db: f64,
    /// The channel centre it was heard on, which the offset is a fraction
    /// of.
    pub carrier_hz: f64,
    /// What the symbols after its long training field said: its L-SIG and
    /// its format.
    pub heard: crate::signal::wifi::format::Heard,
    pub when: std::time::Instant,
}

/// The latest frame of this channel whose symbol clock was read, beside
/// its carrier over the same DATA field: two readings of what the standard
/// requires to be one oscillator; and the preamble's carrier, which lies
/// apart from them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WifiClock {
    /// Its sample clock against ours, in ppm, positive when fast.
    pub clock_ppm: crate::signal::dsp::uncertainty::Uncertain,
    /// Its carrier over the DATA field against our tuning, in ppm of the
    /// channel centre.
    pub carrier_ppm: crate::signal::dsp::uncertainty::Uncertain,
    /// Its carrier over the preamble's long training field, likewise.
    pub preamble_ppm: crate::signal::dsp::uncertainty::Uncertain,
    /// The DATA symbols the clock was read over.
    pub symbols: usize,
    pub when: std::time::Instant,
}

/// Every clock read this session held against its carrier: the
/// inverse-variance sums of clock minus carrier, so the mean, its error
/// and how well the frames agree with one another can be said.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ClockCheck {
    pub frames: u64,
    sum_w: f64,
    sum_wd: f64,
    sum_wdd: f64,
}

impl ClockCheck {
    /// Count one frame's clock against its carrier.
    pub fn add(&mut self, clock: &WifiClock) {
        let d = clock.clock_ppm.value() - clock.carrier_ppm.value();
        let var = clock.clock_ppm.sigma().powi(2) + clock.carrier_ppm.sigma().powi(2);
        if !(var > 0.0 && var.is_finite() && d.is_finite()) {
            return;
        }
        let w = 1.0 / var;
        self.frames += 1;
        self.sum_w += w;
        self.sum_wd += w * d;
        self.sum_wdd += w * d * d;
    }

    /// The weighted mean of clock minus carrier, in ppm, with its error.
    pub fn mean(&self) -> Option<crate::signal::dsp::uncertainty::Uncertain> {
        (self.frames > 0).then(|| {
            crate::signal::dsp::uncertainty::Uncertain::from_variance(
                self.sum_wd / self.sum_w,
                1.0 / self.sum_w,
            )
        })
    }

    /// Chi-square per degree of freedom of the differences about their
    /// mean: near one when every frame agrees within its own uncertainty,
    /// well above when some transmitter's clocks are not one.
    pub fn chi2_per_dof(&self) -> Option<f64> {
        (self.frames > 1).then(|| {
            (self.sum_wdd - self.sum_wd * self.sum_wd / self.sum_w) / (self.frames - 1) as f64
        })
    }
}

/// Where Wi-Fi's receiver runs, or why it does not, published by the
/// worker's Wi-Fi part (`signal::net::worker`'s `wifi`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WifiState {
    /// The Wi-Fi channel the receiver runs on now, when it runs.
    pub channel: Option<u8>,
    /// Why it does not run, when a Wi-Fi view is open and it does not: the
    /// rate, or a tuning off every channel centre. `None` off the Wi-Fi
    /// views, where not running is not a refusal.
    pub refused: Option<String>,
    /// Whether it has run at all this session. Decode health names Wi-Fi
    /// only then, as it does LE Coded: a line for a receiver this session
    /// never started would offer one the view on screen does not start.
    pub ran: bool,
    /// The latest frame of this channel, kept across a stand-down so the
    /// panel can still say what it last saw, dated.
    pub last_frame: Option<WifiFrame>,
    /// The latest frame whose clock was read, and its carrier.
    pub clock: Option<WifiClock>,
    /// Every clock read this session against its carrier.
    pub clock_check: ClockCheck,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::dsp::uncertainty::Uncertain;

    fn reading(clock: f64, carrier: f64) -> WifiClock {
        WifiClock {
            clock_ppm: Uncertain::from_sigma(clock, 1.0),
            carrier_ppm: Uncertain::from_sigma(carrier, 1.0),
            preamble_ppm: Uncertain::from_sigma(carrier + 2.0, 0.1),
            symbols: 100,
            when: std::time::Instant::now(),
        }
    }

    /// Frames that agree give a mean near their difference and a
    /// chi-square near or under one; one frame 20 sigma off lifts the
    /// chi-square far above, though the mean alone moves little.
    #[test]
    fn the_check_says_whether_the_frames_agree() {
        let mut c = ClockCheck::default();
        assert_eq!((c.mean(), c.chi2_per_dof()), (None, None));
        for d in [0.5, -0.5, 0.3, -0.3] {
            c.add(&reading(10.0 + d, 10.0));
        }
        let m = c.mean().unwrap();
        assert!(m.value().abs() < 1e-9 && (m.sigma() - (2.0f64 / 4.0).sqrt()).abs() < 1e-9);
        assert!(c.chi2_per_dof().unwrap() < 1.0);
        for _ in 0..20 {
            c.add(&reading(0.2, 0.0));
        }
        c.add(&reading(28.0, 0.0));
        assert!(c.chi2_per_dof().unwrap() > 10.0);
        assert_eq!(c.frames, 25);
    }
}
