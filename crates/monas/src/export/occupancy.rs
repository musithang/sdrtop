// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The band, one megahertz to a row. The first body that has data, and the one
//! that tests the rule every body keeps: **an export never contains a number
//! the panel would have dashed.**
//!
//! Three ways that bites here, and they are three different absences:
//!
//! - A cell nobody looked at exports its columns **empty**, not zero. Zero is a
//!   measurement of a quiet megahertz and this is the absence of one.
//! - A duty cycle whose uncertainty is wider than the resolution it is shown at
//!   exports empty, with its uncertainty still in its own column, because that
//!   is the thing worth saying about it.
//! - A floor that failed its own preconditions takes every row with it. The
//!   panel draws no profile in that state and the file carries no figures,
//!   because everything on it was measured against that floor.
//!
//! An empty CSV field is what a spreadsheet and `pandas.read_csv` both already
//! read as "no value", so this needs no convention of its own.

use crate::state::SdrMetrics;

/// The columns, and the header row that names them.
///
/// Units are in the names: every column must be interpretable six months later
/// from the header alone, and a column called `duty` is not.
pub const HEADER: &str =
    "mhz,observed,duty_pct,duty_sigma_pct,coverage_pct,mean_dbfs,peak_dbfs,windows";

/// The band as CSV rows, or the reason there are none: the engine's account
/// of it ([`crate::engine::view::net::band_record`]) written as text, empty
/// where the account has no figure. A spreadsheet and `pandas.read_csv` both
/// read an empty field as "no value", so blank needs no convention.
pub fn rows(state: &SdrMetrics) -> Result<Vec<String>, String> {
    let band = crate::engine::view::net::band_record(state)?;
    let opt =
        |v: Option<f64>, places: usize| v.map(|x| format!("{x:.places$}")).unwrap_or_default();
    Ok(band
        .cells
        .iter()
        .map(|c| {
            let mhz = c.mhz;
            if !c.observed {
                // Every measured column empty, and the row still present: the
                // band has this megahertz in it whether or not we looked.
                return format!("{mhz:.1},no,,,,,,0");
            }
            format!(
                "{mhz:.1},yes,{},{},{},{},{},{}",
                opt(c.duty_pct, 2),
                opt(c.duty_sigma_pct, 2),
                opt(c.coverage_pct, 1),
                opt(c.mean_dbfs, 1),
                opt(c.peak_dbfs, 1),
                c.windows
            )
        })
        .collect())
}

/// A measured band for tests, here and the panel's.
#[cfg(any(test, feature = "testing"))]
pub mod sample {
    use crate::state::{BandOccupancy, CellReading, SdrMetrics};

    pub fn band(cells: Vec<CellReading>) -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.band = BandOccupancy {
            cells,
            noise_dbfs: Some(-78.0),
            trusted: true,
            tail: 2.1,
            spread: 30.0,
            window_s: 6.4e-6,
            ..Default::default()
        };
        m.show_preset(crate::signal::net::lock::SURVEY_VIEW);
        m
    }

    pub fn measured(duty: f64, windows: u64) -> CellReading {
        CellReading {
            windows,
            duty,
            mean_dbfs: -60.0,
            peak_dbfs: -30.0,
            coverage: Some(0.16),
            measured: None,
            observed_s: 0.05,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::net::occupancy;
    use crate::state::CellReading;

    /// The band is measured on the survey alone; exported from another view
    /// it would be an old reading with nothing saying how old.
    #[test]
    fn off_the_survey_the_band_is_not_exported() {
        let mut m = sample::band(vec![sample::measured(0.25, 8_000); occupancy::CELLS]);
        m.show_preset("net_ble");
        let why = rows(&m).unwrap_err();
        assert!(why.contains("NET 2"), "{why}");
    }

    #[test]
    fn every_cell_of_the_band_gets_a_row_and_the_header_names_its_units() {
        let m = sample::band(vec![sample::measured(0.25, 8_000); occupancy::CELLS]);
        let rows = rows(&m).unwrap();
        assert_eq!(rows.len(), occupancy::CELLS);
        assert_eq!(HEADER.split(',').count(), rows[0].split(',').count());
        // Units in the names, so a column means something on its own.
        for column in ["mhz", "duty_pct", "mean_dbfs", "windows"] {
            assert!(HEADER.contains(column), "{HEADER}");
        }
        assert!(rows[0].starts_with("2400.5,yes,25.00,"), "{}", rows[0]);
    }

    /// **A cell nobody looked at exports empty, not zero.**
    #[test]
    fn an_unobserved_cell_is_blank_rather_than_a_quiet_one() {
        let mut cells = vec![CellReading::default(); occupancy::CELLS];
        cells[40] = sample::measured(0.5, 8_000);
        let rows = rows(&sample::band(cells)).unwrap();

        let unseen: Vec<&str> = rows[0].split(',').collect();
        assert_eq!(unseen[1], "no");
        for (i, field) in unseen.iter().enumerate().take(7).skip(2) {
            assert!(
                field.is_empty(),
                "field {i} of an unobserved cell: {field:?}"
            );
        }
        // And the measured one is not blank, so the test can tell the two apart.
        let seen: Vec<&str> = rows[40].split(',').collect();
        assert_eq!(seen[1], "yes");
        assert_eq!(seen[2], "50.00");
    }

    /// A duty cycle its own uncertainty cannot support exports empty, and keeps
    /// the uncertainty: that is the thing worth saying about it.
    #[test]
    fn a_duty_cycle_the_panel_would_dash_is_not_exported_as_a_figure() {
        // Forty windows: the binomial spread is nearly eight percent, far wider
        // than the whole percent the reading is shown to.
        let mut cells = vec![CellReading::default(); occupancy::CELLS];
        cells[10] = sample::measured(0.5, 40);
        let rows = rows(&sample::band(cells)).unwrap();
        let f: Vec<&str> = rows[10].split(',').collect();
        assert_eq!(f[1], "yes", "it was observed");
        assert_eq!(f[2], "", "but the figure is not supportable");
        assert!(
            !f[3].is_empty(),
            "the spread is what is left to say: {:?}",
            f[3]
        );
        assert!(f[3].parse::<f64>().unwrap() > 5.0, "{:?}", f[3]);
    }

    /// A floor that failed takes the whole body with it, and says why.
    #[test]
    fn an_untrusted_floor_exports_no_figures_at_all() {
        let mut m = sample::band(vec![sample::measured(0.25, 8_000); occupancy::CELLS]);
        m.net.band.trusted = false;
        m.net.band.tail = 3.9;
        let err = rows(&m).unwrap_err();
        assert!(err.contains("noise floor"), "{err}");
        assert!(err.contains("3.9"), "the figure that decided it: {err}");

        // And a band never measured is a different sentence.
        let mut m = sample::band(Vec::new());
        m.net.band.cells = Vec::new();
        assert!(rows(&m).unwrap_err().contains("no band measurement"));
    }
}
