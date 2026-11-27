// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The floor lab's worker: blocks off its feed into frames, frames into the
//! two sums, and after every frame the products into the state.
//!
//! **An exposure describes one floor.** Anything that moves the floor starts
//! it again, counted, with its reason: a retune, a new rate or bin width, a
//! gain or the amplifier changed, the `floor_reset` command, a gap longer
//! than two frames, or a stream that started over. A shorter gap costs only
//! the frame it cut short. The gains are read in the same lock as the
//! setting, at every block, as the doppler worker reads its carriers.
//!
//! **The sums stay here.** Two f64 per bin (51 MB at the limit) never enter
//! the state; what is published is bounded (see [`crate::state::floor`]).
//! Everything is computed outside the lock and goes in under one.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use crossbeam_channel::Receiver;
use rustfft::num_complex::Complex;

use super::chance::{depth_db, local_floor};
use super::grids::{find_grids, Grid};
use super::lines::find_lines;
use super::settings::{derive, grouped, Derived, FloorSetting, ZoomSpan};
use super::spread::{kinds, spread, spread_sigma_chance, steadiness, steady_spread, BinKind};
use super::{Frames, Sums};
use crate::hardware::{SampleGeometry, StreamBlock};
use crate::state::floor::{Cell, FloorLine, Zoom, MAX_CELLS, MAX_ZOOM_BINS, NO_ZOOM};
use crate::state::SdrMetrics;

pub struct FloorWorker {
    rx: Receiver<StreamBlock>,
    state: Arc<Mutex<SdrMetrics>>,
    geometry: SampleGeometry,
}

impl FloorWorker {
    pub fn new(
        rx: Receiver<StreamBlock>,
        state: Arc<Mutex<SdrMetrics>>,
        geometry: SampleGeometry,
    ) -> Self {
        Self {
            rx,
            state,
            geometry,
        }
    }

    pub fn run(self) {
        let mut session = Session::new(self.geometry);
        while let Ok(block) = self.rx.recv() {
            session.block(&block, &self.state);
        }
    }
}

/// What, changed, starts an exposure again.
#[derive(Clone, PartialEq)]
struct Key {
    centre: u64,
    rate: f64,
    bin_hz: f64,
    gains: Vec<f64>,
    amp: bool,
    generation: u64,
}

impl Key {
    /// Why an exposure under `self` cannot go on under `new`.
    fn changed(&self, new: &Key) -> Option<&'static str> {
        if self.centre != new.centre {
            Some("retuned")
        } else if self.rate != new.rate {
            Some("the sample rate changed")
        } else if self.bin_hz != new.bin_hz {
            Some("the bin width changed")
        } else if self.gains != new.gains {
            Some("a gain changed")
        } else if self.amp != new.amp {
            Some("the amplifier was switched")
        } else if self.generation != new.generation {
            Some("reset by command")
        } else {
            None
        }
    }
}

/// One exposure under way.
struct Run {
    d: Derived,
    frames: Frames,
    sums: Sums,
    first_over: Vec<u32>,
    /// Bin 0's centre, Hz, absolute.
    low_edge_hz: f64,
}

impl Run {
    fn new(d: Derived, centre: u64) -> Run {
        Run {
            frames: Frames::new(&d),
            sums: Sums::new(&d),
            first_over: vec![0; d.bins],
            low_edge_hz: centre as f64 - (d.bins / 2) as f64 * d.bin_hz,
            d,
        }
    }

    /// Start again at `centre`, keeping the buffers.
    fn clear(&mut self, centre: u64) {
        self.frames.clear();
        self.sums.clear();
        self.first_over.iter_mut().for_each(|v| *v = 0);
        self.low_edge_hz = centre as f64 - (self.d.bins / 2) as f64 * self.d.bin_hz;
    }

    fn frame_s(&self) -> f64 {
        1.0 / self.d.bin_hz
    }
}

/// What one frame publishes.
struct Products {
    exposure_s: f64,
    frames: u64,
    dropped: u64,
    depth_db: f64,
    bin_hz: f64,
    span_hz: (f64, f64),
    overview: Vec<Cell>,
    lines: Vec<FloorLine>,
    lines_left_out: usize,
    grids: Vec<Grid>,
    zoom: Result<Zoom, String>,
}

/// The products of the exposure so far, the zoom over `zoom` if one is
/// chosen.
fn products(run: &mut Run, zoom: Option<ZoomSpan>) -> Products {
    let d = &run.d;
    let t = run.sums.frames.max(1);
    let frame_s = run.frame_s();
    let bin_hz = d.bin_hz;
    let mean: Vec<f64> = run.sums.s1.iter().map(|v| v / t as f64).collect();
    let floor = local_floor(&mean, d, t);
    let x: Vec<f64> = mean
        .iter()
        .zip(&floor)
        .map(|(m, f)| if *f > 0.0 { m / f } else { 0.0 })
        .collect();
    let (lines, lines_left_out) = find_lines(
        &x,
        d,
        t,
        frame_s,
        &mut run.first_over,
        run.low_edge_hz,
        bin_hz,
    );
    let grids = find_grids(&lines, 2.0 * bin_hz);
    let sums = &run.sums;
    let lines: Vec<FloorLine> = lines
        .into_iter()
        .map(|line| {
            let p = line.peak_bin;
            let r = spread(sums.s1[p], sums.s2[p], t);
            let a = (x[p] - 1.0).max(0.0);
            FloorLine {
                spread: r,
                spread_sigma: steady_spread(a, t).1.max(spread_sigma_chance(t)),
                steadiness: steadiness(x[p], r, t, frame_s),
                line,
            }
        })
        .collect();

    let centre = |k: usize| run.low_edge_hz + k as f64 * bin_hz;
    let db = |v: f64| (10.0 * v.log10()) as f32;
    let kinds: Vec<BinKind> = kinds(sums, d).collect();
    let per = d.usable.len().div_ceil(MAX_CELLS).max(1);
    let mut scratch: Vec<f32> = Vec::with_capacity(per);
    let overview = d
        .usable
        .clone()
        .step_by(per)
        .map(|a| {
            let b = (a + per).min(d.usable.end);
            scratch.clear();
            scratch.extend((a..b).map(|k| db(x[k])));
            let (mut min, mut max) = (f32::INFINITY, f32::NEG_INFINITY);
            for v in &scratch {
                min = min.min(*v);
                max = max.max(*v);
            }
            let mid = scratch.len() / 2;
            let median = *scratch.select_nth_unstable_by(mid, f32::total_cmp).1;
            let mut cell = Cell {
                from_hz: centre(a) - bin_hz / 2.0,
                to_hz: centre(b - 1) + bin_hz / 2.0,
                min_db: min,
                median_db: median,
                max_db: max,
                chance: 0,
                impulsive: 0,
                steady: 0,
            };
            for k in &kinds[a - d.usable.start..b - d.usable.start] {
                match k {
                    BinKind::Chance => cell.chance += 1,
                    BinKind::Impulsive => cell.impulsive += 1,
                    BinKind::Steady => cell.steady += 1,
                }
            }
            cell
        })
        .collect();

    let span_hz = (
        centre(d.usable.start) - bin_hz / 2.0,
        centre(d.usable.end - 1) + bin_hz / 2.0,
    );
    let zoom = match zoom {
        None => Err(NO_ZOOM.to_string()),
        Some(z) => zoom_bins(z, run.low_edge_hz, d, span_hz).map(|(k0, k1)| Zoom {
            from_hz: centre(k0),
            bin_hz,
            excess_db: (k0..=k1).map(|k| db(x[k])).collect(),
            spread: (k0..=k1)
                .map(|k| spread(sums.s1[k], sums.s2[k], t) as f32)
                .collect(),
        }),
    };

    Products {
        exposure_s: run.sums.frames as f64 * frame_s,
        frames: run.sums.frames,
        dropped: run.sums.dropped,
        depth_db: depth_db(t),
        bin_hz,
        span_hz,
        overview,
        lines,
        lines_left_out,
        grids,
        zoom,
    }
}

/// The first and last bin of a zoom, or why it cannot be had at this tuning.
fn zoom_bins(
    z: ZoomSpan,
    low_edge_hz: f64,
    d: &Derived,
    span_hz: (f64, f64),
) -> Result<(usize, usize), String> {
    let outside = || {
        format!(
            "the zoom {}–{} Hz reaches outside the band examined, {}–{} Hz",
            grouped(z.from_hz as usize),
            grouped(z.to_hz as usize),
            grouped(span_hz.0.ceil() as usize),
            grouped(span_hz.1.floor() as usize),
        )
    };
    let k0 = ((z.from_hz as f64 - low_edge_hz) / d.bin_hz).ceil();
    let k1 = ((z.to_hz as f64 - low_edge_hz) / d.bin_hz).floor();
    if z.to_hz <= z.from_hz || k0 < d.usable.start as f64 || k1 >= d.usable.end as f64 || k1 < k0 {
        return Err(outside());
    }
    let (k0, k1) = (k0 as usize, k1 as usize);
    let n = k1 - k0 + 1;
    if n > MAX_ZOOM_BINS {
        return Err(format!(
            "the zoom spans {} bins; at most {}",
            grouped(n),
            grouped(MAX_ZOOM_BINS)
        ));
    }
    Ok((k0, k1))
}

struct Session {
    geometry: SampleGeometry,
    iq: Vec<Complex<f32>>,
    key: Option<Key>,
    run: Option<Run>,
    refused: Option<String>,
    /// The pair the next block should start at.
    next_pair: Option<u64>,
}

impl Session {
    fn new(geometry: SampleGeometry) -> Self {
        Self {
            geometry,
            iq: Vec::new(),
            key: None,
            run: None,
            refused: None,
            next_pair: None,
        }
    }

    fn block(&mut self, b: &StreamBlock, state: &Mutex<SdrMetrics>) {
        let (setting, key): (FloorSetting, Key) = {
            let m = state.lock().unwrap_or_else(|e| e.into_inner());
            let f = &m.floor;
            (
                f.setting,
                Key {
                    centre: b.centre_hz,
                    rate: b.rate_hz,
                    bin_hz: f.setting.bin_hz,
                    gains: m.radio.gains.clone(),
                    amp: m.radio.amp_enabled,
                    generation: f.reset_generation,
                },
            )
        };
        crate::signal::demod::decode(&b.bytes, self.geometry, usize::MAX, &mut self.iq);

        // Why this exposure cannot go on, if it cannot: what moved first,
        // then the stream itself.
        let mut restart: Option<String> = match &self.key {
            None => None,
            Some(old) => old.changed(&key).map(str::to_string),
        };
        let fresh = self.key.is_none() || restart.is_some();
        let mut gap_before = b.gap_before;
        if let (false, Some(expected), Some(run)) = (fresh, self.next_pair, &self.run) {
            if b.first_pair < expected {
                restart = Some("the stream started over".into());
            } else {
                let jump = b.first_pair - expected;
                if jump > 2 * run.d.frame_len as u64 {
                    restart = Some(format!(
                        "a gap of {:.1} s in the stream",
                        jump as f64 / b.rate_hz
                    ));
                } else if jump > 0 {
                    gap_before = true;
                }
            }
        }
        if fresh || restart.is_some() {
            match derive(&setting, b.rate_hz) {
                Ok(d) => {
                    self.refused = None;
                    match &mut self.run {
                        Some(run) if run.d == d => run.clear(b.centre_hz),
                        _ => self.run = Some(Run::new(d, b.centre_hz)),
                    }
                }
                Err(why) => {
                    self.refused = Some(why);
                    self.run = None;
                }
            }
            // The first block of a new exposure starts it cleanly.
            gap_before = false;
        }
        self.key = Some(key);
        self.next_pair = Some(b.first_pair + self.iq.len() as u64);

        let products = self.run.as_mut().and_then(|run| {
            let Run { frames, sums, .. } = run;
            (frames.push(&self.iq, gap_before, sums) > 0).then(|| products(run, setting.zoom))
        });

        let now = Instant::now();
        let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
        let f = &mut m.floor;
        f.feed.blocks_in += 1;
        f.feed.last_block = Some(now);
        if b.gap_before {
            f.feed.gaps += 1;
            f.feed.last_loss = Some(now);
        }
        if let Some(why) = restart {
            f.restarts += 1;
            f.last_restart = Some(why);
            f.clear_products();
        }
        if self.refused != f.refused {
            f.refused = self.refused.clone();
            f.clear_products();
        }
        if let Some(p) = products {
            f.exposure_s = p.exposure_s;
            f.frames = p.frames;
            f.dropped = p.dropped;
            f.depth_db = p.depth_db;
            f.bin_hz = p.bin_hz;
            f.span_hz = p.span_hz;
            f.overview = p.overview;
            f.lines = p.lines;
            f.lines_left_out = p.lines_left_out;
            f.grids = p.grids;
            f.zoom = p.zoom;
            f.updated = Some(now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::doppler::testing::{int32_bytes, int32_geometry};
    use crate::signal::floor::testing::{add, noise, tone};

    const CENTRE: u64 = 9_980_000;

    fn state() -> Mutex<SdrMetrics> {
        let mut m = SdrMetrics::fixture().streaming();
        m.mode = crate::engine::mode::Mode::named("lab_floor").unwrap();
        Mutex::new(m)
    }

    /// Feeds a session `seconds` of noise and a tone 10 kHz above `CENTRE`
    /// at `excess` times one bin's noise, from pair `start`, tuned to
    /// `centre`; the first block marked as after a gap when `gap_before`.
    struct Feeder {
        rate: f64,
        seed: u64,
        phase: f64,
        excess: f64,
        seq: u64,
    }

    impl Feeder {
        fn new(rate: f64, excess: f64) -> Feeder {
            Feeder {
                rate,
                seed: 17,
                phase: 0.0,
                excess,
                seq: 0,
            }
        }

        fn feed(
            &mut self,
            session: &mut Session,
            st: &Mutex<SdrMetrics>,
            start_s: f64,
            seconds: f64,
            centre: u64,
            gap_before: bool,
        ) {
            let start = (start_s * self.rate) as u64;
            let total = (seconds * self.rate) as u64;
            let bin_frames = self.rate as usize; // 1 Hz bins
            let sigma = 0.01f32;
            let amp = (sigma as f64 * (1.5 * self.excess / bin_frames as f64).sqrt()) as f32;
            let offset = CENTRE as f64 + 10_000.0 - centre as f64;
            let mut at = 0u64;
            let mut first = true;
            while at < total {
                let n = 16_384usize.min((total - at) as usize);
                let pair = start + at;
                let mut iq = noise(n, sigma, &mut self.seed);
                if self.excess > 0.0 {
                    iq = add(iq, &tone(self.rate, offset, amp, pair, n, &mut self.phase));
                }
                self.seq += 1;
                session.block(
                    &StreamBlock {
                        seq: self.seq,
                        gap_before: gap_before && std::mem::take(&mut first),
                        bytes: int32_bytes(&iq, 8_388_608.0),
                        first_pair: pair,
                        centre_hz: centre,
                        rate_hz: self.rate,
                    },
                    st,
                );
                at += n as u64;
            }
        }
    }

    /// Twenty seconds at 192 kS/s grow an exposure of twenty frames, a
    /// +6 dB tone stands among its lines, and its depth is the frames'.
    /// (At +3 dB the tone's bin, 3 ± 0.5 over twenty frames, misses the
    /// threshold of 2.48 about one time in seven.)
    #[test]
    fn a_synthetic_stream_grows_an_exposure_in_the_state() {
        let st = state();
        let mut s = Session::new(int32_geometry());
        let mut f = Feeder::new(192_000.0, 4.0);
        f.feed(&mut s, &st, 0.0, 20.0, CENTRE, false);
        let m = st.lock().unwrap();
        let fl = &m.floor;
        assert!(fl.frames >= 19, "{}", fl.frames);
        assert_eq!(fl.restarts, 0);
        assert!((fl.depth_db - depth_db(fl.frames)).abs() < 1e-12);
        assert!(
            fl.lines
                .iter()
                .any(|l| (l.line.hz - (CENTRE as f64 + 10_000.0)).abs() < 1.0),
            "{:?}",
            fl.lines.iter().map(|l| l.line.hz).collect::<Vec<_>>()
        );
        assert!(!fl.overview.is_empty() && fl.overview.len() <= MAX_CELLS);
        assert!(fl.span_hz.0 < CENTRE as f64 && fl.span_hz.1 > CENTRE as f64);
        assert_eq!(fl.zoom, Err(NO_ZOOM.to_string()));
        assert!(fl.feed.blocks_in > 0);
    }

    /// A gap shorter than two frames loses the frame it cut and nothing more.
    #[test]
    fn a_short_gap_drops_a_frame_and_keeps_the_exposure() {
        let st = state();
        let mut s = Session::new(int32_geometry());
        let mut f = Feeder::new(48_000.0, 0.0);
        f.feed(&mut s, &st, 0.0, 6.5, CENTRE, false);
        f.feed(&mut s, &st, 6.8, 6.0, CENTRE, true);
        let m = st.lock().unwrap();
        assert_eq!(m.floor.restarts, 0, "{:?}", m.floor.last_restart);
        assert!(m.floor.dropped >= 1);
        assert!(m.floor.frames >= 11, "{}", m.floor.frames);
    }

    /// A block the floor's own queue refused arrives as no gap at all from
    /// the driver: only its place in the stream shows it, and the frame it
    /// cut is dropped all the same.
    #[test]
    fn a_block_the_feed_dropped_is_a_gap_too() {
        let st = state();
        let mut s = Session::new(int32_geometry());
        let mut f = Feeder::new(48_000.0, 0.0);
        f.feed(&mut s, &st, 0.0, 6.5, CENTRE, false);
        f.feed(&mut s, &st, 6.8, 6.0, CENTRE, false);
        let m = st.lock().unwrap();
        assert_eq!(m.floor.restarts, 0, "{:?}", m.floor.last_restart);
        assert_eq!(m.floor.dropped, 1);
        assert_eq!(m.floor.feed.gaps, 0, "the driver lost nothing");
    }

    /// A gap longer than two frames starts the exposure again, and says so.
    #[test]
    fn a_long_gap_restarts_it_with_its_reason() {
        let st = state();
        let mut s = Session::new(int32_geometry());
        let mut f = Feeder::new(48_000.0, 0.0);
        f.feed(&mut s, &st, 0.0, 6.0, CENTRE, false);
        f.feed(&mut s, &st, 11.0, 3.5, CENTRE, true);
        let m = st.lock().unwrap();
        assert_eq!(m.floor.restarts, 1);
        let why = m.floor.last_restart.as_deref().unwrap();
        assert!(why.contains("gap") && why.contains("5.0 s"), "{why}");
        assert!(m.floor.frames <= 4, "{}", m.floor.frames);
    }

    /// A gain changed between blocks moves the floor: the exposure starts
    /// again.
    #[test]
    fn a_gain_change_restarts_it() {
        let st = state();
        let mut s = Session::new(int32_geometry());
        let mut f = Feeder::new(48_000.0, 0.0);
        f.feed(&mut s, &st, 0.0, 4.0, CENTRE, false);
        {
            let mut m = st.lock().unwrap();
            let g = m.radio.gains.first().copied().unwrap_or(0.0);
            if m.radio.gains.is_empty() {
                m.radio.gains.push(g + 6.0);
            } else {
                m.radio.gains[0] = g + 6.0;
            }
        }
        f.feed(&mut s, &st, 4.0, 2.5, CENTRE, false);
        let m = st.lock().unwrap();
        assert_eq!(m.floor.restarts, 1);
        assert!(m.floor.last_restart.as_deref().unwrap().contains("gain"));
        assert!(m.floor.frames <= 3, "{}", m.floor.frames);
    }

    /// A retune starts it again.
    #[test]
    fn a_retune_restarts_it() {
        let st = state();
        let mut s = Session::new(int32_geometry());
        let mut f = Feeder::new(48_000.0, 0.0);
        f.feed(&mut s, &st, 0.0, 4.0, CENTRE, false);
        f.feed(&mut s, &st, 4.0, 2.5, CENTRE + 5_000, false);
        let m = st.lock().unwrap();
        assert_eq!(m.floor.restarts, 1);
        assert_eq!(m.floor.last_restart.as_deref(), Some("retuned"));
    }

    /// The command's reset is a restart with its own reason.
    #[test]
    fn a_reset_by_command_restarts_it() {
        let st = state();
        let mut s = Session::new(int32_geometry());
        let mut f = Feeder::new(48_000.0, 0.0);
        f.feed(&mut s, &st, 0.0, 3.0, CENTRE, false);
        st.lock().unwrap().floor.reset_generation += 1;
        f.feed(&mut s, &st, 3.0, 1.5, CENTRE, false);
        let m = st.lock().unwrap();
        assert_eq!(m.floor.restarts, 1);
        assert_eq!(m.floor.last_restart.as_deref(), Some("reset by command"));
    }

    /// A zoom inside the band is published at full resolution; one reaching
    /// outside it is refused with the band's edges.
    #[test]
    fn a_zoom_is_the_band_at_full_resolution_or_refused() {
        let st = state();
        st.lock().unwrap().floor.setting.zoom = Some(ZoomSpan {
            from_hz: CENTRE + 9_990,
            to_hz: CENTRE + 10_010,
        });
        let mut s = Session::new(int32_geometry());
        let mut f = Feeder::new(48_000.0, 20.0);
        f.feed(&mut s, &st, 0.0, 3.0, CENTRE, false);
        {
            let m = st.lock().unwrap();
            let z = m.floor.zoom.as_ref().unwrap();
            assert_eq!(z.excess_db.len(), 21);
            assert_eq!(z.spread.len(), 21);
            assert!((z.from_hz - (CENTRE as f64 + 9_990.0)).abs() < 1e-6);
            let peak = (0..21).max_by(|&a, &b| z.excess_db[a].total_cmp(&z.excess_db[b]));
            assert_eq!(peak, Some(10));
        }
        st.lock().unwrap().floor.setting.zoom = Some(ZoomSpan {
            from_hz: CENTRE + 23_000,
            to_hz: CENTRE + 25_000,
        });
        f.feed(&mut s, &st, 3.0, 1.0, CENTRE, false);
        let m = st.lock().unwrap();
        let why = m.floor.zoom.as_ref().unwrap_err();
        assert!(why.contains("outside the band"), "{why}");
    }
}
