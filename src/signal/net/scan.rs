// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Blocks of bytes into a plane of per-cell powers, and that plane into a duty
//! cycle for every megahertz of the band that was in view.
//!
//! The measurement itself is [`super::occupancy`], which is pure and knows
//! nothing about transforms or sample formats. What lives here is the part that
//! cannot be: the transform, the tuning it was taken at, and the accumulation
//! across a dwell.
//!
//! **The window is a fixed length of time, not a fixed number of samples.** A
//! duty cycle measured in six-microsecond windows on one radio and in
//! forty-microsecond windows on another is two different quantities wearing one
//! label, and rule 5 is exactly about that. [`WINDOW_S`] is the length; the
//! transform size follows from it and the sample rate.

use std::sync::Arc;

use rustfft::{num_complex::Complex, Fft, FftPlanner};

use crate::hardware::SampleGeometry;
use crate::signal::dsp::{compute_window, WindowFn};
use crate::signal::fft::frame::decode_into;
use crate::state::CellReading;

use super::occupancy::{self, Floor};

/// How long one measurement window is.
///
/// Eight microseconds. Short enough to resolve the shortest thing in the band
/// worth calling a burst - an 802.11 OFDM symbol is four microseconds and a
/// short preamble is eight - and long enough that a cell holds several transform
/// bins at every sample rate this section admits.
pub const WINDOW_S: f64 = 8e-6;

/// Smallest transform the scan will use.
///
/// At the bottom of the admitted sample rates, eight microseconds is fewer
/// samples than a transform can usefully resolve the band into. Sixteen bins is
/// the floor; below it a cell would be a fraction of a bin and the mapping would
/// be a fiction.
const MIN_BINS: usize = 16;

/// What one tuning's worth of scanning needs, built once and reused.
///
/// Rebuilt when the tuning, the rate or the usable span changes, because every
/// one of those changes which cell a bin belongs to. Nothing here is rebuilt per
/// block.
pub struct Scan {
    fft: Arc<dyn Fft<f32>>,
    /// Transform size, and the window applied before it.
    n: usize,
    window: Vec<f32>,
    /// `n * sum(w^2)`, the gain a *band* of power picks up through the window.
    ///
    /// **A window has two gains and this is the one a cell power needs.** A tone
    /// concentrated in a single bin picks up the coherent gain `(sum w)^2`; a
    /// signal spread across bins picks up the power gain `n * sum(w^2)`, and
    /// Parseval says the sum of `|X_k|^2` over a band is the second. A cell
    /// power sums bins, so it is a band power.
    ///
    /// Dividing by the coherent gain instead read every level `10*log10(2/3)`
    /// high - 1.76 dB for a Hann window, whatever the transform size - which put
    /// peaks *above full scale* on a live radio. A number above the top of the
    /// scale is not a loud reading, it is a broken scale.
    window_gain: f64,
    /// Which cell each bin lands in, or `None` for one outside the usable span.
    bin_cells: Vec<Option<usize>>,
    /// The tuning this was built for.
    centre_hz: f64,
    rate_hz: f64,
    span_hz: f64,
    /// Scratch, so a steady state allocates nothing.
    samples: Vec<Complex<f32>>,
    /// The per-window, per-cell power plane for one block.
    plane: Vec<f64>,
    /// Per-cell totals across the dwell.
    windows: Vec<u64>,
    busy: Vec<u64>,
    power_sum: Vec<f64>,
    peak: Vec<f64>,
    /// Running mean of the floors the blocks in this dwell were measured
    /// against, and the shape statistics that decided whether to believe them.
    floor_sum: f64,
    tail_sum: f64,
    spread_sum: f64,
    floors: u64,
    trusted: bool,
}

/// The transform size for a rate: the power of two closest to [`WINDOW_S`].
///
/// A power of two rather than the exact sample count because the transform is
/// run tens of thousands of times a second and a mixed-radix size of 157 costs
/// more than the eight percent of window length it would buy back.
pub fn bins_for(rate_hz: f64) -> usize {
    if !rate_hz.is_finite() || rate_hz <= 0.0 {
        return MIN_BINS;
    }
    let want = rate_hz * WINDOW_S;
    let n = 1usize << (want.max(1.0).log2().round() as u32);
    n.max(MIN_BINS)
}

impl Scan {
    pub fn new(centre_hz: f64, rate_hz: f64, span_hz: f64) -> Self {
        let n = bins_for(rate_hz);
        let window = compute_window(WindowFn::Hann, n);
        let power_gain: f64 = window.iter().map(|w| (*w as f64).powi(2)).sum::<f64>() * n as f64;
        Self {
            fft: FftPlanner::<f32>::new().plan_fft_forward(n),
            n,
            window,
            window_gain: power_gain,
            bin_cells: occupancy::bin_cells(centre_hz, rate_hz, span_hz, n),
            centre_hz,
            rate_hz,
            span_hz,
            samples: vec![Complex::default(); n],
            plane: Vec::new(),
            windows: vec![0; occupancy::CELLS],
            busy: vec![0; occupancy::CELLS],
            power_sum: vec![0.0; occupancy::CELLS],
            peak: vec![0.0; occupancy::CELLS],
            floor_sum: 0.0,
            tail_sum: 0.0,
            spread_sum: 0.0,
            floors: 0,
            trusted: true,
        }
    }

    /// How long this dwell has actually been looking at the band.
    ///
    /// Windows, not wall clock: a dwell interrupted by dropped blocks is a
    /// shorter dwell, not a diluted one.
    pub fn observed_s(&self) -> f64 {
        let widest = self.windows.iter().copied().max().unwrap_or(0);
        widest as f64 * self.n as f64 / self.rate_hz.max(1.0)
    }

    /// Whether this scan was built for the tuning now in force.
    pub fn matches(&self, centre_hz: f64, rate_hz: f64, span_hz: f64) -> bool {
        self.centre_hz == centre_hz && self.rate_hz == rate_hz && self.span_hz == span_hz
    }

    /// Fold one block of interleaved bytes into the dwell.
    ///
    /// The floor is derived per block rather than per dwell, and the counts are
    /// what accumulate. A dwell's worth of raw powers would be megabytes to hold
    /// and to select over; a block's is ten thousand samples, which is enough
    /// for a floor good to a few tenths of a decibel, and deriving it afresh
    /// means a gain change part way through a dwell does not poison the rest of
    /// it.
    pub fn push(&mut self, bytes: &[u8], geometry: SampleGeometry) {
        let pair_bytes = geometry.bytes_per_pair();
        let stride = self.n * pair_bytes;
        let windows = bytes.len() / stride;
        if windows == 0 || self.bin_cells.iter().all(Option::is_none) {
            return;
        }

        // The plane is window-major: one row per window, one column per cell
        // that any bin lands in. Cells nothing lands in are not columns.
        let observed = occupancy::cells_observed(self.centre_hz, self.span_hz);
        let width = observed.len();
        self.plane.clear();
        self.plane.resize(windows * width, 0.0);

        for w in 0..windows {
            let frame = &bytes[w * stride..(w + 1) * stride];
            decode_into(frame, &self.window, geometry, &mut self.samples);
            self.fft.process(&mut self.samples);
            let row = &mut self.plane[w * width..(w + 1) * width];
            for (bin, cell) in self.bin_cells.iter().enumerate() {
                if let Some(cell) = cell {
                    let x = self.samples[bin];
                    row[cell - observed.start] +=
                        (x.re as f64 * x.re as f64 + x.im as f64 * x.im as f64) / self.window_gain;
                }
            }
        }

        let mut scratch = self.plane.clone();
        let Some(floor) = occupancy::derive_floor(&mut scratch) else {
            return;
        };
        self.record(&floor, &observed, windows, width);
    }

    fn record(
        &mut self,
        floor: &Floor,
        observed: &std::ops::Range<usize>,
        windows: usize,
        width: usize,
    ) {
        self.floor_sum += floor.power;
        self.tail_sum += floor.tail;
        self.spread_sum += floor.spread;
        self.floors += 1;
        // One untrustworthy block makes the dwell untrustworthy. The alternative
        // is averaging a verdict, and half a saturated dwell is not half a
        // measurement.
        self.trusted &= floor.trusted;

        for w in 0..windows {
            let row = &self.plane[w * width..(w + 1) * width];
            for (i, power) in row.iter().enumerate() {
                let cell = observed.start + i;
                self.windows[cell] += 1;
                self.busy[cell] += u64::from(*power > floor.threshold);
                self.power_sum[cell] += *power;
                self.peak[cell] = self.peak[cell].max(*power);
            }
        }
    }

    /// The dwell so far, as the state carries it, and start again.
    pub fn take(&mut self) -> crate::state::BandOccupancy {
        let cells = (0..occupancy::CELLS)
            .map(|c| CellReading {
                windows: self.windows[c],
                duty: occupancy::duty_cycle(self.busy[c], self.windows[c]),
                mean_dbfs: dbfs(if self.windows[c] > 0 {
                    self.power_sum[c] / self.windows[c] as f64
                } else {
                    0.0
                }),
                peak_dbfs: dbfs(self.peak[c]),
                // A dwell knows nothing about how often it happens. Coverage
                // and the time of measurement are `BandOccupancy::absorb`'s to
                // fill in, because both are about the sequence of dwells rather
                // than about this one.
                coverage: None,
                measured: None,
                observed_s: 0.0,
            })
            .collect();
        let n = self.floors.max(1) as f64;
        let out = crate::state::BandOccupancy {
            cells,
            noise_dbfs: (self.floors > 0).then(|| dbfs(self.floor_sum / n)),
            trusted: self.floors > 0 && self.trusted,
            tail: self.tail_sum / n,
            spread: self.spread_sum / n,
            window_s: self.n as f64 / self.rate_hz.max(1.0),
            // A dwell has no watch of its own: how often the radio comes back
            // here is a fact about the sequence of dwells, and `absorb` owns it.
            watch_start: None,
            // A dwell has no past: the history is the band's, and `absorb`
            // owns it.
            history: Default::default(),
            last_column: None,
            columns_taken: 0,
        };
        self.reset();
        out
    }

    fn reset(&mut self) {
        self.windows.fill(0);
        self.busy.fill(0);
        self.power_sum.fill(0.0);
        self.peak.fill(0.0);
        self.floor_sum = 0.0;
        self.tail_sum = 0.0;
        self.spread_sum = 0.0;
        self.floors = 0;
        self.trusted = true;
    }
}

/// Linear power to dBFS, with a floor rather than a negative infinity.
///
/// The floor is the same one the spectrum uses, so a cell nothing was heard in
/// reads the same "nothing" everywhere in the app. Rule 5.
fn dbfs(power: f64) -> f64 {
    if power > 0.0 {
        (10.0 * power.log10()).max(crate::signal::fft::DB_FLOOR as f64)
    } else {
        crate::signal::fft::DB_FLOOR as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::SampleFormat;
    use crate::signal::dsp::testkit::Rng;
    use std::f64::consts::TAU;

    const RATE: f64 = 20_000_000.0;
    const CENTRE: f64 = 2_437_000_000.0;
    const SPAN: f64 = 18_000_000.0;

    fn eight_bit() -> SampleGeometry {
        SampleGeometry {
            format: SampleFormat::Int8,
            full_scale: 128.0,
        }
    }

    /// Interleaved 8-bit bytes for a complex signal built by `f`.
    fn bytes(pairs: usize, mut f: impl FnMut(usize) -> (f64, f64)) -> Vec<u8> {
        let mut out = Vec::with_capacity(pairs * 2);
        for i in 0..pairs {
            let (re, im) = f(i);
            out.push((re * 127.0).round().clamp(-127.0, 127.0) as i8 as u8);
            out.push((im * 127.0).round().clamp(-127.0, 127.0) as i8 as u8);
        }
        out
    }

    fn run(bytes: &[u8]) -> crate::state::BandOccupancy {
        let mut scan = Scan::new(CENTRE, RATE, SPAN);
        scan.push(bytes, eight_bit());
        scan.take()
    }

    /// **A full-scale signal reads 0 dBFS, and nothing ever reads above it.**
    ///
    /// The whole point of a full-scale reference is that it is the top. A number
    /// above it is not a loud reading, it is a broken scale - and this panel
    /// printed `3.5 peak dBFS` off a live radio, which is what sent me looking.
    ///
    /// The cause is that a window has two gains and only one of them was used. A
    /// tone concentrated in one bin picks up the *coherent* gain `(sum w)^2`; a
    /// signal spread over many bins picks up the *power* gain `n * sum w^2`. The
    /// cell power here is a band power - it sums bins - so it is the second, and
    /// dividing it by the first read every level 1.76 dB high.
    #[test]
    fn a_full_scale_signal_reads_zero_dbfs() {
        // A tone 3.5 MHz above centre is 2440.5 MHz, the middle of cell 40.
        // The middle matters: 3.0 MHz lands on the 2440 boundary and Hann puts
        // the tone's skirts either side of it, so half the power is measured in
        // cell 39 and the reading is low for a reason that is not a bug.
        let tone = bytes(128 * 64, |i| {
            let ph = TAU * 3_500_000.0 * i as f64 / RATE;
            (ph.cos(), ph.sin())
        });
        let band = run(&tone);
        let cell = 40;
        assert!(band.cells[cell].observed());
        let peak = band.cells[cell].peak_dbfs;
        assert!(
            peak.abs() < 0.3,
            "a full-scale tone should read 0 dBFS, got {peak:.2}"
        );
        // And nowhere in the band does anything read above full scale.
        for (i, c) in band.cells.iter().enumerate() {
            assert!(
                c.peak_dbfs <= 0.3,
                "cell {i} reads {:.2} dBFS, above full scale",
                c.peak_dbfs
            );
        }
    }

    /// The same scale for a signal that is not a tone, which is the case the
    /// coherent gain gets wrong.
    ///
    /// Noise filling the span puts its power *in total* across the cells, not in
    /// each: the power is shared out, and a band power that did not add up would
    /// be the same bug wearing a different hat.
    ///
    /// Twenty decibels below full scale rather than at it, because a Gaussian at
    /// unit total power spends much of its time outside the eight-bit rails and
    /// the clipping, not the window, would then be what the test measured.
    #[test]
    fn a_noise_floor_adds_up_across_the_cells() {
        let mut rng = Rng::new(4);
        let noise = rng.noise(128 * 64, 0.01);
        let raw = bytes(128 * 64, |i| (noise[i].re as f64, noise[i].im as f64));
        let band = run(&raw);

        let total: f64 = band
            .cells
            .iter()
            .filter(|c| c.observed())
            .map(|c| 10f64.powf(c.mean_dbfs / 10.0))
            .sum();
        // The observed span is 18 of the 20 MHz sampled, so nine tenths of the
        // power is inside it: -20 dBFS, less 10*log10(20/18).
        let db = 10.0 * total.log10();
        let want = -20.0 + 10.0 * (18.0f64 / 20.0).log10();
        assert!(
            (db - want).abs() < 0.5,
            "18 MHz of a -20 dBFS noise floor should read {want:.2}, got {db:.2}"
        );
    }

    /// Half amplitude is six decibels down, whatever the transform size.
    #[test]
    fn the_scale_is_the_same_at_every_transform_size() {
        for amp in [1.0f64, 0.5, 0.25] {
            let tone = bytes(128 * 64, |i| {
                let ph = TAU * 3_500_000.0 * i as f64 / RATE;
                (amp * ph.cos(), amp * ph.sin())
            });
            let peak = run(&tone).cells[40].peak_dbfs;
            let want = 20.0 * amp.log10();
            assert!(
                (peak - want).abs() < 0.4,
                "amplitude {amp} should read {want:.1} dBFS, got {peak:.2}"
            );
        }
    }
}
