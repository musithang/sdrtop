//! Shared RF-chain calculations: cascade noise figure, minimum detectable
//! signal, gain-staging advice, and ADC utilisation. Used by both the `rf_chain`
//! lab panel and the `micro_gain` field view so the numbers stay identical.

/// One link in the receive chain, used by the cascade NF, the gain lineup, and the
/// level diagram. `gain_db` is signed (a mixer's conversion loss is negative).
#[derive(Clone, Copy, Debug)]
pub struct Stage {
    pub label: &'static str,
    pub gain_db: f64,
    pub nf_db: f64,
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
            label: "AMP",
            gain_db: 14.0,
            nf_db: 2.0,
        });
    }
    stages.push(Stage {
        label: "LNA",
        gain_db: lna_gain as f64,
        nf_db: nf_lna,
    });
    stages.push(Stage {
        label: "MIX",
        gain_db: -7.0,
        nf_db: 7.0,
    });
    stages.push(Stage {
        label: "VGA",
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
#[derive(Clone, Copy, Debug)]
pub struct StageLevel {
    pub label: &'static str,
    pub signal_dbm: f64,
    pub noise_dbm: f64,
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
        label: "ANT",
        signal_dbm: ant_signal,
        noise_dbm: ant_signal - snr_ant,
    });
    let mut cum_gain = 0.0;
    for k in 0..stages.len() {
        cum_gain += stages[k].gain_db;
        let signal = ant_signal + cum_gain;
        let cum_nf = system_nf_db(&stages[..=k]);
        let snr_here = snr_ant - cum_nf;
        out.push(StageLevel {
            label: stages[k].label,
            signal_dbm: signal,
            noise_dbm: signal - snr_here,
        });
    }
    out
}

/// How hard the ADC is driven, from the loudest sample, the RMS level, and the clip
/// count. `peak_counts` is the peak amplitude in 8-bit counts; `enob` is the range
/// the peak actually exercises (6.02 dB per bit), not an SNR-derived ENOB.
#[derive(Clone, Copy, Debug)]
pub struct AdcLoading {
    pub peak_dbfs: f64,
    pub rms_dbfs: f64,
    pub crest_db: f64,
    pub peak_counts: u32,
    pub enob: f64,
    pub clip_events: u64,
    pub n: u64,
}

pub fn adc_loading(peak_dbfs: f64, rms_dbfs: f64, clip_events: u64, n: u64) -> AdcLoading {
    let peak_counts = (127.0 * 10f64.powf(peak_dbfs / 20.0))
        .round()
        .clamp(0.0, 127.0) as u32;
    AdcLoading {
        peak_dbfs,
        rms_dbfs,
        crest_db: peak_dbfs - rms_dbfs,
        peak_counts,
        enob: (8.0 + peak_dbfs / 6.02).clamp(0.0, 8.0),
        clip_events,
        n,
    }
}

/// Large-signal linearity figures. **All modeled** - these need a two-tone source to
/// measure, so they are datasheet-anchored estimates nudged by the live gain, not lab
/// readings. `sfdr_limit_db` is the hard 8-bit quantisation ceiling (6.02·8 + 1.76).
#[derive(Clone, Copy, Debug)]
pub struct Linearity {
    pub p1db_headroom_db: f64,
    pub iip3_dbm: f64,
    pub imd3_dbc: f64,
    pub sfdr_db: f64,
    pub sfdr_limit_db: f64,
}

pub fn linearity(lna_gain: u32, vga_gain: u32) -> Linearity {
    let total = lna_gain as f64 + vga_gain as f64;
    let sfdr_limit: f64 = 6.02 * 8.0 + 1.76; // ≈ 49.9 dB, the 8-bit ceiling
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
        assert!((adc.signal_dbm - adc.noise_dbm - 40.0).abs() < 1e-9);
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
        let ant_snr = ant.signal_dbm - ant.noise_dbm;
        assert!(
            (ant_snr - (40.0 + nf)).abs() < 1e-9,
            "antenna SNR = ADC SNR + NF"
        );
    }

    // --- ADC loading ------------------------------------------------------------
    #[test]
    fn adc_loading_peak_counts_and_crest_and_enob() {
        let l = adc_loading(-8.0, -18.0, 0, 8192);
        assert_eq!(l.peak_counts, 51, "127·10^(−8/20) ≈ 51 counts");
        assert!((l.crest_db - 10.0).abs() < 1e-9);
        assert!(
            (l.enob - (8.0 - 8.0 / 6.02)).abs() < 1e-9,
            "≈ 6.67 effective bits"
        );
    }

    #[test]
    fn adc_loading_clamps_full_scale_peak() {
        let l = adc_loading(0.0, -6.0, 3, 8192);
        assert_eq!(l.peak_counts, 127);
        assert!((l.enob - 8.0).abs() < 1e-9);
        assert_eq!(l.clip_events, 3);
    }

    // --- linearity (modeled) ----------------------------------------------------
    #[test]
    fn linearity_sfdr_ceiling_and_gain_trend() {
        let lo = linearity(8, 0);
        let hi = linearity(40, 62);
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
}
