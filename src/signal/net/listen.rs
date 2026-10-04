// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! One scheduled listen: on this channel, over this stretch of the stream,
//! with these receivers, whatever the reason for listening.
//!
//! A followed connection's event and an AuxPtr's promised auxiliary packet
//! ask the same question of the samples the worker holds: is the channel in
//! the radio's view, are those samples still held, and if both, what did a
//! receiver built for it hear there. The connection follower answered it
//! first, inline; it lives here so the AuxPtr follower asks it the same way
//! rather than through a copy that could come to disagree.

use std::collections::HashMap;

use crate::signal::ble::coded_rx::CodedReceiver;
use crate::signal::ble::data::DataPdu;
use crate::signal::ble::pdu::Packet;
use crate::signal::ble::receive::{DataTiming, Link, Receiver};
use crate::signal::ble::Phy;
use crate::signal::net::measure::Recent;

/// What to listen with: a receiver for one link on one PHY, or LE Coded's
/// own chain (which reads S=2 and S=8 alike, from the CI).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ear {
    Link(Link, Phy),
    Coded,
}

/// One scheduled listen: `channel`, stream pairs `from_pair` to `to_pair`,
/// with every one of `ears`.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub ears: Vec<Ear>,
    pub channel: u8,
    pub from_pair: f64,
    pub to_pair: f64,
}

/// What a listen came to.
#[derive(Debug, Default)]
pub struct Outcome {
    /// The channel was inside the radio's view.
    pub in_view: bool,
    /// In view, but not listened to: its samples were not held (lost to the
    /// feed, or older than what is kept), or no receiver could be built.
    pub feed_lost: bool,
    /// Data channel PDUs heard, placed on the stream.
    pub data: Vec<(DataPdu, DataTiming)>,
    /// Advertising channel PDUs heard (an advertising link's, or LE Coded's).
    pub packets: Vec<Packet>,
}

/// A receiver kept between listens.
enum Kept {
    Link(Box<Receiver>),
    Coded(Box<CodedReceiver>),
}

/// The receivers scheduled listens use, kept between them so a filter is not
/// designed again for every event.
#[derive(Default)]
pub struct Listener {
    kept: HashMap<(Ear, u8), Kept>,
}

impl Listener {
    /// Listen to `job`'s window, if its channel is in the view of a radio
    /// tuned to `centre_hz` seeing `span_hz` at `rate_hz` and its samples
    /// are held: each ear's receiver built or reused, reset, given the
    /// window, and asked what it heard. The receivers' funnels are drained
    /// here, so a scheduled listen never counts into a live funnel.
    pub fn listen(
        &mut self,
        job: &Job,
        held: &Recent,
        rate_hz: f64,
        centre_hz: f64,
        span_hz: f64,
    ) -> Outcome {
        let mut out = Outcome {
            in_view: crate::signal::ble::channel::in_view(job.channel, centre_hz, span_hz),
            ..Outcome::default()
        };
        if !out.in_view {
            return out;
        }
        let start = job.from_pair.max(0.0).floor() as u64;
        let len = (job.to_pair - start as f64).ceil().max(0.0) as usize;
        let Some(samples) = held.slice(start, len) else {
            // Not held: lost to the feed, or older than what is kept. Not
            // listened to either way.
            out.feed_lost = true;
            return out;
        };
        for &ear in &job.ears {
            let key = (ear, job.channel);
            let fits = self.kept.get(&key).is_some_and(|k| match (k, ear) {
                (Kept::Link(r), Ear::Link(_, phy)) => {
                    r.matches(job.channel, rate_hz, phy, centre_hz)
                }
                (Kept::Coded(r), Ear::Coded) => r.matches(job.channel, rate_hz, centre_hz),
                _ => false,
            });
            if !fits {
                let built = match ear {
                    Ear::Link(link, phy) => {
                        Receiver::for_link(rate_hz, job.channel, phy, centre_hz, link)
                            .map(|r| Kept::Link(Box::new(r)))
                    }
                    Ear::Coded => CodedReceiver::new(rate_hz, job.channel, centre_hz)
                        .map(|r| Kept::Coded(Box::new(r))),
                };
                match built {
                    Ok(k) => {
                        self.kept.insert(key, k);
                    }
                    // No receiver, no listening.
                    Err(_) => {
                        out.feed_lost = true;
                        continue;
                    }
                }
            }
            match self.kept.get_mut(&key) {
                Some(Kept::Link(rx)) => {
                    rx.reset();
                    out.packets.extend(rx.push_iq_at(&samples, start));
                    rx.take_funnel();
                    out.data.extend(rx.take_data());
                }
                Some(Kept::Coded(rx)) => {
                    rx.reset();
                    out.packets.extend(rx.push_iq_at(&samples, start));
                    rx.take_funnel();
                }
                None => {}
            }
        }
        out
    }

    /// Let go of the receivers whose ear `keep` refuses.
    pub fn retain(&mut self, keep: impl Fn(&Ear) -> bool) {
        self.kept.retain(|(ear, _), _| keep(ear));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::ble::receive::tests::{data_pdu, synthetic_data_burst};
    use crate::signal::ble::receive::Link;
    use crate::signal::ble::Phy;
    use crate::signal::net::measure::Recent;

    const AA: u32 = 0x5065_4b6a;
    const CRC: u32 = 0x3a_5b7c;

    fn link_job(channel: u8, from: f64, to: f64) -> Job {
        Job {
            ears: vec![Ear::Link(
                Link::Data {
                    access_address: AA,
                    crc_init: CRC,
                },
                Phy::OneM,
            )],
            channel,
            from_pair: from,
            to_pair: to,
        }
    }

    /// A channel the radio does not see is not listened to: not in view,
    /// nothing heard, nothing lost.
    #[test]
    fn a_window_out_of_view_is_not_listened_to() {
        let mut listener = Listener::default();
        let out = listener.listen(
            &link_job(30, 0.0, 1000.0),
            &Recent::new([]),
            20e6,
            2_426e6,
            20e6,
        );
        assert!(!out.in_view && !out.feed_lost && out.data.is_empty());
    }

    /// In view, but its samples are not held: lost to the feed.
    #[test]
    fn a_window_not_held_is_feed_lost() {
        let mut listener = Listener::default();
        let out = listener.listen(
            &link_job(12, 0.0, 1000.0),
            &Recent::new([]),
            20e6,
            2_426e6,
            20e6,
        );
        assert!(out.in_view && out.feed_lost && out.data.is_empty());
    }

    /// A link's packet inside its window is heard, placed on the stream.
    #[test]
    fn a_link_packet_in_its_window_is_heard() {
        let pdu = data_pdu(1, &[]);
        let (iq, start) = synthetic_data_burst(20e6, 12, 2_426e6, AA, CRC, &pdu, 4000);
        let recent = Recent::new([(0, &iq[..])]);
        let mut listener = Listener::default();
        let out = listener.listen(
            &link_job(12, start - 2000.0, iq.len() as f64 - 1.0),
            &recent,
            20e6,
            2_426e6,
            20e6,
        );
        assert!(out.in_view && !out.feed_lost);
        assert_eq!(out.data.len(), 1);
        assert!(
            (out.data[0].1.start_pair - start).abs() < 5.0,
            "{:?}",
            out.data[0].1
        );
    }

    /// A Coded packet inside its window is heard by the Coded ear.
    #[test]
    fn a_coded_packet_in_its_window_is_heard() {
        use crate::signal::ble::coded::{self, Coding};
        use crate::signal::ble::detect::ADVERTISING_ACCESS_ADDRESS;
        let symbols = coded::transmit(
            ADVERTISING_ACCESS_ADDRESS,
            Coding::S2,
            9,
            0x07,
            &[6, 0x18, 1, 2, 3, 4, 5],
        );
        let wave = crate::signal::ble::gfsk::modulate(&symbols, 20, 250e3, 20e6, 0.5);
        // Channel 9 is 4 MHz below a radio at 2426 MHz.
        let step = -std::f64::consts::TAU * 4e6 / 20e6;
        let mut iq = vec![num_complex::Complex::new(0.0f32, 0.0); 3000];
        iq.extend(wave.iter().enumerate().map(|(n, s)| {
            s * num_complex::Complex::from_polar(1.0, (step * (3000 + n) as f64) as f32)
        }));
        iq.extend(vec![num_complex::Complex::new(0.0f32, 0.0); 3000]);
        let recent = Recent::new([(0, &iq[..])]);
        let job = Job {
            ears: vec![Ear::Coded],
            channel: 9,
            from_pair: 0.0,
            to_pair: iq.len() as f64 - 1.0,
        };
        let out = Listener::default().listen(&job, &recent, 20e6, 2_426e6, 20e6);
        assert_eq!(out.packets.len(), 1, "{:?}", out.packets);
        assert!(out.packets[0].crc_ok);
        assert_eq!(out.packets[0].coding, Some(Coding::S2));
    }

    /// What one AuxPtr's listen costs, by the PHY it promises: noise, a
    /// window as `aux::promise` opens it for an offset of 3 ms in 30 us
    /// units, on a data channel in view, a receiver kept between listens as
    /// the worker keeps it. By hand, in release, on the machine in question.
    #[test]
    #[ignore]
    fn aux_listen_bench() {
        use crate::signal::ble::aux::{promise, AuxPhy};
        use crate::signal::dsp::testkit::Rng;
        let rate = 20e6;
        let noise = Rng::new(3).noise(1_000_000, 1e-3);
        if let Err(why) = Receiver::for_link(rate, 12, Phy::TwoM, 2_426e6, Link::Advertising) {
            eprintln!("LE 2M advertising receiver on 12: {why}");
        }
        let recent = Recent::new([(0, &noise[..])]);
        for (phy_bits, ear, name) in [
            (0b000u32, Ear::Link(Link::Advertising, Phy::OneM), "LE 1M"),
            (0b001, Ear::Link(Link::Advertising, Phy::TwoM), "LE 2M"),
            (0b010, Ear::Coded, "LE Coded"),
        ] {
            // AuxPtr: channel 12, CA 1 (50 ppm), 30 us units, 100 of them.
            let v: u32 = 12 | 1 << 6 | 100 << 8 | phy_bits << 21;
            let mut payload = vec![0, 0b0001_0000];
            payload.extend([v as u8, (v >> 8) as u8, (v >> 16) as u8]);
            payload[0] = (payload.len() - 1) as u8;
            let header = crate::signal::ble::ext::parse(&payload).unwrap();
            let p = promise(1, 0.0, &header, 0, rate).unwrap();
            assert_eq!(
                p.phy,
                match phy_bits {
                    0 => AuxPhy::OneM,
                    1 => AuxPhy::TwoM,
                    _ => AuxPhy::Coded,
                }
            );
            let window_us = (p.to_pair - p.from_pair) / rate * 1e6;
            let mut listener = Listener::default();
            let runs = 50;
            let started = std::time::Instant::now();
            for _ in 0..runs {
                let job = Job {
                    ears: vec![ear],
                    channel: 12,
                    from_pair: p.from_pair,
                    to_pair: p.to_pair,
                };
                let out = listener.listen(&job, &recent, rate, 2_426e6, rate);
                if !out.in_view || out.feed_lost {
                    eprintln!("{name}: not listened to (no receiver built here)");
                    break;
                }
            }
            let each_ms = started.elapsed().as_secs_f64() * 1e3 / runs as f64;
            eprintln!(
                "{name}: window {window_us:.0} us, {each_ms:.2} ms a listen ({:.2}x the window)",
                each_ms * 1e3 / window_us
            );
        }
    }
}
