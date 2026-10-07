// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The band occupancy history: one column a dwell, kept as it was measured.

/// How often the band is written onto the time axis.
///
/// Half a second. A canvas column is a moment, and this is about the finest a
/// person reads a minute-long picture at; faster columns would cost memory and
/// redraw for detail nobody can resolve at two metres, which is the acceptance
/// criterion for the panel that draws them.
pub const COLUMN_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

/// How much of the past the canvas keeps.
///
/// Two minutes at [`COLUMN_INTERVAL`], which is long enough to see a device come
/// and go and short enough that eighty-three cells of it is under a hundred
/// kilobytes.
pub const HISTORY_COLUMNS: usize = 240;

/// One megahertz of the band, as measured over the last dwell.
///
/// A cell that was never inside the observed span has `windows` of zero, and
/// that is the difference between "nothing was transmitting here" and "nobody
/// looked here". The panel draws them differently, because they are different
/// answers and rule 2 is about exactly this.
#[derive(Clone, Copy, Debug, Default)]
pub struct CellReading {
    /// Transform windows this cell was measured over.
    pub windows: u64,
    /// Fraction of them with something in this cell, with the false-alarm floor
    /// removed. Zero is a measurement.
    pub duty: f64,
    /// Mean and peak power in the cell, relative to the converter's full scale.
    pub mean_dbfs: f64,
    pub peak_dbfs: f64,
    /// What fraction of wall time this cell has actually been under observation.
    ///
    /// About one in [`crate::signal::net::survey::Plan::hops`] while surveying,
    /// and one while locked. `None` until there is any elapsed time to be a
    /// fraction of.
    ///
    /// **Measured over the whole watch, not between the last two dwells**, and
    /// the difference is not subtle. A dwell publishes every fifty milliseconds
    /// of observation and a hop lasts a hundred, so two dwells land inside one
    /// visit - and the gap between *those* two is fifty milliseconds of looking
    /// in fifty milliseconds of wall clock, which is one. On a live radio with
    /// five positions this read 87 % where the honest answer is 16 %.
    ///
    /// **The duty cycle is not scaled by this**, and the temptation to is worth
    /// naming: a channel busy all the time, seen for a sixth of the time, is
    /// busy all the time. Scaling its reading to sixteen percent would not be a
    /// sampled measurement, it would be a wrong one. What sampling costs is
    /// certainty, not magnitude, and that is carried by the window count and
    /// reported as an uncertainty.
    pub coverage: Option<f64>,
    /// When this cell was last measured.
    ///
    /// Rule 4 says all testimony is dated, and a survey that has not come back
    /// to a cell for a minute is showing a minute-old reading. The occupancy
    /// panel reads the oldest of these as the span its numbers cover, for the
    /// feed-loss caveat, and prints the selected cell's own age in its cursor
    /// readout.
    pub measured: Option<std::time::Instant>,
    /// Seconds this cell has been under observation since the watch began.
    ///
    /// The numerator of [`Self::coverage`]. Accumulated rather than differenced,
    /// because what a survey costs is only visible over a whole pass and a
    /// difference between two dwells cannot see one.
    pub observed_s: f64,
}

impl CellReading {
    /// Whether anybody looked here.
    pub fn observed(&self) -> bool {
        self.windows > 0
    }
}

/// The band as last measured, one megahertz at a time.
#[derive(Clone, Debug, Default)]
pub struct BandOccupancy {
    /// Empty until the first dwell completes; `occupancy::CELLS` long after.
    pub cells: Vec<CellReading>,
    /// The receiver's own noise floor, which every duty cycle here was measured
    /// against. `None` before the first dwell.
    pub noise_dbfs: Option<f64>,
    /// Whether the floor's preconditions held. When they did not, nothing here
    /// is a measurement and the panel says so rather than drawing it.
    pub trusted: bool,
    /// What the plane looked like, kept because it is the reason `trusted` is
    /// what it is and a panel that only showed the verdict would be asking to be
    /// believed.
    pub tail: f64,
    pub spread: f64,
    /// The band as it was, one column per moment, oldest first.
    ///
    /// **A column is a moment, not a pass.** The survey refreshes any one cell
    /// once per pass, so a column taken faster than that repeats the last
    /// measurement for most cells - which is what the cell's value *is*, and the
    /// coverage line already says how often it is renewed. Tying columns to
    /// passes instead would make the time axis stop when the mode is locked.
    pub history: std::collections::VecDeque<Vec<f32>>,
    /// When the newest column was taken.
    pub last_column: Option<std::time::Instant>,
    /// How many columns have ever been taken, so a column has an identity
    /// that survives the history scrolling: the newest is `columns_taken - 1`,
    /// and one `back` steps before it is `columns_taken - 1 - back`. The time
    /// cursor remembers a moment by this, not by its place on screen.
    pub columns_taken: u64,
    /// When the coverage accounting began.
    ///
    /// Restarted when the mode changes, because survey and lock are different
    /// regimes and averaging across the switch would describe neither.
    pub watch_start: Option<std::time::Instant>,
    /// How long one transform window was. The resolution every duty cycle here
    /// was measured at, and what turns a window count back into seconds.
    pub window_s: f64,
}

impl BandOccupancy {
    /// Fold one dwell into the band, keeping every cell the dwell did not see.
    ///
    /// **This is what makes a survey a survey.** Each dwell measures the slice
    /// the radio was pointed at; the rest of the band keeps what the last pass
    /// found there, with the time it was found. A dwell that replaced the whole
    /// band would leave a receiver seeing a fifth of it reporting the other four
    /// fifths as unobserved on every frame, which is a picture of the receiver
    /// rather than of the band.
    ///
    /// The floor is the receiver's rather than the position's, so the newest one
    /// wins outright: it is a fact about the front end at this gain, and the
    /// front end does not change between hops.
    pub fn absorb(&mut self, dwell: BandOccupancy, now: std::time::Instant) {
        if self.cells.len() != dwell.cells.len() {
            self.cells = vec![CellReading::default(); dwell.cells.len()];
        }
        // The watch begins when the *observing* began, not when the first dwell
        // was published: that dwell already carries the time it took to gather,
        // and counting it against a clock that started afterwards makes the
        // first reading look better than it is.
        let first_dwell =
            dwell.cells.iter().map(|c| c.windows).max().unwrap_or(0) as f64 * dwell.window_s;
        let watch_start = *self
            .watch_start
            .get_or_insert(now - std::time::Duration::from_secs_f64(first_dwell.max(0.0)));
        let watched = now.saturating_duration_since(watch_start).as_secs_f64();
        for (old, new) in self.cells.iter_mut().zip(dwell.cells.iter()) {
            if new.windows == 0 {
                continue;
            }
            let observed_s = old.observed_s + new.windows as f64 * dwell.window_s;
            // A fraction needs something to be a fraction of, and until the
            // watch has run for a moment there is nothing.
            let coverage = (watched > 0.0).then(|| (observed_s / watched).min(1.0));
            *old = CellReading {
                coverage,
                measured: Some(now),
                observed_s,
                ..*new
            };
        }
        self.noise_dbfs = dwell.noise_dbfs;
        self.trusted = dwell.trusted;
        self.tail = dwell.tail;
        self.spread = dwell.spread;
        self.window_s = dwell.window_s;
        self.record_column(now);
    }

    /// While the band is not being measured, keep its time axis moving: a
    /// column of "nobody looked" (`-1`, as an unobserved cell is drawn) when
    /// one is due, so the columns stay one interval apart and a column counted
    /// back is that long ago. Nothing before the first measurement, and the
    /// readings are left as they were, dated by their own `measured`.
    pub fn mark_unobserved(&mut self, now: std::time::Instant) {
        let due = self
            .last_column
            .is_none_or(|t| now.saturating_duration_since(t) >= COLUMN_INTERVAL);
        if !due || self.cells.is_empty() {
            return;
        }
        self.last_column = Some(now);
        self.history.push_back(vec![-1.0; self.cells.len()]);
        self.columns_taken += 1;
        while self.history.len() > HISTORY_COLUMNS {
            self.history.pop_front();
        }
    }

    /// Push the band as it stands onto the history, if it is time for a column.
    fn record_column(&mut self, now: std::time::Instant) {
        let due = self
            .last_column
            .is_none_or(|t| now.saturating_duration_since(t) >= COLUMN_INTERVAL);
        if !due || self.cells.is_empty() {
            return;
        }
        self.last_column = Some(now);
        // Unobserved reads as a negative, so the canvas can draw "nobody looked
        // here" differently from "nothing was here" - the same distinction the
        // occupancy profile makes, carried into the time axis.
        self.history.push_back(
            self.cells
                .iter()
                .map(|c| if c.observed() { c.duty as f32 } else { -1.0 })
                .collect(),
        );
        self.columns_taken += 1;
        while self.history.len() > HISTORY_COLUMNS {
            self.history.pop_front();
        }
    }

    /// How many steps back from the newest column the column `id` is, while
    /// the history still holds it.
    pub fn back_of(&self, id: u64) -> Option<usize> {
        let back = self.columns_taken.checked_sub(1)?.checked_sub(id)? as usize;
        (back < self.history.len()).then_some(back)
    }

    /// The identity of the column `back` steps before the newest.
    pub fn id_back(&self, back: usize) -> Option<u64> {
        (back < self.history.len()).then(|| self.columns_taken - 1 - back as u64)
    }

    /// The column `id`, while the history holds it: each cell's duty, negative
    /// where nobody looked.
    pub fn column(&self, id: u64) -> Option<&Vec<f32>> {
        let back = self.back_of(id)?;
        self.history.get(self.history.len() - 1 - back)
    }

    /// Start the coverage accounting again.
    ///
    /// Called when the mode changes. The measurements themselves are kept: they
    /// are still what was on the air. What is thrown away is the accounting of
    /// how often we were looking, because that is the thing the mode changed.
    pub fn restart_watch(&mut self) {
        self.watch_start = None;
        for c in self.cells.iter_mut() {
            c.observed_s = 0.0;
            c.coverage = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// **A column keeps its identity while the history scrolls under it.**
    /// A new column pushes it one step further back, the same id finds the
    /// same values, and once it falls off the end it is gone rather than
    /// quietly naming its neighbour.
    #[test]
    fn a_history_column_is_found_by_its_identity_as_the_history_scrolls() {
        let mut band = BandOccupancy::default();
        let push = |band: &mut BandOccupancy, v: f32| {
            band.history.push_back(vec![v; 3]);
            band.columns_taken += 1;
            while band.history.len() > 4 {
                band.history.pop_front();
            }
        };
        for v in [0.1, 0.2, 0.3] {
            push(&mut band, v);
        }
        let id = band.id_back(1).unwrap();
        assert_eq!(band.column(id).unwrap()[0], 0.2);
        push(&mut band, 0.4);
        assert_eq!(band.back_of(id), Some(2), "one step further back");
        assert_eq!(
            band.column(id).unwrap()[0],
            0.2,
            "and still the same moment"
        );
        push(&mut band, 0.5);
        push(&mut band, 0.6);
        assert_eq!(
            band.back_of(id),
            None,
            "scrolled out of a four-column history"
        );
        assert!(band.column(id).is_none());
        assert_eq!(band.id_back(4), None);
    }

    /// One dwell's worth of band: `cells` measured, the rest untouched.
    fn dwell(cells: &[(usize, f64, u64)]) -> BandOccupancy {
        let mut out = BandOccupancy {
            cells: vec![CellReading::default(); 83],
            noise_dbfs: Some(-78.0),
            trusted: true,
            tail: 2.1,
            spread: 30.0,
            window_s: 6.4e-6,
            watch_start: None,
            // A dwell has no past: the history is the band's, and `absorb`
            // owns it.
            history: Default::default(),
            last_column: None,
            columns_taken: 0,
        };
        for &(c, duty, windows) in cells {
            out.cells[c] = CellReading {
                windows,
                duty,
                mean_dbfs: -60.0,
                peak_dbfs: -30.0,
                coverage: None,
                measured: None,
                observed_s: 0.0,
            };
        }
        out
    }

    /// While the band is not being measured, the time axis still moves: a
    /// column of "nobody looked" every interval, so a column counted back
    /// is still that many half-seconds ago. Before anything was measured
    /// there is no axis to keep, and the readings themselves are untouched.
    #[test]
    fn time_away_is_marked_as_unobserved_columns() {
        let t0 = Instant::now();
        let mut band = BandOccupancy::default();
        band.mark_unobserved(t0);
        assert!(band.history.is_empty(), "nothing measured, no axis");

        band.absorb(dwell(&[(10, 0.4, 8_000)]), t0);
        assert_eq!(band.history.len(), 1);
        band.mark_unobserved(t0 + Duration::from_millis(200));
        assert_eq!(band.history.len(), 1, "not due yet");
        band.mark_unobserved(t0 + Duration::from_millis(500));
        band.mark_unobserved(t0 + Duration::from_millis(1000));
        assert_eq!(band.history.len(), 3);
        assert!(band.history[2].iter().all(|&v| v < 0.0), "nobody looked");
        assert_eq!(band.cells[10].duty, 0.4);
        assert_eq!(band.cells[10].measured, Some(t0));
    }

    /// **This is what makes a survey a survey.** Each dwell sees one slice; the
    /// band keeps what the last pass found everywhere else.
    ///
    /// Without it, a receiver seeing a fifth of the band would report the other
    /// four fifths as unobserved on every frame, which is a picture of the
    /// receiver rather than of the band, and the panel's whole
    /// observed-versus-unobserved distinction would collapse to "wherever the
    /// radio happens to be pointed this instant".
    #[test]
    fn a_dwell_folds_into_the_band_rather_than_replacing_it() {
        let t0 = Instant::now();
        let mut band = BandOccupancy::default();

        band.absorb(dwell(&[(10, 0.4, 8_000)]), t0);
        assert_eq!(band.cells[10].duty, 0.4);
        assert!(band.cells[10].observed());

        // A second dwell somewhere else does not take the first one with it.
        band.absorb(dwell(&[(60, 0.9, 8_000)]), t0 + Duration::from_millis(300));
        assert_eq!(band.cells[60].duty, 0.9);
        assert_eq!(
            band.cells[10].duty, 0.4,
            "the other end of the band is still what the last pass found"
        );
        assert!(band.cells[10].observed());
        // And a cell no pass has reached yet is still unobserved, which is a
        // different answer from empty.
        assert!(!band.cells[30].observed());
    }

    /// Sampling costs certainty, not magnitude.
    ///
    /// A channel busy all the time, watched a sixth of the time, is busy all the
    /// time. Scaling its reading to sixteen percent would not be a sampled
    /// measurement, it would be a wrong one - and it is the obvious thing to
    /// write, which is why it is asserted against.
    ///
    /// The coverage itself is the second half: it is a running average over the
    /// whole watch, so it converges on the fraction of wall time the radio
    /// actually spends here. Measured between the last two dwells instead, it
    /// read 87 % on a live five-position survey, because two dwells fit inside
    /// one hop and the gap between *those* is all observation.
    #[test]
    fn the_coverage_is_reported_and_never_multiplied_into_the_duty_cycle() {
        let t0 = Instant::now();
        let mut band = BandOccupancy::default();

        // A saturated cell, visited once per 625 ms pass. **Two dwells land
        // inside each visit**, fifty milliseconds apart, because the scan
        // publishes every fifty milliseconds of observation and a hop lasts a
        // hundred. That is the shape the old measure got wrong: the gap between
        // those two is all observation, so it read one, and the panel showed
        // 87 % on a five-position survey.
        for pass in 0..40u32 {
            let visit = t0 + Duration::from_millis(625 * pass as u64);
            band.absorb(dwell(&[(10, 1.0, 8_000)]), visit);
            band.absorb(
                dwell(&[(10, 1.0, 8_000)]),
                visit + Duration::from_millis(51),
            );
        }
        assert_eq!(band.cells[10].duty, 1.0, "still busy all the time");

        let coverage = band.cells[10].coverage.expect("a watch has run");
        // Two dwells of 51 ms in every 625: a hundred milliseconds of looking a
        // pass, which is the sixth a five-position survey spends here.
        let want = 2.0 * 0.0512 / 0.625;
        assert!(
            (coverage - want).abs() < 0.005,
            "51 ms of looking in every 625: wanted {want:.3}, got {coverage:.3}"
        );
    }

    /// The very first reading is not flattered by a clock that started after the
    /// observing did.
    #[test]
    fn the_watch_begins_when_the_looking_did() {
        let t0 = Instant::now();
        let mut band = BandOccupancy::default();
        band.absorb(dwell(&[(10, 1.0, 8_000)]), t0);
        // One dwell, and nothing but that dwell has happened: the radio has been
        // looking here the whole time it has been looking at all.
        let coverage = band.cells[10].coverage.expect("a watch has run");
        assert!((coverage - 1.0).abs() < 1e-9, "got {coverage}");
    }

    /// Locked, the receiver is looking almost all the time, and the coverage
    /// says so rather than being pinned to one by the mode.
    #[test]
    fn locking_shows_as_coverage_rather_than_being_assumed() {
        let t0 = Instant::now();
        let mut band = BandOccupancy::default();
        // Back to back: 51 ms of looking every 52 ms of clock.
        for i in 0..40u32 {
            band.absorb(
                dwell(&[(10, 0.3, 8_000)]),
                t0 + Duration::from_millis(52 * i as u64),
            );
        }
        let coverage = band.cells[10].coverage.unwrap();
        assert!(coverage > 0.95, "{coverage}");
        assert!(
            coverage <= 1.0,
            "never more than all of the time: {coverage}"
        );
    }

    /// Switching mode starts the accounting again, and keeps the measurements.
    ///
    /// Survey and lock are different regimes for how often the radio looks at
    /// any one megahertz. Averaging across the switch would describe neither,
    /// and the reading would take a minute to catch up with what the user just
    /// did.
    #[test]
    fn changing_mode_restarts_the_watch_but_keeps_what_was_measured() {
        let t0 = Instant::now();
        let mut band = BandOccupancy::default();
        for pass in 0..20u32 {
            band.absorb(
                dwell(&[(10, 0.42, 8_000)]),
                t0 + Duration::from_millis(625 * pass as u64),
            );
        }
        assert!(band.cells[10].coverage.unwrap() < 0.2);

        band.restart_watch();
        assert_eq!(band.cells[10].coverage, None, "nothing to be a fraction of");
        assert_eq!(band.cells[10].observed_s, 0.0);
        assert_eq!(band.cells[10].duty, 0.42, "the measurement stands");
        assert!(band.cells[10].observed(), "and the cell is still observed");
    }

    /// The floor is the receiver's, not the position's, so the newest wins.
    #[test]
    fn the_newest_floor_is_the_bands_floor() {
        let t0 = Instant::now();
        let mut band = BandOccupancy::default();
        band.absorb(dwell(&[(10, 0.3, 8_000)]), t0);
        let mut second = dwell(&[(60, 0.3, 8_000)]);
        second.noise_dbfs = Some(-71.0);
        second.trusted = false;
        band.absorb(second, t0 + Duration::from_millis(300));
        assert_eq!(band.noise_dbfs, Some(-71.0));
        assert!(!band.trusted, "a front end on its rails is on its rails");
    }
}
