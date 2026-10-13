// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! IEEE 802.11 OFDM (clause 17, and ERP-OFDM on 2.4 GHz, clause 18): the
//! arc beside [`super::ble`] and [`super::bt`].
//!
//! **Two halves, compiled differently.** The receive side runs in the app:
//!
//! - [`detect`]: finding a frame's short training field in the stream.
//! - [`sync`]: the offset, the timing and the long training field's
//!   verdict on whose frame it is.
//! - [`channel`]: the channel each subcarrier came through, with the
//!   uncertainty the noise and the floor's shape leave on it.
//! - [`sig`]: the symbols after the long training field, each equalised
//!   and righted by its pilots, read as BPSK at a rotation or scored for
//!   the axis its power is on.
//! - [`lsig`]: the SIGNAL field, its rate and length, and the frame's
//!   airtime.
//!
//! The stages a frame is built from are written stage by stage from the
//! standard and held to its own worked example (IEEE 802.11-2024, Annex
//! I.1). Their receive halves run in the app; their transmit halves are
//! compiled for the tests only, as the reference every receive stage is
//! tested against. Every constant cites its clause of that edition.
//!
//! - [`signal_field`]: the rates, and the SIGNAL field that names one.
//! - [`scrambler`]: the DATA field's bits and the scrambler.
//! - [`bcc`]: the convolutional code and its puncturing.
//! - [`interleave`]: the block interleaver and its inverse.
//! - [`map`]: coded bits to constellation points, and back.
//! - [`ofdm`]: the subcarriers of one symbol, the pilots, and the training
//!   sequences.
//!
//! And for the tests alone:
//!
//! - `annex`: the worked example's message, and how each stage is held to
//!   the annex without the annex's tables being copied here.
//! - `ppdu`: the whole PPDU in time.
//! - `chain`: the bit chain both ways, a PSDU to its points and back.

pub mod bcc;
pub mod channel;
pub mod detect;
pub mod format;
pub mod htsig;
pub mod interleave;
pub mod lsig;
pub mod map;
pub mod ofdm;
pub mod scrambler;
pub mod sig;
pub mod signal_field;
pub mod sync;

#[cfg(test)]
pub mod annex;
#[cfg(test)]
pub mod chain;
#[cfg(test)]
pub mod ppdu;
