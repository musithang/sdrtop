// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetBtCensusPanel` - classic Bluetooth, who is here.
//!
//! The `net_bt` preset's own panel, and today its only state: B14 landed
//! `signal::bt::access_code`, the specification-precision primitive that
//! would let a live receiver recognise an access code, but not a receiver
//! to feed it a capture. Reusing `net_census`'s own "nothing heard yet"
//! wording here would claim a listening receiver that does not exist -
//! the same "refused, not silent" shape `net_ble_packets` already uses for
//! its own wiring gaps, so a bare panel is never mistaken for a quiet
//! room. See `state::NetState::bt_refused`'s own doc.

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness};

pub struct NetBtCensusPanel;

impl Panel for NetBtCensusPanel {
    fn name(&self) -> &'static str {
        "net_bt_census"
    }

    fn min_size(&self) -> (u16, u16) {
        (32, 5)
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("Classic Bluetooth")
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
        let width = inner.width as usize;

        if let Some(reason) = &state.net.bt_refused {
            let mut lines = vec![Line::from(Span::styled(
                "not decoding".to_string(),
                Style::default().fg(theme.stale),
            ))];
            for chunk in crate::ui::chrome::wrap(reason, width, 4) {
                lines.push(Line::from(Span::styled(
                    chunk,
                    Style::default().fg(theme.label),
                )));
            }
            f.render_widget(Paragraph::new(lines), inner);
            return;
        }

        // No live receiver has ever left this unset - see `bt_refused`'s own
        // doc - so nothing has exercised this arm outside a test. Kept
        // honest anyway rather than unreachable: a future receiver clears
        // the refusal once it exists, and this is the state it lands in
        // before it has heard anything.
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "no devices yet".to_string(),
                Style::default().fg(theme.stale),
            ))),
            inner,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::fixture::draw;

    /// Nothing decoding because no receiver exists yet says so, distinctly
    /// from `net_census`'s own "nothing heard" - the whole reason this
    /// panel exists rather than reusing that one's empty state.
    #[test]
    fn a_refusal_is_shown_rather_than_an_empty_table() {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.bt_refused =
            Some("classic Bluetooth access code correlation has no live receiver yet".to_string());
        let out = draw(NetBtCensusPanel, 60, 10, &m).join("\n");
        assert!(out.contains("not decoding"), "{out}");
        assert!(out.contains("no live"), "{out}");
        assert!(out.contains("receiver yet"), "{out}");
    }

    /// Without a refusal set, the panel still renders rather than panicking
    /// or drawing nothing - the state a future receiver leaves this in
    /// before it has heard anything.
    #[test]
    fn no_refusal_set_shows_the_empty_state_rather_than_panicking() {
        let out = draw(NetBtCensusPanel, 60, 10, &SdrMetrics::fixture().streaming()).join("\n");
        assert!(out.contains("no devices yet"), "{out}");
        assert!(!out.contains("not decoding"), "{out}");
    }

    #[test]
    fn it_fits_every_size_the_layout_can_hand_it() {
        let mut refused = SdrMetrics::fixture().streaming();
        refused.net.bt_refused =
            Some("classic Bluetooth access code correlation has no live receiver yet".to_string());
        for w in 32..90u16 {
            for h in 5..20u16 {
                for m in [refused.clone(), SdrMetrics::fixture()] {
                    for line in draw(NetBtCensusPanel, w, h, &m) {
                        assert!(line.chars().count() <= w as usize, "{w}x{h}: {line:?}");
                    }
                }
            }
        }
    }
}
