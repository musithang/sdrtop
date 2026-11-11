// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The FT8 decodes, one to a row, oldest first: every decode the view still
//! holds (the newest [`crate::state::ft8::DECODES_MAX`]), whatever the list's
//! own filter shows.
//!
//! **What each decode measured, beside what it said** (POLICY.md, rule 3):
//! SNR, DT, tone and its spread, drift, the path's coherence, the Costas
//! tones on their tone, the LDPC's iterations and repairs, OSD's order, and
//! the clock verdict the cycle was read with; the least margin a data
//! symbol was decided by, and whether the radio held its level in the
//! cycle and the noise under the band. **Blank wherever nothing was heard** (rule 2): a
//! message with no locator has no `locator`; a station that never sent one
//! has no place, no distance and no bearing; without your position there is
//! no distance either; OSD not needed is blank, not zero; a cycle gone from
//! the hour has no `held`, and one held, changed or refused has no floor.
//!
//! `locator` is the one this message carried. `station_locator` is the last
//! one the sender sent itself, as known when the file was written: what the
//! map and the list measure the distance from.

use crate::geo::{maidenhead, sphere};
use crate::state::SdrMetrics;

pub const HEADER: &str = "utc,cycle,band,dial_hz,tone_hz,tone_sigma_hz,\
snr_db,dt_s,drift_hz,coherence,sync_of_21,ldpc_iterations,bits_repaired,osd_order,\
block,pass,message,from,to,for_me,locator,station_locator,lat,lon,km,\
bearing_deg,clock_median_dt_s,clock_spread_s,clock_decodes,margin_min_db,held,floor_db_hz";

/// `YYYY-MM-DDTHH:MM:SSZ` of a Unix time.
fn iso(unix: f64) -> String {
    let secs = unix.floor() as i64;
    let days = secs.div_euclid(86_400);
    let t = secs.rem_euclid(86_400);
    // Days since 1970-01-01 to a civil date (H. Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        t / 3600,
        (t / 60) % 60,
        t % 60
    )
}

/// Every decode held, as rows under [`HEADER`].
pub fn rows(state: &SdrMetrics) -> Vec<String> {
    let f = &state.ft8;
    let home = state.home.position();
    f.decodes
        .iter()
        .map(|e| {
            let d = &e.decode;
            let p = &d.physics;
            let msg = &d.message;
            let from = msg.sender();
            let station_loc = from
                .and_then(|c| f.stations.get(c))
                .and_then(|s| s.locator.clone());
            let place = station_loc.as_deref().and_then(maidenhead::centre);
            let path = home
                .zip(place)
                .map(|(h, pl)| (sphere::distance_km(h, pl), sphere::bearing_deg(h, pl)));
            let clock = f
                .cycles
                .iter()
                .find(|c| c.start_unix == e.cycle_unix)
                .and_then(|c| c.clock);
            // The hour's mark of the cycle: whether the radio held its level
            // in it, and the noise under the band. Older than the hour, or
            // held, changed or refused: blank.
            let mark = f.cycle_hour.iter().find(|c| c.start_unix == e.cycle_unix);
            let opt = |v: Option<String>| v.unwrap_or_default();
            [
                iso(e.cycle_unix),
                if (e.cycle_unix / 15.0).round() as i64 % 2 == 0 {
                    "even"
                } else {
                    "odd"
                }
                .to_string(),
                crate::signal::ft8::bands::band_of(e.dial_hz)
                    .unwrap_or_default()
                    .to_string(),
                e.dial_hz.to_string(),
                format!("{:.2}", p.f0),
                format!("{:.3}", p.f0_sigma),
                format!("{:.1}", p.snr_db),
                format!("{:.3}", d.dt),
                format!("{:.2}", p.drift),
                format!("{:.2}", p.coherence),
                d.costas.to_string(),
                d.iterations.to_string(),
                d.repaired.to_string(),
                opt(d.osd.map(|o| o.to_string())),
                d.block.to_string(),
                d.pass.to_string(),
                super::csv_field(&msg.text()).into_owned(),
                opt(from.map(str::to_string)),
                super::csv_field(&msg.addressees().join(" ")).into_owned(),
                e.for_you.to_string(),
                opt(msg.locator().map(str::to_string)),
                opt(station_loc.clone()),
                opt(place.map(|(lat, _)| format!("{lat:.3}"))),
                opt(place.map(|(_, lon)| format!("{lon:.3}"))),
                opt(path.map(|(km, _)| format!("{km:.0}"))),
                opt(path.map(|(_, b)| format!("{b:.0}"))),
                opt(clock.map(|c| format!("{:.3}", c.median_dt))),
                opt(clock.map(|c| format!("{:.3}", c.spread))),
                opt(clock.map(|c| c.decodes.to_string())),
                format!("{:.1}", d.trace.least_margin_db()),
                opt(mark.map(|c| c.held.to_string())),
                opt(mark.and_then(|c| c.floor_db_hz).map(|v| format!("{v:.1}"))),
            ]
            .join(",")
        })
        .collect()
}

/// What an empty file says, or a full one that a filter or the cap may have
/// left short.
pub fn note(state: &SdrMetrics) -> Option<String> {
    let f = &state.ft8;
    if f.decodes.is_empty() {
        return Some(if f.decoded > 0 {
            "no decode held: the list was cleared (c)".to_string()
        } else {
            "nothing has been decoded this session".to_string()
        });
    }
    let held = f.decodes.len() as u64;
    (f.decoded > held).then(|| {
        format!(
            "the newest {held} of {} decodes this session: the view holds no more",
            f.decoded
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::panels::radio::ft8::sample::evening;

    #[test]
    fn a_unix_time_reads_as_its_utc_date() {
        assert_eq!(iso(0.0), "1970-01-01T00:00:00Z");
        // The sheet's moment.
        assert_eq!(iso(1_793_366_100.0), "2026-10-30T13:15:00Z");
        assert_eq!(iso(951_782_400.0), "2000-02-29T00:00:00Z");
    }

    fn column(header: &str, name: &str) -> usize {
        header.split(',').position(|c| c == name).unwrap()
    }

    #[test]
    fn every_row_has_every_column_and_says_what_it_measured() {
        let m = evening();
        let rows = rows(&m);
        assert_eq!(rows.len(), m.ft8.decodes.len());
        let n = HEADER.split(',').count();
        for r in &rows {
            // No field in the sample holds a comma, so a split counts them.
            assert_eq!(r.split(',').count(), n, "{r}");
        }
        let tokyo = rows
            .iter()
            .find(|r| r.contains("HA5ZZT JA1VQD PM95"))
            .expect("the call to you");
        let f: Vec<&str> = tokyo.split(',').collect();
        assert_eq!(f[column(HEADER, "from")], "JA1VQD");
        assert_eq!(f[column(HEADER, "to")], "HA5ZZT");
        assert_eq!(f[column(HEADER, "for_me")], "true");
        assert_eq!(f[column(HEADER, "locator")], "PM95");
        assert_eq!(f[column(HEADER, "band")], "20 m");
        assert!(f[column(HEADER, "utc")].starts_with("2026-10-30T13:1"));
        let km: f64 = f[column(HEADER, "km")].parse().unwrap();
        assert!((8000.0..10_000.0).contains(&km), "{km}");
        assert_eq!(f[column(HEADER, "osd_order")], "2");
        assert_eq!(f[column(HEADER, "coherence")], "1.00");
    }

    #[test]
    fn what_was_not_heard_is_blank_never_filled() {
        let mut m = evening();
        let rows_now = rows(&m);
        // YO3GCI never sent a locator: no place, no distance.
        let yo = rows_now.iter().find(|r| r.contains(",YO3GCI,")).unwrap();
        let f: Vec<&str> = yo.split(',').collect();
        for c in ["station_locator", "lat", "lon", "km", "bearing_deg"] {
            assert_eq!(f[column(HEADER, c)], "", "{c} in {yo}");
        }
        // Without your position there is no distance for anybody.
        m.home.latitude = None;
        for r in rows(&m) {
            let f: Vec<&str> = r.split(',').collect();
            assert_eq!(f[column(HEADER, "km")], "", "{r}");
        }
    }

    #[test]
    fn o_writes_one_file_with_the_shared_header() {
        let dir = std::env::temp_dir().join(format!(
            "sdrtop-export-ft8-{}-{:?}",
            std::process::id(),
            std::time::Instant::now()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let m = evening();
        let w = crate::export::ft8_section(&m, &dir, 1_793_366_130).unwrap();
        assert!(w.path.ends_with("ft8-20261030-131530.csv"), "{:?}", w.path);
        assert_eq!(w.rows, m.ft8.decodes.len());
        let text = std::fs::read_to_string(&w.path).unwrap();
        assert!(
            text.starts_with(crate::export::provenance::COMMENT),
            "{text}"
        );
        assert!(text.contains(&format!("\n{HEADER}\n")), "{text}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The new columns: the least margin on every row; `held` and the floor
    /// from the hour's mark of the cycle, the floor blank where the radio
    /// held its level, both blank for a cycle the hour no longer has.
    #[test]
    fn the_cycle_columns_come_from_the_hours_marks() {
        use crate::state::ft8::{CycleMark, CycleOutcome};
        let mut m = evening();
        let cycles: Vec<f64> = {
            let mut v: Vec<f64> = m.ft8.decodes.iter().map(|e| e.cycle_unix).collect();
            v.dedup();
            v
        };
        assert_eq!(cycles.len(), 2, "{cycles:?}");
        let mark = |start_unix: f64, held: bool, floor: Option<f32>| CycleMark {
            start_unix,
            dial_hz: 14_074_000,
            outcome: CycleOutcome::Decoded,
            decodes: 1,
            floor_db_hz: floor,
            held,
            segment: 0,
        };
        let hour = std::sync::Arc::make_mut(&mut m.ft8.cycle_hour);
        hour.clear();
        hour.push_back(mark(cycles[0], true, None));
        hour.push_back(mark(cycles[1], false, Some(-85.04)));
        let held = column(HEADER, "held");
        let floor = column(HEADER, "floor_db_hz");
        let margin = column(HEADER, "margin_min_db");
        for (e, r) in m.ft8.decodes.iter().zip(rows(&m)) {
            let f: Vec<&str> = r.split(',').collect();
            let want = format!("{:.1}", e.decode.trace.least_margin_db());
            assert_eq!(f[margin], want, "{r}");
            let want = if e.cycle_unix == cycles[0] {
                ("true", "")
            } else {
                ("false", "-85.0")
            };
            assert_eq!((f[held], f[floor]), want, "{r}");
        }
        // Once the hour has let the cycles go, neither is on record.
        std::sync::Arc::make_mut(&mut m.ft8.cycle_hour).clear();
        for r in rows(&m) {
            let f: Vec<&str> = r.split(',').collect();
            assert_eq!((f[held], f[floor]), ("", ""), "{r}");
            assert!(!f[margin].is_empty(), "{r}");
        }
    }

    #[test]
    fn an_empty_file_says_which_empty_it_is() {
        let mut m = evening();
        assert_eq!(note(&m), None);
        m.ft8.clear();
        assert!(note(&m).unwrap().contains("cleared"));
        m.ft8.decoded = 0;
        assert!(note(&m).unwrap().contains("nothing has been decoded"));
    }
}
