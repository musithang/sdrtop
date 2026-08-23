/// Known frequency allocations: (start_hz, end_hz, label).
/// Used by the spectrum and waterfall panels for visual overlays.
pub const BAND_PLAN: &[(u64, u64, &str)] = &[
    (50_000_000,    54_000_000,    "6m"),
    (87_500_000,    108_000_000,   "FM"),
    (108_000_000,   118_000_000,   "VOR/ILS"),
    (118_000_000,   137_000_000,   "AIR"),
    (144_000_000,   146_000_000,   "2m"),
    (222_000_000,   225_000_000,   "1.25m"),
    (430_000_000,   440_000_000,   "70cm"),
    (433_050_000,   434_790_000,   "ISM433"),
    (446_000_000,   446_200_000,   "PMR"),
    (868_000_000,   869_000_000,   "ISM868"),
    (902_000_000,   928_000_000,   "33cm"),
    (1_227_600_000, 1_227_601_000, "GPS-L2"),
    (1_240_000_000, 1_300_000_000, "23cm"),
    (1_575_419_000, 1_575_421_000, "GPS-L1"),
    (1_710_000_000, 2_170_000_000, "CELL"),
    (2_400_000_000, 2_483_500_000, "2.4G"),
    (3_400_000_000, 3_475_000_000, "3.4G"),
    (5_650_000_000, 5_850_000_000, "5.8G"),
];

/// The most specific (narrowest) known band containing `freq_hz`, if any.
/// Narrower allocations win so e.g. ISM433 is reported over the wider 70cm.
pub fn band_at(freq_hz: u64) -> Option<&'static str> {
    BAND_PLAN
        .iter()
        .filter(|(start, end, _)| freq_hz >= *start && freq_hz < *end)
        .min_by_key(|(start, end, _)| end - start)
        .map(|(_, _, name)| *name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_at_picks_narrowest_match() {
        // 433.92 MHz sits in both 70cm and ISM433 → the narrower ISM433 wins.
        assert_eq!(band_at(433_920_000), Some("ISM433"));
        // FM broadcast.
        assert_eq!(band_at(100_000_000), Some("FM"));
        // Nothing allocated here.
        assert_eq!(band_at(420_000_000), None);
    }
}
