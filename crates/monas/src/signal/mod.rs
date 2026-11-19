// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

pub mod assigned;
pub mod ble;
pub mod bt;
pub mod demod;
pub mod fft;
// Read only by tests until the FT8 worker and view use it. `expect` fails the
// build once nothing in it is unread, so this line cannot outlive its reason.
#[cfg_attr(not(test), expect(dead_code))]
pub mod ft8;
pub mod net;
pub mod noise_slope;
pub mod pocsag;
pub mod power;
pub mod rds;
pub mod rds_demod;
pub mod reference;
pub mod retune;
pub mod rf_calc;
pub mod wifi;

/// The building blocks, under the names the engine has always used for them.
pub use monas_dsp as dsp;
pub use monas_dsp::{ballistics, errors, iq, stats, stream};

pub use demod::DemodWorker;
pub use fft::FftWorker;
pub use iq::{corrected_moments, image_rejection_db, iq_correction_coeffs};
pub use net::worker::NetWorker;
pub use power::PowerWorker;
