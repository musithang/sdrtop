// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The band measurement: [`Scan`] over the survey's view, published a dwell
//! at a time. The survey is the one view that shows it; elsewhere its cost
//! would buy nothing on screen, and its time axis is kept moving with
//! "nobody looked" instead.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::signal::net::scan::Scan;
use crate::state::SdrMetrics;

use super::Tuning;

/// How much *observation* a dwell is, before it is published and started again.
///
/// **Not a wall-clock interval, which is what this was first written as.** The
/// feed is lossy and the section can be closed and reopened, so wall time and
/// time spent looking at the band are different quantities, and a duty cycle is
/// a fraction of the second one. Counting windows means a dwell interrupted by
/// dropped blocks is a shorter dwell rather than a diluted one - and it means
/// the measurement can be tested without a clock, which is how the worker's
/// end-to-end test of it exists at all.
///
/// Fifty milliseconds is about eight thousand windows. The duty cycle that
/// supports is good to a quarter of a percent, which is finer than the whole
/// percent it is shown to and not by much - see
/// [`crate::signal::net::occupancy::DUTY_RESOLUTION`], where the two were made
/// to agree. It publishes at most twenty times a second against a screen that
/// redraws thirty.
const DWELL_S: f64 = 0.05;

/// The scan under way, if the survey is in view.
#[derive(Default)]
pub(super) struct Band {
    scan: Option<Scan>,
}

impl Band {
    /// This block's samples, `block`, into the scan, and a finished dwell
    /// into the state.
    pub(super) fn measure(
        &mut self,
        block: Option<&[num_complex::Complex<f32>]>,
        tuning: Tuning,
        now: Instant,
        state: &Arc<Mutex<SdrMetrics>>,
    ) {
        // Retuning invalidates every cell mapping, so the scan is rebuilt and
        // whatever it had accumulated goes with it: half a dwell at one
        // frequency and half at another is a measurement of neither.
        if !self
            .scan
            .as_ref()
            .is_some_and(|s| s.matches(tuning.centre_hz, tuning.rate_hz, tuning.span_hz))
        {
            self.scan = Some(Scan::new(tuning.centre_hz, tuning.rate_hz, tuning.span_hz));
        }
        if let (Some(scan), Some(block)) = (self.scan.as_mut(), block) {
            scan.push_iq(block);
            if scan.observed_s() >= DWELL_S {
                let band = scan.take();
                let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                m.net.band.absorb(band, now);
            }
        }
    }

    /// Not on the survey: no scan, and while the section is `open` the
    /// band's time axis moves on with "nobody looked".
    pub(super) fn unobserved(&mut self, open: bool, now: Instant, state: &Arc<Mutex<SdrMetrics>>) {
        self.scan = None;
        if open {
            let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
            m.net.band.mark_unobserved(now);
        }
    }

    /// The section closed, and the band measurement stops with it. Nothing
    /// has been observed since, and a panel reopened an hour later showing
    /// the last dwell as if it were current is exactly what rule 4 exists to
    /// prevent; the chrome's staleness marks it, and dropping the scan means
    /// the next dwell starts clean.
    pub(super) fn close(&mut self) {
        self.scan = None;
    }
}
