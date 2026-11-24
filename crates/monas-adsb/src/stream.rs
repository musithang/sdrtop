// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use crate::{Df17Error, Df17Frame, PpmError, PpmSlicer, PreambleDetector, PreambleMatch};

const COPY_CHUNK: usize = 4096;
const ADJACENT_CELLS: u64 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameOutcome {
    Accepted(Df17Frame),
    CrcRejected { remainder: u32 },
    UnsupportedFormat { format: u8 },
    Incomplete,
    Ambiguous,
}

/// One outcome after comparing the timings for a preamble neighborhood.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StreamEvent {
    /// Absolute sample pair at the start of the selected sample cell.
    pub first_pair: u64,
    /// The selected timing, with its fractional edge relative to first_pair.
    pub preamble: PreambleMatch,
    pub outcome: FrameOutcome,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReceiverStats {
    pub accepted: u64,
    pub crc_rejected: u64,
    /// Candidate neighborhoods whose best available header was outside DF17.
    /// This is a detection count with tentative header bits.
    pub unsupported_format: u64,
    pub incomplete: u64,
    pub ambiguous: u64,
    /// Breaks caused by a change in the supplied sample positions.
    pub discontinuities: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiverError {
    IndexOverflow,
    InvalidPower { pair: u64 },
}

/// Streaming 1090ES receiver over the power envelope of normalized IQ.
///
/// One instance uses one sample rate. Call [Self::finish] before reusing
/// an instance across a discontinuity whose sample positions are contiguous.
pub struct Receiver {
    detector: PreambleDetector,
    slicer: PpmSlicer,
    group_cells: u64,
    power: Vec<f32>,
    buffer_first: u64,
    next_pair: Option<u64>,
    cursor: usize,
    group: Option<CandidateGroup>,
    stats: ReceiverStats,
}

impl Receiver {
    pub fn new(sample_rate_hz: f64) -> Result<Self, PpmError> {
        Ok(Self {
            detector: PreambleDetector::new(sample_rate_hz)?,
            slicer: PpmSlicer::new(sample_rate_hz)?,
            // Bound a neighborhood to roughly one symbol. A busy stretch
            // of candidate cells must not hold an event indefinitely.
            group_cells: ((sample_rate_hz / 1_000_000.0).ceil() as u64).saturating_add(2),
            power: Vec::new(),
            buffer_first: 0,
            next_pair: None,
            cursor: 0,
            group: None,
            stats: ReceiverStats::default(),
        })
    }

    pub fn stats(&self) -> ReceiverStats {
        self.stats
    }

    /// Consume power samples with their absolute first IQ pair number.
    ///
    /// Empty inputs leave the stream untouched. Invalid input leaves the
    /// receiver unchanged. A noncontiguous block completes the previous
    /// segment first and starts a new one without sharing any samples.
    pub fn push_power(
        &mut self,
        first_pair: u64,
        power: &[f32],
        mut emit: impl FnMut(StreamEvent),
    ) -> Result<(), ReceiverError> {
        if power.is_empty() {
            return Ok(());
        }
        let next_pair = first_pair
            .checked_add(power.len() as u64)
            .ok_or(ReceiverError::IndexOverflow)?;
        if let Some(index) = power.iter().position(|p| !p.is_finite() || *p < 0.0) {
            return Err(ReceiverError::InvalidPower {
                pair: first_pair + index as u64,
            });
        }
        if self
            .next_pair
            .is_some_and(|expected| expected != first_pair)
        {
            self.finish(&mut emit);
            self.stats.discontinuities += 1;
        }
        if self.next_pair.is_none() {
            self.buffer_first = first_pair;
        }
        self.next_pair = Some(next_pair);
        for block in power.chunks(COPY_CHUNK) {
            self.power.extend_from_slice(block);
            self.scan(false, &mut emit);
            if self.cursor >= COPY_CHUNK {
                self.power.copy_within(self.cursor.., 0);
                self.power.truncate(self.power.len() - self.cursor);
                self.buffer_first += self.cursor as u64;
                self.cursor = 0;
            }
        }
        Ok(())
    }

    /// Finish the segment, including frames that end exactly at EOF.
    /// The next input begins a fresh segment. Counters are cumulative.
    pub fn finish(&mut self, mut emit: impl FnMut(StreamEvent)) {
        self.scan(true, &mut emit);
        self.publish_group(&mut emit);
        self.power.clear();
        self.cursor = 0;
        self.next_pair = None;
    }

    fn scan(&mut self, end_of_segment: bool, emit: &mut impl FnMut(StreamEvent)) {
        let lookahead = if end_of_segment {
            self.detector.lookahead()
        } else {
            self.slicer.frame_lookahead()
        };
        while self.power.len() - self.cursor >= lookahead {
            let first_pair = self.buffer_first + self.cursor as u64;
            if self.group.as_ref().is_some_and(|group| {
                first_pair - group.last_cell > ADJACENT_CELLS
                    || first_pair - group.first_cell >= self.group_cells
            }) {
                self.publish_group(emit);
            }
            let power = &self.power[self.cursor..];
            let matches = self
                .detector
                .candidates_at(power, 0)
                .expect("scan lookahead and input power were checked");
            for &preamble in matches.as_slice() {
                let outcome = decode(&self.slicer, power, preamble.start_sample);
                let event = StreamEvent {
                    first_pair,
                    preamble,
                    outcome,
                };
                match &mut self.group {
                    Some(group) => group.consider(event),
                    None => self.group = Some(CandidateGroup::new(event)),
                }
            }
            self.cursor += 1;
        }
    }

    fn publish_group(&mut self, emit: &mut impl FnMut(StreamEvent)) {
        let Some(group) = self.group.take() else {
            return;
        };
        let event = group.event();
        match event.outcome {
            FrameOutcome::Accepted(_) => self.stats.accepted += 1,
            FrameOutcome::CrcRejected { .. } => self.stats.crc_rejected += 1,
            FrameOutcome::UnsupportedFormat { .. } => self.stats.unsupported_format += 1,
            FrameOutcome::Incomplete => self.stats.incomplete += 1,
            FrameOutcome::Ambiguous => self.stats.ambiguous += 1,
        }
        emit(event);
    }
}

fn decode(slicer: &PpmSlicer, power: &[f32], start: f64) -> FrameOutcome {
    match slicer.downlink_format(power, start) {
        Ok(17) => {}
        Ok(format) => return FrameOutcome::UnsupportedFormat { format },
        Err(_) => return FrameOutcome::Incomplete,
    }
    let Ok(bytes) = slicer.slice_frame(power, start, None) else {
        return FrameOutcome::Incomplete;
    };
    match Df17Frame::try_from(bytes.as_slice()) {
        Ok(frame) => FrameOutcome::Accepted(frame),
        Err(Df17Error::Crc { remainder }) => FrameOutcome::CrcRejected { remainder },
        Err(_) => unreachable!("fixed-size DF17 from the same bit decisions"),
    }
}

struct CandidateGroup {
    first_cell: u64,
    last_cell: u64,
    best: StreamEvent,
    conflicting_frames: bool,
}

impl CandidateGroup {
    fn new(event: StreamEvent) -> Self {
        Self {
            first_cell: event.first_pair,
            last_cell: event.first_pair,
            best: event,
            conflicting_frames: false,
        }
    }

    fn consider(&mut self, event: StreamEvent) {
        self.last_cell = event.first_pair;
        if let (FrameOutcome::Accepted(old), FrameOutcome::Accepted(new)) =
            (self.best.outcome, event.outcome)
        {
            self.conflicting_frames |= old != new;
        }
        let rank = priority(event.outcome).cmp(&priority(self.best.outcome));
        if rank.is_gt() || (rank.is_eq() && event.preamble.contrast > self.best.preamble.contrast) {
            self.best = event;
        }
    }

    fn event(self) -> StreamEvent {
        if self.conflicting_frames {
            StreamEvent {
                outcome: FrameOutcome::Ambiguous,
                ..self.best
            }
        } else {
            self.best
        }
    }
}

fn priority(outcome: FrameOutcome) -> u8 {
    match outcome {
        FrameOutcome::Accepted(_) => 3,
        FrameOutcome::CrcRejected { .. } => 2,
        FrameOutcome::Incomplete => 1,
        FrameOutcome::UnsupportedFormat { .. } => 0,
        FrameOutcome::Ambiguous => unreachable!("assigned only when publishing a group"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ppm::tests::{sampled_power, ARCHIVED_FRAME, PUBLISHED_FRAME};

    const RATES: [f64; 4] = [2_400_000.0, 2_500_000.0, 6_000_000.0, 10_000_000.0];
    const FIRST_PAIR: u64 = (1 << 53) + 37;

    fn run(rate: f64, blocks: &[&[f32]]) -> (Vec<StreamEvent>, ReceiverStats) {
        let mut receiver = Receiver::new(rate).unwrap();
        let mut events = Vec::new();
        let mut first_pair = FIRST_PAIR;
        for block in blocks {
            receiver
                .push_power(first_pair, block, |event| events.push(event))
                .unwrap();
            first_pair += block.len() as u64;
        }
        receiver.finish(|event| events.push(event));
        (events, receiver.stats())
    }

    fn accepted(events: &[StreamEvent]) -> Vec<(&StreamEvent, Df17Frame)> {
        events
            .iter()
            .filter_map(|event| match event.outcome {
                FrameOutcome::Accepted(frame) => Some((event, frame)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn discovers_and_emits_each_clean_burst_once_end_to_end() {
        for rate in RATES {
            for reference in [ARCHIVED_FRAME, PUBLISHED_FRAME] {
                for phase in 0..64 {
                    let start = 30.0 + f64::from(phase) / 64.0;
                    for subsamples in [1, 16] {
                        let power = sampled_power(&reference, rate, start, subsamples);
                        let (events, stats) = run(rate, &[&power]);
                        let frames = accepted(&events);
                        assert_eq!(frames.len(), 1, "{rate}, phase {phase}, {subsamples}x");
                        let (event, frame) = frames[0];
                        assert_eq!(frame.bytes(), &reference);
                        let edge =
                            (event.first_pair - FIRST_PAIR) as f64 + event.preamble.start_sample;
                        assert!((edge - start).abs() <= 1.0, "{edge} vs {start}");
                        assert_eq!(stats.accepted, 1);
                    }
                }
            }
        }
    }

    #[test]
    fn every_split_and_archive_block_sizes_preserve_events_and_counts() {
        for rate in RATES {
            let power = sampled_power(&ARCHIVED_FRAME, rate, 7.375, 1);
            let expected = run(rate, &[&power]);
            assert_eq!(expected.1.accepted, 1);
            for split in 0..=power.len() {
                let (left, right) = power.split_at(split);
                assert_eq!(run(rate, &[left, &[], right]), expected, "split {split}");
            }
            for size in [1, 137, 511] {
                let blocks: Vec<_> = power.chunks(size).collect();
                assert_eq!(run(rate, &blocks), expected);
            }
        }
    }

    #[test]
    fn retains_distinct_repetitions_through_buffer_compaction() {
        let rate = 10_000_000.0;
        let mut power = vec![0.0; COPY_CHUNK * 2 - 51];
        let waveform = sampled_power(&ARCHIVED_FRAME, rate, 0.0, 1);
        power.extend_from_slice(&waveform);
        power.extend_from_slice(&waveform);
        let expected = run(rate, &[&power]);
        let frames = accepted(&expected.0);
        assert_eq!(frames.len(), 2);
        assert_eq!(
            frames[1].0.first_pair - frames[0].0.first_pair,
            waveform.len() as u64
        );
        assert_eq!(frames[0].1, frames[1].1);
        let small: Vec<_> = power.chunks(137).collect();
        assert_eq!(run(rate, &small), expected);
    }

    #[test]
    fn a_gap_cannot_join_preamble_and_payload_from_separate_segments() {
        let rate = 10_000_000.0;
        let power = sampled_power(&ARCHIVED_FRAME, rate, 0.0, 1);
        for split in [4, 20, 78, 80, 230, power.len() - 1] {
            let mut receiver = Receiver::new(rate).unwrap();
            let mut events = Vec::new();
            receiver
                .push_power(FIRST_PAIR, &power[..split], |event| events.push(event))
                .unwrap();
            receiver
                .push_power(FIRST_PAIR + split as u64 + 1, &power[split..], |event| {
                    events.push(event)
                })
                .unwrap();
            let whole_frame_start = FIRST_PAIR + power.len() as u64 + 1;
            receiver
                .push_power(whole_frame_start, &power, |event| events.push(event))
                .unwrap();
            receiver.finish(|event| events.push(event));
            let frames = accepted(&events);
            assert_eq!(frames.len(), 1, "split {split}");
            assert_eq!(frames[0].0.first_pair, whole_frame_start);
            assert_eq!(receiver.stats().discontinuities, 1);
        }
    }

    #[test]
    fn eof_distinguishes_complete_cut_corrupt_and_other_format_candidates() {
        let rate = 10_000_000.0;
        let power = sampled_power(&ARCHIVED_FRAME, rate, 0.0, 1);
        let (events, _) = run(rate, &[&power]);
        assert_eq!(accepted(&events).len(), 1);
        let (cut, _) = run(rate, &[&power[..power.len() - 1]]);
        assert_eq!(cut[0].outcome, FrameOutcome::Incomplete);
        assert!(accepted(&cut).is_empty());

        let mut corrupt = ARCHIVED_FRAME;
        corrupt[13] ^= 1;
        let corrupt_power = sampled_power(&corrupt, rate, 0.0, 1);
        let (events, _) = run(rate, &[&corrupt_power]);
        assert_eq!(
            events[0].outcome,
            FrameOutcome::CrcRejected { remainder: 1 }
        );

        let mut short = ARCHIVED_FRAME;
        short[0] = 0x20;
        let short_power = sampled_power(&short, rate, 0.0, 1);
        let (events, _) = run(rate, &[&short_power[..640]]);
        assert_eq!(
            events[0].outcome,
            FrameOutcome::UnsupportedFormat { format: 4 }
        );
    }

    #[test]
    fn invalid_blocks_are_rejected_before_mutating_the_receiver() {
        let rate = 10_000_000.0;
        let mut receiver = Receiver::new(rate).unwrap();
        assert_eq!(
            receiver.push_power(FIRST_PAIR, &[0.0, f32::NAN], |_| panic!("unexpected event")),
            Err(ReceiverError::InvalidPower {
                pair: FIRST_PAIR + 1
            })
        );
        assert_eq!(
            receiver.push_power(u64::MAX, &[0.0], |_| panic!("unexpected event")),
            Err(ReceiverError::IndexOverflow)
        );
        let power = sampled_power(&ARCHIVED_FRAME, rate, 0.0, 1);
        let mut events = Vec::new();
        receiver
            .push_power(FIRST_PAIR, &power, |event| events.push(event))
            .unwrap();
        receiver.finish(|event| events.push(event));
        assert_eq!((events, receiver.stats()), run(rate, &[&power]));
    }

    #[test]
    fn emits_during_stream_and_finish_does_not_republish() {
        let rate = 2_500_000.0;
        let mut power = sampled_power(&ARCHIVED_FRAME, rate, 7.25, 16);
        power.extend([0.0; 30]);
        let mut receiver = Receiver::new(rate).unwrap();
        let mut live = Vec::new();
        receiver
            .push_power(FIRST_PAIR, &power, |event| live.push(event))
            .unwrap();
        assert_eq!(accepted(&live).len(), 1);
        receiver.finish(|event| live.push(event));
        let finished = live.len();
        receiver.finish(|event| live.push(event));
        assert_eq!(live.len(), finished);
        assert_eq!(receiver.stats().accepted, 1);
    }

    #[test]
    fn conflicting_valid_payloads_in_a_group_are_ambiguous() {
        let first = Df17Frame::try_from(ARCHIVED_FRAME.as_slice()).unwrap();
        let second = Df17Frame::try_from(PUBLISHED_FRAME.as_slice()).unwrap();
        let event = StreamEvent {
            first_pair: FIRST_PAIR,
            preamble: PreambleMatch::default(),
            outcome: FrameOutcome::Accepted(first),
        };
        let mut group = CandidateGroup::new(event);
        group.consider(event);
        assert!(!group.conflicting_frames);
        group.consider(StreamEvent {
            outcome: FrameOutcome::Accepted(second),
            ..event
        });
        assert_eq!(group.event().outcome, FrameOutcome::Ambiguous);
    }
}
