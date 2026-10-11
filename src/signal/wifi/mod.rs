// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! IEEE 802.11 OFDM (clause 17, and ERP-OFDM on 2.4 GHz, clause 18), from
//! the bits to the samples: the arc beside [`super::ble`] and [`super::bt`].
//!
//! **Compiled for the tests only, until a receiver in the app uses it.** What
//! is here is the transmit side of the standard, written stage by stage and
//! held to the standard's own worked example (IEEE 802.11-2024, Annex I.1) by
//! [`annex`]: the reference every later receive stage is tested against.
//! Every constant cites its clause of that edition.
//!
//! - [`annex`]: the worked example's message, and how each stage is held to
//!   the annex without the annex's tables being copied here.
//! - [`signal_field`]: the rates, and the SIGNAL field that names one.
//! - [`scrambler`]: the DATA field's bits and the scrambler.
//! - [`bcc`]: the convolutional code and its puncturing.
//! - [`interleave`]: the block interleaver and its inverse.

pub mod annex;
pub mod bcc;
pub mod interleave;
pub mod scrambler;
pub mod signal_field;
