//! Channel › Airtime's numbers, as pure functions of the state, so its
//! panels only draw: the hero's columns and the card's reading from the
//! survey's visits on the locked channel.

use super::wifi::WifiSurvey;
use std::time::{Duration, Instant};

/// How long a hero column covers: a second, two of the survey's
/// half-second visits on a locked channel.
pub const AIR_COLUMN: Duration = Duration::from_secs(1);

/// The share of its span a column must have heard to be read: under it, a
/// gap in the feed would decide what the column says.
pub const HEARD_WHOLE: f64 = 0.9;

/// The columns the reading is over: twenty seconds, fewer than the hero
/// shows at any size, so every column read is one on screen.
pub const READING_COLUMNS: usize = 20;

/// The fewest columns whose spread is a spread, as for the survey's
/// visits.
pub const MIN_COLUMNS: usize = super::wifi::MIN_VISITS;

/// One column of the hero: the visits to a channel that ended in one
/// [`AIR_COLUMN`], their busy share and their frames' airtime by family
/// (in [`super::Family::ALL`]'s order) weighted by the seconds each heard,
/// the seconds heard and the span they covered.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AirColumn {
    pub busy: f64,
    pub airtime: [f64; 7],
    pub seconds: f64,
    pub span_s: f64,
    pub visits: usize,
}

impl AirColumn {
    /// The share of the column its frames stated, every family together.
    pub fn frames(&self) -> f64 {
        self.airtime.iter().sum()
    }

    /// Whether the column was heard whole: a visit ended in it, and its
    /// visits heard at least [`HEARD_WHOLE`] of their span.
    pub fn whole(&self) -> bool {
        self.visits > 0 && self.seconds >= HEARD_WHOLE * self.span_s
    }
}

/// A mean over the whole columns, and the standard error of it once there
/// are [`MIN_COLUMNS`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AirReading {
    pub mean: f64,
    pub sigma: Option<f64>,
}

impl AirReading {
    fn of(x: &[f64]) -> Self {
        let n = x.len() as f64;
        let mean = x.iter().sum::<f64>() / n;
        let sigma = (x.len() >= MIN_COLUMNS).then(|| {
            let var = x.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1.0);
            (var / n).sqrt()
        });
        AirReading { mean, sigma }
    }
}

/// The card's reading: the frames' share of the time and the energy's busy
/// share over the whole columns, the frames' share by family, how many
/// columns were read, and the seconds heard of the span every column with a
/// visit covered, the ones a gap cut into included.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AirShare {
    pub frames: AirReading,
    pub busy: AirReading,
    pub airtime: [f64; 7],
    pub columns: usize,
    pub heard_s: f64,
    pub span_s: f64,
}

impl WifiSurvey {
    /// Channel `ch`'s last `n` columns before `now`, the oldest first. A
    /// visit that does not say when it ended is in none.
    pub fn columns(&self, ch: u8, now: Instant, n: usize) -> Vec<AirColumn> {
        let mut out = vec![AirColumn::default(); n];
        for v in self.of(ch) {
            let Some(ended) = v.ended else { continue };
            let back = (now.saturating_duration_since(ended).as_secs_f64()
                / AIR_COLUMN.as_secs_f64()) as usize;
            let Some(c) = n.checked_sub(back + 1).and_then(|i| out.get_mut(i)) else {
                continue;
            };
            c.busy += v.busy * v.seconds;
            for (a, va) in c.airtime.iter_mut().zip(v.airtime) {
                *a += va * v.seconds;
            }
            c.seconds += v.seconds;
            c.span_s += v.span_s;
            c.visits += 1;
        }
        for c in out.iter_mut().filter(|c| c.seconds > 0.0) {
            c.busy /= c.seconds;
            for a in c.airtime.iter_mut() {
                *a /= c.seconds;
            }
        }
        out
    }

    /// Channel `ch`'s reading over its last `n` columns before `now`;
    /// `None` until one of them was heard whole.
    pub fn share(&self, ch: u8, now: Instant, n: usize) -> Option<AirShare> {
        let cols = self.columns(ch, now, n);
        let whole: Vec<&AirColumn> = cols.iter().filter(|c| c.whole()).collect();
        if whole.is_empty() {
            return None;
        }
        let frames: Vec<f64> = whole.iter().map(|c| c.frames()).collect();
        let busy: Vec<f64> = whole.iter().map(|c| c.busy).collect();
        let mut airtime = [0.0; 7];
        for c in &whole {
            for (a, ca) in airtime.iter_mut().zip(c.airtime) {
                *a += ca / whole.len() as f64;
            }
        }
        Some(AirShare {
            frames: AirReading::of(&frames),
            busy: AirReading::of(&busy),
            airtime,
            columns: whole.len(),
            heard_s: cols.iter().map(|c| c.seconds).sum(),
            span_s: cols.iter().map(|c| c.span_s).sum(),
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::state::{Visit, WifiSurvey};
    use std::time::{Duration, Instant};

    /// A visit to channel 6 that heard `seconds` of a `span_s` span, ended
    /// `ago` before `now`, `busy` and with 802.11b `air` of its time.
    fn visit(now: Instant, ago: f64, seconds: f64, span_s: f64, busy: f64, air: f64) -> Visit {
        let mut airtime = [0.0; 7];
        airtime[5] = air;
        Visit {
            seconds,
            span_s,
            ended: Some(now - Duration::from_secs_f64(ago)),
            busy,
            airtime,
            ..Default::default()
        }
    }

    fn survey(visits: Vec<Visit>) -> WifiSurvey {
        let mut s = WifiSurvey::default();
        s.visits[5] = visits;
        s
    }

    /// The far side of now, so `now - ago` never runs before the clock.
    fn now() -> Instant {
        Instant::now() + Duration::from_secs(60)
    }

    /// **A column is the visits that ended in its second**, the newest
    /// last: two in the newest second weigh by the seconds each heard, one
    /// a second further back is the column before, and one older than the
    /// columns is in none.
    #[test]
    fn a_column_is_the_visits_that_ended_in_its_second() {
        let now = now();
        let s = survey(vec![
            visit(now, 10.0, 0.5, 0.5, 0.9, 0.9),
            visit(now, 1.5, 0.5, 0.5, 0.3, 0.1),
            visit(now, 0.7, 0.5, 0.5, 0.2, 0.2),
            visit(now, 0.2, 0.25, 0.25, 0.8, 0.5),
        ]);
        let c = s.columns(6, now, 5);
        assert_eq!(c.len(), 5);
        assert_eq!(
            c.iter().map(|c| c.visits).collect::<Vec<_>>(),
            [0, 0, 0, 1, 2]
        );
        let newest = c[4];
        assert!((newest.busy - 0.4).abs() < 1e-9, "{newest:?}");
        assert!((newest.frames() - 0.3).abs() < 1e-9, "{newest:?}");
        assert!((newest.seconds - 0.75).abs() < 1e-9, "{newest:?}");
        assert!((c[3].busy - 0.3).abs() < 1e-9, "{c:?}");
        // Another channel's visits are not this one's.
        assert!(s.columns(11, now, 5).iter().all(|c| c.visits == 0));
    }

    /// **A column heard under 90 % of its span is not whole**, and neither
    /// is one no visit ended in: a gap in the feed, or nothing heard, is
    /// not quiet air.
    #[test]
    fn a_column_not_heard_whole_says_so() {
        let now = now();
        let s = survey(vec![
            visit(now, 2.5, 0.5, 0.5, 0.1, 0.1),
            visit(now, 0.5, 0.4, 0.5, 0.1, 0.1),
        ]);
        let c = s.columns(6, now, 4);
        assert!(!c[0].whole(), "nothing ended in it: {c:?}");
        assert!(c[1].whole(), "{c:?}");
        assert!(!c[2].whole(), "{c:?}");
        assert!(!c[3].whole(), "a gap took a fifth of its span: {c:?}");
    }

    /// A visit that does not say when it ended is in no column: it cannot
    /// be placed in time, and is not guessed into one.
    #[test]
    fn a_visit_without_an_end_is_in_no_column() {
        let now = now();
        let mut v = visit(now, 0.2, 0.5, 0.5, 0.1, 0.1);
        v.ended = None;
        let s = survey(vec![v]);
        assert!(s.columns(6, now, 3).iter().all(|c| c.visits == 0));
    }

    /// **The reading is the whole columns' mean, its 1 sigma from three
    /// on**, the frames' share and the energy's each; a column a gap cut
    /// into is left out of it but counted in the time heard.
    #[test]
    fn the_reading_is_the_whole_columns_mean_and_spread() {
        let now = now();
        let s = survey(vec![
            visit(now, 3.5, 0.5, 0.5, 0.2, 0.1),
            visit(now, 2.5, 0.5, 0.5, 0.2, 0.2),
            visit(now, 1.5, 0.5, 0.5, 0.2, 0.3),
            visit(now, 0.5, 0.2, 0.5, 0.9, 0.9),
        ]);
        let r = s.share(6, now, 4).expect("three whole columns");
        assert_eq!(r.columns, 3);
        assert!((r.frames.mean - 0.2).abs() < 1e-9, "{r:?}");
        let sigma = r.frames.sigma.expect("three columns");
        assert!((sigma - (0.01f64 / 3.0).sqrt()).abs() < 1e-9, "{r:?}");
        assert!((r.busy.mean - 0.2).abs() < 1e-9, "{r:?}");
        assert!(r.busy.sigma.is_some_and(|s| s.abs() < 1e-9), "{r:?}");
        assert!((r.airtime[5] - 0.2).abs() < 1e-9, "{r:?}");
        assert!((r.heard_s - 1.7).abs() < 1e-9, "{r:?}");
        assert!((r.span_s - 2.0).abs() < 1e-9, "{r:?}");
    }

    /// Two whole columns have a mean and no spread; none have no reading.
    #[test]
    fn short_of_three_columns_there_is_no_spread_and_short_of_one_no_reading() {
        let now = now();
        let two = survey(vec![
            visit(now, 1.5, 0.5, 0.5, 0.2, 0.1),
            visit(now, 0.5, 0.5, 0.5, 0.2, 0.3),
        ]);
        let r = two.share(6, now, 4).expect("two whole columns");
        assert_eq!((r.columns, r.frames.sigma, r.busy.sigma), (2, None, None));
        let cut = survey(vec![visit(now, 0.5, 0.2, 0.5, 0.2, 0.1)]);
        assert_eq!(cut.share(6, now, 4), None);
        assert_eq!(survey(vec![]).share(6, now, 4), None);
    }
}
