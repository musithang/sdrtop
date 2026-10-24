// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

pub mod assigned;
pub mod ballistics;
pub mod ble;
pub mod bt;
pub mod demod;
// Public because `ui::widgets::reading` needs `dsp::uncertainty`: the
// rule that a value is printed to no more precision than its uncertainty
// supports is a numerical decision the UI has to be able to ask about.
pub mod dsp;
pub mod errors;
pub mod fft;
pub mod iq;
pub mod net;
pub mod noise_slope;
// Received and tested ahead of the worker and the face that will use it.
// `expect` fails the build as soon as something outside the tests reads it,
// so this line cannot outlive its reason.
#[cfg_attr(not(test), expect(dead_code))]
pub mod pocsag;
pub mod power;
pub mod rds;
pub mod rds_demod;
pub mod reference;
pub mod retune;
mod stats;
pub mod stream;
pub mod wifi;

pub use demod::DemodWorker;
pub use fft::FftWorker;
pub use iq::{corrected_moments, image_rejection_db, iq_correction_coeffs};
pub use net::worker::NetWorker;
pub use power::PowerWorker;
