// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The arithmetic behind the readings. No theme, no width, no frame.

/// Binary MB/s ceiling of the USB link for this device: the byte rate at the
/// device's maximum sample rate. Honest per-device headroom reference rather
/// than a magic constant.
///
/// `bytes_per_pair` comes from the device's own geometry rather than the 2 this
/// assumed. A 16-bit radio moves twice the bytes at the same sample rate, and
/// telling it otherwise puts it comfortably inside a budget it is sitting on
/// the edge of.
pub(super) fn link_ceiling_mbps(sample_rate_max_hz: f64, bytes_per_pair: usize) -> f64 {
    (sample_rate_max_hz * bytes_per_pair as f64) / (1024.0 * 1024.0)
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
        let c = link_ceiling_mbps(20_000_000.0, 2);
        assert!((c - 38.147).abs() < 0.01, "got {c}");
        assert_eq!(link_ceiling_mbps(0.0, 2), 0.0);
    }

    /// A wider sample is more bytes at the same rate. The same 20 Msps on a
    /// 16-bit radio is twice the traffic, and the headroom reading has to say so
    /// or the panel will call a saturated link healthy.
    #[test]
    fn a_wider_sample_doubles_the_link_ceiling() {
        let narrow = link_ceiling_mbps(20_000_000.0, 2);
        let wide = link_ceiling_mbps(20_000_000.0, 4);
        assert!((wide - narrow * 2.0).abs() < 1e-9, "{wide} vs {narrow}");
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
