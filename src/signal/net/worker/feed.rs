// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! What the stream brought: whether a block follows the last one, the
//! samples held for the measurement path, and what handling it cost.

use crate::hardware::SampleGeometry;

/// `bytes` as samples, decoded into `slot` the first time a receiver asks
/// and handed out as they are after that.
pub(super) fn decoded_block<'a>(
    slot: &'a mut Option<Vec<num_complex::Complex<f32>>>,
    bytes: &[u8],
    geometry: SampleGeometry,
) -> &'a [num_complex::Complex<f32>] {
    slot.get_or_insert_with(|| {
        let mut out = Vec::new();
        crate::signal::demod::decode(bytes, geometry, usize::MAX, &mut out);
        out
    })
}

/// The blocks the measurement path may cut a burst from: those held from
/// before, and this one.
pub(super) fn held<'a>(
    recent: &'a std::collections::VecDeque<(u64, Vec<num_complex::Complex<f32>>)>,
    current: Option<(u64, &'a [num_complex::Complex<f32>])>,
) -> crate::signal::net::measure::Recent<'a> {
    crate::signal::net::measure::Recent::new(
        recent
            .iter()
            .map(|(p, v)| (*p, v.as_slice()))
            .chain(current),
    )
}

/// What the worker carries from one block to the next.
///
/// Three fields, and two of them exist only so that a gap can be told from a
/// pause. See [`Run::suspend`].
#[derive(Default)]
pub(super) struct Run {
    pub(super) last_seq: u64,
    /// The sequence of the previous block *that reached us*, or `None` when the
    /// run has not started.
    pub(super) drop_ref: Option<u64>,
    /// Unbroken blocks since the last gap.
    pub(super) blocks: u64,
}

impl Run {
    /// The section was closed, so the feed stopped forwarding.
    ///
    /// The device goes on counting every callback, so the next block to arrive
    /// will be thousands of sequence numbers away without a single one having
    /// been lost. Clearing `drop_ref` is what stops that jump being reported as
    /// the worst loss event of the session. The run length goes with it: there
    /// is no run any more.
    pub(super) fn suspend(&mut self) {
        self.drop_ref = None;
        self.blocks = 0;
    }
}

/// How long one decode-load reading averages over: half a second of stream,
/// or half a second of work, whichever comes first.
///
/// Half a second: at the block sizes the native radios deliver that is dozens
/// of blocks, enough that one slow block (a page fault, a scheduler hiccup)
/// does not read as a worker in trouble, and short enough that the figure
/// follows the user opening a heavier preset within a glance.
///
/// **Either clock closes the window, and the second one is not optional.** A
/// window counted in stream time alone takes longer to fill the slower the
/// worker is, because a worker that cannot keep up only ever sees the blocks
/// the bounded feed had room for. The first live run showed exactly that: a
/// worker in trouble whose load never appeared at all, twenty seconds in. So
/// the figure that matters most arrived last, or not at all. Closing the
/// window on work time too means an overloaded worker reports within half a
/// second, at whatever it really is.
const LOAD_WINDOW_S: f64 = 0.5;

/// The decode-load accumulator: wall time spent against stream time covered.
///
/// Pure, with the clock read by the caller, so the arithmetic is testable
/// without a radio or a real stopwatch.
#[derive(Default)]
pub(super) struct Load {
    busy: std::time::Duration,
    stream_s: f64,
}

impl Load {
    /// Add one block: `spent` handling it, `pairs` I/Q pairs of it at
    /// `rate_hz`. Returns a reading once a whole window has been covered, and
    /// starts the next one.
    pub(super) fn add(
        &mut self,
        spent: std::time::Duration,
        pairs: u64,
        rate_hz: f64,
    ) -> Option<f64> {
        // A rate that is not a positive number covers no stream time, and
        // dividing by it would invent a load.
        if rate_hz.is_nan() || rate_hz <= 0.0 {
            return None;
        }
        self.busy += spent;
        self.stream_s += pairs as f64 / rate_hz;
        if self.stream_s < LOAD_WINDOW_S && self.busy.as_secs_f64() < LOAD_WINDOW_S {
            return None;
        }
        let load = self.busy.as_secs_f64() / self.stream_s;
        *self = Load::default();
        Some(load)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wall time over stream time, reported once a window is covered.
    #[test]
    fn the_load_is_wall_time_over_the_stream_time_it_covered() {
        use std::time::Duration;
        let mut load = Load::default();
        // 1 MHz, 100 000 pairs a block: 0.1 s of stream each, 25 ms to handle.
        for _ in 0..4 {
            assert_eq!(
                load.add(Duration::from_millis(25), 100_000, 1e6),
                None,
                "under half a second of stream, no reading yet"
            );
        }
        let reading = load
            .add(Duration::from_millis(25), 100_000, 1e6)
            .expect("five blocks cover the window");
        assert!((reading - 0.25).abs() < 1e-9, "{reading}");
        // And the next window starts from nothing.
        assert_eq!(load.add(Duration::from_millis(25), 100_000, 1e6), None);
    }

    /// Above one means the worker cannot keep up with the stream, and the
    /// figure must be free to say so rather than be clamped.
    #[test]
    fn a_load_above_one_is_reported_not_clamped() {
        use std::time::Duration;
        let mut load = Load::default();
        let reading = load
            .add(Duration::from_millis(900), 600_000, 1e6)
            .expect("0.6 s of stream covers the window");
        assert!((reading - 1.5).abs() < 1e-9, "{reading}");
    }

    /// A worker too slow to cover a window of stream still reports within
    /// half a second of work, at the figure it really is - the case a
    /// stream-time window alone would hide longest.
    #[test]
    fn an_overloaded_worker_reports_on_work_time_alone() {
        use std::time::Duration;
        let mut load = Load::default();
        // 10 000 pairs at 1 Msps is 10 ms of stream; each takes 200 ms.
        assert_eq!(load.add(Duration::from_millis(200), 10_000, 1e6), None);
        assert_eq!(load.add(Duration::from_millis(200), 10_000, 1e6), None);
        let reading = load
            .add(Duration::from_millis(200), 10_000, 1e6)
            .expect("0.6 s of work closes the window");
        assert!((reading - 20.0).abs() < 1e-9, "{reading}");
    }

    /// A rate that is not a positive number covers no stream time: no
    /// reading, rather than a division that invents one.
    #[test]
    fn no_rate_means_no_load() {
        use std::time::Duration;
        let mut load = Load::default();
        for rate in [0.0, -1.0, f64::NAN] {
            assert_eq!(load.add(Duration::from_secs(1), 1_000_000, rate), None);
        }
    }
}
