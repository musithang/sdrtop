//! Lab "instrument mode" measurement state — the REF / averaging / reference-
//! trace controls the lab presets' instrument-chrome (banner + marker bar) shows
//! and the lab spectrum reacts to. Driven from the banner focus (`b`); the marker
//! data itself lives in [`SpectrumState`](super::SpectrumState) (not duplicated).

use std::sync::Arc;

/// Smallest / largest selectable trace-averaging depth (`AVG ×N`).
pub const AVG_MIN: u16 = 1;
pub const AVG_MAX: u16 = 16;
/// dBFS bounds for the reference level.
pub const REF_MIN: f32 = -120.0;
pub const REF_MAX: f32 = 0.0;

/// A frozen Lab RF display snapshot (`[⎵]`/`[F]`). When present, the ADC-loading and
/// level-diagram panels render this captured state instead of the live stream, so the
/// bench can be studied without the histogram and traces moving. RX keeps running — only
/// the display is held. Captures everything the two panels derive from.
#[derive(Clone)]
pub struct RfFreeze {
    pub signed_hist: [u64; 32],
    pub peak_dbfs: f32,
    pub rms_dbfs: f32,
    pub clip_events: u64,
    pub snr_db: f32,
    pub amp_enabled: bool,
    pub lna_gain: u32,
    pub vga_gain: u32,
}

/// Measurement-state for the lab instrument-chrome.
#[derive(Clone)]
pub struct LabState {
    /// Reference level (dBFS) — drawn as a horizontal line on the lab spectrum,
    /// and the marker readout shows Δ-from-REF. `None` → `—`, no line.
    pub ref_dbfs: Option<f32>,
    /// Spectrum trace-averaging depth. Maps to the FFT EMA: `alpha = 1/avg_n`.
    /// `1` = no averaging (instant). Default `5` ≈ the historical `alpha = 0.2`.
    pub avg_n: u16,
    /// Captured reference trace (CAL): drawn as a static ghost on the lab spectrum
    /// for before/after comparison. `Some` ⇒ `CAL ✓`.
    pub ref_trace: Option<Arc<Vec<f32>>>,
    /// Lab IQ carrier/image marker override. `None` → the marker bar auto-tracks the
    /// strongest carrier and its mirror live; `Some((carrier_hz, image_hz))` pins
    /// them (set by `[M]`), so the readout freezes onto a chosen pair.
    pub iq_marker_pin: Option<(u64, u64)>,
    /// Lab RF auto-gain continuous-track latch (`[A]` toggles it once the chain is
    /// already optimal). When set, the rx poll task re-nudges LNA/VGA toward the
    /// optimal ADC level on drift; any manual gain key clears it. The RF Diagnostics
    /// chip `✓` follows this flag.
    pub rf_autotrack: bool,
    /// Lab RF display freeze (`[⎵]`/`[F]`): captured ADC-loading + level-diagram state,
    /// or `None` when live.
    pub rf_freeze: Option<RfFreeze>,
}

impl Default for LabState {
    fn default() -> Self {
        Self {
            ref_dbfs: None,
            avg_n: 5,
            ref_trace: None,
            iq_marker_pin: None,
            rf_autotrack: false,
            rf_freeze: None,
        }
    }
}

impl LabState {
    /// FFT EMA smoothing factor for the current averaging depth (`1/avg_n`).
    pub fn ema_alpha(&self) -> f32 {
        1.0 / self.avg_n.clamp(AVG_MIN, AVG_MAX) as f32
    }

    /// Nudge the averaging depth by `delta` steps, clamped to `[AVG_MIN, AVG_MAX]`.
    pub fn adjust_avg(&mut self, delta: i32) {
        let n = (self.avg_n as i32 + delta).clamp(AVG_MIN as i32, AVG_MAX as i32);
        self.avg_n = n as u16;
    }

    /// Nudge the reference level by `delta` dBFS, initialising to `-10` when unset,
    /// clamped to `[REF_MIN, REF_MAX]`.
    pub fn adjust_ref(&mut self, delta: f32) {
        let cur = self.ref_dbfs.unwrap_or(-10.0);
        self.ref_dbfs = Some((cur + delta).clamp(REF_MIN, REF_MAX));
    }

    /// `REF` banner field: e.g. `-10 dBFS`, or `—` when unset.
    pub fn ref_label(&self) -> String {
        match self.ref_dbfs {
            Some(db) => format!("{db:.0} dBFS"),
            None => "\u{2014}".to_string(),
        }
    }

    /// `AVG` banner field: `×8` when averaging, else `OFF`.
    pub fn avg_label(&self) -> String {
        if self.avg_n > 1 {
            format!("\u{00D7}{}", self.avg_n)
        } else {
            "OFF".to_string()
        }
    }

    /// `CAL` banner field: `✓` when a reference trace is captured, else `—`.
    pub fn cal_label(&self) -> &'static str {
        if self.ref_trace.is_some() {
            "\u{2713}"
        } else {
            "\u{2014}"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_reads_as_x5_no_ref_no_cal() {
        let s = LabState::default();
        assert_eq!(s.ref_label(), "\u{2014}"); // —
        assert_eq!(s.avg_label(), "\u{00D7}5"); // ×5
        assert_eq!(s.cal_label(), "\u{2014}"); // —
        assert!((s.ema_alpha() - 0.2).abs() < 1e-6); // preserves historical 0.2
    }

    #[test]
    fn avg_adjust_clamps_and_maps_to_alpha() {
        let mut s = LabState::default();
        s.adjust_avg(100);
        assert_eq!(s.avg_n, AVG_MAX);
        s.adjust_avg(-100);
        assert_eq!(s.avg_n, AVG_MIN);
        assert_eq!(s.avg_label(), "OFF");
        assert!((s.ema_alpha() - 1.0).abs() < 1e-6); // N=1 → no smoothing
    }

    #[test]
    fn ref_adjust_inits_and_clamps() {
        let mut s = LabState::default();
        s.adjust_ref(-5.0); // unset → starts at -10, then -5
        assert_eq!(s.ref_dbfs, Some(-15.0));
        s.adjust_ref(1000.0);
        assert_eq!(s.ref_dbfs, Some(REF_MAX));
        assert_eq!(s.ref_label(), "0 dBFS");
    }

    #[test]
    fn rf_bench_defaults_idle_and_live() {
        let s = LabState::default();
        assert!(!s.rf_autotrack, "auto-track latch starts off");
        assert!(s.rf_freeze.is_none(), "display starts live, not frozen");
    }

    #[test]
    fn cal_label_follows_ref_trace() {
        let mut s = LabState::default();
        assert_eq!(s.cal_label(), "\u{2014}");
        s.ref_trace = Some(Arc::new(vec![-50.0; 8]));
        assert_eq!(s.cal_label(), "\u{2713}");
    }
}
