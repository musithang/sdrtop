// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Shared RF-chain calculations: cascade noise figure, minimum detectable
//! signal, gain-staging advice, and ADC utilisation. Used by both the `rf_chain`
//! lab panel and the `micro_gain` field view so the numbers stay identical.

/// One link in the receive chain, used by the cascade NF, the gain lineup, and the
/// level diagram. `gain_db` is signed (a mixer's conversion loss is negative).
#[derive(Clone, Debug)]
pub struct Stage {
    /// The stage's name. Owned, because on a device sdrtop did not model the
    /// name comes from the driver at runtime rather than from a literal here.
    pub label: String,
    pub gain_db: f64,
    /// Noise added by this stage. **`NaN` when it is not known**, which is the
    /// case for every device sdrtop has not modelled.
    ///
    /// NaN rather than zero on purpose: a zero here is a plausible-looking noise
    /// figure, and it would flow into a cascade sum and out onto the screen as a
    /// confident number nobody measured. NaN cannot be mistaken for an answer,
    /// and any comparison against it is false, so a path that reads it by
    /// accident fails visibly rather than quietly.
    pub nf_db: f64,
}

impl Stage {
    /// A stage whose gain is known and whose noise is not: the driver told us
    /// what it amplifies by, and nothing at all about what it adds.
    pub fn unmodelled(label: &str, gain_db: f64) -> Self {
        Self {
            label: label.to_string(),
            gain_db,
            nf_db: f64::NAN,
        }
    }
}

/// The HackRF One receive chain as an ordered stage list - the single model behind
/// the cascade NF, the gain lineup, and the level diagram, so they can never drift.
///
/// Stage approximations (HackRF One / MAX2837):
///   AMP  - MGA-81563 front-end LNA (only when enabled): gain 14 dB, NF ~2.0 dB
///   LNA  - MAX2837 LNA: NF ~3.5 dB at max gain (40 dB), degrades ~0.15 dB per dB of
///          gain reduction (NF_LNA = 3.5 + (40−G)×0.15)
///   MIX  - down-conversion mixer: ~7 dB conversion loss, NF ~7 dB
///   VGA  - MAX2837 baseband VGA: gain `vga_db`, NF ~10 dB
pub fn cascade(amp_enabled: bool, lna_gain: u32, vga_gain: u32) -> Vec<Stage> {
    let nf_lna = 3.5 + (40.0 - lna_gain as f64).max(0.0) * 0.15;
    let mut stages = Vec::with_capacity(4);
    if amp_enabled {
        stages.push(Stage {
            label: "AMP".to_string(),
            gain_db: 14.0,
            nf_db: 2.0,
        });
    }
    stages.push(Stage {
        label: "LNA".to_string(),
        gain_db: lna_gain as f64,
        nf_db: nf_lna,
    });
    stages.push(Stage {
        label: "MIX".to_string(),
        gain_db: -7.0,
        nf_db: 7.0,
    });
    stages.push(Stage {
        label: "VGA".to_string(),
        gain_db: vga_gain as f64,
        nf_db: 10.0,
    });
    stages
}

/// System Noise Figure (dB) of a cascade via Friis:
///   F = F₁ + (F₂−1)/G₁ + (F₃−1)/(G₁G₂) + …   (linear, → back to dB)
/// The last stage's gain never enters (no stage follows it to suppress), so VGA gain
/// is irrelevant to the NF - only its NF and the gains ahead of it matter.
pub fn system_nf_db(stages: &[Stage]) -> f64 {
    let lin = |db: f64| 10f64.powf(db / 10.0);
    let mut f_total = 0.0;
    let mut g_preceding = 1.0; // product of gains before the current stage
    for s in stages {
        f_total += (lin(s.nf_db) - 1.0) / g_preceding;
        g_preceding *= lin(s.gain_db);
    }
    // First stage uses F₁ (not F₁−1); the loop above used (F₁−1)/1, so add the +1.
    if stages.is_empty() {
        return 0.0;
    }
    10.0 * (f_total + 1.0).log10()
}

/// Cascade Noise Figure (dB) for the live front-end - the one number shown app-wide.
/// Thin wrapper over [`system_nf_db`]`(`[`cascade`]`)`; VGA gain is irrelevant to NF
/// so a nominal 0 is passed.
pub fn estimate_nf_db(amp_enabled: bool, lna_gain: u32) -> f64 {
    system_nf_db(&cascade(amp_enabled, lna_gain, 0))
}

/// Minimum Detectable Signal in dBm.
///
/// MDS = kTB + NF  where kT = −174 dBm/Hz at 290 K.
/// Returns None when the BB filter bandwidth is unknown (0 Hz).
pub fn estimate_mds_dbm(bb_filter_hz: u32, nf_db: f64) -> Option<f64> {
    if bb_filter_hz == 0 {
        return None;
    }
    Some(-174.0 + 10.0 * (bb_filter_hz as f64).log10() + nf_db)
}

/// Gain-staging advice from the IQ amplitude histogram.
/// Returns `(text, severity)` where severity: 0 = OK, 1 = warn, 2 = crit.
pub fn gain_advice(hist: &[u64; 32]) -> (&'static str, u8) {
    let total: u64 = hist.iter().sum();
    if total == 0 {
        return ("no signal — start RX", 0);
    }
    let low: u64 = hist[..8].iter().sum();
    let high: u64 = hist[24..].iter().sum();
    let low_pct = low * 100 / total;
    let high_pct = high * 100 / total;
    if high_pct > 10 {
        ("⬇ clipping — reduce gain", 2)
    } else if low_pct > 90 {
        ("⬆ weak — increase LNA +8 dB", 1)
    } else if low_pct > 70 {
        ("⬆ under-utilised — try +8 dB", 1)
    } else {
        ("✓ gain staging OK", 0)
    }
}

/// ADC utilisation: fraction of samples in the mid-range bins (8–23) of the IQ
/// amplitude histogram. 0 when there is no data.
pub fn adc_utilisation_ratio(hist: &[u64; 32]) -> f64 {
    let total: u64 = hist.iter().sum();
    let mid: u64 = hist[8..24].iter().sum();
    if total > 0 {
        mid as f64 / total as f64
    } else {
        0.0
    }
}

/// `0 dBFS = 0 dBm` reference that anchors the modeled dBm lineup to the measured ADC
/// level. The HackRF is not power-calibrated, so every dBm here is **modeled /
/// relative** - useful for staging, not a wattmeter reading.
pub const ADC_DBFS_REF_DBM: f64 = 0.0;

/// Signal and (modeled) noise level at one node of the chain, in dBm.
#[derive(Clone, Debug)]
pub struct StageLevel {
    pub label: String,
    pub signal_dbm: f64,
    /// Where the noise sits at this node, when the chain's noise figures are
    /// known.
    ///
    /// `None` on a device whose stages sdrtop has never been told the noise
    /// figures for. The signal walk is still exact there, because it needs only
    /// the gains and the measured ADC level; the noise walk is not, because it
    /// is anchored on the cascade NF. Drawing it anyway with a zero would put a
    /// confident line on the screen for a number nobody has.
    pub noise_dbm: Option<f64>,
}

/// Level lineup down the chain: signal climbs by each stage's gain; the noise climbs
/// with it but starts lower at the antenna, so the gap shrinks from `snr + NF` at the
/// antenna to the measured `snr` at the ADC (the NF *is* that SNR loss). The signal is
/// anchored at the ADC by the measured `adc_signal_dbfs` and walked back to the
/// antenna; nodes returned are `[ANT, <each stage out>]` (the last stage out = ADC).
pub fn level_lineup(adc_signal_dbfs: f64, snr_db: f64, stages: &[Stage]) -> Vec<StageLevel> {
    let total_gain: f64 = stages.iter().map(|s| s.gain_db).sum();
    let ant_signal = adc_signal_dbfs + ADC_DBFS_REF_DBM - total_gain;
    let system_nf = system_nf_db(stages);
    let snr_ant = snr_db + system_nf;

    let mut out = Vec::with_capacity(stages.len() + 1);
    out.push(StageLevel {
        label: "ANT".to_string(),
        signal_dbm: ant_signal,
        noise_dbm: Some(ant_signal - snr_ant),
    });
    let mut cum_gain = 0.0;
    for k in 0..stages.len() {
        cum_gain += stages[k].gain_db;
        let signal = ant_signal + cum_gain;
        let cum_nf = system_nf_db(&stages[..=k]);
        let snr_here = snr_ant - cum_nf;
        out.push(StageLevel {
            label: stages[k].label.clone(),
            signal_dbm: signal,
            noise_dbm: Some(signal - snr_here),
        });
    }
    out
}

/// The same walk down the chain with **only** the part that needs no noise
/// figures: where the signal sits after each stage.
///
/// This is what an unmodelled device can honestly show. The gains are the
/// driver's own, the ADC level is measured, and the antenna end follows from the
/// two; none of it needs to know how much noise any stage adds. What is missing
/// is the noise line, and it is missing rather than guessed.
pub fn signal_lineup(adc_signal_dbfs: f64, stages: &[Stage]) -> Vec<StageLevel> {
    let total_gain: f64 = stages.iter().map(|s| s.gain_db).sum();
    let ant_signal = adc_signal_dbfs + ADC_DBFS_REF_DBM - total_gain;

    let mut out = Vec::with_capacity(stages.len() + 1);
    out.push(StageLevel {
        label: "ANT".to_string(),
        signal_dbm: ant_signal,
        noise_dbm: None,
    });
    let mut cum_gain = 0.0;
    for stage in stages {
        cum_gain += stage.gain_db;
        out.push(StageLevel {
            label: stage.label.clone(),
            signal_dbm: ant_signal + cum_gain,
            noise_dbm: None,
        });
    }
    out
}

/// How hard the ADC is driven, from the loudest sample, the RMS level, and the clip
/// count. `peak_counts` is the peak amplitude in the converter's own counts; `enob`
/// is the range the peak actually exercises (6.02 dB per bit), not an SNR-derived
/// ENOB.
#[derive(Clone, Copy, Debug)]
pub struct AdcLoading {
    pub peak_dbfs: f64,
    pub rms_dbfs: f64,
    pub crest_db: f64,
    pub peak_counts: u32,
    /// The count that reads as full scale, so the panel can print `x / y cts`
    /// without deriving the ceiling a second time.
    pub full_scale_counts: u32,
    pub enob: f64,
    /// The converter's depth, carried through so the readout and the number
    /// agree by construction rather than by both being handed 8.
    pub bits: u8,
    pub clip_events: u64,
    pub n: u64,
}

/// `bits` is the device's converter depth, from
/// [`crate::hardware::SampleGeometry::bits`]. Every figure here scales with it,
/// which is the whole reason it is a parameter: this used to be hardcoded to 8,
/// and 8 is true of exactly the two radios sdrtop could open.
pub fn adc_loading(
    peak_dbfs: f64,
    rms_dbfs: f64,
    clip_events: u64,
    n: u64,
    bits: u8,
) -> AdcLoading {
    let bits = bits.clamp(1, 31);
    let full = ((1u32 << (bits - 1)) - 1).max(1) as f64;
    let peak_counts = (full * 10f64.powf(peak_dbfs / 20.0))
        .round()
        .clamp(0.0, full) as u32;
    AdcLoading {
        peak_dbfs,
        rms_dbfs,
        crest_db: peak_dbfs - rms_dbfs,
        peak_counts,
        full_scale_counts: full as u32,
        enob: (bits as f64 + peak_dbfs / 6.02).clamp(0.0, bits as f64),
        bits,
        clip_events,
        n,
    }
}

/// Large-signal linearity figures. **All modeled** - these need a two-tone source to
/// measure, so they are datasheet-anchored estimates nudged by the live gain, not lab
/// readings. `sfdr_limit_db` is the hard quantisation ceiling for `bits`
/// (6.02·bits + 1.76).
///
/// **The datasheet the estimates are anchored to is the HackRF's.** The
/// quantisation ceiling now follows the device, but `iip3_dbm`, `imd3_dbc` and
/// the 52 dB SFDR cap are a specific front end's numbers wearing a generic name.
/// A caller must not draw this card for a device whose chain it does not know:
/// gate it on `DeviceCapabilities::friis_applicable`, the same flag that gates
/// the Friis cascade for the same reason.
#[derive(Clone, Copy, Debug)]
pub struct Linearity {
    pub p1db_headroom_db: f64,
    pub iip3_dbm: f64,
    pub imd3_dbc: f64,
    pub sfdr_db: f64,
    pub sfdr_limit_db: f64,
}

/// The ideal signal-to-quantisation-noise ratio of a `bits`-deep converter:
/// 6.02 dB per bit plus 1.76. About 50 dB on an 8-bit converter, about 86 on a
/// 14-bit one.
///
/// One function because two panels want it: the linearity card prints it as an
/// SFDR ceiling, and the level diagram draws it as the floor the signal must
/// stay above. They were separately hardcoded to the 8-bit answer, which is how
/// a pair of numbers ends up disagreeing after somebody edits one.
pub fn quantisation_snr_db(bits: u8) -> f64 {
    6.02 * bits.clamp(1, 31) as f64 + 1.76
}

pub fn linearity(lna_gain: u32, vga_gain: u32, bits: u8) -> Linearity {
    let total = lna_gain as f64 + vga_gain as f64;
    let sfdr_limit = quantisation_snr_db(bits);
    Linearity {
        // More front-end gain pushes the input intercept / compression point down.
        p1db_headroom_db: (12.0 - total * 0.08).max(0.0),
        iip3_dbm: 10.0 - total * 0.25,
        imd3_dbc: -52.0,
        sfdr_db: sfdr_limit.min(52.0),
        sfdr_limit_db: sfdr_limit,
    }
}

/// Where the ADC peak should land for clean staging.
pub const OPT_PEAK_DBFS: f64 = -8.0;

/// Staging verdict from the ADC peak level. `(word, severity)` with severity
/// 0 = OK, 1 = warn, 2 = crit - same scale as [`gain_advice`].
pub fn staging_verdict(peak_dbfs: f64) -> (&'static str, u8) {
    if peak_dbfs >= -1.0 {
        ("CLIPPING", 2)
    } else if peak_dbfs >= -4.0 {
        ("HOT", 1)
    } else if peak_dbfs >= -14.0 {
        ("WELL-STAGED", 0)
    } else if peak_dbfs >= -28.0 {
        ("UNDER-UTILISED", 1)
    } else {
        ("WEAK", 1)
    }
}

/// Auto-gain target (HackRF legal steps: LNA 0..40/8, VGA 0..62/2) that lands the ADC
/// peak near [`OPT_PEAK_DBFS`]. NF-aware: when the level must come **down** it trims the
/// VGA first (preserving LNA gain for noise figure); when it must come **up** it adds
/// LNA first. Clamped to the legal grid; the result may not hit the target exactly when
/// the chain is already at a rail.
pub fn staging_target(peak_dbfs: f64, lna_gain: u32, vga_gain: u32) -> (u32, u32) {
    let delta = OPT_PEAK_DBFS - peak_dbfs; // +ve = need more gain
    let mut lna = lna_gain as f64;
    let mut vga = vga_gain as f64;
    if delta >= 0.0 {
        // Need more: fill LNA first (best NF), spill the rest into VGA.
        let head_lna = 40.0 - lna;
        let add_lna = delta.min(head_lna.max(0.0));
        lna += add_lna;
        vga = (vga + (delta - add_lna)).min(62.0);
    } else {
        // Need less: drop VGA first (keep LNA gain for NF), then LNA.
        let drop = -delta;
        let cut_vga = drop.min(vga);
        vga -= cut_vga;
        lna = (lna - (drop - cut_vga)).max(0.0);
    }
    let lna_c = ((lna / 8.0).round() * 8.0).clamp(0.0, 40.0) as u32;
    let vga_c = ((vga / 2.0).round() * 2.0).clamp(0.0, 62.0) as u32;
    (lna_c, vga_c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nf_amp_on_max_gain_is_near_amp_nf() {
        let nf = estimate_nf_db(true, 40);
        assert!(nf > 2.0 && nf < 3.0, "expected ~2.1 dB, got {:.2}", nf);
    }

    #[test]
    fn nf_amp_off_max_lna_gain_near_lna_nf() {
        let nf = estimate_nf_db(false, 40);
        assert!(nf > 3.4 && nf < 4.0, "expected ~3.5 dB, got {:.2}", nf);
    }

    #[test]
    fn nf_degrades_at_lower_lna_gain() {
        let nf_high = estimate_nf_db(false, 40);
        let nf_low = estimate_nf_db(false, 8);
        assert!(nf_low > nf_high, "NF should be worse at lower LNA gain");
    }

    #[test]
    fn nf_amp_lowers_cascade_nf() {
        let nf_no_amp = estimate_nf_db(false, 24);
        let nf_amp = estimate_nf_db(true, 24);
        assert!(nf_amp < nf_no_amp, "AMP should improve cascade NF");
    }

    #[test]
    fn gain_advice_clipping_is_crit() {
        let mut hist = [0u64; 32];
        hist[24] = 20;
        hist[0] = 80; // >10% in high bins
        let (_, sev) = gain_advice(&hist);
        assert_eq!(sev, 2);
    }

    #[test]
    fn gain_advice_weak_is_warn() {
        let mut hist = [0u64; 32];
        hist[0] = 95;
        hist[8] = 5; // >90% in low bins
        let (_, sev) = gain_advice(&hist);
        assert_eq!(sev, 1);
    }

    #[test]
    fn gain_advice_ok_is_zero() {
        let mut hist = [0u64; 32];
        hist[8] = 50;
        hist[16] = 50; // mid-range utilisation
        let (_, sev) = gain_advice(&hist);
        assert_eq!(sev, 0);
    }

    #[test]
    fn mds_none_when_bb_filter_zero() {
        assert!(estimate_mds_dbm(0, 3.5).is_none());
    }

    #[test]
    fn mds_10mhz_3_5db_nf() {
        // MDS = -174 + 10*log10(10_000_000) + 3.5 = -174 + 70 + 3.5 = -100.5 dBm
        let mds = estimate_mds_dbm(10_000_000, 3.5).unwrap();
        assert!(
            (mds - (-100.5)).abs() < 0.1,
            "expected ~-100.5 dBm, got {:.1}",
            mds
        );
    }

    #[test]
    fn mds_improves_with_narrower_bw() {
        let mds_wide = estimate_mds_dbm(10_000_000, 3.5).unwrap();
        let mds_narrow = estimate_mds_dbm(5_000_000, 3.5).unwrap();
        assert!(
            (mds_wide - mds_narrow - 3.0).abs() < 0.1,
            "halving BW should improve MDS by 3 dB"
        );
    }

    #[test]
    fn adc_util_zero_when_empty() {
        assert_eq!(adc_utilisation_ratio(&[0u64; 32]), 0.0);
    }

    #[test]
    fn adc_util_counts_mid_bins() {
        let mut hist = [0u64; 32];
        hist[0] = 50; // low (out of mid range)
        hist[16] = 50; // mid
        assert!((adc_utilisation_ratio(&hist) - 0.5).abs() < 1e-9);
    }

    // --- cascade / NF -----------------------------------------------------------
    #[test]
    fn estimate_nf_matches_cascade_system_nf() {
        for &(amp, lna) in &[(false, 40), (false, 8), (true, 24), (true, 40)] {
            let direct = estimate_nf_db(amp, lna);
            let viacas = system_nf_db(&cascade(amp, lna, 32));
            assert!(
                (direct - viacas).abs() < 1e-9,
                "amp={amp} lna={lna}: {direct} vs {viacas}"
            );
        }
    }

    #[test]
    fn system_nf_empty_is_zero() {
        assert_eq!(system_nf_db(&[]), 0.0);
    }

    #[test]
    fn vga_gain_does_not_change_nf() {
        let a = system_nf_db(&cascade(false, 32, 0));
        let b = system_nf_db(&cascade(false, 32, 62));
        assert!(
            (a - b).abs() < 1e-12,
            "VGA is the last stage — its gain can't affect NF"
        );
    }

    // --- level lineup -----------------------------------------------------------
    #[test]
    fn lineup_anchors_at_adc_and_gap_is_snr() {
        let stages = cascade(false, 32, 32); // total gain = 32 − 7 + 32 = 57 dB
        let lv = level_lineup(-8.0, 40.0, &stages);
        let adc = lv.last().unwrap();
        assert!(
            (adc.signal_dbm - (-8.0)).abs() < 1e-9,
            "ADC node = measured level"
        );
        // SNR at the ADC equals the measured SNR.
        assert!((adc.signal_dbm - adc.noise_dbm.unwrap() - 40.0).abs() < 1e-9);
        // Signal climbs by the chain gain back from the antenna.
        let ant = &lv[0];
        assert!((adc.signal_dbm - ant.signal_dbm - 57.0).abs() < 1e-9);
    }

    #[test]
    fn lineup_antenna_snr_is_better_by_the_nf() {
        let stages = cascade(false, 32, 32);
        let nf = system_nf_db(&stages);
        let lv = level_lineup(-8.0, 40.0, &stages);
        let ant = &lv[0];
        let ant_snr = ant.signal_dbm - ant.noise_dbm.unwrap();
        assert!(
            (ant_snr - (40.0 + nf)).abs() < 1e-9,
            "antenna SNR = ADC SNR + NF"
        );
    }

    // --- ADC loading ------------------------------------------------------------
    /// Both radios sdrtop can open natively.
    const EIGHT: u8 = 8;

    #[test]
    fn adc_loading_peak_counts_and_crest_and_enob() {
        let l = adc_loading(-8.0, -18.0, 0, 8192, EIGHT);
        assert_eq!(l.peak_counts, 51, "127·10^(−8/20) ≈ 51 counts");
        assert!((l.crest_db - 10.0).abs() < 1e-9);
        assert!(
            (l.enob - (8.0 - 8.0 / 6.02)).abs() < 1e-9,
            "≈ 6.67 effective bits"
        );
    }

    #[test]
    fn adc_loading_clamps_full_scale_peak() {
        let l = adc_loading(0.0, -6.0, 3, 8192, EIGHT);
        assert_eq!(l.peak_counts, 127);
        assert_eq!(l.full_scale_counts, 127);
        assert!((l.enob - 8.0).abs() < 1e-9);
        assert_eq!(l.clip_events, 3);
    }

    /// The same signal on a deeper converter. A 14-bit radio at full scale has
    /// 14 effective bits and 8191 counts, not 8 and 127, and reporting the 8-bit
    /// answer for it was the whole reason this became a parameter.
    #[test]
    fn adc_loading_follows_the_converter_depth() {
        let l = adc_loading(0.0, -6.0, 0, 8192, 14);
        assert_eq!(l.peak_counts, 8191);
        assert_eq!(l.full_scale_counts, 8191);
        assert!((l.enob - 14.0).abs() < 1e-9);
        assert_eq!(l.bits, 14);
    }

    /// A driver reporting a silly depth must not shift by 255 or divide by zero.
    #[test]
    fn adc_loading_survives_a_nonsense_depth() {
        for bits in [0u8, 1, 200] {
            let l = adc_loading(-6.0, -12.0, 0, 8192, bits);
            assert!(l.full_scale_counts >= 1);
            assert!(l.enob >= 0.0);
        }
    }

    // --- linearity (modeled) ----------------------------------------------------
    #[test]
    fn linearity_sfdr_ceiling_and_gain_trend() {
        let lo = linearity(8, 0, EIGHT);
        let hi = linearity(40, 62, EIGHT);
        assert!(
            (lo.sfdr_limit_db - 49.92).abs() < 0.05,
            "8-bit ideal SFDR ≈ 49.9 dB"
        );
        assert!(hi.iip3_dbm < lo.iip3_dbm, "more gain → lower IIP3");
        assert!(
            hi.p1db_headroom_db < lo.p1db_headroom_db,
            "more gain → less compression headroom"
        );
    }

    /// 6.02 dB per bit, so a deeper converter has a higher quantisation ceiling.
    /// A 14-bit radio told it could only reach 50 dB would be being described by
    /// a HackRF's datasheet.
    #[test]
    fn the_quantisation_ceiling_follows_the_depth() {
        assert!((linearity(0, 0, 12).sfdr_limit_db - 73.99).abs() < 0.05);
        assert!((linearity(0, 0, 14).sfdr_limit_db - 86.04).abs() < 0.05);
        assert!((linearity(0, 0, 16).sfdr_limit_db - 98.08).abs() < 0.05);
    }

    // --- staging ----------------------------------------------------------------
    #[test]
    fn staging_verdict_bands() {
        assert_eq!(staging_verdict(-8.0).1, 0, "−8 dBFS is well-staged");
        assert_eq!(staging_verdict(0.0), ("CLIPPING", 2));
        assert_eq!(staging_verdict(-40.0).0, "WEAK");
    }

    #[test]
    fn staging_target_clamps_to_legal_grid() {
        let (lna, vga) = staging_target(-30.0, 0, 0); // very weak → add gain
        assert!(
            lna <= 40 && lna % 8 == 0,
            "LNA on the /8 grid within range: {lna}"
        );
        assert!(
            vga <= 62 && vga % 2 == 0,
            "VGA on the /2 grid within range: {vga}"
        );
    }

    #[test]
    fn staging_target_reduces_gain_when_clipping_and_trims_vga_first() {
        // Peak at 0 dBFS, both stages mid → needs −8 dB; VGA should drop, LNA held.
        let (lna, vga) = staging_target(0.0, 32, 32);
        assert_eq!(lna, 32, "LNA kept for NF when only a small cut is needed");
        assert!(vga < 32, "VGA trimmed to drop the level: {vga}");
    }

    #[test]
    fn staging_target_near_optimum_is_stable() {
        let (lna, vga) = staging_target(OPT_PEAK_DBFS, 32, 30);
        assert_eq!((lna, vga), (32, 30), "already optimal → no change");
    }

    /// The signal-only walk is the part that needs no noise figures: the same
    /// signal levels as the full lineup, and no noise line at all.
    #[test]
    fn the_signal_only_lineup_matches_the_full_one_and_omits_the_noise() {
        let stages = cascade(false, 32, 32);
        let full = level_lineup(-8.0, 40.0, &stages);
        let signal = signal_lineup(-8.0, &stages);

        assert_eq!(full.len(), signal.len());
        for (a, b) in full.iter().zip(&signal) {
            assert_eq!(a.label, b.label);
            assert!(
                (a.signal_dbm - b.signal_dbm).abs() < 1e-9,
                "the signal walk needs only the gains: {} vs {}",
                a.signal_dbm,
                b.signal_dbm
            );
            assert!(a.noise_dbm.is_some());
            assert!(
                b.noise_dbm.is_none(),
                "and the noise line is absent, not zero"
            );
        }
    }

    /// A device with no stages still has an antenna node, and the ADC anchor is
    /// the antenna level when nothing amplifies in between.
    #[test]
    fn a_chain_with_no_stages_is_just_the_antenna() {
        let lv = signal_lineup(-8.0, &[]);
        assert_eq!(lv.len(), 1);
        assert_eq!(lv[0].label, "ANT");
    }
}
