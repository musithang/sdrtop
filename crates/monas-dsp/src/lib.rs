// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Signal-processing primitives, shared and protocol-agnostic: the building
//! blocks every signal crate of sdrtop's engine is made of, and the sample
//! vocabulary they all speak.
//!
//! Published only so that the programs built on it can be installed from
//! crates.io. It has no stability promise of its own: its version is
//! sdrtop's, and a version of it works with the same version of the other
//! `monas` crates and no other.
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
//! * [`sample`] - how a radio's bytes encode I/Q, the blocks they arrive in,
//!   and the decoder from one to the other.
//! * [`modulation`] - what the carrier at centre looks like, from its
//!   bandwidth alone.
//! * [`gfsk`] - the GFSK modulator, which the Bluetooth receivers build their
//!   references with and their tests their signals.
//!
//! Policy stays with its owner. The FM demodulator decides how sharp a channel
//! filter has to be and what decimation reaches its target rate; this crate
//! only knows how to build the filter it is asked for.

// The documentation is its developers', so a public item's comment may
// point at the private one that explains it. rustdoc still resolves every
// such link, and a broken one fails the doc build as it always did.
#![allow(rustdoc::private_intra_doc_links)]

pub mod ballistics;
pub mod carrier;
pub mod code;
pub mod correlate;
pub mod deviation;
pub mod discriminate;
pub mod errors;
pub mod estimate;
pub mod filter;
pub mod fir;
pub mod gfsk;
pub mod iq;
pub mod modulation;
pub mod nco;
pub mod resample;
pub mod sample;
pub mod special;
pub mod stats;
pub mod stream;
#[cfg(any(test, feature = "testing"))]
pub mod testkit;
pub mod timing;
pub mod uncertainty;
pub mod window;

pub use window::{compute_window, WindowFn};
