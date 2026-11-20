// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Spectrum analysis: the FFT worker thread and the measurements it publishes.
//!
//! Split by **when the lock is held**, which is the property that matters in a
//! thread feeding the UI:
//!
//! - `frame`: per-frame DSP - decode, transform, average. Full rate, no lock.
//! - `analysis`: the measurements. Display rate, no lock, no clock.
//! - [`readout`]: what the Lab shows of them, paced for the eye (the carrier
//!   figures, held through a gap). Display only, no lock.
//! - [`publish`]: the one lock block.
//! - [`worker`]: the loop that sequences them.
//!
//! The first two, and the pure DSP the panels share (`carrier`, the carrier
//! at centre, and `acpr`, the adjacent channels), are `monas_spectrum`, named
//! here as they always were; the rest is the engine's, beside its state.

pub use monas_spectrum::*;

mod publish;
mod readout;
mod worker;

pub use worker::FftWorker;
