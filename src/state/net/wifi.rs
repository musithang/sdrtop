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

/// The latest 802.11b frame whose SYNC was found and SFD read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WifiDsss {
    pub preamble: crate::signal::wifi::dsss::header::Preamble,
    /// What its header said, or why it was refused.
    pub header:
        Result<crate::signal::wifi::dsss::header::Header, crate::signal::wifi::dsss::header::Fault>,
    /// Its carrier against our tuning, from its SYNC.
    pub offset_hz: crate::signal::dsp::uncertainty::Uncertain,
    /// The channel centre it was heard on.
    pub carrier_hz: f64,
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

/// What a frame of this channel is, by the format its signalling named:
/// the classes every Wi-Fi view colours alike. A frame of another channel
/// is none of them; it is counted, never given airtime here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    NonHt,
    Ht,
    Vht,
    He,
    Eht,
    Dsss,
    /// Signalling that did not read, or a PHY past EHT, whose airtime its
    /// L-SIG still stated.
    Undecoded,
}

impl Family {
    /// Every family, in the order a column stacks them.
    pub const ALL: [Family; 7] = [
        Family::NonHt,
        Family::Ht,
        Family::Vht,
        Family::He,
        Family::Eht,
        Family::Dsss,
        Family::Undecoded,
    ];

    /// The name the views give it.
    pub fn label(self) -> &'static str {
        match self {
            Family::NonHt => "non-HT",
            Family::Ht => "HT",
            Family::Vht => "VHT",
            Family::He => "HE",
            Family::Eht => "EHT",
            Family::Dsss => "DSSS",
            Family::Undecoded => "not decoded",
        }
    }
}

/// One visit the survey paid a channel: how long it listened, how busy its
/// busiest megahertz was, the share of the visit the frames read on its
/// centre stated as airtime (by family, in [`Family::ALL`]'s order), and how
/// many frames were read there and heard there from other channels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Visit {
    pub seconds: f64,
    pub busy: f64,
    pub airtime: [f64; 7],
    pub read: u32,
    pub other: u32,
}

/// The fewest visits whose spread is a spread: with one or two, the
/// standard error of their mean says nothing anyone should lean on.
pub const MIN_VISITS: usize = 3;

/// The channels the survey visits: 1 to 13.
pub const SURVEYED: usize = 13;

/// A channel's busy share over its visits: their mean, and the standard
/// error of that mean once there are [`MIN_VISITS`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Busy {
    pub mean: f64,
    pub sigma: Option<f64>,
    pub visits: usize,
    pub seconds: f64,
}

/// Which channel is least busy, or why none can be named yet.
#[derive(Clone, Debug, PartialEq)]
pub enum LeastBusy {
    /// Not every channel has been visited.
    FirstPass { visited: usize },
    /// Every channel visited, but some fewer than [`MIN_VISITS`] times, so
    /// no spread to compare against; `passes` more give every one its three.
    NoSpreadYet { passes: usize },
    /// One channel, apart from every other by more than 2 sigma.
    Alone(u8),
    /// The quietest and those within 2 sigma of it, in channel order.
    Few(Vec<u8>),
    /// Every channel within 2 sigma of the quietest. `passes`: how many more
    /// would set the first one apart, at the spread a visit has now; `None`
    /// when all read the same and no number of passes would.
    NotApart { passes: Option<usize> },
}

/// The survey's visits, channel by channel, since the view opened or was
/// started again.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WifiSurvey {
    /// Channel n's visits at index n - 1.
    pub visits: [Vec<Visit>; SURVEYED],
    /// The channel the view has selected, when focused.
    pub selected: Option<u8>,
}

impl WifiSurvey {
    fn of(&self, ch: u8) -> &[Visit] {
        usize::from(ch)
            .checked_sub(1)
            .and_then(|i| self.visits.get(i))
            .map_or(&[], |v| v.as_slice())
    }

    /// Channel `ch`'s busy share, `None` before its first visit.
    pub fn busy(&self, ch: u8) -> Option<Busy> {
        let v = self.of(ch);
        if v.is_empty() {
            return None;
        }
        let n = v.len() as f64;
        let mean = v.iter().map(|x| x.busy).sum::<f64>() / n;
        let sigma = (v.len() >= MIN_VISITS).then(|| {
            let var = v.iter().map(|x| (x.busy - mean).powi(2)).sum::<f64>() / (n - 1.0);
            (var / n).sqrt()
        });
        Some(Busy {
            mean,
            sigma,
            visits: v.len(),
            seconds: v.iter().map(|x| x.seconds).sum(),
        })
    }

    /// The share of channel `ch`'s time its frames stated, by family, over
    /// its visits.
    pub fn airtime(&self, ch: u8) -> [f64; 7] {
        let v = self.of(ch);
        let mut out = [0.0; 7];
        for x in v {
            for (o, a) in out.iter_mut().zip(x.airtime) {
                *o += a / v.len() as f64;
            }
        }
        out
    }

    /// The least busy channel by the plain 2 sigma rule: the quietest, and
    /// every channel whose mean is within twice their combined standard
    /// error of it. No other threshold.
    pub fn least_busy(&self) -> LeastBusy {
        let channels = 1..=SURVEYED as u8;
        let busy: Vec<Option<Busy>> = channels.clone().map(|c| self.busy(c)).collect();
        let visited = busy.iter().flatten().count();
        if visited < SURVEYED {
            return LeastBusy::FirstPass { visited };
        }
        let busy: Vec<Busy> = busy.into_iter().flatten().collect();
        let fewest = busy.iter().map(|b| b.visits).min().unwrap_or(0);
        if fewest < MIN_VISITS {
            return LeastBusy::NoSpreadYet {
                passes: MIN_VISITS - fewest,
            };
        }
        let sigma = |b: &Busy| b.sigma.unwrap_or(0.0);
        let q = busy
            .iter()
            .min_by(|a, b| a.mean.total_cmp(&b.mean))
            .copied()
            .unwrap_or(busy[0]);
        let tied: Vec<u8> = channels
            .zip(&busy)
            .filter(|(_, b)| b.mean - q.mean <= 2.0 * sigma(b).hypot(sigma(&q)))
            .map(|(c, _)| c)
            .collect();
        match tied.len() {
            1 => LeastBusy::Alone(tied[0]),
            n if n < SURVEYED => LeastBusy::Few(tied),
            _ => {
                // The spread a visit has stays; the standard error shrinks
                // as one over the root of the visits. Apart once
                // 2 sqrt((s_c^2 + s_q^2) / n) falls below the difference.
                let per_visit = |b: &Busy| sigma(b) * (b.visits as f64).sqrt();
                let passes = busy
                    .iter()
                    .filter(|b| b.mean > q.mean)
                    .map(|b| {
                        let d = b.mean - q.mean;
                        let need = (4.0 * (per_visit(b).powi(2) + per_visit(&q).powi(2)) / (d * d))
                            .floor() as usize
                            + 1;
                        need.saturating_sub(b.visits.min(q.visits)).max(1)
                    })
                    .min();
                LeastBusy::NotApart { passes }
            }
        }
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
    /// The latest 802.11b frame read.
    pub last_dsss: Option<WifiDsss>,
    /// The survey's visits, channel by channel.
    pub survey: WifiSurvey,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::dsp::uncertainty::Uncertain;

    /// The busiest megahertz of each channel at each of its six visits, as
    /// the survey recorded on 2026-10-14 heard them (channel 1 first).
    const RECORDED: [[f64; 6]; 13] = [
        [0.0005, 0.0, 0.0, 0.0, 0.0, 0.0],
        [0.0005, 0.0, 0.0010, 0.0, 0.0, 0.0],
        [0.0, 0.0004, 0.0005, 0.0, 0.0, 0.0],
        [0.0013, 0.0115, 0.0008, 0.0, 0.0, 0.0],
        [0.0004, 0.0664, 0.0, 0.0, 0.0, 0.0],
        [0.0, 0.0648, 0.0, 0.0, 0.0, 0.0],
        [0.0, 0.0993, 0.0, 0.0, 0.0, 0.0],
        [0.0, 0.1051, 0.0, 0.0, 0.0002, 0.0],
        [0.0196, 0.6634, 0.0195, 0.0284, 0.0209, 0.0267],
        [0.0191, 0.7678, 0.0197, 0.0379, 0.0203, 0.6827],
        [0.0196, 0.5527, 0.0247, 0.0291, 0.0242, 0.5197],
        [0.0396, 0.2029, 0.0265, 0.0191, 0.0253, 0.5636],
        [0.0242, 0.0016, 0.0245, 0.0196, 0.0395, 0.7191],
    ];

    /// The first `n` visits of every channel, with `busy` from `table`.
    fn survey(table: &[[f64; 6]; 13], n: usize) -> WifiSurvey {
        let mut s = WifiSurvey::default();
        for (ch, row) in table.iter().enumerate() {
            s.visits[ch] = row[..n]
                .iter()
                .map(|b| Visit {
                    seconds: 0.475,
                    busy: *b,
                    ..Default::default()
                })
                .collect();
        }
        s
    }

    /// A channel's busy is its visits' mean; its spread, the standard error
    /// of the mean, only from the third visit on.
    #[test]
    fn busy_is_the_mean_of_the_visits_and_its_spread_comes_at_three() {
        let six = survey(&RECORDED, 6).busy(10).unwrap();
        assert!((six.mean - 0.2579).abs() < 1e-4, "{six:?}");
        assert!((six.sigma.unwrap() - 0.1482).abs() < 1e-4, "{six:?}");
        assert_eq!((six.visits, (six.seconds * 1e3).round()), (6, 2850.0));
        assert_eq!(survey(&RECORDED, 2).busy(10).unwrap().sigma, None);
        assert!(survey(&RECORDED, 3).busy(10).unwrap().sigma.is_some());
        assert_eq!(WifiSurvey::default().busy(10), None);
        // Six visits of nothing: a mean of nought and no spread about it.
        let quiet = survey(&[[0.0; 6]; 13], 6).busy(3).unwrap();
        assert_eq!((quiet.mean, quiet.sigma), (0.0, Some(0.0)));
    }

    /// **The recorded survey names no channel, and says when one would
    /// be.** Six visits of bursty traffic: even channel 10, 25.8 % plus or
    /// minus 14.8, is within 2 sigma of the quietest, and at the spread a
    /// visit has, eight visits would set it apart: two more passes.
    #[test]
    fn the_recorded_survey_tells_no_channel_apart_yet() {
        assert_eq!(
            survey(&RECORDED, 6).least_busy(),
            LeastBusy::NotApart { passes: Some(2) }
        );
    }

    /// The verdicts in turn: a channel not yet visited, a spread not yet
    /// had, one channel apart, a few together, all the same.
    #[test]
    fn the_least_busy_is_named_only_as_far_as_the_visits_carry() {
        let mut first = survey(&RECORDED, 1);
        first.visits[12].clear();
        assert_eq!(first.least_busy(), LeastBusy::FirstPass { visited: 12 });
        assert_eq!(
            survey(&RECORDED, 2).least_busy(),
            LeastBusy::NoSpreadYet { passes: 1 }
        );
        // Steady channels: 6 quiet and alone, then 1 and 11 as quiet.
        let mut steady = [[0.3, 0.32, 0.28, 0.3, 0.31, 0.29]; 13];
        steady[5] = [0.01, 0.012, 0.008, 0.01, 0.011, 0.009];
        assert_eq!(survey(&steady, 6).least_busy(), LeastBusy::Alone(6));
        steady[0] = steady[5];
        steady[10] = [0.012, 0.01, 0.009, 0.011, 0.01, 0.008];
        assert_eq!(
            survey(&steady, 6).least_busy(),
            LeastBusy::Few(vec![1, 6, 11])
        );
        assert_eq!(
            survey(&[[0.0; 6]; 13], 6).least_busy(),
            LeastBusy::NotApart { passes: None }
        );
    }

    /// The airtime by family is the visits' mean share.
    #[test]
    fn airtime_is_the_mean_share_of_the_visits() {
        let mut s = WifiSurvey::default();
        let mut a = Visit::default();
        a.airtime[5] = 0.2;
        let mut b = Visit::default();
        b.airtime[5] = 0.1;
        b.airtime[1] = 0.04;
        s.visits[10] = vec![a, b];
        let at = s.airtime(11);
        assert!((at[5] - 0.15).abs() < 1e-12 && (at[1] - 0.02).abs() < 1e-12);
        assert_eq!(s.airtime(1), [0.0; 7]);
    }

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
