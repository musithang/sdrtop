// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! A device's advertising interval and its random delay, from when its
//! packets arrived on one channel (Bluetooth design measurement 9,
//! net-ux-polish-plan 4.5).
//!
//! **What the specification says, and where it was checked.** Core Vol 6
//! Part B 4.4.2.2: advInterval is an integer multiple of 0.625 ms from 20 ms
//! to 10.24 s, and each advertising event starts advInterval plus advDelay
//! after the last, advDelay a pseudo-random 0 to 10 ms drawn by the Link Layer
//! for every event (`T_advEvent = advInterval + advDelay`), for undirected and
//! low-duty-cycle directed advertising. Confirmed against public
//! documentation of that section, not a licensed copy read this session.
//!
//! **One channel, one packet an event.** An event sends on 37, 38 and 39 in
//! turn; a receiver locked to one of them hears at most one packet of each,
//! so the gap between two consecutive arrivals is `advInterval + advDelay`
//! when no event was missed, and `m` intervals plus `m` delays when `m - 1`
//! were. Only single-event gaps are used: they lie in `[I, I + 10 ms]`, and
//! the next cluster starts at `2I`, at least 20 ms later, so every gap within
//! 10 ms of the smallest is a single event and none of the others is. That
//! fails in one way, stated rather than hidden: a device whose every other
//! event is lost reads as twice its interval.
//!
//! **The estimate is of a uniform distribution's edges.** Single-event gaps
//! are `I + d` with `d` uniform, so the smallest gap sits a little above `I`
//! and the largest a little below `I + W`. The minimum-variance unbiased
//! estimates of the two edges (`(n·min - max)/(n - 1)` and its mirror) give
//! the interval and the delay's width; the width is what shows a device with
//! no random delay at all, which measurement 9 set out to catch, and a
//! Kolmogorov-Smirnov test of the gaps against the fitted uniform says
//! whether the delay looks like the random draw the specification asks for.
//!
//! **Only meaningful in LOCK**, and the caller holds that line: while
//! surveying, the dwell schedule decides which arrivals are heard, and gaps
//! between them measure the survey, not the device.
//!
//! The timebase is the radio's own sample clock (`pdu::Packet::at_pair`),
//! good to a symbol once running and to about 5 µs on a receiver's first
//! packet (`receive`'s own test). The device's advertising is timed by its
//! sleep clock, which may be off by hundreds of ppm, so a measured interval is
//! the device's interval in our seconds; that is a real reading of it, and
//! the grid comparison below allows for it rather than correcting it.

use crate::signal::dsp::uncertainty::Uncertain;

/// The advInterval's step, and its legacy range, in seconds.
pub const INTERVAL_STEP_S: f64 = 0.625e-3;
pub const INTERVAL_MIN_S: f64 = 20e-3;
pub const INTERVAL_MAX_S: f64 = 10.24;

/// The widest advDelay the specification allows, in seconds.
pub const ADV_DELAY_MAX_S: f64 = 10e-3;

/// Single-event gaps needed before anything is said: fewer, and the edges of
/// a uniform are too loosely pinned for the width to tell "random" from
/// "fixed", or the test to mean anything.
pub const MIN_EVENTS: usize = 8;

/// A delay narrower than this is no random delay at all: the timebase's own
/// few microseconds, and a sleep clock's drift across the capture, stay well
/// under it, while a real draw over 0 to 10 ms is wider in all but a vanishing
/// share of runs of [`MIN_EVENTS`].
const NO_DELAY_S: f64 = 0.3e-3;

/// How far a device's advertising clock may be from ours, as a fraction: the
/// sleep clock accuracy a Link Layer may declare runs to 500 ppm. Scales how
/// close to the 0.625 ms grid an interval must land to be called on it.
const SLEEP_CLOCK_PPM: f64 = 500.0;

/// The timebase's floor on the interval's uncertainty: a symbol, and the
/// cold-start offset `receive` measures.
const TIMING_FLOOR_S: f64 = 5e-6;

/// Why no estimate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Refusal {
    /// Not enough single-event gaps yet.
    Collecting { have: usize, need: usize },
    /// Packets arrive faster than any legacy advertising interval allows: a
    /// high-duty-cycle directed advertiser, or something other than the
    /// periodic advertising this model describes. The smallest gap, in s.
    BelowMinimum(f64),
    /// Every gap is longer than the longest legacy interval plus its delay:
    /// no two consecutive events were heard, so no gap is a single one. The
    /// smallest gap, in s.
    NoSingleEvents(f64),
}

/// What the delay looks like.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Delay {
    /// Narrower than [`NO_DELAY_S`]: the device adds no random delay.
    Absent,
    /// A width (s), and whether the gaps fit a uniform draw over it, with the
    /// test's statistic and its 5 % critical value.
    Spread {
        width_s: f64,
        uniform: bool,
        ks: f64,
        critical: f64,
    },
}

/// Where the interval sits against the specification's 0.625 ms grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Grid {
    /// Within what the measurement and the clocks allow of `n` steps.
    On(u32),
    /// Off the nearest `n` steps by this much (s), further than they allow.
    Off { n: u32, by_s: f64 },
    /// The allowance is half a step or more: the interval is too long, or
    /// too loosely known, for the grid to be told from a sleep clock's drift.
    CannotTell,
}

/// A device's advertising timing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Estimate {
    /// advInterval, in our seconds.
    pub interval_s: Uncertain,
    /// Single-event gaps it rests on.
    pub events: usize,
    pub delay: Delay,
    pub grid: Grid,
}

/// The advertising timing from `pairs`, the stream positions of one device's
/// periodic advertising packets on one channel, in the order they arrived,
/// at `rate_hz`.
pub fn estimate(pairs: &[u64], rate_hz: f64) -> Result<Estimate, Refusal> {
    let gaps: Vec<f64> = pairs
        .windows(2)
        .filter(|w| w[1] > w[0])
        .map(|w| (w[1] - w[0]) as f64 / rate_hz)
        .collect();
    let smallest = gaps.iter().copied().fold(f64::INFINITY, f64::min);
    let mut single: Vec<f64> = gaps
        .iter()
        .copied()
        .filter(|g| *g <= smallest + ADV_DELAY_MAX_S)
        .collect();
    if single.len() < MIN_EVENTS {
        return Err(Refusal::Collecting {
            have: single.len(),
            need: MIN_EVENTS,
        });
    }
    if smallest < INTERVAL_MIN_S {
        return Err(Refusal::BelowMinimum(smallest));
    }
    if smallest > INTERVAL_MAX_S + ADV_DELAY_MAX_S {
        return Err(Refusal::NoSingleEvents(smallest));
    }
    single.sort_by(f64::total_cmp);

    let n = single.len() as f64;
    let (lo, hi) = (single[0], single[single.len() - 1]);
    let a = (n * lo - hi) / (n - 1.0);
    let b = (n * hi - lo) / (n - 1.0);
    let width = (b - a).max(0.0);
    // The spread of the minimum of n uniform draws over the width.
    let sigma = (width * (n / ((n + 1.0).powi(2) * (n + 2.0))).sqrt()).max(TIMING_FLOOR_S);
    let interval = Uncertain::from_sigma(a, sigma);

    let delay = if width < NO_DELAY_S {
        Delay::Absent
    } else {
        let ks = single
            .iter()
            .enumerate()
            .map(|(i, g)| {
                let u = ((g - a) / width).clamp(0.0, 1.0);
                let (below, above) = (i as f64 / n, (i + 1) as f64 / n);
                (u - below).abs().max((above - u).abs())
            })
            .fold(0.0, f64::max);
        let critical = 1.36 / n.sqrt();
        Delay::Spread {
            width_s: width,
            uniform: ks <= critical,
            ks,
            critical,
        }
    };

    let steps = (a / INTERVAL_STEP_S).round();
    let allowance = 2.0 * sigma + a * SLEEP_CLOCK_PPM * 1e-6;
    let grid = if allowance >= INTERVAL_STEP_S / 2.0 {
        Grid::CannotTell
    } else if (a - steps * INTERVAL_STEP_S).abs() <= allowance {
        Grid::On(steps as u32)
    } else {
        Grid::Off {
            n: steps as u32,
            by_s: a - steps * INTERVAL_STEP_S,
        }
    };

    Ok(Estimate {
        interval_s: interval,
        events: single.len(),
        delay,
        grid,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::dsp::testkit::Rng;

    const RATE: f64 = 8e6;

    /// Arrival positions for `events` advertising events of `interval_s`,
    /// each delayed by `delay(rng)`, every `drop`-th one not heard.
    fn arrivals(
        interval_s: f64,
        events: usize,
        drop: usize,
        mut delay: impl FnMut(&mut Rng) -> f64,
    ) -> Vec<u64> {
        let mut rng = Rng::new(7);
        let mut t = 1.0;
        let mut out = Vec::new();
        for k in 0..events {
            t += interval_s + delay(&mut rng);
            if drop == 0 || k % drop != 0 {
                out.push((t * RATE) as u64);
            }
        }
        out
    }

    /// **A device advertising every 100 ms with the specification's random
    /// delay**, one event in five missed: the interval to a fraction of a
    /// millisecond and within its own uncertainty, the delay about 10 ms wide
    /// and uniform, and 160 steps of 0.625 ms.
    #[test]
    fn a_compliant_advertiser_is_read_back() {
        let got = estimate(
            &arrivals(0.100, 300, 5, |r| r.unit() * ADV_DELAY_MAX_S),
            RATE,
        )
        .unwrap();
        let i = got.interval_s;
        assert!((i.value() - 0.100).abs() < 3.0 * i.sigma(), "{got:?}");
        assert!((i.value() - 0.100).abs() < 0.2e-3, "{got:?}");
        match got.delay {
            Delay::Spread {
                width_s, uniform, ..
            } => {
                assert!((width_s - ADV_DELAY_MAX_S).abs() < 0.5e-3, "{got:?}");
                assert!(uniform, "{got:?}");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(got.grid, Grid::On(160));
        // Only single events: the missed ones' double gaps are not in it.
        assert!(got.events < 300 && got.events > 150, "{got:?}");
    }

    /// **No random delay is caught**, which is what measurement 9 was for.
    #[test]
    fn a_device_without_the_random_delay_is_seen_to_have_none() {
        let got = estimate(&arrivals(0.050, 60, 0, |_| 0.0), RATE).unwrap();
        assert_eq!(got.delay, Delay::Absent, "{got:?}");
        assert_eq!(got.grid, Grid::On(80));
    }

    /// A delay that is there but is not a uniform draw (two values only) is
    /// told apart from the random one.
    #[test]
    fn a_delay_that_is_not_random_is_not_called_uniform() {
        let got = estimate(
            &arrivals(0.100, 200, 0, |r| if r.unit() < 0.5 { 0.0 } else { 9e-3 }),
            RATE,
        )
        .unwrap();
        match got.delay {
            Delay::Spread {
                uniform,
                ks,
                critical,
                ..
            } => {
                assert!(!uniform, "{ks} against {critical}")
            }
            other => panic!("{other:?}"),
        }
    }

    /// An interval between the 0.625 ms steps is called off the grid by how
    /// much; one too long to be told from a sleep clock's drift is not called
    /// either way.
    #[test]
    fn the_grid_is_judged_only_where_the_clocks_allow() {
        let off = estimate(&arrivals(0.1003, 400, 0, |_| 0.0), RATE).unwrap();
        match off.grid {
            Grid::Off { n, by_s } => {
                assert_eq!(n, 160);
                assert!((by_s - 0.3e-3).abs() < 0.05e-3, "{off:?}");
            }
            other => panic!("{other:?}"),
        }
        let long = estimate(&arrivals(1.28, 20, 0, |_| 0.0), RATE).unwrap();
        assert_eq!(long.grid, Grid::CannotTell, "{long:?}");
    }

    /// Too few events is said as such, with how many there are; packets
    /// faster than any legacy interval are refused rather than fitted.
    #[test]
    fn what_cannot_be_estimated_is_refused_with_its_reason() {
        let few = estimate(&arrivals(0.1, 5, 0, |_| 0.0), RATE);
        assert_eq!(
            few,
            Err(Refusal::Collecting {
                have: 4,
                need: MIN_EVENTS
            })
        );
        let fast = estimate(&arrivals(3.75e-3, 50, 0, |_| 0.0), RATE);
        assert!(matches!(fast, Err(Refusal::BelowMinimum(_))), "{fast:?}");
        let slow = estimate(&arrivals(11.0, 20, 0, |_| 0.0), RATE);
        assert!(matches!(slow, Err(Refusal::NoSingleEvents(_))), "{slow:?}");
        assert_eq!(
            estimate(&[], RATE),
            Err(Refusal::Collecting {
                have: 0,
                need: MIN_EVENTS
            })
        );
    }
}
