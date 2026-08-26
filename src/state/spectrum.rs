use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpectrumMarker {
    pub freq_hz: u64,
    pub label: String,
    #[serde(default)]
    pub channel_bw_hz: Option<u64>,
    #[serde(skip)]
    pub measured_bw_hz: Option<u64>,
}

/// How the spectrum trace is rendered (Design §4.2 / Grafikai §01). All three
/// share the same canvas chrome (grid, markers, cursor, peak-hold); only the
/// signal layer differs. Cycled live with `D` in spectrum focus; persisted in
/// `[display] spectrum_style`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpectrumStyle {
    /// Airy dot-cloud — a single point per bin, no fill or connecting line.
    Scatter,
    /// Solid height-gradient filled columns — a heavy "body", no thin edge.
    Fill,
    /// Sharp thin 2×4 braille trace over a soft dimmed body — the default.
    #[default]
    Braille,
}

impl SpectrumStyle {
    /// Cycle order for the `D` toggle: Braille → Fill → Scatter → Braille.
    pub fn next(self) -> Self {
        match self {
            Self::Braille => Self::Fill,
            Self::Fill => Self::Scatter,
            Self::Scatter => Self::Braille,
        }
    }

    /// Lower-case name for log messages (matches the serialized form).
    pub fn label(self) -> &'static str {
        match self {
            Self::Scatter => "scatter",
            Self::Fill => "fill",
            Self::Braille => "braille",
        }
    }
}

#[derive(Clone)]
pub struct SpectrumState {
    pub step_hz: u64,
    pub y_min: f32,
    pub y_max: f32,
    pub hold: Option<Arc<Vec<f32>>>,
    pub cursor_freq: Option<u64>,
    pub markers: Vec<SpectrumMarker>,
    pub pending_marker: Option<u64>,
    /// Trace render style; cycled with `D`. See [`SpectrumStyle`].
    pub style: SpectrumStyle,
}
