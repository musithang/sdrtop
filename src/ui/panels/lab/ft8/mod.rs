// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The FT8 Lab's panels: the decoder's own measurements of what it heard,
//! drawn from the traces and accounts the worker publishes.
//!
//! The Lab shares World's worker, stations and selection (`L` crosses
//! between them), so every panel here reads the transmission
//! [`crate::state::ft8::Ft8State::shown`] picks and never chooses its own.
//! Each says what it cannot show rather than drawing a guess: before the
//! first decode it waits, and for a station whose decodes have left the list
//! it says the trace is no longer kept (POLICY.md, rule 2).

pub mod ladder;

use crate::state::ft8::DecodeEntry;
use crate::ui::panels::radio::ft8::{hhmmss, signed};

/// The transmission a panel's title names: its message, cycle, SNR and
/// tone 0.
pub fn caption(d: &DecodeEntry) -> String {
    let p = &d.decode.physics;
    format!(
        "  {}  {}  {} dB  {:.1} Hz",
        d.decode.message.text(),
        hhmmss(d.cycle_unix),
        signed(p.snr_db, 0),
        p.f0
    )
}
