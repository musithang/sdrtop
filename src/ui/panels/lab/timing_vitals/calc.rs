//! The arithmetic behind the readings. No theme, no width, no frame.

/// Binary MB/s ceiling of the USB link for this device: the byte rate at the
/// device's maximum sample rate (8-bit I/Q ⇒ 2 bytes per complex sample). Honest
/// per-device headroom reference rather than a magic constant.
pub(super) fn link_ceiling_mbps(sample_rate_max_hz: f64) -> f64 {
    (sample_rate_max_hz * 2.0) / (1024.0 * 1024.0)
}

/// Overrun margin: how much ring-buffer headroom remains below the ceiling, from
/// the session peak fill. Clamped to a sane 0..=100.
pub(super) fn overrun_margin_pct(peak_fill_pct: f64) -> f64 {
    (100.0 - peak_fill_pct).clamp(0.0, 100.0)
}

/// `HH:MM:SS` uptime from a whole-second count.
pub(super) fn fmt_uptime(secs: u64) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_ceiling_is_the_devices_own_byte_rate() {
        // 20 Msps HackRF → 40 MB(byte)/s → ~38.1 binary MB/s.
        let c = link_ceiling_mbps(20_000_000.0);
        assert!((c - 38.147).abs() < 0.01, "got {c}");
        assert_eq!(link_ceiling_mbps(0.0), 0.0);
    }

    #[test]
    fn overrun_margin_never_goes_negative() {
        // A peak above the ceiling cannot push the margin negative.
        assert_eq!(overrun_margin_pct(0.0), 100.0);
        assert_eq!(overrun_margin_pct(40.0), 60.0);
        assert_eq!(overrun_margin_pct(62.0), 38.0);
        assert_eq!(overrun_margin_pct(100.0), 0.0);
        assert_eq!(overrun_margin_pct(140.0), 0.0);
    }

    #[test]
    fn uptime_carries_hours_and_pads() {
        assert_eq!(fmt_uptime(0), "00:00:00");
        assert_eq!(fmt_uptime(59), "00:00:59");
        assert_eq!(fmt_uptime(3_661), "01:01:01");
        assert_eq!(fmt_uptime(15_127), "04:12:07");
        assert_eq!(fmt_uptime(360_000), "100:00:00");
    }
}
