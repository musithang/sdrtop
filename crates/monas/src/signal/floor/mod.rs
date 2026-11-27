// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! A long exposure of the noise floor: the whole tuned band in fine bins,
//! summed frame after frame for as long as the tuning holds, and what rises
//! out of it.
//!
//! **Why sums.** Pure chance has a fingerprint: in a fine spectrum each
//! bin's power, frame after frame, is exponentially distributed, its spread
//! exactly its mean, and averaging T frames narrows that spread by the root
//! of T. Two running sums per bin (of the power and of its square) are
//! enough to say how deep the exposure has reached, which bins chance cannot
//! explain, and whether a bin behaves like chance, like a steady line or
//! like a source that clicks. Frames do not overlap, so they are independent
//! and those statistics are exact rather than approximate.
//!
//! - `settings`: the bin width, what follows from it, and its limits.
//! - `frames`: samples to frames to the two sums.
//! - `chance`: what chance looks like: the local floor under each bin, the
//!   depth reached, and the level chance does not pass.
//! - `lines`: the bins chance cannot explain, grouped into lines.
//! - `grids`: lines on one spacing, found only when chance could not have
//!   put them there.
//! - `spread`: whether a bin behaves like chance, a steady line or clicks.

pub mod chance;
pub mod frames;
pub mod grids;
pub mod lines;
pub mod settings;
pub mod spread;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use frames::{Frames, Sums};
pub use settings::{derive, Derived, FloorSetting, ZoomSpan};
