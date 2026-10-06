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

use crate::signal::net::occupancy;
use crate::state::SdrMetrics;

/// The columns, and the header row that names them.
///
/// Units are in the names: every column must be interpretable six months later
/// from the header alone, and a column called `duty` is not.
pub const HEADER: &str =
    "mhz,observed,duty_pct,duty_sigma_pct,coverage_pct,mean_dbfs,peak_dbfs,windows";

/// The band as CSV rows, or the reason there are none.
pub fn rows(state: &SdrMetrics) -> Result<Vec<String>, String> {
    let band = &state.net.band;
    if state.ui.active_preset != crate::signal::net::lock::SURVEY_VIEW {
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

    Ok(band
        .cells
        .iter()
        .enumerate()
        .map(|(cell, c)| {
            let mhz = occupancy::cell_centre_hz(cell) as f64 / 1e6;
            if !c.observed() {
                // Every measured column empty, and the row still present: the
                // band has this megahertz in it whether or not we looked.
                return format!("{mhz:.1},no,,,,,,0");
            }
            let duty = occupancy::duty_uncertain(c.duty, c.windows).scale(100.0);
            // The same question the panel asks before it prints a number, asked
            // of the same `Uncertain`, so the file and the screen cannot
            // disagree about which values exist.
            let shown = if duty.is_resolved(occupancy::DUTY_RESOLUTION * 100.0) {
                format!("{:.2}", duty.value())
            } else {
                String::new()
            };
            let sigma = if duty.sigma().is_finite() {
                format!("{:.2}", duty.sigma())
            } else {
                String::new()
            };
            let coverage = c
                .coverage
                .map(|f| format!("{:.1}", f * 100.0))
                .unwrap_or_default();
            format!(
                "{mhz:.1},yes,{shown},{sigma},{coverage},{:.1},{:.1},{}",
                c.mean_dbfs, c.peak_dbfs, c.windows
            )
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{BandOccupancy, CellReading};

    fn band(cells: Vec<CellReading>) -> SdrMetrics {
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
        m.ui.active_preset = crate::signal::net::lock::SURVEY_VIEW.to_string();
        m
    }

    /// The band is measured on the survey alone; exported from another view
    /// it would be an old reading with nothing saying how old.
    #[test]
    fn off_the_survey_the_band_is_not_exported() {
        let mut m = band(vec![measured(0.25, 8_000); occupancy::CELLS]);
        m.ui.active_preset = "net_ble".to_string();
        let why = rows(&m).unwrap_err();
        assert!(why.contains("NET 2"), "{why}");
    }

    fn measured(duty: f64, windows: u64) -> CellReading {
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

    #[test]
    fn every_cell_of_the_band_gets_a_row_and_the_header_names_its_units() {
        let m = band(vec![measured(0.25, 8_000); occupancy::CELLS]);
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
        cells[40] = measured(0.5, 8_000);
        let rows = rows(&band(cells)).unwrap();

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
        cells[10] = measured(0.5, 40);
        let rows = rows(&band(cells)).unwrap();
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
        let mut m = band(vec![measured(0.25, 8_000); occupancy::CELLS]);
        m.net.band.trusted = false;
        m.net.band.tail = 3.9;
        let err = rows(&m).unwrap_err();
        assert!(err.contains("noise floor"), "{err}");
        assert!(err.contains("3.9"), "the figure that decided it: {err}");

        // And a band never measured is a different sentence.
        let mut m = band(Vec::new());
        m.net.band.cells = Vec::new();
        assert!(rows(&m).unwrap_err().contains("no band measurement"));
    }

    /// **The file and the screen agree**, asserted against one fixture.
    ///
    /// The rule is not that the export is careful, it is that
    /// it is careful in exactly the same places. Both ask the same `Uncertain`
    /// the same question, so this checks the wiring rather than the arithmetic.
    #[test]
    fn the_export_is_blank_wherever_the_panel_dashes() {
        let mut cells = vec![CellReading::default(); occupancy::CELLS];
        cells[41] = measured(0.95, 7_800); // well resolved
        let m = band(cells);

        let panel =
            crate::state::fixture::draw(crate::ui::NetOccupancyPanel, 80, 12, &m).join("\n");
        let rows = rows(&m).unwrap();

        // The panel prints the busiest cell's figure; so does the file.
        assert!(panel.contains("95."), "{panel}");
        assert_eq!(rows[41].split(',').nth(2).unwrap(), "95.00");

        // The panel draws the unobserved cells as unlooked-at; the file leaves
        // them blank. Neither shows a zero.
        assert!(panel.contains('·'), "{panel}");
        assert_eq!(rows[0].split(',').nth(2).unwrap(), "");
        assert!(!rows[0].contains(",0.00,"), "{}", rows[0]);
    }
}
