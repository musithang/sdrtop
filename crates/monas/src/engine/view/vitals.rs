// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The pipeline's vital signs, as the Hardware Vitals bench reads them:
//! samples lost and whose loss it was, the converter's saturation, what the
//! process costs, the link's traffic against its ceiling, and the spectrum
//! feed's queue.

use serde::Serialize;

use super::{Clock, Group, Reading};
use crate::engine::testimony::{Staleness, Testimony};
use crate::state::{RadioDropAccount, SdrMetrics};

/// The vitals group's readings.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Vitals {
    pub drops_per_s: Reading<u64>,
    /// Samples lost this session, and whose loss each was.
    pub lost_session: Reading<Lost>,
    /// What the radio dropped in itself before anything reached the host,
    /// where it keeps such a count: no hole in the stream shows these.
    pub radio_drops: Reading<RadioDrops>,
    pub adc_saturation_pct: Reading<f32>,
    pub adc_saturation_peak_pct: Reading<f32>,
    pub cpu_pct: f32,
    pub rss_mb: u64,
    pub usb_errors_session: u64,
    pub throughput: Reading<Throughput>,
    /// How full the spectrum feed's queue got, at its fullest lately.
    pub fft_queue_peak_pct: Reading<f32>,
    /// Blocks the spectrum feed had no room for: frames never drawn.
    pub fft_blocks_dropped: Reading<Dropped>,
}

/// Samples lost: by the driver before the engine saw them, or by the
/// engine's own intake.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Lost {
    pub all: u64,
    pub driver: u64,
    pub monas: u64,
}

/// The radio's own count of what it dropped since the stream started.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct RadioDrops {
    pub stream: u32,
    /// The longest of them, in milliseconds of samples.
    pub longest_ms: f64,
}

/// The link's traffic: on the bus, where the radio's wire format is known,
/// else what was delivered; and the ceiling the radio's top rate would need.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Throughput {
    /// `bus` or `delivered`.
    pub what: &'static str,
    pub mb_s: f64,
    pub ceiling_mb_s: f64,
    /// `mb_s` over the ceiling, from 0 to 1.
    pub utilisation: f64,
}

/// A count lately and over the session.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Dropped {
    pub recent: u64,
    pub session: u64,
}

/// The binary megabytes a second a radio at `sample_rate_max_hz` sends with
/// `bytes_per_pair` bytes per sample pair: the link's ceiling for it, an
/// honest per-device reference rather than a magic constant.
///
/// `bytes_per_pair` is the device's own geometry: a 16-bit radio moves twice
/// the bytes at the same rate, and telling it otherwise puts it comfortably
/// inside a budget it is sitting on the edge of.
pub fn link_ceiling_mbps(sample_rate_max_hz: f64, bytes_per_pair: f64) -> f64 {
    (sample_rate_max_hz * bytes_per_pair) / (1024.0 * 1024.0)
}

/// The link's traffic and ceiling, figures and all, whether or not the
/// radio is streaming: [`vitals_readings`] decides whether it is shown.
pub fn throughput(m: &SdrMetrics) -> Throughput {
    let delivered = m.timing.throughput_mean_mbps;
    let per_pair = m.caps.sample_geometry.bytes_per_pair() as f64;
    let (what, mb_s, bytes) = match m.timing.wire_bytes_per_pair.filter(|&w| w > 0.0) {
        Some(w) => ("bus", delivered * w / per_pair, w),
        None => ("delivered", delivered, per_pair),
    };
    let ceiling_mb_s = link_ceiling_mbps(m.caps.sample_rate_max_hz, bytes);
    let utilisation = if ceiling_mb_s > 0.0 {
        (mb_s / ceiling_mb_s).clamp(0.0, 1.0)
    } else {
        0.0
    };
    Throughput {
        what,
        mb_s,
        ceiling_mb_s,
        utilisation,
    }
}

/// The radio's own drop count as a reading: absent where it keeps none, or
/// where it could not be read, with why.
pub fn radio_drops(m: &SdrMetrics) -> Reading<RadioDrops> {
    match &m.signal.radio_drops {
        RadioDropAccount::NotKept => Reading::Absent("this radio keeps no count of its own".into()),
        RadioDropAccount::Unreadable(why) => Reading::Absent(why.clone()),
        RadioDropAccount::Counted {
            stream,
            longest_bytes,
            ..
        } => {
            let pair = m.caps.sample_geometry.bytes_per_pair() as f64;
            let rate = m.radio.config_sample_rate.max(1.0);
            Reading::Value(RadioDrops {
                stream: *stream,
                longest_ms: f64::from(*longest_bytes) / pair / rate * 1e3,
            })
        }
    }
}

/// The vitals group. Stale when the radio is not streaming, and then the
/// figures that run with the stream are absent; the process's cost and the
/// session's USB errors are counted either way. Dated by the last poll.
pub fn vitals(m: &SdrMetrics, clock: Clock) -> Group<Vitals> {
    Group {
        testimony: Testimony::resolve(
            m,
            Staleness::NotStreaming,
            None,
            Some(clock.unix_ms_of(m.radio.last_poll_time)),
        ),
        readings: vitals_readings(m),
    }
}

/// The vitals readings alone.
pub fn vitals_readings(m: &SdrMetrics) -> Vitals {
    let live = !Staleness::NotStreaming.resolve(m);
    let why = || "the radio is not streaming".to_string();
    let s = &m.signal;
    Vitals {
        drops_per_s: Reading::of(live.then_some(s.drops_per_sec), why),
        lost_session: Reading::of(
            live.then(|| Lost {
                all: s.total_drops_session,
                driver: s.driver_drops_session,
                monas: s.total_drops_session.saturating_sub(s.driver_drops_session),
            }),
            why,
        ),
        radio_drops: radio_drops(m),
        adc_saturation_pct: Reading::of(live.then_some(s.adc_saturation_pct), why),
        adc_saturation_peak_pct: Reading::of(live.then_some(s.adc_saturation_peak), why),
        cpu_pct: m.system.process_cpu_pct,
        rss_mb: m.system.process_rss_mb,
        usb_errors_session: s.usb_errors_session,
        throughput: Reading::of(live.then(|| throughput(m)), why),
        fft_queue_peak_pct: Reading::of(live.then_some(m.iq.buf_fill_pct), why),
        fft_blocks_dropped: Reading::of(
            live.then_some(Dropped {
                recent: m.iq.fft_drops,
                session: m.iq.fft_drops_session,
            }),
            why,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_ceiling_is_the_devices_own_byte_rate() {
        // 20 Msps HackRF: 40 MB(byte)/s, about 38.1 binary MB/s.
        let c = link_ceiling_mbps(20_000_000.0, 2.0);
        assert!((c - 38.147).abs() < 0.01, "got {c}");
        assert_eq!(link_ceiling_mbps(0.0, 2.0), 0.0);
    }

    /// A wider sample is more bytes at the same rate: the same 20 Msps on a
    /// 16-bit radio is twice the traffic, and the headroom has to say so or
    /// a saturated link reads as healthy.
    #[test]
    fn a_wider_sample_doubles_the_link_ceiling() {
        let narrow = link_ceiling_mbps(20_000_000.0, 2.0);
        let wide = link_ceiling_mbps(20_000_000.0, 4.0);
        assert!((wide - narrow * 2.0).abs() < 1e-9, "{wide} vs {narrow}");
    }

    /// A stopped radio has no stream figures; what the process costs is
    /// counted all the same.
    #[test]
    fn a_stopped_radio_has_no_stream_figures() {
        let v = vitals_readings(&SdrMetrics::fixture());
        assert!(v.drops_per_s.absent().is_some());
        assert!(v.throughput.absent().is_some());
        assert!(v.fft_blocks_dropped.absent().is_some());
        let live = vitals_readings(&SdrMetrics::fixture().streaming().with_timing(0.3));
        assert!(live.drops_per_s.value().is_some());
        assert!(live.throughput.value().is_some());
    }

    /// The radio's own count says why when there is none.
    #[test]
    fn the_radios_own_count_says_why_it_is_missing() {
        let mut m = SdrMetrics::fixture().streaming();
        assert!(radio_drops(&m).absent().is_some());
        m.signal.radio_drops = RadioDropAccount::Unreadable("the register would not read".into());
        assert_eq!(
            radio_drops(&m).absent(),
            Some("the register would not read")
        );
        m.signal.radio_drops = RadioDropAccount::Counted {
            stream: 3,
            window: 1,
            longest_bytes: 20_000,
        };
        assert_eq!(radio_drops(&m).value().map(|d| d.stream), Some(3));
    }
}
