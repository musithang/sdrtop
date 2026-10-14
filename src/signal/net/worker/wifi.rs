// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Wi-Fi's part of the worker: whether its receiver runs on this block, and
//! on which channel.
//!
//! **It runs only where its question is asked**: a Wi-Fi view, at exactly
//! the rate a 20 MHz channel needs (`signal::net::wifi::rate_refusal`), tuned
//! to a channel's centre, since a network is decoded nowhere else. Anywhere
//! else it stands down, and the Bluetooth receivers in [`super::le`] and
//! [`super::classic`] stand down on its views in turn: the two benches are
//! two receivers, and a view runs the one it needs.
//!
//! Where it runs, it looks for preambles ([`StfDetector`]), judges each by
//! its long training field, reads the signalling after it (the L-SIG and
//! the format), and counts what it found and what a break cut short. Where it
//! runs and why not are written when the answer changes, not once a block.

use std::sync::{Arc, Mutex};

use num_complex::Complex;

use super::{Tuning, View};
use crate::signal::dsp::uncertainty::Uncertain;
use crate::signal::net::measure::Recent;
use crate::signal::net::scan::Scan;
use crate::signal::net::survey::{SETTLE, WIFI_DWELL};
use crate::signal::wifi::clock::{measure, plan, Plan};
use crate::signal::wifi::detect::{Detection, StfDetector};
use crate::signal::wifi::dsss::receive::{
    examine as dsss_examine, needs as dsss_needs, Found, SyncDetector, Verdict as DsssVerdict,
};
use crate::signal::wifi::format::{read, Signalling};
use crate::signal::wifi::sync::{examine, needs, Sync, Verdict};
use crate::state::{Family, SdrMetrics, Visit, WifiClock, WifiDsss};

/// Where the receiver runs, or why not, its detector, the repeats found
/// that wait for their long training field to be held, and the frames that
/// wait for their DATA field's end to have their clock read.
#[derive(Default)]
pub(super) struct Wifi {
    published: (Option<u8>, Option<String>),
    detector: StfDetector,
    waiting: Vec<Detection>,
    clocks: Vec<Pending>,
    /// 802.11b's detector, run beside the OFDM one, and the SYNCs found
    /// that wait for their header to be held.
    dsss: SyncDetector,
    dsss_waiting: Vec<Found>,
    /// The stream pair the last 802.11b frame read ends at: a SYNC before
    /// it is that frame's chips found again.
    dsss_until: u64,
    /// On the survey, the visit under way, and how long one lasts.
    visit: Option<Visiting>,
    rhythm: Rhythm,
}

/// How long a visit lets the tuner settle, and how long it then listens:
/// the survey's own figures, held here so the visits are cut where the
/// rotation moves on.
#[derive(Clone, Copy)]
struct Rhythm {
    settle: std::time::Duration,
    dwell: std::time::Duration,
}

impl Default for Rhythm {
    fn default() -> Self {
        Rhythm {
            settle: SETTLE,
            dwell: WIFI_DWELL,
        }
    }
}

/// A frame whose clock is read once its last DATA symbol is held.
struct Pending {
    sync: Sync,
    plan: Plan,
    centre_hz: f64,
}

impl Wifi {
    /// Decide for this block and publish a change, in one short lock block.
    pub(super) fn choose(&mut self, tuning: Tuning, view: View, state: &Arc<Mutex<SdrMetrics>>) {
        let answer = if view.open && view.wifi {
            decide(tuning.centre_hz as u64, tuning.rate_hz)
        } else {
            (None, None)
        };
        // Another channel, or none: what the detector was in the middle of
        // belonged to the old one, and so did the visit.
        if answer.0 != self.published.0 {
            self.interrupted(state);
            self.end_visit(state);
        }
        self.publish(answer, state);
    }

    /// Whether it runs on this block.
    pub(super) fn running(&self) -> bool {
        self.published.0.is_some()
    }

    /// The stream broke or the radio moved: a preamble in progress was cut
    /// short, and is counted as that.
    pub(super) fn interrupted(&mut self, state: &Arc<Mutex<SdrMetrics>>) {
        // The visit goes on: a gap in the feed has not moved the radio,
        // and a visit counts what it heard. A retune, a new stream, another
        // channel or another view ends it (`end_visit`).
        let cut = self.detector.cut() as u64 + self.waiting.len() as u64;
        self.waiting.clear();
        // A clock still waiting for its frame's end will not see it.
        self.clocks.clear();
        // An 802.11b SYNC waiting for its header will not see it either, and
        // the runs in progress belonged to the stream before the break.
        let dsss_cut = self.dsss_waiting.len() as u64;
        self.dsss_waiting.clear();
        self.dsss = SyncDetector::new();
        if cut > 0 || dsss_cut > 0 {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.net.health.wifi.cut += cut;
            m.net.health.wifi.dsss_cut += dsss_cut;
        }
    }

    /// The survey's visit under way ends here, kept if long enough; nothing
    /// else changes.
    pub(super) fn end_visit(&mut self, state: &Arc<Mutex<SdrMetrics>>) {
        if let Some(v) = self.visit.take() {
            keep(v, self.rhythm, state);
        }
    }

    /// Whether a repeat waits for samples still to come, so the feed holds
    /// a little more.
    pub(super) fn waiting(&self) -> bool {
        !self.waiting.is_empty() || !self.clocks.is_empty() || !self.dsss_waiting.is_empty()
    }

    /// Look through a block that starts at stream pair `first_pair`, and
    /// count what was found.
    pub(super) fn look(
        &mut self,
        first_pair: u64,
        block: &[Complex<f32>],
        tuning: Tuning,
        survey: bool,
        state: &Arc<Mutex<SdrMetrics>>,
    ) {
        // The survey's visit: begun past the retune's settling, ended by the
        // next retune or, locked, after a dwell, so a locked channel keeps
        // gaining visits.
        match (survey, self.published.0) {
            (true, Some(ch)) => {
                let fresh = self.visit.as_ref().is_none_or(|v| v.channel != ch);
                if fresh {
                    if let Some(v) = self.visit.take() {
                        keep(v, self.rhythm, state);
                    }
                    let settle = (self.rhythm.settle.as_secs_f64() * tuning.rate_hz) as u64;
                    self.visit = Some(Visiting::new(ch, tuning, settle));
                }
                if let Some(v) = self.visit.as_mut() {
                    v.hear(block);
                    if v.samples as f64 >= self.rhythm.dwell.as_secs_f64() * tuning.rate_hz {
                        let done = std::mem::replace(v, Visiting::new(ch, tuning, 0));
                        keep(done, self.rhythm, state);
                    }
                }
            }
            _ => self.visit = None,
        }
        // The OFDM and the 802.11b detectors on one block, side by side
        // where there is a second core: each owns all it touches, so the
        // order they run in changes nothing.
        let (ofdm, dsss) = (&mut self.detector, &mut self.dsss);
        let (found, syncs) = if parallel() {
            std::thread::scope(|scope| {
                let syncs = scope.spawn(|| dsss.push(first_pair, block));
                let found = ofdm.push(first_pair, block);
                let syncs = syncs
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
                (found, syncs)
            })
        } else {
            (ofdm.push(first_pair, block), dsss.push(first_pair, block))
        };
        if !found.is_empty() || !syncs.is_empty() {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.net.health.wifi.detected += found.len() as u64;
            m.net.health.wifi.dsss_found += syncs.len() as u64;
        }
        self.waiting.extend(found);
        self.dsss_waiting.extend(syncs);
    }

    /// Judge every waiting repeat whose stretch is held by now (`held_end`
    /// is the pair after the last one held), on `centre_hz`. Those still to
    /// come wait; one whose stretch can never be held is cut.
    pub(super) fn judge(
        &mut self,
        window: &Recent<'_>,
        held_end: u64,
        centre_hz: f64,
        now: std::time::Instant,
        state: &Arc<Mutex<SdrMetrics>>,
    ) {
        let mut counts = crate::state::WifiFunnel::default();
        let mut last = None;
        self.waiting.retain(|d| {
            let Some((from, len)) = needs(d.peak) else {
                counts.cut += 1;
                return false;
            };
            if from + len as u64 > held_end {
                return true;
            }
            let Some(stretch) = window.slice(from, len) else {
                // Held no longer: a hole the feed left.
                counts.cut += 1;
                return false;
            };
            match examine(&stretch, from, d.peak) {
                Some(Verdict::ThisChannel(s)) => match read(&stretch, from, &s) {
                    // The stretch holds the symbols a format is told from
                    // (`needs`); were it ever short, the frame was not heard
                    // whole.
                    None => counts.cut += 1,
                    Some(heard) => {
                        counts.this_channel += 1;
                        if let Some(v) = self.visit.as_mut() {
                            let (f, us) = family(&heard);
                            v.frame(f, us);
                        }
                        match heard.signalling() {
                            Signalling::LSigRead => counts.lsig_read += 1,
                            Signalling::Greenfield => counts.greenfield += 1,
                            Signalling::Unreadable => counts.unreadable += 1,
                        }
                        if let Some(plan) = plan(&heard) {
                            self.clocks.push(Pending {
                                sync: s,
                                plan,
                                centre_hz,
                            });
                        }
                        last = Some(crate::state::WifiFrame {
                            offset_hz: s.offset,
                            snr_db: 10.0 * s.snr.log10(),
                            carrier_hz: centre_hz,
                            heard,
                            when: now,
                        });
                    }
                },
                Some(Verdict::Adjacent { .. }) => {
                    counts.adjacent += 1;
                    if let Some(v) = self.visit.as_mut() {
                        v.other += 1;
                    }
                }
                Some(Verdict::NotPreamble) => counts.not_preamble += 1,
                // Held no longer, or never whole: a hole the feed left.
                None => counts.cut += 1,
            }
            false
        });
        // Every frame whose DATA field is held by now has its clock read;
        // one that a hole took is dropped, its other counts already made.
        let mut clocks = Vec::new();
        self.clocks.retain(|p| {
            let (from, len) = (p.sync.ltf_start, p.plan.end());
            if from + len as u64 > held_end {
                return true;
            }
            let clock = window
                .slice(from, len)
                .and_then(|stretch| measure(&stretch, from, &p.sync, &p.plan));
            if let Some(clock_ppm) = clock {
                let ppm = |hz: f64| hz / p.centre_hz * 1e6;
                clocks.push(WifiClock {
                    clock_ppm: clock_ppm.clock_ppm,
                    carrier_ppm: Uncertain::from_sigma(
                        ppm(clock_ppm.carrier_hz.value()),
                        ppm(clock_ppm.carrier_hz.sigma()),
                    ),
                    preamble_ppm: Uncertain::from_sigma(
                        ppm(p.sync.offset.value()),
                        ppm(p.sync.offset.sigma()),
                    ),
                    symbols: p.plan.count,
                    when: now,
                });
            }
            false
        });
        // Every 802.11b SYNC whose header is held by now is read.
        let mut last_dsss = None;
        let until = &mut self.dsss_until;
        let visit = &mut self.visit;
        self.dsss_waiting.retain(|f| {
            if f.first < *until {
                counts.dsss_inside += 1;
                return false;
            }
            let Some((from, len)) = dsss_needs(f) else {
                counts.dsss_cut += 1;
                return false;
            };
            if from + len as u64 > held_end {
                return true;
            }
            let verdict = window
                .slice(from, len)
                .and_then(|stretch| dsss_examine(&stretch, from, f));
            match verdict {
                // Another channel's frame: its chips whole megahertz from
                // ours. Its airtime still holds its own chips found again.
                Some(DsssVerdict::Frame(r)) if r.shift_mhz != 0 => {
                    counts.dsss_adjacent += 1;
                    if let Some(v) = visit.as_mut() {
                        v.other += 1;
                    }
                    if let Ok(h) = r.header {
                        *until = r.start + 20 * h.airtime_us(r.preamble) as u64;
                    }
                }
                Some(DsssVerdict::Frame(r)) => {
                    match r.header {
                        Ok(h) => {
                            counts.dsss_read += 1;
                            *until = r.start + 20 * h.airtime_us(r.preamble) as u64;
                        }
                        Err(_) => counts.dsss_refused += 1,
                    }
                    if let Some(v) = visit.as_mut() {
                        let us = r.header.ok().map(|h| f64::from(h.airtime_us(r.preamble)));
                        v.frame(Family::Dsss, us);
                    }
                    last_dsss = Some(WifiDsss {
                        preamble: r.preamble,
                        header: r.header,
                        offset_hz: r.offset_hz,
                        carrier_hz: centre_hz,
                        when: now,
                    });
                }
                Some(DsssVerdict::NoSfd) => counts.dsss_no_sfd += 1,
                Some(DsssVerdict::Elsewhere { .. }) => {
                    counts.dsss_adjacent += 1;
                    if let Some(v) = visit.as_mut() {
                        v.other += 1;
                    }
                }
                // Held no longer, or never whole: a hole the feed left.
                None => counts.dsss_cut += 1,
            }
            false
        });
        if counts != Default::default() || !clocks.is_empty() {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            if last_dsss.is_some() {
                m.net.wifi.last_dsss = last_dsss;
            }
            for c in &clocks {
                m.net.wifi.clock_check.add(c);
            }
            if let Some(c) = clocks.last() {
                m.net.wifi.clock = Some(*c);
            }
            let f = &mut m.net.health.wifi;
            f.this_channel += counts.this_channel;
            f.lsig_read += counts.lsig_read;
            f.greenfield += counts.greenfield;
            f.unreadable += counts.unreadable;
            f.adjacent += counts.adjacent;
            f.not_preamble += counts.not_preamble;
            f.cut += counts.cut;
            f.dsss_read += counts.dsss_read;
            f.dsss_refused += counts.dsss_refused;
            f.dsss_no_sfd += counts.dsss_no_sfd;
            f.dsss_cut += counts.dsss_cut;
            f.dsss_inside += counts.dsss_inside;
            f.dsss_adjacent += counts.dsss_adjacent;
            if last.is_some() {
                m.net.wifi.last_frame = last;
            }
        }
    }

    /// The section closed: nothing runs, and nothing is refused.
    pub(super) fn close(&mut self, state: &Arc<Mutex<SdrMetrics>>) {
        self.interrupted(state);
        self.end_visit(state);
        self.publish((None, None), state);
    }

    fn publish(&mut self, answer: (Option<u8>, Option<String>), state: &Arc<Mutex<SdrMetrics>>) {
        if answer == self.published {
            return;
        }
        let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
        m.net.wifi.channel = answer.0;
        m.net.wifi.refused = answer.1.clone();
        if answer.0.is_some() {
            m.net.wifi.ran = true;
        }
        drop(m);
        self.published = answer;
    }
}

/// Whether there is a second core to run the 802.11b detector on.
fn parallel() -> bool {
    static CORES: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *CORES.get_or_init(|| std::thread::available_parallelism().map_or(1, |n| n.get())) > 1
}

/// On a Wi-Fi view: the channel, or the sentence saying why not.
fn decide(centre_hz: u64, rate_hz: f64) -> (Option<u8>, Option<String>) {
    if let Some(why) = crate::signal::net::wifi::rate_refusal(rate_hz) {
        return (None, Some(why));
    }
    match crate::signal::net::band::wifi_channel(centre_hz) {
        Some(ch) => (Some(ch), None),
        None => (
            None,
            Some(format!(
                "not on a Wi-Fi channel centre ({:.3} MHz)",
                centre_hz as f64 / 1e6
            )),
        ),
    }
}

/// The family a frame of this channel is, and the airtime its signalling
/// stated, in microseconds, when it stated one.
fn family(heard: &crate::signal::wifi::format::Heard) -> (Family, Option<f64>) {
    use crate::signal::wifi::format::Format;
    // The L-SIG's airtime is the frame's in every format but greenfield,
    // which has none and states its own in its HT-SIG.
    let lsig = heard
        .lsig
        .and_then(|l| l.ok())
        .map(|l| f64::from(l.airtime_us()));
    match heard.format {
        None => (Family::Undecoded, None),
        Some(Format::NonHt) => (Family::NonHt, lsig),
        Some(Format::HtMixed(_)) => (Family::Ht, lsig),
        Some(Format::Greenfield(Ok(sig))) => (
            Family::Ht,
            sig.greenfield_airtime_ns()
                .ok()
                .map(|ns| f64::from(ns) / 1e3),
        ),
        Some(Format::Greenfield(Err(_))) => (Family::Undecoded, None),
        Some(Format::Vht(_)) => (Family::Vht, lsig),
        Some(Format::He { .. }) => (Family::He, lsig),
        Some(Format::USig { sig: Ok(u), .. }) if u.version == 0 => (Family::Eht, lsig),
        Some(Format::USig { .. }) => (Family::Undecoded, lsig),
    }
}

/// Keep a finished visit, if it listened for at least half a dwell: a
/// scrap a break left is not a visit, and weighting it as one would let
/// the break decide the survey's spread.
fn keep(v: Visiting, rhythm: Rhythm, state: &Arc<Mutex<SdrMetrics>>) {
    let channel = v.channel;
    let rate = v.rate_hz;
    let least = (rhythm.dwell.as_secs_f64() * rate / 2.0) as u64;
    if let Some(visit) = v.finish(rate, least) {
        let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(visits) = m.net.wifi.survey.visits.get_mut(usize::from(channel) - 1) {
            visits.push(visit);
        }
    }
}

/// A survey's visit under way: the channel, how much of it has been
/// listened to past the retune's settling, the band scan over it, and what
/// its frames stated.
struct Visiting {
    channel: u8,
    rate_hz: f64,
    /// Samples still to let pass before the visit counts: the tuner's
    /// settling after a retune.
    settle: u64,
    samples: u64,
    scan: Scan,
    airtime_us: [f64; 7],
    read: u32,
    other: u32,
}

impl Visiting {
    fn new(channel: u8, tuning: Tuning, settle: u64) -> Self {
        Visiting {
            channel,
            rate_hz: tuning.rate_hz,
            settle,
            samples: 0,
            scan: Scan::new(tuning.centre_hz, tuning.rate_hz, tuning.span_hz),
            airtime_us: [0.0; 7],
            read: 0,
            other: 0,
        }
    }

    /// A block of `block` heard on this visit.
    fn hear(&mut self, block: &[Complex<f32>]) {
        let skip = (self.settle as usize).min(block.len());
        self.settle -= skip as u64;
        let heard = &block[skip..];
        if !heard.is_empty() {
            self.scan.push_iq(heard);
            self.samples += heard.len() as u64;
        }
    }

    /// A frame read on this channel's centre.
    fn frame(&mut self, family: Family, airtime_us: Option<f64>) {
        self.read += 1;
        if let (Some(i), Some(us)) = (Family::ALL.iter().position(|f| *f == family), airtime_us) {
            self.airtime_us[i] += us;
        }
    }

    /// The visit as kept, at `rate_hz`; `None` when it was listened to for
    /// less than `least` samples, too short to be a visit.
    fn finish(mut self, rate_hz: f64, least: u64) -> Option<Visit> {
        if self.samples < least.max(1) {
            return None;
        }
        let seconds = self.samples as f64 / rate_hz;
        // The busiest megahertz of the channel's 20, by the band's own
        // measure; a cell the view never covered has no say.
        let band = self.scan.take();
        let centre = crate::signal::net::band::wifi_centre_hz(self.channel)? as f64;
        let busy = (-10..10)
            .filter_map(|k| crate::signal::net::occupancy::cell_of(centre + (k as f64 + 0.5) * 1e6))
            .filter_map(|c| band.cells.get(c))
            .filter(|c| c.windows > 0)
            .map(|c| c.duty)
            .fold(0.0, f64::max);
        // A frame's stated airtime can run past the visit's end; its share
        // of the visit never past all of it.
        let airtime = self.airtime_us.map(|us| (us / 1e6 / seconds).min(1.0));
        Some(Visit {
            seconds,
            busy,
            airtime,
            read: self.read,
            other: self.other,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::wifi::format::{Format, Heard};
    use crate::signal::wifi::lsig::LSig;
    use crate::signal::wifi::signal_field::Rate;

    fn heard(format: Option<Format>, length: u16) -> Heard {
        Heard {
            lsig: Some(Ok(LSig {
                rate: Rate::R6,
                length,
            })),
            format,
        }
    }

    /// Each frame is the family its signalling named, with the airtime its
    /// L-SIG stated; a frame whose L-SIG was refused is undecoded and states
    /// nothing.
    #[test]
    fn a_frame_is_the_family_its_signalling_named() {
        let air = LSig {
            rate: Rate::R6,
            length: 100,
        }
        .airtime_us() as f64;
        assert_eq!(
            family(&heard(Some(Format::NonHt), 100)),
            (Family::NonHt, Some(air))
        );
        assert_eq!(
            family(&heard(Some(Format::Vht(Ok(()))), 100)),
            (Family::Vht, Some(air))
        );
        assert_eq!(
            family(&Heard {
                lsig: Some(Err(crate::signal::wifi::lsig::Refused::Parity)),
                format: None
            }),
            (Family::Undecoded, None)
        );
    }

    fn tuning() -> Tuning {
        Tuning {
            first_pair: 0,
            centre_hz: 2_437e6,
            rate_hz: 20e6,
            span_hz: 20e6,
        }
    }

    fn state() -> Arc<Mutex<SdrMetrics>> {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = "wifi_channel".into();
        m.ui.active_preset = "wifi_survey".into();
        Arc::new(Mutex::new(m))
    }

    fn view() -> View {
        View {
            open: true,
            classic: false,
            survey: false,
            coded: false,
            wifi_survey: true,
            wifi: true,
            phy: crate::signal::ble::Phy::default(),
            locked: false,
            following: false,
        }
    }

    /// **The survey keeps a visit a dwell long, and drops a scrap.** On a
    /// short rhythm (no settling, 10 ms a dwell): 12 ms on channel 6 keep
    /// one visit of 10 ms, and the 2 ms after it, ended by the move to 11,
    /// are too short to be one; 6 ms on 11, ended with the feed, are kept.
    #[test]
    fn the_survey_keeps_a_visit_a_dwell_long_and_drops_a_scrap() {
        let st = state();
        let mut w = Wifi {
            rhythm: Rhythm {
                settle: std::time::Duration::ZERO,
                dwell: std::time::Duration::from_millis(10),
            },
            ..Default::default()
        };
        let mut rng = crate::signal::dsp::testkit::Rng::new(9);
        let on = |hz: f64, first_pair: u64| Tuning {
            first_pair,
            centre_hz: hz,
            rate_hz: 20e6,
            span_hz: 20e6,
        };
        let mut at = 0u64;
        for (hz, blocks) in [(2_437e6, 60), (2_462e6, 30)] {
            for _ in 0..blocks {
                let t = on(hz, at);
                w.choose(t, view(), &st);
                w.look(at, &rng.noise(4000, 0.01), t, true, &st);
                at += 4000;
            }
        }
        w.end_visit(&st);
        let m = st.lock().unwrap();
        let six = &m.net.wifi.survey.visits[5];
        assert_eq!(six.len(), 1, "{six:?}");
        assert!((six[0].seconds - 0.010).abs() < 1e-9, "{six:?}");
        let eleven = &m.net.wifi.survey.visits[10];
        assert_eq!(eleven.len(), 1, "{eleven:?}");
        assert!((eleven[0].seconds - 0.006).abs() < 1e-9, "{eleven:?}");
        assert!(m.net.wifi.survey.visits.iter().map(Vec::len).sum::<usize>() == 2);
    }

    /// **A gap in the feed does not end a visit.** The radio has not
    /// moved, so the visit goes on listening; what it heard is what is
    /// counted. Live, a busy machine dropped a block every few, and every
    /// visit ended at the first drop as a scrap too short to keep.
    #[test]
    fn a_gap_in_the_feed_does_not_end_a_visit() {
        let st = state();
        let mut w = Wifi {
            rhythm: Rhythm {
                settle: std::time::Duration::ZERO,
                dwell: std::time::Duration::from_millis(10),
            },
            ..Default::default()
        };
        let mut rng = crate::signal::dsp::testkit::Rng::new(11);
        let t = Tuning {
            first_pair: 0,
            centre_hz: 2_437e6,
            rate_hz: 20e6,
            span_hz: 20e6,
        };
        w.choose(t, view(), &st);
        // 4 ms, a gap, 7 ms: one visit of 10 ms heard, and a 1 ms scrap.
        for i in 0..55 {
            if i == 20 {
                w.interrupted(&st);
            }
            w.look(0, &rng.noise(4000, 0.01), t, true, &st);
        }
        let m = st.lock().unwrap();
        let six = &m.net.wifi.survey.visits[5];
        assert_eq!(six.len(), 1, "{six:?}");
        assert!((six[0].seconds - 0.010).abs() < 1e-9, "{six:?}");
    }

    /// Leaving the survey drops the visit under way: what a visit would
    /// have heard off the survey is not the survey's.
    #[test]
    fn leaving_the_survey_drops_the_visit_under_way() {
        let st = state();
        let mut w = Wifi {
            rhythm: Rhythm {
                settle: std::time::Duration::ZERO,
                dwell: std::time::Duration::from_millis(10),
            },
            ..Default::default()
        };
        let mut rng = crate::signal::dsp::testkit::Rng::new(10);
        let t = Tuning {
            first_pair: 0,
            centre_hz: 2_437e6,
            rate_hz: 20e6,
            span_hz: 20e6,
        };
        w.choose(t, view(), &st);
        // 8 ms on the survey, long enough to be kept, then a block off it.
        for _ in 0..40 {
            w.look(0, &rng.noise(4000, 0.01), t, true, &st);
        }
        w.look(0, &rng.noise(4000, 0.01), t, false, &st);
        w.interrupted(&st);
        assert!(st
            .lock()
            .unwrap()
            .net
            .wifi
            .survey
            .visits
            .iter()
            .all(Vec::is_empty));
    }

    /// A visit counts the samples past its settling, keeps its frames'
    /// airtime by family as a share of what was heard, and its busiest
    /// megahertz; one shorter than the least is no visit.
    #[test]
    fn a_visit_keeps_what_it_heard() {
        let mut rng = crate::signal::dsp::testkit::Rng::new(5);
        let mut v = Visiting::new(6, tuning(), 4096);
        for _ in 0..40 {
            v.hear(&rng.noise(4096, 0.01));
        }
        v.frame(Family::Dsss, Some(1000.0));
        v.frame(Family::Undecoded, None);
        v.other += 2;
        let kept = v.finish(20e6, 4096).expect("long enough");
        let heard_s = 39.0 * 4096.0 / 20e6;
        assert!((kept.seconds - heard_s).abs() < 1e-9, "{kept:?}");
        assert!((kept.airtime[5] - 1e-3 / heard_s).abs() < 1e-9, "{kept:?}");
        assert_eq!((kept.read, kept.other), (2, 2));
        assert!(kept.busy < 0.05, "noise alone is not busy: {kept:?}");
        let short = Visiting::new(6, tuning(), 0);
        assert!(short.finish(20e6, 1).is_none());
    }
}
