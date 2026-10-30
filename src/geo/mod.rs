// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The Earth, as far as a radio map needs it: where a locator is, how far
//! and in what direction a place lies, how the globe is laid flat around the
//! listener, where the sun is, and where the land is.
//!
//! Pure functions of latitude and longitude in degrees (north and east
//! positive) and of Unix time; no radio, no screen. Apart from `signal/`,
//! because nothing here is a signal and more than one view will want it.
//!
//! - [`maidenhead`]: the amateur locator grid, both ways.
//! - [`sphere`]: great-circle distance, initial bearing, and the point a
//!   bearing and a distance reach.
//! - [`projection`]: azimuthal equidistant about the listener, with a
//!   logarithmic or a linear distance scale.
//! - [`sun`]: the subsolar point for a moment, and the sun's altitude
//!   anywhere, so the map can show day, twilight and night.
//! - [`land`]: Natural Earth's land at 1:110m, embedded, and whether a point
//!   is on it.
//!
//! **The Earth here is a sphere** of the mean radius. Distances are off from
//! the ellipsoid's by up to about half a per cent, which a terminal cell
//! cannot show and a map of radio paths does not need; the card says "about"
//! nowhere it would matter.

pub mod land;
pub mod maidenhead;
pub mod projection;
pub mod sphere;
pub mod sun;
