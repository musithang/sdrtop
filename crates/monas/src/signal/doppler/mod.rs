// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! A narrowband carrier spectrogram: a few carriers watched closely, each as
//! rows of a narrow spectrum around it, each row dated and its strongest line
//! given with its uncertainty.
//!
//! **General on purpose.** Each carrier carries its own span and resolution,
//! and everything else (the narrow rate, the window, the hop) follows from
//! those two, so the same measurement watches the ionosphere move a time
//! signal by a tenth of a hertz, a satellite sweep ten kilohertz in a pass,
//! or a transmitter's crystal drift as it warms.
//!
//! **What every frequency here is relative to.** A row's offsets are from the
//! carrier's nominal frequency *as received through our oscillator*, so our
//! own error is in every carrier's rows. With two or more carriers it is the
//! same fraction of each, and the differential against the first has none of
//! it; that is the one figure offered as free of our oscillator.
//!
//! - `settings`: a carrier's span and resolution, what follows from them, and
//!   whether the tuning can hold it.
//! - `track`: one carrier's samples to dated rows and their strongest line.
//! - `differential`: a carrier's shift against the first's, our error gone.

pub mod differential;
pub mod settings;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod track;

pub use differential::{differential, pair};
pub use settings::{derive, placement, CarrierSetting, Derived, MAX_CARRIERS};
pub use track::{Line, Row, Tracker};
