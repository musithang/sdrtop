// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

pub mod demod;
pub mod doppler;
pub mod fft;
pub mod ft8;
pub mod net;
pub mod noise_slope;
pub mod pocsag;
pub mod power;
pub mod reference;
pub mod retune;
pub mod rf_calc;

/// The Bluetooth LE receivers and measurements, under their old name.
pub use monas_ble as ble;
/// The classic Bluetooth receivers, and the SIG's assigned numbers they and
/// the LE side name companies and services by, under their old names.
pub use monas_bt as bt;
pub use monas_bt::assigned;
/// The building blocks, under the names the engine has always used for them.
pub use monas_dsp as dsp;
pub use monas_dsp::{ballistics, errors, iq, stats, stream};
/// The RDS decoder and the demodulator that feeds it, under their old names.
pub use monas_rds as rds;
pub use monas_rds::demod as rds_demod;
/// The 802.11 receive chain, under its old name.
pub use monas_wifi as wifi;

pub use demod::DemodWorker;
pub use fft::FftWorker;
pub use iq::{corrected_moments, image_rejection_db, iq_correction_coeffs};
pub use net::worker::NetWorker;
pub use power::PowerWorker;
