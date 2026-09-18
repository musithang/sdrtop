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
use crate::signal::dsp::correlate::{threshold_for_false_alarm, MatchedFilter};
use crate::signal::dsp::discriminate::discriminate;
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

/// How rare a false trigger has to be to live with continuously, at 4
/// million matched-filter evaluations a second.
///
/// **Measured against real air, not just the model.** `1e-9` - about one
/// false trigger every four minutes under `false_alarm_rate`'s assumption of
/// circularly symmetric Gaussian noise - was tried first on a real HackRF
/// capture and found the coherence at every real trigger sitting at 0.12 to
/// 0.17, a hair above that rate's own 0.122 threshold, on a busy real
/// channel. Real RF is not the model: correlated interference, ADC
/// artefacts and genuine nearby transmissions on other protocols give a
/// matched filter far more near-misses than white Gaussian noise would, so
/// the same formula's rate needs to be pushed much further to buy a
/// threshold that actually rejects them. This is that adjustment - a rate
/// with no claim to being the true false-alarm probability of a live
/// receiver, only to producing a threshold high enough that this arc's own
/// clean synthetic detections (coherence 0.85 upward) still clear it
/// comfortably while the marginal real-air ones measured here do not.
const FALSE_ALARM_RATE: f64 = 1e-30;

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

/// One channel's live receiver: the decimator, the matched filter, and the
/// capture in progress, if any.
pub struct Receiver {
    decim: StreamingDecimator,
    filter: MatchedFilter,
    threshold: f64,
    /// The reference's own length, once filtered and decimated: not
    /// [`super::detect::REFERENCE_SYMBOLS`] `*` [`WORKING_SPS`], because
    /// [`front_end`]'s filter changes how many samples the reference comes
    /// out to. Both [`threshold_for_false_alarm`] and `snr_from_metric`
    /// below need the window length the coherence was actually measured
    /// over, not the unfiltered figure.
    window_len: usize,
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
    history: Vec<Complex<f32>>,
    capture: Vec<Complex<f32>>,
    capturing: bool,
    last_coherence: f64,
}

impl Receiver {
    pub fn new(raw_rate: f64, channel: u8, phy: Phy) -> Result<Self, String> {
        let decim = front_end(raw_rate, phy)?;
        let reference = matched_reference(raw_rate, phy)?;
        let window_len = reference.len();
        let threshold = threshold_for_false_alarm(window_len, FALSE_ALARM_RATE);
        Ok(Self {
            decim,
            filter: MatchedFilter::new(&reference),
            threshold,
            window_len,
            channel,
            raw_rate,
            phy,
            history: Vec::new(),
            capture: Vec::new(),
            capturing: false,
            last_coherence: 0.0,
        })
    }

    /// Whether this receiver is still the right one for `channel` at
    /// `raw_rate` on `phy` - a retune, a sample-rate change or a PHY change
    /// invalidates the detector's own reference and the capture in
    /// progress alike, so the caller rebuilds rather than reusing.
    pub fn matches(&self, channel: u8, raw_rate: f64, phy: Phy) -> bool {
        self.channel == channel && (self.raw_rate - raw_rate).abs() < 1.0 && self.phy == phy
    }

    /// Feed one block of raw device bytes. Returns every packet fully
    /// decoded from it - almost always zero, and rarely more than one: a
    /// legacy advertising PDU is under a third of a millisecond of air time.
    pub fn push(&mut self, bytes: &[u8], geometry: SampleGeometry) -> Vec<Packet> {
        let mut iq = Vec::new();
        decode_iq(bytes, geometry, usize::MAX, &mut iq);
        let mut working = Vec::new();
        self.decim.process(&iq, &mut working);

        let cap_limit = (16 + MAX_PDU_BYTES * 8) * WORKING_SPS;
        let mut found = Vec::new();
        for &sample in &working {
            self.history.push(sample);
            if self.history.len() > LOOKBACK_SAMPLES {
                let excess = self.history.len() - LOOKBACK_SAMPLES;
                self.history.drain(..excess);
            }
            if self.capturing {
                self.capture.push(sample);
                match self.try_decode() {
                    Some(packet) => {
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
                        if let Some(packet) = self.try_decode_from(LOOKBACK_SAMPLES) {
                            found.push(packet);
                        }
                        self.capturing = false;
                        self.capture.clear();
                    }
                    None => {}
                }
            } else if let Some(coherence) = self.filter.push(sample).and_then(|m| m.coherence()) {
                if coherence > self.threshold {
                    self.capturing = true;
                    self.capture = self.history.clone();
                    self.last_coherence = coherence;
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
    /// **Why a search, and not a single trusted position.** `try_decode_from`
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
        let center = LOOKBACK_SAMPLES as isize;
        let step = WORKING_SPS as isize;
        let span = HEADER_SEARCH_SYMBOLS as isize;
        for k in -span..=span {
            let skip = center + k * step;
            if skip < 0 {
                continue;
            }
            if let Some(packet) = self.try_decode_from(skip as usize) {
                if packet.crc_ok {
                    return Some(packet);
                }
            }
        }
        None
    }

    /// The actual decode, from one candidate header-start position:
    /// `self.capture[skip..]` is treated as running from the header's own
    /// first bit. [`try_decode`] is the search over candidate `skip`
    /// values; `push`'s own give-up path is the other caller, once, at
    /// exactly [`LOOKBACK_SAMPLES`] - the nominal boundary - so a real
    /// packet that really was aligned there and genuinely failed its CRC
    /// still gets reported as that, rather than the search silently
    /// discarding it for lack of any clean candidate.
    ///
    /// **Runs `find_phase` on the capture, and does not trust the detector's
    /// peak position for anything beyond where the capture starts.** The
    /// first version indexed directly from the peak, reasoning that
    /// `signal::ble::detect`'s own tests measured it landing exactly on the
    /// symbol boundary - true for a synthetic packet built by the same
    /// `gfsk::modulate` call the detector's own reference comes from, and
    /// false for a real transmitter: a real symbol clock has no reason to
    /// share a sample-aligned phase with this receiver's, only an
    /// unsynchronised, arbitrary one. Real hardware measured this directly -
    /// every field decoded correctly (plausible PDU types, real advertiser
    /// addresses) while every CRC failed, which is exactly the signature of
    /// a small, consistent sub-sample timing error rather than a wrong
    /// algorithm. `find_phase` is the same tool B4 built for exactly this;
    /// the deterministic shortcut only worked on the test signal that could
    /// never have shown the bug.
    ///
    /// **Real hardware's CRC still fails even with this fixed** - the
    /// working hypothesis after B6 is a real device's own crystal offset
    /// (BLE allows up to ±150 ppm, which at 2.4 GHz is up to ±360 kHz,
    /// larger than the ±250 kHz deviation itself), uncorrected. B7's
    /// `freq_offset_hz` below is that same offset, finally measured and
    /// reported rather than only corrected for blindly - the honest first
    /// step toward deciding whether that hypothesis is the right one.
    fn try_decode_from(&self, skip: usize) -> Option<Packet> {
        if skip >= self.capture.len() {
            return None;
        }
        let capture = &self.capture[skip..];
        let mut inst = Vec::new();
        discriminate(capture, working_rate_hz(self.phy), &mut inst);
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
        let rough_offset = crate::signal::dsp::uncertainty::mean_with_uncertainty(&inst);
        let (mut bits, raw_symbols) = super::sync::slice(
            &inst,
            WORKING_SPS as f64,
            symbols,
            rough_offset.value() as f32,
        );
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
        // The *reported* offset, unlike `rough_offset` above, is read from
        // one sample per symbol rather than the raw four-per-symbol trace.
        // B9's own `measure::drift` found why that distinction is load-
        // bearing: `mean_with_uncertainty` assumes independent samples, and
        // four samples spanning one Gaussian-filtered symbol are one
        // slowly-varying value read four times, not four independent ones -
        // feeding it the raw trace divides by an `N` four times too large
        // and understates the uncertainty by about half. `rough_offset`
        // above never had to be exact, only good enough to slice against;
        // this one is what a reader sees a `+/-` on.
        let offset = crate::signal::dsp::uncertainty::mean_with_uncertainty(raw_symbols);
        packet.freq_offset_hz = Some(offset);
        // `snr_from_metric` was derived for `Coherence::metric` - two noisy
        // copies of the same unknown signal correlated against each other -
        // and `Match::coherence` is a different measurement, a noisy signal
        // correlated against a known, noiseless reference. Worked through
        // for a unit-power reference: at the window lengths this arc uses
        // the two converge to the same `rho = snr / (1 + snr)` relationship
        // in the limit, so this reuses the already-tested inverse rather
        // than deriving and separately validating a second one - reasoned
        // to be a close approximation at this receiver's own window length,
        // not proven exact for a matched filter's own statistics. `None`
        // only at a coherence of one, which the false-alarm threshold
        // already keeps every real reading comfortably under.
        packet.snr_db =
            snr_from_metric(self.last_coherence, self.window_len).map(|snr| 10.0 * snr.log10());
        packet.modulation = super::measure::modulation_quality(raw_bits, raw_symbols);
        packet.drift = super::measure::drift(raw_symbols);
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
        let mut payload = addr.to_vec();
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

    /// B17's own exit condition: the identical chain, on LE 2M, at twice
    /// the symbol rate and twice the preamble length - design section
    /// 1.2's own "nothing new except the numbers" - decodes a real ADV_IND
    /// whole. Not a second, hand-duplicated test: [`synthetic_packet_iq`],
    /// [`Receiver::new`] and everything downstream take `phy` as data.
    #[test]
    fn a_synthetic_adv_ind_is_received_whole_on_le_2m() {
        let addr = [0xAA, 0xBB, 0xCC, 0x11, 0x22, 0x33];
        let mut payload = addr.to_vec();
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
        let payload = addr.to_vec();
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
        let mut payload = addr.to_vec();
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
        let mut payload = addr.to_vec();
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
