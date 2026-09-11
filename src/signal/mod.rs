// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

pub mod ble;
pub mod demod;
// Public from N8: `ui::widgets::reading` needs `dsp::uncertainty`, because the
// rule that a value is printed to no more precision than its uncertainty
// supports is a numerical decision the UI has to be able to ask about.
pub mod dsp;
pub mod fft;
pub mod iq;
pub mod net;
pub mod noise_slope;
pub mod power;
pub mod rds;
pub mod rds_demod;
pub mod reference;
mod stats;
pub mod stream;

pub use demod::DemodWorker;
pub use fft::FftWorker;
pub use iq::{corrected_moments, image_rejection_db, iq_correction_coeffs};
pub use net::worker::NetWorker;
pub use power::PowerWorker;
