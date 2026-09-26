// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use std::collections::VecDeque;
use std::time::Instant;

#[derive(Clone)]
pub struct RadioState {
    pub frequency: u64,
    pub config_sample_rate: f64,
    pub actual_sample_rate: u32,
    pub bb_filter_hz: u32,
    /// One value per stage, in the order `caps.gain.stages()` lists them.
    ///
    /// **The single source of truth for gain.** It used to be two `u32` fields
    /// named after a HackRF, which is the shape the other two radios were forced
    /// into: an RTL-SDR's `vga` meant nothing and a SoapySDR device's `lna` was a
    /// whole-chain figure. Position, not name, decides what a value is now.
    ///
    /// `f64` because a stage can have a fractional step or a negative minimum;
    /// see [`crate::hardware::StageSpec`].
    pub gains: Vec<f64>,
    pub amp_enabled: bool,
    pub rx_enabled: bool,
    pub hw_streaming: bool,
    /// When the current RX session started - `Some` while streaming, `None` when
    /// stopped. Drives the micro_health session timer.
    pub rx_start_time: Option<Instant>,
    pub bytes_since_last_poll: u64,
    pub last_poll_time: Instant,
    pub current_throughput_bps: u64,
    pub throughput_history: VecDeque<u64>,
    pub sample_rate_history: VecDeque<u64>,
    /// Our own oscillator's error, once somebody has established it.
    ///
    /// `None` until then, which is not the same as zero: zero is a measurement
    /// and this is the absence of one. See [`FrequencyReference`].
    pub reference: Option<FrequencyReference>,
    /// Set by the key, cleared by whoever takes the capture.
    ///
    /// A flag rather than a channel because the measurement needs raw samples
    /// and the FFT worker is already holding them: asking it to look once is a
    /// bool, and a fourth sample feed for a once-a-session measurement would be
    /// a lot of plumbing for one keypress.
    pub reference_request: bool,
}

impl RadioState {
    /// The front stage's value, rounded, for the many readouts that still speak
    /// in whole dB.
    ///
    /// **A view, not a second copy.** The vector is the truth; this is the
    /// projection the existing panels were written against. A device whose
    /// stages have fractional steps is displayed to the nearest dB by these two
    /// until the panels learn otherwise, which is a display limit rather than a
    /// storage one.
    pub fn primary_gain(&self) -> u32 {
        Self::whole(self.gains.first().copied())
    }

    /// The second stage, or zero on a device that has only one.
    pub fn secondary_gain(&self) -> u32 {
        Self::whole(self.gains.get(1).copied())
    }

    /// Everything the chain is contributing, added up.
    ///
    /// What a single-knob device's readout means. On an RTL-SDR there is one
    /// stage so this equals the primary; on a SoapySDR device the knob sets a
    /// total that sdrtop then distributes, and this is the figure that was
    /// actually achieved.
    pub fn total_gain(&self) -> f64 {
        self.gains.iter().copied().filter(|v| v.is_finite()).sum()
    }

    /// One stage by position, exact.
    ///
    /// The exact-value pair to [`Self::set_stage_gain`], read from G8 where the
    /// knob starts distributing across stages. The two rounding views above are
    /// what the panels use until then.
    #[allow(dead_code)] // read from G8
    pub fn stage_gain(&self, index: usize) -> f64 {
        self.gains.get(index).copied().unwrap_or(0.0)
    }

    /// Set one stage by position, growing the vector if the caller is ahead of
    /// it. Nothing here snaps: that is [`crate::hardware::StageSpec::snap`]'s
    /// job and it needs the shape, which lives in `caps`.
    pub fn set_stage_gain(&mut self, index: usize, db: f64) {
        if self.gains.len() <= index {
            self.gains.resize(index + 1, 0.0);
        }
        self.gains[index] = db;
    }

    pub fn set_primary_gain(&mut self, db: u32) {
        self.set_stage_gain(0, db as f64);
    }

    pub fn set_secondary_gain(&mut self, db: u32) {
        self.set_stage_gain(1, db as f64);
    }

    fn whole(v: Option<f64>) -> u32 {
        v.filter(|x| x.is_finite())
            .map(|x| x.max(0.0).round() as u32)
            .unwrap_or(0)
    }
}

/// How long a frequency reference stays a reference.
///
/// **A policy, and stated as one rather than dressed up as physics.** A crystal
/// drifts with temperature and a board warms up; fifteen minutes is the
/// timescale over which a room and a radio change measurably. What actually
/// decides it is the asymmetry: expiring early costs one keypress, and expiring
/// late puts a stale correction on every ppm reading in the app where nobody
/// will question it. When somebody measures a HackRF's drift against
/// temperature, this becomes a number with a source and this paragraph goes.
pub const REFERENCE_STALE_S: u64 = 15 * 60;

/// What a ppm reading is worth, which depends entirely on what it was measured
/// against.
///
/// Design section 7.1. The three are not degrees of confidence in one quantity;
/// they are three different quantities that happen to share a unit, and a panel
/// showing one while meaning another is the failure this enum exists to make
/// impossible.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Provenance {
    /// No reference has been established. **Relative only**: differences between
    /// devices are valid, absolute values are not, and the panel says so.
    #[default]
    Unreferenced,
    /// The user named a transmitter they trust, and how far: its error is
    /// taken as zero within their stated accuracy. Relative to that
    /// transmitter, which is named on screen with the figure the user gave.
    ///
    /// Set from the census (`T` on a selected device, net-ux-polish-plan 4.7,
    /// [`FrequencyReference::from_trusted`]), where naming a transmitter is
    /// already the idiom. **Never promoted to [`Self::Traceable`]**: a
    /// user's statement about a device is not a standard station, however
    /// confident the statement.
    Referenced,
    /// Measured against a source whose accuracy is guaranteed by regulation.
    /// Absolute, within the stated uncertainty.
    Traceable,
}

impl Provenance {
    pub fn label(&self) -> &'static str {
        match self {
            Provenance::Unreferenced => "RELATIVE",
            Provenance::Referenced => "REFERENCED",
            Provenance::Traceable => "TRACEABLE",
        }
    }
}

/// Our own oscillator's error, once somebody has established it.
///
/// Design section 7.2: it carries its value, its uncertainty, its provenance
/// **and its age**, and it expires. A reference measured an hour ago in a cold
/// room is not a reference now, and the readings that depend on it fall back to
/// [`Provenance::Unreferenced`] rather than silently going on being corrected.
#[derive(Clone, Debug)]
pub struct FrequencyReference {
    /// Our oscillator's fractional error in ppm, and its standard uncertainty.
    pub ppm: f64,
    pub sigma_ppm: f64,
    /// What it was measured against, as captured.
    pub provenance: Provenance,
    /// The transmitter's name, for the panel to show beside the number.
    pub source: String,
    /// When it was captured.
    pub at: std::time::Instant,
    /// How close the estimate came to the Cramer-Rao bound for the block and
    /// SNR it was measured from (`signal::reference::CarrierOffset`), `None`
    /// for a reference no bounded estimator produced. Design section 5.4:
    /// the bound is the floor, and it is displayed.
    pub efficiency: Option<f64>,
    /// The census device a user-stated reference rests on, so the same `T`
    /// that trusted it can let it go; `None` for a standard station.
    pub trusted: Option<[u8; 6]>,
}

impl FrequencyReference {
    /// Our oscillator's error, from a transmitter the user trusts: `raw` is
    /// its offset as the air delivered it (their error minus ours, how the
    /// census keeps it), `stated_ppm` how far the user says its crystal can be
    /// off, `name` how the device is shown.
    ///
    /// **Their error is taken as zero, so ours is minus the reading.** The
    /// reading is `t - e`; with `t = 0 ± stated`, `e = -raw`, and its
    /// uncertainty is the reading's and the statement's in quadrature, the
    /// two being independent. The provenance is [`Provenance::Referenced`]
    /// and says whose word it rests on, `user-stated ±x ppm`, in the source
    /// every panel and export names.
    pub fn from_trusted(
        raw: crate::signal::dsp::uncertainty::Uncertain,
        stated_ppm: f64,
        name: &str,
        address: [u8; 6],
        at: std::time::Instant,
    ) -> Self {
        Self {
            ppm: -raw.value(),
            sigma_ppm: (raw.sigma().powi(2) + stated_ppm.powi(2)).sqrt(),
            provenance: Provenance::Referenced,
            source: format!("{name} (user-stated ±{stated_ppm} ppm)"),
            at,
            efficiency: None,
            trusted: Some(address),
        }
    }

    pub fn age(&self, now: std::time::Instant) -> std::time::Duration {
        now.saturating_duration_since(self.at)
    }

    /// **Declared here, never computed in a panel.** The same rule every lab
    /// panel's staleness already lives under: a panel that decided for itself
    /// when a reading went cold would be one more place for the answer to
    /// differ.
    pub fn is_stale(&self, now: std::time::Instant) -> bool {
        self.age(now).as_secs() >= REFERENCE_STALE_S
    }

    /// What this reference is worth *now*.
    ///
    /// A stale one is worth what no reference is worth, which is the whole point
    /// of section 7.2: it does not quietly keep being applied.
    pub fn effective(&self, now: std::time::Instant) -> Provenance {
        if self.is_stale(now) {
            Provenance::Unreferenced
        } else {
            self.provenance
        }
    }
}

impl RadioState {
    /// Correct a raw ppm reading for our own oscillator, and say what the result
    /// is worth.
    ///
    /// **Provenance travels with the number, and that is the whole function.**
    /// Every ppm reading in the app is their error *minus* ours; taking ours
    /// back out is arithmetic, and the interesting part is that the answer's
    /// meaning changes with what we know. With no reference, or a stale one,
    /// the raw reading comes back untouched and marked relative - untouched
    /// rather than corrected-by-zero, because a correction of zero is a claim
    /// and this is the absence of one.
    ///
    /// **The sign is `signal::reference`'s, followed through.** Our synthesiser
    /// lands at `nominal(1 + e)`, where `e` is [`FrequencyReference::ppm`]; a
    /// transmitter whose crystal is off by `t` sends on `nominal(1 + t)`. The
    /// difference, which is all a receiver ever sees, is `nominal(t - e)`, so
    /// the transmitter's own error is the reading *plus* ours. Written as a
    /// subtraction until the first caller arrived, with a test that checked
    /// the subtraction against itself; `the_correction_undoes_what_the_air_did`
    /// now builds both numbers from the physics instead.
    ///
    /// The uncertainties add in quadrature: ours and theirs are independent.
    pub fn corrected_ppm(
        &self,
        raw: crate::signal::dsp::uncertainty::Uncertain,
        now: std::time::Instant,
    ) -> (crate::signal::dsp::uncertainty::Uncertain, Provenance) {
        use crate::signal::dsp::uncertainty::Uncertain;
        let Some(r) = self.reference.as_ref().filter(|r| !r.is_stale(now)) else {
            return (raw, Provenance::Unreferenced);
        };
        let corrected = Uncertain::from_variance(
            raw.value() + r.ppm,
            raw.sigma().powi(2) + r.sigma_ppm.powi(2),
        );
        (corrected, r.effective(now))
    }

    /// A transmitter's frequency offset as the receiver measured it, turned
    /// into what it says about the transmitter's own crystal: the ppm, and
    /// the same error in kHz at `carrier_hz`. What both are worth is
    /// [`Self::offset_basis`], which the engine puts on the panel's chrome.
    ///
    /// **The one conversion every offset in the NET section goes through**,
    /// so a packet row, a census row and an export cannot come to three
    /// different answers about one clock. ppm first because a crystal's error
    /// is fractional: the same device reads 3 % more kHz on 2480 MHz than on
    /// 2402 MHz, and only ppm can be compared, or combined, across channels.
    pub fn transmitter_offset(
        &self,
        offset_hz: crate::signal::dsp::uncertainty::Uncertain,
        carrier_hz: f64,
        now: std::time::Instant,
    ) -> TransmitterOffset {
        let (ppm, _) = self.corrected_ppm(offset_ppm(offset_hz, carrier_hz), now);
        TransmitterOffset {
            khz: ppm.scale(carrier_hz * 1e-9),
            ppm,
        }
    }

    /// What every offset on screen is worth right now, for the chrome tag:
    /// the reference's provenance, and whether one was established and has
    /// since expired, which reads the same as none for the numbers but is a
    /// different thing to tell the user.
    pub fn offset_basis(&self, now: std::time::Instant) -> OffsetBasis {
        match self.reference.as_ref() {
            None => OffsetBasis {
                provenance: Provenance::Unreferenced,
                expired: false,
            },
            Some(r) => OffsetBasis {
                provenance: r.effective(now),
                expired: r.is_stale(now),
            },
        }
    }
}

/// A frequency offset in Hz at `carrier_hz`, as the fraction of the carrier it
/// is, in ppm. Uncorrected: what the air delivered, their error minus ours.
///
/// Its own function because two places need it and must agree: the census
/// stores offsets in ppm as they arrive (`signal::net::worker`), and
/// [`RadioState::transmitter_offset`] converts one for display.
pub fn offset_ppm(
    offset_hz: crate::signal::dsp::uncertainty::Uncertain,
    carrier_hz: f64,
) -> crate::signal::dsp::uncertainty::Uncertain {
    offset_hz.scale(1e6 / carrier_hz)
}

/// A transmitter's crystal error, from [`RadioState::transmitter_offset`].
#[derive(Clone, Copy, Debug)]
pub struct TransmitterOffset {
    pub ppm: crate::signal::dsp::uncertainty::Uncertain,
    pub khz: crate::signal::dsp::uncertainty::Uncertain,
}

/// What the offsets on screen are worth, from [`RadioState::offset_basis`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OffsetBasis {
    pub provenance: Provenance,
    /// A reference was established and has expired, so `provenance` has
    /// fallen back to [`Provenance::Unreferenced`].
    pub expired: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::dsp::uncertainty::Uncertain;
    use std::time::Duration;

    fn reference(ppm: f64, at: Instant, provenance: Provenance) -> FrequencyReference {
        FrequencyReference {
            ppm,
            sigma_ppm: 0.3,
            provenance,
            source: "WWV 10 MHz".to_string(),
            at,
            efficiency: None,
            trusted: None,
        }
    }

    fn radio(reference: Option<FrequencyReference>) -> RadioState {
        let mut r = crate::state::SdrMetrics::fixture().radio;
        r.reference = reference;
        r
    }

    /// **Provenance travels with the number.**
    ///
    /// A reading is our oscillator's error plus theirs. Subtracting ours is
    /// arithmetic; what makes it worth doing is that the answer's *meaning*
    /// changes, and a panel that got the number without the meaning would print
    /// an absolute claim it has no right to.
    #[test]
    fn a_reading_from_an_unreferenced_radio_is_relative_and_untouched() {
        let now = Instant::now();
        let raw = Uncertain::from_sigma(12.0, 0.4);

        let (out, p) = radio(None).corrected_ppm(raw, now);
        assert_eq!(p, Provenance::Unreferenced);
        // Untouched, not corrected by zero: a correction of zero is a claim, and
        // this is the absence of one. The uncertainty must not grow either.
        assert_eq!(out.value(), 12.0);
        assert_eq!(out.sigma(), 0.4);
    }

    #[test]
    fn a_traceable_reference_makes_the_reading_absolute() {
        let now = Instant::now();
        let raw = Uncertain::from_sigma(12.0, 0.4);
        let radio = radio(Some(reference(2.0, now, Provenance::Traceable)));

        let (out, p) = radio.corrected_ppm(raw, now);
        assert_eq!(p, Provenance::Traceable);
        assert!(
            (out.value() - 14.0).abs() < 1e-12,
            "ours goes back onto theirs"
        );
        // Independent, so in quadrature and never by simple addition.
        let want = (0.4f64.powi(2) + 0.3f64.powi(2)).sqrt();
        assert!((out.sigma() - want).abs() < 1e-12, "got {}", out.sigma());
        assert!(
            out.sigma() > 0.4,
            "correcting cannot make a reading sharper"
        );
    }

    /// **The correction undoes what the air did**, with both numbers built
    /// from the physics rather than from the function under test: a
    /// reference captured the way `signal::reference::capture` captures one,
    /// and a transmitter's offset arriving the way `nominal(t - e)` says it
    /// must. The earlier test checked the correction against its own
    /// arithmetic and passed with the sign backwards.
    #[test]
    fn the_correction_undoes_what_the_air_did() {
        let now = Instant::now();
        let ours = 3.0; // ppm, fast
        let theirs = 20.0; // ppm, the transmitter's crystal

        // The reference: WWV at 10 MHz, which a fast oscillator sees below
        // centre, through the capture's own conversion.
        let wwv = crate::signal::reference::standard_at(10_000_000).unwrap();
        let station_hz = 10e6 * (0.0 - ours) * 1e-6;
        let e = crate::signal::reference::lo_error_ppm(Uncertain::exact(station_hz), wwv);
        let mut radio = radio(Some(reference(e.value(), now, Provenance::Traceable)));
        radio.reference.as_mut().unwrap().sigma_ppm = 0.0;

        // The transmitter on BLE channel 37, as the receiver sees it.
        let carrier = 2402e6;
        let seen_hz = carrier * (theirs - ours) * 1e-6;
        let got = radio.transmitter_offset(Uncertain::exact(seen_hz), carrier, now);
        assert!(
            (got.ppm.value() - theirs).abs() < 1e-9,
            "got {}",
            got.ppm.value()
        );
        assert!((got.khz.value() - carrier * theirs * 1e-9).abs() < 1e-6);
        assert_eq!(radio.offset_basis(now).provenance, Provenance::Traceable);

        // With no reference the same reading is what the air delivered,
        // relative, and the kHz is the receiver's own figure unchanged.
        let bare = radio_none().transmitter_offset(Uncertain::exact(seen_hz), carrier, now);
        assert!((bare.ppm.value() - (theirs - ours)).abs() < 1e-9);
        assert!((bare.khz.value() * 1e3 - seen_hz).abs() < 1e-6);
        assert_eq!(
            radio_none().offset_basis(now).provenance,
            Provenance::Unreferenced
        );
    }

    fn radio_none() -> RadioState {
        radio(None)
    }

    #[test]
    fn the_basis_tells_an_expired_reference_from_none() {
        let now = Instant::now();
        let none = radio(None).offset_basis(now);
        assert_eq!(none.provenance, Provenance::Unreferenced);
        assert!(!none.expired);

        let fresh = radio(Some(reference(2.0, now, Provenance::Traceable))).offset_basis(now);
        assert_eq!(fresh.provenance, Provenance::Traceable);
        assert!(!fresh.expired);

        let old = now - Duration::from_secs(REFERENCE_STALE_S);
        let gone = radio(Some(reference(2.0, old, Provenance::Traceable))).offset_basis(now);
        assert_eq!(gone.provenance, Provenance::Unreferenced);
        assert!(gone.expired);
    }

    /// **A stale reference falls back rather than quietly going on being
    /// applied.** Design section 7.2, and the failure it prevents is the quiet
    /// one: an hour-old correction from a cold room, still subtracted, on a
    /// number nobody will re-derive.
    #[test]
    fn a_stale_reference_falls_back_to_unreferenced() {
        let now = Instant::now();
        let old = now - Duration::from_secs(REFERENCE_STALE_S + 1);
        let raw = Uncertain::from_sigma(12.0, 0.4);
        let radio = radio(Some(reference(2.0, old, Provenance::Traceable)));

        let (out, p) = radio.corrected_ppm(raw, now);
        assert_eq!(
            p,
            Provenance::Unreferenced,
            "a stale reference is no reference"
        );
        assert_eq!(out.value(), 12.0, "and the old correction is not applied");
        assert_eq!(out.sigma(), 0.4);
    }

    /// Expiry is declared by the state, on the interval the state names, and a
    /// panel never works it out for itself.
    #[test]
    fn the_reference_expires_on_the_declared_interval() {
        let now = Instant::now();
        let fresh = reference(2.0, now, Provenance::Traceable);
        assert!(!fresh.is_stale(now));
        assert_eq!(fresh.effective(now), Provenance::Traceable);

        // One second short of the interval is still a reference.
        let nearly = reference(
            2.0,
            now - Duration::from_secs(REFERENCE_STALE_S - 1),
            Provenance::Traceable,
        );
        assert!(!nearly.is_stale(now));
        assert_eq!(nearly.effective(now), Provenance::Traceable);

        // The interval itself is the boundary, and it is inclusive.
        let expired = reference(
            2.0,
            now - Duration::from_secs(REFERENCE_STALE_S),
            Provenance::Traceable,
        );
        assert!(expired.is_stale(now));
        assert_eq!(expired.effective(now), Provenance::Unreferenced);
        // The stored provenance is untouched: what expired is its worth now, not
        // the record of what it was measured against.
        assert_eq!(expired.provenance, Provenance::Traceable);
        assert!(expired.age(now).as_secs() >= REFERENCE_STALE_S);
    }

    /// A reference the user established against a transmitter they trust is
    /// still only as good as that trust, and it says so.
    #[test]
    fn a_referenced_radio_is_not_a_traceable_one() {
        let now = Instant::now();
        let radio = radio(Some(reference(2.0, now, Provenance::Referenced)));
        let (_, p) = radio.corrected_ppm(Uncertain::from_sigma(12.0, 0.4), now);
        assert_eq!(p, Provenance::Referenced);
        assert_eq!(p.label(), "REFERENCED");
        assert_eq!(Provenance::Unreferenced.label(), "RELATIVE");
    }

    /// **A trusted transmitter corrects every other clock, built from the
    /// physics rather than from the arithmetic under test.** Our oscillator
    /// 10 ppm fast, a trusted device dead on, another device 25 ppm fast: the
    /// air shows them at -10 and +15. Trusting the first recovers our +10,
    /// and the second corrects to its true +25. The provenance says whose word
    /// it rests on and is never traceable.
    #[test]
    fn a_trusted_transmitter_recovers_our_error_and_corrects_the_rest() {
        use crate::signal::dsp::uncertainty::Uncertain;
        let (ours, trusted_true, other_true) = (10.0, 0.0, 25.0);
        let seen = |t: f64| Uncertain::from_sigma(t - ours, 0.3);
        let now = std::time::Instant::now();
        let r = FrequencyReference::from_trusted(
            seen(trusted_true),
            2.0,
            "a4:83:e7:1c:09:be",
            [0xa4, 0x83, 0xe7, 0x1c, 0x09, 0xbe],
            now,
        );
        assert!((r.ppm - ours).abs() < 1e-12, "{}", r.ppm);
        assert!((r.sigma_ppm - (0.3f64.powi(2) + 4.0).sqrt()).abs() < 1e-12);
        assert_eq!(r.provenance, Provenance::Referenced);
        assert_eq!(r.source, "a4:83:e7:1c:09:be (user-stated ±2 ppm)");

        let mut radio = crate::state::SdrMetrics::fixture().radio;
        radio.reference = Some(r);
        let (corrected, provenance) = radio.corrected_ppm(seen(other_true), now);
        assert!(
            (corrected.value() - other_true).abs() < 1e-12,
            "{corrected:?}"
        );
        assert_eq!(provenance, Provenance::Referenced);
    }
}
