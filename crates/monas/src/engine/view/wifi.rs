// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Wi-Fi, as its views show it: the survey of channels 1 to 13 and which is
//! least busy, the frames heard on the channel the radio sits on, and that
//! channel's airtime (its share, who spent it, and the rhythm of the gaps).
//!
//! What a frame said is worded here, once: the Frames list draws these words
//! and the API serves them. A frame of another channel is only its start,
//! because nothing it said was read; a reading the screen cannot state, the
//! record leaves out.

use std::time::Instant;

use serde::Serialize;

use super::net::Sigma;
use super::{Clock, Group, Reading};
use crate::engine::testimony::{FeedSpan, Staleness, Testimony};
use crate::signal::wifi::format::{Format, Heard};
use crate::signal::wifi::hesig::{HeKind, HePpdu};
use crate::state::{Family, LeastBusy, RowKind, SdrMetrics, WifiRow, READING_COLUMNS};

/// The channels the survey visits.
const CHANNELS: std::ops::RangeInclusive<u8> = 1..=13;

/// A share of the time, by family.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FamilyShare {
    pub family: &'static str,
    /// From 0 to 1.
    pub share: f64,
}

fn by_family(airtime: &[f64; 7]) -> Vec<FamilyShare> {
    Family::ALL
        .iter()
        .zip(airtime)
        .map(|(f, &share)| FamilyShare {
            family: f.label(),
            share,
        })
        .collect()
}

/// One channel of the survey.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Channel {
    pub channel: u8,
    pub busy: Reading<Busy>,
    /// The share of its time its frames stated, by family, over its visits.
    pub airtime: Vec<FamilyShare>,
}

/// A channel's busy share over its visits.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Busy {
    /// The mean share, from 0 to 1.
    pub mean: f64,
    /// The standard error of the mean, once there are enough visits.
    pub sigma: Option<f64>,
    pub visits: usize,
    pub seconds: f64,
}

/// Which channel is least busy, or why none can be named yet.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Verdict {
    /// `first_pass`, `no_spread_yet`, `alone`, `few` or `not_apart`.
    pub status: &'static str,
    /// The quietest channel and those within 2 sigma of it.
    pub channels: Vec<u8>,
    /// Channels visited so far, on the first pass.
    pub visited: Option<usize>,
    /// Passes more that would settle it, where that can be said.
    pub passes: Option<usize>,
}

/// The survey, for its route.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Survey {
    /// Why Wi-Fi's receiver does not run, when it does not.
    pub refused: Option<String>,
    pub channels: Vec<Channel>,
    pub least_busy: Verdict,
}

/// The survey as it stands.
pub fn survey(m: &SdrMetrics, clock: Clock) -> Group<Survey> {
    let s = &m.net.wifi.survey;
    let channels = CHANNELS
        .map(|ch| Channel {
            channel: ch,
            busy: Reading::of(
                s.busy(ch).map(|b| Busy {
                    mean: b.mean,
                    sigma: b.sigma,
                    visits: b.visits,
                    seconds: b.seconds,
                }),
                || "not visited yet".to_string(),
            ),
            airtime: by_family(&s.airtime(ch)),
        })
        .collect();
    let least_busy = match s.least_busy() {
        LeastBusy::FirstPass { visited } => Verdict {
            status: "first_pass",
            channels: vec![],
            visited: Some(visited),
            passes: None,
        },
        LeastBusy::NoSpreadYet { passes } => Verdict {
            status: "no_spread_yet",
            channels: vec![],
            visited: None,
            passes: Some(passes),
        },
        LeastBusy::Alone(ch) => Verdict {
            status: "alone",
            channels: vec![ch],
            visited: None,
            passes: None,
        },
        LeastBusy::Few(chs) => Verdict {
            status: "few",
            channels: chs,
            visited: None,
            passes: None,
        },
        LeastBusy::NotApart { passes } => Verdict {
            status: "not_apart",
            channels: vec![],
            visited: None,
            passes,
        },
    };
    Group {
        testimony: testimony(m, clock),
        readings: Survey {
            refused: m.net.wifi.refused.clone(),
            channels,
            least_busy,
        },
    }
}

/// What a frame's signalling said, worded as the Frames list words it.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Said {
    /// Its format (`HT`, `HE SU`, `11b long`), or for another channel's
    /// frame, how far off it is and what kind (`+5 MHz · OFDM`).
    pub format: String,
    /// `54 Mb/s`, `MCS 7`, where it was read.
    pub rate: Option<String>,
    /// Its length in octets, where it was read.
    pub length_bytes: Option<u32>,
    /// HE and later: the BSS colour and the direction.
    pub network: Option<String>,
}

/// A format's name, as its signalling said it.
pub fn format_name(heard: &Heard) -> &'static str {
    match heard.format {
        None => "unreadable",
        Some(Format::NonHt) => "non-HT",
        Some(Format::HtMixed(_)) => "HT",
        Some(Format::Greenfield(Ok(_))) => "HT GF",
        Some(Format::Greenfield(Err(_))) => "unreadable",
        Some(Format::Vht(_)) => "VHT",
        Some(Format::He { sig_a: Ok(a), .. }) => match a.ppdu {
            HePpdu::Su => "HE SU",
            HePpdu::ErSu => "HE ER SU",
            HePpdu::Mu => "HE MU",
            HePpdu::Tb => "HE TB",
        },
        Some(Format::He { kind, .. }) => match kind {
            HeKind::SuOrTb => "HE",
            HeKind::Mu => "HE MU",
            HeKind::ErSu => "HE ER SU",
        },
        Some(Format::USig { sig: Ok(u), .. }) if u.version == 0 => "EHT",
        Some(Format::USig { sig: Ok(_), .. }) => "after EHT",
        Some(Format::USig { sig: Err(_), .. }) => "unreadable",
    }
}

/// What a row's signalling said: its format, its rate, its length and, for
/// HE and later, its network.
pub fn said(kind: &RowKind) -> Said {
    let preamble = |p| match p {
        crate::signal::wifi::dsss::header::Preamble::Long => "long",
        crate::signal::wifi::dsss::header::Preamble::Short => "short",
    };
    let dsss = |header: &Result<crate::signal::wifi::dsss::header::Header, _>| match header {
        Ok(h) => (Some(format!("{} Mb/s", h.rate.mbps())), Some(h.octets())),
        Err(_) => (None, None),
    };
    let network = |bss: &crate::signal::wifi::hesig::Bss| {
        format!(
            "colour {} {}",
            bss.colour,
            if bss.uplink { "\u{2191}" } else { "\u{2193}" }
        )
    };
    match kind {
        RowKind::Ofdm(heard) => {
            let (rate, length_bytes) = match (heard.format, heard.lsig) {
                (Some(Format::NonHt), Some(Ok(l))) => (
                    Some(format!("{} Mb/s", l.rate.mbps())),
                    Some(u32::from(l.length)),
                ),
                (Some(Format::HtMixed(Ok(h)) | Format::Greenfield(Ok(h))), _) => {
                    (Some(format!("MCS {}", h.mcs)), Some(u32::from(h.length)))
                }
                _ => (None, None),
            };
            let network = match heard.format {
                Some(Format::He { sig_a: Ok(a), .. }) => Some(network(&a.bss)),
                Some(Format::USig { sig: Ok(u), .. }) => Some(network(&u.bss)),
                _ => None,
            };
            Said {
                format: format_name(heard).to_string(),
                rate,
                length_bytes,
                network,
            }
        }
        RowKind::Dsss {
            preamble: p,
            header,
        } => {
            let (rate, length_bytes) = dsss(header);
            Said {
                format: format!("11b {}", preamble(*p)),
                rate,
                length_bytes,
                network: None,
            }
        }
        RowKind::OtherOfdm { shift_mhz } => Said {
            format: format!("{shift_mhz:+} MHz \u{b7} OFDM"),
            ..Said::default()
        },
        RowKind::OtherDsss {
            shift_mhz, header, ..
        } => {
            let (rate, length_bytes) = dsss(header);
            Said {
                format: format!("{shift_mhz:+} MHz \u{b7} 11b"),
                rate,
                length_bytes,
                network: None,
            }
        }
    }
}

/// The frames heard on the radio's centre, newest first. The survey's visits
/// to other channels leave their frames in the ring too; they are those
/// channels' lists, not this one's.
pub fn shown(m: &SdrMetrics) -> Vec<&WifiRow> {
    let centre = m.radio.frequency as f64;
    m.net
        .wifi
        .rows
        .iter()
        .rev()
        .filter(|r| (r.carrier_hz - centre).abs() < 1.0)
        .collect()
}

/// One frame.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Frame {
    /// Its place in the session's arrivals, from 1.
    pub seq: u64,
    /// The stream's clock it was heard on, and the run of it: starts compare
    /// only within one run of one clock.
    pub epoch: u64,
    pub run: u64,
    /// The stream pair its first sample arrived at.
    pub start: u64,
    pub age_s: u64,
    /// The centre of the channel it was sent on: ours, or another's.
    pub centre_hz: f64,
    /// Its family, for a frame of this channel; another channel's is none.
    pub family: Option<&'static str>,
    pub said: Said,
    /// The airtime it stated, where it was read.
    pub airtime_us: Option<u32>,
    /// From the end of the frame before it, where that is known.
    pub gap_us: Option<f64>,
    /// Its carrier against its channel's centre, corrected for our
    /// oscillator as on screen.
    pub offset_khz: Option<Sigma>,
    pub snr_db: Option<f64>,
}

/// The frames, for their route.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Frames {
    pub channel: Option<u8>,
    pub refused: Option<String>,
    /// Frames heard this session, including those the list let go.
    pub heard_session: u64,
    /// Newest first; `since` leaves out those numbered up to it.
    pub frames: Vec<Frame>,
}

/// The frames on the radio's centre, numbered past `since`.
pub fn frames(m: &SdrMetrics, clock: Clock, since: Option<u64>) -> Group<Frames> {
    let now = Instant::now();
    let frames = shown(m)
        .into_iter()
        .filter(|r| since.is_none_or(|s| r.seq > s))
        .map(|r| Frame {
            seq: r.seq,
            epoch: r.epoch,
            run: r.run,
            start: r.start,
            age_s: now.saturating_duration_since(r.when).as_secs(),
            centre_hz: r.centre_hz(),
            family: r.family().map(Family::label),
            said: said(&r.kind),
            airtime_us: r.airtime_us,
            gap_us: r.gap_us,
            offset_khz: Sigma::of(
                r.offset_hz
                    .map(|o| m.radio.transmitter_offset(o, r.centre_hz(), now).khz),
            ),
            snr_db: r.snr_db,
        })
        .collect();
    Group {
        testimony: testimony(m, clock),
        readings: Frames {
            channel: m.net.wifi.channel,
            refused: m.net.wifi.refused.clone(),
            heard_session: m.net.wifi.rows_heard,
            frames,
        },
    }
}

/// The locked channel's share of the time over the last whole seconds.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Share {
    /// The frames' share of the time, mean and standard error.
    pub frames: AirReading,
    /// The energy's busy share.
    pub busy: AirReading,
    pub airtime: Vec<FamilyShare>,
    pub columns: usize,
    pub heard_s: f64,
    pub span_s: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct AirReading {
    pub mean: f64,
    pub sigma: Option<f64>,
}

/// Who spent the channel's airtime, by sender and rate.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Spent {
    /// The spender and its rate, as the ledger names them.
    pub who: String,
    pub frames: usize,
    /// Frames that stated their length.
    pub stated: usize,
    pub air_us: f64,
    /// The share of the heard time.
    pub share: f64,
    pub per_s: f64,
    pub mean_us: Option<f64>,
}

/// The gaps between frames, binned, as the rhythm strip draws them.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RhythmView {
    pub bins: Vec<u32>,
    pub under: u32,
    pub past: u32,
    pub corrected: u32,
    pub unplaced: u32,
}

/// The airtime views, for their route.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Airtime {
    /// The channel the radio is locked on, when it is.
    pub channel: Option<u8>,
    pub share: Reading<Share>,
    pub ledger: Reading<Vec<Spent>>,
    pub rhythm: Reading<RhythmView>,
}

/// The channel the radio is locked on, when it is.
pub fn locked(m: &SdrMetrics) -> Option<u8> {
    (m.net.mode == crate::state::NetMode::Lock)
        .then(|| crate::signal::net::band::wifi_channel(m.radio.frequency))
        .flatten()
}

/// The locked channel's airtime: its share, its ledger, its rhythm.
pub fn airtime(m: &SdrMetrics, clock: Clock) -> Group<Airtime> {
    let ch = locked(m);
    let centre = m.radio.frequency as f64;
    let not_locked = || "not locked on a Wi-Fi channel".to_string();
    let share = match ch {
        None => Reading::Absent(not_locked()),
        Some(ch) => Reading::of(
            m.net
                .wifi
                .survey
                .share(ch, Instant::now(), READING_COLUMNS)
                .map(|r| Share {
                    frames: AirReading {
                        mean: r.frames.mean,
                        sigma: r.frames.sigma,
                    },
                    busy: AirReading {
                        mean: r.busy.mean,
                        sigma: r.busy.sigma,
                    },
                    airtime: by_family(&r.airtime),
                    columns: r.columns,
                    heard_s: r.heard_s,
                    span_s: r.span_s,
                }),
            || format!("no whole second heard yet in the last {READING_COLUMNS}"),
        ),
    };
    let ledger = match ch {
        None => Reading::Absent(not_locked()),
        Some(_) => Reading::of(
            m.net.wifi.ledger(centre).map(|l| {
                l.folded()
                    .iter()
                    .map(|row| Spent {
                        who: row.label(),
                        frames: row.frames,
                        stated: row.stated,
                        air_us: row.air_us,
                        share: l.share(row),
                        per_s: l.per_s(row),
                        mean_us: row.mean_us(),
                    })
                    .collect()
            }),
            || "no frame heard here yet".to_string(),
        ),
    };
    let rhythm = match ch {
        None => Reading::Absent(not_locked()),
        Some(_) => Reading::of(
            m.net.wifi.rhythm(centre).map(|r| RhythmView {
                bins: r.bins.to_vec(),
                under: r.under,
                past: r.past,
                corrected: r.corrected,
                unplaced: r.unplaced,
            }),
            || "no frame heard here yet".to_string(),
        ),
    };
    Group {
        testimony: testimony(m, clock),
        readings: Airtime {
            channel: ch,
            share,
            ledger,
            rhythm,
        },
    }
}

fn testimony(m: &SdrMetrics, clock: Clock) -> Testimony {
    Testimony::resolve(
        m,
        Staleness::NotStreaming,
        Some(FeedSpan::Session),
        m.net.health.last_block.map(|t| clock.unix_ms_of(t)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clock() -> Clock {
        Clock {
            now: Instant::now(),
            unix_ms: 1_788_632_561_000,
        }
    }

    /// Before any visit, every channel says it was not visited, and the
    /// verdict says the first pass is still running.
    #[test]
    fn an_unvisited_survey_says_so() {
        let g = survey(&SdrMetrics::fixture(), clock());
        assert_eq!(g.readings.channels.len(), 13);
        assert!(g
            .readings
            .channels
            .iter()
            .all(|c| c.busy.absent() == Some("not visited yet")));
        assert_eq!(g.readings.least_busy.status, "first_pass");
    }

    /// Off a Wi-Fi channel, the airtime views say why they have nothing.
    #[test]
    fn airtime_off_a_channel_says_why() {
        let g = airtime(&SdrMetrics::fixture(), clock());
        assert_eq!(g.readings.channel, None);
        assert_eq!(
            g.readings.share.absent(),
            Some("not locked on a Wi-Fi channel")
        );
        assert!(frames(&SdrMetrics::fixture(), clock(), None)
            .readings
            .frames
            .is_empty());
    }

    /// Another channel's frame is only how far off it is and its kind.
    #[test]
    fn another_channels_frame_says_only_where_it_was() {
        let s = said(&RowKind::OtherOfdm { shift_mhz: -5 });
        assert_eq!(s.format, "-5 MHz \u{b7} OFDM");
        assert_eq!((s.rate, s.length_bytes, s.network), (None, None, None));
    }
}
