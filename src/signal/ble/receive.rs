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

#[cfg(test)]
use crate::hardware::SampleGeometry;
#[cfg(test)]
use crate::signal::demod::decode as decode_iq;
use crate::signal::dsp::code::lfsr::whiten;
use crate::signal::dsp::correlate::ShapeMatcher;
use crate::signal::dsp::discriminate::{discriminate, instantaneous_freq_hz, Oversampled};
use crate::signal::dsp::estimate::snr_from_metric;
use crate::signal::dsp::fir::{design_lowpass_to_spec, StreamingDecimator};
use crate::signal::dsp::nco::Nco;

use super::data::{self, DataPdu};
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
        // LE Coded never reaches this receiver ([`front_end`] refuses it).
        Phy::OneM | Phy::Coded(_) => 107.8,
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
        Phy::OneM | Phy::Coded(_) => 1_500_000.0,
        Phy::TwoM => 3_000_000.0,
    }
}
/// Transition width, in Hz, either side of the cutoff.
fn anti_alias_transition_hz(phy: Phy) -> f64 {
    match phy {
        Phy::OneM | Phy::Coded(_) => 500_000.0,
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
    // LE Coded has a chain of its own (`coded_rx`), with a filter chosen for
    // its sensitivity; this one is LE 1M's and LE 2M's.
    if matches!(phy, Phy::Coded(_)) {
        return Err("LE Coded is received by its own chain, not this one".to_string());
    }
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

fn matched_reference(
    raw_rate: f64,
    phy: Phy,
    access_address: u32,
) -> Result<Vec<Complex<f32>>, String> {
    let sps = (raw_rate / phy.symbol_rate_hz()).round().max(1.0) as usize;
    let sample_rate = sps as f64 * phy.symbol_rate_hz();
    let mut sync_bits = preamble_bits(access_address, phy);
    sync_bits.extend_from_slice(&access_address_bits(access_address));

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

/// Which packets a receiver is for: the advertising channels' own, or one
/// connection's, by the access address and CRC initial value its
/// CONNECT_IND set (Core 5.4 Vol 6 Part B 2.1.2, 3.1.1). The whitening is
/// the channel index's either way.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Link {
    Advertising,
    /// Secondary advertising, where an AuxPtr points: the advertising
    /// access address, and an extended PDU's 255 octets (2.3.4).
    Auxiliary,
    Data {
        access_address: u32,
        crc_init: u32,
    },
}

impl Link {
    fn access_address(self) -> u32 {
        match self {
            Link::Advertising | Link::Auxiliary => ADVERTISING_ACCESS_ADDRESS,
            Link::Data { access_address, .. } => access_address,
        }
    }

    /// The longest a PDU on this link can be, header through CRC, in bits.
    fn longest_pdu_bits(self) -> usize {
        match self {
            Link::Advertising => MAX_PDU_BYTES * 8,
            Link::Auxiliary => (2 + 255 + 3) * 8,
            // Header, CTEInfo and 255 octets of payload and MIC (2.4).
            Link::Data { .. } => data::HEADER_BITS + 8 + 255 * 8 + data::CRC_BITS,
        }
    }
}

/// Where a data channel packet sits in the stream, in I/Q pairs on the
/// radio's own clock, fractional: from the start of its preamble's first bit
/// to the end of its CRC's last, the two instants the Inter Frame Space is
/// measured between (Core 5.4 Vol 6 Part B 4.1.1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DataTiming {
    pub start_pair: f64,
    pub end_pair: f64,
}

/// One alignment's decode, on whichever link the receiver is for.
#[derive(Clone, Debug)]
enum Heard {
    Advertising(Box<Packet>),
    Data(DataPdu, Option<DataTiming>),
}

impl Heard {
    fn crc_ok(&self) -> bool {
        match self {
            Heard::Advertising(p) => p.crc_ok,
            Heard::Data(d, _) => d.crc_ok,
        }
    }
}

/// One channel's live receiver: the decimator, the sync-word detector, and
/// the capture in progress, if any.
pub struct Receiver {
    /// Brings the channel to baseband when the radio is not tuned exactly to
    /// it; `None` when it is. `channel::channel_of` accepts a tuning up to a
    /// quarter of the channel spacing away, and a survey dwell at `x.500 MHz`
    /// sits right at that edge: without this, the half megahertz between the
    /// tuning and the channel was read as the transmitter's own carrier
    /// offset, 200 ppm on every device, and the deviation it widened as a
    /// modulation index near 1.
    mixer: Option<Nco>,
    tuned_centre_hz: f64,
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
    /// The capture through the discriminator, grown as the capture grows:
    /// a reading depends on two samples only, so the ones already made
    /// never change. A capture is tried every octet, and discriminating the
    /// whole of it each time made the work grow with the square of the
    /// packet.
    inst: Vec<f32>,
    /// `inst_sums[i]` is the sum of the first `i` readings: each candidate's
    /// threshold, the mean from its start, as one difference.
    inst_sums: Vec<f64>,
    /// The phase search over the capture so far (`sync::growing`), grown
    /// with it.
    search: crate::signal::dsp::timing::PhaseSearch,
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
    /// The stream position of the first sample this receiver was given: the
    /// origin of the decimator's own output stream.
    first_pair: Option<u64>,
    /// Working samples produced before the current block, since the first.
    worked: u64,
    /// The index, counted as [`Self::worked`] counts, of `capture[0]`: with
    /// the decimator's delay and factor, where any capture sample sits in
    /// the stream exactly (`pdu::Packet::pdu_pair`).
    capture_origin: u64,
    /// Whose packets it hears.
    link: Link,
    /// Data channel packets heard since the caller last took them
    /// ([`Self::take_data`]).
    data: Vec<(DataPdu, DataTiming)>,
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

    /// Put the receiver down: the funnel since the last [`Self::take_funnel`],
    /// with a capture under way counted as given up, so every trigger ends
    /// somewhere even when the receiver does not.
    pub fn finish(mut self) -> Funnel {
        if self.capturing {
            self.funnel.gave_up += 1;
        }
        self.funnel
    }

    /// A receiver for `channel` on `phy`, with the radio at `raw_rate` and
    /// tuned to `tuned_centre_hz`, which need not be the channel's own centre.
    pub fn new(raw_rate: f64, channel: u8, phy: Phy, tuned_centre_hz: f64) -> Result<Self, String> {
        Self::for_link(raw_rate, channel, phy, tuned_centre_hz, Link::Advertising)
    }

    /// [`Self::new`] for `link`'s packets: a data channel receiver listens
    /// for its connection's access address and checks its CRC from the
    /// connection's initial value.
    pub fn for_link(
        raw_rate: f64,
        channel: u8,
        phy: Phy,
        tuned_centre_hz: f64,
        link: Link,
    ) -> Result<Self, String> {
        let channel_hz = super::channel::centre_hz(channel)
            .ok_or_else(|| format!("BLE channel {channel} does not exist"))?;
        // The channel sits at `+offset` in the raw stream, so the oscillator
        // runs at `-offset` to bring it to zero.
        let offset_hz = channel_hz as f64 - tuned_centre_hz;
        let mixer = (offset_hz.abs() >= 1.0).then(|| Nco::new(-offset_hz, raw_rate));
        let decim = front_end(raw_rate, phy)?;
        let reference = matched_reference(raw_rate, phy, link.access_address())?;
        let shape = frequency_template(&reference, phy);
        let reference_energy = reference.iter().map(|s| s.norm_sqr() as f64).sum();
        Ok(Self {
            mixer,
            tuned_centre_hz,
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
            inst: Vec::new(),
            inst_sums: vec![0.0],
            search: super::sync::growing(WORKING_SPS as f64),
            capturing: false,
            funnel: Funnel::default(),
            raw_per_working: raw_rate / working_rate_hz(phy),
            next_pair: 0,
            trigger_pair: 0,
            first_pair: None,
            worked: 0,
            capture_origin: 0,
            link,
            data: Vec::new(),
        })
    }

    /// Forget the stream, keep the filters and the reference: for a window
    /// that does not follow on from the last one, as a connection event's
    /// does not. The next sample pushed is read as if it were the first.
    pub fn reset(&mut self) {
        if let Some(mixer) = self.mixer.as_mut() {
            mixer.reset();
        }
        self.decim.reset();
        self.shape.reset();
        self.last_sample = None;
        self.recent.clear();
        self.sync_window.clear();
        self.sync_rho = 0.0;
        self.trigger_len = 0;
        self.history.clear();
        self.capture.clear();
        self.capturing = false;
        self.inst.clear();
        self.inst_sums = vec![0.0];
        self.search = super::sync::growing(WORKING_SPS as f64);
        self.next_pair = 0;
        self.trigger_pair = 0;
        self.first_pair = None;
        self.worked = 0;
        self.capture_origin = 0;
    }

    /// The data channel packets heard since the last call, with where each
    /// sits in the stream. Always empty on an advertising receiver.
    pub fn take_data(&mut self) -> Vec<(DataPdu, DataTiming)> {
        std::mem::take(&mut self.data)
    }

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

    /// Whether this receiver is still the right one: a change of channel,
    /// sample rate, PHY or tuning each invalidates the mixer, the reference or
    /// the capture in progress, so the caller rebuilds rather than reusing.
    pub fn matches(&self, channel: u8, raw_rate: f64, phy: Phy, tuned_centre_hz: f64) -> bool {
        self.channel == channel
            && (self.raw_rate - raw_rate).abs() < 1.0
            && self.phy == phy
            && (self.tuned_centre_hz - tuned_centre_hz).abs() < 1.0
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
    #[cfg(test)]
    pub fn push_at(
        &mut self,
        bytes: &[u8],
        geometry: SampleGeometry,
        first_pair: u64,
    ) -> Vec<Packet> {
        let mut iq = Vec::new();
        decode_iq(bytes, geometry, usize::MAX, &mut iq);
        self.push_iq_at(&iq, first_pair)
    }

    /// [`Self::push_at`] on a block already decoded: the worker decodes each
    /// block once and hands the same samples to every receiver, where each
    /// used to decode its own copy of the same bytes.
    pub fn push_iq_at(&mut self, iq: &[Complex<f32>], first_pair: u64) -> Vec<Packet> {
        let mixed;
        let iq = match self.mixer.as_mut() {
            Some(mixer) => {
                let mut out = Vec::new();
                mixer.mix_into(iq, &mut out);
                mixed = out;
                &mixed[..]
            }
            None => iq,
        };
        let mut working = Vec::new();
        self.decim.process(iq, &mut working);

        let cap_limit = (16 + self.link.longest_pdu_bits()) * WORKING_SPS;
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
        self.first_pair.get_or_insert(first_pair);
        let worked = self.worked;
        self.worked += working.len() as u64;
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
                    Some(heard) => {
                        self.funnel.decoded += 1;
                        self.keep(heard, &mut found);
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
                            Some(heard) => {
                                if heard.crc_ok() {
                                    self.funnel.decoded += 1;
                                } else {
                                    self.funnel.crc_failed += 1;
                                }
                                self.keep(heard, &mut found);
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
                    self.inst.clear();
                    self.inst_sums.clear();
                    self.inst_sums.push(0.0);
                    self.search = super::sync::growing(WORKING_SPS as f64);
                    self.trigger_len = self.capture.len();
                    // The history ends with this sample.
                    self.capture_origin = worked + j as u64 + 1 - self.trigger_len as u64;
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
    fn try_decode(&mut self) -> Option<Heard> {
        self.candidates()
            .into_iter()
            .find(|(.., heard)| heard.crc_ok())
            .map(|(skip, phase, heard)| self.measured(skip, phase, heard))
    }

    /// A finished capture's packet to where it belongs: an advertising one
    /// to the caller, stamped with its trigger, a data one to
    /// [`Self::take_data`].
    fn keep(&mut self, heard: Heard, found: &mut Vec<Packet>) {
        match heard {
            Heard::Advertising(mut packet) => {
                packet.at_pair = Some(self.trigger_pair);
                found.push(*packet);
            }
            Heard::Data(pdu, Some(timing)) => self.data.push((pdu, timing)),
            // Not placed in the stream is not heard: nothing could time it.
            Heard::Data(_, None) => {}
        }
    }

    /// Every alignment [`Self::try_decode`] searches, in order, decoded: the
    /// header start it was read from, the phase it was sliced at, and the
    /// packet, CRC passed or not, not yet measured ([`Self::measured`]).
    fn candidates(&mut self) -> Vec<(usize, f64, Heard)> {
        let center = LOOKBACK_SAMPLES as isize;
        let step = WORKING_SPS as isize;
        let span = HEADER_SEARCH_SYMBOLS as isize;
        // One discriminator pass for every candidate (see `decode_at`),
        // grown by what arrived since the last try.
        self.grow();
        // And one phase search. The candidates are whole symbols apart, so
        // they share where inside a symbol to sample; searching from the
        // earliest one uses every symbol any of them will read. Thirteen
        // searches, each over nearly the same samples, were most of what a
        // capture that never passed its CRC cost; one search from scratch
        // on every try was most of what was left.
        let earliest = (center - span * step).max(0) as usize;
        let from_earliest = &self.inst[earliest.min(self.inst.len())..];
        let phase = self
            .search
            .phase(from_earliest, from_earliest.len() / WORKING_SPS);
        let mut out = Vec::new();
        // From the nominal boundary outwards, where the right one nearly
        // always is: each alignment that decodes costs a slice of the whole
        // packet, and at most one passes its CRC, so the order only decides
        // how many are sliced before it.
        let order = (0..=span).flat_map(|d| if d == 0 { vec![0] } else { vec![-d, d] });
        for k in order {
            let skip = center + k * step;
            if skip < 0 {
                continue;
            }
            let skip = skip as usize;
            if skip >= self.inst.len() {
                continue;
            }
            // The capture's own mean from here on: see `decode_at`.
            let n = self.inst.len();
            let mean = (self.inst_sums[n] - self.inst_sums[skip]) / (n - skip) as f64;
            if let Some(heard) = self.decode_at(&self.inst, skip, phase, mean as f32) {
                let passed = heard.crc_ok();
                out.push((skip, phase, heard));
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
    fn best_failed_candidate(&mut self) -> Option<Heard> {
        let end = energy_end(&self.capture, LOOKBACK_SAMPLES)?;
        let tolerance = END_TOLERANCE_SYMBOLS * WORKING_SPS;
        self.candidates()
            .into_iter()
            .filter_map(|(skip, phase, heard)| {
                let bits = match &heard {
                    Heard::Advertising(p) => pdu::used_bits(p.length),
                    Heard::Data(d, _) => {
                        data::HEADER_BITS
                            + 8 * d.cte_info.is_some() as usize
                            + d.payload.len() * 8
                            + data::CRC_BITS
                    }
                };
                let ends = skip + bits * WORKING_SPS;
                let off = ends.abs_diff(end);
                (off <= tolerance).then_some((off, skip, phase, heard))
            })
            .min_by_key(|(off, ..)| *off)
            .map(|(_, skip, phase, heard)| self.measured(skip, phase, heard))
    }

    /// The discriminator over the capture, extended to its end.
    fn grow(&mut self) {
        let rate = working_rate_hz(self.phy);
        for i in self.inst.len()..self.capture.len().saturating_sub(1) {
            let f = instantaneous_freq_hz(self.capture[i], self.capture[i + 1], rate);
            self.inst.push(f);
            let total = self.inst_sums[self.inst_sums.len() - 1] + f as f64;
            self.inst_sums.push(total);
        }
    }

    /// The decode at one candidate header start, `skip` samples into the
    /// capture, given the whole capture already discriminated.
    ///
    /// **The discriminator of a capture started `skip` samples in is exactly
    /// the whole capture's discriminator from `skip` on** - each reading is a
    /// function of two neighbouring samples and nothing else - so the search
    /// over candidate boundaries slices one pass rather than making thirteen.
    ///
    /// `phase` is the sub-symbol sampling phase found for the capture (see
    /// `candidates`), and `threshold` the mean of the readings from `skip`
    /// on.
    fn decode_at(&self, whole: &[f32], skip: usize, phase: f64, threshold: f32) -> Option<Heard> {
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
        let sps = WORKING_SPS as f64;
        // The header first, alone. Most attempts come before the packet has
        // finished arriving, and its length says so from sixteen bits;
        // slicing and decoding the rest only to find it short was most of
        // what a busy channel cost. The same bits either way - at a fixed
        // phase and threshold each symbol is sliced on its own - so this
        // only skips work whose answer was already `None`.
        let (mut header, _) = super::sync::slice_at(inst, sps, pdu::HEADER_BITS, threshold, phase);
        whiten(&mut header, self.channel);
        let wanted = match self.link {
            Link::Advertising | Link::Auxiliary => pdu::used_bits(pdu::length(&header)?),
            Link::Data { .. } => data::used_bits(&header)?,
        };
        if wanted > symbols {
            return None;
        }
        // The packet's own bits, not the whole capture: a candidate whose
        // header reads a short length would otherwise slice everything
        // captured so far on every try, which a busy channel paid for with
        // the square of the capture. `decode` reads no further than this.
        let (mut bits, _) = super::sync::slice_at(inst, sps, wanted, threshold, phase);
        // The modulation-quality measurement needs the physically
        // transmitted (still-whitened) symbols - exactly what `bits` is
        // before the next line undoes whitening to recover the data
        // underneath them.
        // See `measure`'s own module doc for why the physical bits, not the
        // decoded ones, are what a Gaussian filter's settling depends on.
        let raw_bits = bits.clone();
        whiten(&mut bits, self.channel);
        // Where the PDU's first bit is centred, in pairs: `inst[i]` stands
        // for capture instant `i + 0.5`, and working sample `w` for raw
        // instant `delay + w * d` after the receiver's first.
        let pdu_pair = self.first_pair.map(|first| {
            let working = self.capture_origin as f64 + skip as f64 + phase + 0.5;
            first as f64 + self.decim.delay() + working * self.decim.factor() as f64
        });
        if let Link::Data { crc_init, .. } = self.link {
            let pdu = data::decode(&bits, crc_init)?;
            // A bit is WORKING_SPS working samples; back half a bit to its
            // start, then over the access address and the preamble.
            let bit = (WORKING_SPS * self.decim.factor()) as f64;
            let timing = pdu_pair.map(|centre| {
                let first_bit = centre - 0.5 * bit;
                let sync = (self.phy.preamble_bits_len() + 32) as f64;
                DataTiming {
                    start_pair: first_bit - sync * bit,
                    end_pair: first_bit + wanted as f64 * bit,
                }
            });
            return Some(Heard::Data(pdu, timing));
        }
        let mut packet = pdu::decode(&bits)?;
        // Exactly this packet's own bits: the capture runs on past it
        // (see this struct's own `push`), and letting a measurement wander
        // into trailing noise or the next packet's preamble would mix an
        // unrelated signal's deviation into this one's own reading.
        let used = pdu::used_bits(packet.length).min(raw_bits.len());
        let raw_bits = &raw_bits[..used];
        // Where the PDU sits in the stream, for the measurement to find it
        // again in the raw samples: `inst[i]` stands for capture instant
        // `i + 0.5`, and working sample `w` for raw instant `delay + w * d`
        // after the receiver's first.
        packet.air = raw_bits.to_vec();
        packet.pdu_pair = pdu_pair;
        Some(Heard::Advertising(Box::new(packet)))
    }

    /// `packet`, decoded at `skip` and `phase`, with its offset, SNR, modulation and drift
    /// read: once, for the packet the search settled on, never for the
    /// alignments it tried on the way. Building the rebuilt waveform for
    /// every alignment that decoded, CRC or not, on every attempt while a
    /// packet was still arriving, put a clean channel at 35 times real time.
    ///
    /// The figures are read on the slicer's own bit grid, but from the
    /// rebuilt waveform, not from a straight line between two readings a
    /// quarter of a symbol apart (`Oversampled`'s doc for what that cost),
    /// and as the test suites define them (`dsp::deviation::suite_readings`).
    /// The slicer keeps the plain readings: a bit is decided by which side
    /// of the line it falls, and that the chord gets right. `inst[i]` sits
    /// halfway between capture samples `i` and `i + 1`, scaled by the PHY's
    /// own symbol rate.
    ///
    /// **The modulation and drift on LE 2M only.** LE 1M is read by the
    /// worker from the raw samples
    /// (`signal::net::measure`), through the measurement filter and timed by
    /// the packet's known bits, and reading it here as well built the
    /// rebuilt waveform twice for every packet, a fifth of what a busy
    /// channel cost, for figures the worker then replaced.
    fn measured(&self, skip: usize, phase: f64, heard: Heard) -> Heard {
        match heard {
            Heard::Advertising(packet) => {
                Heard::Advertising(Box::new(self.measured_advertising(skip, phase, *packet)))
            }
            // A data packet's figures are its timing, already read.
            data @ Heard::Data(..) => data,
        }
    }

    fn measured_advertising(&self, skip: usize, phase: f64, mut packet: Packet) -> Packet {
        // The *reported* offset is read against the sync word, not the
        // packet's data: see `sync_offset`. The slicing threshold never had
        // to be exact, only good enough to slice against; this one is what a
        // reader sees a `+/-` on. It and the SNR depend on the sync word
        // alone, not on the alignment: worked out for every alignment that
        // decoded, they were a sine and a cosine a sample of the sync word,
        // many times a packet.
        let offset = self.sync_offset();
        packet.freq_offset_hz = offset;
        packet.snr_db = offset.and_then(|o| self.corrected_snr_db(o.value()));
        if self.phy == Phy::OneM {
            return packet;
        }
        let fine = Oversampled::new(&self.capture, working_rate_hz(self.phy));
        let sps = WORKING_SPS as f64;
        // Bit `x` periods into the PDU, as a capture instant: bit `k`'s
        // centre, `k + 0.5`, is where the slicer sampled it.
        let at = |x: f64| fine.at(skip as f64 + phase + (x - 0.5) * sps + 0.5);
        // Every bit read once, for the modulation and the carrier both.
        let readings = crate::signal::dsp::deviation::BitReadings::read(packet.air.len(), at);
        packet.modulation =
            crate::signal::dsp::deviation::suite_readings_from(&packet.air, &readings).and_then(
                |(settled, alternating)| {
                    super::measure::modulation_from(&settled, &alternating, self.phy)
                },
            );
        // The preamble is not in the capture: the first block stands for f0.
        let carrier = crate::signal::dsp::carrier::by_bit_from(&packet.air, &readings);
        let crc_start = packet.air.len().saturating_sub(pdu::CRC_BITS);
        let blocks = crate::signal::dsp::carrier::ten_bit_blocks(&carrier, 1, crc_start);
        packet.drift = super::measure::drift_from(None, &blocks, self.phy);
        packet
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::hardware::{SampleFormat, StreamBlock};
    use crate::signal::ble::channel;
    use crate::signal::ble::data::DataPdu;
    use crate::signal::ble::gfsk::modulate;
    use crate::signal::dsp::testkit::{at_snr, Rng};

    /// A receiver with the radio tuned exactly to the channel: what every test
    /// here means unless it says otherwise.
    fn centred(raw_rate: f64, ch: u8, phy: Phy) -> Result<Receiver, String> {
        let tuned = channel::centre_hz(ch).expect("a real channel") as f64;
        Receiver::new(raw_rate, ch, phy, tuned)
    }

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

    /// A data channel packet for one link, at the radio's own rate with its
    /// channel at its offset from `tuned_hz`: sixteen random bits of lead-in
    /// after `lead` pairs of silence, the preamble `aa` calls for, `aa`, the
    /// PDU CRC'd under `crc_init` and whitened for `ch`, a short tail. Returns
    /// the samples and where the preamble's first bit starts, in pairs.
    pub(crate) fn synthetic_data_burst(
        raw_rate: f64,
        ch: u8,
        tuned_hz: f64,
        aa: u32,
        crc_init: u32,
        pdu: &DataPdu,
        lead: usize,
    ) -> (Vec<Complex<f32>>, f64) {
        let sps = (raw_rate / 1e6).round() as usize;
        let mut rng = Rng::new(4242);
        let mut bits: Vec<bool> = (0..16).map(|_| rng.next_u64() & 1 == 1).collect();
        bits.extend(super::super::detect::preamble_bits(aa, Phy::OneM));
        bits.extend_from_slice(&super::super::detect::access_address_bits(aa));
        bits.extend(super::super::data::encode(pdu, crc_init, ch));
        bits.extend((0..16).map(|_| rng.next_u64() & 1 == 1));
        let base = modulate(&bits, sps, Phy::OneM.deviation_hz(), raw_rate, 0.5);
        let offset = channel::centre_hz(ch).unwrap() as f64 - tuned_hz;
        let mut iq = vec![Complex::new(0.0f32, 0.0); lead];
        iq.extend(base.iter().enumerate().map(|(n, s)| {
            let ph = std::f64::consts::TAU * offset * (lead + n) as f64 / raw_rate;
            s * Complex::new(ph.cos() as f32, ph.sin() as f32)
        }));
        let noisy = at_snr(&iq[lead..], 25.0, &mut Rng::new(99));
        iq.truncate(lead);
        iq.extend(noisy);
        iq.extend(vec![Complex::new(0.0, 0.0); 2000]);
        (iq, (lead + 16 * sps) as f64)
    }

    pub(crate) fn data_pdu(llid: u8, payload: &[u8]) -> DataPdu {
        DataPdu {
            llid,
            nesn: false,
            sn: true,
            md: false,
            cte_info: None,
            payload: payload.to_vec(),
            crc_ok: true,
        }
    }

    const LINK_AA: u32 = 0x5065_4b6a;
    const LINK_CRC: u32 = 0x3a_5b7c;

    fn link_receiver(ch: u8) -> Receiver {
        Receiver::for_link(
            20e6,
            ch,
            Phy::OneM,
            2_426e6,
            Link::Data {
                access_address: LINK_AA,
                crc_init: LINK_CRC,
            },
        )
        .unwrap()
    }

    /// A data PDU on data channel 12 at 20 Msps, the radio at 2426 MHz,
    /// under the link's own access address: heard, CRC passing, its bytes
    /// as sent, and placed in time to within a quarter of a symbol at both
    /// ends (the two instants T_IFS is measured between).
    #[test]
    fn a_link_receiver_hears_its_own_access_address() {
        let sent = data_pdu(3, &[0x0c, 0x0c, 0x4c, 0x00, 0x34, 0x12]);
        let (iq, start) = synthetic_data_burst(20e6, 12, 2_426e6, LINK_AA, LINK_CRC, &sent, 1000);
        let mut rx = link_receiver(12);
        assert!(rx.push_iq_at(&iq, 0).is_empty(), "no advertising packets");
        let got = rx.take_data();
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].0, sent);
        // Preamble 8, address 32, header 16, payload, CRC 24, at 20 pairs a bit.
        let bits = 8 + 32 + 16 + 6 * 8 + 24;
        let end = start + bits as f64 * 20.0;
        assert!(
            (got[0].1.start_pair - start).abs() < 5.0,
            "{:?} vs {start}",
            got[0].1
        );
        assert!(
            (got[0].1.end_pair - end).abs() < 5.0,
            "{:?} vs {end}",
            got[0].1
        );
    }

    /// The advertising access address is not this link's: nothing heard,
    /// and the advertising receiver does not hear the link's either.
    #[test]
    fn a_link_receiver_hears_no_other_access_address() {
        let sent = data_pdu(1, &[]);
        let (iq, _) = synthetic_data_burst(
            20e6,
            12,
            2_426e6,
            ADVERTISING_ACCESS_ADDRESS,
            0x55_5555,
            &sent,
            1000,
        );
        let mut rx = link_receiver(12);
        rx.push_iq_at(&iq, 0);
        assert!(rx.take_data().is_empty());

        let (iq, _) = synthetic_data_burst(20e6, 38, 2_426e6, LINK_AA, LINK_CRC, &sent, 1000);
        let mut adv = Receiver::new(20e6, 38, Phy::OneM, 2_426e6).unwrap();
        assert!(adv.push_iq_at(&iq, 0).is_empty());
    }

    /// After `reset`, a window far later in the stream is read as if it
    /// were the first, and placed where it is.
    #[test]
    fn reset_forgets_the_stream() {
        let sent = data_pdu(2, &[1, 2, 3, 4]);
        let (iq, start) = synthetic_data_burst(20e6, 12, 2_426e6, LINK_AA, LINK_CRC, &sent, 1000);
        let mut rx = link_receiver(12);
        rx.push_iq_at(&iq, 0);
        rx.reset();
        rx.push_iq_at(&iq, 5_000_000);
        let got = rx.take_data();
        assert_eq!(got.len(), 2, "one from each window: {got:?}");
        let later = got[1].1.start_pair - 5_000_000.0;
        assert!((later - start).abs() < 5.0, "{:?}", got[1].1);
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

        let mut rx = centred(working_rate_hz(Phy::OneM), 37, Phy::OneM).unwrap();
        let packets = rx.push(&bytes, geometry);
        assert_eq!(packets.len(), 1, "expected exactly one packet");
        let p = &packets[0];
        assert_eq!(p.pdu_type, pdu::PduType::AdvInd);
        assert_eq!(p.adv_addr, Some(addr));
        assert!(p.crc_ok);
    }

    /// A receiver put down in the middle of a capture, as the worker does
    /// at a break, a retune or a closing view, still ends that trigger: it
    /// gave up, and the funnel it hands back says so.
    #[test]
    fn a_receiver_put_down_mid_capture_counts_it_given_up() {
        let payload: Vec<u8> = (0..31u8).collect();
        let iq = synthetic_packet_iq(Phy::OneM, 37, 0x02, &payload, 25.0);
        let geometry = eight_bit();
        let bytes = bytes_for(&iq, geometry);
        let mut rx = centred(working_rate_hz(Phy::OneM), 37, Phy::OneM).unwrap();
        // Through the sync word and into the payload, not to its end.
        let cut = (bytes.len() * 2 / 3) & !1;
        assert!(rx.push(&bytes[..cut], geometry).is_empty());
        let f = rx.finish();
        assert_eq!((f.triggered, f.gave_up), (1, 1), "{f:?}");
        assert_eq!(f.triggered, f.decoded + f.crc_failed + f.gave_up);
    }

    /// **An auxiliary packet is read whole.** Extended advertising carries
    /// up to 255 octets (2.3.4), where a legacy advertising PDU stops at 37:
    /// an `AUX_ADV_IND` of 100 octets on data channel 12 is received by the
    /// auxiliary link and not by the advertising one, which keeps its legacy
    /// limit so a corrupt length on a primary channel costs no longer
    /// capture.
    #[test]
    fn an_auxiliary_packet_longer_than_legacy_is_read_whole() {
        let payload: Vec<u8> = (0..100u8).collect();
        let iq = synthetic_packet_iq(Phy::OneM, 12, 0x07, &payload, 25.0);
        let geometry = eight_bit();
        let bytes = bytes_for(&iq, geometry);
        let rate = working_rate_hz(Phy::OneM);
        let tuned = channel::centre_hz(12).unwrap() as f64;

        let mut aux = Receiver::for_link(rate, 12, Phy::OneM, tuned, Link::Auxiliary).unwrap();
        let got = aux.push(&bytes, geometry);
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(got[0].crc_ok);
        assert_eq!(got[0].payload, payload);

        let mut legacy = Receiver::for_link(rate, 12, Phy::OneM, tuned, Link::Advertising).unwrap();
        assert!(legacy.push(&bytes, geometry).iter().all(|p| !p.crc_ok));
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

        let mut rx = centred(working_rate_hz(Phy::OneM), 37, Phy::OneM).unwrap();
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
        let mut rx = centred(rate, 37, Phy::OneM).unwrap();
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
        let mut rx = centred(rate, 37, Phy::OneM).unwrap();
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
        let mut rx = centred(rate, 37, Phy::OneM).unwrap();
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
                let mut rx = centred(rate, 37, Phy::OneM).unwrap();
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
            let mut rx = centred(rate, 37, Phy::OneM).unwrap();
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
            let mut rx = centred(rate, 37, Phy::OneM).unwrap();
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
                let mut rx = centred(rate, 37, Phy::OneM).unwrap();
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
            let reference = matched_reference(raw_rate, phy, ADVERTISING_ACCESS_ADDRESS).unwrap();
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

        let mut rx = centred(working_rate_hz(Phy::TwoM), 37, Phy::TwoM).unwrap();
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

        let mut rx = centred(working_rate_hz(Phy::OneM), 37, Phy::OneM).unwrap();
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
        let mut rx = centred(working_rate_hz(Phy::OneM), 37, Phy::OneM).unwrap();
        assert!(rx.push(&bytes, geometry).is_empty());
    }

    /// A channel half a megahertz off the tuning, where a survey dwell puts
    /// channel 37, reads the transmitter's own carrier offset and deviation,
    /// not the tuning's. Before the mixer, the same packet reported the half
    /// megahertz as a 200 ppm crystal.
    #[test]
    fn a_channel_off_the_tuned_centre_is_measured_from_its_own_centre() {
        let addr = [0x0A, 0x1B, 0x2C, 0x3D, 0x4E, 0x5F];
        let mut payload = crate::signal::ble::pdu::air_octets(addr).to_vec();
        payload.push(0xFF);
        let raw_rate = 8_000_000.0;
        let sps = (raw_rate / Phy::OneM.symbol_rate_hz()) as usize;
        let mut bits = super::super::detect::preamble_bits(ADVERTISING_ACCESS_ADDRESS, Phy::OneM);
        bits.extend_from_slice(&super::super::detect::access_address_bits(
            ADVERTISING_ACCESS_ADDRESS,
        ));
        bits.extend_from_slice(&pdu::encode(37, 0x00, &payload));
        let mut rng = Rng::new(8642);
        bits.extend((0..64).map(|_| rng.next_u64() & 1 == 1));
        let mut lead: Vec<bool> = (0..32).map(|_| rng.next_u64() & 1 == 1).collect();
        lead.extend(bits);
        let clean = modulate(&lead, sps, Phy::OneM.deviation_hz(), raw_rate, 0.5);
        let ch_hz = channel::centre_hz(37).unwrap() as f64;
        for offset_hz in [500_000.0, -500_000.0, 250_000.0] {
            // The radio is tuned `offset_hz` below the channel, so the packet
            // arrives `offset_hz` above zero.
            let mut placed = clean.clone();
            Nco::new(offset_hz, raw_rate).mix(&mut placed);
            let noisy = at_snr(&placed, 25.0, &mut Rng::new(11));
            let geometry = eight_bit();
            let bytes = bytes_for(&noisy, geometry);

            let mut rx = Receiver::new(raw_rate, 37, Phy::OneM, ch_hz - offset_hz).unwrap();
            let packets = rx.push(&bytes, geometry);
            assert_eq!(packets.len(), 1, "{offset_hz} Hz off: expected one packet");
            let p = &packets[0];
            assert!(p.crc_ok, "{offset_hz} Hz off: CRC failed");
            let cfo = p.freq_offset_hz.expect("an offset is measured").value();
            assert!(cfo.abs() < 10_000.0, "{offset_hz} Hz off: CFO {cfo} Hz");
            // LE 1M's modulation is the worker's to read, from the raw
            // samples (`signal::net::measure::le_1m`, whose own test runs a
            // channel off the tuning).
            assert!(p.modulation.is_none(), "{offset_hz} Hz off: read twice");
        }
    }

    /// A retune rebuilds the receiver: the mixer's offset belongs to the
    /// tuning it was made for.
    #[test]
    fn a_retune_does_not_match_the_old_receiver() {
        let ch_hz = channel::centre_hz(37).unwrap() as f64;
        let rx = Receiver::new(8_000_000.0, 37, Phy::OneM, ch_hz - 500_000.0).unwrap();
        assert!(rx.matches(37, 8_000_000.0, Phy::OneM, ch_hz - 500_000.0));
        assert!(!rx.matches(37, 8_000_000.0, Phy::OneM, ch_hz));
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

        let mut rx = centred(raw_rate, 38, Phy::OneM).unwrap();
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

        let mut rx = centred(raw_rate, 37, Phy::OneM).unwrap();
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
        let mut rx = centred(
            working_rate_hz(Phy::OneM),
            channel::channel_of(2_426_000_000).unwrap(),
            Phy::OneM,
        )
        .unwrap();
        assert!(rx.push(&block.bytes, geometry).is_empty());
    }
}
