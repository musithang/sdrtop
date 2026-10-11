// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! IEEE 802.11 OFDM (clause 17, and ERP-OFDM on 2.4 GHz, clause 18): the
//! arc beside [`super::ble`] and [`super::bt`].
//!
//! **Two halves, compiled differently.** The receive side runs in the app:
//!
//! - [`detect`]: finding a frame's short training field in the stream.
//!
//! The transmit side is compiled for the tests only: written stage by stage
//! from the standard and held to its own worked example (IEEE 802.11-2024,
//! Annex I.1), it is the reference every receive stage is tested against.
//! Every constant cites its clause of that edition.
//!
//! - `annex`: the worked example's message, and how each stage is held to
//!   the annex without the annex's tables being copied here.
//! - `signal_field`: the rates, and the SIGNAL field that names one.
//! - `scrambler`: the DATA field's bits and the scrambler.
//! - `bcc`: the convolutional code and its puncturing.
//! - `interleave`: the block interleaver and its inverse.
//! - `map`: coded bits to constellation points, and back.
//! - `ofdm`: the subcarriers of one symbol, the pilots, and the training
//!   sequences.
//! - `ppdu`: the whole PPDU in time.
//! - `chain`: the bit chain both ways, a PSDU to its points and back.

pub mod detect;

#[cfg(test)]
pub mod annex;
#[cfg(test)]
pub mod bcc;
#[cfg(test)]
pub mod chain;
#[cfg(test)]
pub mod interleave;
#[cfg(test)]
pub mod map;
#[cfg(test)]
pub mod ofdm;
#[cfg(test)]
pub mod ppdu;
#[cfg(test)]
pub mod scrambler;
#[cfg(test)]
pub mod signal_field;
