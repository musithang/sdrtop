// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Signal-processing primitives, shared and protocol-agnostic.
//!
//! **Nothing in here knows what it is filtering.** A resampler that has heard of
//! Bluetooth is a bug, and a filter that knows about FM channel spacing belongs
//! with the demodulator that chose that spacing. What lives here is arithmetic
//! with a closed-form answer, which is also what makes every part of it testable
//! against something better than "it looked right on screen".
//!
//! The split:
//!
//! * [`window`] - the transform windows, for spectral analysis. Hann, Hamming,
//!   Blackman over a whole frame.
//! * [`fir`] - filter design and streaming decimation. The window used *inside*
//!   `fir` shapes a filter kernel rather than a transform input, which is why
//!   the two are separate modules despite both saying "window".
//! * [`nco`] - the oscillator and the complex mixer, on an integer phase
//!   accumulator. Everything that has to move a signal in frequency without
//!   putting a phase step or a slow phase creep into it goes through here.
//! * [`resample`] - rational resampling by L/M, polyphase, on a filter designed
//!   by `fir` to a rejection the caller states. Moving a stream in frequency and
//!   moving it onto another rate are the two things needed to put an arbitrary
//!   radio on the grid a mode expects.
//!
//! Policy stays with its owner. `signal::demod` decides how sharp an FM channel
//! filter has to be and what decimation reaches its target rate; this module
//! only knows how to build the filter it is asked for.

pub mod correlate;
pub mod discriminate;
pub mod estimate;
pub mod fir;
pub mod nco;
pub mod resample;
#[cfg(test)]
pub mod testkit;
pub mod uncertainty;
pub mod window;

pub use window::{compute_window, WindowFn};
