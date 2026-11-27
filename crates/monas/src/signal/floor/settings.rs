// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The floor's one setting, the bin width, and what follows from it.
//!
//! **One setting, the rest derived.** A frame is `rate / bin_hz` samples,
//! because a transform resolves no finer than one over its length; the bins
//! are the frame's length; the usable span is the band's middle, because the
//! radio's own anti-alias filter occupies the edges; the floor is estimated
//! in blocks of about a kilohertz.

use serde::{Deserialize, Serialize};

/// The finest bin: a frame of four seconds. Finer and a line from the sky,
/// which wanders by a tenth of a hertz in seconds, smears over several bins.
pub const MIN_BIN_HZ: f64 = 0.25;

/// The coarsest bin. Coarser and the floor's structure, lines a few hertz
/// apart, merges into the noise it is measured against.
pub const MAX_BIN_HZ: f64 = 10.0;

/// The most bins: 0.25 Hz at 768 kS/s. Two sums of f64 per bin is about
/// 51 MB held by the worker at this size.
pub const MAX_BINS: usize = 3_200_000;

/// The share of the sampled band examined, centred: the outer tenth on each
/// side is the radio's anti-alias filter rolling off, where the floor falls
/// away and would read as structure.
pub const USABLE_FRACTION: f64 = 0.8;

/// The floor's block: about a kilohertz, and never fewer bins than this.
pub const MIN_BLOCK: usize = 64;

/// What the floor is asked to do: its bin width, and an optional span to
/// serve at full resolution.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FloorSetting {
    /// The bin width, Hz: 0.25 to 10.
    #[serde(default = "default_bin_hz")]
    pub bin_hz: f64,
    /// A span served at full resolution, for a closer look.
    #[serde(default)]
    pub zoom: Option<ZoomSpan>,
}

/// An absolute frequency span, Hz.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZoomSpan {
    pub from_hz: u64,
    pub to_hz: u64,
}

fn default_bin_hz() -> f64 {
    1.0
}

impl Default for FloorSetting {
    fn default() -> Self {
        FloorSetting {
            bin_hz: default_bin_hz(),
            zoom: None,
        }
    }
}

/// What follows from a setting at an input rate.
#[derive(Clone, Debug, PartialEq)]
pub struct Derived {
    /// Samples per frame.
    pub frame_len: usize,
    /// Bins per frame: the frame's length.
    pub bins: usize,
    /// The bin width delivered, `rate / frame_len`, which the asked width is
    /// rounded to.
    pub bin_hz: f64,
    /// The bins examined, the band's middle.
    pub usable: std::ops::Range<usize>,
    /// Bins per block of the floor estimate.
    pub block: usize,
}

/// Bins a zoom spans, at most.
pub const MAX_ZOOM_BINS: usize = 8192;

/// Bin 0's centre, Hz, absolute, at a tuning of `centre`.
pub fn low_edge_hz(d: &Derived, centre: u64) -> f64 {
    centre as f64 - (d.bins / 2) as f64 * d.bin_hz
}

/// The band examined, Hz, absolute: from the first usable bin's lower edge
/// to the last one's upper edge.
pub fn span_hz(d: &Derived, centre: u64) -> (f64, f64) {
    let low = low_edge_hz(d, centre);
    (
        low + (d.usable.start as f64 - 0.5) * d.bin_hz,
        low + (d.usable.end as f64 - 0.5) * d.bin_hz,
    )
}

/// Why a zoom cannot be had.
#[derive(Clone, Debug, PartialEq)]
pub enum ZoomMiss {
    /// It reaches outside the band examined.
    Outside(String),
    /// It spans more than [`MAX_ZOOM_BINS`].
    TooWide(String),
}

impl ZoomMiss {
    pub fn into_message(self) -> String {
        match self {
            ZoomMiss::Outside(s) | ZoomMiss::TooWide(s) => s,
        }
    }
}

/// The first and last bin of a zoom at a tuning of `centre`, or why it
/// cannot be had there.
pub fn zoom_bins(z: ZoomSpan, d: &Derived, centre: u64) -> Result<(usize, usize), ZoomMiss> {
    let low = low_edge_hz(d, centre);
    let k0 = ((z.from_hz as f64 - low) / d.bin_hz).ceil();
    let k1 = ((z.to_hz as f64 - low) / d.bin_hz).floor();
    if z.to_hz <= z.from_hz || k0 < d.usable.start as f64 || k1 >= d.usable.end as f64 || k1 < k0 {
        let span = span_hz(d, centre);
        return Err(ZoomMiss::Outside(format!(
            "the zoom {}–{} Hz reaches outside the band examined, {}–{} Hz",
            grouped(z.from_hz as usize),
            grouped(z.to_hz as usize),
            grouped(span.0.ceil() as usize),
            grouped(span.1.floor() as usize),
        )));
    }
    let (k0, k1) = (k0 as usize, k1 as usize);
    let n = k1 - k0 + 1;
    if n > MAX_ZOOM_BINS {
        return Err(ZoomMiss::TooWide(format!(
            "the zoom spans {} bins; at most {}",
            grouped(n),
            grouped(MAX_ZOOM_BINS)
        )));
    }
    Ok((k0, k1))
}

/// A number with a space between thousands, as the refusals print them.
pub(crate) fn grouped(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// What a setting comes to at `rate_hz`, or the limit it breaks.
pub fn derive(s: &FloorSetting, rate_hz: f64) -> Result<Derived, String> {
    if s.bin_hz.is_nan() || s.bin_hz < MIN_BIN_HZ {
        return Err(format!(
            "{} Hz bins are finer than the {MIN_BIN_HZ} Hz a frame of four seconds gives",
            s.bin_hz
        ));
    }
    if s.bin_hz > MAX_BIN_HZ {
        return Err(format!(
            "{} Hz bins are coarser than the {MAX_BIN_HZ} Hz the floor's structure allows",
            s.bin_hz
        ));
    }
    let frame_len = (rate_hz / s.bin_hz).round() as usize;
    if frame_len > MAX_BINS {
        return Err(format!(
            "{} Hz bins at {:.0} kS/s would be {} bins; at most {}",
            s.bin_hz,
            rate_hz / 1e3,
            grouped(frame_len),
            grouped(MAX_BINS)
        ));
    }
    let edge = ((1.0 - USABLE_FRACTION) / 2.0 * frame_len as f64).round() as usize;
    Ok(Derived {
        frame_len,
        bins: frame_len,
        bin_hz: rate_hz / frame_len as f64,
        usable: edge..frame_len - edge,
        block: ((1000.0 / s.bin_hz).round() as usize).max(MIN_BLOCK),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bins(hz: f64) -> FloorSetting {
        FloorSetting {
            bin_hz: hz,
            zoom: None,
        }
    }

    #[test]
    fn one_hz_at_768k_is_768000_bins_with_the_middle_80_percent_usable() {
        let d = derive(&bins(1.0), 768_000.0).unwrap();
        assert_eq!(d.frame_len, 768_000);
        assert_eq!(d.bins, 768_000);
        assert_eq!(d.usable, 76_800..691_200);
        assert_eq!(d.block, 1000);
    }

    #[test]
    fn bins_and_frame_length_are_refused_past_their_limits() {
        let why = derive(&bins(0.25), 912_000.0).unwrap_err();
        assert!(
            why.contains("3 648 000") && why.contains("3 200 000"),
            "{why}"
        );
        let why = derive(&bins(0.2), 192_000.0).unwrap_err();
        assert!(why.contains("0.25"), "{why}");
        let why = derive(&bins(20.0), 192_000.0).unwrap_err();
        assert!(why.contains("10"), "{why}");
        assert!(derive(&bins(10.0), 192_000.0).is_ok());
    }

    /// Left out of a command or a config, the bin is one hertz.
    #[test]
    fn the_bin_defaults_to_one_hertz() {
        let s: FloorSetting = serde_json::from_str("{}").unwrap();
        assert_eq!(s.bin_hz, 1.0);
        assert!(s.zoom.is_none());
    }
}
