//! FM demodulation as a *measurement*, not audio (see `dev_docs/demod-plan.md`).
//!
//! One worker covers all three modes, branching on the classifier: WFM (polar
//! discriminator + MPX baseband + 19 kHz pilot), NFM (discriminator + CTCSS tone),
//! and AM (envelope depth). An NCO selects the channel so it need not sit on the
//! tuned centre, where both front-ends park their DC offset and LO leakage.
//!
//! Two properties of the sample pipeline shape everything here:
//!
//! * **The block stream is lossy by design.** `process_block` forwards blocks with
//!   `try_send` on a bounded channel, so blocks are dropped under load - correct
//!   load-shedding for a display feed. Statistics tolerate that happily. CTCSS
//!   does not: telling ~2 Hz-apart tones apart needs half a second of *unbroken*
//!   audio, so the demod feed carries a sequence number and the channel filter
//!   ([`StreamingDecimator`]) keeps its state across blocks. A gap resets the run.
//! * **CPU is a displayed metric.** Work is bounded twice: at most [`SLICE_PAIRS`]
//!   input pairs per update, and updates at [`UPDATE_INTERVAL`] regardless of how
//!   fast blocks arrive - so cost is independent of the device sample rate, since
//!   the decimating FIR computes only every `d`-th output and scales with the
//!   *channel* rate. Narrow-band FM is the exception: continuity outranks the duty
//!   cycle there, so it pays for every block.

use std::time::Duration;

use num_complex::Complex;

use crate::hardware::SampleFormat;
use crate::state::{AmMeasure, CtcssMeasure, FmMeasure, Modulation};

/// Channel rate targeted for wide-band FM.
///
/// Sized from Carson's rule, not from the deviation limit alone: a broadcast
/// signal at ±75 kHz deviation with 53 kHz of MPX occupies 2·(75 + 53) ≈ 256 kHz,
/// so the channel must pass roughly ±128 kHz. Filtering an FM carrier more
/// narrowly than its Carson bandwidth collapses the envelope on large excursions
/// and produces click artefacts - 2π phase steps the discriminator reports as
/// excursions pinned at its ambiguity limit. At a 320 kHz target the filter
/// (cutoff 0.4 × channel rate) passes ±128 kHz or more at every supported device
/// rate. It also leaves ample room for the 57 kHz RDS subcarrier in the recovered
/// baseband, which a later phase needs.
pub const WFM_TARGET_HZ: f64 = 320_000.0;
/// Channel rate targeted for narrow-band FM voice (12.5 / 25 kHz channels).
pub const NFM_TARGET_HZ: f64 = 25_000.0;

/// Ceiling on I/Q pairs processed per update. 65 536 pairs is 6.5 ms of signal at
/// 10 Msps and 33 ms at 2 Msps - both far more than deviation statistics need,
/// while capping the per-update cost at a fixed number of samples.
pub const SLICE_PAIRS: usize = 65_536;

/// How often the demod actually runs. Blocks arriving in between are discarded:
/// this is the duty cycle that bounds CPU independently of the device rate.
pub const UPDATE_INTERVAL: Duration = Duration::from_millis(250);

/// Peak-deviation hold decay per update. The peak reading behaves like a bench
/// instrument's peak hold - it latches the loudest excursion and bleeds down when
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
/// rather than the target - reported as such, in keeping with the app's rule of
/// showing measured values instead of intended ones.
pub fn decimation_factor(sample_rate: f64, target_hz: f64) -> usize {
    // `is_finite` first so a NaN rate can never slip through the comparisons
    // below (every ordering test against NaN is false).
    if !(sample_rate.is_finite() && target_hz.is_finite()) || sample_rate <= 0.0 || target_hz <= 0.0
    {
        return 1;
    }
    (sample_rate / target_hz).floor().max(1.0) as usize
}

/// The channel rate a decimation factor actually lands on.
pub fn channel_rate(sample_rate: f64, d: usize) -> f64 {
    if d == 0 {
        return sample_rate;
    }
    sample_rate / d as f64
}

/// Tap count for a decimate-by-`d` channel filter.
///
/// A Hamming-windowed sinc has a transition width of roughly `3.3 / taps`
/// (normalised to the input rate). Holding that to about a fifth of the channel
/// bandwidth needs ~16.5 × `d` taps. Clamped: below 31 the filter is too soft to
/// reject a neighbouring station, and above 511 the cost stops buying quality -
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
        let sinc = if x.abs() < 1e-9 {
            2.0 * fc
        } else {
            (2.0 * PI * fc * x).sin() / (PI * x)
        };
        let w = 0.54 - 0.46 * (2.0 * PI * i as f64 / (taps - 1).max(1) as f64).cos();
        let v = sinc * w;
        sum += v;
        h.push(v);
    }
    // Normalise to unit DC gain so decimation does not change the level, and the
    // deviation figures stay in real Hz.
    if sum.abs() > 1e-12 {
        for v in h.iter_mut() {
            *v /= sum;
        }
    }
    h.into_iter().map(|v| v as f32).collect()
}

/// Decode raw wire bytes into complex samples, taking at most `max_pairs`.
///
/// Deliberately unwindowed - this is a time-domain signal path, not an FFT input.
pub fn decode(buf: &[u8], format: SampleFormat, max_pairs: usize, out: &mut Vec<Complex<f32>>) {
    out.clear();
    let pairs = (buf.len() / 2).min(max_pairs);
    out.reserve(pairs);
    let bytes = &buf[..pairs * 2];
    match format {
        SampleFormat::Int8 => {
            for pair in bytes.as_chunks::<2>().0 {
                out.push(Complex {
                    re: pair[0] as i8 as f32 / 128.0,
                    im: pair[1] as i8 as f32 / 128.0,
                });
            }
        }
        SampleFormat::Uint8 => {
            for pair in bytes.as_chunks::<2>().0 {
                out.push(Complex {
                    re: (pair[0] as f32 - 127.5) / 127.5,
                    im: (pair[1] as f32 - 127.5) / 127.5,
                });
            }
        }
    }
}

/// Envelope floor, as a fraction of the block's RMS amplitude, below which a
/// sample's phase carries no usable information and is discarded.
///
/// An FM carrier has a constant envelope, so on a clean signal every sample
/// passes this gate. The envelope only collapses where the phase is meaningless
/// anyway - noise nulls, and the beat nulls of a second carrier inside the
/// channel. Those are exactly the samples that produce a full 2π phase step,
/// which the discriminator reports as an excursion pinned at its ±rate/2
/// ambiguity limit. Ungated, a handful of them dominate the peak reading: on a
/// strong broadcast station the peak tracks the ambiguity rail (and rises when
/// the channel widens) instead of the modulation.
const ENVELOPE_GATE: f32 = 0.35;

/// Polar discriminator: instantaneous frequency in Hz.
///
/// `f[n] = arg(z[n+1] · conj(z[n])) · rate / 2π`, unambiguous to ±`rate`/2 - at a
/// 333 kHz channel rate that is ±166 kHz, comfortably clear of the 75 kHz WFM
/// limit. Always yields `len − 1` outputs: the missing first sample is precisely
/// the block-splice guard, since the previous block's last phase is not usable.
///
/// Samples failing [`ENVELOPE_GATE`] are replaced by the previous trustworthy
/// value rather than removed. Dropping them would leave a non-uniform time base,
/// which the MPX baseband spectrum cannot work from - a gap shifts every later
/// sample in time and smears the 19 kHz pilot. Holding keeps the sample grid
/// intact, and since the gate only fires on rare envelope collapses, the
/// spectral cost is far smaller than the aliasing that dropping would cause.
pub fn fm_discriminate(iq: &[Complex<f32>], rate: f64, out: &mut Vec<f32>) {
    use std::f64::consts::PI;
    out.clear();
    if iq.len() < 2 {
        return;
    }

    // Mean power over the block sets the gate. Compared in the squared domain so
    // the per-sample test needs no square root.
    let mean_sq = iq.iter().map(|z| z.norm_sqr() as f64).sum::<f64>() / iq.len() as f64;
    if !mean_sq.is_finite() || mean_sq <= 0.0 {
        return;
    }
    let floor_sq = (mean_sq * (ENVELOPE_GATE * ENVELOPE_GATE) as f64) as f32;

    out.reserve(iq.len() - 1);
    let scale = (rate / (2.0 * PI)) as f32;
    let mut held = 0.0f32;
    let mut have_held = false;
    for w in iq.windows(2) {
        // Both endpoints must be trustworthy - the phase step spans the pair.
        if w[0].norm_sqr() < floor_sq || w[1].norm_sqr() < floor_sq {
            out.push(held);
            continue;
        }
        let prod = w[1] * w[0].conj();
        let f = prod.im.atan2(prod.re) * scale;
        if !have_held {
            // Backfill any leading run gated out before the first valid sample,
            // so the block never opens with a fabricated zero.
            for v in out.iter_mut() {
                *v = f;
            }
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
/// on - the point being that the tuned centre is exactly where both front-ends
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
    if offset_hz == 0.0 || !sample_rate.is_finite() || sample_rate <= 0.0 {
        return;
    }
    let dphi = -2.0 * PI * offset_hz / sample_rate;
    let step = Complex {
        re: dphi.cos() as f32,
        im: dphi.sin() as f32,
    };
    let mut ph = Complex {
        re: 1.0f32,
        im: 0.0f32,
    };
    for (i, z) in iq.iter_mut().enumerate() {
        *z *= ph;
        ph *= step;
        if i % 1024 == 1023 {
            let n = ph.norm();
            if n > 0.0 {
                ph /= n;
            }
        }
    }
}

/// FFT size for the recovered MPX baseband. At a ~333 kHz channel rate this gives
/// ~163 Hz resolution - ample to isolate the 19 kHz pilot from its neighbourhood
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
/// bin's amplitude is the deviation contributed by that MPX component - which is
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
    if inst_hz.len() < n || n == 0 {
        return;
    }

    // Remove the carrier offset first: it is a DC term in this domain, and a large
    // one would leak across the low bins through the window's skirts.
    let mean = inst_hz[..n].iter().map(|&f| f as f64).sum::<f64>() / n as f64;

    scratch.clear();
    scratch.extend(
        inst_hz[..n]
            .iter()
            .zip(window.iter())
            .map(|(&f, &w)| Complex {
                re: (f as f64 - mean) as f32 * w,
                im: 0.0,
            }),
    );
    fft.process(scratch);

    let w_sum: f32 = window.iter().sum();
    if w_sum <= 0.0 {
        return;
    }
    let scale = 2.0 / w_sum;
    // Only the positive half carries information for a real input.
    out.extend(scratch[..n / 2].iter().map(|z| z.norm() * scale));
}

/// How confidently a 19 kHz stereo pilot is present.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PilotState {
    Absent,
    /// Detectable but below a trustworthy injection level - reported as such
    /// rather than being called stereo.
    Marginal,
    Locked,
}

/// A pilot measurement: its deviation contribution and injection ratio.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PilotMeasure {
    pub state: PilotState,
    pub deviation_hz: f32,
    /// Deviation as a percentage of the mode's peak-deviation limit. Broadcast
    /// practice nominally injects the pilot at 8–10 %.
    pub injection_pct: f32,
}

/// Injection at or above this percentage counts as a locked pilot - half the
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
    let absent = PilotMeasure {
        state: PilotState::Absent,
        deviation_hz: 0.0,
        injection_pct: 0.0,
    };
    if mags_hz.is_empty() || bin_hz <= 0.0 || limit_hz <= 0.0 {
        return absent;
    }

    let centre = (PILOT_HZ / bin_hz).round() as usize;
    let lo = centre.saturating_sub(2);
    let hi = (centre + 2).min(mags_hz.len().saturating_sub(1));
    if lo > hi {
        return absent;
    }

    let dev = mags_hz[lo..=hi].iter().copied().fold(0.0f32, f32::max);
    let pct = dev / limit_hz * 100.0;
    let state = if pct >= PILOT_LOCK_PCT {
        PilotState::Locked
    } else if pct >= PILOT_MARGINAL_PCT {
        PilotState::Marginal
    } else {
        PilotState::Absent
    };
    PilotMeasure {
        state,
        deviation_hz: dev,
        injection_pct: pct,
    }
}

/// Quantile used for the peak-deviation reading - a quasi-peak detector rather
/// than a raw maximum.
///
/// A single corrupted sample pair produces an enormous phase step, and the
/// discriminator turns that into an excursion pinned near its ±rate/2 ambiguity
/// limit. Taking the absolute maximum therefore reports impulse noise as
/// modulation: on a strong broadcast station it reads ~125 kHz against a 16 kHz
/// RMS, a crest factor no real transmitter produces. Ignoring the top 0.1 % of
/// samples rejects those outliers while still tracking genuine programme peaks -
/// for a sine the 99.9th percentile sits within 0.001 % of the true peak.
const PEAK_QUANTILE: f64 = 0.999;

/// Value at quantile `q` of `data`, found by partial sort - the same O(n)
/// `select_nth_unstable` approach the FFT worker uses for its noise floor.
/// Reorders `data`, which is always caller-owned scratch.
fn quantile(data: &mut [f32], q: f64) -> f32 {
    if data.is_empty() {
        return 0.0;
    }
    let idx = (((data.len() - 1) as f64) * q.clamp(0.0, 1.0)).round() as usize;
    data.select_nth_unstable_by(idx, |a, b| {
        a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
    });
    data[idx]
}

/// Peak / RMS deviation and carrier offset from a discriminator output.
///
/// The carrier offset is the mean instantaneous frequency, and deviation is
/// measured *about that mean* - otherwise a mistuned radio would report its own
/// tuning error as modulation and inflate the deviation figure. The peak is a
/// [`PEAK_QUANTILE`] quasi-peak, found with the same O(n) partial sort the FFT
/// worker uses for its noise floor.
pub fn fm_stats(inst_hz: &[f32]) -> Option<FmMeasure> {
    if inst_hz.is_empty() {
        return None;
    }
    let n = inst_hz.len() as f64;
    let mean = inst_hz.iter().map(|&f| f as f64).sum::<f64>() / n;

    let mut sq = 0.0f64;
    let mut devs: Vec<f32> = Vec::with_capacity(inst_hz.len());
    for &f in inst_hz {
        let d = f as f64 - mean;
        sq += d * d;
        devs.push(d.abs() as f32);
    }

    let peak = quantile(&mut devs, PEAK_QUANTILE);

    Some(FmMeasure {
        peak_dev_hz: peak,
        rms_dev_hz: (sq / n).sqrt() as f32,
        carrier_offset_hz: mean as f32,
    })
}

/// Channel rate targeted for AM. An AM channel is 2 × the audio bandwidth, so
/// ~10 kHz; a 16 kHz channel (filter passband 0.4 × 16 kHz = 6.4 kHz) clears it.
pub const AM_TARGET_HZ: f64 = 16_000.0;

/// The channel rate to target for a modulation, or `None` for a carrier that has
/// not been classified - an unclassified signal deliberately produces no reading
/// rather than a wrong one.
pub fn target_rate_for(modulation: Modulation) -> Option<f64> {
    match modulation {
        Modulation::Wfm => Some(WFM_TARGET_HZ),
        Modulation::Nfm => Some(NFM_TARGET_HZ),
        Modulation::Am => Some(AM_TARGET_HZ),
        Modulation::Unknown => None,
    }
}

/// AM envelope: `|z|` per sample, the amplitude the modulation rides on.
pub fn am_envelope(iq: &[Complex<f32>], out: &mut Vec<f32>) {
    out.clear();
    out.reserve(iq.len());
    out.extend(iq.iter().map(|z| z.norm()));
}

/// Modulation depth and asymmetry from an AM envelope.
///
/// Depth is the classic `(Vmax − Vmin) / (Vmax + Vmin)`. The peaks are taken as
/// quantiles rather than absolute extremes for the same reason the FM peak is -
/// one impulse would otherwise define the reading.
///
/// Positive and negative depths are reported separately because they fail
/// differently: a negative depth reaching 100 % means the carrier is being
/// pinched off, which clips and splatters, while an asymmetric pair points at a
/// modulator fault rather than simply too much level.
pub fn am_stats(env: &[f32]) -> Option<AmMeasure> {
    if env.len() < 8 {
        return None;
    }
    let carrier = env.iter().map(|&v| v as f64).sum::<f64>() / env.len() as f64;
    if carrier <= 0.0 {
        return None;
    }

    let mut scratch: Vec<f32> = env.to_vec();
    let hi = quantile(&mut scratch, PEAK_QUANTILE) as f64;
    let lo = quantile(&mut scratch, 1.0 - PEAK_QUANTILE) as f64;

    let depth = if hi + lo > 0.0 {
        (hi - lo) / (hi + lo)
    } else {
        0.0
    };
    Some(AmMeasure {
        depth_pct: (depth * 100.0) as f32,
        positive_pct: (((hi - carrier) / carrier) * 100.0) as f32,
        negative_pct: (((carrier - lo) / carrier) * 100.0) as f32,
        carrier_dbfs: if carrier > 0.0 {
            20.0 * (carrier as f32).log10()
        } else {
            f32::NEG_INFINITY
        },
    })
}

/// The CTCSS tone table, in Hz: the 38 standard EIA/TIA tones plus 69.3 and
/// 254.1, which virtually all equipment also offers.
pub const CTCSS_TONES: [f64; 40] = [
    67.0, 69.3, 71.9, 74.4, 77.0, 79.7, 82.5, 85.4, 88.5, 91.5, 94.8, 97.4, 100.0, 103.5, 107.2,
    110.9, 114.8, 118.8, 123.0, 127.3, 131.8, 136.5, 141.3, 146.2, 151.4, 156.7, 162.2, 167.9,
    173.8, 179.9, 186.2, 192.8, 203.5, 210.7, 218.1, 225.7, 233.6, 241.8, 250.3, 254.1,
];

/// Seconds of contiguous audio a CTCSS decision needs.
///
/// The closest standard tones are ~2.3 Hz apart (67.0 vs 69.3). Telling them
/// apart needs an observation long enough that one tone's response at its
/// neighbour's detector has fallen away - roughly `1 / Δf`, so ~430 ms minimum.
/// Half a second gives margin. This is why the CTCSS path cannot run off the
/// duty-cycled 33 ms snippets the deviation statistics are happy with.
pub const CTCSS_WINDOW_S: f64 = 0.5;

/// Smallest tone deviation accepted as a real CTCSS tone. Broadcast practice puts
/// CTCSS at 10–15 % of an NFM channel's 5 kHz, so 500–750 Hz; 100 Hz is well
/// under any real encoder while staying clear of incidental low-frequency content.
const CTCSS_MIN_DEV_HZ: f32 = 100.0;

/// How far the winning tone must stand above the best *non-adjacent* candidate
/// before the detection is reported, in dB. Guards against voice energy or hum
/// lighting up several detectors at once.
const CTCSS_MARGIN_DB: f32 = 6.0;

/// Goertzel amplitude estimate for one frequency, in the input's own units.
///
/// A single-bin DFT: far cheaper than a full transform when only a few dozen
/// frequencies matter, and unconstrained by bin spacing - the CTCSS tones do not
/// land on FFT bin centres. Scaled `2·|X| / Σw` so the result reads directly as
/// the tone's amplitude.
pub fn goertzel_amplitude(x: &[f32], window: &[f32], freq: f64, rate: f64) -> f32 {
    let n = x.len().min(window.len());
    if n == 0 || rate <= 0.0 {
        return 0.0;
    }
    let w = 2.0 * std::f64::consts::PI * freq / rate;
    let coeff = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for i in 0..n {
        let s0 = (x[i] * window[i]) as f64 + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let power = (s1 * s1 + s2 * s2 - coeff * s1 * s2).max(0.0);
    let w_sum: f32 = window[..n].iter().sum();
    if w_sum <= 0.0 {
        return 0.0;
    }
    2.0 * power.sqrt() as f32 / w_sum
}

/// Identify the CTCSS tone present in a block of discriminator output, if any.
///
/// Returns the winning tone only when it is both strong enough in absolute terms
/// and clearly ahead of every non-adjacent rival - a tone that merely edges out
/// its neighbour is an unresolved measurement, not a detection.
pub fn ctcss_detect(audio_hz: &[f32], window: &[f32], rate: f64) -> Option<CtcssMeasure> {
    if audio_hz.len() < window.len() || window.is_empty() {
        return None;
    }
    let amps: Vec<f32> = CTCSS_TONES
        .iter()
        .map(|&f| goertzel_amplitude(audio_hz, window, f, rate))
        .collect();

    let (best_i, best) =
        amps.iter().enumerate().fold(
            (0usize, 0.0f32),
            |(bi, bv), (i, &v)| if v > bv { (i, v) } else { (bi, bv) },
        );
    if best < CTCSS_MIN_DEV_HZ {
        return None;
    }

    // Adjacent table entries sit inside each other's skirts, so the runner-up is
    // taken from the rest of the table.
    let rival = amps
        .iter()
        .enumerate()
        .filter(|(i, _)| i.abs_diff(best_i) > 1)
        .fold(0.0f32, |m, (_, &v)| m.max(v));
    let margin_db = if rival > 0.0 {
        20.0 * (best / rival).log10()
    } else {
        f32::INFINITY
    };
    if margin_db < CTCSS_MARGIN_DB {
        return None;
    }

    Some(CtcssMeasure {
        tone_hz: CTCSS_TONES[best_i] as f32,
        deviation_hz: best,
        margin_db,
    })
}

/// A decimating FIR that keeps its state between calls, so successive blocks
/// produce one seamless output stream.
///
/// The stateless [`decimate`] restarts at each block: it discards the first
/// `taps` samples and resets the decimation grid, which puts a small timing step
/// at every block boundary. Deviation statistics never notice, but a narrowband
/// tone detector does - the CTCSS window spans several blocks, and a phase step
/// inside it destroys the coherence the detection depends on.
pub struct StreamingDecimator {
    taps: Vec<f32>,
    d: usize,
    /// Input samples carried over so the next block's first output can see the
    /// full filter history.
    tail: Vec<Complex<f32>>,
    /// Where the decimation grid resumes inside the next block.
    phase: usize,
}

impl StreamingDecimator {
    pub fn new(taps: Vec<f32>, d: usize) -> Self {
        Self {
            taps,
            d: d.max(1),
            tail: Vec::new(),
            phase: 0,
        }
    }

    /// Forget the carried state - after a dropped block, or a parameter change.
    /// The next output block starts a fresh contiguous run.
    pub fn reset(&mut self) {
        self.tail.clear();
        self.phase = 0;
    }

    pub fn process(&mut self, input: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        out.clear();
        let n = self.taps.len();
        if n == 0 || input.is_empty() {
            return;
        }

        // Splice the carried history in front of the new samples.
        let mut buf = std::mem::take(&mut self.tail);
        buf.extend_from_slice(input);
        if buf.len() < n {
            self.tail = buf;
            return;
        }

        let mut start = self.phase;
        while start + n <= buf.len() {
            let w = &buf[start..start + n];
            let mut acc = Complex {
                re: 0.0f32,
                im: 0.0f32,
            };
            for (s, &h) in w.iter().zip(self.taps.iter()) {
                acc.re += s.re * h;
                acc.im += s.im * h;
            }
            out.push(acc);
            start += self.d;
        }

        // Keep the samples the next output still needs, and remember where the
        // grid stands relative to them. When the stride overshoots the buffer
        // entirely, the leftover stride carries into the next block as phase.
        let consumed = start.min(buf.len());
        buf.drain(..consumed);
        self.phase = start - consumed;
        self.tail = buf;
    }
}

/// The demod thread. Mirrors [`crate::signal::FftWorker`]: owns its scratch
/// buffers, consumes raw blocks, and writes finished measurements into the shared
/// metrics.
mod worker;

pub use worker::DemodWorker;

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    /// Synthesise a complex FM tone: carrier at `offset_hz`, sinusoidally
    /// modulated at `mod_hz` with peak deviation `dev_hz`.
    fn fm_signal(
        rate: f64,
        n: usize,
        offset_hz: f64,
        dev_hz: f64,
        mod_hz: f64,
    ) -> Vec<Complex<f32>> {
        let mut phase = 0.0f64;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let t = i as f64 / rate;
            let inst = offset_hz + dev_hz * (2.0 * PI * mod_hz * t).sin();
            phase += 2.0 * PI * inst / rate;
            out.push(Complex {
                re: phase.cos() as f32,
                im: phase.sin() as f32,
            });
        }
        out
    }

    #[test]
    fn decimation_factor_rounds_down_and_floors_at_one() {
        assert_eq!(decimation_factor(2_000_000.0, 250_000.0), 8);
        // 9.6 rounds *down* to 9 - a wider channel than asked for, never narrower.
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
        for sr in [
            2_000_000.0,
            2_400_000.0,
            3_200_000.0,
            10_000_000.0,
            20_000_000.0,
        ] {
            let d = decimation_factor(sr, WFM_TARGET_HZ);
            let passband = 0.4 * channel_rate(sr, d);
            assert!(
                passband >= 128_000.0,
                "sr={sr}: passband {passband} Hz is inside Carson bandwidth"
            );
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
        assert!(
            eval(0.20) < 0.01,
            "stopband gain {} should be < -40 dB",
            eval(0.20)
        );
    }

    #[test]
    fn discriminator_recovers_a_constant_offset() {
        let rate = 250_000.0;
        let iq = fm_signal(rate, 4096, 10_000.0, 0.0, 0.0);
        let mut inst = Vec::new();
        fm_discriminate(&iq, rate, &mut inst);
        let s = fm_stats(&inst).expect("stats");
        assert!(
            (s.carrier_offset_hz - 10_000.0).abs() < 50.0,
            "offset = {}",
            s.carrier_offset_hz
        );
        // An unmodulated carrier has no deviation.
        assert!(s.peak_dev_hz < 50.0, "peak = {}", s.peak_dev_hz);
    }

    #[test]
    fn discriminator_measures_known_deviation() {
        let rate = 250_000.0;
        // 40 kHz peak deviation, 1 kHz tone - a textbook WFM test signal.
        let iq = fm_signal(rate, 1 << 15, 0.0, 40_000.0, 1_000.0);
        let mut inst = Vec::new();
        fm_discriminate(&iq, rate, &mut inst);
        let s = fm_stats(&inst).expect("stats");
        assert!(
            (s.peak_dev_hz - 40_000.0).abs() < 500.0,
            "peak = {}",
            s.peak_dev_hz
        );
        // A sine's RMS is its peak / √2.
        let expect_rms = 40_000.0 / 2f32.sqrt();
        assert!(
            (s.rms_dev_hz - expect_rms).abs() < 500.0,
            "rms = {}",
            s.rms_dev_hz
        );
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
        assert!(
            (s.carrier_offset_hz - 30_000.0).abs() < 500.0,
            "offset = {}",
            s.carrier_offset_hz
        );
        assert!(
            (s.peak_dev_hz - 20_000.0).abs() < 500.0,
            "peak = {}",
            s.peak_dev_hz
        );
    }

    #[test]
    fn decimate_keeps_a_tone_and_lowers_the_rate() {
        // A 5 kHz tone at 2 Msps, decimated by 8 → 250 kHz, is still 5 kHz.
        let rate = 2_000_000.0;
        let d = 8;
        let iq = fm_signal(rate, 1 << 16, 5_000.0, 0.0, 0.0);
        let mut sd = StreamingDecimator::new(design_lowpass(tap_count(d), 0.4 / d as f64), d);
        let mut dec = Vec::new();
        sd.process(&iq, &mut dec);
        assert!(
            dec.len() > 1000,
            "expected a decimated block, got {}",
            dec.len()
        );
        let mut inst = Vec::new();
        fm_discriminate(&dec, channel_rate(rate, d), &mut inst);
        let s = fm_stats(&inst).expect("stats");
        assert!(
            (s.carrier_offset_hz - 5_000.0).abs() < 100.0,
            "offset = {}",
            s.carrier_offset_hz
        );
    }

    #[test]
    fn decimate_is_a_noop_when_input_is_shorter_than_the_filter() {
        let mut sd = StreamingDecimator::new(design_lowpass(63, 0.1), 4);
        let input = vec![
            Complex {
                re: 1.0f32,
                im: 0.0
            };
            10
        ];
        let mut out = Vec::new();
        sd.process(&input, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn streaming_decimator_matches_one_long_block() {
        // The property CTCSS depends on: feeding a signal in pieces must give the
        // same output as feeding it whole - same samples, same count, no timing
        // step at the seams.
        let rate = 2_000_000.0;
        let d = 8;
        let iq = fm_signal(rate, 1 << 15, 5_000.0, 0.0, 0.0);
        let taps = design_lowpass(tap_count(d), 0.4 / d as f64);

        let mut whole = Vec::new();
        StreamingDecimator::new(taps.clone(), d).process(&iq, &mut whole);

        let mut sd = StreamingDecimator::new(taps, d);
        let mut pieced = Vec::new();
        let mut part = Vec::new();
        // Deliberately ragged chunks, none a multiple of the decimation factor.
        for chunk in iq.chunks(3_001) {
            sd.process(chunk, &mut part);
            pieced.extend_from_slice(&part);
        }

        assert_eq!(
            pieced.len(),
            whole.len(),
            "sample count diverged across blocks"
        );
        for (i, (a, b)) in pieced.iter().zip(whole.iter()).enumerate() {
            assert!((a - b).norm() < 1e-3, "sample {i} differs: {a} vs {b}");
        }
    }

    #[test]
    fn streaming_decimator_reset_starts_a_fresh_run() {
        let d = 4;
        let taps = design_lowpass(31, 0.1);
        let iq = fm_signal(200_000.0, 4096, 1_000.0, 0.0, 0.0);
        let mut sd = StreamingDecimator::new(taps, d);
        let (mut a, mut b) = (Vec::new(), Vec::new());
        sd.process(&iq, &mut a);
        sd.reset();
        sd.process(&iq, &mut b);
        // After a reset the filter has no history, so it must re-warm exactly as
        // it did the first time rather than splice onto stale samples.
        assert_eq!(a.len(), b.len());
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
        // discriminator's ambiguity rail - exactly what a few bad sample pairs
        // look like. The reading must follow the signal, not the impulses.
        let rate = 250_000.0;
        let iq = fm_signal(rate, 1 << 14, 0.0, 20_000.0, 1_000.0);
        let mut inst = Vec::new();
        fm_discriminate(&iq, rate, &mut inst);
        for i in [10usize, 500, 4000, 9001] {
            inst[i] = 125_000.0;
        }
        let s = fm_stats(&inst).expect("stats");
        assert!(
            s.peak_dev_hz < 22_000.0,
            "impulses must not be reported as deviation, got {}",
            s.peak_dev_hz
        );
        assert!(
            s.peak_dev_hz > 18_000.0,
            "genuine peak still tracked, got {}",
            s.peak_dev_hz
        );
    }

    #[test]
    fn envelope_nulls_are_gated_out() {
        let rate = 250_000.0;
        let mut iq = fm_signal(rate, 4096, 0.0, 20_000.0, 1_000.0);
        // Collapse a run of samples to near zero, as a beat null or noise null
        // does. Their phase is meaningless and must not reach the statistics.
        for z in iq.iter_mut().skip(1000).take(50) {
            *z = Complex {
                re: 1e-4,
                im: -1e-4,
            };
        }
        let mut inst = Vec::new();
        fm_discriminate(&iq, rate, &mut inst);
        // The time base must stay uniform for the MPX spectrum, so gated samples
        // are held at the last trustworthy value rather than removed.
        assert_eq!(inst.len(), 4095, "gating must not disturb the sample grid");
        // Nothing survives near the ±rate/2 ambiguity rail.
        let worst = inst.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
        assert!(
            worst < 100_000.0,
            "rail-level sample survived the gate: {worst}"
        );
        // The held region carries a plausible deviation, not a fabricated zero.
        assert!(
            inst[1020].abs() <= 20_000.0 + 1.0,
            "held value = {}",
            inst[1020]
        );
    }

    #[test]
    fn a_leading_gated_run_is_backfilled() {
        // If the block opens inside a null there is no previous value to hold, so
        // the first valid sample is written backwards over the gap - otherwise the
        // block would start with a fabricated zero and put a step in the spectrum.
        let rate = 250_000.0;
        let mut iq = fm_signal(rate, 2048, 15_000.0, 0.0, 0.0);
        for z in iq.iter_mut().take(40) {
            *z = Complex { re: 1e-4, im: 1e-4 };
        }
        let mut inst = Vec::new();
        fm_discriminate(&iq, rate, &mut inst);
        assert_eq!(inst.len(), 2047);
        assert!(
            (inst[0] - 15_000.0).abs() < 200.0,
            "leading gap must be backfilled, got {}",
            inst[0]
        );
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
        // Over-deviation is a real fault the panel must be able to show - the
        // outlier rejection must not flatten a genuinely hot signal.
        let rate = 250_000.0;
        let iq = fm_signal(rate, 1 << 15, 0.0, 90_000.0, 1_000.0);
        let mut inst = Vec::new();
        fm_discriminate(&iq, rate, &mut inst);
        let s = fm_stats(&inst).expect("stats");
        assert!(
            s.peak_dev_hz > 85_000.0,
            "expected ~90 kHz, got {}",
            s.peak_dev_hz
        );
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
            out.push(Complex {
                re: phase.cos() as f32,
                im: phase.sin() as f32,
            });
        }
        out
    }

    fn mpx_of(inst: &[f32], rate: f64) -> (Vec<f32>, f64) {
        let fft = rustfft::FftPlanner::<f32>::new().plan_fft_forward(MPX_FFT_SIZE);
        let window =
            super::super::dsp::compute_window(super::super::dsp::WindowFn::Hann, MPX_FFT_SIZE);
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
        assert!(
            s.carrier_offset_hz.abs() < 200.0,
            "residual offset {}",
            s.carrier_offset_hz
        );
    }

    #[test]
    fn mix_offset_is_a_noop_at_zero_and_keeps_amplitude() {
        let rate = 2_000_000.0;
        let original = fm_signal(rate, 256, 50_000.0, 0.0, 0.0);
        let mut untouched = original.clone();
        mix_offset(&mut untouched, 0.0, rate);
        assert_eq!(
            untouched[10], original[10],
            "zero offset must not touch the samples"
        );

        // Mixing is a rotation: it must not change the envelope, or it would move
        // samples across the gate threshold.
        let mut mixed = original.clone();
        mix_offset(&mut mixed, 250_000.0, rate);
        for i in [0usize, 100, 255] {
            assert!(
                (mixed[i].norm() - original[i].norm()).abs() < 1e-3,
                "envelope changed at {i}"
            );
        }
    }

    #[test]
    fn mpx_spectrum_recovers_pilot_deviation() {
        // 30 kHz audio + a 7.5 kHz pilot - 10 % injection against the 75 kHz limit.
        let rate = 333_000.0;
        let iq = wfm_signal(rate, MPX_FFT_SIZE * 2, 30_000.0, 7_500.0);
        let mut inst = Vec::new();
        fm_discriminate(&iq, rate, &mut inst);
        let (mags, bin_hz) = mpx_of(&inst, rate);
        assert!(!mags.is_empty());

        let p = pilot_measure(&mags, bin_hz, 75_000.0);
        assert_eq!(p.state, PilotState::Locked);
        // Amplitude scaling must return real Hz of deviation, not arbitrary units.
        assert!(
            (p.deviation_hz - 7_500.0).abs() < 800.0,
            "pilot dev = {}",
            p.deviation_hz
        );
        assert!(
            (p.injection_pct - 10.0).abs() < 1.5,
            "injection = {}",
            p.injection_pct
        );
    }

    #[test]
    fn mono_signal_reports_no_pilot() {
        let rate = 333_000.0;
        let iq = wfm_signal(rate, MPX_FFT_SIZE * 2, 30_000.0, 0.0);
        let mut inst = Vec::new();
        fm_discriminate(&iq, rate, &mut inst);
        let (mags, bin_hz) = mpx_of(&inst, rate);
        let p = pilot_measure(&mags, bin_hz, 75_000.0);
        assert_eq!(
            p.state,
            PilotState::Absent,
            "injection = {}",
            p.injection_pct
        );
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
        assert_eq!(
            p.state,
            PilotState::Marginal,
            "injection = {}",
            p.injection_pct
        );
    }

    #[test]
    fn mpx_spectrum_declines_a_short_block() {
        // Fewer samples than the transform needs yields nothing, never a
        // zero-padded spectrum that would read as real signal.
        let fft = rustfft::FftPlanner::<f32>::new().plan_fft_forward(MPX_FFT_SIZE);
        let window =
            super::super::dsp::compute_window(super::super::dsp::WindowFn::Hann, MPX_FFT_SIZE);
        let (mut scratch, mut mags) = (Vec::new(), Vec::new());
        mpx_spectrum(&[0.0; 100], &window, fft.as_ref(), &mut scratch, &mut mags);
        assert!(mags.is_empty());
    }

    #[test]
    fn pilot_measure_refuses_degenerate_input() {
        assert_eq!(
            pilot_measure(&[], 163.0, 75_000.0).state,
            PilotState::Absent
        );
        assert_eq!(
            pilot_measure(&[1.0, 2.0], 0.0, 75_000.0).state,
            PilotState::Absent
        );
        assert_eq!(
            pilot_measure(&[1.0, 2.0], 163.0, 0.0).state,
            PilotState::Absent
        );
    }

    #[test]
    fn target_rate_per_modulation() {
        assert_eq!(target_rate_for(Modulation::Wfm), Some(WFM_TARGET_HZ));
        assert_eq!(target_rate_for(Modulation::Nfm), Some(NFM_TARGET_HZ));
        assert_eq!(target_rate_for(Modulation::Am), Some(AM_TARGET_HZ));
        // An unclassified carrier must produce no reading at all.
        assert_eq!(target_rate_for(Modulation::Unknown), None);
    }

    /// AM carrier of unit amplitude, modulated `depth` (0..1) by a 1 kHz tone.
    fn am_signal(rate: f64, n: usize, depth: f64) -> Vec<Complex<f32>> {
        (0..n)
            .map(|i| {
                let t = i as f64 / rate;
                let a = 1.0 + depth * (2.0 * PI * 1_000.0 * t).sin();
                Complex {
                    re: a as f32,
                    im: 0.0,
                }
            })
            .collect()
    }

    #[test]
    fn am_depth_matches_a_known_modulation() {
        let iq = am_signal(16_000.0, 1 << 13, 0.5);
        let mut env = Vec::new();
        am_envelope(&iq, &mut env);
        let s = am_stats(&env).expect("stats");
        assert!((s.depth_pct - 50.0).abs() < 2.0, "depth = {}", s.depth_pct);
        // A symmetric modulator swings equally either side of the carrier.
        assert!(
            (s.positive_pct - s.negative_pct).abs() < 3.0,
            "asymmetry {} vs {}",
            s.positive_pct,
            s.negative_pct
        );
    }

    #[test]
    fn am_unmodulated_carrier_reads_zero_depth() {
        let iq = am_signal(16_000.0, 1 << 13, 0.0);
        let mut env = Vec::new();
        am_envelope(&iq, &mut env);
        let s = am_stats(&env).expect("stats");
        assert!(s.depth_pct < 1.0, "depth = {}", s.depth_pct);
    }

    #[test]
    fn am_stats_declines_a_tiny_or_dead_block() {
        assert!(am_stats(&[1.0, 1.0]).is_none());
        // A dead carrier has no depth to speak of, and dividing by it would be
        // worse than saying nothing.
        assert!(am_stats(&[0.0; 64]).is_none());
    }

    /// Discriminator-domain audio: a CTCSS tone plus a louder voice-band tone,
    /// as a real NFM channel carries it.
    fn ctcss_audio(rate: f64, n: usize, tone_hz: f64, tone_dev: f64, voice_dev: f64) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let t = i as f64 / rate;
                (tone_dev * (2.0 * PI * tone_hz * t).sin()
                    + voice_dev * (2.0 * PI * 900.0 * t).sin()) as f32
            })
            .collect()
    }

    #[test]
    fn ctcss_identifies_the_right_tone_under_voice() {
        let rate = 25_000.0;
        let n = (CTCSS_WINDOW_S * rate) as usize;
        let window = super::super::dsp::compute_window(super::super::dsp::WindowFn::Hann, n);
        // 103.5 Hz at 600 Hz deviation, under 3 kHz of voice - typical proportions.
        let audio = ctcss_audio(rate, n, 103.5, 600.0, 3_000.0);
        let m = ctcss_detect(&audio, &window, rate).expect("tone detected");
        assert_eq!(m.tone_hz, 103.5);
        assert!(
            (m.deviation_hz - 600.0).abs() < 100.0,
            "dev = {}",
            m.deviation_hz
        );
    }

    #[test]
    fn ctcss_separates_the_closest_tone_pair() {
        // 67.0 and 69.3 are only 2.3 Hz apart - the reason the window is half a
        // second. Each must be identified as itself, not as its neighbour.
        let rate = 25_000.0;
        let n = (CTCSS_WINDOW_S * rate) as usize;
        let window = super::super::dsp::compute_window(super::super::dsp::WindowFn::Hann, n);
        for want in [67.0f32, 69.3] {
            let audio = ctcss_audio(rate, n, want as f64, 600.0, 1_000.0);
            let m = ctcss_detect(&audio, &window, rate)
                .unwrap_or_else(|| panic!("no detection for {want}"));
            assert_eq!(m.tone_hz, want, "confused {want} with {}", m.tone_hz);
        }
    }

    #[test]
    fn ctcss_reports_nothing_without_a_tone() {
        let rate = 25_000.0;
        let n = (CTCSS_WINDOW_S * rate) as usize;
        let window = super::super::dsp::compute_window(super::super::dsp::WindowFn::Hann, n);
        // Voice only - a carrier with no subaudible tone must not invent one.
        let audio = ctcss_audio(rate, n, 100.0, 0.0, 3_000.0);
        assert!(ctcss_detect(&audio, &window, rate).is_none());
    }

    #[test]
    fn ctcss_declines_a_short_run() {
        // Fewer samples than the window means the run is not yet long enough to
        // decide - the caller must show "searching", not "no tone".
        let rate = 25_000.0;
        let n = (CTCSS_WINDOW_S * rate) as usize;
        let window = super::super::dsp::compute_window(super::super::dsp::WindowFn::Hann, n);
        let audio = ctcss_audio(rate, n / 4, 103.5, 600.0, 500.0);
        assert!(ctcss_detect(&audio, &window, rate).is_none());
    }

    #[test]
    fn goertzel_measures_a_tone_amplitude() {
        let rate = 25_000.0;
        let n = 8192;
        let window = super::super::dsp::compute_window(super::super::dsp::WindowFn::Hann, n);
        let x: Vec<f32> = (0..n)
            .map(|i| (700.0 * (2.0 * PI * 150.0 * i as f64 / rate).sin()) as f32)
            .collect();
        let a = goertzel_amplitude(&x, &window, 150.0, rate);
        assert!((a - 700.0).abs() < 20.0, "amplitude = {a}");
        // Far off the tone the detector must read near zero.
        let off = goertzel_amplitude(&x, &window, 500.0, rate);
        assert!(off < 20.0, "off-tone leakage = {off}");
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
