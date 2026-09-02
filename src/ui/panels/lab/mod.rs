// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The measurement benches (`5` to `9`) and the instrument bars that wrap them.
//!
//! [`bars`] holds the top banner and the bottom marker readout that every lab
//! preset carries, and `rf_bench` the row vocabulary the two RF bench columns
//! share; the rest are the benches themselves, one module per panel.

pub mod adc_loading;
pub mod bars;
pub mod fm_demod;
pub mod image_scope;
pub mod iq_constellation;
pub mod iq_diagnostics;
pub mod iq_histogram;
pub mod level_diagram;
mod rf_bench;
pub mod rf_chain;
pub mod signal_characterization;
pub mod signal_metrics;
pub mod sweep_panel;
pub mod sweep_strip;
pub mod timing_diagnostics;
pub mod timing_stripchart;
pub mod timing_vitals;
