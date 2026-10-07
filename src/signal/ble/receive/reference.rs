// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! What the detector looks for and how sure it must be: the sync word's
//! frequency template, built as it will actually be received, and the
//! threshold noise alone crosses at the rate the receiver accepts.

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
use super::*;

pub(super) fn shape_n_eff(phy: Phy) -> f64 {
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
pub(super) const SHAPE_Z: f64 = 6.0;

/// The detector threshold for `phy`: [`SHAPE_Z`] standard deviations of its
/// noise statistic. About 0.58 for LE 1M; on a recording of real traffic
/// every CRC-clean packet an independent receiver found peaked at 0.68 or
/// more.
pub(super) fn shape_threshold(phy: Phy) -> f64 {
    SHAPE_Z / shape_n_eff(phy).sqrt()
}

/// The sync-word reference to correlate against, built the way it will
/// actually be received rather than the way the tests' `detect::Detector`
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
pub(super) fn frequency_template(reference: &[Complex<f32>], phy: Phy) -> Vec<f32> {
    let mut track = Vec::new();
    discriminate(reference, working_rate_hz(phy), &mut track);
    track
}

pub(super) fn matched_reference(
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
pub(super) const MARGIN_SYMBOLS: usize = 16;

/// A fixed, non-degenerate bit pattern for [`matched_reference`]'s own
/// margin - alternating, the same character as the preamble it sits next
/// to, chosen only to give the shaping and anti-alias filters real content
/// to settle against rather than to be decoded as anything itself.
pub(super) fn margin_bits(len: usize) -> Vec<bool> {
    (0..len).map(|i| i % 2 == 0).collect()
}
