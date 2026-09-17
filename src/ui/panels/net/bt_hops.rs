// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetBtHopsPanel` - B15's own exit condition made visible: a
//! time-versus-channel scatter of classic Bluetooth access-code hits,
//! design section 2.3's measurement 11 ("Bluetooth hop timing... makes a
//! piconet's existence obvious without decoding anything").
//!
//! **What is actually on screen is capped, and says so.** `signal::net::
//! worker` watches at most `[net].bt_channels` of however many classic BT
//! channels the current tuning lets it see at all - `signal::bt::receive`'s
//! own doc has the measured reason - so this panel's own "watching N
//! channels" line is not decoration, it is the honest scope of what the dots
//! below it could possibly show.
//!
//! Shares the refused-state shape `net_bt_census` already uses: check
//! `state.net.bt_refused` first, because "no receiver exists" and "a
//! receiver exists and has heard nothing in the last window" are different
//! sentences.

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness};

pub struct NetBtHopsPanel;

/// How much of the past the scatter shows at once. Long enough to see a
/// piconet's own rhythm - 1600 hops a second is a blur inside any one
/// second - short enough that the dots stay legible rather than smearing
/// into a solid band.
const WINDOW_S: f64 = 20.0;

/// Columns reserved on the left for the channel-number scale.
const SCALE_COLS: u16 = 3;
/// Rows reserved under the grid for the time axis caption.
const AXIS_ROWS: u16 = 1;

/// Which row a channel at position `idx` of `n` watched channels (low to
/// high) lands on, across `rows` available - the lowest channel at the
/// bottom, the same convention `net_coexist`'s own frequency scale uses.
///
/// When there are more watched channels than rows, several channels share a
/// row rather than one being silently dropped - an honest loss of
/// resolution, not a loss of a channel's own hits.
fn row_for(idx: usize, n: usize, rows: usize) -> usize {
    if rows == 0 {
        return 0;
    }
    let scaled = idx * rows / n.max(1);
    rows - 1 - scaled.min(rows - 1)
}

impl Panel for NetBtHopsPanel {
    fn name(&self) -> &'static str {
        "net_bt_hops"
    }

    fn min_size(&self) -> (u16, u16) {
        (30, 6)
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("Classic Bluetooth Hops")
            .stale_when(Staleness::NotStreaming)
            .tag_if(true, state.net.mode.tag())
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        if inner.width == 0 || inner.height == 0 {
            return;
        }

        if let Some(reason) = &state.net.bt_refused {
            let mut lines = vec![Line::from(Span::styled(
                "not watching".to_string(),
                Style::default().fg(theme.stale),
            ))];
            for chunk in crate::ui::chrome::wrap(reason, inner.width as usize, 4) {
                lines.push(Line::from(Span::styled(
                    chunk,
                    Style::default().fg(theme.label),
                )));
            }
            f.render_widget(Paragraph::new(lines), inner);
            return;
        }

        let channels = &state.net.bt_channels_watched;
        // "Nothing heard yet" and "too small to draw a grid" both fall back
        // to the same one-line status - the receiver's own state is the
        // thing worth saying either way, not an empty grid.
        if state.net.bt_hops.is_empty()
            || channels.is_empty()
            || inner.width <= SCALE_COLS
            || inner.height <= AXIS_ROWS
        {
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    format!("watching {} channels - no access codes yet", channels.len()),
                    Style::default().fg(theme.stale),
                ))),
                inner,
            );
            return;
        }

        let width = (inner.width - SCALE_COLS) as usize;
        let rows = (inner.height - AXIS_ROWS) as usize;
        let now = std::time::Instant::now();

        let mut grid = vec![vec![false; width]; rows];
        for hop in &state.net.bt_hops {
            let Some(idx) = channels.iter().position(|&c| c == hop.channel) else {
                continue;
            };
            let elapsed = now.saturating_duration_since(hop.seen).as_secs_f64();
            if elapsed > WINDOW_S {
                continue;
            }
            let from_right = ((elapsed / WINDOW_S) * width as f64) as usize;
            if from_right >= width {
                continue;
            }
            let col = width - 1 - from_right;
            let row = row_for(idx, channels.len(), rows);
            grid[row][col] = true;
        }

        let mut lines = Vec::with_capacity(rows + 1);
        for (r, cells) in grid.iter().enumerate() {
            // The lowest channel that lands on this row, if any - the same
            // "one label per row, from the bottom edge" shape
            // `net_coexist::scale_label` uses for its own frequency scale.
            let label = (0..channels.len())
                .find(|&idx| row_for(idx, channels.len(), rows) == r)
                .map(|idx| format!("{:>width$}", channels[idx], width = SCALE_COLS as usize))
                .unwrap_or_else(|| " ".repeat(SCALE_COLS as usize));

            let mut spans = vec![Span::styled(label, Style::default().fg(theme.label))];
            let dots: String = cells
                .iter()
                .map(|&hit| if hit { '\u{25cf}' } else { ' ' })
                .collect();
            spans.push(Span::styled(dots, Style::default().fg(theme.value_hi)));
            lines.push(Line::from(spans));
        }

        lines.push(Line::from(Span::styled(
            format!(
                "{:<width$}-{WINDOW_S:.0} s{:>right$}now",
                "",
                "",
                width = SCALE_COLS as usize,
                right = width.saturating_sub(9)
            ),
            Style::default().fg(theme.label),
        )));

        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{fixture::draw, BtHop};
    use std::time::{Duration, Instant};

    /// "No receiver at all" and "a receiver has heard nothing" are different
    /// claims - the same distinction `net_bt_census`'s own refusal test
    /// makes.
    #[test]
    fn a_refusal_is_shown_rather_than_a_scatter() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.bt_refused =
            Some("no classic Bluetooth channel fits inside the current 2.0 MHz view".to_string());
        let out = draw(NetBtHopsPanel, 60, 10, &m).join("\n");
        assert!(out.contains("not watching"), "{out}");
        assert!(out.contains("2.0"), "{out}");
        assert!(out.contains("MHz"), "{out}");
    }

    /// A live receiver with nothing heard yet says how many channels it is
    /// actually watching, which is the honest scope of what the scatter
    /// could show - not the full 79, and not silently the whole span
    /// either.
    #[test]
    fn no_hits_yet_says_how_many_channels_are_watched() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.bt_channels_watched = vec![10, 20, 30];
        let out = draw(NetBtHopsPanel, 60, 10, &m).join("\n");
        assert!(out.contains("watching 3 channels"), "{out}");
        assert!(!out.contains("not watching"), "{out}");
    }

    /// B15's own exit condition for this panel: a hit on a known channel,
    /// seen just now, lands on that channel's own row, at the right edge
    /// (`now`).
    #[test]
    fn a_recent_hit_lands_on_its_own_channels_row_near_now() {
        let mut m = SdrMetrics::fixture().streaming();
        // Eight channels and eight rows: a 1:1 mapping, so the row a hit on
        // channel 14 lands on is exactly predictable.
        m.net.bt_channels_watched = (10..18).collect();
        m.net.bt_hops.push_back(BtHop {
            channel: 14,
            lap: 0x0011_2233,
            seen: Instant::now(),
        });
        let lines = draw(NetBtHopsPanel, 40, 9, &m); // 8 grid rows + 1 axis row
        let want_row = row_for(
            m.net
                .bt_channels_watched
                .iter()
                .position(|&c| c == 14)
                .unwrap(),
            8,
            8,
        );
        assert!(
            lines[want_row].contains('\u{25cf}'),
            "row {want_row} should carry the hit:\n{}",
            lines.join("\n")
        );
        // The dot sits near the right edge - `now`.
        let dot_at = lines[want_row]
            .chars()
            .position(|c| c == '\u{25cf}')
            .unwrap();
        assert!(
            dot_at as isize >= lines[want_row].chars().count() as isize - 4,
            "expected the dot near the right edge, got column {dot_at} of {:?}",
            lines[want_row]
        );
    }

    /// A hit older than the window is not drawn at all - it happened, but
    /// not inside the picture this panel is currently showing.
    #[test]
    fn a_hit_older_than_the_window_is_not_drawn() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.bt_channels_watched = vec![10];
        m.net.bt_hops.push_back(BtHop {
            channel: 10,
            lap: 0x0011_2233,
            seen: Instant::now() - Duration::from_secs(60),
        });
        let out = draw(NetBtHopsPanel, 40, 6, &m).join("\n");
        assert!(!out.contains('\u{25cf}'), "{out}");
    }

    /// A hit on a channel that is not in the watched list - a stale hop from
    /// before the last retune reshuffled which channels are watched - is not
    /// drawn either, rather than landing on some other channel's row.
    #[test]
    fn a_hit_on_an_unwatched_channel_is_not_drawn() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.bt_channels_watched = vec![10, 20];
        m.net.bt_hops.push_back(BtHop {
            channel: 50,
            lap: 0x0011_2233,
            seen: Instant::now(),
        });
        let out = draw(NetBtHopsPanel, 40, 6, &m).join("\n");
        assert!(!out.contains('\u{25cf}'), "{out}");
    }

    #[test]
    fn it_fits_every_size_the_layout_can_hand_it() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.bt_channels_watched = (10..18).collect();
        for i in 0..8 {
            m.net.bt_hops.push_back(BtHop {
                channel: 10 + i,
                lap: i as u32,
                seen: Instant::now() - Duration::from_secs(i as u64),
            });
        }
        for w in 30..70u16 {
            for h in 6..20u16 {
                for line in draw(NetBtHopsPanel, w, h, &m) {
                    assert!(line.chars().count() <= w as usize, "{w}x{h}: {line:?}");
                }
            }
        }
    }
}
