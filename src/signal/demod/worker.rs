// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The demodulator worker: one thread, one block at a time.
//!
//! Everything this module calls is in [`super`] and is a pure function of its
//! arguments. What lives here is the part that is *not* pure - the session state
//! carried between blocks, and the decisions about when to throw it away.
//!
//! Those decisions are the whole difficulty. Four different things invalidate
//! four different subsets of the session, and getting the subset wrong is not a
//! crash but a wrong reading that looks plausible: a station name outliving a
//! retune, or RadioText thrown away on every dropped block. Each reset is a named
//! method on [`Session`] with the reason written next to it, rather than a run of
//! assignments inlined four times.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use crossbeam_channel::Receiver;
use rustfft::num_complex::Complex;

use crate::hardware::SampleGeometry;
use crate::state::{AmMeasure, CtcssMeasure, FmMeasure, Modulation, SdrMetrics};

use super::{
    am_envelope, am_stats, channel_rate, ctcss_detect, decimation_factor, decode, design_lowpass,
    fm_discriminate, fm_stats, mix_offset, mpx_spectrum, pilot_measure, tap_count, target_rate_for,
    StreamingDecimator, CTCSS_WINDOW_S, EMA_ALPHA, MPX_FFT_SIZE, PEAK_DECAY_HZ, SLICE_PAIRS,
    UPDATE_INTERVAL,
};

pub struct DemodWorker {
    pub sample_rx: Receiver<(u64, Vec<u8>)>,
    pub state: Arc<Mutex<SdrMetrics>>,
    pub geometry: SampleGeometry,
}

/// What the worker knows about a block before it looks at the samples.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct BlockPlan {
    /// This block follows the previous one with nothing missing between them.
    pub contiguous: bool,
    /// How many blocks the bounded channel lost since the last one that reached
    /// us. `process` forwards with `try_send` and discards the result, so this
    /// gap is the only record that they existed.
    pub dropped: u64,
}

/// Read the sequence numbers.
///
/// `drop_ref` is the sequence of the previous block *that was forwarded to us*,
/// or `None` when the run has not started - distinct from `last_seq` because the
/// device counts every callback, so the jump across a stretch when the demod was
/// switched off is not a loss and must not be counted as one.
///
/// Wrapping throughout: the counter is a `u64` that never resets, and arithmetic
/// that panics on overflow in a release build would silently do the wrong thing
/// in a debug one.
pub(super) fn plan_block(seq: u64, last_seq: u64, drop_ref: Option<u64>) -> BlockPlan {
    BlockPlan {
        contiguous: seq == last_seq.wrapping_add(1),
        dropped: drop_ref.map_or(0, |prev| seq.wrapping_sub(prev).saturating_sub(1)),
    }
}

/// Whether this modulation must see every block, or can be sampled on the
/// display cadence.
///
/// CTCSS needs an unbroken half-second and RDS a run of whole groups at 87.6 ms
/// each, so both FM modes forgo the duty cycle and pay for every block. AM stays
/// cheap: nothing in it needs continuity.
pub(super) fn needs_continuity(modulation: Modulation) -> bool {
    matches!(modulation, Modulation::Nfm | Modulation::Wfm)
}

/// Everything the worker carries from one block to the next.
///
/// Scratch buffers are reused so a steady state allocates nothing; the rest is
/// the state that makes a *run* of blocks mean more than the blocks separately.
struct Session {
    // Scratch, reused across updates.
    iq: Vec<Complex<f32>>,
    dec: Vec<Complex<f32>>,
    inst: Vec<f32>,
    env: Vec<f32>,
    mpx_scratch: Vec<Complex<f32>>,
    mpx_mags: Vec<f32>,

    /// Channel filter, redesigned only when the decimation factor changes, which
    /// happens on a sample-rate change or a WFM/NFM reclassification.
    taps: Vec<f32>,
    taps_for_d: usize,
    sdec: Option<StreamingDecimator>,
    /// Last decimated sample of the previous block, so the discriminator does not
    /// lose the sample pair that spans a block boundary.
    carry: Option<Complex<f32>>,

    /// Contiguous discriminator output feeding the CTCSS detector, and its window.
    audio: Vec<f32>,
    ctcss_window: Vec<f32>,
    ctcss_len: usize,

    /// The subcarrier demod is rebuilt whenever the channel rate moves; the
    /// protocol decoder outlives it, since PS and RadioText take seconds to
    /// assemble and a rate change is not a reason to forget the station.
    rds: Option<super::super::rds_demod::RdsDemod>,
    rds_dec: super::super::rds::RdsDecoder,
    /// When a whole group last completed. RDS accumulates, so unlike every other
    /// measurement here it has to be told when it stops being about the station
    /// in front of it.
    rds_last_group: Option<Instant>,

    last_seq: u64,
    drop_ref: Option<u64>,
    last_mod: Modulation,
    last_channel: Option<(u64, i64)>,
    last_update: Instant,
}

impl Session {
    fn new() -> Self {
        Self {
            iq: Vec::new(),
            dec: Vec::new(),
            inst: Vec::new(),
            env: Vec::new(),
            mpx_scratch: Vec::new(),
            mpx_mags: Vec::new(),
            taps: Vec::new(),
            taps_for_d: 0,
            sdec: None,
            carry: None,
            audio: Vec::new(),
            ctcss_window: Vec::new(),
            ctcss_len: 0,
            rds: None,
            rds_dec: super::super::rds::RdsDecoder::new(),
            rds_last_group: None,
            last_seq: 0,
            drop_ref: None,
            last_mod: Modulation::Unknown,
            last_channel: None,
            last_update: Instant::now()
                .checked_sub(UPDATE_INTERVAL)
                .unwrap_or_else(Instant::now),
        }
    }

    /// Nothing we hold describes the station in front of us any more: the demod
    /// was switched off, or the radio moved to a different channel.
    ///
    /// A **fresh** decoder rather than `reset()`, which deliberately keeps the PI
    /// code across a dropout. That is exactly the field that must not survive,
    /// since it identifies the station.
    fn forget_station(&mut self) {
        self.rds = None;
        self.rds_dec = super::super::rds::RdsDecoder::new();
        self.rds_last_group = None;
        self.audio.clear();
        self.carry = None;
        if let Some(sd) = self.sdec.as_mut() {
            sd.reset();
        }
    }

    /// A different demodulator is running: a station decoded as WFM has nothing
    /// to say about the next one.
    ///
    /// `reset()`, not a fresh decoder, and the filter is left alone - the samples
    /// are still the same samples, only their interpretation changed.
    fn switch_modulation(&mut self, modulation: Modulation) {
        self.last_mod = modulation;
        self.rds = None;
        self.rds_dec.reset();
        self.rds_last_group = None;
        self.audio.clear();
    }

    /// Blocks were lost, so the run is broken.
    ///
    /// Bits either side of a gap are not the same message: the demod's timing and
    /// the decoder's block sync both start over. `resync`, **not** `reset` - the
    /// text already confirmed is kept, since the station has not changed. `reset`
    /// here used to throw away the name and the RadioText on every dropped block.
    fn break_run(&mut self) {
        if let Some(sd) = self.sdec.as_mut() {
            sd.reset();
        }
        self.audio.clear();
        self.carry = None;
        if let Some(r) = self.rds.as_mut() {
            r.reset();
        }
        self.rds_dec.resync();
    }
}

/// The measurements one block produced, all optional because each modulation
/// fills a different subset.
#[derive(Default)]
struct Measured {
    fm: Option<FmMeasure>,
    am: Option<AmMeasure>,
    mpx: Option<Arc<crate::state::MpxFrame>>,
    pilot: Option<super::PilotMeasure>,
    ctcss: Option<CtcssMeasure>,
    /// How full the CTCSS window is, 0.0 to 1.0.
    fill: f32,
}

impl DemodWorker {
    pub fn new(
        sample_rx: Receiver<(u64, Vec<u8>)>,
        state: Arc<Mutex<SdrMetrics>>,
        geometry: SampleGeometry,
    ) -> Self {
        Self {
            sample_rx,
            state,
            geometry,
        }
    }

    pub fn run(self) {
        // MPX transform, planned once. Hann keeps the pilot's skirts tight enough
        // that a neighbouring MPX component cannot be mistaken for it.
        let mpx_fft = rustfft::FftPlanner::<f32>::new().plan_fft_forward(MPX_FFT_SIZE);
        let mpx_window =
            super::super::dsp::compute_window(super::super::dsp::WindowFn::Hann, MPX_FFT_SIZE);

        let mut s = Session::new();

        while let Ok((seq, chunk)) = self.sample_rx.recv() {
            let plan = plan_block(seq, s.last_seq, s.drop_ref);
            let now = (plan.dropped > 0).then(Instant::now);
            s.last_seq = seq;
            s.drop_ref = Some(seq);

            let (sample_rate, modulation, snr_db, streaming, offset_hz, frequency, enabled) = {
                let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                // Integer accumulation inside the lock, the clock read outside it -
                // and here rather than at the publish, because every path below this
                // can `continue`, and a worker so far behind that it never publishes
                // is exactly the one whose drops need reporting.
                if let Some(t) = now {
                    m.demod.blocks_dropped = m.demod.blocks_dropped.saturating_add(plan.dropped);
                    m.demod.last_drop = Some(t);
                }
                // The user's mode choice outranks the classifier - see
                // `DemodState::mode_override` for why the heuristic is too coarse
                // to pick a demodulator on its own.
                (
                    m.radio.config_sample_rate,
                    m.demod.effective_modulation(m.signal.modulation),
                    m.signal.peak_to_nf_db,
                    m.radio.hw_streaming,
                    m.demod.offset_hz,
                    m.radio.frequency,
                    m.demod.enabled,
                )
            };

            // Switching the demod off stops `process` forwarding, but up to four
            // blocks are already in the channel - and publishing those overwrites
            // the state the key handler just cleared, so the panel went on showing a
            // live pilot lock and a station name under a "DEMOD OFF" headline.
            // Clearing the intent is the user's job; honouring it is this loop's.
            if !enabled {
                s.forget_station();
                // Nothing is forwarded while off, so the next block will be many
                // sequence numbers away without a single one having been lost.
                s.drop_ref = None;
                self.clear();
                continue;
            }

            // A different channel is a different station, and nothing the decoder
            // holds describes it. Retuning does not break block contiguity - the
            // radio keeps streaming - and it does not change the modulation either,
            // so without this check nothing invalidated the RDS state at all: the
            // panel sat naming the old station nine seconds after the radio had
            // moved to 96.6 MHz, group counter frozen.
            let channel = (frequency, offset_hz);
            if s.last_channel.is_some_and(|c| c != channel) {
                s.forget_station();
                self.clear();
                // The drop count answers "is this station undecodable, or is the
                // host behind?", so it belongs to the channel being asked about.
                let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                m.demod.blocks_dropped = 0;
                m.demod.last_drop = None;
            }
            s.last_channel = Some(channel);

            if modulation != s.last_mod {
                s.switch_modulation(modulation);
            }

            // Refuse to guess: without a carrier the classifier reports Unknown,
            // and a reading off noise would be meaningless.
            let Some(target) = target_rate_for(modulation) else {
                self.clear();
                s.audio.clear();
                s.rds_dec.reset();
                continue;
            };
            if !streaming || snr_db < crate::state::CLASSIFY_MIN_SNR_DB {
                self.clear();
                s.audio.clear();
                // Same reasoning as the contiguity break below: a fade under the
                // gate stops the stream, it does not move the radio. The channel
                // check above is what handles a change of station.
                s.rds_dec.resync();
                continue;
            }

            let continuous = needs_continuity(modulation);
            let due = s.last_update.elapsed() >= UPDATE_INTERVAL;
            if !continuous && !due {
                continue;
            }

            let d = decimation_factor(sample_rate, target);
            if d != s.taps_for_d {
                // Cutoff at 40 % of the channel rate leaves a guard band before the
                // fold-over point at 50 %.
                s.taps = design_lowpass(tap_count(d), 0.4 / d as f64);
                s.taps_for_d = d;
                s.sdec = Some(StreamingDecimator::new(s.taps.clone(), d));
                s.audio.clear();
                s.carry = None;
            }
            if s.sdec.is_none() {
                s.sdec = Some(StreamingDecimator::new(s.taps.clone(), d));
            }
            // A dropped block, or a skipped one under the duty cycle, ends the run.
            if !plan.contiguous {
                s.break_run();
            }

            // Continuity means every sample counts; otherwise the bounded slice
            // keeps the cost flat regardless of device rate.
            let cap = if continuous { usize::MAX } else { SLICE_PAIRS };
            decode(&chunk, self.geometry, cap, &mut s.iq);
            // Bring the selected channel to DC before filtering, so the channel
            // filter (centred at DC) selects it rather than whatever sits at the
            // tuned frequency.
            mix_offset(&mut s.iq, offset_hz as f64, sample_rate);
            let Some(sd) = s.sdec.as_mut() else { continue };
            sd.process(&s.iq, &mut s.dec);
            let rate = channel_rate(sample_rate, d);
            if s.dec.is_empty() {
                continue;
            }

            // Rejoin the boundary sample pair so a contiguous run really is one.
            if let Some(prev) = s.carry.take() {
                s.dec.insert(0, prev);
            }
            if continuous {
                s.carry = s.dec.last().copied();
            }

            let measured =
                self.measure(&mut s, modulation, rate, due, &mpx_window, mpx_fft.as_ref());

            // Publishing stays on the update cadence even when the intake does
            // not, so the display rate and the lock traffic are unchanged.
            if !due {
                continue;
            }
            s.last_update = Instant::now();

            // Snapshot the accumulating RDS state for the display. Cloned here,
            // after the cadence gate, so the per-block path stays allocation-free.
            let (rds_out, rds_sync) = if matches!(modulation, Modulation::Wfm) {
                (Some(Arc::new(s.rds_dec.data().clone())), s.rds_dec.locked())
            } else {
                (None, false)
            };

            let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
            // Last word on whether this measurement is still wanted, taken under the
            // lock that is about to publish it. The check at the top of the loop
            // cannot be enough: up to four blocks sit in the channel, and any of
            // them read `enabled` before the key press and would publish afterwards.
            // Same for the channel - an offset key or a retune between the intake
            // and here makes this measurement about a frequency nobody is on.
            if !m.demod.enabled || (m.radio.frequency, m.demod.offset_hz) != channel {
                m.demod.clear_measurements();
                continue;
            }
            m.demod.decimation = d;
            m.demod.channel_rate_hz = rate;
            m.demod.mpx = measured.mpx;
            m.demod.pilot = measured.pilot;
            m.demod.rds = rds_out;
            m.demod.rds_sync = rds_sync;
            m.demod.rds_last_group = s.rds_last_group;
            m.demod.am = measured.am;
            m.demod.ctcss = measured.ctcss;
            m.demod.ctcss_fill = measured.fill;
            m.demod.fm = measured.fm.map(|fresh| match m.demod.fm {
                Some(prev) => FmMeasure {
                    // Peak hold with decay; EMA on the steadier figures.
                    peak_dev_hz: fresh.peak_dev_hz.max(prev.peak_dev_hz - PEAK_DECAY_HZ),
                    rms_dev_hz: EMA_ALPHA * fresh.rms_dev_hz + (1.0 - EMA_ALPHA) * prev.rms_dev_hz,
                    carrier_offset_hz: EMA_ALPHA * fresh.carrier_offset_hz
                        + (1.0 - EMA_ALPHA) * prev.carrier_offset_hz,
                },
                None => fresh,
            });
            m.demod.last_update = Some(Instant::now());
        }
    }

    /// Run whichever demodulator the modulation calls for over the decimated
    /// block, and collect what it measured.
    ///
    /// Takes the whole session because the FM path accumulates: RDS rides on
    /// every block and CTCSS builds a contiguous half-second, so neither can be
    /// expressed as a function of this block alone.
    fn measure(
        &self,
        s: &mut Session,
        modulation: Modulation,
        rate: f64,
        due: bool,
        mpx_window: &[f32],
        mpx_fft: &dyn rustfft::Fft<f32>,
    ) -> Measured {
        let mut out = Measured::default();
        match modulation {
            Modulation::Am => {
                am_envelope(&s.dec, &mut s.env);
                out.am = am_stats(&s.env);
            }
            Modulation::Wfm | Modulation::Nfm => {
                fm_discriminate(&s.dec, rate, &mut s.inst);
                out.fm = fm_stats(&s.inst);
                if matches!(modulation, Modulation::Wfm) {
                    self.wfm(s, rate, due, mpx_window, mpx_fft, &mut out);
                } else {
                    self.nfm(s, rate, &mut out);
                }
            }
            Modulation::Unknown => {}
        }
        out
    }

    /// Broadcast FM: RDS every block, MPX and pilot on the display cadence.
    fn wfm(
        &self,
        s: &mut Session,
        rate: f64,
        due: bool,
        mpx_window: &[f32],
        mpx_fft: &dyn rustfft::Fft<f32>,
        out: &mut Measured,
    ) {
        // RDS rides on every block: the bitstream only survives if the run is
        // unbroken, so unlike the spectrum below it cannot be sampled on the
        // display cadence.
        if s.rds.as_ref().is_some_and(|r| r.rate() != rate) {
            s.rds = None;
            s.rds_dec.reset();
        }
        let rd = s
            .rds
            .get_or_insert_with(|| super::super::rds_demod::RdsDemod::new(rate));
        // Compared across the call rather than against a tracked counter:
        // `reset()` zeroes `groups_ok`, and any scheme that remembers the old
        // value across a reset stops stamping.
        let before = s.rds_dec.data().groups_ok;
        rd.process(&s.inst, &mut s.rds_dec);
        if s.rds_dec.data().groups_ok != before {
            s.rds_last_group = Some(Instant::now());
        }

        // MPX baseband + pilot, only when it will actually be published - a
        // spectrum nobody reads is wasted work. A short block simply yields no
        // spectrum rather than a padded, misleading one.
        if !due {
            return;
        }
        mpx_spectrum(
            &s.inst,
            mpx_window,
            mpx_fft,
            &mut s.mpx_scratch,
            &mut s.mpx_mags,
        );
        out.mpx = (!s.mpx_mags.is_empty()).then(|| {
            Arc::new(crate::state::MpxFrame {
                bin_hz: rate / MPX_FFT_SIZE as f64,
                mags_hz: s.mpx_mags.clone(),
            })
        });
        out.pilot = out.mpx.as_ref().map(|f| {
            pilot_measure(
                &f.mags_hz,
                f.bin_hz,
                crate::state::deviation_limit_hz(Modulation::Wfm),
            )
        });
    }

    /// Narrow-band FM: accumulate the contiguous run the tone detector needs.
    fn nfm(&self, s: &mut Session, rate: f64, out: &mut Measured) {
        let want = (CTCSS_WINDOW_S * rate) as usize;
        if want != s.ctcss_len && want > 0 {
            s.ctcss_len = want;
            s.ctcss_window =
                super::super::dsp::compute_window(super::super::dsp::WindowFn::Hann, want);
            s.audio.clear();
        }
        s.audio.extend_from_slice(&s.inst);
        if s.audio.len() > s.ctcss_len {
            let excess = s.audio.len() - s.ctcss_len;
            s.audio.drain(..excess);
        }
        out.fill = if s.ctcss_len > 0 {
            (s.audio.len() as f32 / s.ctcss_len as f32).min(1.0)
        } else {
            0.0
        };
        if s.audio.len() >= s.ctcss_len && s.ctcss_len > 0 {
            out.ctcss = ctcss_detect(&s.audio, &s.ctcss_window, rate);
        }
    }

    /// Drop the measurements so the panel falls back to its idle state rather than
    /// leaving stale numbers on screen.
    fn clear(&self) {
        let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
        m.demod.clear_measurements();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_that_follows_the_last_one_is_contiguous() {
        assert!(plan_block(5, 4, Some(4)).contiguous);
        assert!(!plan_block(6, 4, Some(4)).contiguous);
        // The very first block: nothing precedes it, so nothing was dropped.
        assert_eq!(plan_block(0, 0, None).dropped, 0);
    }

    #[test]
    fn dropped_counts_the_gap_and_not_the_block_itself() {
        // 4 then 7: blocks 5 and 6 were lost, which is two, not three.
        assert_eq!(plan_block(7, 4, Some(4)).dropped, 2);
        assert_eq!(
            plan_block(5, 4, Some(4)).dropped,
            0,
            "back to back loses nothing"
        );
    }

    #[test]
    fn a_stretch_with_the_demod_off_is_not_a_drop() {
        // `drop_ref` is cleared when the demod is switched off, so the jump back
        // in is not counted. Without that, every pause would report thousands of
        // lost blocks and the panel would blame the host.
        assert_eq!(plan_block(9_000, 8_999, None).dropped, 0);
    }

    #[test]
    fn the_sequence_counter_wrapping_does_not_invent_a_drop() {
        // The counter never resets, so the arithmetic has to wrap rather than
        // panic in debug or produce a vast bogus gap in release.
        let p = plan_block(0, u64::MAX, Some(u64::MAX));
        assert!(p.contiguous, "0 follows u64::MAX");
        assert_eq!(p.dropped, 0);
        assert_eq!(plan_block(2, u64::MAX, Some(u64::MAX)).dropped, 2);
    }

    #[test]
    fn only_the_fm_modes_pay_for_every_block() {
        // CTCSS needs an unbroken half-second and RDS a run of whole groups.
        assert!(needs_continuity(Modulation::Wfm));
        assert!(needs_continuity(Modulation::Nfm));
        // AM has nothing that spans blocks, and Unknown never gets this far.
        assert!(!needs_continuity(Modulation::Am));
        assert!(!needs_continuity(Modulation::Unknown));
    }

    #[test]
    fn forgetting_a_station_drops_its_identity_but_keeps_the_buffers() {
        let mut s = Session::new();
        s.audio.extend_from_slice(&[1.0, 2.0, 3.0]);
        s.carry = Some(Complex::new(1.0, 0.0));
        s.rds_last_group = Some(Instant::now());
        s.taps_for_d = 31;

        s.forget_station();
        assert!(s.audio.is_empty() && s.carry.is_none() && s.rds_last_group.is_none());
        // The filter is still the right filter: the channel moved, not the rate.
        assert_eq!(
            s.taps_for_d, 31,
            "a retune must not force a filter redesign"
        );
    }

    #[test]
    fn breaking_a_run_keeps_what_the_station_already_said() {
        // The bug this guards: `reset` here threw away the confirmed name and the
        // RadioText on every dropped block. A gap breaks the bit timing, not the
        // station's identity.
        let mut s = Session::new();
        s.audio.extend_from_slice(&[1.0, 2.0]);
        s.carry = Some(Complex::new(1.0, 0.0));
        let stamp = Instant::now();
        s.rds_last_group = Some(stamp);

        s.break_run();
        assert!(s.audio.is_empty() && s.carry.is_none());
        assert_eq!(s.rds_last_group, Some(stamp), "the station has not changed");
    }

    #[test]
    fn switching_modulation_leaves_the_filter_and_the_carry_alone() {
        // The samples are the same samples; only their interpretation changed.
        let mut s = Session::new();
        s.carry = Some(Complex::new(1.0, 0.0));
        s.taps_for_d = 31;

        s.switch_modulation(Modulation::Nfm);
        assert_eq!(s.last_mod, Modulation::Nfm);
        assert!(
            s.carry.is_some(),
            "the boundary sample still spans the same stream"
        );
        assert_eq!(s.taps_for_d, 31);
        assert!(
            s.audio.is_empty(),
            "a CTCSS run under the old mode is meaningless"
        );
    }
}
