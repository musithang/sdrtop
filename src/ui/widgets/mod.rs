// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Drawing primitives shared across panels.
//!
//! Nothing here knows about layout or about any particular panel: these are the
//! braille strips, block bars, meters, sparklines, digit renderers, band-plan
//! lookups and read-out formatters that panels compose. If a helper is used by
//! exactly one panel it belongs in that panel, not here.

pub mod band_plan;
pub mod bigdigits;
pub mod canvas;
pub mod charts;
// Drawn by nothing until the pager face draws it. `expect` fails the build
// as soon as it is, so this line cannot outlive its reason.
#[cfg_attr(not(test), expect(dead_code))]
pub mod dotmatrix;
pub mod limit;
pub mod micro_common;
pub mod reading;
#[cfg(test)]
pub mod stem;
pub mod table;
pub mod timing_fmt;
