// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! FM, AM, CTCSS and pilot measurement in the engine: the measurements
//! (`monas_demod`, every part of it named here as it always was) and the
//! worker that runs them on the radio's stream and publishes what they read.

pub use monas_demod::*;

/// The demod thread. Mirrors [`crate::signal::FftWorker`]: owns its scratch
/// buffers, consumes raw blocks, and writes finished measurements into the shared
/// metrics.
mod worker;

pub use worker::DemodWorker;
