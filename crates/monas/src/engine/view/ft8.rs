// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! FT8: the decoder's state for the snapshot, and the decodes it holds,
//! each with what it measured beside what it said.
//!
//! One account of a decode, built here and read by both the CSV export
//! and the API: the same fields, blank (here `None`) where the file is
//! blank, for the same reasons. A message with no locator has none; a
//! station that never sent one has no place, distance or bearing; without
//! your position there is no distance; OSD not needed is no order, not
//! zero; a cycle gone from the hour has no `held`, and one held, changed or
//! refused has no floor.

use serde::Serialize;

use super::pager::Feed;
use super::{Group, Reading};
use crate::engine::testimony::{Staleness, Testimony};
use crate::geo::{maidenhead, sphere};
use crate::state::ft8::CycleOutcome;
use crate::state::SdrMetrics;

/// The decoder's summary, for the snapshot.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Ft8 {
    /// Whether a cycle is being decoded now.
    pub decoding: bool,
    pub decoded_session: u64,
    /// Decodes held, at most the newest few hundred.
    pub held: usize,
    /// The clock as the band hears it: the decodes' median time offset.
    pub clock: Reading<ClockVerdict>,
    pub last_cycle: Reading<Cycle>,
    pub feed: Feed,
}

/// The clock as heard: the median of the decodes' DT, their spread, and
/// how many it rests on.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct ClockVerdict {
    pub median_dt_s: f64,
    pub spread_s: f64,
    pub decodes: usize,
}

/// One 15 s cycle and what became of it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Cycle {
    pub start_unix: f64,
    pub dial_hz: u64,
    /// `decoded`, `cut` (the stream broke or was retuned inside it) or
    /// `skipped` (the decoder was still busy with the one before).
    pub outcome: &'static str,
    pub decodes: usize,
    pub candidates: usize,
    pub malformed: usize,
    /// How long the decode took.
    pub seconds: f64,
    pub floor_db_hz: Option<f32>,
}

/// The decodes, for their own route.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Decodes {
    /// Oldest first; `since` leaves out the ones numbered up to it.
    pub decodes: Vec<Decode>,
    /// What the list cannot hold or no longer holds, when that is anything.
    pub note: Option<String>,
}

/// One decode: what it said, and what it measured.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Decode {
    /// Its number this session, rising.
    pub n: u64,
    pub cycle_unix: f64,
    /// `even` or `odd`.
    pub slot: &'static str,
    pub band: Option<&'static str>,
    pub dial_hz: u64,
    pub tone_hz: f64,
    pub tone_sigma_hz: f64,
    pub snr_db: f64,
    pub dt_s: f64,
    pub drift_hz: f64,
    pub coherence: f64,
    /// Costas tones on their tone, of 21.
    pub sync_of_21: u8,
    pub ldpc_iterations: u32,
    pub bits_repaired: u32,
    pub osd_order: Option<u8>,
    pub block: u8,
    pub pass: u8,
    pub message: String,
    pub from: Option<String>,
    pub to: Vec<String>,
    pub for_me: bool,
    /// The locator this message carried.
    pub locator: Option<String>,
    /// The last locator the sender sent itself, which the distance is from.
    pub station_locator: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub km: Option<f64>,
    pub bearing_deg: Option<f64>,
    /// The clock verdict the cycle was read with.
    pub clock: Option<ClockVerdict>,
    /// The least margin a data symbol was decided by.
    pub margin_min_db: f32,
    /// Whether the radio held its level in the cycle.
    pub held: Option<bool>,
    pub floor_db_hz: Option<f32>,
    /// `start` or `end`: the part of the transmission read as unknown.
    pub unread: Option<&'static str>,
}

/// Every decode held, oldest first.
pub fn decode_list(state: &SdrMetrics) -> Vec<Decode> {
    let f = &state.ft8;
    let home = state.home.position();
    f.decodes
        .iter()
        .map(|e| {
            let d = &e.decode;
            let p = &d.physics;
            let msg = &d.message;
            let from = msg.sender();
            let station_locator = from
                .and_then(|c| f.stations.get(c))
                .and_then(|s| s.locator.clone());
            let place = station_locator.as_deref().and_then(maidenhead::centre);
            let path = home
                .zip(place)
                .map(|(h, pl)| (sphere::distance_km(h, pl), sphere::bearing_deg(h, pl)));
            let clock = f
                .cycles
                .iter()
                .find(|c| c.start_unix == e.cycle_unix)
                .and_then(|c| c.clock)
                .map(|c| ClockVerdict {
                    median_dt_s: c.median_dt,
                    spread_s: c.spread,
                    decodes: c.decodes,
                });
            // The hour's mark of the cycle: whether the radio held its level
            // in it, and the noise under the band.
            let mark = f.cycle_hour.iter().find(|c| c.start_unix == e.cycle_unix);
            Decode {
                n: e.n,
                cycle_unix: e.cycle_unix,
                slot: if (e.cycle_unix / 15.0).round() as i64 % 2 == 0 {
                    "even"
                } else {
                    "odd"
                },
                band: crate::signal::ft8::bands::band_of(e.dial_hz),
                dial_hz: e.dial_hz,
                tone_hz: p.f0,
                tone_sigma_hz: p.f0_sigma,
                snr_db: p.snr_db,
                dt_s: d.dt,
                drift_hz: p.drift,
                coherence: p.coherence,
                sync_of_21: d.costas,
                ldpc_iterations: d.iterations,
                bits_repaired: d.repaired,
                osd_order: d.osd,
                block: d.block,
                pass: d.pass,
                message: msg.text(),
                from: from.map(str::to_string),
                to: msg.addressees().into_iter().map(str::to_string).collect(),
                for_me: e.for_you,
                locator: msg.locator().map(str::to_string),
                station_locator,
                lat: place.map(|(lat, _)| lat),
                lon: place.map(|(_, lon)| lon),
                km: path.map(|(km, _)| km),
                bearing_deg: path.map(|(_, b)| b),
                clock,
                margin_min_db: d.trace.least_margin_db(),
                held: mark.map(|c| c.held),
                floor_db_hz: mark.and_then(|c| c.floor_db_hz),
                unread: d.unread.map(|u| match u {
                    crate::signal::ft8::demod::Unread::Start => "start",
                    crate::signal::ft8::demod::Unread::End => "end",
                }),
            }
        })
        .collect()
}

/// The decoder's group for the snapshot. Stale when the radio is not
/// streaming; dated by the newest cycle's start.
pub fn ft8(m: &SdrMetrics) -> Group<Ft8> {
    let f = &m.ft8;
    Group {
        testimony: testimony(m),
        readings: Ft8 {
            decoding: f.decoding,
            decoded_session: f.decoded,
            held: f.decodes.len(),
            clock: Reading::of(
                f.clock.map(|c| ClockVerdict {
                    median_dt_s: c.median_dt,
                    spread_s: c.spread,
                    decodes: c.decodes,
                }),
                || "too few decodes to say how the clock is heard".to_string(),
            ),
            last_cycle: Reading::of(
                f.cycles.back().map(|c| Cycle {
                    start_unix: c.start_unix,
                    dial_hz: c.dial_hz,
                    outcome: match c.outcome {
                        CycleOutcome::Decoded => "decoded",
                        CycleOutcome::Cut => "cut",
                        CycleOutcome::Skipped => "skipped",
                    },
                    decodes: c.decodes,
                    candidates: c.candidates,
                    malformed: c.malformed,
                    seconds: c.seconds,
                    floor_db_hz: c.floor_db_hz,
                }),
                || "no cycle decoded yet".to_string(),
            ),
            feed: Feed::of(&f.feed),
        },
    }
}

/// The decodes numbered past `since`, with the decoder's testimony.
pub fn decodes(m: &SdrMetrics, since: Option<u64>) -> Group<Decodes> {
    Group {
        testimony: testimony(m),
        readings: Decodes {
            decodes: decode_list(m)
                .into_iter()
                .filter(|d| since.is_none_or(|s| d.n > s))
                .collect(),
            note: crate::export::ft8::note(m),
        },
    }
}

fn testimony(m: &SdrMetrics) -> Testimony {
    let newest = m
        .ft8
        .cycles
        .back()
        .map(|c| (c.start_unix * 1000.0).max(0.0) as u64);
    Testimony::resolve(m, Staleness::NotStreaming, None, newest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn before_the_first_cycle_the_decoder_says_so() {
        let g = ft8(&SdrMetrics::fixture());
        assert_eq!(g.readings.last_cycle.absent(), Some("no cycle decoded yet"));
        assert!(g.readings.clock.absent().is_some());
        assert!(g.testimony.stale);
        let d = decodes(&SdrMetrics::fixture(), None);
        assert!(d.readings.decodes.is_empty());
        assert_eq!(
            d.readings.note.as_deref(),
            Some("nothing has been decoded this session")
        );
    }
}
