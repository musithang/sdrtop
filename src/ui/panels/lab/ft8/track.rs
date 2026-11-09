// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The frequency track: tone 0 through the transmission against the line
//! the decode fitted (its frequency and its drift), group by group.
//!
//! Each point is the trace's ([`crate::signal::ft8::trace::TrackPoint`]),
//! drawn with its own spread as a bar. A point whose spread is a tenth of the
//! tone spacing or more is too weak to say where the tone was, and shows as a
//! dot on the line's row, never as a place. The plot is +-2 Hz; a point past
//! that is drawn at the edge as an arrow, so a jump stays visible without
//! being drawn where it was not.

use ratatui::{layout::Rect, style::Style, Frame};

use super::{say, shown_or_say, NARROW_BODY};
use crate::signal::ft8::modulate::{SYMBOLS, SYMBOL_SECONDS, TONE_SPACING};
use crate::state::ft8::DecodeEntry;
use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness};
use crate::ui::panels::radio::ft8::{put, signed};

pub struct Ft8TrackPanel;

/// The plot's half height, Hz.
pub const SPAN_HZ: f64 = 2.0;

/// A point is placed only with a spread under this: a tenth of the tone
/// spacing.
pub const TRACKED_SIGMA_HZ: f64 = TONE_SPACING / 10.0;

/// Tone 0 on the fitted line `at` seconds into the transmission.
pub fn on_the_line(f0: f64, drift: f64, at: f64) -> f64 {
    let length = SYMBOLS as f64 * SYMBOL_SECONDS;
    f0 + drift * (at - length / 2.0) / length
}

impl Panel for Ft8TrackPanel {
    fn name(&self) -> &'static str {
        "ft8_track"
    }

    fn reads_spectrum(&self) -> bool {
        false
    }

    fn min_size(&self) -> (u16, u16) {
        (30, 7)
    }

    fn column_width(&self, body: u16) -> Option<u16> {
        (body < NARROW_BODY).then_some(0)
    }

    fn chrome(&self, _state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("Frequency track").stale_when(Staleness::NotStreaming)
    }

    fn render(
        &self,
        f: &mut Frame,
        area: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        let buf = f.buffer_mut();
        let Some(d) = shown_or_say(buf, area, state, theme) else {
            return;
        };
        if area.height < 4 || area.width < 20 {
            say(buf, area, "too small for the track", theme);
            return;
        }
        draw(buf, area, &d, theme);
    }
}

fn draw(buf: &mut ratatui::buffer::Buffer, area: Rect, d: &DecodeEntry, t: &crate::Theme) {
    let tr = &d.decode.trace;
    let p = &d.decode.physics;
    let label = Style::default().fg(t.label);
    let rows = area.height - 1;
    let x0 = area.x + 8;
    let plot_w = area.width.saturating_sub(9).max(2);
    let row_of = |hz: f64| -> u16 {
        let k = (SPAN_HZ - hz.clamp(-SPAN_HZ, SPAN_HZ)) / (2.0 * SPAN_HZ);
        (k * f64::from(rows - 1)).round() as u16
    };
    for (hz, text) in [(SPAN_HZ, "+2 Hz"), (0.0, " line"), (-SPAN_HZ, "-2 Hz")] {
        put(buf, area, area.x + 1, area.y + row_of(hz), text, label);
    }
    let zero = area.y + row_of(0.0);
    for c in 0..plot_w {
        put(
            buf,
            area,
            x0 + c,
            zero,
            "┄",
            Style::default().fg(t.border_dim),
        );
    }
    let length = SYMBOLS as f64 * SYMBOL_SECONDS;
    for pt in &tr.f0_track {
        let col = x0 + ((pt.at / length) * f64::from(plot_w - 1)).round() as u16;
        let dev = pt.hz - on_the_line(p.f0, p.drift, pt.at);
        if pt.sigma >= TRACKED_SIGMA_HZ {
            put(buf, area, col, zero, "·", label);
            continue;
        }
        let (top, bottom) = (row_of(dev + pt.sigma), row_of(dev - pt.sigma));
        for r in top..=bottom {
            put(
                buf,
                area,
                col,
                area.y + r,
                "│",
                Style::default().fg(t.value),
            );
        }
        let mark = if dev > SPAN_HZ {
            "▲"
        } else if dev < -SPAN_HZ {
            "▼"
        } else {
            "●"
        };
        put(
            buf,
            area,
            col,
            area.y + row_of(dev),
            mark,
            Style::default().fg(t.value_hi),
        );
    }
    let foot = area.y + rows;
    let x = put(buf, area, x0, foot, "line ", label);
    let x = put(
        buf,
        area,
        x,
        foot,
        &format!("{:.2} Hz", p.f0),
        Style::default().fg(t.value),
    );
    let x = put(buf, area, x, foot, "  drift ", label);
    put(
        buf,
        area,
        x,
        foot,
        &format!("{} Hz", signed(p.drift, 2)),
        Style::default().fg(t.value),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::fixture::draw as render;
    use std::sync::Arc;

    #[test]
    fn every_tracked_point_is_drawn_and_the_weak_ones_are_dots() {
        let m = SdrMetrics::fixture().streaming().ft8_traced();
        for d in &m.ft8.decodes {
            let mut m = m.clone();
            m.ft8.view.selected = d.decode.message.sender().map(str::to_string);
            let crate::state::ft8::Shown::Decode(s) = m.ft8.shown() else {
                panic!()
            };
            let tracked = s
                .decode
                .trace
                .f0_track
                .iter()
                .filter(|p| p.sigma < TRACKED_SIGMA_HZ)
                .count();
            let lines = render(Ft8TrackPanel, 63, 18, &m).join("\n");
            let placed = lines.matches('●').count()
                + lines.matches('▲').count()
                + lines.matches('▼').count();
            assert_eq!(placed, tracked, "{lines}");
            assert!(
                lines.contains(&format!("line {:.2} Hz", s.decode.physics.f0)),
                "{lines}"
            );
        }
    }

    /// A point 3 Hz off its line is drawn at the edge as an arrow, not
    /// placed where it was not; one with a spread of a tenth of the tone
    /// spacing is only a dot.
    #[test]
    fn a_point_off_the_plot_is_an_arrow_and_a_weak_one_a_dot() {
        let mut m = SdrMetrics::fixture().streaming().ft8_traced();
        let entry = m.ft8.decodes.back().unwrap().clone();
        let mut d = (*entry).clone();
        let mut trace = (*d.decode.trace).clone();
        let p = d.decode.physics;
        for (i, pt) in trace.f0_track.iter_mut().enumerate() {
            pt.sigma = if i == 1 { TRACKED_SIGMA_HZ } else { 0.05 };
            pt.hz = on_the_line(p.f0, p.drift, pt.at) + if i == 0 { 3.0 } else { 0.0 };
        }
        d.decode.trace = Arc::new(trace);
        *m.ft8.decodes.back_mut().unwrap() = Arc::new(d);
        m.ft8.view.selected = None;
        let lines = render(Ft8TrackPanel, 63, 18, &m).join("\n");
        assert_eq!(lines.matches('▲').count(), 1, "{lines}");
        assert_eq!(lines.matches('·').count(), 1, "{lines}");
        assert_eq!(lines.matches('●').count(), 8, "{lines}");
    }

    #[test]
    fn it_gives_way_on_a_narrow_body() {
        assert_eq!(Ft8TrackPanel.column_width(149), Some(0));
        assert_eq!(Ft8TrackPanel.column_width(189), None);
    }
}
