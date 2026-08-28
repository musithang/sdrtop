//! Spectrum analysis: the FFT worker thread and the measurements it publishes.
//!
//! Split by **when the lock is held**, which is the property that matters in a
//! thread feeding the UI:
//!
//! - [`frame`]: per-frame DSP — decode, transform, average. Full rate, no lock.
//! - [`analysis`]: the measurements. Display rate, no lock, no clock.
//! - [`publish`]: the one lock block.
//! - [`worker`]: the loop that sequences them.
//!
//! Plus the pure DSP the panels share: [`carrier`] (the carrier at centre, its
//! occupied bandwidth and channel power) and [`acpr`] (the adjacent channels).

mod acpr;
mod analysis;
mod carrier;
mod frame;
mod publish;
mod worker;

pub use carrier::{centre_radius_bins, strongest_real_bin};
pub use worker::FftWorker;

/// Floor for a magnitude of zero, so silence reads as a number rather than −∞.
const DB_FLOOR: f32 = -160.0;
