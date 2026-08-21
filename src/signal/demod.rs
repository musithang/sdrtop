//! FM demodulation as a *measurement*, not audio (see `dev_docs/demod-plan.md`).
//!
//! Phase 2 of the demod plan: a polar discriminator feeding peak / RMS deviation
//! and carrier offset. No NCO (the channel is taken at centre), no MPX baseband
//! FFT, no audio path — those are later phases that slot into the same worker.
//!
//! Two properties of the sample pipeline shape everything here:
//!
//! * **The block stream is lossy by design.** `process_block` forwards blocks with
//!   `try_send` on a bounded channel, so blocks are dropped under load — correct
//!   load-shedding for a display feed, but it means phase cannot be carried across
//!   a block boundary. Every block is therefore demodulated independently, and the
//!   discriminator inherently drops one sample per block (N inputs → N−1 outputs),
//!   which is exactly the splice guard we need.
//! * **CPU is a displayed metric.** Work is bounded twice: at most [`SLICE_PAIRS`]
//!   input pairs are processed per update, and updates run at [`UPDATE_INTERVAL`]
//!   regardless of how fast blocks arrive. Cost is then independent of the device
//!   sample rate — the decimating FIR only computes every `d`-th output, so its
//!   multiply count scales with the *channel* rate, not the ADC rate.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use num_complex::Complex;

use crate::hardware::SampleFormat;
use crate::state::{FmMeasure, Modulation, SdrMetrics};

/// Channel rate targeted for wide-band FM.
///
/// Sized from Carson's rule, not from the deviation limit alone: a broadcast
/// signal at ±75 kHz deviation with 53 kHz of MPX occupies 2·(75 + 53) ≈ 256 kHz,
/// so the channel must pass roughly ±128 kHz. Filtering an FM carrier more
/// narrowly than its Carson bandwidth collapses the envelope on large excursions
/// and produces click artefacts — 2π phase steps the discriminator reports as
/// excursions pinned at its ambiguity limit. At a 320 kHz target the filter
/// (cutoff 0.4 × channel rate) passes ±128 kHz or more at every supported device
/// rate. It also leaves ample room for the 57 kHz RDS subcarrier in the recovered
/// baseband, which a later phase needs.
pub const WFM_TARGET_HZ: f64 = 320_000.0;
/// Channel rate targeted for narrow-band FM voice (12.5 / 25 kHz channels).
pub const NFM_TARGET_HZ: f64 = 25_000.0;

/// Ceiling on I/Q pairs processed per update. 65 536 pairs is 6.5 ms of signal at
/// 10 Msps and 33 ms at 2 Msps — both far more than deviation statistics need,
/// while capping the per-update cost at a fixed number of samples.
pub const SLICE_PAIRS: usize = 65_536;

/// How often the demod actually runs. Blocks arriving in between are discarded:
/// this is the duty cycle that bounds CPU independently of the device rate.
pub const UPDATE_INTERVAL: Duration = Duration::from_millis(250);

/// Peak-deviation hold decay per update. The peak reading behaves like a bench
/// instrument's peak hold — it latches the loudest excursion and bleeds down when
/// modulation quietens, instead of flickering with every update.
const PEAK_DECAY_HZ: f32 = 2_000.0;

/// Smoothing applied to the RMS-deviation and carrier-offset readouts, so they
/// settle instead of jittering at the update rate.
const EMA_ALPHA: f32 = 0.3;

/// Decimation factor to bring `sample_rate` down to roughly `target_hz`.
///
/// Rounded **down**, so the resulting channel is never narrower than the target.
/// The error is deliberately one-sided: a channel slightly wider than asked for
/// costs a few more samples to process, while one slightly narrower can clip the
/// signal's Carson bandwidth and make the discriminator click. Rounding to
/// nearest gets this wrong at 2.4 Msps, where ÷8 lands on a 300 kHz channel whose
/// passband misses ±128 kHz.
///
/// The factor is an integer, so the *actual* channel rate is [`channel_rate`]
/// rather than the target — reported as such, in keeping with the app's rule of
/// showing measured values instead of intended ones.
pub fn decimation_factor(sample_rate: f64, target_hz: f64) -> usize {
    // `is_finite` first so a NaN rate can never slip through the comparisons
    // below (every ordering test against NaN is false).
    if !(sample_rate.is_finite() && target_hz.is_finite()) || sample_rate <= 0.0 || target_hz <= 0.0 {
        return 1;
    }
    (sample_rate / target_hz).floor().max(1.0) as usize
}

/// The channel rate a decimation factor actually lands on.
pub fn channel_rate(sample_rate: f64, d: usize) -> f64 {
    if d == 0 { return sample_rate; }
    sample_rate / d as f64
}

/// Tap count for a decimate-by-`d` channel filter.
///
/// A Hamming-windowed sinc has a transition width of roughly `3.3 / taps`
/// (normalised to the input rate). Holding that to about a fifth of the channel
/// bandwidth needs ~16.5 × `d` taps. Clamped: below 31 the filter is too soft to
/// reject a neighbouring station, and above 511 the cost stops buying quality —
/// at very high sample rates the filter degrades gracefully, which is why the
/// panel advises dropping the sample rate rather than pretending otherwise.
pub fn tap_count(d: usize) -> usize {
    let n = ((d as f64) * 16.5).round() as usize;
    let n = n.clamp(31, 511);
    n | 1 // odd → a symmetric filter with an exact centre tap
}

/// Decimation factor beyond which [`tap_count`] saturates at its 511-tap cap and
/// the channel filter stops sharpening. Past this the panel advises a lower
/// sample rate rather than silently returning a softer measurement.
pub const FILTER_QUALITY_D_LIMIT: usize = 31;

/// Hamming-windowed sinc low-pass. `fc` is the cutoff in cycles/sample (< 0.5).
pub fn design_lowpass(taps: usize, fc: f64) -> Vec<f32> {
    use std::f64::consts::PI;
    let taps = taps.max(1) | 1;
    let m = (taps - 1) as f64 / 2.0;
    let mut h = Vec::with_capacity(taps);
    let mut sum = 0.0f64;
    for i in 0..taps {
        let x = i as f64 - m;
        // sinc, with the removable singularity at the centre tap handled exactly.
        let sinc = if x.abs() < 1e-9 { 2.0 * fc } else { (2.0 * PI * fc * x).sin() / (PI * x) };
        let w = 0.54 - 0.46 * (2.0 * PI * i as f64 / (taps - 1).max(1) as f64).cos();
        let v = sinc * w;
        sum += v;
        h.push(v);
    }
    // Normalise to unit DC gain so decimation does not change the level, and the
    // deviation figures stay in real Hz.
    if sum.abs() > 1e-12 {
        for v in h.iter_mut() { *v /= sum; }
    }
    h.into_iter().map(|v| v as f32).collect()
}

/// Decode raw wire bytes into complex samples, taking at most `max_pairs`.
///
/// Deliberately unwindowed — this is a time-domain signal path, not an FFT input.
pub fn decode(buf: &[u8], format: SampleFormat, max_pairs: usize, out: &mut Vec<Complex<f32>>) {
    out.clear();
    let pairs = (buf.len() / 2).min(max_pairs);
    out.reserve(pairs);
    let bytes = &buf[..pairs * 2];
    match format {
        SampleFormat::Int8 => {
            for pair in bytes.chunks_exact(2) {
                out.push(Complex {
                    re: pair[0] as i8 as f32 / 128.0,
                    im: pair[1] as i8 as f32 / 128.0,
                });
            }
        }
        SampleFormat::Uint8 => {
            for pair in bytes.chunks_exact(2) {
                out.push(Complex {
                    re: (pair[0] as f32 - 127.5) / 127.5,
                    im: (pair[1] as f32 - 127.5) / 127.5,
                });
            }
        }
    }
}

/// Decimating FIR channel filter.
///
/// Only every `d`-th output is computed, so the multiply count is
/// `taps × input.len() / d` — proportional to the channel rate, not the ADC rate.
/// That is what keeps the cost flat from 2 Msps to 20 Msps.
pub fn decimate(input: &[Complex<f32>], taps: &[f32], d: usize, out: &mut Vec<Complex<f32>>) {
    out.clear();
    let d = d.max(1);
    let n = taps.len();
    if input.len() < n || n == 0 { return; }
    out.reserve((input.len() - n) / d + 1);
    let mut start = 0usize;
    while start + n <= input.len() {
        let window = &input[start..start + n];
        let mut acc = Complex { re: 0.0f32, im: 0.0f32 };
        for (s, &h) in window.iter().zip(taps.iter()) {
            acc.re += s.re * h;
            acc.im += s.im * h;
        }
        out.push(acc);
        start += d;
    }
}

/// Envelope floor, as a fraction of the block's RMS amplitude, below which a
/// sample's phase carries no usable information and is discarded.
///
/// An FM carrier has a constant envelope, so on a clean signal every sample
/// passes this gate. The envelope only collapses where the phase is meaningless
/// anyway — noise nulls, and the beat nulls of a second carrier inside the
/// channel. Those are exactly the samples that produce a full 2π phase step,
/// which the discriminator reports as an excursion pinned at its ±rate/2
/// ambiguity limit. Ungated, a handful of them dominate the peak reading: on a
/// strong broadcast station the peak tracks the ambiguity rail (and rises when
/// the channel widens) instead of the modulation.
const ENVELOPE_GATE: f32 = 0.35;

/// Polar discriminator: instantaneous frequency in Hz.
///
/// `f[n] = arg(z[n+1] · conj(z[n])) · rate / 2π`, unambiguous to ±`rate`/2 — at a
/// 333 kHz channel rate that is ±166 kHz, comfortably clear of the 75 kHz WFM
/// limit. Always yields `len − 1` outputs: the missing first sample is precisely
/// the block-splice guard, since the previous block's last phase is not usable.
///
/// Samples failing [`ENVELOPE_GATE`] are replaced by the previous trustworthy
/// value rather than removed. Dropping them would leave a non-uniform time base,
/// which the MPX baseband spectrum cannot work from — a gap shifts every later
/// sample in time and smears the 19 kHz pilot. Holding keeps the sample grid
/// intact, and since the gate only fires on rare envelope collapses, the
/// spectral cost is far smaller than the aliasing that dropping would cause.
pub fn fm_discriminate(iq: &[Complex<f32>], rate: f64, out: &mut Vec<f32>) {
    use std::f64::consts::PI;
    out.clear();
    if iq.len() < 2 { return; }

    // Mean power over the block sets the gate. Compared in the squared domain so
    // the per-sample test needs no square root.
    let mean_sq = iq.iter().map(|z| z.norm_sqr() as f64).sum::<f64>() / iq.len() as f64;
    if !mean_sq.is_finite() || mean_sq <= 0.0 { return; }
    let floor_sq = (mean_sq * (ENVELOPE_GATE * ENVELOPE_GATE) as f64) as f32;

    out.reserve(iq.len() - 1);
    let scale = (rate / (2.0 * PI)) as f32;
    let mut held = 0.0f32;
    let mut have_held = false;
    for w in iq.windows(2) {
        // Both endpoints must be trustworthy — the phase step spans the pair.
        if w[0].norm_sqr() < floor_sq || w[1].norm_sqr() < floor_sq {
            out.push(held);
            continue;
        }
        let prod = w[1] * w[0].conj();
        let f = prod.im.atan2(prod.re) * scale;
        if !have_held {
            // Backfill any leading run gated out before the first valid sample,
            // so the block never opens with a fabricated zero.
            for v in out.iter_mut() { *v = f; }
            have_held = true;
        }
        held = f;
        out.push(f);
    }
}

/// Mix the stream down by `offset_hz`, bringing a channel at that offset from the
/// tuned centre to DC ready for the channel filter.
///
/// This is what lets the bench demodulate a station the radio is *not* centred
/// on — the point being that the tuned centre is exactly where both front-ends
/// put their DC offset and LO leakage, so a channel taken there competes with the
/// artefact (see the plan's §3.5). Phase is restarted per block: blocks are
/// independent by design, and a measurement does not care about phase continuity
/// across a gap.
///
/// The phasor advances by repeated complex multiplication rather than a `sin`/`cos`
/// per sample, renormalised periodically so rounding cannot let it drift off the
/// unit circle over a long block.
pub fn mix_offset(iq: &mut [Complex<f32>], offset_hz: f64, sample_rate: f64) {
    use std::f64::consts::PI;
    if offset_hz == 0.0 || !sample_rate.is_finite() || sample_rate <= 0.0 { return; }
    let dphi = -2.0 * PI * offset_hz / sample_rate;
    let step = Complex { re: dphi.cos() as f32, im: dphi.sin() as f32 };
    let mut ph = Complex { re: 1.0f32, im: 0.0f32 };
    for (i, z) in iq.iter_mut().enumerate() {
        *z *= ph;
        ph *= step;
        if i % 1024 == 1023 {
            let n = ph.norm();
            if n > 0.0 { ph /= n; }
        }
    }
}

/// FFT size for the recovered MPX baseband. At a ~333 kHz channel rate this gives
/// ~163 Hz resolution — ample to isolate the 19 kHz pilot from its neighbourhood
/// and to place the 38 kHz stereo subcarrier and 57 kHz RDS.
pub const MPX_FFT_SIZE: usize = 2048;

/// Upper edge of the MPX display span. Covers the pilot (19 kHz), the stereo
/// difference signal (38 kHz) and RDS (57 kHz) with a little headroom.
pub const MPX_SPAN_HZ: f64 = 60_000.0;

/// The stereo pilot's frequency.
pub const PILOT_HZ: f64 = 19_000.0;

/// Spectrum of the recovered MPX baseband, in **Hz of deviation per bin**.
///
/// The discriminator's output is already instantaneous deviation in Hz, so each
/// bin's amplitude is the deviation contributed by that MPX component — which is
/// exactly how pilot injection is specified. Scaling is `2·|X[k]| / Σw`, the
/// standard single-sided amplitude recovery for a windowed transform.
///
/// Only the first `MPX_FFT_SIZE` samples are transformed; the caller supplies the
/// planner-built transform and its window so nothing is re-planned per update.
pub fn mpx_spectrum(
    inst_hz: &[f32],
    window: &[f32],
    fft: &dyn rustfft::Fft<f32>,
    scratch: &mut Vec<Complex<f32>>,
    out: &mut Vec<f32>,
) {
    out.clear();
    let n = window.len();
    if inst_hz.len() < n || n == 0 { return; }

    // Remove the carrier offset first: it is a DC term in this domain, and a large
    // one would leak across the low bins through the window's skirts.
    let mean = inst_hz[..n].iter().map(|&f| f as f64).sum::<f64>() / n as f64;

    scratch.clear();
    scratch.extend(inst_hz[..n].iter().zip(window.iter())
        .map(|(&f, &w)| Complex { re: (f as f64 - mean) as f32 * w, im: 0.0 }));
    fft.process(scratch);

    let w_sum: f32 = window.iter().sum();
    if w_sum <= 0.0 { return; }
    let scale = 2.0 / w_sum;
    // Only the positive half carries information for a real input.
    out.extend(scratch[..n / 2].iter().map(|z| z.norm() * scale));
}

/// How confidently a 19 kHz stereo pilot is present.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PilotState {
    Absent,
    /// Detectable but below a trustworthy injection level — reported as such
    /// rather than being called stereo.
    Marginal,
    Locked,
}

/// A pilot measurement: its deviation contribution and injection ratio.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PilotMeasure {
    pub state:         PilotState,
    pub deviation_hz:  f32,
    /// Deviation as a percentage of the mode's peak-deviation limit. Broadcast
    /// practice nominally injects the pilot at 8–10 %.
    pub injection_pct: f32,
}

/// Injection at or above this percentage counts as a locked pilot — half the
/// 8–10 % nominal, so a weakly injected but genuine pilot still reads as stereo.
const PILOT_LOCK_PCT: f32 = 4.0;
/// Below this the line is indistinguishable from baseband content near 19 kHz.
const PILOT_MARGINAL_PCT: f32 = 1.5;

/// Measure the pilot from an MPX spectrum.
///
/// Takes the strongest bin within a small neighbourhood of 19 kHz rather than one
/// exact bin: the pilot rarely lands on a bin centre, and window leakage spreads
/// it over its neighbours.
pub fn pilot_measure(mags_hz: &[f32], bin_hz: f64, limit_hz: f32) -> PilotMeasure {
    let absent = PilotMeasure { state: PilotState::Absent, deviation_hz: 0.0, injection_pct: 0.0 };
    if mags_hz.is_empty() || bin_hz <= 0.0 || limit_hz <= 0.0 { return absent; }

    let centre = (PILOT_HZ / bin_hz).round() as usize;
    let lo = centre.saturating_sub(2);
    let hi = (centre + 2).min(mags_hz.len().saturating_sub(1));
    if lo > hi { return absent; }

    let dev = mags_hz[lo..=hi].iter().copied().fold(0.0f32, f32::max);
    let pct = dev / limit_hz * 100.0;
    let state = if pct >= PILOT_LOCK_PCT          { PilotState::Locked }
                else if pct >= PILOT_MARGINAL_PCT { PilotState::Marginal }
                else                              { PilotState::Absent };
    PilotMeasure { state, deviation_hz: dev, injection_pct: pct }
}

/// Quantile used for the peak-deviation reading — a quasi-peak detector rather
/// than a raw maximum.
///
/// A single corrupted sample pair produces an enormous phase step, and the
/// discriminator turns that into an excursion pinned near its ±rate/2 ambiguity
/// limit. Taking the absolute maximum therefore reports impulse noise as
/// modulation: on a strong broadcast station it reads ~125 kHz against a 16 kHz
/// RMS, a crest factor no real transmitter produces. Ignoring the top 0.1 % of
/// samples rejects those outliers while still tracking genuine programme peaks —
/// for a sine the 99.9th percentile sits within 0.001 % of the true peak.
const PEAK_QUANTILE: f64 = 0.999;

/// Peak / RMS deviation and carrier offset from a discriminator output.
///
/// The carrier offset is the mean instantaneous frequency, and deviation is
/// measured *about that mean* — otherwise a mistuned radio would report its own
/// tuning error as modulation and inflate the deviation figure. The peak is a
/// [`PEAK_QUANTILE`] quasi-peak, found with the same O(n) partial sort the FFT
/// worker uses for its noise floor.
pub fn fm_stats(inst_hz: &[f32]) -> Option<FmMeasure> {
    if inst_hz.is_empty() { return None; }
    let n = inst_hz.len() as f64;
    let mean = inst_hz.iter().map(|&f| f as f64).sum::<f64>() / n;

    let mut sq = 0.0f64;
    let mut devs: Vec<f32> = Vec::with_capacity(inst_hz.len());
    for &f in inst_hz {
        let d = f as f64 - mean;
        sq += d * d;
        devs.push(d.abs() as f32);
    }

    let idx = (((devs.len() - 1) as f64) * PEAK_QUANTILE).round() as usize;
    devs.select_nth_unstable_by(idx, |a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let peak = devs[idx];

    Some(FmMeasure {
        peak_dev_hz:       peak,
        rms_dev_hz:        (sq / n).sqrt() as f32,
        carrier_offset_hz: mean as f32,
    })
}

/// The channel rate to target for a modulation, or `None` when the signal is not
/// something an FM discriminator says anything meaningful about. AM and an
/// unclassified carrier deliberately produce no reading rather than a wrong one.
pub fn target_rate_for(modulation: Modulation) -> Option<f64> {
    match modulation {
        Modulation::Wfm => Some(WFM_TARGET_HZ),
        Modulation::Nfm => Some(NFM_TARGET_HZ),
        Modulation::Am | Modulation::Unknown => None,
    }
}

/// The demod thread. Mirrors [`crate::signal::FftWorker`]: owns its scratch
/// buffers, consumes raw blocks, and writes finished measurements into the shared
/// metrics.
pub struct DemodWorker {
    pub sample_rx: Receiver<Vec<u8>>,
    pub state: Arc<Mutex<SdrMetrics>>,
    pub format: SampleFormat,
}

impl DemodWorker {
    pub fn new(sample_rx: Receiver<Vec<u8>>, state: Arc<Mutex<SdrMetrics>>, format: SampleFormat) -> Self {
        Self { sample_rx, state, format }
    }

    pub fn run(self) {
        // Scratch reused across updates — no per-update allocation.
        let mut iq:   Vec<Complex<f32>> = Vec::new();
        let mut dec:  Vec<Complex<f32>> = Vec::new();
        let mut inst: Vec<f32>          = Vec::new();
        let mut mpx_scratch: Vec<Complex<f32>> = Vec::new();
        let mut mpx_mags:    Vec<f32>          = Vec::new();

        // MPX transform, planned once. Hann keeps the pilot's skirts tight enough
        // that a neighbouring MPX component cannot be mistaken for it.
        let mpx_fft = rustfft::FftPlanner::<f32>::new().plan_fft_forward(MPX_FFT_SIZE);
        let mpx_window = super::dsp::compute_window(super::dsp::WindowFn::Hann, MPX_FFT_SIZE);
        // Filter cache: redesigned only when the decimation factor changes, which
        // happens on a sample-rate change or a WFM↔NFM reclassification.
        let mut taps: Vec<f32> = Vec::new();
        let mut taps_for_d: usize = 0;

        let mut last_update = Instant::now()
            .checked_sub(UPDATE_INTERVAL)
            .unwrap_or_else(Instant::now);

        while let Ok(chunk) = self.sample_rx.recv() {
            // Duty cycle: discard whatever arrives between updates. This is the
            // CPU bound, and dropping here is free — the measurement is
            // statistical, so it does not need a contiguous stream.
            if last_update.elapsed() < UPDATE_INTERVAL { continue; }
            last_update = Instant::now();

            let (sample_rate, modulation, snr_db, streaming, offset_hz) = {
                let m = self.state.lock().unwrap_or_else(|e| e.into_inner());
                (m.radio.config_sample_rate, m.signal.modulation,
                 m.signal.peak_to_nf_db, m.radio.hw_streaming, m.demod.offset_hz)
            };

            // Refuse to guess: without a carrier the classifier reports Unknown,
            // and an FM reading off noise would be meaningless.
            let Some(target) = target_rate_for(modulation) else {
                self.clear();
                continue;
            };
            if !streaming || snr_db < crate::state::CLASSIFY_MIN_SNR_DB {
                self.clear();
                continue;
            }

            let d = decimation_factor(sample_rate, target);
            if d != taps_for_d {
                // Cutoff at 40 % of the channel rate leaves a guard band before the
                // fold-over point at 50 %.
                taps = design_lowpass(tap_count(d), 0.4 / d as f64);
                taps_for_d = d;
            }

            decode(&chunk, self.format, SLICE_PAIRS, &mut iq);
            // Bring the selected channel to DC before filtering, so the channel
            // filter (centred at DC) selects it rather than whatever sits at the
            // tuned frequency.
            mix_offset(&mut iq, offset_hz as f64, sample_rate);
            decimate(&iq, &taps, d, &mut dec);
            let rate = channel_rate(sample_rate, d);
            fm_discriminate(&dec, rate, &mut inst);

            let Some(fresh) = fm_stats(&inst) else {
                self.clear();
                continue;
            };

            // MPX baseband + pilot. A short block simply yields no spectrum this
            // update rather than a padded, misleading one.
            mpx_spectrum(&inst, &mpx_window, mpx_fft.as_ref(), &mut mpx_scratch, &mut mpx_mags);
            let mpx_frame = (!mpx_mags.is_empty()).then(|| {
                Arc::new(crate::state::MpxFrame {
                    bin_hz:  rate / MPX_FFT_SIZE as f64,
                    mags_hz: mpx_mags.clone(),
                })
            });
            let pilot = mpx_frame.as_ref().map(|f| {
                pilot_measure(&f.mags_hz, f.bin_hz, crate::state::deviation_limit_hz(modulation))
            });

            let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
            m.demod.decimation      = d;
            m.demod.channel_rate_hz = rate;
            m.demod.mpx             = mpx_frame;
            m.demod.pilot           = pilot;
            m.demod.fm = Some(match m.demod.fm {
                Some(prev) => FmMeasure {
                    // Peak hold with decay; EMA on the steadier figures.
                    peak_dev_hz:       fresh.peak_dev_hz.max(prev.peak_dev_hz - PEAK_DECAY_HZ),
                    rms_dev_hz:        EMA_ALPHA * fresh.rms_dev_hz + (1.0 - EMA_ALPHA) * prev.rms_dev_hz,
                    carrier_offset_hz: EMA_ALPHA * fresh.carrier_offset_hz
                                         + (1.0 - EMA_ALPHA) * prev.carrier_offset_hz,
                },
                None => fresh,
            });
            m.demod.last_update = Some(Instant::now());
        }
    }

    /// Drop the measurement so the panel falls back to its idle state rather than
    /// leaving a stale number on screen.
    fn clear(&self) {
        let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
        m.demod.fm    = None;
        m.demod.mpx   = None;
        m.demod.pilot = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    /// Synthesise a complex FM tone: carrier at `offset_hz`, sinusoidally
    /// modulated at `mod_hz` with peak deviation `dev_hz`.
    fn fm_signal(rate: f64, n: usize, offset_hz: f64, dev_hz: f64, mod_hz: f64) -> Vec<Complex<f32>> {
        let mut phase = 0.0f64;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let t = i as f64 / rate;
            let inst = offset_hz + dev_hz * (2.0 * PI * mod_hz * t).sin();
            phase += 2.0 * PI * inst / rate;
            out.push(Complex { re: phase.cos() as f32, im: phase.sin() as f32 });
        }
        out
    }

    #[test]
    fn decimation_factor_rounds_down_and_floors_at_one() {
        assert_eq!(decimation_factor(2_000_000.0, 250_000.0), 8);
        // 9.6 rounds *down* to 9 — a wider channel than asked for, never narrower.
        assert_eq!(decimation_factor(2_400_000.0, 250_000.0), 9);
        assert_eq!(decimation_factor(10_000_000.0, 250_000.0), 40);
        // A target above the sample rate cannot decimate at all.
        assert_eq!(decimation_factor(100_000.0, 250_000.0), 1);
        // Degenerate inputs never produce a zero factor (would divide by zero).
        assert_eq!(decimation_factor(0.0, 250_000.0), 1);
        assert_eq!(decimation_factor(2_000_000.0, 0.0), 1);
    }

    #[test]
    fn channel_rate_is_derived_not_assumed() {
        // 2.4 Msps cannot hit the 320 kHz target exactly: ÷7 lands on ~342.9 kHz,
        // and that is what must be reported rather than the target.
        let d = decimation_factor(2_400_000.0, WFM_TARGET_HZ);
        assert_eq!(d, 7);
        assert!((channel_rate(2_400_000.0, d) - 342_857.14).abs() < 1.0);
    }

    #[test]
    fn wfm_channel_covers_carson_bandwidth_at_every_device_rate() {
        // Too narrow a channel makes the discriminator click, so the filter's
        // passband (0.4 × channel rate) must clear ±128 kHz everywhere.
        for sr in [2_000_000.0, 2_400_000.0, 3_200_000.0, 10_000_000.0, 20_000_000.0] {
            let d = decimation_factor(sr, WFM_TARGET_HZ);
            let passband = 0.4 * channel_rate(sr, d);
            assert!(passband >= 128_000.0,
                    "sr={sr}: passband {passband} Hz is inside Carson bandwidth");
        }
    }

    #[test]
    fn tap_count_is_odd_and_clamped() {
        assert_eq!(tap_count(1) % 2, 1);
        assert!(tap_count(1) >= 31);
        assert!(tap_count(1000) <= 511);
        for d in [2usize, 8, 40, 80] {
            assert_eq!(tap_count(d) % 2, 1, "d={d} must give an odd tap count");
        }
    }

    #[test]
    fn lowpass_has_unit_dc_gain() {
        let h = design_lowpass(65, 0.05);
        let dc: f32 = h.iter().sum();
        assert!((dc - 1.0).abs() < 1e-4, "DC gain = {dc}");
    }

    #[test]
    fn lowpass_rejects_out_of_band() {
        // Response at a frequency well inside the stopband should be far down.
        let fc = 0.05;
        let h = design_lowpass(129, fc);
        let eval = |f: f64| -> f64 {
            let (mut re, mut im) = (0.0, 0.0);
            for (n, &c) in h.iter().enumerate() {
                let ph = -2.0 * PI * f * n as f64;
                re += c as f64 * ph.cos();
                im += c as f64 * ph.sin();
            }
            (re * re + im * im).sqrt()
        };
        assert!(eval(0.0) > 0.99, "passband gain {}", eval(0.0));
        assert!(eval(0.20) < 0.01, "stopband gain {} should be < -40 dB", eval(0.20));
    }

    #[test]
    fn discriminator_recovers_a_constant_offset() {
        let rate = 250_000.0;
        let iq = fm_signal(rate, 4096, 10_000.0, 0.0, 0.0);
        let mut inst = Vec::new();
        fm_discriminate(&iq, rate, &mut inst);
        let s = fm_stats(&inst).expect("stats");
        assert!((s.carrier_offset_hz - 10_000.0).abs() < 50.0, "offset = {}", s.carrier_offset_hz);
        // An unmodulated carrier has no deviation.
        assert!(s.peak_dev_hz < 50.0, "peak = {}", s.peak_dev_hz);
    }

    #[test]
    fn discriminator_measures_known_deviation() {
        let rate = 250_000.0;
        // 40 kHz peak deviation, 1 kHz tone — a textbook WFM test signal.
        let iq = fm_signal(rate, 1 << 15, 0.0, 40_000.0, 1_000.0);
        let mut inst = Vec::new();
        fm_discriminate(&iq, rate, &mut inst);
        let s = fm_stats(&inst).expect("stats");
        assert!((s.peak_dev_hz - 40_000.0).abs() < 500.0, "peak = {}", s.peak_dev_hz);
        // A sine's RMS is its peak / √2.
        let expect_rms = 40_000.0 / 2f32.sqrt();
        assert!((s.rms_dev_hz - expect_rms).abs() < 500.0, "rms = {}", s.rms_dev_hz);
    }

    #[test]
    fn deviation_is_measured_about_the_carrier_not_zero() {
        let rate = 250_000.0;
        // Same modulation, but the radio is mistuned by 30 kHz. The tuning error
        // must land in `carrier_offset_hz`, not inflate the deviation.
        let iq = fm_signal(rate, 1 << 15, 30_000.0, 20_000.0, 1_000.0);
        let mut inst = Vec::new();
        fm_discriminate(&iq, rate, &mut inst);
        let s = fm_stats(&inst).expect("stats");
        assert!((s.carrier_offset_hz - 30_000.0).abs() < 500.0, "offset = {}", s.carrier_offset_hz);
        assert!((s.peak_dev_hz - 20_000.0).abs() < 500.0, "peak = {}", s.peak_dev_hz);
    }

    #[test]
    fn decimate_keeps_a_tone_and_lowers_the_rate() {
        // A 5 kHz tone at 2 Msps, decimated by 8 → 250 kHz, is still 5 kHz.
        let rate = 2_000_000.0;
        let d = 8;
        let iq = fm_signal(rate, 1 << 16, 5_000.0, 0.0, 0.0);
        let taps = design_lowpass(tap_count(d), 0.4 / d as f64);
        let mut dec = Vec::new();
        decimate(&iq, &taps, d, &mut dec);
        assert!(dec.len() > 1000, "expected a decimated block, got {}", dec.len());
        let mut inst = Vec::new();
        fm_discriminate(&dec, channel_rate(rate, d), &mut inst);
        let s = fm_stats(&inst).expect("stats");
        assert!((s.carrier_offset_hz - 5_000.0).abs() < 100.0, "offset = {}", s.carrier_offset_hz);
    }

    #[test]
    fn decimate_is_a_noop_when_input_is_shorter_than_the_filter() {
        let taps = design_lowpass(63, 0.1);
        let input = vec![Complex { re: 1.0f32, im: 0.0 }; 10];
        let mut out = Vec::new();
        decimate(&input, &taps, 4, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn discriminator_yields_one_fewer_sample_than_input() {
        // The missing first output is the block-splice guard.
        let iq = fm_signal(250_000.0, 100, 0.0, 0.0, 0.0);
        let mut inst = Vec::new();
        fm_discriminate(&iq, 250_000.0, &mut inst);
        assert_eq!(inst.len(), 99);

        let mut empty = Vec::new();
        fm_discriminate(&[], 250_000.0, &mut empty);
        assert!(empty.is_empty());
    }

    #[test]
    fn fm_stats_none_for_empty_input() {
        assert!(fm_stats(&[]).is_none());
    }

    #[test]
    fn peak_ignores_impulse_outliers() {
        // A clean ±20 kHz sine with a handful of samples corrupted to the
        // discriminator's ambiguity rail — exactly what a few bad sample pairs
        // look like. The reading must follow the signal, not the impulses.
        let rate = 250_000.0;
        let iq = fm_signal(rate, 1 << 14, 0.0, 20_000.0, 1_000.0);
        let mut inst = Vec::new();
        fm_discriminate(&iq, rate, &mut inst);
        for i in [10usize, 500, 4000, 9001] {
            inst[i] = 125_000.0;
        }
        let s = fm_stats(&inst).expect("stats");
        assert!(s.peak_dev_hz < 22_000.0,
                "impulses must not be reported as deviation, got {}", s.peak_dev_hz);
        assert!(s.peak_dev_hz > 18_000.0, "genuine peak still tracked, got {}", s.peak_dev_hz);
    }

    #[test]
    fn envelope_nulls_are_gated_out() {
        let rate = 250_000.0;
        let mut iq = fm_signal(rate, 4096, 0.0, 20_000.0, 1_000.0);
        // Collapse a run of samples to near zero, as a beat null or noise null
        // does. Their phase is meaningless and must not reach the statistics.
        for z in iq.iter_mut().skip(1000).take(50) {
            *z = Complex { re: 1e-4, im: -1e-4 };
        }
        let mut inst = Vec::new();
        fm_discriminate(&iq, rate, &mut inst);
        // The time base must stay uniform for the MPX spectrum, so gated samples
        // are held at the last trustworthy value rather than removed.
        assert_eq!(inst.len(), 4095, "gating must not disturb the sample grid");
        // Nothing survives near the ±rate/2 ambiguity rail.
        let worst = inst.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
        assert!(worst < 100_000.0, "rail-level sample survived the gate: {worst}");
        // The held region carries a plausible deviation, not a fabricated zero.
        assert!(inst[1020].abs() <= 20_000.0 + 1.0, "held value = {}", inst[1020]);
    }

    #[test]
    fn a_leading_gated_run_is_backfilled() {
        // If the block opens inside a null there is no previous value to hold, so
        // the first valid sample is written backwards over the gap — otherwise the
        // block would start with a fabricated zero and put a step in the spectrum.
        let rate = 250_000.0;
        let mut iq = fm_signal(rate, 2048, 15_000.0, 0.0, 0.0);
        for z in iq.iter_mut().take(40) {
            *z = Complex { re: 1e-4, im: 1e-4 };
        }
        let mut inst = Vec::new();
        fm_discriminate(&iq, rate, &mut inst);
        assert_eq!(inst.len(), 2047);
        assert!((inst[0] - 15_000.0).abs() < 200.0,
                "leading gap must be backfilled, got {}", inst[0]);
    }

    #[test]
    fn a_clean_carrier_passes_the_gate_untouched() {
        // Constant envelope: the gate must be a no-op on a healthy signal.
        let iq = fm_signal(250_000.0, 1000, 0.0, 30_000.0, 1_000.0);
        let mut inst = Vec::new();
        fm_discriminate(&iq, 250_000.0, &mut inst);
        assert_eq!(inst.len(), 999);
    }

    #[test]
    fn peak_still_tracks_a_real_over_deviation() {
        // Over-deviation is a real fault the panel must be able to show — the
        // outlier rejection must not flatten a genuinely hot signal.
        let rate = 250_000.0;
        let iq = fm_signal(rate, 1 << 15, 0.0, 90_000.0, 1_000.0);
        let mut inst = Vec::new();
        fm_discriminate(&iq, rate, &mut inst);
        let s = fm_stats(&inst).expect("stats");
        assert!(s.peak_dev_hz > 85_000.0, "expected ~90 kHz, got {}", s.peak_dev_hz);
    }

    /// Synthesise a WFM-style composite: an audio tone plus a 19 kHz pilot at a
    /// known injection, expressed as an FM carrier.
    fn wfm_signal(rate: f64, n: usize, audio_dev: f64, pilot_dev: f64) -> Vec<Complex<f32>> {
        let mut phase = 0.0f64;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let t = i as f64 / rate;
            let inst = audio_dev * (2.0 * PI * 1_000.0 * t).sin()
                     + pilot_dev * (2.0 * PI * PILOT_HZ * t).sin();
            phase += 2.0 * PI * inst / rate;
            out.push(Complex { re: phase.cos() as f32, im: phase.sin() as f32 });
        }
        out
    }

    fn mpx_of(inst: &[f32], rate: f64) -> (Vec<f32>, f64) {
        let fft = rustfft::FftPlanner::<f32>::new().plan_fft_forward(MPX_FFT_SIZE);
        let window = super::super::dsp::compute_window(super::super::dsp::WindowFn::Hann, MPX_FFT_SIZE);
        let (mut scratch, mut mags) = (Vec::new(), Vec::new());
        mpx_spectrum(inst, &window, fft.as_ref(), &mut scratch, &mut mags);
        (mags, rate / MPX_FFT_SIZE as f64)
    }

    #[test]
    fn mix_offset_moves_a_carrier_to_dc() {
        // A carrier 100 kHz up, mixed down by 100 kHz, must sit at 0 Hz.
        let rate = 2_000_000.0;
        let mut iq = fm_signal(rate, 4096, 100_000.0, 0.0, 0.0);
        mix_offset(&mut iq, 100_000.0, rate);
        let mut inst = Vec::new();
        fm_discriminate(&iq, rate, &mut inst);
        let s = fm_stats(&inst).expect("stats");
        assert!(s.carrier_offset_hz.abs() < 200.0, "residual offset {}", s.carrier_offset_hz);
    }

    #[test]
    fn mix_offset_is_a_noop_at_zero_and_keeps_amplitude() {
        let rate = 2_000_000.0;
        let original = fm_signal(rate, 256, 50_000.0, 0.0, 0.0);
        let mut untouched = original.clone();
        mix_offset(&mut untouched, 0.0, rate);
        assert_eq!(untouched[10], original[10], "zero offset must not touch the samples");

        // Mixing is a rotation: it must not change the envelope, or it would move
        // samples across the gate threshold.
        let mut mixed = original.clone();
        mix_offset(&mut mixed, 250_000.0, rate);
        for i in [0usize, 100, 255] {
            assert!((mixed[i].norm() - original[i].norm()).abs() < 1e-3,
                    "envelope changed at {i}");
        }
    }

    #[test]
    fn mpx_spectrum_recovers_pilot_deviation() {
        // 30 kHz audio + a 7.5 kHz pilot — 10 % injection against the 75 kHz limit.
        let rate = 333_000.0;
        let iq = wfm_signal(rate, MPX_FFT_SIZE * 2, 30_000.0, 7_500.0);
        let mut inst = Vec::new();
        fm_discriminate(&iq, rate, &mut inst);
        let (mags, bin_hz) = mpx_of(&inst, rate);
        assert!(!mags.is_empty());

        let p = pilot_measure(&mags, bin_hz, 75_000.0);
        assert_eq!(p.state, PilotState::Locked);
        // Amplitude scaling must return real Hz of deviation, not arbitrary units.
        assert!((p.deviation_hz - 7_500.0).abs() < 800.0, "pilot dev = {}", p.deviation_hz);
        assert!((p.injection_pct - 10.0).abs() < 1.5, "injection = {}", p.injection_pct);
    }

    #[test]
    fn mono_signal_reports_no_pilot() {
        let rate = 333_000.0;
        let iq = wfm_signal(rate, MPX_FFT_SIZE * 2, 30_000.0, 0.0);
        let mut inst = Vec::new();
        fm_discriminate(&iq, rate, &mut inst);
        let (mags, bin_hz) = mpx_of(&inst, rate);
        let p = pilot_measure(&mags, bin_hz, 75_000.0);
        assert_eq!(p.state, PilotState::Absent, "injection = {}", p.injection_pct);
    }

    #[test]
    fn a_weak_pilot_reads_marginal_not_stereo() {
        // 2 % injection: detectable, but not enough to claim stereo.
        let rate = 333_000.0;
        let iq = wfm_signal(rate, MPX_FFT_SIZE * 2, 30_000.0, 1_500.0);
        let mut inst = Vec::new();
        fm_discriminate(&iq, rate, &mut inst);
        let (mags, bin_hz) = mpx_of(&inst, rate);
        let p = pilot_measure(&mags, bin_hz, 75_000.0);
        assert_eq!(p.state, PilotState::Marginal, "injection = {}", p.injection_pct);
    }

    #[test]
    fn mpx_spectrum_declines_a_short_block() {
        // Fewer samples than the transform needs yields nothing, never a
        // zero-padded spectrum that would read as real signal.
        let fft = rustfft::FftPlanner::<f32>::new().plan_fft_forward(MPX_FFT_SIZE);
        let window = super::super::dsp::compute_window(super::super::dsp::WindowFn::Hann, MPX_FFT_SIZE);
        let (mut scratch, mut mags) = (Vec::new(), Vec::new());
        mpx_spectrum(&[0.0; 100], &window, fft.as_ref(), &mut scratch, &mut mags);
        assert!(mags.is_empty());
    }

    #[test]
    fn pilot_measure_refuses_degenerate_input() {
        assert_eq!(pilot_measure(&[], 163.0, 75_000.0).state, PilotState::Absent);
        assert_eq!(pilot_measure(&[1.0, 2.0], 0.0, 75_000.0).state, PilotState::Absent);
        assert_eq!(pilot_measure(&[1.0, 2.0], 163.0, 0.0).state, PilotState::Absent);
    }

    #[test]
    fn target_rate_only_for_fm_modulations() {
        assert_eq!(target_rate_for(Modulation::Wfm), Some(WFM_TARGET_HZ));
        assert_eq!(target_rate_for(Modulation::Nfm), Some(NFM_TARGET_HZ));
        // AM and "no idea" must produce no reading at all.
        assert_eq!(target_rate_for(Modulation::Am), None);
        assert_eq!(target_rate_for(Modulation::Unknown), None);
    }

    #[test]
    fn decode_respects_the_slice_cap_and_format() {
        let bytes: Vec<u8> = (0..200u16).map(|v| v as u8).collect();
        let mut out = Vec::new();
        decode(&bytes, SampleFormat::Int8, 10, &mut out);
        assert_eq!(out.len(), 10, "must stop at the slice cap");

        // Uint8 decodes around the 127.5 bias: 0x80 is ~zero.
        decode(&[0x80, 0x80], SampleFormat::Uint8, 8, &mut out);
        assert_eq!(out.len(), 1);
        assert!(out[0].re.abs() < 0.01 && out[0].im.abs() < 0.01);
    }
}
