// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The FT8 view's panels, and what they share.
//!
//! - [`map`]: the world around you, as you hear it.

pub mod map;

use crate::state::{Ft8State, SdrMetrics, Station};
use ratatui::style::Color;

/// How fresh a station is, measured from the newest decoded cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Age {
    /// Heard in the newest cycle.
    Now,
    /// Within the last 45 s.
    Recent,
    /// Within five minutes.
    Earlier,
    /// Longer ago, until it leaves the table.
    Old,
}

/// The newest cycle that was decoded, Unix seconds.
pub fn newest_cycle(f: &Ft8State) -> Option<f64> {
    f.cycles
        .iter()
        .rev()
        .find(|c| c.outcome == crate::state::CycleOutcome::Decoded)
        .map(|c| c.start_unix)
}

pub fn age(s: &Station, newest: Option<f64>) -> Age {
    let Some(newest) = newest else {
        return Age::Old;
    };
    match newest - s.last_unix {
        d if d <= 0.0 => Age::Now,
        d if d <= 45.0 => Age::Recent,
        d if d <= 300.0 => Age::Earlier,
        _ => Age::Old,
    }
}

/// A station's ink: calling you outranks its age.
pub fn ink(s: &Station, age: Age, theme: &crate::Theme) -> Color {
    if s.called_you {
        return theme.status_crit;
    }
    match age {
        Age::Now => theme.value_hi,
        Age::Recent => theme.value,
        Age::Earlier => theme.label,
        Age::Old => theme.border_dim,
    }
}

/// The time the map's sun is drawn for: the cycle being heard, else the
/// newest one. `None` before the stream has a time.
pub fn sun_time(m: &SdrMetrics) -> Option<f64> {
    m.ft8
        .hearing
        .or_else(|| m.ft8.cycles.back().map(|c| c.start_unix))
}

/// `HH:MM` UTC of a Unix time.
pub fn hhmm(unix: f64) -> String {
    let s = unix.rem_euclid(86_400.0) as u64;
    format!("{:02}:{:02}", s / 3600, (s / 60) % 60)
}
