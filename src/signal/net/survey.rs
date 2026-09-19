// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Where to point the radio, and for how long, to see the whole band.
//!
//! A receiver that reaches this section sees eighteen or twenty megahertz of an
//! eighty-three megahertz band, so the only way to survey it is to hop. Design
//! section 13.1 makes the consequence part of the reading rather than a footnote:
//! **a duty-cycle-sampled census and a complete capture are different claims**,
//! and every number gathered this way is marked with how it was gathered.
//!
//! Everything here is plain arithmetic over plain data, so the plan can be
//! asserted with no radio anywhere. The task that executes it only steers the
//! tuner and waits.

use std::time::Duration;

use super::{band, occupancy};

/// Samples discarded after a retune while the PLL settles.
///
/// The same figure the frequency sweep uses, from `state::SWEEP_SETTLING_MS`,
/// because it is a fact about the radio rather than about either feature. Two
/// numbers here would be two chances to disagree about the same PLL.
pub const SETTLE: Duration = Duration::from_millis(crate::state::SWEEP_SETTLING_MS);

/// How long to sit on one position.
///
/// Twice the dwell the scan needs to publish a measurement, because the feed is
/// lossy: fifty milliseconds of *observation* takes more than fifty milliseconds
/// of wall clock whenever a block is dropped, and a hop that moved on at exactly
/// fifty would publish nothing on the positions where it mattered most.
pub const DWELL: Duration = Duration::from_millis(100);

/// One pass across the band.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// Centre frequencies, low to high.
    pub hops: Vec<u64>,
    /// What each of them sees.
    pub span_hz: f64,
}

impl Plan {
    /// The positions needed to cover the band with a receiver seeing `span_hz`.
    ///
    /// **Derived from the radio, not chosen.** The number of positions is the
    /// fewest whose spans cover the band, and they are spread evenly across it
    /// so the overlap is shared rather than piled up at one end. Rounding the
    /// centres by at most half a megahertz cannot open a gap in a plan whose
    /// steps are already shorter than its spans.
    ///
    /// **Centres land in the middle of a cell, not on the boundary between
    /// two.** A hop is blind in its own DC, and a centre on a boundary spreads
    /// that blindness across the two cells either side of it - so the one-cell
    /// walk of [`Self::DODGE_HZ`] would leave the middle one dark on both
    /// passes. Half a megahertz along, the shadow is one cell wide and the walk
    /// clears it.
    /// How far the positions walk on alternate passes.
    ///
    /// **One cell, and it exists because of the DC guard.** Every hop is blind
    /// in the megahertz it is tuned to - the front end's own leakage sits there
    /// and `occupancy::bin_cells` drops those bins rather than reporting the
    /// artefact as a fully busy channel. The positions do not overlap at their
    /// centres, so a fixed plan would leave one permanent blind stripe per
    /// position: five megahertz of band the survey never mentions.
    ///
    /// Walking the whole plan one cell along on alternate passes costs nothing -
    /// the same number of positions, the same dwell - and puts every blind cell
    /// of one pass in the clear on the next.
    pub const DODGE_HZ: u64 = occupancy::CELL_HZ;

    pub fn for_span(span_hz: f64, pass: u64) -> Self {
        // Spread over the band plus the half cell the mid-cell rounding below
        // can move a position: without it the first centre rounds *up*, its span
        // starts half a megahertz inside the band, and cell zero is never
        // measured by any pass.
        let half = occupancy::CELL_HZ as f64 / 2.0;
        let low = band::LOW_HZ as f64 - half;
        let width = band::HIGH_HZ as f64 + half - low;
        if !span_hz.is_finite() || span_hz <= 0.0 {
            return Self {
                hops: Vec::new(),
                span_hz: 0.0,
            };
        }
        // One position is enough for a receiver that can see the whole band, and
        // for one that cannot see a whole cell there is nothing to plan.
        let n = (width / span_hz).ceil().max(1.0) as usize;
        let first = low + span_hz / 2.0;
        let step = if n > 1 {
            (width - span_hz) / (n - 1) as f64
        } else {
            0.0
        };
        let dodge = (pass % 2) * Self::DODGE_HZ;
        let hops = (0..n)
            .map(|k| {
                let hz = first + k as f64 * step;
                let cell = occupancy::CELL_HZ as f64;
                let mid = ((hz - cell / 2.0) / cell).round() as u64 * occupancy::CELL_HZ
                    + occupancy::CELL_HZ / 2;
                mid + dodge
            })
            .collect();
        Self { hops, span_hz }
    }

    /// How long one pass takes.
    pub fn cycle(&self) -> Duration {
        (SETTLE + DWELL) * self.hops.len() as u32
    }

    /// Whether this plan measures any cell at all.
    ///
    /// A receiver whose usable span is narrower than a cell plus its own DC
    /// guard has positions to visit and nothing to learn at any of them. Better
    /// to say so once than to hop for ever publishing an empty band.
    pub fn covers_anything(&self, rate_hz: f64, bins: usize) -> bool {
        self.covered(rate_hz, bins).iter().any(|c| *c)
    }

    /// Every cell this pass actually measures: inside a span, and not in a
    /// position's own DC shadow.
    ///
    /// `bins` is the transform size the scan will use, because the shadow's
    /// width is a bin count. The plan is only a plan if two consecutive passes
    /// between them cover the band; `the_plan_covers_the_whole_band` is the
    /// assertion, and it is the one that matters, because a gap here is a
    /// stretch of spectrum the survey reports as unobserved for ever without
    /// anything saying why.
    pub fn covered(&self, rate_hz: f64, bins: usize) -> Vec<bool> {
        let mut seen = vec![false; occupancy::CELLS];
        for hz in &self.hops {
            let shadow = occupancy::dc_shadow(*hz as f64, rate_hz, bins);
            for c in occupancy::cells_observed(*hz as f64, self.span_hz) {
                seen[c] |= !shadow.contains(&c);
            }
        }
        seen
    }
}

/// Where to lock the radio so the megahertz `cell` is received, and why there.
///
/// **Never on the cell's own centre, unless the decoder needs it.** Every
/// tuning is blind in its own DC: the front end's leakage sits on the tuned
/// frequency and `occupancy` drops those bins (`DC_GUARD_BINS`). Locking a cell
/// on its centre would put the one megahertz the user asked about in that
/// shadow. So the radio goes one cell along ([`Plan::DODGE_HZ`], the step the
/// survey dodges its own DC with), up, or down at the top of the band.
///
/// **Except on a BLE advertising channel**, where the receiver decodes only when
/// tuned to the channel's exact centre (`signal::ble::channel::channel_of`,
/// `ble_refused` otherwise). There the centre is what locking is for, and the
/// reason says so.
pub fn lock_target(cell: usize) -> crate::state::LockTarget {
    let cell = cell.min(occupancy::CELLS - 1);
    let lo = band::LOW_HZ + cell as u64 * occupancy::CELL_HZ;
    let hi = lo + occupancy::CELL_HZ;
    if let Some((channel, hz)) = [37u8, 38, 39]
        .into_iter()
        .zip(crate::signal::ble::channel::advertising_channels_hz())
        .find(|(_, hz)| (lo..hi).contains(hz))
    {
        return crate::state::LockTarget {
            tune_hz: hz,
            why: format!("on BLE advertising channel {channel}'s centre, where the decoder runs"),
        };
    }
    let centre = occupancy::cell_centre_hz(cell);
    let (tune_hz, side) = if centre + Plan::DODGE_HZ < band::HIGH_HZ {
        (centre + Plan::DODGE_HZ, "above")
    } else {
        (centre - Plan::DODGE_HZ, "below")
    };
    crate::state::LockTarget {
        tune_hz,
        why: format!(
            "{} MHz {side} {:.1} MHz, so the cell is clear of the radio's own DC",
            Plan::DODGE_HZ / 1_000_000,
            centre as f64 / 1e6
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lock lands one cell away from the cell asked for, up or, at the top
    /// of the band, down, so the cell is out of the tuned DC; on a BLE
    /// advertising channel's cell it lands on the channel's exact centre.
    #[test]
    fn a_lock_dodges_its_own_dc_except_where_the_decoder_needs_the_centre() {
        let t = lock_target(41);
        assert_eq!(t.tune_hz, occupancy::cell_centre_hz(41) + Plan::DODGE_HZ);
        assert!(t.why.contains("above 2441.5 MHz"), "{}", t.why);
        let top = lock_target(occupancy::CELLS - 1);
        assert_eq!(
            top.tune_hz,
            occupancy::cell_centre_hz(occupancy::CELLS - 1) - Plan::DODGE_HZ
        );
        assert!(top.why.contains("below"), "{}", top.why);
        for (cell, ch, hz) in [
            (2usize, 37u8, 2_402_000_000u64),
            (26, 38, 2_426_000_000),
            (80, 39, 2_480_000_000),
        ] {
            let t = lock_target(cell);
            assert_eq!(t.tune_hz, hz, "cell {cell}");
            assert!(t.why.contains(&format!("channel {ch}")), "{}", t.why);
            assert_eq!(crate::signal::ble::channel::channel_of(t.tune_hz), Some(ch));
        }
    }

    /// **The one that matters.** A gap here is a stretch of band the survey
    /// reports as unobserved for ever, with nothing on screen saying why.
    ///
    /// Two consecutive passes, because one pass cannot cover the band: every
    /// position is blind in the megahertz it is tuned to, and the positions do
    /// not overlap at their centres. The plan walks one cell along on alternate
    /// passes so each pass covers what the other could not.
    #[test]
    fn the_plan_covers_the_whole_band() {
        for span in [
            4_000_000.0,
            8_000_000.0,
            10_000_000.0,
            18_000_000.0,
            20_000_000.0,
            40_000_000.0,
            100_000_000.0,
        ] {
            // The rate a radio with this usable span would be running at, and
            // the transform the scan picks for it.
            let rate = span * 20.0 / 18.0;
            let bins = crate::signal::net::scan::bins_for(rate);
            let mut covered = [false; occupancy::CELLS];
            for pass in 0..2u64 {
                for (c, seen) in Plan::for_span(span, pass)
                    .covered(rate, bins)
                    .iter()
                    .enumerate()
                {
                    covered[c] |= *seen;
                }
            }
            let missed: Vec<usize> = (0..occupancy::CELLS).filter(|c| !covered[*c]).collect();
            assert!(missed.is_empty(), "span {span}: missed cells {missed:?}");
        }
    }

    /// **A two-megahertz receiver cannot survey this band, and the plan says so
    /// rather than hopping for ever producing nothing.**
    ///
    /// The gate admits it: two megasamples is what the cheapest PHY needs, so a
    /// radio that reaches the band and can run at that rate gets the section.
    /// But a two-megahertz span holds one whole cell, and that cell is the one
    /// the local oscillator sits in - excluded, because otherwise it reads as a
    /// fully busy megahertz. One megahertz of view, none of it usable.
    ///
    /// Receiving a channel and surveying a band are different questions and the
    /// gate only asks the first. This is where the second is answered.
    #[test]
    fn a_receiver_too_narrow_to_see_past_its_own_oscillator_covers_nothing() {
        let rate = 2_200_000.0;
        let bins = crate::signal::net::scan::bins_for(rate);
        let plan = Plan::for_span(2_000_000.0, 0);
        assert!(!plan.hops.is_empty(), "it still has positions to visit");
        assert!(
            !plan.covers_anything(rate, bins),
            "a 2 MHz span cannot see past its own DC"
        );
        // Twice that, and it works.
        assert!(Plan::for_span(4_000_000.0, 0).covers_anything(4_400_000.0, bins));
    }

    /// And one pass on its own does *not* cover it, which is why there are two.
    ///
    /// Written as an assertion rather than left implied: if a future change made
    /// one pass sufficient, the alternation would be dead weight nobody would
    /// think to remove, and if it made two insufficient the test above would
    /// fail without saying what changed.
    #[test]
    fn a_single_pass_is_blind_where_it_is_tuned() {
        let span = 18_000_000.0;
        let rate = 20_000_000.0;
        let bins = crate::signal::net::scan::bins_for(rate);
        let plan = Plan::for_span(span, 0);
        let covered = plan.covered(rate, bins);
        let blind: Vec<usize> = (0..occupancy::CELLS).filter(|c| !covered[*c]).collect();
        assert_eq!(
            blind.len(),
            plan.hops.len(),
            "one blind cell per position: {blind:?}"
        );
        // And each blind cell is a position's own centre.
        for hz in &plan.hops {
            let cell = occupancy::cell_of(*hz as f64).unwrap();
            assert!(blind.contains(&cell), "{hz} is not blind but should be");
        }
        // The next pass has them, and is blind one cell along.
        let next = Plan::for_span(span, 1).covered(rate, bins);
        for c in blind {
            assert!(next[c], "cell {c} is blind on both passes");
        }
    }

    /// A pass is what its parts add up to, and the cycle time is what sets how
    /// much of the band any one reading covers.
    #[test]
    fn a_pass_is_the_settle_and_the_dwell_at_every_position() {
        let plan = Plan::for_span(18_000_000.0, 0);
        assert_eq!(plan.cycle(), (SETTLE + DWELL) * 5);
        assert_eq!(plan.cycle(), Duration::from_millis(625));
        // Which is the coverage every cell in the section is reported at: one
        // dwell in every pass.
        let coverage = DWELL.as_secs_f64() / plan.cycle().as_secs_f64();
        assert!((coverage - 0.16).abs() < 0.01, "{coverage}");
    }
}
