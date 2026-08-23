//! The measurement benches (`5` to `9`) and the instrument bars that wrap them.
//!
//! [`bars`] holds the top banner and the bottom marker readout that every lab
//! preset carries; the rest are the benches themselves, one module per panel.

pub mod adc_loading;
pub mod bars;
pub mod fm_demod;
pub mod image_scope;
pub mod iq_constellation;
pub mod iq_diagnostics;
pub mod iq_histogram;
pub mod level_diagram;
pub mod rf_chain;
pub mod signal_characterization;
pub mod signal_metrics;
pub mod sweep_panel;
pub mod sweep_strip;
pub mod timing_diagnostics;
pub mod timing_stripchart;
pub mod timing_vitals;
