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

pub mod bearings;
pub mod clock;
pub mod decode;
pub mod hour;
pub mod ladder;
pub mod power;
pub mod reach;
pub mod sensitivity;
pub mod skip;
pub mod slots;
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

/// A decode placed on the map of distances: when, how far, which way, how
/// strong, who. Placed only by a locator the station sent itself (the
/// station table's latest), and only with your position set.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub unix: f64,
    pub km: f64,
    pub bearing: f64,
    pub snr: f64,
    pub call: Arc<str>,
}

/// The hour's decodes placed, and how many could not be (no locator heard,
/// or no sender in full). `None` without your position.
pub fn placed(m: &SdrMetrics) -> Option<(Vec<Placed>, usize)> {
    let home = m.home.position()?;
    let mut out = Vec::new();
    let mut unplaced = 0;
    for r in m.ft8.history.iter() {
        let there = r
            .call
            .as_deref()
            .and_then(|c| m.ft8.stations.get(c))
            .and_then(|s| s.locator.as_deref())
            .and_then(crate::geo::maidenhead::centre);
        match (there, &r.call) {
            (Some(there), Some(call)) => out.push(Placed {
                unix: r.cycle_unix,
                km: crate::geo::sphere::distance_km(home, there),
                bearing: crate::geo::sphere::bearing_deg(home, there),
                snr: f64::from(r.snr_db),
                call: Arc::clone(call),
            }),
            _ => unplaced += 1,
        }
    }
    Some((out, unplaced))
}

/// The stretch of time the hour's views cover: from the first cycle kept
/// (an hour back at most) to the end of the newest. `None` before any.
pub fn window(m: &SdrMetrics) -> Option<(f64, f64)> {
    let hour = &m.ft8.cycle_hour;
    let t1 = hour.back()?.start_unix + 15.0;
    let t0 = hour
        .front()?
        .start_unix
        .max(t1 - crate::state::ft8::HISTORY_SECONDS);
    Some((t0, t1))
}

/// The SNR scale the Band view's colours share, dB (sdrtop's, in 2500 Hz).
pub const SNR_RANGE: (f64, f64) = (-24.0, 6.0);

/// A mark's colour for an SNR: the theme's gradient over [`SNR_RANGE`],
/// never below its lowest fifth (the background in most themes), so a weak
/// station still shows. One key for the reach, the bearings and SNR against
/// distance.
pub fn snr_colour(t: &crate::Theme, snr: f64) -> Color {
    let (lo, hi) = SNR_RANGE;
    let k = ((snr - lo) / (hi - lo)).clamp(0.0, 1.0);
    t.palette_color((0.2 + 0.8 * k) as f32)
}

/// A plot of half-block pixels: each cell two pixels, one above the other
/// (`▀` and `▄`), solid enough to read at any font size. A pixel holds the
/// largest value set on it; an empty half shows the terminal's own
/// background.
pub struct Pixels {
    cols: u16,
    rows: u16,
    v: Vec<Option<f64>>,
}

impl Pixels {
    /// `cols` cells across, `rows` cells down: `rows * 2` pixels down.
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols,
            rows,
            v: vec![None; usize::from(cols) * usize::from(rows) * 2],
        }
    }

    /// Pixels down.
    pub fn height(&self) -> u16 {
        self.rows * 2
    }

    /// Keep the larger of `value` and what pixel (`x`, `y`) holds.
    pub fn set(&mut self, x: u16, y: u16, value: f64) {
        if x < self.cols && y < self.rows * 2 {
            let i = usize::from(y) * usize::from(self.cols) + usize::from(x);
            self.v[i] = Some(self.v[i].map_or(value, |old: f64| old.max(value)));
        }
    }

    /// Draw from (`x0`, `y0`), each pixel coloured by `colour` of its value.
    pub fn draw(
        &self,
        buf: &mut ratatui::buffer::Buffer,
        area: Rect,
        x0: u16,
        y0: u16,
        colour: &dyn Fn(f64) -> Color,
    ) {
        let at = |x: u16, y: u16| self.v[usize::from(y) * usize::from(self.cols) + usize::from(x)];
        for r in 0..self.rows {
            for c in 0..self.cols {
                let (top, bottom) = (at(c, 2 * r), at(c, 2 * r + 1));
                let style = match (top, bottom) {
                    (Some(a), Some(b)) => Some(("▀", Style::default().fg(colour(a)).bg(colour(b)))),
                    (Some(a), None) => Some(("▀", Style::default().fg(colour(a)).bg(Color::Reset))),
                    (None, Some(b)) => Some(("▄", Style::default().fg(colour(b)).bg(Color::Reset))),
                    (None, None) => None,
                };
                if let Some((glyph, style)) = style {
                    put(buf, area, x0 + c, y0 + r, glyph, style);
                }
            }
        }
    }
}

/// A bar hanging down from a rule, `eighths` of a cell long: drawn with the
/// upper blocks there are (an eighth, a half, a whole), the nearest of them.
pub fn hanging(eighths: usize) -> Option<&'static str> {
    match eighths {
        0 => None,
        1..=2 => Some("▔"),
        3..=5 => Some("▀"),
        _ => Some("█"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pixel keeps the largest value set on it, and a cell draws its two
    /// pixels as one half block, an empty half left to the background.
    #[test]
    fn pixels_keep_the_largest_and_draw_half_blocks() {
        let mut px = Pixels::new(3, 1);
        px.set(0, 0, -10.0);
        px.set(0, 0, -20.0);
        px.set(1, 1, 3.0);
        px.set(2, 0, 1.0);
        px.set(2, 1, 2.0);
        px.set(9, 9, 0.0);
        let area = Rect::new(0, 0, 3, 1);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        let seen = std::cell::RefCell::new(Vec::new());
        px.draw(&mut buf, area, 0, 0, &|v| {
            seen.borrow_mut().push(v);
            Color::Indexed(0)
        });
        assert_eq!(buf[(0, 0)].symbol(), "▀");
        assert_eq!(buf[(0, 0)].bg, Color::Reset);
        assert_eq!(buf[(1, 0)].symbol(), "▄");
        assert_eq!(buf[(2, 0)].symbol(), "▀");
        assert!(seen.borrow().contains(&-10.0) && !seen.borrow().contains(&-20.0));
    }

    #[test]
    fn a_hanging_bar_takes_the_nearest_upper_block() {
        assert_eq!(hanging(0), None);
        assert_eq!(hanging(1), Some("▔"));
        assert_eq!(hanging(4), Some("▀"));
        assert_eq!(hanging(8), Some("█"));
    }
}
