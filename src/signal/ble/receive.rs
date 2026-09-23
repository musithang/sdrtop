// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The live pipeline: raw device bytes to a decoded advertising channel PDU.
//!
//! Four stages, run in order over every sample: [`Detector`] (a matched
//! filter over raw IQ) finds the sync word; once it does, the samples that
//! follow are captured; [`discriminate`] turns the capture into instantaneous
//! frequency; [`super::sync::slice`] finds the symbol phase and slices it to
//! bits; and [`pdu::decode`], fed those bits de-whitened, either returns a
//! packet or says there was not enough of one yet.
//!
//! **Started as a deterministic peak-derived index, not a search - and real
//! hardware is why it no longer is one.** `signal::ble::detect`'s own tests
//! measured the matched filter's peak landing at the exact sample a
//! synthetic sync word's last symbol ends on, so indexing directly from it
//! looked like a known position rather than an estimate. True only because
//! that synthetic transmitter and the detector's own reference come from
//! the same call to [`super::gfsk::modulate`] at the same sample phase - a
//! real transmitter's clock has no reason to share it. `[super::sync::slice]`
//! is B4's own answer for a burst whose alignment is not known this
//! precisely, which turned out to be every real one.

use num_complex::Complex;

use crate::hardware::SampleGeometry;
use crate::signal::demod::decode as decode_iq;
use crate::signal::dsp::code::lfsr::whiten;
use crate::signal::dsp::correlate::ShapeMatcher;
use crate::signal::dsp::discriminate::{discriminate, instantaneous_freq_hz};
use crate::signal::dsp::estimate::snr_from_metric;
use crate::signal::dsp::fir::{design_lowpass_to_spec, StreamingDecimator};

use super::detect::{access_address_bits, preamble_bits, ADVERTISING_ACCESS_ADDRESS};
use super::gfsk;
use super::pdu::{self, Packet};
use super::Phy;

/// Samples per symbol this arc demodulates at. Not a specification
/// requirement - a PHY's own symbol rate ([`Phy::symbol_rate_hz`]) is fixed,
/// and this is comfortably enough resolution for the matched filter and the
/// discriminator slice both, without decimating a wide capture further than
/// it has to. The same for both PHYs B17 supports: `LOOKBACK_SAMPLES`,
/// `HEADER_SEARCH_SYMBOLS` and every other constant counted in symbols below
/// stays correct unchanged when the PHY changes, because it is this figure -
/// not the PHY's own absolute rate - that fixes the conversion between the
/// two units.
const WORKING_SPS: usize = 4;

/// The actual working rate a receiver on `phy` runs at, once decimated -
/// B6 through B16's own fixed `WORKING_RATE_HZ` constant, now one number
/// per PHY rather than one for the whole file, since [`Phy::TwoM`] transmits
/// at twice [`Phy::OneM`]'s own symbol rate (design section 1.2's own "the
/// same chain at twice the symbol rate").
fn working_rate_hz(phy: Phy) -> f64 {
    phy.symbol_rate_hz() * WORKING_SPS as f64
}

/// The longest a legacy advertising PDU can be: 2-byte header, up to 37
/// bytes of payload, 3-byte CRC.
const MAX_PDU_BYTES: usize = 2 + 37 + 3;

/// How many symbols of context [`Receiver`] keeps *before* a trigger fires -
/// see `Receiver::history` and `Receiver::try_decode`'s own docs for why the
/// true header boundary can land on either side of the trigger sample
/// itself, not only after it.
const LOOKBACK_SYMBOLS: usize = 8;
const LOOKBACK_SAMPLES: usize = LOOKBACK_SYMBOLS * WORKING_SPS;

/// How many symbols either side of the nominal boundary
/// (`Receiver::history`'s own length) `Receiver::try_decode` searches for a
/// clean CRC. Generous relative to the few symbols of smearing `front_end`'s
/// anti-alias filter measurably costs at the sync/header boundary - see
/// `front_end`'s own doc - with room to spare rather than tuned to the exact
/// worst case measured so far.
const HEADER_SEARCH_SYMBOLS: usize = 6;

/// Silence, in symbols, that ends a packet: well past a Gaussian pulse's own
/// tail (about a symbol) and short of the 150 us gap before a reply on the
/// same channel, so the end found is this packet's, not the next one's.
const END_GAP_SYMBOLS: usize = 8;

/// How far a candidate's own end may sit from the measured one and still be
/// the packet that was there: a symbol of pulse tail either side, and a
/// symbol or two of where the silence detector calls the drop.
const END_TOLERANCE_SYMBOLS: usize = 4;

/// Where the signal in `capture[from..]` stops: the start of the first run of
/// [`END_GAP_SYMBOLS`] symbols whose power is nearer the capture's noise than
/// its signal. `None` when there is no clear signal to have stopped (less than
/// 6 dB between the two) or it never stops inside the capture.
///
/// The levels come from the capture itself: the signal from the first forty
/// symbols after `from` (the header and address, which every PDU has), the
/// noise from the quietest tenth of all its symbols. The threshold between
/// them is their geometric mean.
fn energy_end(capture: &[Complex<f32>], from: usize) -> Option<usize> {
    let sps = WORKING_SPS;
    let symbols: Vec<f32> = capture
        .chunks(sps)
        .map(|c| c.iter().map(|s| s.norm_sqr()).sum::<f32>() / c.len() as f32)
        .collect();
    let start = from / sps;
    let lead = symbols.get(start..start + 40)?;
    let median = |v: &[f32]| {
        let mut v = v.to_vec();
        v.sort_by(f32::total_cmp);
        v[v.len() / 2]
    };
    let signal = median(lead);
    let mut all = symbols.clone();
    all.sort_by(f32::total_cmp);
    let noise = all[all.len() / 10];
    // A NaN level is no contrast either.
    if signal.is_nan() || signal <= 4.0 * noise {
        return None;
    }
    let threshold = (signal * noise).sqrt();
    let mut run = 0;
    for (i, &p) in symbols.iter().enumerate().skip(start) {
        if p < threshold {
            run += 1;
            if run == END_GAP_SYMBOLS {
                return Some((i + 1 - run) * sps);
            }
        } else {
            run = 0;
        }
    }
    None
}

/// How often a capture in progress is tried for a finished packet: once an
/// octet's worth of samples, not once a sample.
///
/// A PDU grows an octet at a time, so trying between octets can only find a
/// packet a few microseconds sooner than trying at them. Trying every sample
/// did find it sooner - and paid for it with a full search of
/// [`HEADER_SEARCH_SYMBOLS`] candidate boundaries, each a discriminator, a
/// phase search and a decode over the whole capture, on every one of the
/// ~1300 samples a capture runs to: tens of thousands of decodes per trigger.
/// On a busy real channel that put the receiver at around 280 times real
/// time (`dev_docs/case-study-ble-crc.md`, section 11).
const DECODE_EVERY_SAMPLES: usize = 8 * WORKING_SPS;

/// The sync word's detector statistic, Pearson's correlation between the
/// discriminator's frequency track and the ideal one (`ShapeMatcher`), has
/// under noise alone a distribution close to normal about zero with variance
/// `1 / N_eff`. `N_eff` is this PHY's, measured: complex Gaussian noise
/// through this module's own [`front_end`] and discriminator, correlated
/// against the PHY's own frequency template - 107.8 for LE 1M (the same at 4,
/// 8 and 20 Msps raw, since the front end fixes what reaches the working
/// rate) and 133.1 for LE 2M. The normal approximation was measured to hold
/// out to 4.5 standard deviations (exceedances within 30 % of prediction);
/// `the_shape_detector_s_noise_statistics_are_the_ones_measured` re-measures
/// both, so a change to the front end that moves them fails a test rather
/// than silently moving the threshold's meaning.
fn shape_n_eff(phy: Phy) -> f64 {
    match phy {
        Phy::OneM => 107.8,
        Phy::TwoM => 133.1,
    }
}

/// Standard deviations above zero the statistic must reach to trigger: the
/// one-sided normal tail at `1e-9` a sample, about one false trigger every
/// four minutes of pure noise at 4 Msps.
///
/// **Replaces a coherent detector whose threshold had to be forced.** The
/// matched filter this detector took over from correlated coherently across
/// the whole 40-microsecond sync word, and its false-alarm rate had been
/// pushed to `1e-30` after real triggers were measured at coherences of 0.12
/// to 0.17 - which was a transmitter's carrier offset turning the phase
/// across the window, not interference. Here an offset is a constant the
/// statistic removes, and the threshold means what it says.
const SHAPE_Z: f64 = 6.0;

/// The detector threshold for `phy`: [`SHAPE_Z`] standard deviations of its
/// noise statistic. About 0.58 for LE 1M; on a recording of real traffic
/// every CRC-clean packet an independent receiver found peaked at 0.68 or
/// more (`dev_docs/case-study-ble-crc.md`, section 13).
fn shape_threshold(phy: Phy) -> f64 {
    SHAPE_Z / shape_n_eff(phy).sqrt()
}

/// The anti-alias filter's passband edge, in Hz: comfortably beyond this
/// PHY's own occupied bandwidth (`Phy::deviation_hz`'s own peak deviation on
/// a BT=0.5 Gaussian-shaped symbol rate) so the matched filter's own
/// waveform correlation is not the thing narrowed, and comfortably inside
/// the working Nyquist ([`working_rate_hz`] / 2) so there is real stopband
/// left before that boundary. See `front_end`'s own doc for why LE 1M's
/// figure, not a narrower one, is the one that was tried first; LE 2M's own
/// is the identical reasoning at twice the numbers - not separately
/// measured against real hardware, the same honest gap the whole of B17 has
/// (this arc's own doc).
fn anti_alias_cutoff_hz(phy: Phy) -> f64 {
    match phy {
        Phy::OneM => 1_500_000.0,
        Phy::TwoM => 3_000_000.0,
    }
}
/// Transition width, in Hz, either side of the cutoff.
fn anti_alias_transition_hz(phy: Phy) -> f64 {
    match phy {
        Phy::OneM => 500_000.0,
        Phy::TwoM => 1_000_000.0,
    }
}
/// Stopband attenuation the transition band settles to. Chosen for real
/// rejection of what a wideband capture actually carries - neighbouring BLE
/// channels, Wi-Fi - without the tap count a much deeper stopband would cost
/// for no measured benefit here. The same figure for both PHYs: it is a
/// property of how much rejection is worth paying for, not of the signal's
/// own bandwidth.
const ANTI_ALIAS_STOPBAND_DB: f64 = 40.0;

/// Build the decimator from `raw_rate` to [`working_rate_hz`], or say why it
/// cannot be built.
///
/// Refuses rather than approximating when `raw_rate` is narrower than the
/// working rate, or is not close to a whole multiple of it: a decode running
/// against a rate it silently disagreed with about would scale every
/// deviation and timing figure downstream by exactly the mismatch, with
/// nothing on screen to say so - the same reasoning N15's survey refusal
/// follows for a span too narrow to plan a sweep across.
///
/// **Now carries an anti-alias filter; it did not for B6 through B9, and a
/// real-hardware session is why it does now.** Every step through B9 shipped
/// with plain decimation - keep every `d`th sample, filter nothing - because
/// a first attempt at a Kaiser lowpass here, sized narrow ("well inside the
/// working Nyquist"), collapsed this arc's own synthetic matched-filter
/// coherence from about 0.99 to under 0.15, for a cause that attempt did not
/// run to ground, and the plain-decimation version was what shipped instead.
/// A real HackRF session locked continuously on channel 37 at 20 Msps, after
/// B9, found the failure that leaving this out was always a risk for rather
/// than a known-broken one: a flood of detector triggers, several a second,
/// every one failing CRC, with no two decoding to consistent-looking fields -
/// the signature of the detector matching noise and out-of-channel energy
/// aliased back into the working band, not of a timing or CFO error on real
/// packets (B6 and B7's own real-hardware notes describe a *different*
/// symptom: plausible, repeatable fields with a consistent CRC failure,
/// which is what a small timing or CFO error looks like). The two symptoms
/// are different enough to be different bugs, so this was pursued as a
/// second, separate fix, not a retry of B6's.
///
/// **The cause the first attempt did not run to ground, found this session
/// by measuring rather than guessing again.** A second attempt at a filter
/// here - the same shape, a cutoff reasoned to be wide enough this time -
/// reproduced the first attempt's failure exactly, at a size that should not
/// have: even a bare 19-tap filter with its cutoff at 0.375 cycles/sample,
/// on an unchanged (undecimated) working-rate signal, collapsed a clean
/// packet's peak coherence from 0.9998 to 0.22 against this receiver's own
/// threshold of 0.35. That ruled out "too narrow a cutoff" as the
/// explanation for either attempt. What a side-by-side measurement found
/// instead: [`super::detect::Detector`]'s reference comes straight from
/// `gfsk::modulate`, unfiltered, and correlating a *filtered* signal against
/// an *unfiltered* reference is what collapses coherence - a matched
/// filter's coherence is an inner product, far less forgiving of a shape
/// mismatch between its two sides than an ordinary demodulator is, and it does
/// not matter how generous the filter's own passband is if only one side of
/// the correlation goes through it. Filtering the reference through the
/// identical pipeline restored the same clean packet's coherence to
/// 0.99999997. [`matched_reference`] is that fix: not a differently-sized
/// filter, a differently-built reference.
///
/// **What is left honestly open.** [`anti_alias_cutoff_hz`] itself is still
/// reasoned rather than swept - wide enough to pass a PHY's own waveform
/// comfortably (this fix's own tests confirm that for LE 1M) and narrow
/// enough to give the aliasing case real stopband before [`working_rate_hz`]'s
/// Nyquist, but no measurement here says it is the *right* number, only a
/// defensible one. Verified against this arc's synthetic coherence tests
/// (unchanged pass/fail, same peak positions) and against a new one this fix
/// added - `a_strong_out_of_channel_interferer_no_longer_defeats_detection` -
/// that puts a second, unrelated GFSK signal at a raw offset chosen to fold
/// straight onto this receiver's own passband under decimation, and checks
/// detection survives it. **Not yet re-verified against real hardware** -
/// that is the next real-hardware session's job, the same honest gap B6
/// through B9 each left behind them, and B17's own LE 2M numbers besides.
pub fn front_end(raw_rate: f64, phy: Phy) -> Result<StreamingDecimator, String> {
    let rate_hz = working_rate_hz(phy);
    if raw_rate < rate_hz {
        return Err(format!(
            "BLE decode needs at least {:.1} Msps; the radio is at {:.3} Msps",
            rate_hz / 1e6,
            raw_rate / 1e6
        ));
    }
    let d = (raw_rate / rate_hz).round().max(1.0) as usize;
    let achieved = raw_rate / d as f64;
    if (achieved - rate_hz).abs() > rate_hz * 0.01 {
        return Err(format!(
            "BLE decode needs a sample rate near a whole multiple of {:.1} Msps; {:.3} Msps is not one",
            rate_hz / 1e6,
            raw_rate / 1e6
        ));
    }
    let taps = design_lowpass_to_spec(
        anti_alias_cutoff_hz(phy) / raw_rate,
        anti_alias_transition_hz(phy) / raw_rate,
        ANTI_ALIAS_STOPBAND_DB,
    );
    Ok(StreamingDecimator::new(taps, d))
}

/// The sync-word reference to correlate against, built the way it will
/// actually be received rather than the way [`super::detect::Detector`]
/// builds its own.
///
/// **Why this cannot reuse `Detector`.** A matched filter's coherence is an
/// inner product, not an energy measurement, and it is far less forgiving of
/// a shape mismatch between the two sides than an ordinary demodulator is:
/// measured directly while diagnosing the false-alarm flood `front_end`'s own
/// doc describes, correlating this receiver's now-filtered signal against
/// `Detector`'s unfiltered reference collapsed a clean packet's peak
/// coherence from 0.9998 to 0.22 - comfortably under this receiver's own
/// threshold - while filtering the reference through the identical pipeline
/// restored it to 0.99999996. `Detector` stays exactly as it was for
/// `signal::ble::detect`'s own tests, which deliberately test the detection
/// algorithm with no front end in the picture; this is the front end's own
/// reference, filtered exactly as the signal it correlates against will be.
///
/// Generated at the raw rate and put through [`front_end`]'s own filter and
/// decimation - not a separately-tuned equivalent at [`working_rate_hz`] -
/// so the two literally cannot drift out of step with each other the way a
/// hand-matched pair of filter designs eventually would.
///
/// **Built with margin on both sides, and trimmed back afterwards - not
/// modulated as a bare 40 symbols.** The first version did exactly that, and
/// measured two whole symbols of garbage ahead of every real header: a
/// finite-length signal has nothing before its own first sample or after its
/// last, so both `gfsk::modulate`'s own Gaussian filter and this front end's
/// anti-alias filter taper their two ends toward that edge rather than
/// toward what a real, continuing transmission would actually put there. On
/// air the sync word is never alone - something real precedes and follows
/// it - so [`margin_bits`] stands in for that, and only the middle, once both
/// filters have had real context to settle against, is kept.
///
/// **Where to cut is exact, not swept for.** [`front_end`]'s decimator starts
/// every fresh instance at raw sample 0 with its grid phase at 0, so decimated
/// output index `k` is always the window starting at raw sample `k * d` -
/// meaning [`MARGIN_SYMBOLS`] worth of *decimated* samples is exactly
/// `MARGIN_SYMBOLS * WORKING_SPS`, independent of `raw_rate`, `d`, or the
/// filter's own tap count: decimation preserves symbol timing exactly
/// whenever `d` divides the raw samples per symbol evenly, which every
/// `raw_rate` [`front_end`] accepts does by construction (`clean_multiples_
/// of_the_working_rate_are_accepted`already holds it to that).
///
/// **The window's length is measured, not assumed to be
/// `REFERENCE_SYMBOLS * WORKING_SPS`.** That figure is the *raw*, unfiltered
/// symbol count;
/// [`front_end`]'s filter costs `taps - 1` samples of length wherever it
/// meets a real edge, which the margin moves away from the sync word's own
/// edges but does not make disappear - it still costs samples in total, at
/// the padded array's own two ends instead. Filtering the sync word alone
/// (with no margin, discarding *only its length*, not this copy's content)
/// gives the exact number of fully-real-context output samples to keep;
/// asking for more than that would run the extracted window past the true
/// sync content and into the suffix margin at its far edge - measured
/// directly during this fix's own development, and the second of the two
/// bugs finding this reference construction cost.
/// The sync word's ideal frequency track: [`matched_reference`] - the sync
/// word as it arrives through the front end - through the discriminator.
/// What the detector correlates against.
fn frequency_template(reference: &[Complex<f32>], phy: Phy) -> Vec<f32> {
    let mut track = Vec::new();
    discriminate(reference, working_rate_hz(phy), &mut track);
    track
}

fn matched_reference(raw_rate: f64, phy: Phy) -> Result<Vec<Complex<f32>>, String> {
    let sps = (raw_rate / phy.symbol_rate_hz()).round().max(1.0) as usize;
    let sample_rate = sps as f64 * phy.symbol_rate_hz();
    let mut sync_bits = preamble_bits(ADVERTISING_ACCESS_ADDRESS, phy);
    sync_bits.extend_from_slice(&access_address_bits(ADVERTISING_ACCESS_ADDRESS));

    let mut padded_bits = margin_bits(MARGIN_SYMBOLS);
    padded_bits.extend_from_slice(&sync_bits);
    padded_bits.extend_from_slice(&margin_bits(MARGIN_SYMBOLS));
    let padded_raw = gfsk::modulate(&padded_bits, sps, phy.deviation_hz(), sample_rate, 0.5);
    let mut padded_filtered = Vec::new();
    front_end(raw_rate, phy)?.process(&padded_raw, &mut padded_filtered);

    let sync_raw = gfsk::modulate(&sync_bits, sps, phy.deviation_hz(), sample_rate, 0.5);
    let mut sync_filtered = Vec::new();
    front_end(raw_rate, phy)?.process(&sync_raw, &mut sync_filtered);
    let want = sync_filtered.len();

    let skip = MARGIN_SYMBOLS * WORKING_SPS;
    if skip + want > padded_filtered.len() {
        return Err(
            "BLE anti-alias reference construction produced a shorter capture than expected"
                .to_string(),
        );
    }
    Ok(padded_filtered[skip..skip + want].to_vec())
}

/// How many symbols of [`margin_bits`] pad each side of [`matched_reference`]'s
/// sync word: enough for `gfsk::modulate`'s own Gaussian filter and
/// [`front_end`]'s own anti-alias filter to both settle into real content
/// well before the sync word starts, with room to spare.
const MARGIN_SYMBOLS: usize = 16;

/// A fixed, non-degenerate bit pattern for [`matched_reference`]'s own
/// margin - alternating, the same character as the preamble it sits next
/// to, chosen only to give the shaping and anti-alias filters real content
/// to settle against rather than to be decoded as anything itself.
fn margin_bits(len: usize) -> Vec<bool> {
    (0..len).map(|i| i % 2 == 0).collect()
}

/// One channel's live receiver: the decimator, the sync-word detector, and
/// the capture in progress, if any.
pub struct Receiver {
    decim: StreamingDecimator,
    /// The detector: the discriminator's frequency track against the sync
    /// word's ideal one. See [`shape_threshold`].
    shape: ShapeMatcher,
    threshold: f64,
    /// The last working sample, so the discriminator runs straight across
    /// block boundaries.
    last_sample: Option<Complex<f32>>,
    /// The sync word as it should arrive, filtered and decimated: not for
    /// detection any more, but for the packet's SNR once its carrier offset
    /// is known (see [`Self::corrected_snr_db`]).
    reference: Vec<Complex<f32>>,
    reference_energy: f64,
    /// The last `reference.len()` working samples.
    recent: std::collections::VecDeque<Complex<f32>>,
    /// The samples of the sync word this capture was triggered on, taken at
    /// the strongest reading in the first two symbols after the trigger.
    sync_window: Vec<Complex<f32>>,
    sync_rho: f64,
    /// `capture.len()` at the trigger, to count samples since it.
    trigger_len: usize,
    channel: u8,
    raw_rate: f64,
    /// Which PHY this receiver decodes - B17's own addition. Fixed for the
    /// receiver's own lifetime the same way `channel` and `raw_rate` are:
    /// a change on any of the three invalidates the matched filter's own
    /// reference and the decimator's own filter, so [`Self::matches`] holds
    /// all three to account and the caller rebuilds rather than reusing.
    phy: Phy,
    /// The last [`LOOKBACK_SAMPLES`] filtered samples, kept continuously
    /// regardless of `capturing` - see `try_decode`'s own doc for why a
    /// trigger's own position is not trusted to be the header's own first
    /// sample, and needs samples from *before* the trigger to search
    /// against as well as after it.
    history: std::collections::VecDeque<Complex<f32>>,
    capture: Vec<Complex<f32>>,
    capturing: bool,
    /// What happened to each trigger since the worker last asked
    /// ([`Self::take_funnel`]).
    funnel: Funnel,
    /// Raw I/Q pairs per working sample: how a position in the working
    /// stream becomes one in the radio's.
    raw_per_working: f64,
    /// Where the next block is assumed to start when the caller does not say
    /// ([`Self::push`]): straight after the last one.
    next_pair: u64,
    /// Where the capture now under way triggered, in stream pairs: the time
    /// its packet is stamped with (`pdu::Packet::at_pair`).
    trigger_pair: u64,
}

/// What the receiver did with the samples it was given, counted as it went:
/// the decode funnel the decode-health panel reads.
///
/// **Every trigger ends one of three ways, and each is counted once.** The
/// capture yields a packet whose CRC passed at one of the alignments searched
/// (`decoded`); or it reaches the longest a PDU can be without one, and the
/// alignment whose length agrees with where the signal actually stopped is
/// reported as a failed CRC (`crc_failed`), or, when none agrees, nothing is
/// (`gave_up`). See `Receiver::best_failed_candidate`. There is no separate "header parsed" stage to
/// count: the search returns only a position whose CRC passes, and a counter
/// for a stage the receiver does not have would be an invented one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Funnel {
    pub triggered: u64,
    pub decoded: u64,
    pub crc_failed: u64,
    pub gave_up: u64,
}

impl Funnel {
    pub fn add(&mut self, other: Funnel) {
        self.triggered += other.triggered;
        self.decoded += other.decoded;
        self.crc_failed += other.crc_failed;
        self.gave_up += other.gave_up;
    }

    pub fn is_empty(&self) -> bool {
        *self == Funnel::default()
    }
}

impl Receiver {
    /// The funnel counted since the last call, and a fresh one started.
    pub fn take_funnel(&mut self) -> Funnel {
        std::mem::take(&mut self.funnel)
    }

    pub fn new(raw_rate: f64, channel: u8, phy: Phy) -> Result<Self, String> {
        let decim = front_end(raw_rate, phy)?;
        let reference = matched_reference(raw_rate, phy)?;
        let shape = frequency_template(&reference, phy);
        let reference_energy = reference.iter().map(|s| s.norm_sqr() as f64).sum();
        Ok(Self {
            decim,
            shape: ShapeMatcher::new(&shape),
            threshold: shape_threshold(phy),
            last_sample: None,
            recent: std::collections::VecDeque::with_capacity(reference.len() + 1),
            reference,
            reference_energy,
            sync_window: Vec::new(),
            sync_rho: 0.0,
            trigger_len: 0,
            channel,
            raw_rate,
            phy,
            history: std::collections::VecDeque::with_capacity(LOOKBACK_SAMPLES + 1),
            capture: Vec::new(),
            capturing: false,
            funnel: Funnel::default(),
            raw_per_working: raw_rate / working_rate_hz(phy),
            next_pair: 0,
            trigger_pair: 0,
        })
    }

    /// Whether this receiver is still the right one for `channel` at
    /// `raw_rate` on `phy` - a retune, a sample-rate change or a PHY change
    /// invalidates the detector's own reference and the capture in
    /// progress alike, so the caller rebuilds rather than reusing.
    /// The carrier offset, read from the sync word this capture was triggered
    /// on.
    ///
    /// **Data-aided, because the data is not balanced.** B7 took the mean of
    /// the packet's own symbols, on the reasoning that whitened data has as
    /// many ones as zeros. Over a real stretch of bits it has roughly as many,
    /// and a short packet's surplus of either moves the mean by the deviation
    /// times the surplus fraction: a synthetic packet sent 15 kHz off was
    /// reported at 35. The preamble and access address are known exactly.
    ///
    /// **And read as a tone, not as a mean of frequencies.** Multiplying the
    /// received sync word by the conjugate of the reference strips the
    /// modulation off and leaves `z[k]`, a constant-amplitude tone at the
    /// carrier offset. Its frequency is the slope of its phase, fitted by
    /// least squares - every sample of the sync word contributing, rather
    /// than one discriminator
    /// reading per symbol, and precise enough that turning the sync word back
    /// by it leaves the coherence the SNR is read from intact (see
    /// [`Self::corrected_snr_db`]). A first version read the offset from one
    /// discriminator reading per symbol; its ±20 kHz of scatter was enough
    /// to spin the phase across the window again and report CRC-clean
    /// packets at -7 dB.
    ///
    /// The uncertainty is the fit's: the scatter of the tone's phase about the
    /// line, through as many independent points as the residuals'
    /// autocorrelation says there are. Two earlier versions each got this
    /// wrong in a different direction - the scatter of per-sample phase steps
    /// overstated it several times (neighbouring steps share a sample, so
    /// their errors cancel along the line), and one point per symbol
    /// overstated it by two thirds. `the_offset_s_uncertainty_matches_its_
    /// scatter` holds the stated figure to the measured one.
    fn sync_offset(&self) -> Option<crate::signal::dsp::uncertainty::Uncertain> {
        let n = self.reference.len();
        if self.sync_window.len() != n || n < 2 * WORKING_SPS {
            return None;
        }
        let tone: Vec<Complex<f64>> = self
            .sync_window
            .iter()
            .zip(&self.reference)
            .map(|(w, r)| {
                Complex::new(w.re as f64, w.im as f64) * Complex::new(r.re as f64, -(r.im as f64))
            })
            .collect();
        let steps: Vec<Complex<f64>> = tone.windows(2).map(|p| p[1] * p[0].conj()).collect();
        let sum: Complex<f64> = steps.iter().sum();
        if sum.norm() == 0.0 {
            return None;
        }
        let mean_step = sum.arg();
        // The tone's phase, unwrapped about the mean step so a large offset
        // never wraps, then a least-squares line through it: its slope is the
        // offset, its scatter about the line the noise the slope was read
        // through.
        let mut phase = Vec::with_capacity(tone.len());
        let mut acc = 0.0f64;
        phase.push(0.0);
        for (k, step) in steps.iter().enumerate() {
            acc += (step * Complex::from_polar(1.0, -mean_step)).arg();
            phase.push(acc + mean_step * (k + 1) as f64);
        }
        let n_pts = phase.len() as f64;
        let k_mean = (n_pts - 1.0) / 2.0;
        let p_mean = phase.iter().sum::<f64>() / n_pts;
        let sxx: f64 = (0..phase.len()).map(|k| (k as f64 - k_mean).powi(2)).sum();
        let sxy: f64 = phase
            .iter()
            .enumerate()
            .map(|(k, p)| (k as f64 - k_mean) * (p - p_mean))
            .sum();
        let slope = sxy / sxx;
        let residual: Vec<f64> = phase
            .iter()
            .enumerate()
            .map(|(k, p)| p - p_mean - slope * (k as f64 - k_mean))
            .collect();
        let residual_var = residual.iter().map(|r| r * r).sum::<f64>() / (n_pts - 2.0).max(1.0);
        if residual_var <= 0.0 {
            return Some(crate::signal::dsp::uncertainty::Uncertain::exact(
                slope * working_rate_hz(self.phy) / std::f64::consts::TAU,
            ));
        }
        // How many independent points the line was really fitted through.
        // The samples are not independent - the front end and the Gaussian
        // shaping both spread each error over its neighbours - and assuming
        // they were understates the uncertainty; assuming one per symbol
        // overstated it. The residuals' own autocorrelation says: the usual
        // effective sample size, `n / (1 + 2 sum rho_lag)`, over the lags a
        // symbol or two spans.
        let lag_sum: f64 = (1..=2 * WORKING_SPS)
            .map(|lag| {
                residual
                    .iter()
                    .zip(&residual[lag..])
                    .map(|(a, b)| a * b)
                    .sum::<f64>()
                    / ((residual.len() - lag) as f64 * residual_var)
            })
            .sum();
        let n_eff = (n_pts / (1.0 + 2.0 * lag_sum)).clamp(2.0, n_pts);
        // A line's slope through `n_eff` independent points spread over the
        // same span has variance `12 sigma^2 / (n (n^2 - 1))` per spacing
        // squared; the spacing is `n_pts / n_eff` samples.
        let spacing = n_pts / n_eff;
        let per_sample = (12.0 * residual_var / (n_eff * (n_eff * n_eff - 1.0))).sqrt() / spacing;
        let rate = working_rate_hz(self.phy);
        let to_hz = rate / std::f64::consts::TAU;
        Some(crate::signal::dsp::uncertainty::Uncertain::from_sigma(
            slope * to_hz,
            per_sample * to_hz,
        ))
    }

    /// The packet's SNR, from the sync word it was triggered on, once its
    /// carrier offset is known.
    ///
    /// The sync word's samples are turned back by `offset_hz` and correlated
    /// coherently with the reference, and the coherence goes through
    /// `snr_from_metric`, as B7 set out. Before the offset was taken out, the
    /// coherence - and so the SNR - was pulled down by the transmitter's own
    /// crystal error, reporting an offset device as a weak one.
    ///
    /// `snr_from_metric` was derived for `Coherence::metric` - two noisy
    /// copies of the same unknown signal correlated against each other - and
    /// this is a noisy signal against a known, noiseless reference. For a
    /// unit-power reference at this window length the two converge to the
    /// same `rho = snr / (1 + snr)` relationship, so the tested inverse is
    /// reused: a close approximation here, not proven exact.
    fn corrected_snr_db(&self, offset_hz: f64) -> Option<f64> {
        let n = self.reference.len();
        if self.sync_window.len() != n || self.reference_energy <= 0.0 {
            return None;
        }
        let rate = working_rate_hz(self.phy);
        let mut value = Complex::new(0.0f64, 0.0);
        let mut window_energy = 0.0f64;
        for (k, (r, w)) in self.reference.iter().zip(&self.sync_window).enumerate() {
            let ph = -std::f64::consts::TAU * offset_hz * k as f64 / rate;
            let w = Complex::new(w.re as f64, w.im as f64) * Complex::new(ph.cos(), ph.sin());
            value += Complex::new(r.re as f64, -(r.im as f64)) * w;
            window_energy += w.norm_sqr();
        }
        if window_energy <= 0.0 {
            return None;
        }
        let coherence = value.norm_sqr() / (self.reference_energy * window_energy);
        snr_from_metric(coherence, n).map(|snr| 10.0 * snr.log10())
    }

    pub fn matches(&self, channel: u8, raw_rate: f64, phy: Phy) -> bool {
        self.channel == channel && (self.raw_rate - raw_rate).abs() < 1.0 && self.phy == phy
    }

    /// Feed one block of raw device bytes. Returns every packet fully
    /// decoded from it - almost always zero, and rarely more than one: a
    /// legacy advertising PDU is under a third of a millisecond of air time.
    /// [`Self::push_at`] for a block assumed to follow the last one without a
    /// gap: what a test feeding a capture in pieces means, and nothing a live
    /// stream should rely on, which is why only the tests have it.
    #[cfg(test)]
    pub fn push(&mut self, bytes: &[u8], geometry: SampleGeometry) -> Vec<Packet> {
        self.push_at(bytes, geometry, self.next_pair)
    }

    /// Feed one block whose first pair sits at `first_pair` in the stream
    /// (`hardware::StreamBlock::first_pair`), and return the packets it
    /// completed, each stamped with where its sync word triggered.
    ///
    /// **The stamp is the trigger's, not the decode's.** A packet completes
    /// blocks after it began, and the trigger is where it began: the same
    /// place in every packet to within the few samples `candidates` searches
    /// over, microseconds at most, which is what timing one packet against the
    /// next needs. A trigger in the working stream maps back to the radio's
    /// pairs through the decimation ratio; the decimator's own delay is the
    /// same for every packet, so it cancels from any difference of two.
    pub fn push_at(
        &mut self,
        bytes: &[u8],
        geometry: SampleGeometry,
        first_pair: u64,
    ) -> Vec<Packet> {
        let mut iq = Vec::new();
        decode_iq(bytes, geometry, usize::MAX, &mut iq);
        let mut working = Vec::new();
        self.decim.process(&iq, &mut working);

        let cap_limit = (16 + MAX_PDU_BYTES * 8) * WORKING_SPS;
        // Every sample through the detector, capturing or not, as one block:
        // the discriminator first, straight across the block boundary, then
        // the correlation against the sync word's frequency track. A
        // detector fed only between captures used to resume after each one
        // with a window joining samples from before the capture to samples
        // after it - a splice of its own, on every trigger.
        let rate = working_rate_hz(self.phy);
        let mut track = Vec::with_capacity(working.len());
        for &s in &working {
            track.push(match self.last_sample {
                Some(prev) => instantaneous_freq_hz(prev, s, rate),
                None => 0.0,
            });
            self.last_sample = Some(s);
        }
        let mut readings = Vec::new();
        self.shape.process_block(&track, &mut readings);

        self.next_pair = first_pair + iq.len() as u64;
        let mut found = Vec::new();
        for (j, (&sample, reading)) in working.iter().zip(&readings).enumerate() {
            self.recent.push_back(sample);
            if self.recent.len() > self.reference.len() {
                self.recent.pop_front();
            }
            // A ring: one push and at most one pop a sample, where a `Vec`
            // trimmed from the front moved every sample it held, every time.
            self.history.push_back(sample);
            if self.history.len() > LOOKBACK_SAMPLES {
                self.history.pop_front();
            }
            if self.capturing {
                self.capture.push(sample);
                // The trigger fires on the way up; the sync word's own
                // samples are taken where the reading peaks, within two
                // symbols of it.
                if self.capture.len() - self.trigger_len <= 2 * WORKING_SPS {
                    if let Some(rho) = *reading {
                        if rho > self.sync_rho {
                            self.sync_rho = rho;
                            self.sync_window = self.recent.iter().copied().collect();
                        }
                    }
                }
                let due = self.capture.len().is_multiple_of(DECODE_EVERY_SAMPLES);
                match due.then(|| self.try_decode()).flatten() {
                    Some(mut packet) => {
                        self.funnel.decoded += 1;
                        packet.at_pair = Some(self.trigger_pair);
                        found.push(packet);
                        self.capturing = false;
                        self.capture.clear();
                    }
                    None if self.capture.len() > cap_limit => {
                        // Either a corrupt length field or a false trigger
                        // with nothing real behind it. Either way, waiting
                        // longer only spends memory: give up. One last,
                        // honest attempt at the trigger's own nominal
                        // boundary before giving up entirely - not to widen
                        // the search further, only so a real packet that
                        // truly was aligned there, and genuinely failed its
                        // CRC, still shows up as that rather than vanishing
                        // silently the way a wrong-alignment guess would.
                        match self.best_failed_candidate() {
                            Some(mut packet) => {
                                packet.at_pair = Some(self.trigger_pair);
                                if packet.crc_ok {
                                    self.funnel.decoded += 1;
                                } else {
                                    self.funnel.crc_failed += 1;
                                }
                                found.push(packet);
                            }
                            None => self.funnel.gave_up += 1,
                        }
                        self.capturing = false;
                        self.capture.clear();
                    }
                    None => {}
                }
            } else if let Some(rho) = *reading {
                if rho > self.threshold {
                    self.funnel.triggered += 1;
                    self.capturing = true;
                    self.trigger_pair = first_pair + (j as f64 * self.raw_per_working) as u64;
                    self.capture = self.history.iter().copied().collect();
                    self.trigger_len = self.capture.len();
                    self.sync_rho = rho;
                    self.sync_window = self.recent.iter().copied().collect();
                }
            }
        }
        found
    }

    /// Search a small range of candidate header start positions around the
    /// trigger's own nominal boundary, and return the first one whose CRC
    /// actually passes. `None` means either "not enough captured yet for
    /// any candidate" or "no candidate in range has a clean CRC yet" - both
    /// are the same instruction to the caller: keep capturing.
    ///
    /// **Why a search, and not a single trusted position.** `decode_at`
    /// does the real work at one candidate boundary; this exists because
    /// the boundary itself is not a single sample, on this receiver. Design
    /// intent was "the sample right after the trigger is the header's own
    /// first sample" - true with no filtering in the path (B6 through B9),
    /// and false once `front_end` gained its anti-alias filter: a filter
    /// with any real transition band smears the sync word's own energy into
    /// its neighbours over roughly its own settling time, on both sides of
    /// the true boundary, and a matched filter's own peak inside that
    /// smeared region can land on whichever nearby sample happens to
    /// correlate best for a given capture's own noise and content - a real,
    /// data-dependent few symbols, not a fixed offset a formula could give
    /// back. Measured directly while chasing this: the same construction,
    /// changed only in incidental ways (adding sixteen realistic symbols of
    /// lead-in before the sync word, which no earlier step's synthetic
    /// tests ever included), moved the boundary from two symbols early to
    /// three. `find_phase` already solves the identical problem one layer
    /// down, for the *sub-symbol* phase within one candidate; this is that
    /// same idea at the symbol grid above it, over [`HEADER_SEARCH_SYMBOLS`]
    /// either side of [`LOOKBACK_SAMPLES`], which is `self.capture`'s own
    /// nominal boundary once `push` starts seeding it from
    /// [`Receiver::history`] rather than empty.
    fn try_decode(&self) -> Option<Packet> {
        self.candidates()
            .into_iter()
            .find_map(|(_, packet)| packet.crc_ok.then_some(packet))
    }

    /// Every alignment [`Self::try_decode`] searches, in order, decoded: the
    /// header start it was read from and the packet, CRC passed or not.
    fn candidates(&self) -> Vec<(usize, Packet)> {
        let center = LOOKBACK_SAMPLES as isize;
        let step = WORKING_SPS as isize;
        let span = HEADER_SEARCH_SYMBOLS as isize;
        // One discriminator pass for every candidate: see `decode_at`.
        let inst = self.discriminated();
        // And one phase search. The candidates are whole symbols apart, so
        // they share where inside a symbol to sample; searching from the
        // earliest one uses every symbol any of them will read. Thirteen
        // searches, each over nearly the same samples, were most of what a
        // capture that never passed its CRC cost.
        let earliest = (center - span * step).max(0) as usize;
        let from_earliest = &inst[earliest.min(inst.len())..];
        let phase = super::sync::phase(
            from_earliest,
            WORKING_SPS as f64,
            from_earliest.len() / WORKING_SPS,
        );
        let mut out = Vec::new();
        for k in -span..=span {
            let skip = center + k * step;
            if skip < 0 {
                continue;
            }
            if let Some(packet) = self.decode_at(&inst, skip as usize, Some(phase)) {
                let passed = packet.crc_ok;
                out.push((skip as usize, packet));
                // The search stops at the first CRC that passes, as it
                // always has: later alignments cannot beat a passing one.
                if passed {
                    break;
                }
            }
        }
        out
    }

    /// At give-up, the one alignment the capture itself vouches for, reported
    /// as the packet it decodes to, CRC and all.
    ///
    /// **No alignment passed its CRC, so which to believe is chosen by
    /// something other than the CRC: the energy.** Each candidate's header
    /// says how long its packet is, which says where the packet ends; the
    /// capture says where the signal actually stopped ([`energy_end`]). The
    /// candidate whose end agrees, within [`END_TOLERANCE_SYMBOLS`], is the
    /// packet that was on the air with a bit error in it, and is counted and
    /// listed as a failed CRC. If none agrees (a false trigger, or energy that
    /// never stops because the next transmission follows), nothing is
    /// reported: choosing among alignments that nothing vouches for would be
    /// an invented packet (Viktor's decision, `net-ux-polish-plan.md` 3.4.c).
    ///
    /// The one this replaced decoded at the nominal boundary alone, a few
    /// symbols from where the receiver's own measurements put the real one.
    /// On the 2026-09-19 channel 37 recording it reported 41 failed CRCs, every
    /// one a reserved PDU type from a recurring non-BLE source, and none of
    /// the three bit-error packets from a device heard fifteen times cleanly
    /// on the same recording. This reports those three (one address bit
    /// flipped in each) and 9 of the 41, where their length agrees with the
    /// signal; the CRC-good output is byte-identical on both recordings.
    fn best_failed_candidate(&self) -> Option<Packet> {
        let end = energy_end(&self.capture, LOOKBACK_SAMPLES)?;
        let tolerance = END_TOLERANCE_SYMBOLS * WORKING_SPS;
        self.candidates()
            .into_iter()
            .filter_map(|(skip, packet)| {
                let ends = skip + pdu::used_bits(packet.length) * WORKING_SPS;
                let off = ends.abs_diff(end);
                (off <= tolerance).then_some((off, packet))
            })
            .min_by_key(|(off, _)| *off)
            .map(|(_, packet)| packet)
    }

    /// The whole capture through the discriminator, once.
    fn discriminated(&self) -> Vec<f32> {
        let mut inst = Vec::new();
        discriminate(&self.capture, working_rate_hz(self.phy), &mut inst);
        inst
    }

    /// The decode at one candidate header start, `skip` samples into the
    /// capture, given the whole capture already discriminated.
    ///
    /// **The discriminator of a capture started `skip` samples in is exactly
    /// the whole capture's discriminator from `skip` on** - each reading is a
    /// function of two neighbouring samples and nothing else - so the search
    /// over candidate boundaries slices one pass rather than making thirteen.
    ///
    /// `phase`, when given, is the sub-symbol sampling phase already found
    /// for this capture (see `try_decode`); `None` searches for it here.
    fn decode_at(&self, whole: &[f32], skip: usize, phase: Option<f64>) -> Option<Packet> {
        if skip >= self.capture.len() {
            return None;
        }
        let inst = &whole[skip.min(whole.len())..];
        let symbols = inst.len() / WORKING_SPS;
        if symbols < pdu::HEADER_BITS {
            return None;
        }
        // A tuning sitting exactly on the channel's own centre - which this
        // arc's is, since it never mixes off it - is exactly where a real
        // front end's LO leakage and IQ DC offset concentrate, and where a
        // real transmitter's own crystal error shows up too: both are a
        // constant added to every discriminator sample, indistinguishable
        // from each other at this stage and not present in this arc's own
        // synthetic tests, which is why slicing against a fixed zero passed
        // every one of them and no real capture. The capture's own mean is
        // the honest estimate of that constant - GFSK data is balanced over
        // any real stretch of bits - and this only needs a point estimate:
        // a threshold decision does not need a calibrated uncertainty, only
        // the displayed reading below does, and gets its own.
        let rough_offset = crate::signal::dsp::uncertainty::mean_with_uncertainty(inst);
        let sps = WORKING_SPS as f64;
        let phase = phase.unwrap_or_else(|| super::sync::phase(inst, sps, symbols));
        let threshold = rough_offset.value() as f32;
        // The header first, alone. Most attempts come before the packet has
        // finished arriving, and its length says so from sixteen bits;
        // slicing and decoding the rest only to find it short was most of
        // what a busy channel cost. The same bits either way - at a fixed
        // phase and threshold each symbol is sliced on its own - so this
        // only skips work whose answer was already `None`.
        let (mut header, _) = super::sync::slice_at(inst, sps, pdu::HEADER_BITS, threshold, phase);
        whiten(&mut header, self.channel);
        if pdu::used_bits(pdu::length(&header)?) > symbols {
            return None;
        }
        let (mut bits, raw_symbols) = super::sync::slice_at(inst, sps, symbols, threshold, phase);
        // B8's modulation-quality measurement needs the physically
        // transmitted (still-whitened) symbols and their raw discriminator
        // readings - exactly what `bits` and `raw_symbols` are before the
        // next line undoes whitening to recover the data underneath them.
        // See `measure`'s own module doc for why the physical bits, not the
        // decoded ones, are what a Gaussian filter's settling depends on.
        let raw_bits = bits.clone();
        whiten(&mut bits, self.channel);
        let mut packet = pdu::decode(&bits)?;
        // Trimmed to exactly this packet's own bits before measuring: `bits`
        // and `raw_symbols` run to the end of whatever has been captured,
        // which is deliberately more than one packet's worth (see this
        // struct's own `push`), and letting the search wander into trailing
        // noise or the next packet's preamble would mix an unrelated
        // signal's deviation into this one's own reading.
        let used = pdu::used_bits(packet.length).min(raw_bits.len());
        let raw_bits = &raw_bits[..used];
        let raw_symbols = &raw_symbols[..used];
        // The *reported* offset is read against the sync word, not the
        // packet's data: see `sync_offset`. `rough_offset` above never had to
        // be exact, only good enough to slice against; this one is what a
        // reader sees a `+/-` on.
        let offset = self.sync_offset();
        packet.freq_offset_hz = offset;
        packet.snr_db = offset.and_then(|o| self.corrected_snr_db(o.value()));
        // On either PHY, each scaled by its own symbol rate
        // (net-ux-polish-plan 5.5).
        packet.modulation = super::measure::modulation_quality(raw_bits, raw_symbols, self.phy);
        packet.drift = super::measure::drift(raw_symbols, self.phy);
        Some(packet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::{SampleFormat, StreamBlock};
    use crate::signal::ble::channel;
    use crate::signal::ble::gfsk::modulate;
    use crate::signal::dsp::testkit::{at_snr, Rng};

    fn eight_bit() -> SampleGeometry {
        SampleGeometry {
            format: SampleFormat::Int8,
            full_scale: 128.0,
        }
    }

    /// A synthetic packet, preamble through CRC, at `phy`'s own working
    /// rate - the same construction B3's own detection tests use, extended
    /// with a real PDU instead of random bits after the sync word. B17's
    /// own addition: `phy`, so the identical construction proves both PHYs
    /// rather than a second, separately-written one for LE 2M.
    fn synthetic_packet_iq(
        phy: Phy,
        ch: u8,
        header_byte0: u8,
        payload: &[u8],
        noise_snr_db: f64,
    ) -> Vec<Complex<f32>> {
        let sps = WORKING_SPS;
        let sample_rate = working_rate_hz(phy);
        // A real capture never starts the instant a packet's own first bit
        // begins either - there is always earlier stream before it, whether
        // the channel's own noise floor or another packet's tail. Leading
        // padding stands in for that, the same reason the trailing padding
        // below exists: [`front_end`]'s anti-alias filter and `gfsk::modulate`'s
        // own Gaussian filter both taper toward a real edge, and a synthetic
        // signal with no lead-in hands the detector exactly the edge
        // [`matched_reference`]'s own margin exists to avoid needing.
        let mut rng = Rng::new(4242);
        let mut bits: Vec<bool> = (0..16).map(|_| rng.next_u64() & 1 == 1).collect();
        bits.extend(super::super::detect::preamble_bits(
            ADVERTISING_ACCESS_ADDRESS,
            phy,
        ));
        bits.extend_from_slice(&super::super::detect::access_address_bits(
            ADVERTISING_ACCESS_ADDRESS,
        ));
        bits.extend_from_slice(&pdu::encode(ch, header_byte0, payload));
        // A real capture never stops the instant a packet's own last bit
        // ends - there is always more stream after it, whether the next
        // packet's own preamble or just the channel's noise floor. Trailing
        // padding stands in for that, so decoding the last symbol never
        // waits on a sample this test would otherwise never provide.
        let mut rng = Rng::new(1234);
        bits.extend((0..16).map(|_| rng.next_u64() & 1 == 1));
        let clean = modulate(&bits, sps, phy.deviation_hz(), sample_rate, 0.5);
        if noise_snr_db.is_finite() {
            at_snr(&clean, noise_snr_db, &mut Rng::new(99))
        } else {
            clean
        }
    }

    fn bytes_for(iq: &[Complex<f32>], geometry: SampleGeometry) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(iq.len() * 2);
        for s in iq {
            let re = (s.re * geometry.full_scale).clamp(-127.0, 127.0) as i8;
            let im = (s.im * geometry.full_scale).clamp(-127.0, 127.0) as i8;
            bytes.push(re as u8);
            bytes.push(im as u8);
        }
        bytes
    }

    /// B6's exit condition, built with a synthetic transmitter standing in
    /// for the real one: a full ADV_IND, correctly received end to end -
    /// detection, timing, de-whitening and CRC all agreeing.
    #[test]
    fn a_synthetic_adv_ind_is_received_whole() {
        let addr = [0xAA, 0xBB, 0xCC, 0x11, 0x22, 0x33];
        let mut payload = crate::signal::ble::pdu::air_octets(addr).to_vec();
        payload.extend_from_slice(&[0x02, 0x01, 0x06]);
        let iq = synthetic_packet_iq(Phy::OneM, 37, 0x00, &payload, 20.0);
        let geometry = eight_bit();
        let bytes = bytes_for(&iq, geometry);

        let mut rx = Receiver::new(working_rate_hz(Phy::OneM), 37, Phy::OneM).unwrap();
        let packets = rx.push(&bytes, geometry);
        assert_eq!(packets.len(), 1, "expected exactly one packet");
        let p = &packets[0];
        assert_eq!(p.pdu_type, pdu::PduType::AdvInd);
        assert_eq!(p.adv_addr, Some(addr));
        assert!(p.crc_ok);
    }

    /// **A packet is stamped with where it began in the radio's own sample
    /// clock**, lost pairs included: identical packets in blocks placed with
    /// gaps between them (driver drops) are as far apart as their blocks say.
    /// The timing of one advertising event against the next rests on this
    /// (`signal::ble::interval`).
    ///
    /// Once running, to within a symbol. The very first packet a new
    /// receiver hears triggers 18 working samples (4.5 µs at 4 Msps) earlier
    /// than every later one, measured here: the detector settling from a cold
    /// start. A thousandth of the advertising delay being measured, so it is
    /// bounded here rather than designed away.
    #[test]
    fn packets_are_stamped_in_stream_pairs_across_a_gap() {
        let addr = [0xAA, 0xBB, 0xCC, 0x11, 0x22, 0x33];
        let mut payload = crate::signal::ble::pdu::air_octets(addr).to_vec();
        payload.extend_from_slice(&[0x02, 0x01, 0x06]);
        let one = synthetic_packet_iq(Phy::OneM, 37, 0x00, &payload, 25.0);
        let geometry = eight_bit();
        // The same quiet lead-in before each, so every trigger finds the
        // detector in the same state.
        let quiet = vec![Complex::new(0.0, 0.0); 20_000];
        let block: Vec<Complex<f32>> = quiet.iter().chain(&one).copied().collect();
        let tail = vec![Complex::new(0.0, 0.0); 4_000];

        let mut rx = Receiver::new(working_rate_hz(Phy::OneM), 37, Phy::OneM).unwrap();
        let mut at = 0u64;
        let mut stamps = Vec::new();
        for gap in [50_000u64, 70_000, 3] {
            let mut got = rx.push_at(&bytes_for(&block, geometry), geometry, at);
            got.extend(rx.push(&bytes_for(&tail, geometry), geometry));
            assert_eq!(got.len(), 1, "{got:?}");
            stamps.push((at, got[0].at_pair.unwrap()));
            at += (block.len() + tail.len()) as u64 + gap;
        }
        let apart = |i: usize| {
            let ((b0, s0), (b1, s1)) = (stamps[i], stamps[i + 1]);
            (s1 - s0) as i64 - (b1 - b0) as i64
        };
        let rate = working_rate_hz(Phy::OneM);
        assert!(apart(1).abs() <= WORKING_SPS as i64, "running: {stamps:?}");
        let cold_us = apart(0).abs() as f64 / rate * 1e6;
        assert!(cold_us <= 10.0, "first packet {cold_us} us off: {stamps:?}");
    }

    /// **Every trigger is counted once, by how it ended.** A clean packet is
    /// one trigger and one CRC-good decode, and taking the funnel resets it.
    ///
    /// **And a bit error is a failed CRC, not a disappearance.** A packet with
    /// one bit flipped in its PDU triggers the same way and is never accepted
    /// during the capture (the search wants a passing CRC). At the longest a
    /// PDU can be, the candidate whose length agrees with where the signal
    /// stopped is reported: a failed CRC, with the packet's own header. Until
    /// 2026-09-19 the give-up decoded at the nominal boundary alone and this
    /// packet came back as nothing ("gave up"); this test is what measured it.
    ///
    /// And where no candidate's length agrees, because the signal never stops
    /// (a transmission that runs the whole capture), nothing is reported.
    #[test]
    fn the_funnel_counts_each_trigger_by_how_it_ended() {
        let rate = working_rate_hz(Phy::OneM);
        let geometry = eight_bit();
        let addr = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
        let mut payload = crate::signal::ble::pdu::air_octets(addr).to_vec();
        payload.extend_from_slice(&[0x02, 0x01, 0x06]);

        let iq = synthetic_packet_iq(Phy::OneM, 37, 0x00, &payload, 25.0);
        let mut rx = Receiver::new(rate, 37, Phy::OneM).unwrap();
        let packets = rx.push(&bytes_for(&iq, geometry), geometry);
        assert_eq!(packets.len(), 1);
        let f = rx.take_funnel();
        assert_eq!(
            (f.triggered, f.decoded, f.crc_failed, f.gave_up),
            (1, 1, 0, 0)
        );
        assert!(rx.take_funnel().is_empty(), "taken, and started again");

        // The same packet with one PDU bit flipped, then silence (noise only)
        // for long enough that the capture reaches the longest a PDU can be.
        let sps = WORKING_SPS;
        let mut rng = Rng::new(4242);
        let mut bits: Vec<bool> = (0..16).map(|_| rng.next_u64() & 1 == 1).collect();
        bits.extend(super::super::detect::preamble_bits(
            ADVERTISING_ACCESS_ADDRESS,
            Phy::OneM,
        ));
        bits.extend_from_slice(&super::super::detect::access_address_bits(
            ADVERTISING_ACCESS_ADDRESS,
        ));
        let mut pdu_bits = pdu::encode(37, 0x00, &payload);
        pdu_bits[40] = !pdu_bits[40];
        bits.extend_from_slice(&pdu_bits);
        let mut clean = modulate(&bits, sps, Phy::OneM.deviation_hz(), rate, 0.5);
        let signal = clean.len();
        clean.extend(vec![
            Complex::new(0.0, 0.0);
            (16 + MAX_PDU_BYTES * 8 + 64) * sps
        ]);
        // Noise at 25 dB under the packet, over the silence as well.
        let noise_power = clean[..signal]
            .iter()
            .map(|s| s.norm_sqr() as f64)
            .sum::<f64>()
            / signal as f64
            / 10f64.powf(2.5);
        let noise = Rng::new(99).noise(clean.len(), noise_power);
        let iq: Vec<Complex<f32>> = clean.iter().zip(&noise).map(|(s, z)| s + z).collect();
        let mut rx = Receiver::new(rate, 37, Phy::OneM).unwrap();
        let packets = rx.push(&bytes_for(&iq, geometry), geometry);
        let f = rx.take_funnel();
        assert_eq!(f.triggered, 1, "{f:?}");
        assert_eq!(f.decoded, 0, "{f:?}");
        assert_eq!((f.crc_failed, f.gave_up), (1, 0), "{f:?}");
        assert_eq!(packets.len(), 1, "{packets:?}");
        assert!(!packets[0].crc_ok);
        assert_eq!(packets[0].length, 9, "the packet's own header");

        // The same broken packet followed by more transmission, not silence:
        // the signal never stops inside the capture, no candidate's length
        // can be checked against it, and nothing is reported.
        let mut bits = bits.clone();
        let mut tail = Rng::new(7);
        bits.extend((0..(16 + MAX_PDU_BYTES * 8) + 64).map(|_| tail.next_u64() & 1 == 1));
        let clean = modulate(&bits, sps, Phy::OneM.deviation_hz(), rate, 0.5);
        let iq = at_snr(&clean, 25.0, &mut Rng::new(99));
        let mut rx = Receiver::new(rate, 37, Phy::OneM).unwrap();
        let packets = rx.push(&bytes_for(&iq, geometry), geometry);
        let f = rx.take_funnel();
        assert_eq!((f.triggered, f.crc_failed, f.gave_up), (1, 0, 1), "{f:?}");
        assert!(packets.is_empty(), "{packets:?}");
    }

    /// **Hearing does not depend on what the packet says.** At the standard's
    /// offset edge plus our own oscillator's share (±200 kHz, see
    /// `a_packet_with_a_crystal_offset_is_still_heard`), eight different
    /// payloads at 20 dB all decode. One payload is one draw of the slicer's
    /// data-dependent threshold; several are a claim about the receiver.
    #[test]
    fn hearing_does_not_depend_on_what_the_packet_says() {
        let rate = working_rate_hz(Phy::OneM);
        let geometry = eight_bit();
        for seed in 0..8u8 {
            let addr = [
                seed.wrapping_mul(37),
                seed ^ 0x5a,
                0x11u8.wrapping_add(seed),
                0xcc,
                seed.wrapping_mul(91),
                0xaa ^ seed,
            ];
            let mut payload = crate::signal::ble::pdu::air_octets(addr).to_vec();
            payload.extend_from_slice(&[0x02, 0x01, 0x06]);
            for cfo_hz in [-200_000.0, 200_000.0] {
                let mut iq = synthetic_packet_iq(Phy::OneM, 37, 0x40, &payload, 20.0);
                for (n, s) in iq.iter_mut().enumerate() {
                    let ph = std::f64::consts::TAU * cfo_hz * n as f64 / rate;
                    *s *= Complex::new(ph.cos() as f32, ph.sin() as f32);
                }
                let mut rx = Receiver::new(rate, 37, Phy::OneM).unwrap();
                let packets = rx.push(&bytes_for(&iq, geometry), geometry);
                assert!(
                    packets.len() == 1 && packets[0].crc_ok,
                    "payload {seed} at {cfo_hz} Hz: {} packets",
                    packets.len()
                );
                assert_eq!(packets[0].adv_addr, Some(addr));
            }
        }
    }

    /// **A transmitter's crystal is never exactly on frequency, and the
    /// receiver must hear it anyway.** Core 5.4 Vol 6 Part A 3.3: "The
    /// deviation of the center frequency during the packet shall not exceed
    /// ±150 kHz, including both the initial frequency offset and drift."
    /// What arrives is that plus our own oscillator's error, about ±50 kHz
    /// more for a ±20 ppm radio at 2.4 GHz, so the receiver is held to
    /// ±200 kHz. The first detector correlated coherently across the whole
    /// 40-symbol sync word, and an offset of 15 kHz - one real device in the
    /// test flat - turned the phase far enough across it to put the packet
    /// under the trigger threshold nine times in ten (`dev_docs/
    /// case-study-ble-crc.md`, section 13). The offsets here: that device,
    /// an ordinary crystal, and the far edge. The offset the packet reports
    /// is the one it was sent with.
    ///
    /// This test once used -300 kHz, twice the standard's limit, and passed
    /// on its payload's particular bits: measured over 24 payloads at 20 dB,
    /// ±300 kHz loses 8 of 48 packets while ±150 and ±200 kHz lose none of
    /// 96. It surfaced when addresses moved to the written octet order and
    /// the same payload put different bits on the air.
    /// `hearing_does_not_depend_on_what_the_packet_says` holds the ±200 kHz
    /// edge over several payloads so the next one cannot pass by luck.
    #[test]
    fn a_packet_with_a_crystal_offset_is_still_heard() {
        let addr = [0xAA, 0xBB, 0xCC, 0x11, 0x22, 0x33];
        let mut payload = crate::signal::ble::pdu::air_octets(addr).to_vec();
        payload.extend_from_slice(&[0x02, 0x01, 0x06]);
        let rate = working_rate_hz(Phy::OneM);
        let snr_at = |cfo_hz: f64| -> f64 {
            let mut iq = synthetic_packet_iq(Phy::OneM, 37, 0x00, &payload, 20.0);
            for (n, s) in iq.iter_mut().enumerate() {
                let ph = std::f64::consts::TAU * cfo_hz * n as f64 / rate;
                *s *= Complex::new(ph.cos() as f32, ph.sin() as f32);
            }
            let geometry = eight_bit();
            let mut rx = Receiver::new(rate, 37, Phy::OneM).unwrap();
            rx.push(&bytes_for(&iq, geometry), geometry)[0]
                .snr_db
                .expect("an SNR is measured")
        };
        // The SNR is the signal's, not the crystal's: a transmitter off
        // frequency reads the same as one on it.
        let on_frequency = snr_at(0.0);
        for cfo_hz in [15_000.0, 100_000.0, -200_000.0] {
            let off = snr_at(cfo_hz);
            assert!(
                (off - on_frequency).abs() < 1.5,
                "{cfo_hz} Hz: SNR {off} dB against {on_frequency} dB on frequency"
            );
        }
        for cfo_hz in [15_000.0, 100_000.0, -200_000.0] {
            let mut iq = synthetic_packet_iq(Phy::OneM, 37, 0x00, &payload, 20.0);
            for (n, s) in iq.iter_mut().enumerate() {
                let ph = std::f64::consts::TAU * cfo_hz * n as f64 / rate;
                *s *= Complex::new(ph.cos() as f32, ph.sin() as f32);
            }
            let geometry = eight_bit();
            let bytes = bytes_for(&iq, geometry);
            let mut rx = Receiver::new(rate, 37, Phy::OneM).unwrap();
            let packets = rx.push(&bytes, geometry);
            assert_eq!(packets.len(), 1, "{cfo_hz} Hz: expected exactly one packet");
            let p = &packets[0];
            assert!(p.crc_ok, "{cfo_hz} Hz: CRC failed");
            assert_eq!(p.adv_addr, Some(addr));
            let reported = p.freq_offset_hz.expect("an offset is measured").value();
            assert!(
                (reported - cfo_hz).abs() < 10_000.0,
                "{cfo_hz} Hz: reported {reported} Hz"
            );
        }
    }

    /// The carrier offset's stated uncertainty is the one it actually has.
    ///
    /// Forty packets, each with its own noise, all sent 50 kHz off: the
    /// scatter of the offsets they report must agree with the uncertainty
    /// they each report. An uncertainty several times too large is not
    /// caution - it dashes readings that are good and weighs every device's
    /// crystal estimate in the census wrongly - and one too small is a claim
    /// the measurement cannot pay for.
    #[test]
    fn the_offset_s_uncertainty_matches_its_scatter() {
        let addr = [0xAA, 0xBB, 0xCC, 0x11, 0x22, 0x33];
        let mut payload = crate::signal::ble::pdu::air_octets(addr).to_vec();
        payload.extend_from_slice(&[0x02, 0x01, 0x06]);
        let rate = working_rate_hz(Phy::OneM);
        let clean = synthetic_packet_iq(Phy::OneM, 37, 0x00, &payload, f64::INFINITY);
        for snr_db in [14.0, 16.0, 22.0] {
            let mut values = Vec::new();
            let mut sigmas = Vec::new();
            for seed in 0..40u64 {
                let mut iq = at_snr(&clean, snr_db, &mut Rng::new(1000 + seed));
                for (n, s) in iq.iter_mut().enumerate() {
                    let ph = std::f64::consts::TAU * 50_000.0 * n as f64 / rate;
                    *s *= Complex::new(ph.cos() as f32, ph.sin() as f32);
                }
                let geometry = eight_bit();
                let mut rx = Receiver::new(rate, 37, Phy::OneM).unwrap();
                if let Some(p) = rx.push(&bytes_for(&iq, geometry), geometry).first() {
                    if let Some(u) = p.freq_offset_hz {
                        values.push(u.value());
                        sigmas.push(u.sigma());
                    }
                }
            }
            assert!(
                values.len() >= 25,
                "{snr_db} dB: only {} of 40 decoded",
                values.len()
            );
            let m = values.len() as f64;
            let mean = values.iter().sum::<f64>() / m;
            let scatter =
                (values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (m - 1.0)).sqrt();
            let stated = sigmas.iter().sum::<f64>() / m;
            let ratio = stated / scatter;
            eprintln!(
                "{snr_db} dB: stated {stated:.0} Hz, scatter {scatter:.0} Hz, ratio {ratio:.2}"
            );
            assert!(
                (0.6..1.6).contains(&ratio),
                "{snr_db} dB: stated sigma {stated:.0} Hz against a scatter of {scatter:.0} Hz (ratio {ratio:.2})"
            );
            assert!(
                (mean - 50_000.0).abs() < 3.0 * scatter.max(stated),
                "{snr_db} dB: mean {mean:.0} Hz"
            );
        }
    }

    /// The detector threshold is `SHAPE_Z` standard deviations of a noise
    /// statistic whose spread, `1 / N_eff`, was measured - so it is measured
    /// again here. Complex Gaussian noise through each PHY's own front end,
    /// discriminator and template: the variance must match the constant the
    /// threshold is built from, and the tail at four standard deviations must
    /// look like the normal one the threshold extrapolates. A front-end change
    /// that moves either fails here instead of quietly changing what the
    /// threshold means.
    #[test]
    fn the_shape_detector_s_noise_statistics_are_the_ones_measured() {
        for (phy, raw_rate) in [(Phy::OneM, 4e6), (Phy::TwoM, 8e6)] {
            let reference = matched_reference(raw_rate, phy).unwrap();
            let template = frequency_template(&reference, phy);
            let mut rng = Rng::new(17);
            let raw: Vec<Complex<f32>> = (0..400_000)
                .map(|_| {
                    let (a, b) = rng.normal_pair();
                    Complex::new(a as f32, b as f32)
                })
                .collect();
            let mut working = Vec::new();
            front_end(raw_rate, phy)
                .unwrap()
                .process(&raw, &mut working);
            let mut track = Vec::new();
            discriminate(&working, working_rate_hz(phy), &mut track);
            let mut matcher = ShapeMatcher::new(&template);
            let mut out = Vec::new();
            matcher.process_block(&track, &mut out);
            let rho: Vec<f64> = out.into_iter().flatten().collect();
            let n = rho.len() as f64;
            let mean = rho.iter().sum::<f64>() / n;
            let var = rho.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / n;
            let measured = 1.0 / var;
            let stated = shape_n_eff(phy);
            assert!(
                (measured / stated - 1.0).abs() < 0.1,
                "{phy:?}: N_eff measured {measured:.1}, the threshold assumes {stated}"
            );
            let four_sigma = 4.0 / stated.sqrt();
            let exceed = rho.iter().filter(|&&r| r > four_sigma).count() as f64 / n;
            // One-sided normal tail at 4 sigma.
            let normal = 3.17e-5;
            assert!(
                (0.3..3.0).contains(&(exceed / normal)),
                "{phy:?}: {exceed:.2e} above 4 sigma against the normal {normal:.2e}"
            );
        }
    }

    /// B17's own exit condition: the identical chain, on LE 2M, at twice
    /// the symbol rate and twice the preamble length - design section
    /// 1.2's own "nothing new except the numbers" - decodes a real ADV_IND
    /// whole. Not a second, hand-duplicated test: [`synthetic_packet_iq`],
    /// [`Receiver::new`] and everything downstream take `phy` as data.
    #[test]
    fn a_synthetic_adv_ind_is_received_whole_on_le_2m() {
        let addr = [0xAA, 0xBB, 0xCC, 0x11, 0x22, 0x33];
        let mut payload = crate::signal::ble::pdu::air_octets(addr).to_vec();
        payload.extend_from_slice(&[0x02, 0x01, 0x06]);
        let iq = synthetic_packet_iq(Phy::TwoM, 37, 0x00, &payload, 20.0);
        let geometry = eight_bit();
        let bytes = bytes_for(&iq, geometry);

        let mut rx = Receiver::new(working_rate_hz(Phy::TwoM), 37, Phy::TwoM).unwrap();
        let packets = rx.push(&bytes, geometry);
        assert_eq!(packets.len(), 1, "expected exactly one packet");
        let p = &packets[0];
        assert_eq!(p.pdu_type, pdu::PduType::AdvInd);
        assert_eq!(p.adv_addr, Some(addr));
        assert!(p.crc_ok);
    }

    /// The same packet, fed one byte at a time - the shape a real capture
    /// actually arrives in, blocks with no relation to a packet's own
    /// boundaries.
    #[test]
    fn a_packet_split_across_many_small_blocks_still_arrives() {
        let addr = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06];
        let payload = crate::signal::ble::pdu::air_octets(addr).to_vec();
        let iq = synthetic_packet_iq(Phy::OneM, 37, 0x02, &payload, 20.0); // ADV_NONCONN_IND
        let geometry = eight_bit();
        let bytes = bytes_for(&iq, geometry);

        let mut rx = Receiver::new(working_rate_hz(Phy::OneM), 37, Phy::OneM).unwrap();
        let mut found = Vec::new();
        for chunk in bytes.chunks(6) {
            found.extend(rx.push(chunk, geometry));
        }
        assert_eq!(found.len(), 1);
        assert!(found[0].crc_ok);
        assert_eq!(found[0].adv_addr, Some(addr));
    }

    /// A capture with nothing in it produces nothing - noise alone must not
    /// manufacture a packet.
    #[test]
    fn noise_alone_produces_no_packets() {
        let mut rng = Rng::new(3);
        let noise = rng.noise(200_000, 1.0);
        let geometry = eight_bit();
        let bytes = bytes_for(&noise, geometry);
        let mut rx = Receiver::new(working_rate_hz(Phy::OneM), 37, Phy::OneM).unwrap();
        assert!(rx.push(&bytes, geometry).is_empty());
    }

    /// A sample rate below the working rate is refused, not silently
    /// mis-scaled.
    #[test]
    fn a_sample_rate_below_the_working_rate_is_refused() {
        assert!(front_end(2_000_000.0, Phy::OneM).is_err());
    }

    /// A sample rate that decimates cleanly to the working rate is accepted,
    /// at a few realistic HackRF rates.
    #[test]
    fn clean_multiples_of_the_working_rate_are_accepted() {
        for rate in [4_000_000.0, 8_000_000.0, 20_000_000.0] {
            assert!(
                front_end(rate, Phy::OneM).is_ok(),
                "rate {rate} should be accepted"
            );
        }
    }

    /// LE 2M needs twice LE 1M's own minimum sample rate - the same
    /// refusal shape, at the doubled working rate.
    #[test]
    fn le_2m_needs_twice_the_minimum_sample_rate() {
        assert!(front_end(4_000_000.0, Phy::TwoM).is_err());
        assert!(front_end(8_000_000.0, Phy::TwoM).is_ok());
    }

    /// A real capture at 20 Msps, decimated down, still finds the packet -
    /// the one test in this module that exercises [`front_end`]'s own filter
    /// rather than assuming the working rate directly.
    #[test]
    fn a_packet_survives_decimation_from_a_wider_capture_rate() {
        let addr = [0x10, 0x20, 0x30, 0x40, 0x50, 0x60];
        let mut payload = crate::signal::ble::pdu::air_octets(addr).to_vec();
        payload.push(0xFF);
        let raw_rate = 20_000_000.0;
        let sps = (raw_rate / Phy::OneM.symbol_rate_hz()) as usize;
        let mut bits = super::super::detect::preamble_bits(ADVERTISING_ACCESS_ADDRESS, Phy::OneM);
        bits.extend_from_slice(&super::super::detect::access_address_bits(
            ADVERTISING_ACCESS_ADDRESS,
        ));
        bits.extend_from_slice(&pdu::encode(38, 0x00, &payload));
        // See `synthetic_packet_iq`'s own comment: a real stream keeps going
        // past a packet's last bit, and the decimating filter's own warm-up
        // needs a few dozen more samples of margin besides.
        let mut rng = Rng::new(4321);
        bits.extend((0..64).map(|_| rng.next_u64() & 1 == 1));
        let clean = modulate(&bits, sps, Phy::OneM.deviation_hz(), raw_rate, 0.5);
        let noisy = at_snr(&clean, 25.0, &mut Rng::new(7));
        let geometry = eight_bit();
        let bytes = bytes_for(&noisy, geometry);

        let mut rx = Receiver::new(raw_rate, 38, Phy::OneM).unwrap();
        let packets = rx.push(&bytes, geometry);
        assert_eq!(packets.len(), 1, "expected exactly one packet");
        assert!(packets[0].crc_ok);
        assert_eq!(packets[0].adv_addr, Some(addr));
    }

    /// `front_end`'s anti-alias filter's own exit condition: a second,
    /// unrelated GFSK burst riding in the same wideband capture at a raw
    /// offset this front end's own decimate-by-5 folds straight onto DC (8
    /// MHz, a whole multiple of the 4 MHz working rate) must not defeat
    /// detection of the wanted packet. This is the failure a real HackRF
    /// session found after B9 - a flood of false triggers on channel 37 with
    /// no two decoding to consistent fields - that no earlier synthetic test
    /// exercised, because every one of them put exactly one signal in the
    /// capture.
    #[test]
    fn a_strong_out_of_channel_interferer_no_longer_defeats_detection() {
        let addr = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
        let mut payload = crate::signal::ble::pdu::air_octets(addr).to_vec();
        payload.push(0x01);
        let raw_rate = 20_000_000.0;
        let sps = (raw_rate / Phy::OneM.symbol_rate_hz()) as usize;
        let mut bits = super::super::detect::preamble_bits(ADVERTISING_ACCESS_ADDRESS, Phy::OneM);
        bits.extend_from_slice(&super::super::detect::access_address_bits(
            ADVERTISING_ACCESS_ADDRESS,
        ));
        bits.extend_from_slice(&pdu::encode(37, 0x00, &payload));
        let mut rng = Rng::new(555);
        bits.extend((0..64).map(|_| rng.next_u64() & 1 == 1));
        let wanted = modulate(&bits, sps, Phy::OneM.deviation_hz(), raw_rate, 0.5);

        // An unrelated burst, same shape and same amplitude as the wanted
        // signal - equal-power interference is the harder case, not a
        // softened one - carrying its own random content over the same
        // span, then mixed up to the aliasing offset.
        let mut irng = Rng::new(9001);
        let interferer_bits: Vec<bool> = (0..wanted.len() / sps)
            .map(|_| irng.next_u64() & 1 == 1)
            .collect();
        let interferer_baseband = modulate(
            &interferer_bits,
            sps,
            Phy::OneM.deviation_hz(),
            raw_rate,
            0.5,
        );
        const INTERFERER_OFFSET_HZ: f64 = 8_000_000.0;
        let mixed: Vec<Complex<f32>> = wanted
            .iter()
            .zip(interferer_baseband.iter())
            .enumerate()
            .map(|(n, (&w, &i))| {
                let phase = 2.0 * std::f64::consts::PI * INTERFERER_OFFSET_HZ * n as f64 / raw_rate;
                let rot = Complex::new(phase.cos() as f32, phase.sin() as f32);
                // Each half amplitude, so the combined peak sits where the
                // single-signal tests already do rather than clipping
                // `bytes_for`'s own eight-bit scale in a way that would
                // confound saturation with the aliasing this test targets.
                w * 0.5 + i * rot * 0.5
            })
            .collect();
        let noisy = at_snr(&mixed, 20.0, &mut Rng::new(77));
        let geometry = eight_bit();
        let bytes = bytes_for(&noisy, geometry);

        let mut rx = Receiver::new(raw_rate, 37, Phy::OneM).unwrap();
        let packets = rx.push(&bytes, geometry);
        assert_eq!(
            packets.len(),
            1,
            "expected exactly one packet despite the interferer"
        );
        assert!(packets[0].crc_ok);
        assert_eq!(packets[0].adv_addr, Some(addr));
    }

    /// A block carried through `StreamBlock`, the real shape blocks arrive
    /// in from the worker - not load-bearing for the decode itself, just
    /// that the geometry plumbing is the same type the real worker uses.
    #[test]
    fn the_block_shape_matches_what_the_worker_hands_it() {
        let geometry = eight_bit();
        let block = StreamBlock {
            seq: 1,
            gap_before: false,
            bytes: vec![0u8; 64],
            first_pair: 0,
            centre_hz: 2_426_000_000,
            rate_hz: working_rate_hz(Phy::OneM),
        };
        let mut rx = Receiver::new(
            working_rate_hz(Phy::OneM),
            channel::channel_of(2_426_000_000).unwrap(),
            Phy::OneM,
        )
        .unwrap();
        assert!(rx.push(&block.bytes, geometry).is_empty());
    }
}
