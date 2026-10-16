//! Channel › Airtime's numbers, as pure functions of the state, so its
//! panels only draw: the hero's columns and the card's reading from the
//! survey's visits on the locked channel, and the ledger from the frames
//! kept for the Frames view.

use super::wifi::{Family, RowKind, WifiRow, WifiState, WifiSurvey};
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

/// What a ledger row is spent by: a format of this channel, or another
/// channel's frames, which are counted and never given a format here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spender {
    Format(Family),
    OtherChannel,
}

impl Spender {
    /// The standards' own order: 802.11b, a/g, n, ac, ax, be, then another
    /// channel's and the frames not decoded. Fixed, so a row a reader is
    /// reaching for never moves.
    fn order(self) -> u8 {
        match self {
            Spender::Format(Family::Dsss) => 0,
            Spender::Format(Family::NonHt) => 1,
            Spender::Format(Family::Ht) => 2,
            Spender::Format(Family::Vht) => 3,
            Spender::Format(Family::He) => 4,
            Spender::Format(Family::Eht) => 5,
            Spender::OtherChannel => 6,
            Spender::Format(Family::Undecoded) => 7,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Spender::Format(f) => f.label(),
            Spender::OtherChannel => "another channel",
        }
    }
}

/// A rate a frame's signalling named: 802.11b's header, a non-HT L-SIG, an
/// HT-SIG's MCS. VHT, HE and EHT name theirs in fields not read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedgerRate {
    Dsss(crate::signal::wifi::dsss::header::Rate),
    NonHt(crate::signal::wifi::signal_field::Rate),
    Mcs(u8),
}

impl LedgerRate {
    fn of(row: &WifiRow) -> Option<LedgerRate> {
        use crate::signal::wifi::format::Format;
        match row.kind {
            RowKind::Dsss { header: Ok(h), .. } => Some(LedgerRate::Dsss(h.rate)),
            RowKind::Ofdm(heard) => match (heard.format, heard.lsig) {
                (Some(Format::NonHt), Some(Ok(l))) => Some(LedgerRate::NonHt(l.rate)),
                (Some(Format::HtMixed(Ok(h)) | Format::Greenfield(Ok(h))), _) => {
                    Some(LedgerRate::Mcs(h.mcs))
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// Its place among its format's rates, slowest first.
    fn key(self) -> u32 {
        use crate::signal::wifi::dsss::header::Rate as D;
        match self {
            LedgerRate::Dsss(D::M1) => 10,
            LedgerRate::Dsss(D::M2) => 20,
            LedgerRate::Dsss(D::M5_5) => 55,
            LedgerRate::Dsss(D::M11) => 110,
            LedgerRate::NonHt(r) => r.mbps(),
            LedgerRate::Mcs(m) => u32::from(m),
        }
    }

    pub fn label(self) -> String {
        match self {
            LedgerRate::Dsss(r) => format!("{} Mb/s", r.mbps()),
            LedgerRate::NonHt(r) => format!("{} Mb/s", r.mbps()),
            LedgerRate::Mcs(m) => format!("MCS {m}"),
        }
    }
}

/// One row of the ledger: what spent the time, at which rate (none where
/// none was read), how many frames, how many of them stated their airtime
/// and how much they stated; `rates` is more than one in a folded row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LedgerRow {
    pub spender: Spender,
    pub rate: Option<LedgerRate>,
    pub frames: usize,
    pub stated: usize,
    pub air_us: f64,
    pub rates: usize,
}

impl LedgerRow {
    pub fn label(&self) -> String {
        match (self.rates, self.rate) {
            (2.., _) => format!("{} \u{b7} {} rates", self.spender.label(), self.rates),
            (_, Some(r)) => format!("{} {}", self.spender.label(), r.label()),
            (_, None) => self.spender.label().to_string(),
        }
    }

    /// The airtime a frame stated, over the frames that stated one; `None`
    /// where none did.
    pub fn mean_us(&self) -> Option<f64> {
        (self.stated > 0).then(|| self.air_us / self.stated as f64)
    }
}

/// The ledger: its rows in the standards' order, the span of the stream
/// its frames were heard over and how much of it was heard (the span less
/// what feed gaps took).
#[derive(Clone, Debug, PartialEq)]
pub struct Ledger {
    pub rows: Vec<LedgerRow>,
    pub span_s: f64,
    pub heard_s: f64,
}

impl Ledger {
    /// A row's share of the time heard, from the airtime its frames stated.
    pub fn share(&self, row: &LedgerRow) -> f64 {
        row.air_us / 1e6 / self.heard_s
    }

    /// A row's frames a second of the time heard.
    pub fn per_s(&self, row: &LedgerRow) -> f64 {
        row.frames as f64 / self.heard_s
    }

    /// The rows a format each, its rates together: what gives way when a
    /// row a rate does not fit.
    pub fn folded(&self) -> Vec<LedgerRow> {
        let mut out: Vec<LedgerRow> = Vec::new();
        for r in &self.rows {
            match out.last_mut().filter(|o| o.spender == r.spender) {
                Some(o) => {
                    o.frames += r.frames;
                    o.stated += r.stated;
                    o.air_us += r.air_us;
                    o.rates += r.rates;
                }
                None => out.push(*r),
            }
        }
        out
    }
}

/// The pairs a second the Wi-Fi receiver runs at.
const PAIRS_PER_S: f64 = 20e6;

impl WifiState {
    /// The ledger of the frames heard on `centre_hz` on the newest clock
    /// they were heard on: from the first of them to the stream's edge (or,
    /// on another clock, the last one's end), less the spans a feed gap
    /// took. `None` until a frame was heard there.
    pub fn ledger(&self, centre_hz: f64) -> Option<Ledger> {
        let here = |r: &&WifiRow| (r.carrier_hz - centre_hz).abs() < 1.0;
        let epoch = self.rows.iter().filter(here).map(|r| r.epoch).max()?;
        let rows: Vec<&WifiRow> = self
            .rows
            .iter()
            .filter(here)
            .filter(|r| r.epoch == epoch)
            .collect();
        let from = rows.iter().map(|r| r.start).min()?;
        let last_end = rows
            .iter()
            .map(|r| r.start + (f64::from(r.airtime_us.unwrap_or(0)) * PAIRS_PER_S / 1e6) as u64)
            .max()?;
        let to = match self.heard_to {
            Some((e, heard)) if e == epoch => heard.max(last_end),
            _ => last_end,
        };
        let lost: u64 = self
            .lost
            .iter()
            .filter(|l| l.epoch == epoch)
            .map(|l| l.to.min(to).saturating_sub(l.from.max(from)))
            .sum();
        let span = to.saturating_sub(from);
        let heard_s = span.saturating_sub(lost) as f64 / PAIRS_PER_S;
        if heard_s <= 0.0 {
            return None;
        }
        let mut out: Vec<LedgerRow> = Vec::new();
        for r in rows {
            let spender = r.family().map_or(Spender::OtherChannel, Spender::Format);
            let rate = LedgerRate::of(r).filter(|_| spender != Spender::OtherChannel);
            let at = match out
                .iter()
                .position(|o| o.spender == spender && o.rate == rate)
            {
                Some(i) => i,
                None => {
                    out.push(LedgerRow {
                        spender,
                        rate,
                        frames: 0,
                        stated: 0,
                        air_us: 0.0,
                        rates: 1,
                    });
                    out.len() - 1
                }
            };
            let o = &mut out[at];
            o.frames += 1;
            if let Some(us) = r.airtime_us {
                o.stated += 1;
                o.air_us += f64::from(us);
            }
        }
        out.sort_by_key(|o| (o.spender.order(), o.rate.map_or(0, LedgerRate::key)));
        Some(Ledger {
            rows: out,
            span_s: span as f64 / PAIRS_PER_S,
            heard_s,
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

    // The ledger.

    use crate::signal::wifi::dsss::header::{Header, Preamble, Rate as DsssRate};
    use crate::signal::wifi::format::{Format, Heard};
    use crate::signal::wifi::htsig::HtSig;
    use crate::signal::wifi::lsig::LSig;
    use crate::signal::wifi::signal_field::Rate;
    use crate::state::{Lost, RowKind, WifiRow};

    const ELEVEN: f64 = 2_462e6;

    /// A frame `at_s` into epoch `epoch` on `centre`, stating `air_us`.
    fn row(kind: RowKind, at_s: f64, air_us: Option<u32>, epoch: u64, centre: f64) -> WifiRow {
        WifiRow {
            seq: 0,
            start: (at_s * 20e6) as u64,
            run: epoch,
            when: Instant::now(),
            carrier_hz: centre,
            kind,
            airtime_us: air_us,
            gap_us: None,
            offset_hz: None,
            snr_db: None,
            channel: None,
            clock: None,
            epoch,
        }
    }

    fn b(rate: DsssRate) -> RowKind {
        RowKind::Dsss {
            preamble: Preamble::Long,
            header: Ok(Header {
                rate,
                locked: true,
                extension: false,
                length_us: 2000,
            }),
        }
    }

    fn non_ht(rate: Rate) -> RowKind {
        RowKind::Ofdm(Heard {
            lsig: Some(Ok(LSig { rate, length: 100 })),
            format: Some(Format::NonHt),
        })
    }

    fn ht(mcs: u8) -> RowKind {
        RowKind::Ofdm(Heard {
            lsig: Some(Ok(LSig {
                rate: Rate::R6,
                length: 100,
            })),
            format: Some(Format::HtMixed(Ok(HtSig {
                mcs,
                cbw40: false,
                length: 1500,
                stbc: 0,
                ldpc: false,
                short_gi: false,
                ness: 0,
            }))),
        })
    }

    fn unread() -> RowKind {
        RowKind::Ofdm(Heard {
            lsig: None,
            format: None,
        })
    }

    /// One second of channel 11: two 802.11b beacons, two non-HT rates
    /// written out of order, an HT frame, another channel's and an
    /// unreadable one; heard to the second's end.
    fn second() -> crate::state::net::wifi::WifiState {
        let mut w = crate::state::net::wifi::WifiState::default();
        for r in [
            row(b(DsssRate::M1), 0.0, Some(2000), 0, ELEVEN),
            row(non_ht(Rate::R24), 0.1, Some(50), 0, ELEVEN),
            row(ht(7), 0.2, Some(100), 0, ELEVEN),
            row(non_ht(Rate::R6), 0.3, Some(100), 0, ELEVEN),
            row(RowKind::OtherOfdm { shift_mhz: -5 }, 0.4, None, 0, ELEVEN),
            row(unread(), 0.5, None, 0, ELEVEN),
            row(b(DsssRate::M1), 0.6, Some(2000), 0, ELEVEN),
        ] {
            w.push_row(r);
        }
        w.heard_to = Some((0, 20_000_000));
        w
    }

    /// **One row a format and rate, in the standards' order**, never by
    /// share: 802.11b, non-HT, HT, then another channel's and the
    /// unreadable; each its share of the time heard, frames a second and
    /// airtime a frame, none for a row that stated none.
    #[test]
    fn the_ledger_is_a_row_a_format_and_rate_in_the_standards_order() {
        let l = second().ledger(ELEVEN).expect("frames heard");
        let labels: Vec<String> = l.rows.iter().map(|r| r.label()).collect();
        assert_eq!(
            labels,
            [
                "802.11b 1 Mb/s",
                "non-HT 6 Mb/s",
                "non-HT 24 Mb/s",
                "HT MCS 7",
                "another channel",
                "not decoded"
            ]
        );
        assert!((l.span_s - 1.0).abs() < 1e-9, "{l:?}");
        assert!((l.heard_s - 1.0).abs() < 1e-9, "{l:?}");
        let b = &l.rows[0];
        assert!((l.share(b) - 0.004).abs() < 1e-9, "{l:?}");
        assert!((l.per_s(b) - 2.0).abs() < 1e-9, "{l:?}");
        assert_eq!(b.mean_us(), Some(2000.0));
        assert_eq!(l.rows[5].mean_us(), None);
        assert_eq!(l.share(&l.rows[5]), 0.0);
        assert!((l.per_s(&l.rows[5]) - 1.0).abs() < 1e-9, "{l:?}");
    }

    /// **The time heard is the span less what a feed gap took**: a lost
    /// quarter second of this clock comes off, another clock's does not;
    /// an older clock's frames and another centre's are not this ledger's.
    #[test]
    fn the_time_heard_is_the_span_less_its_lost_spans() {
        let mut w = second();
        w.lose(Lost {
            epoch: 0,
            from: 10_000_000,
            to: 15_000_000,
        });
        w.lose(Lost {
            epoch: 7,
            from: 0,
            to: 20_000_000,
        });
        let l = w.ledger(ELEVEN).unwrap();
        assert!((l.span_s - 1.0).abs() < 1e-9, "{l:?}");
        assert!((l.heard_s - 0.75).abs() < 1e-9, "{l:?}");
        assert!((l.per_s(&l.rows[0]) - 2.0 / 0.75).abs() < 1e-9, "{l:?}");

        // A newer clock: only its frames, from its first.
        w.push_row(row(b(DsssRate::M2), 3.0, Some(1000), 1, ELEVEN));
        w.push_row(row(b(DsssRate::M2), 3.0, Some(1000), 1, 2_437e6));
        w.heard_to = Some((1, 80_000_000));
        let l = w.ledger(ELEVEN).unwrap();
        assert_eq!(l.rows.len(), 1, "{l:?}");
        assert_eq!(l.rows[0].label(), "802.11b 2 Mb/s");
        assert!((l.span_s - 1.0).abs() < 1e-9, "{l:?}");
    }

    /// Heard to no further than its last frame's end where the stream's
    /// edge is another clock's: the span is never guessed past what was
    /// heard.
    #[test]
    fn without_the_streams_edge_the_span_ends_with_the_last_frame() {
        let mut w = second();
        w.heard_to = None;
        let l = w.ledger(ELEVEN).unwrap();
        assert!((l.span_s - (0.6 + 0.002)).abs() < 1e-9, "{l:?}");
    }

    /// **Folded, a format's rates are one row**: shares and frames a
    /// second summed, airtime a frame the mean over the frames that stated
    /// one; a format of one rate keeps its name.
    #[test]
    fn folded_a_formats_rates_are_one_row() {
        let l = second().ledger(ELEVEN).unwrap();
        let f = l.folded();
        let labels: Vec<String> = f.iter().map(|r| r.label()).collect();
        assert_eq!(
            labels,
            [
                "802.11b 1 Mb/s",
                "non-HT \u{b7} 2 rates",
                "HT MCS 7",
                "another channel",
                "not decoded"
            ]
        );
        assert_eq!(f[1].mean_us(), Some(75.0));
        assert!((l.share(&f[1]) - 150e-6).abs() < 1e-12, "{f:?}");
        assert!((l.per_s(&f[1]) - 2.0).abs() < 1e-9, "{f:?}");
    }

    /// Nothing heard on this centre is no ledger, not an empty one.
    #[test]
    fn nothing_heard_is_no_ledger() {
        assert!(second().ledger(2_412e6).is_none());
        assert!(crate::state::net::wifi::WifiState::default()
            .ledger(ELEVEN)
            .is_none());
    }
}
