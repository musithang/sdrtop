// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! What a recording knows about itself, decided from what each block carries.
//!
//! No file, no clock of its own, no lock: the writer hands each message here,
//! is told how many pairs of it to write and whether to stop, and writes. So
//! every rule about segments, losses, limits and time is tested with numbers.
//!
//! **Two accounts of time, kept apart.** Positions count the samples the
//! driver says it delivered or dropped; each block's arrival on the system
//! clock is the other. Where they disagree, the radio delivered fewer samples
//! than its rate without saying so, and the recording says it instead.

/// The wall time a recording must span before its delivered rate is stated:
/// blocks arrive in bursts a few milliseconds apart, so a shorter span would
/// measure the bursts.
const MIN_WALL_S: f64 = 2.0;

/// How far short of its rate the radio may run before the recording says so.
/// Arrival jitter over [`MIN_WALL_S`] is a few tenths of a percent; a real
/// shortfall on a starved USB link is tens of percent.
const SHORT_TOLERANCE: f64 = 0.01;

use crate::hardware::record_tap::{BlockAt, RadioNote};

/// One SigMF capture segment: where it starts in the file, where that is in
/// the stream the radio delivered, and what the radio was tuned to.
#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub sample_start: u64,
    pub global_index: u64,
    pub frequency_hz: u64,
    /// When the block holding this segment's first sample reached sdrtop, on
    /// the system clock: measured, never extrapolated from the sample count.
    pub arrived_unix: f64,
}

/// One SigMF annotation. Most mark a moment and carry a `sample_count` of
/// zero (SigMF 1.12: without one it would mean "to the end of the capture");
/// a drop inside the radio marks the stretch it is known to lie in.
#[derive(Clone, Debug, PartialEq)]
pub struct Annotation {
    pub sample_start: u64,
    pub sample_count: u64,
    pub label: String,
    pub comment: String,
}

/// Why a recording ended.
#[derive(Clone, Debug, PartialEq)]
pub enum Stop {
    /// The key, again.
    Asked,
    /// The length limit.
    Duration { secs: f64 },
    /// The size limit.
    Size { bytes: u64 },
    /// SigMF has one rate for a whole recording (1.10.2 is a global field).
    RateChanged { from_hz: f64, to_hz: f64 },
    /// The stream stopped or started again from zero.
    StreamEnded,
    /// The disk said no.
    Write(String),
}

impl Stop {
    /// The sentence the file and the log carry.
    pub fn sentence(&self) -> String {
        match self {
            Stop::Asked => "stopped by the user".to_string(),
            Stop::Duration { secs } => format!("reached the length limit, {secs} s"),
            Stop::Size { bytes } => {
                format!("reached the size limit, {:.1} GB", *bytes as f64 / 1e9)
            }
            Stop::RateChanged { from_hz, to_hz } => format!(
                "the sample rate changed from {:.3} to {:.3} Msps, and one recording has one rate",
                from_hz / 1e6,
                to_hz / 1e6
            ),
            Stop::StreamEnded => "the stream stopped".to_string(),
            Stop::Write(why) => format!("could not write: {why}"),
        }
    }
}

/// The two limits, whichever is reached first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    pub max_secs: f64,
    pub max_bytes: u64,
}

impl Limits {
    pub const DEFAULT: Limits = Limits {
        max_secs: 60.0,
        max_bytes: 4_000_000_000,
    };

    /// The pairs a recording may hold at `rate_hz`, and which limit that is.
    fn pairs(&self, rate_hz: f64, bytes_per_pair: u64) -> (u64, Stop) {
        let by_time = (self.max_secs * rate_hz).floor().max(0.0) as u64;
        let by_size = self.max_bytes / bytes_per_pair.max(1);
        if by_time <= by_size {
            (
                by_time,
                Stop::Duration {
                    secs: self.max_secs,
                },
            )
        } else {
            (
                by_size,
                Stop::Size {
                    bytes: self.max_bytes,
                },
            )
        }
    }

    /// The most bytes a recording at `rate_hz` can write.
    pub fn bytes(&self, rate_hz: f64, bytes_per_pair: u64) -> u64 {
        self.pairs(rate_hz, bytes_per_pair).0 * bytes_per_pair
    }
}

/// A recording in progress, as positions and counts.
#[derive(Clone, Debug)]
pub struct Recording {
    pub rate_hz: f64,
    bytes_per_pair: u64,
    max_pairs: u64,
    at_limit: Stop,
    /// Stream position of the first pair the recording was offered, which is
    /// `global_index` zero.
    origin: Option<u64>,
    /// Where the next block must start for the file to be unbroken.
    next: u64,
    pub written: u64,
    pub segments: Vec<Segment>,
    pub annotations: Vec<Annotation>,
    /// Missing pairs by cause, over the whole recording.
    pub lost_driver: u64,
    pub lost_queue: u64,
    pub lost_unexplained: u64,
    /// The hole the next written block will close: driver, queue, unexplained.
    pending: (u64, u64, u64),
    gain: Option<String>,
    /// `(arrival, stream position at the block's end)` for the first and
    /// the latest message, for the delivered rate.
    first_seen: Option<(f64, u64)>,
    last_seen: Option<(f64, u64)>,
    /// The radio's own drops while recording: `None` until a reading comes,
    /// which from a radio that keeps no count is never.
    pub radio_drops: Option<u64>,
    /// Why the radio's count could not be read, if it could not.
    pub radio_unreadable: Option<String>,
    /// Windows with drops in them, as stream positions, mapped to the file
    /// each time the metadata is written, so a bracket still in flight at one
    /// writing is whole at the next.
    radio_windows: Vec<(u32, u64, u64, u32)>,
}

impl Recording {
    pub fn new(rate_hz: f64, bytes_per_pair: u64, limits: Limits) -> Self {
        let (max_pairs, at_limit) = limits.pairs(rate_hz, bytes_per_pair);
        Self {
            rate_hz,
            bytes_per_pair,
            max_pairs,
            at_limit,
            origin: None,
            next: 0,
            written: 0,
            segments: Vec::new(),
            annotations: Vec::new(),
            lost_driver: 0,
            lost_queue: 0,
            lost_unexplained: 0,
            pending: (0, 0, 0),
            gain: None,
            first_seen: None,
            last_seen: None,
            radio_drops: None,
            radio_unreadable: None,
            radio_windows: Vec::new(),
        }
    }

    /// System-clock seconds from the first block to the latest.
    pub fn wall_seconds(&self) -> f64 {
        match (self.first_seen, self.last_seen) {
            (Some(a), Some(b)) => (b.0 - a.0).max(0.0),
            _ => 0.0,
        }
    }

    /// Samples per second the stream actually advanced by, drops the driver
    /// reported included, against the system clock. `None` until the
    /// recording has run long enough to say.
    pub fn delivered_hz(&self) -> Option<f64> {
        let (a, b) = (self.first_seen?, self.last_seen?);
        let wall = b.0 - a.0;
        (wall >= MIN_WALL_S).then(|| (b.1 - a.1) as f64 / wall)
    }

    /// The fraction of its rate the radio did not deliver, when that is past
    /// [`SHORT_TOLERANCE`] and nothing reported it.
    pub fn shortfall(&self) -> Option<f64> {
        let short = 1.0 - self.delivered_hz()? / self.rate_hz;
        (short > SHORT_TOLERANCE).then_some(short)
    }

    /// What a shortfall means for the file, as a sentence.
    pub fn shortfall_sentence(&self) -> Option<String> {
        let short = self.shortfall()?;
        Some(format!(
            "the radio delivered {:.1} % of its {:.3} Msps over {:.1} s of wall time and reported no drop: \
             {:.0} % of the samples are missing at places the file cannot show, so a sample's position \
             is not its time; the capture segments' datetimes are measured and remain right",
            (1.0 - short) * 100.0,
            self.rate_hz / 1e6,
            self.wall_seconds(),
            short * 100.0
        ))
    }

    pub fn bytes_written(&self) -> u64 {
        self.written * self.bytes_per_pair
    }

    /// Pairs missing from the file so far, of every cause.
    pub fn lost(&self) -> u64 {
        self.lost_driver + self.lost_queue + self.lost_unexplained
    }

    /// Account for the stretch between the last message and this one. `None`
    /// when the stream went backwards, which only a restart does.
    fn arrive(&mut self, at: &BlockAt) -> Option<()> {
        if self.origin.is_none() {
            self.origin = Some(at.first_pair);
            self.next = at.first_pair;
        }
        let seen = (at.arrived_unix, at.first_pair + at.pairs);
        self.first_seen.get_or_insert(seen);
        self.last_seen = Some(seen);
        let gap = at.first_pair.checked_sub(self.next)?;
        let driver = gap.min(at.driver_dropped);
        self.pending.0 += driver;
        self.pending.2 += gap - driver;
        self.lost_driver += driver;
        self.lost_unexplained += gap - driver;
        Some(())
    }

    /// A block the queue had no room for.
    pub fn refused(&mut self, at: &BlockAt) -> Option<Stop> {
        if self.arrive(at).is_none() {
            return Some(Stop::StreamEnded);
        }
        self.pending.1 += at.pairs;
        self.lost_queue += at.pairs;
        self.next = at.first_pair + at.pairs;
        None
    }

    /// A block to write: how many of its pairs go into the file (from its
    /// start), and whether the recording ends with it.
    pub fn block(&mut self, at: &BlockAt, gain: &str) -> (u64, Option<Stop>) {
        if at.rate_hz != self.rate_hz {
            return (
                0,
                Some(Stop::RateChanged {
                    from_hz: self.rate_hz,
                    to_hz: at.rate_hz,
                }),
            );
        }
        if self.arrive(at).is_none() {
            return (0, Some(Stop::StreamEnded));
        }
        let origin = self.origin.unwrap_or(at.first_pair);
        let global_index = at.first_pair - origin;
        let hole = self.pending;
        let missing = hole.0 + hole.1 + hole.2;
        let retuned = self
            .segments
            .last()
            .is_some_and(|s| s.frequency_hz != at.centre_hz);
        if self.segments.is_empty() || missing > 0 || retuned {
            self.segments.push(Segment {
                sample_start: self.written,
                global_index,
                frequency_hz: at.centre_hz,
                arrived_unix: at.arrived_unix,
            });
        }
        if missing > 0 && self.written > 0 {
            self.annotations.push(Annotation {
                sample_start: self.written,
                sample_count: 0,
                label: format!("lost {missing}"),
                comment: format!(
                    "{missing} pairs ({:.3} ms) missing before this sample: {} dropped by the driver, {} refused by the recorder's queue{}",
                    missing as f64 / self.rate_hz * 1e3,
                    hole.0,
                    hole.1,
                    if hole.2 > 0 {
                        format!(", {} for no reason sdrtop was told", hole.2)
                    } else {
                        String::new()
                    }
                ),
            });
        }
        if retuned {
            self.annotations.push(Annotation {
                sample_start: self.written,
                sample_count: 0,
                label: format!("tuned {:.3} MHz", at.centre_hz as f64 / 1e6),
                comment: "the frequency sdrtop had set when this block arrived; \
                          the radio's own settling after a retune is not in the file"
                    .to_string(),
            });
        }
        match &self.gain {
            Some(was) if was != gain => self.annotations.push(Annotation {
                sample_start: self.written,
                sample_count: 0,
                label: "gain".to_string(),
                comment: format!(
                    "{gain}: the setting when this block arrived, not the moment the radio applied it"
                ),
            }),
            _ => {}
        }
        self.gain = Some(gain.to_string());
        self.pending = (0, 0, 0);
        self.next = at.first_pair + at.pairs;

        let take = at.pairs.min(self.max_pairs - self.written);
        self.written += take;
        let stop = (self.written >= self.max_pairs).then(|| self.at_limit.clone());
        (take, stop)
    }

    /// A reading of the radio's own count. A window that ends before the
    /// recording's first sample belongs to no part of it.
    pub fn radio(&mut self, note: RadioNote) {
        match note {
            RadioNote::Unreadable(why) => self.radio_unreadable = Some(why),
            RadioNote::Counted {
                events,
                from_pair,
                to_pair,
                longest_bytes,
            } => {
                let started = self.origin.is_some_and(|o| to_pair >= o);
                let total = self.radio_drops.get_or_insert(0);
                if started && events > 0 {
                    *total += u64::from(events);
                    self.radio_windows
                        .push((events, from_pair, to_pair, longest_bytes));
                }
            }
        }
    }

    /// Where a stream position lies in the file: through the segment that
    /// holds it, at a hole the start of the segment after it, and never past
    /// what is written. `None` before the first block.
    pub fn file_index(&self, position: u64) -> Option<u64> {
        let origin = self.origin?;
        let global = position.saturating_sub(origin);
        let at = self
            .segments
            .iter()
            .rposition(|s| s.global_index <= global)
            .unwrap_or(0);
        let seg = self.segments.get(at)?;
        let mut index = seg.sample_start + (global - seg.global_index.min(global));
        if let Some(next) = self.segments.get(at + 1) {
            index = index.min(next.sample_start);
        }
        Some(index.min(self.written))
    }

    /// Every annotation, the moments and the radio's stretches, in the order
    /// SigMF asks for (1.12: sorted by `sample_start`).
    pub fn all_annotations(&self) -> Vec<Annotation> {
        let mut all = self.annotations.clone();
        for &(events, from, to, longest) in &self.radio_windows {
            let (Some(a), Some(b)) = (self.file_index(from), self.file_index(to)) else {
                continue;
            };
            let longest_ms = longest as f64 / self.bytes_per_pair as f64 / self.rate_hz * 1e3;
            all.push(Annotation {
                sample_start: a,
                sample_count: b.saturating_sub(a),
                label: format!("radio dropped {events}"),
                comment: format!(
                    "the radio dropped samples {events} time(s) somewhere in these samples, the longest {longest_ms:.2} ms: \
                     a drop inside the radio leaves no mark in the stream, so the samples either side of it \
                     are joined here and it can be placed only this closely"
                ),
            });
        }
        all.sort_by_key(|a| a.sample_start);
        all
    }

    /// The last word: an annotation at the end of the file saying why.
    pub fn finish(&mut self, stop: &Stop) {
        self.annotations.push(Annotation {
            sample_start: self.written,
            sample_count: 0,
            label: "stopped".to_string(),
            comment: stop.sentence(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 1e6;

    fn at(first_pair: u64, pairs: u64) -> BlockAt {
        BlockAt {
            first_pair,
            pairs,
            driver_dropped: 0,
            centre_hz: 100_000_000,
            rate_hz: RATE,
            // Delivered exactly at its rate, unless a test says otherwise.
            arrived_unix: (first_pair + pairs) as f64 / RATE,
        }
    }

    fn roomy() -> Recording {
        Recording::new(RATE, 2, Limits::DEFAULT)
    }

    /// An unbroken run is one segment, whatever position the stream was at
    /// when the recording started.
    #[test]
    fn an_unbroken_run_is_one_segment_from_global_zero() {
        let mut r = roomy();
        for k in 0..5 {
            assert_eq!(r.block(&at(7_000 + k * 100, 100), "LNA=16"), (100, None));
        }
        assert_eq!(
            r.segments,
            vec![Segment {
                sample_start: 0,
                global_index: 0,
                frequency_hz: 100_000_000,
                arrived_unix: 7_100.0 / RATE,
            }]
        );
        assert!(r.annotations.is_empty());
        assert_eq!((r.written, r.lost()), (500, 0));
    }

    /// The spec's own example, as sdrtop meets it: a hole opens a segment
    /// whose `global_index` runs ahead of its `sample_start` by exactly what
    /// is missing, and an annotation says who lost it.
    #[test]
    fn a_hole_opens_a_segment_and_says_who_lost_what() {
        let mut r = roomy();
        r.block(&at(0, 500), "");
        // The queue refuses one block, and the driver loses 30 pairs before
        // the next.
        assert_eq!(r.refused(&at(500, 470)), None);
        let mut after = at(1_000, 500);
        after.driver_dropped = 30;
        assert_eq!(r.block(&after, ""), (500, None));
        assert_eq!(
            r.segments[1],
            Segment {
                sample_start: 500,
                global_index: 1_000,
                frequency_hz: 100_000_000,
                arrived_unix: 1_500.0 / RATE,
            }
        );
        assert_eq!(
            (r.lost_queue, r.lost_driver, r.lost_unexplained),
            (470, 30, 0)
        );
        let note = &r.annotations[0];
        assert_eq!((note.sample_start, note.label.as_str()), (500, "lost 500"));
        assert!(
            note.comment
                .contains("30 dropped by the driver, 470 refused"),
            "{}",
            note.comment
        );
    }

    /// A gap nobody reported is counted as exactly that, not assigned.
    #[test]
    fn a_gap_nobody_explained_is_counted_as_unexplained() {
        let mut r = roomy();
        r.block(&at(0, 100), "");
        r.block(&at(150, 100), "");
        assert_eq!(r.lost_unexplained, 50);
        assert!(r.annotations[0].comment.contains("50 for no reason"));
    }

    /// A retune is a new segment at the new frequency with no loss, and a
    /// gain change an annotation; neither ends the recording.
    #[test]
    fn a_retune_and_a_gain_change_are_marked_where_they_arrived() {
        let mut r = roomy();
        r.block(&at(0, 100), "LNA=16");
        let mut moved = at(100, 100);
        moved.centre_hz = 101_000_000;
        r.block(&moved, "LNA=16");
        r.block(
            &{
                let mut b = at(200, 100);
                b.centre_hz = 101_000_000;
                b
            },
            "LNA=24",
        );
        assert_eq!(r.segments.len(), 2);
        assert_eq!(
            (r.segments[1].sample_start, r.segments[1].frequency_hz),
            (100, 101_000_000)
        );
        let labels: Vec<_> = r
            .annotations
            .iter()
            .map(|a| (a.sample_start, a.label.as_str()))
            .collect();
        assert_eq!(labels, vec![(100, "tuned 101.000 MHz"), (200, "gain")]);
        assert_eq!(r.lost(), 0);
    }

    /// One recording has one rate, so a new one ends it; a stream that
    /// starts again from zero ends it too.
    #[test]
    fn a_new_rate_or_a_restarted_stream_ends_it() {
        let mut r = roomy();
        r.block(&at(0, 100), "");
        let mut faster = at(100, 100);
        faster.rate_hz = 2e6;
        assert!(matches!(
            r.block(&faster, "").1,
            Some(Stop::RateChanged { .. })
        ));
        assert_eq!(r.block(&at(0, 100), ""), (0, Some(Stop::StreamEnded)));
    }

    /// The limit cuts the block it falls in, to the pair, and names itself.
    #[test]
    fn the_limit_cuts_to_the_pair_and_names_which_limit() {
        let mut r = Recording::new(
            RATE,
            2,
            Limits {
                max_secs: 0.00025,
                max_bytes: 4_000,
            },
        );
        assert_eq!(r.block(&at(0, 200), "").0, 200);
        let (take, stop) = r.block(&at(200, 200), "");
        assert_eq!((take, stop), (50, Some(Stop::Duration { secs: 0.00025 })));
        let mut small = Recording::new(
            RATE,
            2,
            Limits {
                max_secs: 60.0,
                max_bytes: 300,
            },
        );
        assert_eq!(
            small.block(&at(0, 200), ""),
            (150, Some(Stop::Size { bytes: 300 }))
        );
        assert_eq!(Limits::DEFAULT.bytes(20e6, 2), 2_400_000_000);
        assert_eq!(Limits::DEFAULT.bytes(20e6, 4), 4_000_000_000);
    }
    /// **A radio short of its rate, with nothing reported**: the positions
    /// run unbroken, only the clock shows the samples arrived at 69 % of the
    /// rate, and the recording says so. A second segment's time is the clock
    /// when it arrived, not its position over the rate.
    #[test]
    fn a_radio_that_delivers_less_than_its_rate_is_caught_by_the_clock() {
        let mut r = roomy();
        let short = |first_pair: u64| {
            let mut b = at(first_pair, 1_000);
            b.arrived_unix = (first_pair + 1_000) as f64 / (RATE * 0.69);
            b
        };
        for k in 0..3_000 {
            r.block(&short(k * 1_000), "");
        }
        assert_eq!(r.lost(), 0, "nothing reported a drop");
        let delivered = r.delivered_hz().unwrap();
        assert!((delivered / RATE - 0.69).abs() < 1e-9, "{delivered}");
        assert!((r.shortfall().unwrap() - 0.31).abs() < 1e-9);
        assert!(r.shortfall_sentence().unwrap().contains("69.0 %"));

        let mut moved = short(3_000_000);
        moved.centre_hz = 101_000_000;
        r.block(&moved, "");
        assert_eq!(r.segments[1].arrived_unix, 3_001_000.0 / (RATE * 0.69));

        let mut on_time = roomy();
        for k in 0..3_000 {
            on_time.block(&at(k * 1_000, 1_000), "");
        }
        assert_eq!(on_time.shortfall(), None);
    }

    fn counted(events: u32, from_pair: u64, to_pair: u64) -> RadioNote {
        RadioNote::Counted {
            events,
            from_pair,
            to_pair,
            longest_bytes: 2_000,
        }
    }

    /// A drop inside the radio is marked over the stretch of the file its
    /// bracket maps to: through the segments, across a hole to the next
    /// segment's start, and never past what is written. A bracket still in
    /// flight grows as the file does.
    #[test]
    fn the_radios_drops_are_marked_over_the_stretch_they_lie_in() {
        let mut r = roomy();
        r.block(&at(1_000, 100), "");
        r.radio(counted(0, 0, 1_100));
        assert_eq!(r.radio_drops, Some(0), "a reading of none is a reading");
        r.block(&at(1_100, 100), "");
        // A hole of 300, then the rest.
        r.block(&at(1_500, 100), "");
        r.radio(counted(2, 1_150, 1_700));
        let a = r.all_annotations();
        let radio: Vec<_> = a.iter().filter(|a| a.label == "radio dropped 2").collect();
        assert_eq!(radio.len(), 1);
        assert_eq!(
            (radio[0].sample_start, radio[0].sample_count),
            (150, 150),
            "clamped to what is written"
        );
        assert!(radio[0].comment.contains("1.00 ms"), "{}", radio[0].comment);
        r.block(&at(1_600, 200), "");
        let a = r.all_annotations();
        let radio = a.iter().find(|a| a.label == "radio dropped 2").unwrap();
        assert_eq!(
            (radio.sample_start, radio.sample_count),
            (150, 250),
            "the bracket filled in: stream 1700 is file 400, past the hole"
        );
        assert!(
            a.windows(2).all(|w| w[0].sample_start <= w[1].sample_start),
            "sorted"
        );
        assert_eq!(r.radio_drops, Some(2));
    }

    /// A reading from before the recording's first block belongs to none of
    /// it; one that could not be read keeps its reason.
    #[test]
    fn a_drop_before_the_recording_is_not_its_and_an_unreadable_count_says_why() {
        let mut r = roomy();
        r.radio(counted(4, 0, 900));
        r.block(&at(1_000, 100), "");
        r.radio(counted(1, 0, 900));
        assert_eq!(r.radio_drops, Some(0));
        assert!(r.all_annotations().is_empty());
        r.radio(RadioNote::Unreadable("old firmware".into()));
        assert_eq!(r.radio_unreadable.as_deref(), Some("old firmware"));
    }
}
