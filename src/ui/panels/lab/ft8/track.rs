// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The frequency track: tone 0 through the transmission against the line
//! the decode fitted (its frequency and its drift), group by group.
//!
//! Hz from the decode's tone 0 up the side, the transmission's 12.64 s
//! across. The fitted line is drawn as it runs, so its drift shows as a
//! slope; each point is the trace's ([`crate::signal::ft8::trace::TrackPoint`]),
//! tone 0 over eight symbols, drawn with its own spread as a bar. A point
//! whose spread is a tenth of the tone spacing or more is too weak to say
//! where the tone was: it is not drawn, and the figures say how many there
//! were. The plot is +-2 Hz; a point past that is drawn at the edge as an
//! arrow, so a jump stays visible without being drawn where it was not.

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
    let value = Style::default().fg(t.value);
    // Plot rows; then the time axis, the figures and, with room, the key.
    let keyed = area.height >= 9;
    let rows = area.height - if keyed { 3 } else { 2 };
    let x0 = area.x + 8;
    let plot_w = area.width.saturating_sub(9).max(2);
    let length = SYMBOLS as f64 * SYMBOL_SECONDS;
    let row_of = |hz: f64| -> u16 {
        let k = (SPAN_HZ - hz.clamp(-SPAN_HZ, SPAN_HZ)) / (2.0 * SPAN_HZ);
        (k * f64::from(rows - 1)).round() as u16
    };
    let col_of = |at: f64| -> u16 { x0 + ((at / length) * f64::from(plot_w - 1)).round() as u16 };
    // The scale: Hz from the decode's tone 0.
    let mut ticks = vec![(SPAN_HZ, "+2 Hz"), (0.0, "   f0"), (-SPAN_HZ, "-2 Hz")];
    if rows >= 7 {
        ticks.extend([(1.0, "   +1"), (-1.0, "   -1")]);
    }
    for (hz, text) in ticks {
        put(buf, area, area.x + 1, area.y + row_of(hz), text, label);
    }
    // The fitted line, its drift as a slope: tone 0 at every column.
    for c in 0..plot_w {
        let at = f64::from(c) / f64::from(plot_w - 1) * length;
        let hz = on_the_line(p.f0, p.drift, at) - p.f0;
        put(
            buf,
            area,
            x0 + c,
            area.y + row_of(hz),
            "┈",
            Style::default().fg(t.border_default),
        );
    }
    // The points: each where the samples put tone 0, with its spread.
    let mut weak = 0;
    for pt in &tr.f0_track {
        if pt.sigma >= TRACKED_SIGMA_HZ {
            weak += 1;
            continue;
        }
        let col = col_of(pt.at);
        let hz = pt.hz - p.f0;
        let (top, bottom) = (row_of(hz + pt.sigma), row_of(hz - pt.sigma));
        for r in top..=bottom {
            put(buf, area, col, area.y + r, "│", value);
        }
        let mark = if hz > SPAN_HZ {
            "▲"
        } else if hz < -SPAN_HZ {
            "▼"
        } else {
            "●"
        };
        put(
            buf,
            area,
            col,
            area.y + row_of(hz),
            mark,
            Style::default().fg(t.value_hi),
        );
    }
    // Time across.
    let axis = area.y + rows;
    put(buf, area, x0, axis, "0 s", label);
    let mid = col_of(length / 2.0);
    put(buf, area, mid.saturating_sub(1), axis, "6.3", label);
    put(
        buf,
        area,
        (x0 + plot_w).saturating_sub(6),
        axis,
        "12.6 s",
        label,
    );
    // The figures.
    let fig = axis + 1;
    let x = put(buf, area, x0, fig, "f0 ", label);
    let x = put(buf, area, x, fig, &format!("{:.2} Hz", p.f0), value);
    let x = put(buf, area, x, fig, "  drift ", label);
    let x = put(
        buf,
        area,
        x,
        fig,
        &format!("{} Hz", signed(p.drift, 2)),
        value,
    );
    if weak > 0 {
        put(
            buf,
            area,
            x,
            fig,
            &format!("  {weak} too weak to place"),
            label,
        );
    }
    if keyed {
        let key = fig + 1;
        let x = put(buf, area, x0, key, "●", Style::default().fg(t.value_hi));
        let x = put(buf, area, x + 1, key, "8 symbols  ", label);
        let x = put(buf, area, x, key, "│", value);
        let x = put(buf, area, x + 1, key, "1 sigma  ", label);
        let x = put(
            buf,
            area,
            x,
            key,
            "┈┈",
            Style::default().fg(t.border_default),
        );
        put(buf, area, x + 1, key, "fitted f0 and drift", label);
    }
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
            // Less the one in the key.
            let placed = lines.matches('●').count() - 1
                + lines.matches('▲').count()
                + lines.matches('▼').count();
            assert_eq!(placed, tracked, "{lines}");
            assert!(
                lines.contains(&format!("f0 {:.2} Hz", s.decode.physics.f0)),
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
        assert!(lines.contains("1 too weak to place"), "{lines}");
        // Eight placed, and one in the key.
        assert_eq!(lines.matches('●').count(), 8 + 1, "{lines}");
    }

    #[test]
    fn it_gives_way_on_a_narrow_body() {
        assert_eq!(Ft8TrackPanel.column_width(149), Some(0));
        assert_eq!(Ft8TrackPanel.column_width(189), None);
    }
}
