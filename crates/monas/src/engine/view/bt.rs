// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The classic Bluetooth hits the section keeps, and the frame error rate
//! against SNR: the BR/EDR and FER exports' own accounts, built here once,
//! written as text by the exports and served as JSON by the API.
//!
//! A slot residual only for a hit its piconet's grid was fitted over, on the
//! same stream: beyond it, it would be an extrapolation or a time on another
//! clock. Header fields only where the header was read under a resolved UAP.
//! A frame error rate only where the panel draws one: below the minimum
//! count a bin keeps its counts and has no rate.

use serde::Serialize;

use super::net::Sigma;
use super::{Clock, Group};
use crate::engine::testimony::{FeedSpan, Staleness, Testimony};
use crate::signal::ble::fer::{edges, FerCurve};
use crate::signal::bt::header::PacketType;
use crate::signal::bt::piconet::{HeaderRead, Inquiry, Kind};
use crate::signal::dsp::uncertainty::Uncertain;
use crate::state::SdrMetrics;

/// One classic Bluetooth hit.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Hit {
    pub age_s: u64,
    /// The hit's time on the stream's sample clock, microseconds: comparable
    /// only within one run of the stream.
    pub stream_us: f64,
    pub channel: u8,
    /// As the address mode allows; an inquiry code in hex, being no one's.
    pub lap: String,
    /// `piconet`, `paged`, or the inquiry code's name (`GIAC`, ...).
    pub lap_kind: &'static str,
    /// Once it is one value.
    pub uap: Option<String>,
    pub uap_candidates: Option<usize>,
    pub slot_residual_us: Option<f64>,
    /// The piconet's slot clock against ours, from the same fit, corrected
    /// as the reference allows.
    pub clock_ppm: Option<Sigma>,
    /// `decoded`, `none`, `not read: UAP unresolved` or `did not decode`.
    pub header: &'static str,
    pub lt_addr: Option<u8>,
    pub packet_type: Option<String>,
    pub flow: Option<u8>,
    pub arqn: Option<u8>,
    pub seqn: Option<u8>,
}

/// The hits, for their route.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Hits {
    /// Where the header fields come from, and whether the session heard more
    /// than it keeps.
    pub note: String,
    /// Oldest first.
    pub hits: Vec<Hit>,
}

/// The hits kept, oldest first.
pub fn hit_records(state: &SdrMetrics) -> Vec<Hit> {
    let now = std::time::Instant::now();
    state
        .net
        .bt_hops
        .iter()
        .rev()
        .map(|h| {
            let uaps = state.net.bt_uap.get(&h.lap);
            let piconet = state.net.bt_piconets.iter().find(|p| p.lap == h.lap);
            let fit = piconet
                .filter(|p| p.slots_stream == h.stream)
                .and_then(|p| p.slots.as_ref())
                .and_then(|s| s.as_ref().ok());
            let clock = fit.map(|fit| {
                let raw = Uncertain::from_sigma(-fit.rate_ppm, fit.rate_sigma_ppm);
                let c = state.radio.corrected_ppm(raw, now).0;
                Sigma {
                    value: c.value(),
                    sigma: c.sigma(),
                }
            });
            let (header, fields) = match h.header {
                None => ("none", None),
                Some(HeaderRead::Unresolved) => ("not read: UAP unresolved", None),
                Some(HeaderRead::Undecoded) => ("did not decode", None),
                Some(HeaderRead::Decoded(hd)) => ("decoded", Some(hd)),
            };
            Hit {
                age_s: now.saturating_duration_since(h.seen).as_secs(),
                stream_us: h.at_us,
                channel: h.channel,
                lap: match Inquiry::of(h.lap) {
                    Some(_) => format!("{:#08x}", h.lap),
                    None => state.net.show_lap(h.lap),
                },
                lap_kind: match piconet.map(|p| p.kind()) {
                    Some(Kind::Inquiry(i)) => i.short(),
                    Some(k) => k.word(),
                    None => Inquiry::of(h.lap).map_or("piconet", |i| i.short()),
                },
                uap: match uaps.map(|u| u.as_slice()) {
                    Some([one]) => Some(state.net.show_uap(*one)),
                    _ => None,
                },
                uap_candidates: uaps.map(|u| u.len()),
                slot_residual_us: fit.and_then(|fit| fit.residual_at(h.at_us)),
                clock_ppm: clock,
                header,
                lt_addr: fields.map(|hd| hd.lt_addr),
                packet_type: fields.map(|hd| PacketType::from_code(hd.packet_type.code()).shown()),
                flow: fields.map(|hd| hd.flags & 1),
                arqn: fields.map(|hd| hd.flags >> 1 & 1),
                seqn: fields.map(|hd| hd.flags >> 2 & 1),
            }
        })
        .collect()
}

/// The hits' group: their counts span the session.
pub fn hits(m: &SdrMetrics, clock: Clock) -> Group<Hits> {
    Group {
        testimony: Testimony::resolve(
            m,
            Staleness::NotStreaming,
            Some(FeedSpan::Session),
            m.net.health.last_block.map(|t| clock.unix_ms_of(t)),
        ),
        readings: Hits {
            note: crate::export::bt::note(m),
            hits: hit_records(m),
        },
    }
}

/// One SNR bin of a frame error curve.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Bin {
    /// `all`, or a census device's address as the address mode shows it.
    pub scope: String,
    pub snr_from_db: Option<f64>,
    pub snr_to_db: Option<f64>,
    pub packets: u32,
    pub crc_failed: u32,
    /// Only where the panel draws a rate.
    pub fer_pct: Option<Sigma>,
}

/// The curves, for their route.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Fer {
    /// What the counts are, and why a device's rate is a ceiling.
    pub note: &'static str,
    pub bins: Vec<Bin>,
}

fn curve_bins(scope: &str, curve: &FerCurve) -> Vec<Bin> {
    let Some(span) = curve.span() else {
        return Vec::new();
    };
    span.filter(|&b| curve.packets(b) > 0)
        .map(|b| {
            let (from, to) = edges(b);
            Bin {
                scope: scope.to_string(),
                snr_from_db: from,
                snr_to_db: to,
                packets: curve.packets(b),
                crc_failed: curve.failed[b],
                fer_pct: curve.rate(b).map(|r| Sigma {
                    value: r.value() * 100.0,
                    sigma: r.sigma() * 100.0,
                }),
            }
        })
        .collect()
}

/// The section's curve, then each device's, in the census's order.
pub fn fer_bins(state: &SdrMetrics) -> Vec<Bin> {
    let now = std::time::Instant::now();
    let mut out = curve_bins("all", &state.net.fer);
    for d in state.net.census.ordered(now, &state.radio) {
        out.extend(curve_bins(&d.address_text(&state.net, None), &d.fer));
    }
    out
}

/// The curves' group: they span the session.
pub fn fer(m: &SdrMetrics, clock: Clock) -> Group<Fer> {
    Group {
        testimony: Testimony::resolve(
            m,
            Staleness::NotStreaming,
            Some(FeedSpan::Session),
            m.net.health.last_block.map(|t| clock.unix_ms_of(t)),
        ),
        readings: Fer {
            note: crate::export::fer::NOTE,
            bins: fer_bins(m),
        },
    }
}
