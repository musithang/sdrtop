// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! A carrier's two settings, what follows from them, and whether the tuning
//! can hold it.
//!
//! **Two settings, the rest derived.** `span_hz` is the half-width watched
//! around the carrier, `resolution_hz` the finest separation between two
//! lines that is wanted. The window is `1 / resolution_hz`, because a
//! transform resolves no finer than one over its length; the hop is half the
//! window; the narrow rate is an integer fraction of the input rate at or
//! above `2.5 · span_hz`, which puts the span inside the band with room for
//! the anti-alias filter's skirt on either side (the 2.5 is Nyquist's 2 and
//! a quarter of the band for the skirt). The fraction is the largest that
//! holds the span, rounded down to a multiple of [`LAST_STAGE`] when it is
//! large enough to split into an average and a filter.
//!
//! **The hop is counted in input pairs, not narrow samples.** A window ends
//! at every input pair that is a multiple of the hop, so two carriers of one
//! resolution end their windows at the same moment whatever their spans, and
//! share the window's number. That shared number is what the differential
//! pairs them by.

use serde::{Deserialize, Serialize};

/// The most carriers watched at once. Each costs a mixer and a decimator at
/// the full input rate, and four keeps that a small part of one core at the
/// widest rate Monas runs; the differential needs only two.
pub const MAX_CARRIERS: usize = 4;

/// The pairs a second the mixers may take between them, carriers times
/// input rate. Measured in a release build: a carrier costs 0.53 % of a core
/// at 768 kS/s and 13 % at 20 MS/s, so four at 20 MS/s were 52 %; sixty
/// million keeps every setting under 40 %, which at the HF+'s rate is no
/// limit at all and at a HackRF's is three carriers.
pub const MIX_BUDGET_PAIRS_PER_S: f64 = 60e6;

/// Whether `carriers` mixers fit [`MIX_BUDGET_PAIRS_PER_S`] at
/// `input_rate_hz`, or how many do.
pub fn within_budget(carriers: usize, input_rate_hz: f64) -> Result<(), String> {
    if carriers as f64 * input_rate_hz <= MIX_BUDGET_PAIRS_PER_S {
        return Ok(());
    }
    let most = (MIX_BUDGET_PAIRS_PER_S / input_rate_hz).floor() as usize;
    Err(format!(
        "{carriers} carriers at {:.0} MS/s would take too much of a core; at most {most} at this rate",
        input_rate_hz / 1e6
    ))
}

/// The longest row: `2 · span_hz / resolution_hz` bins. Four thousand values
/// a row is a few hundred kilobytes a minute over the API at the fastest
/// hop a setting this long can have, and finer than any screen draws.
pub const MAX_BINS: usize = 4096;

/// The longest window, in seconds: a minute. Longer and the medium being
/// measured (the ionosphere, a satellite's pass) moves inside one window, so
/// the line smears and the resolution claimed is not the one delivered.
pub const MAX_WINDOW_S: f64 = 60.0;

/// The narrow rate's margin over the span, per side: Nyquist's two and a
/// quarter of the band for the filter's skirt.
pub const RATE_OVER_SPAN: f64 = 2.5;

/// The decimator's last stage: a filter that divides by this much, after an
/// average that divides by the rest. Thirty-two puts the average's first
/// alias zone thirty-two narrow bandwidths away, where its own nulls hold a
/// carrier off by more than forty decibels, and leaves the filter a short
/// one to run at a rate already low.
pub const LAST_STAGE: usize = 32;

/// How far from the band's edges a carrier must stay: the radio's own
/// anti-alias roll-off occupies the last few kilohertz of every capture.
pub const EDGE_GUARD_HZ: f64 = 10_000.0;

/// How far from the tuned centre a carrier must stay: the DC artefact sits
/// on the centre, and a kilohertz clears it at every rate Monas runs.
pub const DC_GUARD_HZ: f64 = 1_000.0;

/// One carrier to watch: its absolute frequency and its two settings.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CarrierSetting {
    /// The carrier's nominal frequency, Hz.
    pub hz: u64,
    /// The half-width watched around it, Hz: 25 when left out.
    #[serde(default = "default_span")]
    pub span_hz: f64,
    /// The finest separation between two lines that is wanted, Hz: 0.1
    /// when left out.
    #[serde(default = "default_resolution")]
    pub resolution_hz: f64,
}

/// The ionosphere's span: its wander is a fraction of a hertz, a keyed time
/// signal's comb a few hertz, and co-channel stations within ten.
fn default_span() -> f64 {
    25.0
}

/// The ionosphere's resolution: a tenth of a hertz resolves its wander and a
/// 1 Hz keying comb, in a 10 s window the F2 layer barely moves in.
fn default_resolution() -> f64 {
    0.1
}

/// What a carrier's settings come to at one input rate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Derived {
    /// The rate the carrier is decimated to, S/s.
    pub narrow_rate_hz: f64,
    /// Input pairs per narrow sample.
    pub factor: usize,
    /// Narrow samples in one window.
    pub window: usize,
    /// Input pairs from one window's end to the next.
    pub hop_pairs: u64,
    /// Values in one row: `2 · span_hz / resolution_hz`.
    pub bins: usize,
}

/// What `s` comes to at `input_rate_hz`, or the limit it breaks.
pub fn derive(s: &CarrierSetting, input_rate_hz: f64) -> Result<Derived, String> {
    let (span, res) = (s.span_hz, s.resolution_hz);
    if !(span.is_finite() && span > 0.0 && res.is_finite() && res > 0.0) {
        return Err(format!(
            "a span of {span} Hz at a resolution of {res} Hz is not a setting: both must be above zero"
        ));
    }
    if !(input_rate_hz.is_finite() && input_rate_hz > 0.0) {
        return Err("the radio has no sample rate yet".to_string());
    }
    let bins = (2.0 * span / res).round() as usize;
    if bins > MAX_BINS {
        return Err(format!(
            "a row of {bins} bins (2 x {span} Hz / {res} Hz) is longer than the {MAX_BINS} a row may hold"
        ));
    }
    let window_s = 1.0 / res;
    if window_s > MAX_WINDOW_S {
        return Err(format!(
            "a {window_s:.0} s window (1 / {res} Hz) is longer than the {MAX_WINDOW_S:.0} s one may last"
        ));
    }
    if span > input_rate_hz / 10.0 {
        return Err(format!(
            "a span of {span} Hz is wider than a tenth of the sample rate ({:.0} Hz)",
            input_rate_hz / 10.0
        ));
    }
    // The most the rate may be divided by and still hold the span; above
    // twice the last stage's factor it is rounded down to a multiple of it,
    // so the decimator splits into an average and a filter exactly.
    let most = ((input_rate_hz / (RATE_OVER_SPAN * span)).floor() as usize).max(1);
    let factor = if most >= 2 * LAST_STAGE {
        most / LAST_STAGE * LAST_STAGE
    } else {
        most
    };
    let narrow_rate_hz = input_rate_hz / factor as f64;
    Ok(Derived {
        narrow_rate_hz,
        factor,
        window: (narrow_rate_hz * window_s).round() as usize,
        hop_pairs: (input_rate_hz * window_s / 2.0).round() as u64,
        bins,
    })
}

/// Where `s` sits in a capture centred on `centre_hz`: its offset from the
/// centre in Hz, or why the tuning cannot hold it.
pub fn placement(s: &CarrierSetting, centre_hz: u64, input_rate_hz: f64) -> Result<f64, String> {
    let offset = s.hz as f64 - centre_hz as f64;
    let usable = input_rate_hz / 2.0 - EDGE_GUARD_HZ;
    if offset.abs() > usable {
        return Err(format!(
            "{:.3} MHz is outside the usable span, {:.3} to {:.3} MHz",
            s.hz as f64 / 1e6,
            (centre_hz as f64 - usable) / 1e6,
            (centre_hz as f64 + usable) / 1e6
        ));
    }
    if offset.abs() <= DC_GUARD_HZ {
        return Err(format!(
            "{:.3} MHz is within {DC_GUARD_HZ:.0} Hz of the tuned centre, on the DC artefact: tune a little away",
            s.hz as f64 / 1e6
        ));
    }
    Ok(offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ionosphere(hz: u64) -> CarrierSetting {
        CarrierSetting {
            hz,
            span_hz: 25.0,
            resolution_hz: 0.1,
        }
    }

    /// The ionosphere's setting at the HF+'s rate, as the design works it out.
    #[test]
    fn the_ionosphere_derives_as_the_design_says() {
        let d = derive(&ionosphere(9_996_000), 768_000.0).unwrap();
        assert_eq!(d.narrow_rate_hz, 62.5);
        assert_eq!(d.factor, 12_288);
        assert_eq!(d.window, 625);
        assert_eq!(d.hop_pairs, 3_840_000);
        assert_eq!(d.bins, 500);
    }

    /// Every use the design names derives, so the settings are proven beyond
    /// the first one.
    #[test]
    fn every_row_of_the_uses_table_derives() {
        let at = |span_hz, resolution_hz| {
            derive(
                &CarrierSetting {
                    hz: 0,
                    span_hz,
                    resolution_hz,
                },
                768_000.0,
            )
            .unwrap()
        };
        let sat = at(10_000.0, 10.0);
        assert_eq!(
            (sat.narrow_rate_hz, sat.factor, sat.bins),
            (25_600.0, 30, 2000)
        );
        // Above 64 the factor is a multiple of the last stage's 32, so the
        // narrow rate lands a little above the least the span needs.
        let meteor = at(1_000.0, 2.0);
        assert_eq!((meteor.factor, meteor.bins), (288, 1000));
        let drift = at(500.0, 0.5);
        assert_eq!((drift.factor, drift.bins), (608, 2000));
        assert!(meteor.narrow_rate_hz >= 2.5 * 1_000.0 && drift.narrow_rate_hz >= 2.5 * 500.0);
    }

    /// A setting that cannot be honoured is refused with the number that
    /// broke the limit and the limit, never rounded into one that can.
    #[test]
    fn a_setting_that_breaks_a_limit_is_refused_with_the_limit() {
        let bins = derive(
            &CarrierSetting {
                hz: 0,
                span_hz: 25.0,
                resolution_hz: 0.01,
            },
            768_000.0,
        )
        .unwrap_err();
        assert!(
            bins.contains("5000 bins") && bins.contains("4096"),
            "{bins}"
        );
        let window = derive(
            &CarrierSetting {
                hz: 0,
                span_hz: 1.0,
                resolution_hz: 0.01,
            },
            768_000.0,
        )
        .unwrap_err();
        assert!(
            window.contains("100 s") && window.contains("60 s"),
            "{window}"
        );
        let wide = derive(
            &CarrierSetting {
                hz: 0,
                span_hz: 100_000.0,
                resolution_hz: 100.0,
            },
            768_000.0,
        )
        .unwrap_err();
        assert!(wide.contains("a tenth of the sample rate"), "{wide}");
    }

    /// A carrier must be inside the captured span and off the DC artefact.
    #[test]
    fn a_carrier_outside_the_span_or_on_the_dc_is_refused() {
        let centre = 9_946_000;
        assert_eq!(
            placement(&ionosphere(9_996_000), centre, 768_000.0),
            Ok(50_000.0)
        );
        let dc = placement(&ionosphere(9_946_500), centre, 768_000.0).unwrap_err();
        assert!(dc.contains("DC"), "{dc}");
        let out = placement(&ionosphere(10_330_000), centre, 768_000.0).unwrap_err();
        assert!(out.contains("9.572 to 10.320 MHz"), "{out}");
    }

    /// The mixer's budget: four carriers at the HF+'s rate, three at a
    /// HackRF's 20 MS/s, and the refusal says how many fit.
    #[test]
    fn the_carriers_must_fit_the_mixers_budget() {
        assert_eq!(within_budget(4, 768_000.0), Ok(()));
        assert_eq!(within_budget(3, 20_000_000.0), Ok(()));
        let why = within_budget(4, 20_000_000.0).unwrap_err();
        assert!(
            why.contains("at most 3") && why.contains("20 MS/s"),
            "{why}"
        );
    }

    /// Left out of a command or a config, span and resolution are the
    /// ionosphere's.
    #[test]
    fn span_and_resolution_default_to_the_ionosphere() {
        let s: CarrierSetting = serde_json::from_str(r#"{"hz": 9996000}"#).unwrap();
        assert_eq!(s, ionosphere(9_996_000));
    }
}
