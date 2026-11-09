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

pub mod decode;
pub mod ladder;
pub mod power;
pub mod track;

use std::sync::Arc;

use ratatui::{
    layout::Rect,
    style::{Color, Style},
};

use crate::state::ft8::{DecodeEntry, Shown};
use crate::state::SdrMetrics;
use crate::ui::panels::radio::ft8::{hhmmss, put, signed};

/// The heat's range, dB over the noise: the ladder's cells and the power's
/// columns, one quantity on one scale.
pub const HEAT_DB: (f64, f64) = (-5.0, 30.0);

/// `db` over the noise on the theme's own gradient (the waterfall's).
pub fn heat(t: &crate::Theme, db: f64) -> Color {
    t.palette_color((((db - HEAT_DB.0) / (HEAT_DB.1 - HEAT_DB.0)).clamp(0.0, 1.0)) as f32)
}

/// The sent tone's energy over the noise in symbol `s`, dB: the ladder's
/// lit cell.
pub fn sent_db(tr: &crate::signal::ft8::trace::Trace, s: usize) -> f64 {
    let noise = f64::from(tr.noise).max(f64::MIN_POSITIVE);
    let e = f64::from(tr.energies[s][usize::from(tr.tones[s]) + 1]);
    10.0 * (e / noise).max(1e-6).log10()
}

/// Below this body width the views that need room give way: the
/// frequency track and the power, leaving the ladder and the numbers.
pub const NARROW_BODY: u16 = 150;

/// The transmission to draw, or, written centred in `area`, why there is
/// none: before the first decode, or a chosen station whose decodes have
/// left the list.
pub fn shown_or_say(
    buf: &mut ratatui::buffer::Buffer,
    area: Rect,
    state: &SdrMetrics,
    theme: &crate::Theme,
) -> Option<Arc<DecodeEntry>> {
    let text = match state.ft8.shown() {
        Shown::Decode(d) => return Some(d),
        Shown::Waiting => "waiting for a decode".to_string(),
        Shown::Gone(call) => {
            format!("{call}: its decodes have left the list; trace no longer kept")
        }
    };
    say(buf, area, &text, theme);
    None
}

/// `text` centred in `area`, in the label colour.
pub fn say(buf: &mut ratatui::buffer::Buffer, area: Rect, text: &str, theme: &crate::Theme) {
    let y = area.y + area.height / 2;
    let x = area.x + area.width.saturating_sub(text.chars().count() as u16) / 2;
    put(buf, area, x, y, text, Style::default().fg(theme.label));
}

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
