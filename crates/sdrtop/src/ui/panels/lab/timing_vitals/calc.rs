// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The arithmetic behind the readings. No theme, no width, no frame.

/// A duration as the bench prints it: hours past a day keep counting.
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
    fn uptime_carries_hours_and_pads() {
        assert_eq!(fmt_uptime(0), "00:00:00");
        assert_eq!(fmt_uptime(59), "00:00:59");
        assert_eq!(fmt_uptime(3_661), "01:01:01");
        assert_eq!(fmt_uptime(15_127), "04:12:07");
        assert_eq!(fmt_uptime(360_000), "100:00:00");
    }
}
