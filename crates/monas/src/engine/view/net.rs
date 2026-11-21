// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The 2.4 GHz band: what the receivers are doing and how their feed is
//! holding up, the band's occupancy one megahertz at a time, and the census
//! of transmitters heard.
//!
//! The band and the census are the CSV exports' own accounts: each is built
//! here once, and the export formats it as text while the API serves it as
//! JSON, so the two cannot differ. **Nothing the panel would dash**, in
//! either: a megahertz nobody looked at is unobserved, not zero; a duty
//! cycle whose uncertainty is wider than its resolution has no value, its
//! uncertainty still stated; a floor that failed its preconditions takes
//! the whole band with it; a mean from one packet has no figure.
//!
//! Every count here is a lower bound once the feed has lost samples, so
//! each group's testimony says whether a loss falls inside the span its
//! numbers cover, by the same rule as the screen's `[FEED LOSS]`.

use std::time::{Duration, Instant};

use serde::Serialize;

use super::{Clock, Group, Reading};
use crate::engine::testimony::{FeedSpan, Staleness, Testimony};
use crate::signal::ble::interval::{Delay, Grid, Refusal};
use crate::signal::dsp::uncertainty::Uncertain;
use crate::signal::net::census::Device as CensusDevice;
use crate::signal::net::occupancy;
use crate::state::SdrMetrics;

/// A value and its standard uncertainty.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Sigma {
    pub value: f64,
    pub sigma: f64,
}

impl Sigma {
    /// The reading as a value with its sigma, where both can be stated: a
    /// mean from one packet has an infinite sigma, and no figure.
    pub fn of(u: Option<Uncertain>) -> Option<Sigma> {
        u.filter(|u| u.value().is_finite() && u.sigma().is_finite())
            .map(|u| Sigma {
                value: u.value(),
                sigma: u.sigma(),
            })
    }
}

/// The receivers' summary, for the snapshot.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Net {
    /// `survey`, `lock` and the like, as the panels' tag says it.
    pub mode: String,
    /// Why no survey runs where the mode says one should.
    pub survey_refused: Option<String>,
    /// How addresses are shown: in full, by registrant, or masked.
    pub addresses: &'static str,
    pub devices: usize,
    pub ble_packets_heard: u64,
    pub coded_packets_heard: u64,
    pub feed: NetFeed,
}

/// The NET feed's account.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct NetFeed {
    pub blocks_in: u64,
    pub pairs_in: u64,
    /// Breaks in the stream the worker saw, and blocks lost inside them.
    pub gaps: u64,
    pub blocks_lost: u64,
    /// Blocks the feed had no room for, lately and over the session.
    pub refused: u64,
    pub refused_session: u64,
    /// The decoders' time per block over the stream time it covered.
    pub decode_load: Option<f64>,
    /// Seconds since the feed last lost anything, if it ever has.
    pub last_loss_s: Option<f64>,
}

/// The receivers' group. Stale when the radio is not streaming; its counts
/// span the session, so any loss there has been caveats them.
pub fn net(m: &SdrMetrics, clock: Clock) -> Group<Net> {
    let n = &m.net;
    let h = &n.health;
    Group {
        testimony: Testimony::resolve(
            m,
            Staleness::NotStreaming,
            Some(FeedSpan::Session),
            h.last_block.map(|t| clock.unix_ms_of(t)),
        ),
        readings: Net {
            mode: n.mode.label().to_string(),
            survey_refused: n.survey_refused.clone(),
            addresses: n.address_display.label(),
            devices: n.census.devices.len(),
            ble_packets_heard: n.ble_heard,
            coded_packets_heard: n.coded_heard,
            feed: NetFeed {
                blocks_in: h.blocks_in,
                pairs_in: h.pairs_in,
                gaps: h.gaps,
                blocks_lost: h.blocks_lost,
                refused: h.refused,
                refused_session: h.refused_session,
                decode_load: h.decode_load,
                last_loss_s: h
                    .last_loss
                    .map(|t| clock.now.saturating_duration_since(t).as_secs_f64()),
            },
        },
    }
}

/// The band's occupancy, one megahertz to a cell.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Band {
    /// The noise floor every cell was measured against, dBFS.
    pub noise_dbfs: Option<f64>,
    /// The length of one measuring window, seconds.
    pub window_s: f64,
    pub cells: Vec<Cell>,
}

/// One megahertz of the band.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Cell {
    pub mhz: f64,
    /// Whether anyone looked: an unobserved cell has no figures at all,
    /// which is not the same as a quiet one.
    pub observed: bool,
    /// The share of windows something was on, where its uncertainty is
    /// within the resolution it is shown at.
    pub duty_pct: Option<f64>,
    /// The duty cycle's uncertainty, stated even when the value is not.
    pub duty_sigma_pct: Option<f64>,
    /// The share of the cell's time it was watched.
    pub coverage_pct: Option<f64>,
    pub mean_dbfs: Option<f64>,
    pub peak_dbfs: Option<f64>,
    pub windows: u64,
}

/// How long ago the oldest measured cell was measured: the span the band's
/// figures cover, a whole pass while surveying, moments while locked.
/// `None` when no cell has been measured.
pub fn oldest_reading_age(m: &SdrMetrics) -> Option<Duration> {
    let oldest: Instant = m.net.band.cells.iter().filter_map(|c| c.measured).min()?;
    Some(oldest.elapsed())
}

/// The band, or why there is none to give.
pub fn band_record(m: &SdrMetrics) -> Result<Band, String> {
    let band = &m.net.band;
    if !m.mode.net.survey {
        return Err(
            "the band is measured on NET 2 Survey only; export from there for a current one"
                .to_string(),
        );
    }
    if band.cells.is_empty() {
        return Err("no band measurement yet".to_string());
    }
    if !band.trusted {
        return Err(format!(
            "the noise floor failed its preconditions (lower tail {:.2}, decile spread {:.1}), \
             so nothing here was measured against a floor",
            band.tail, band.spread
        ));
    }
    let cells = band
        .cells
        .iter()
        .enumerate()
        .map(|(cell, c)| {
            let mhz = occupancy::cell_centre_hz(cell) as f64 / 1e6;
            if !c.observed() {
                return Cell {
                    mhz,
                    observed: false,
                    duty_pct: None,
                    duty_sigma_pct: None,
                    coverage_pct: None,
                    mean_dbfs: None,
                    peak_dbfs: None,
                    windows: 0,
                };
            }
            let duty = occupancy::duty_uncertain(c.duty, c.windows).scale(100.0);
            // The question the panel asks before it prints a number, asked
            // of the same `Uncertain`.
            Cell {
                mhz,
                observed: true,
                duty_pct: duty
                    .is_resolved(occupancy::DUTY_RESOLUTION * 100.0)
                    .then(|| duty.value()),
                duty_sigma_pct: duty.sigma().is_finite().then(|| duty.sigma()),
                coverage_pct: c.coverage.map(|f| f * 100.0),
                mean_dbfs: Some(c.mean_dbfs),
                peak_dbfs: Some(c.peak_dbfs),
                windows: c.windows,
            }
        })
        .collect();
    Ok(Band {
        noise_dbfs: band.noise_dbfs,
        window_s: band.window_s,
        cells,
    })
}

/// The band's group. Stale when the radio is not streaming; a feed loss
/// caveats it only if it falls inside the span its cells cover.
pub fn band(m: &SdrMetrics, clock: Clock) -> Group<Reading<Band>> {
    Group {
        testimony: Testimony::resolve(
            m,
            Staleness::NotStreaming,
            oldest_reading_age(m).map(FeedSpan::Window),
            m.net.band.last_column.map(|t| clock.unix_ms_of(t)),
        ),
        readings: match band_record(m) {
            Ok(b) => Reading::Value(b),
            Err(why) => Reading::Absent(why),
        },
    }
}

/// One transmitter of the census, every column the panel shows.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Device {
    /// As the addresses are shown: in full, by registrant, or masked.
    pub address: String,
    pub kind: &'static str,
    /// What it advertised about itself, where it has.
    pub name: Option<String>,
    pub name_complete: Option<bool>,
    pub tx_power_dbm: Option<i8>,
    pub company_id: Option<u16>,
    pub company: Option<&'static str>,
    pub packets: u64,
    pub crc_failed: u64,
    pub crc_pass_pct: f64,
    pub best_snr_db: Option<f32>,
    pub mean_snr_db: Option<Sigma>,
    /// Corrected for our oscillator as the panel shows it; the provenance's
    /// reference says what that is worth.
    pub crystal_offset_ppm: Option<Sigma>,
    pub modulation_index: Option<Sigma>,
    pub pdu_types: Vec<String>,
    pub advertising: Advertising,
    pub first_seen_s: u64,
    pub last_seen_s: u64,
}

/// The advertising timing as read in LOCK, or why there is none.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Advertising {
    /// `estimated`, or why not: `not timed in LOCK`, `collecting 3 of 8`, ...
    pub status: String,
    pub interval_ms: Option<Sigma>,
    pub events: Option<u64>,
    pub channel: Option<u8>,
    /// `none`, `uniform` or `not uniform`.
    pub delay: Option<&'static str>,
    pub delay_width_ms: Option<f64>,
    /// `on`, `off` or `cannot tell`.
    pub grid: Option<&'static str>,
    pub grid_steps: Option<String>,
    pub grid_off_ms: Option<f64>,
}

fn advertising(d: &CensusDevice) -> Advertising {
    let status = |text: String| Advertising {
        status: text,
        interval_ms: None,
        events: None,
        channel: None,
        delay: None,
        delay_width_ms: None,
        grid: None,
        grid_steps: None,
        grid_off_ms: None,
    };
    let e = match d.advertising() {
        None => return status("not timed in LOCK".to_string()),
        Some(Err(Refusal::Collecting { have, need })) => {
            return status(format!("collecting {have} of {need}"))
        }
        Some(Err(Refusal::BelowMinimum(_))) => {
            return status("faster than any legacy interval".to_string())
        }
        Some(Err(Refusal::NoSingleEvents(_))) => {
            return status("no two consecutive events".to_string())
        }
        Some(Ok(e)) => e,
    };
    let (shape, width) = match e.delay {
        Delay::Absent { width_s } => ("none", width_s),
        Delay::Spread {
            width_s,
            uniform: true,
            ..
        } => ("uniform", width_s),
        Delay::Spread { width_s, .. } => ("not uniform", width_s),
    };
    let (grid, steps, off) = match e.grid {
        Grid::On(n) => ("on", Some(n.to_string()), None),
        Grid::Off { n, by_s } => ("off", Some(n.to_string()), Some(by_s * 1e3)),
        Grid::CannotTell => ("cannot tell", None, None),
    };
    Advertising {
        status: "estimated".to_string(),
        interval_ms: Sigma::of(Some(e.interval_s.scale(1e3))),
        events: Some(e.events as u64),
        channel: d.arrivals.as_ref().map(|a| a.channel),
        delay: Some(shape),
        delay_width_ms: Some(width * 1e3),
        grid: Some(grid),
        grid_steps: steps,
        grid_off_ms: off,
    }
}

/// The census, in the order the panel is showing it, so a file and the
/// screen it was taken from read side by side.
pub fn census_records(m: &SdrMetrics) -> Vec<Device> {
    let now = Instant::now();
    m.net
        .census
        .ordered(now, &m.radio)
        .iter()
        .map(|d| {
            let said = m.net.advertised.get(&d.address);
            let company = said.and_then(|a| a.company);
            let name = said.and_then(|a| a.name.as_ref());
            Device {
                address: d.address_text(&m.net, None),
                kind: d.kind().label(),
                name: name.map(|(text, _)| m.net.show_name(text)),
                name_complete: name.map(|(_, complete)| *complete),
                tx_power_dbm: said.and_then(|a| a.tx_power_dbm),
                company_id: company,
                company: company.and_then(crate::signal::assigned::company),
                packets: d.packets,
                crc_failed: d.crc_failed,
                crc_pass_pct: d.crc_pass_rate() * 100.0,
                best_snr_db: d.best_snr_db,
                mean_snr_db: Sigma::of(d.mean_snr_db()),
                crystal_offset_ppm: Sigma::of(
                    d.crystal_offset_ppm
                        .map(|u| m.radio.corrected_ppm(u, now).0),
                ),
                modulation_index: Sigma::of(d.modulation_index),
                pdu_types: d
                    .ble_pdu_codes()
                    .map(crate::signal::net::census::pdu_type_name)
                    .collect(),
                advertising: advertising(d),
                first_seen_s: now.saturating_duration_since(d.first_seen).as_secs(),
                last_seen_s: now.saturating_duration_since(d.last_seen).as_secs(),
            }
        })
        .collect()
}

/// The census, for its own route.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Census {
    pub devices: Vec<Device>,
}

/// The census's group: its counts span the session.
pub fn census(m: &SdrMetrics, clock: Clock) -> Group<Census> {
    Group {
        testimony: Testimony::resolve(
            m,
            Staleness::NotStreaming,
            Some(FeedSpan::Session),
            m.net.health.last_block.map(|t| clock.unix_ms_of(t)),
        ),
        readings: Census {
            devices: census_records(m),
        },
    }
}
