// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetState` - what the 2.4 GHz receiver is doing right now.
//!
//! Deliberately small, and it will stay smaller than it looks like it should.
//! What belongs here is the state a *header* has to read, because that is the
//! one thing every panel in the section shares: the rest lives with the panel
//! that produced it. See `dev_docs/net-foundation-design.md` section 9.2.

/// Whether the receiver is walking the band or sitting on one channel.
///
/// **This changes what every number below it means**, which is why it is the
/// first field in the header and why it is never absent. A duty cycle measured
/// while sweeping is a sample of a channel; measured while locked it is that
/// channel's whole story. Design section 13.1 makes every panel say which one it
/// is looking at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NetMode {
    /// Stepping across the band. The default, because nothing has been chosen
    /// yet and a receiver that has not been told where to sit is surveying.
    #[default]
    Survey,
    /// Parked on one channel.
    Lock,
}

impl NetMode {
    /// The word the header shows. Upper case because it is a mode, not a
    /// reading, and the eye has to find it without looking.
    pub fn label(self) -> &'static str {
        match self {
            NetMode::Survey => "SURVEY",
            NetMode::Lock => "LOCK",
        }
    }

    /// The chrome tag every panel in the section carries.
    ///
    /// A panel says *which claim its numbers are*, and the engine spells and
    /// colours it, which is the rule for every tag. Design section 13.1: the
    /// mode is part of the reading, so this is not optional for a panel here and
    /// `every_net_panel_says_how_its_numbers_were_gathered` is what makes that
    /// true rather than customary.
    pub fn tag(self) -> crate::ui::panel::Tag {
        match self {
            NetMode::Survey => crate::ui::panel::Tag::Survey,
            NetMode::Lock => crate::ui::panel::Tag::Lock,
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            NetMode::Survey => NetMode::Lock,
            NetMode::Lock => NetMode::Survey,
        }
    }
}

/// Where the radio belongs once the survey gives the tuner back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetExit {
    pub tune_hz: u64,
    /// Whether the radio stays where the pass left it, rather than going back
    /// where the survey found it.
    pub locked: bool,
}

#[derive(Clone, Debug, Default)]
pub struct NetState {
    pub mode: NetMode,
    /// Why no survey is running, when the mode says one should be.
    ///
    /// **A refusal nobody can see is a silence.** The survey declines on a
    /// receiver too narrow to see past its own oscillator, which is right, and
    /// it said so only in the log - which the survey and coexistence screens do
    /// not carry. What the user saw was a panel claiming to be waiting for RX
    /// while the feed panel beside it counted blocks arriving. The sentence
    /// belongs on the screen the reader is on, so it lives here rather than only
    /// in the log.
    pub survey_refused: Option<String>,
    pub census: CensusState,
    pub health: NetDecodeHealth,
    pub band: BandOccupancy,
    /// Advertising channel PDUs decoded so far this session, newest first,
    /// capped at [`BLE_PACKET_LIMIT`].
    pub ble_packets: std::collections::VecDeque<BlePacket>,
    /// Why nothing is being decoded, when the radio can otherwise stream.
    ///
    /// B6's decoder needs the working rate `signal::ble::receive::front_end`
    /// states, and needs the tuning to actually be one of the three
    /// advertising channels - two conditions `net_survey`'s occupancy
    /// measurement does not share, so this is its own refusal rather than
    /// reusing `survey_refused`. Same reasoning as that field's own doc: a
    /// refusal nobody can see is a silence.
    pub ble_refused: Option<String>,
    /// The tuning the survey interrupted, so it can be given back.
    ///
    /// **In the state rather than in the task**, for the reason
    /// [`crate::state::SweepState::end`] gives about the same field: the task is
    /// not the only thing that has to put the radio back. Quitting mid-pass
    /// never reaches another iteration of that loop - the process ends - and
    /// `save_config` would write out whichever hop the survey was parked on, so
    /// the app would reopen somewhere in the middle of the band, one position
    /// further along each time.
    pub pre_survey_hz: Option<u64>,
}

impl NetState {
    /// Give the tuner back, and say where the radio belongs.
    ///
    /// **The two ways out of a survey want opposite answers, and treating them
    /// as one was a bug.** Leaving the section ends the survey, so the radio goes
    /// back where it was found. Switching to lock means "stay here": the user
    /// pressed the key while looking at a position, and that position is what
    /// they meant. The first version restored in both cases, while the key
    /// handler logged `NET locked to <the current hop>` - so the log said one
    /// thing and the radio did another.
    ///
    /// `tuned_hz` is `radio.frequency`. Safe on a state that never surveyed, and
    /// safe to call twice: the second call has nothing left to take and answers
    /// with whatever the caller wrote back after the first.
    pub fn end(&mut self, tuned_hz: u64) -> NetExit {
        let pre = self.pre_survey_hz.take();
        if self.mode == NetMode::Lock {
            NetExit {
                tune_hz: tuned_hz,
                locked: true,
            }
        } else {
            NetExit {
                tune_hz: pre.unwrap_or(tuned_hz),
                locked: false,
            }
        }
    }
}

/// What the receiver missed, and what it never had a chance to see.
///
/// Design section 13.2: this is not a debug panel, it is testimony. Without it
/// every count in the section is a lower bound presented as a total, because the
/// three ways a sample can go missing are all invisible from downstream. The
/// driver drops them before the block is stamped; the bounded feed refuses whole
/// blocks under load and carries no record that it did; and a run broken in the
/// middle takes whatever was being assembled with it.
///
/// Every field here is a count of something that happened, not a rate and not a
/// judgement. The panel does the dividing.
#[derive(Clone, Debug, Default)]
pub struct NetDecodeHealth {
    /// Blocks that reached the worker.
    pub blocks_in: u64,
    /// I/Q pairs in them.
    pub pairs_in: u64,
    /// Times the stream was interrupted: a block that did not continue the one
    /// before it, for either reason.
    ///
    /// **The first block of a run is not one of these.** Opening the section
    /// mid-stream means the first block to arrive is thousands of sequence
    /// numbers past nothing, and counting that as an interruption would put a
    /// phantom gap on the panel every time the user looked at it.
    pub gaps: u64,
    /// A floor on the blocks that went missing, summed over those gaps.
    ///
    /// A floor because the driver says samples went, never how many: one is the
    /// smallest number that is certainly not an overstatement, and zero would
    /// leave the panel calling a lossy link healthy. Same rule as
    /// `BlockPlan::dropped`, which is where this comes from.
    pub blocks_lost: u64,
    /// Unbroken blocks since the last gap.
    pub run_blocks: u64,
    /// Blocks the bounded feed refused in the last poll window, and since the
    /// radio was opened.
    pub refused: u64,
    pub refused_session: u64,
    /// The deepest the feed queue got in the last window.
    pub peak_depth: u64,
    /// When the last block arrived. `None` before the first one.
    pub last_block: Option<std::time::Instant>,
}

/// How many recent PDUs [`NetState::ble_packets`] keeps. A bench instrument
/// is read a screenful at a time, not scrolled back through a session's
/// worth of advertising traffic; old rows fall off the end rather than
/// growing the list forever.
pub const BLE_PACKET_LIMIT: usize = 200;

/// One decoded advertising channel PDU, as a panel shows it.
///
/// `crate::signal::ble::pdu::Packet` is the decode itself, pure and knowing
/// nothing about a screen; this adds the two facts a panel needs that decode
/// alone does not carry - which channel it arrived on and when.
#[derive(Clone, Debug)]
pub struct BlePacket {
    pub channel: u8,
    pub pdu_type: crate::signal::ble::pdu::PduType,
    pub tx_add_random: bool,
    pub length: u8,
    pub adv_addr: Option<[u8; 6]>,
    pub crc_ok: bool,
    pub seen: std::time::Instant,
}

/// How the census table is being read: what orders it, and where the cursor is.
///
/// **The cursor is an address, not a row number.** Rows move: a re-sort
/// reorders them, and a live census reorders them anyway as packet counts
/// change. A cursor stored as an index would slide onto whichever device
/// happened to land in that row, which is the sort of bug that looks like the
/// user's own mistake.
#[derive(Clone, Debug, Default)]
pub struct CensusState {
    /// The population, unordered.
    ///
    /// **Stored as found, ordered when drawn.** Keeping it sorted would mean
    /// re-sorting on every packet for a question only the panel asks, and would
    /// put the display's choice of column into the place the measurements live.
    pub devices: Vec<crate::signal::net::census::Device>,
    /// Index into `signal::net::census::SORT_KEYS`.
    pub sort: usize,
    pub descending: bool,
    /// The device the cursor is on. `None` before anything has been selected.
    pub selected: Option<[u8; 6]>,
    /// Where the viewport starts, so a long list does not jump under the cursor.
    pub first_visible: usize,
}

impl CensusState {
    /// The next column along, wrapping.
    ///
    /// One key, cycling, because design section 9.1 asks for the sort key to be
    /// *shown* rather than remembered - and a control the panel advertises in
    /// one direction is one the user can use without being told twice.
    pub fn cycle_sort(&mut self) {
        let keys = crate::signal::net::census::SORT_KEYS.len().max(1);
        self.sort = (self.sort + 1) % keys;
        // Back to the top of the new column rather than wherever the last one
        // left the cursor: the rows underneath are different rows now.
        self.first_visible = 0;
    }

    pub fn reverse(&mut self) {
        self.descending = !self.descending;
        self.first_visible = 0;
    }

    /// Move the cursor `delta` rows through `ordered`, and remember the device
    /// rather than the position.
    pub fn move_cursor(&mut self, ordered: &[[u8; 6]], delta: isize) {
        if ordered.is_empty() {
            self.selected = None;
            return;
        }
        let at = self
            .selected
            .and_then(|a| ordered.iter().position(|x| *x == a))
            .map(|i| i as isize)
            .unwrap_or(if delta < 0 {
                ordered.len() as isize
            } else {
                -1
            });
        let next = (at + delta).clamp(0, ordered.len() as isize - 1) as usize;
        self.selected = Some(ordered[next]);
    }

    /// Where the cursor is now, in this ordering. `None` when the device it was
    /// on is no longer in the list - which is an answer, not an error: it means
    /// that transmitter has gone.
    pub fn cursor(&self, ordered: &[[u8; 6]]) -> Option<usize> {
        let a = self.selected?;
        ordered.iter().position(|x| *x == a)
    }
}

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
    /// Written but not yet read: rule 4 says all testimony is dated, and a
    /// survey that has not come back to a cell for a minute is showing a
    /// minute-old reading with nothing on screen saying so. The panel needs this
    /// to say it, and does not yet.
    #[allow(dead_code)]
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
        while self.history.len() > HISTORY_COLUMNS {
            self.history.pop_front();
        }
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

    /// **Locking means "stay here". Leaving the section means "put it back".**
    ///
    /// Treating the two as one exit was the bug: the survey restored the
    /// pre-survey tuning on both, while the key handler logged
    /// `NET locked to <the current hop>`. The log said one thing and the radio
    /// did another, and the user pressed the key precisely because of what they
    /// were looking at.
    #[test]
    fn locking_keeps_the_position_and_leaving_gives_it_back() {
        // A survey started at 2412 and is currently parked on 2442.
        let mut net = NetState {
            pre_survey_hz: Some(2_412_000_000),
            ..Default::default()
        };

        // Locked: the position the pass is on is the one the user meant.
        net.mode = NetMode::Lock;
        let exit = net.end(2_442_000_000);
        assert_eq!(exit.tune_hz, 2_442_000_000);
        assert!(exit.locked);

        // Still surveying and leaving the section: back where it was found.
        let mut net = NetState {
            pre_survey_hz: Some(2_412_000_000),
            ..Default::default()
        };
        let exit = net.end(2_442_000_000);
        assert_eq!(exit.tune_hz, 2_412_000_000);
        assert!(!exit.locked);
    }

    /// Safe on a state that never surveyed, and safe to call twice, because the
    /// quit path calls it unconditionally and the task may have called it first.
    #[test]
    fn ending_a_survey_that_never_ran_leaves_the_radio_alone() {
        let mut net = NetState::default();
        assert_eq!(net.end(2_437_000_000).tune_hz, 2_437_000_000);

        let mut net = NetState {
            pre_survey_hz: Some(2_412_000_000),
            ..Default::default()
        };
        assert_eq!(net.end(2_442_000_000).tune_hz, 2_412_000_000);
        // The task wrote the answer back; quitting must not move it again.
        assert_eq!(net.end(2_412_000_000).tune_hz, 2_412_000_000);
        assert_eq!(net.pre_survey_hz, None);
    }

    /// Leaving the section after locking does not undo the lock.
    ///
    /// The lock already took the interrupted tuning, so there is nothing left to
    /// restore and the radio stays where the user put it - which is what they
    /// asked for and is why `take` rather than a read is the right call.
    #[test]
    fn leaving_the_section_does_not_undo_a_lock() {
        let mut net = NetState {
            pre_survey_hz: Some(2_412_000_000),
            ..Default::default()
        };
        net.mode = NetMode::Lock;
        assert_eq!(net.end(2_442_000_000).tune_hz, 2_442_000_000);
        // Now the user leaves NET entirely, still locked.
        assert_eq!(net.end(2_442_000_000).tune_hz, 2_442_000_000);
    }

    fn addr(tail: u8) -> [u8; 6] {
        [0xa4, 0x83, 0xe7, 0x1c, 0x09, tail]
    }

    /// **The one that matters: the cursor stays on the device, not the row.**
    ///
    /// Rows move. A re-sort reorders them, and a live census reorders them
    /// anyway as counts change. A cursor stored as an index slides onto whatever
    /// lands in that row, which reads as the user's own mistake.
    #[test]
    fn the_cursor_follows_the_device_through_a_resort() {
        let by_packets = [addr(3), addr(1), addr(2)];
        let by_address = [addr(1), addr(2), addr(3)];
        let mut c = CensusState::default();

        c.move_cursor(&by_packets, 1);
        assert_eq!(c.selected, Some(addr(3)), "the first row of this ordering");
        assert_eq!(c.cursor(&by_packets), Some(0));

        // The table is re-sorted. The cursor is on the same device, further down.
        assert_eq!(c.cursor(&by_address), Some(2));

        // And a device that has gone leaves the cursor nowhere rather than on
        // whatever took its place.
        assert_eq!(c.cursor(&[addr(1), addr(2)]), None);
    }

    #[test]
    fn the_cursor_stops_at_the_ends_rather_than_wrapping() {
        let rows = [addr(1), addr(2), addr(3)];
        let mut c = CensusState::default();
        c.move_cursor(&rows, 1);
        assert_eq!(c.selected, Some(addr(1)));
        for _ in 0..5 {
            c.move_cursor(&rows, 1);
        }
        assert_eq!(c.selected, Some(addr(3)), "the bottom, and it stays there");
        for _ in 0..5 {
            c.move_cursor(&rows, -1);
        }
        assert_eq!(c.selected, Some(addr(1)), "and the top");

        // Up from nothing lands on the last row, down from nothing on the first:
        // the cursor enters the list from the direction it was moving.
        let mut c = CensusState::default();
        c.move_cursor(&rows, -1);
        assert_eq!(c.selected, Some(addr(3)));

        // An empty census has nothing to be on.
        let mut c = CensusState::default();
        c.move_cursor(&[], 1);
        assert_eq!(c.selected, None);
    }

    #[test]
    fn cycling_the_sort_key_walks_the_columns_and_comes_back() {
        let keys = crate::signal::net::census::SORT_KEYS.len();
        let mut c = CensusState::default();
        assert_eq!(c.sort, 0);
        for expected in 1..keys {
            c.cycle_sort();
            assert_eq!(c.sort, expected);
        }
        c.cycle_sort();
        assert_eq!(c.sort, 0, "and round again");

        // Reversing is a separate act from choosing the column.
        assert!(!c.descending);
        c.reverse();
        assert!(c.descending);
        assert_eq!(c.sort, 0, "reversing does not move the column");
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
