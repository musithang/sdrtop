// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The R2's gain, as measured on its cable, setting by setting.
//!
//! **The setting is a position in the band, the dB is what was measured.**
//! The AD9361 reads `hardwaregain` from a different table in each of three LO
//! bands (to 1.3 GHz, to 4 GHz, above), and each starts somewhere else: -1,
//! -3 and -10 dB on the R2. So sdrtop's setting is the position from the
//! band's bottom (0 is -1, -3 or -10 dB), and its decibels are read from what
//! the bench measured: every 1 dB step at 25 frequencies, back to back
//! against one carrier, accumulated from the band's lowest setting, which is
//! taken at the radio's own figure (`user_docs/plutosky-r2-measured.md`).
//!
//! **Nothing between the measured frequencies is invented.** Each band of
//! tuning takes the table of the measured frequency nearest it (on a log
//! scale), never one across a gain-table band edge. Where a frequency's top
//! settings could not be measured, its table stops at the last that was; the
//! device keeps the radio within it ([`super::device`]).
//!
//! The measurements are data ([`MEASURED`], one line per setting) and the
//! tables are built from them here, so the figures on screen trace back to
//! the bench without a table typed by hand in between.

use crate::hardware::BandTable;

/// One line per measured setting: `frequency_hz,hardwaregain_db,measured_db`.
pub const MEASURED: &str = include_str!("gain_measured.csv");

/// The upper edges of the AD9361's three gain-table bands (inclusive), as
/// its driver chooses the table by the LO.
pub const BAND_EDGES_HZ: [u64; 3] = [1_300_000_000, 4_000_000_000, 6_000_000_000];
const FIRST_HZ: u64 = 70_000_000;

/// Which gain-table band an LO is in, 0..2.
pub fn band_of(hz: u64) -> usize {
    BAND_EDGES_HZ.iter().position(|&e| hz <= e).unwrap_or(2)
}

/// One measured frequency: its band, its lowest setting's `hardwaregain`, and
/// the measured dB of each setting from there up.
#[derive(Clone, Debug, PartialEq)]
pub struct Point {
    pub hz: u64,
    pub bottom_db: i32,
    pub db: Vec<f64>,
}

/// Reads [`MEASURED`]-shaped text into points, frequency order. A point
/// whose settings are not consecutive from its first is cut at the first
/// gap; lines that do not parse are skipped.
pub fn points(csv: &str) -> Vec<Point> {
    let mut rows: Vec<(u64, i32, f64)> = csv
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut f = l.split(',');
            Some((
                f.next()?.trim().parse().ok()?,
                f.next()?.trim().parse().ok()?,
                f.next()?.trim().parse().ok()?,
            ))
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut out: Vec<Point> = Vec::new();
    for (hz, setting, db) in rows {
        match out.last_mut() {
            Some(p) if p.hz == hz => {
                if setting == p.bottom_db + p.db.len() as i32 {
                    p.db.push(db);
                }
            }
            _ => out.push(Point {
                hz,
                bottom_db: setting,
                db: vec![db],
            }),
        }
    }
    out
}

/// The bands of tuning each measured point stands for: from the band's
/// start, or the geometric middle with the point below, to the middle with
/// the point above, or the band's end.
pub fn tables(points: &[Point]) -> Vec<BandTable> {
    let mut out = Vec::new();
    for band in 0..BAND_EDGES_HZ.len() {
        let start = if band == 0 {
            FIRST_HZ
        } else {
            BAND_EDGES_HZ[band - 1] + 1
        };
        let end = BAND_EDGES_HZ[band];
        let here: Vec<&Point> = points.iter().filter(|p| band_of(p.hz) == band).collect();
        for (i, p) in here.iter().enumerate() {
            let lo = if i == 0 {
                start
            } else {
                ((here[i - 1].hz as f64 * p.hz as f64).sqrt() as u64) + 1
            };
            let hi = match here.get(i + 1) {
                Some(next) => (p.hz as f64 * next.hz as f64).sqrt() as u64,
                None => end,
            };
            out.push(BandTable {
                band: lo..=hi,
                db: p.db.clone(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const CSV: &str = "# hz,setting,db
100000000,-1,-1.0
100000000,0,0.1
100000000,1,1.0
433000000,-1,-1.0
433000000,0,0.0
433000000,2,9.9
2400000000,-3,-3.0
2400000000,-2,-2.1
";

    #[test]
    fn points_are_read_and_cut_at_a_gap() {
        let p = points(CSV);
        assert_eq!(p.len(), 3);
        assert_eq!(
            (p[0].hz, p[0].bottom_db, p[0].db.clone()),
            (100_000_000, -1, vec![-1.0, 0.1, 1.0])
        );
        assert_eq!(
            p[1].db,
            vec![-1.0, 0.0],
            "433 MHz stops where setting 1 is missing"
        );
        assert_eq!(p[2].bottom_db, -3);
    }

    /// Each band of tuning takes the nearest measured point, on a log scale,
    /// and no table crosses a gain-table band edge.
    #[test]
    fn tables_cover_the_range_without_crossing_an_edge() {
        let t = tables(&points(CSV));
        assert_eq!(t.len(), 3);
        assert_eq!(*t[0].band.start(), 70_000_000);
        let middle = (100e6f64 * 433e6).sqrt() as u64;
        assert_eq!(*t[0].band.end(), middle);
        assert_eq!(*t[1].band.start(), middle + 1);
        assert_eq!(*t[1].band.end(), 1_300_000_000, "the low band's end");
        assert_eq!(*t[2].band.start(), 1_300_000_001);
        assert_eq!(*t[2].band.end(), 4_000_000_000);
    }

    #[test]
    fn the_bands_are_the_drivers() {
        assert_eq!(band_of(1_300_000_000), 0);
        assert_eq!(band_of(1_300_000_001), 1);
        assert_eq!(band_of(4_000_000_000), 1);
        assert_eq!(band_of(5_800_000_000), 2);
    }

    /// The data the backend ships: every gain-table band covered from its
    /// first to its last frequency, each table starting at its band's bottom
    /// and climbing.
    #[test]
    fn the_shipped_measurements_cover_every_band() {
        let p = points(MEASURED);
        let t = tables(&p);
        for hz in [
            70_000_000u64,
            1_300_000_000,
            1_300_000_001,
            4_000_000_000,
            4_000_000_001,
            6_000_000_000,
        ] {
            assert!(t.iter().any(|b| b.band.contains(&hz)), "{hz} not covered");
        }
        let bottoms = [-1, -3, -10];
        for pt in &p {
            assert_eq!(
                pt.bottom_db,
                bottoms[band_of(pt.hz)],
                "{} MHz",
                pt.hz / 1_000_000
            );
            assert!(
                pt.db.len() >= 40,
                "{} MHz: {} settings",
                pt.hz / 1_000_000,
                pt.db.len()
            );
            let rise = pt.db.last().unwrap() - pt.db[0];
            assert!(rise > 40.0, "{} MHz climbs {rise} dB", pt.hz / 1_000_000);
        }
    }
}
