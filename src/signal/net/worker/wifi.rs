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
use crate::signal::wifi::clock::{measure, plan, Plan};
use crate::signal::wifi::detect::{Detection, StfDetector};
use crate::signal::wifi::format::{read, Signalling};
use crate::signal::wifi::sync::{examine, needs, Sync, Verdict};
use crate::state::{SdrMetrics, WifiClock};

/// Where the receiver runs, or why not, its detector, the repeats found
/// that wait for their long training field to be held, and the frames that
/// wait for their DATA field's end to have their clock read.
#[derive(Default)]
pub(super) struct Wifi {
    published: (Option<u8>, Option<String>),
    detector: StfDetector,
    waiting: Vec<Detection>,
    clocks: Vec<Pending>,
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
        // belonged to the old one.
        if answer.0 != self.published.0 {
            self.interrupted(state);
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
        let cut = self.detector.cut() as u64 + self.waiting.len() as u64;
        self.waiting.clear();
        // A clock still waiting for its frame's end will not see it.
        self.clocks.clear();
        if cut > 0 {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.net.health.wifi.cut += cut;
        }
    }

    /// Whether a repeat waits for samples still to come, so the feed holds
    /// a little more.
    pub(super) fn waiting(&self) -> bool {
        !self.waiting.is_empty() || !self.clocks.is_empty()
    }

    /// Look through a block that starts at stream pair `first_pair`, and
    /// count what was found.
    pub(super) fn look(
        &mut self,
        first_pair: u64,
        block: &[Complex<f32>],
        state: &Arc<Mutex<SdrMetrics>>,
    ) {
        let found = self.detector.push(first_pair, block);
        if !found.is_empty() {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.net.health.wifi.detected += found.len() as u64;
        }
        self.waiting.extend(found);
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
                Some(Verdict::Adjacent { .. }) => counts.adjacent += 1,
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
        if counts != Default::default() || !clocks.is_empty() {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
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
            if last.is_some() {
                m.net.wifi.last_frame = last;
            }
        }
    }

    /// The section closed: nothing runs, and nothing is refused.
    pub(super) fn close(&mut self, state: &Arc<Mutex<SdrMetrics>>) {
        self.interrupted(state);
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
