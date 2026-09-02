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
pub mod charts;
pub mod micro_common;
pub mod timing_fmt;
