// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The gain step measurement: what a receiver's noise floor does when you move
//! one stage, and what that says about where the gain should sit.
//!
//! **Why this exists.** A modelled chain gives a noise figure from a datasheet.
//! Most radios sdrtop can now open have no such model, and inventing one is the
//! thing this whole backend refuses to do. But the question a noise figure is
//! *asked* for has an answer that needs no datasheet at all: raise one stage by
//! a known amount and watch the noise floor.
//!
//! - `ΔN ≈ ΔG`: the receiver's own noise dominates. Signal and noise rise
//!   together, so the extra gain buys nothing.
//! - `ΔN ≈ 0`: the converter's quantisation noise dominates. More gain genuinely
//!   improves sensitivity.
//! - The **knee** between them is the lowest gain at which the receiver is noise
//!   limited, which is where it should sit.
//!
//! What this cannot give is an absolute noise figure in dB. That needs a
//! calibrated noise source. The reading is relative and complete, and the panel
//! has to say so rather than implying otherwise.
//!
//! **One stage at a time, and that is not a detail.** Measured on a HackRF
//! through SoapyHackRF by walking the *whole chain* in 2 dB steps, the noise
//! floor came out looking like this:
//!
//! ```text
//! G= 2  +3.25    G=10  +3.19    G=18  +3.20
//! G= 4  +1.10    G=12  +1.40    G=20  +0.90
//! G= 6  -1.85    G=14  -1.64    G=22  -1.00
//! G= 8  -2.59    G=16  -2.66    G=24  -1.21
//! ```
//!
//! That is not noise, it is an eight dB sawtooth, and eight is the LNA's step.
//! The knob distributes front to back, so across each eight dB span the VGA
//! climbs and then the LNA jumps and the VGA drops back. The noise floor follows
//! the **arrangement**, because the front stage is what sets the noise figure,
//! not the total. A sweep that stepped the knob would measure its own
//! distribution policy and report it as the radio's behaviour.
//!
//! No clock and no device in here. The machine is fed readings and says what it
//! wants done, which is what makes it testable with neither.

/// Frames to discard after changing a stage, before believing the reading.
///
/// **Measured, not chosen.** On a HackRF through SoapyHackRF at 10 Msps, the
/// noise floor reached its new plateau within one or two frames of a gain
/// change, 0 to 67 ms at the ~15 frames per second the FFT publishes at. Three
/// is that with a margin, and it is cheap: it costs 200 ms per point.
#[allow(dead_code)] // read from M2
pub const SETTLE_FRAMES: u32 = 3;

/// Frames averaged into each point once settled.
///
/// Also measured: a settled plateau still moves 0.67 dB peak to peak frame to
/// frame. Eight frames brings that down to something a slope can be read from,
/// and costs about half a second per point.
#[allow(dead_code)] // read from M2
pub const AVERAGE_FRAMES: u32 = 8;

/// Slope, in dB of noise floor per dB of gain, above which the receiver counts
/// as noise limited.
///
/// The ideal is 1.0 and reality is below it, so this is a threshold rather than
/// a comparison: 0.7 says "most of the extra gain is showing up as noise, so the
/// front end is what we are listening to".
#[allow(dead_code)] // read from M2
pub const NOISE_LIMITED_SLOPE: f32 = 0.7;

/// What the machine wants the caller to do next.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    /// Put this stage at this value, then keep feeding readings.
    Set { stage: usize, db: f64 },
    /// Nothing to do; this reading was used or discarded.
    Hold,
    /// The sweep is over. The stage has already been asked to go back to where
    /// it started by a preceding `Set`.
    Done,
}

/// One measured point: the stage's value, and the noise floor there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub gain_db: f64,
    pub noise_dbfs: f32,
}

/// What the sweep found.
#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    /// Every point visited, in the order visited.
    pub points: Vec<Point>,
    /// dB of noise floor per dB of gain, across the whole span.
    pub slope: f32,
    /// The lowest gain at which the receiver is already noise limited, when the
    /// sweep found one. `None` means it never was, which is itself an answer:
    /// on this stage, more gain still buys sensitivity.
    pub knee_db: Option<f64>,
}

/// The sweep, as a state machine.
///
/// Drive it by calling [`Self::feed`] once per FFT frame with that frame's noise
/// floor, and doing whatever the returned [`Action`] says.
#[derive(Clone, Debug)]
pub struct GainSweep {
    stage: usize,
    plan: Vec<f64>,
    at: usize,
    restore_db: f64,
    settling: u32,
    samples: Vec<f32>,
    points: Vec<Point>,
    finished: bool,
    /// A `Set` has been emitted and the radio has not been told yet.
    ///
    /// **The two halves of this measurement run on different threads.** Readings
    /// arrive on the FFT worker, which must never touch the device; device calls
    /// belong to the poll task, which runs at its own pace. So a step is emitted
    /// here, applied there, and the settle count does not start until
    /// [`Self::applied`] says the radio has actually moved. Counting from the
    /// moment the machine *decided* would fold up to one poll period of the old
    /// setting into the new point.
    pending: Option<(usize, f64)>,
    /// The sweep was stopped rather than completed.
    ///
    /// Kept apart from `finished` because the two mean different things to the
    /// caller: a completed sweep has an answer, a stopped one has some points
    /// and no answer. Two points do define a line, and printing that line as the
    /// result of a measurement the user deliberately interrupted would be the
    /// instrument answering a question it was not allowed to finish asking.
    aborted: bool,
}

impl GainSweep {
    /// Plan a sweep of one stage across `values`, returning to `restore_db`.
    ///
    /// `values` is the caller's business because only it knows the stage's own
    /// grid; this walks whatever list it is given, in order.
    pub fn new(stage: usize, values: Vec<f64>, restore_db: f64) -> Self {
        Self {
            stage,
            plan: values,
            at: 0,
            restore_db,
            settling: SETTLE_FRAMES,
            samples: Vec::new(),
            points: Vec::new(),
            finished: false,
            pending: None,
            aborted: false,
        }
    }

    /// The first thing to do: go to the first point.
    ///
    /// Separate from `feed` so a caller cannot start collecting before the stage
    /// has been moved, which would fold one reading of the old setting into the
    /// first point.
    pub fn begin(&mut self) -> Action {
        match self.plan.first() {
            Some(&db) => {
                self.pending = Some((self.stage, db));
                Action::Set {
                    stage: self.stage,
                    db,
                }
            }
            None => {
                self.finished = true;
                Action::Done
            }
        }
    }

    /// What the radio still needs to be told, if anything.
    ///
    /// The poll task reads this, drops the lock, makes the call, and then says
    /// [`Self::applied`].
    pub fn pending_set(&self) -> Option<(usize, f64)> {
        self.pending
    }

    /// The radio has been moved. Start counting settle frames from now.
    pub fn applied(&mut self) {
        self.pending = None;
        self.settling = SETTLE_FRAMES;
        self.samples.clear();
    }

    /// Fold in one frame's noise floor.
    pub fn feed(&mut self, noise_dbfs: f32) -> Action {
        if self.finished || self.at >= self.plan.len() {
            return Action::Done;
        }
        // Still waiting for the radio to be moved: this frame is of the previous
        // setting and belongs to nothing.
        if self.pending.is_some() {
            return Action::Hold;
        }
        if self.settling > 0 {
            self.settling -= 1;
            return Action::Hold;
        }
        if noise_dbfs.is_finite() {
            self.samples.push(noise_dbfs);
        }
        if (self.samples.len() as u32) < AVERAGE_FRAMES {
            return Action::Hold;
        }

        let mean = self.samples.iter().sum::<f32>() / self.samples.len() as f32;
        self.points.push(Point {
            gain_db: self.plan[self.at],
            noise_dbfs: mean,
        });
        self.samples.clear();
        self.settling = SETTLE_FRAMES;
        self.at += 1;

        match self.plan.get(self.at) {
            Some(&db) => {
                self.pending = Some((self.stage, db));
                Action::Set {
                    stage: self.stage,
                    db,
                }
            }
            None => {
                self.finished = true;
                self.pending = Some((self.stage, self.restore_db));
                // The last thing the sweep does is put the stage back. It is a
                // `Set` rather than something the caller has to remember,
                // because a measurement that leaves the radio somewhere else is
                // a measurement nobody will run twice.
                Action::Set {
                    stage: self.stage,
                    db: self.restore_db,
                }
            }
        }
    }

    /// Give up now. The stage goes back to where it started.
    ///
    /// **Every exit restores**, including this one. An interrupted sweep that
    /// left the gain half way through its walk would be worse than one that
    /// never ran.
    pub fn abort(&mut self) -> Action {
        self.finished = true;
        self.aborted = true;
        self.pending = Some((self.stage, self.restore_db));
        Action::Set {
            stage: self.stage,
            db: self.restore_db,
        }
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Points measured so far, and points planned.
    pub fn steps(&self) -> (usize, usize) {
        (self.points.len(), self.plan.len())
    }

    /// The stage being swept and the value it must be put back to.
    pub fn restore(&self) -> (usize, f64) {
        (self.stage, self.restore_db)
    }

    /// Which stage is being swept.
    pub fn stage(&self) -> usize {
        self.stage
    }

    /// How far along, 0.0 to 1.0, for a progress indicator.
    pub fn progress(&self) -> f32 {
        if self.plan.is_empty() {
            return 1.0;
        }
        self.at as f32 / self.plan.len() as f32
    }

    /// What was found, or `None` if the sweep was stopped or measured fewer than
    /// two points: a slope needs two, and a stopped sweep has no answer at all.
    pub fn reading(&self) -> Option<Reading> {
        if self.aborted || self.points.len() < 2 {
            return None;
        }
        let first = self.points.first()?;
        let last = self.points.last()?;
        let span = (last.gain_db - first.gain_db) as f32;
        let slope = if span.abs() > f32::EPSILON {
            (last.noise_dbfs - first.noise_dbfs) / span
        } else {
            0.0
        };
        Some(Reading {
            points: self.points.clone(),
            slope,
            knee_db: self.knee(),
        })
    }

    /// The lowest gain from which the receiver stays noise limited.
    ///
    /// Read from the **local** slope between neighbouring points rather than
    /// from the overall one, because the whole point of the knee is that the
    /// behaviour changes part way along. Once found, the rest of the sweep has
    /// to agree: a single steep pair in the middle of a flat curve is noise, not
    /// a knee.
    fn knee(&self) -> Option<f64> {
        let slopes: Vec<(f64, f32)> = self
            .points
            .windows(2)
            .filter_map(|w| {
                let d = (w[1].gain_db - w[0].gain_db) as f32;
                (d.abs() > f32::EPSILON)
                    .then(|| (w[0].gain_db, (w[1].noise_dbfs - w[0].noise_dbfs) / d))
            })
            .collect();
        slopes
            .iter()
            .position(|(_, s)| *s >= NOISE_LIMITED_SLOPE)
            .filter(|&i| slopes[i..].iter().all(|(_, s)| *s >= NOISE_LIMITED_SLOPE))
            .map(|i| slopes[i].0)
    }
}

/// The most points a sweep will visit.
///
/// Each point costs `SETTLE_FRAMES + AVERAGE_FRAMES` frames, so the whole
/// measurement is this many times that. At the measured ~15 frames/s that is
/// about six seconds for eight points, which is short enough that the antenna
/// and the band have not moved underneath it.
pub const MAX_POINTS: usize = 8;

/// Where to sample a stage.
///
/// Not the same question as "what settings does this stage have". A gain table
/// with twenty-nine entries does not need twenty-nine measurements; the slope is
/// a straight line fit and the ends carry it. So this spreads at most
/// [`MAX_POINTS`] evenly across the stage's real positions, always including
/// both ends, and returns nothing for a stage that cannot give an answer.
///
/// A switch is refused deliberately. Two points do define a line, but a boost
/// that is either in or out is not a gain axis: the number that came back would
/// be the difference between two front ends, printed as a slope.
pub fn plan_for(spec: &crate::hardware::StageSpec) -> Vec<f64> {
    let Some(n) = spec.positions() else {
        return Vec::new();
    };
    if n < 3 {
        return Vec::new();
    }
    let all: Vec<f64> = if spec.table.is_empty() {
        (0..n)
            .map(|i| spec.min_db + f64::from(i) * spec.step_db)
            .collect()
    } else {
        spec.table.clone()
    };
    if all.len() <= MAX_POINTS {
        return all;
    }
    // Evenly spaced indices with both ends included: i * (len-1) / (MAX-1).
    let last = all.len() - 1;
    (0..MAX_POINTS)
        .map(|i| all[i * last / (MAX_POINTS - 1)])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranged(min_db: f64, max_db: f64, step_db: f64) -> crate::hardware::StageSpec {
        crate::hardware::StageSpec {
            name: "LNA".into(),
            min_db,
            max_db,
            step_db,
            table: Vec::new(),
        }
    }

    #[test]
    fn a_stopped_sweep_reports_no_reading_however_far_it_got() {
        let mut s = GainSweep::new(0, vec![0.0, 8.0, 16.0, 24.0], 40.0);
        s.begin();
        s.applied();
        // Two full points measured, which is enough for a slope.
        for _ in 0..2 * (SETTLE_FRAMES + AVERAGE_FRAMES) {
            if let Action::Set { .. } = s.feed(-90.0) {
                s.applied();
            }
        }
        assert!(s.steps().0 >= 2, "the run must reach two points first");
        s.abort();
        assert_eq!(
            s.reading(),
            None,
            "a stopped sweep has points but no answer"
        );
    }

    #[test]
    fn frames_arriving_before_the_radio_moves_are_discarded() {
        // The gap between deciding a step and the poll task making the call is
        // real, and every frame in it is of the *old* setting. Counting them as
        // settle frames would fold the old gain into the new point.
        let mut s = GainSweep::new(0, vec![0.0, 8.0], 0.0);
        assert_eq!(s.begin(), Action::Set { stage: 0, db: 0.0 });
        for _ in 0..50 {
            assert_eq!(s.feed(-90.0), Action::Hold);
        }
        assert_eq!(
            s.steps(),
            (0, 2),
            "nothing may be measured before the set lands"
        );
        s.applied();
        // Now the settle count runs, and only then does a point get measured.
        for _ in 0..SETTLE_FRAMES {
            assert_eq!(s.feed(-90.0), Action::Hold);
        }
        assert_eq!(s.steps(), (0, 2));
        for _ in 0..AVERAGE_FRAMES {
            s.feed(-90.0);
        }
        assert_eq!(s.steps(), (1, 2));
    }

    #[test]
    fn a_short_stage_is_swept_at_every_setting() {
        // HackRF LNA: 0..40 in 8 dB steps is six settings, and six is under the
        // cap, so nothing is skipped.
        assert_eq!(
            plan_for(&ranged(0.0, 40.0, 8.0)),
            vec![0.0, 8.0, 16.0, 24.0, 32.0, 40.0]
        );
    }

    #[test]
    fn a_long_gain_table_is_thinned_but_keeps_both_ends() {
        let mut spec = ranged(0.0, 0.0, 0.0);
        spec.table = (0..29).map(|i| f64::from(i) * 1.4).collect();
        let plan = plan_for(&spec);
        assert_eq!(plan.len(), MAX_POINTS);
        assert_eq!(plan.first(), Some(&0.0));
        assert_eq!(plan.last(), Some(&(28.0 * 1.4)));
        // Strictly increasing: no setting is visited twice.
        assert!(plan.windows(2).all(|w| w[1] > w[0]), "{plan:?}");
    }

    #[test]
    fn a_switch_gives_no_plan() {
        // An RF amp is on or off. Two points make a line, but not this line.
        assert!(plan_for(&ranged(0.0, 14.0, 14.0)).is_empty());
    }

    #[test]
    fn a_stage_that_cannot_move_gives_no_plan() {
        assert!(plan_for(&ranged(0.0, 0.0, 0.0)).is_empty());
        assert!(plan_for(&ranged(20.0, 20.0, 1.0)).is_empty());
    }

    /// Run a whole sweep against a synthetic receiver, returning what it found.
    ///
    /// `noise_at` is the radio: given the stage's current value it says where the
    /// noise floor sits. That is the entire coupling to hardware, which is what
    /// lets this file be tested with neither a radio nor a library.
    fn run(values: Vec<f64>, restore: f64, noise_at: impl Fn(f64) -> f32) -> (Reading, f64) {
        let mut s = GainSweep::new(0, values, restore);
        let mut current = match s.begin() {
            Action::Set { db, .. } => db,
            _ => panic!("a sweep with points must start by setting one"),
        };
        s.applied();
        for _ in 0..10_000 {
            match s.feed(noise_at(current)) {
                Action::Set { db, .. } => {
                    current = db;
                    s.applied();
                }
                Action::Hold => {}
                Action::Done => break,
            }
            if s.is_finished() {
                break;
            }
        }
        assert!(s.is_finished(), "the sweep never ended");
        (s.reading().expect("two points at least"), current)
    }

    /// A receiver whose own noise dominates: the floor tracks the gain one for
    /// one, so the knee is at the very first point.
    #[test]
    fn a_noise_limited_receiver_reads_a_slope_of_one() {
        let (r, _) = run(vec![0.0, 8.0, 16.0, 24.0], 16.0, |g| -100.0 + g as f32);
        assert_eq!(r.points.len(), 4);
        assert!((r.slope - 1.0).abs() < 1e-3, "{}", r.slope);
        assert_eq!(r.knee_db, Some(0.0), "already noise limited at the bottom");
    }

    /// A receiver the converter is listening to: adding gain moves the signal
    /// and leaves the noise where it was, so there is no knee at all.
    #[test]
    fn a_converter_limited_receiver_reads_a_flat_slope_and_no_knee() {
        let (r, _) = run(vec![0.0, 8.0, 16.0, 24.0], 0.0, |_| -95.0);
        assert!(r.slope.abs() < 1e-3, "{}", r.slope);
        assert_eq!(r.knee_db, None, "more gain still buys sensitivity");
    }

    /// The case the bench is for: flat, then a knee, then noise limited.
    #[test]
    fn a_receiver_with_a_knee_reports_where_it_is() {
        // Flat to 16 dB, then one for one above it.
        let (r, _) = run(vec![0.0, 8.0, 16.0, 24.0, 32.0], 8.0, |g| {
            if g <= 16.0 {
                -100.0
            } else {
                -100.0 + (g - 16.0) as f32
            }
        });
        assert_eq!(r.knee_db, Some(16.0));
        assert!(
            r.slope > 0.4 && r.slope < 0.6,
            "half the span climbs: {}",
            r.slope
        );
    }

    /// **The sweep always puts the stage back.** A measurement that leaves the
    /// radio somewhere else is one nobody runs twice.
    #[test]
    fn the_stage_goes_back_where_it_started() {
        let (_, ended_at) = run(vec![0.0, 8.0, 16.0], 24.0, |g| -100.0 + g as f32);
        assert_eq!(ended_at, 24.0);
    }

    /// Including when it is interrupted part way.
    #[test]
    fn aborting_restores_too() {
        let mut s = GainSweep::new(1, vec![0.0, 8.0, 16.0], 40.0);
        assert_eq!(s.begin(), Action::Set { stage: 1, db: 0.0 });
        s.applied();
        for _ in 0..5 {
            s.feed(-90.0);
        }
        assert!(!s.is_finished());
        assert_eq!(s.abort(), Action::Set { stage: 1, db: 40.0 });
        assert!(s.is_finished());
        assert_eq!(s.feed(-90.0), Action::Done, "and it stays finished");
    }

    /// The settle frames are discarded, not averaged in. Feeding the machine a
    /// transient and then a plateau must produce the plateau.
    #[test]
    fn the_frames_right_after_a_step_are_thrown_away() {
        let mut s = GainSweep::new(0, vec![0.0, 8.0], 0.0);
        s.begin();
        s.applied();
        for _ in 0..SETTLE_FRAMES {
            assert_eq!(s.feed(-40.0), Action::Hold, "a transient, discarded");
        }
        for _ in 0..AVERAGE_FRAMES {
            s.feed(-90.0);
        }
        // First point is complete; it must be the plateau, not the transient.
        let mid = s.points[0].noise_dbfs;
        assert!((mid + 90.0).abs() < 1e-3, "settled value, got {mid}");
    }

    /// Frame to frame the floor moves about 0.67 dB peak to peak on real
    /// hardware, so a point is an average and a single noisy frame must not
    /// decide anything.
    #[test]
    fn a_point_is_an_average_rather_than_one_frame() {
        let mut s = GainSweep::new(0, vec![0.0, 8.0], 0.0);
        s.begin();
        s.applied();
        for _ in 0..SETTLE_FRAMES {
            s.feed(-90.0);
        }
        let dither = [-90.3, -89.7, -90.4, -89.6, -90.2, -89.8, -90.1, -89.9];
        for v in dither {
            s.feed(v);
        }
        assert!(
            (s.points[0].noise_dbfs + 90.0).abs() < 0.05,
            "the dither should average out, got {}",
            s.points[0].noise_dbfs
        );
    }

    /// One steep pair in an otherwise flat curve is noise, not a knee.
    #[test]
    fn a_single_steep_pair_is_not_a_knee() {
        let noise = |g: f64| {
            if (g - 16.0).abs() < 1e-9 {
                -98.0
            } else {
                -100.0
            }
        };
        let (r, _) = run(vec![0.0, 8.0, 16.0, 24.0, 32.0], 0.0, noise);
        assert_eq!(r.knee_db, None, "it does not stay noise limited after");
    }

    /// A stage with nowhere to go, and a reading asked for too early.
    #[test]
    fn a_sweep_with_no_points_is_answered_rather_than_indexed() {
        let mut s = GainSweep::new(0, Vec::new(), 12.0);
        assert_eq!(s.begin(), Action::Done);
        assert_eq!(s.progress(), 1.0);
        assert_eq!(s.reading(), None);

        let mut one = GainSweep::new(0, vec![5.0], 12.0);
        one.begin();
        one.applied();
        for _ in 0..(SETTLE_FRAMES + AVERAGE_FRAMES) {
            one.feed(-90.0);
        }
        assert!(one.is_finished());
        assert_eq!(one.reading(), None, "one point is not a slope");
    }

    /// A frame that is not a number must not poison the average.
    #[test]
    fn a_nonsense_reading_is_skipped() {
        let mut s = GainSweep::new(0, vec![0.0, 8.0], 0.0);
        s.begin();
        s.applied();
        for _ in 0..SETTLE_FRAMES {
            s.feed(-90.0);
        }
        s.feed(f32::NAN);
        s.feed(f32::INFINITY);
        for _ in 0..AVERAGE_FRAMES {
            s.feed(-90.0);
        }
        assert!((s.points[0].noise_dbfs + 90.0).abs() < 1e-3);
    }
}
