// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetBtCensusPanel` - classic Bluetooth, who is here.
//!
//! **Still not this panel's answer, even after B15.** B15 gave classic
//! Bluetooth a live receiver - `net_bt_hops` plots what it hears - but
//! deliberately did not fold a hit's LAP into a census entry: a LAP
//! identifies a *piconet's master*, not a device the way a BD_ADDR does,
//! and design section 2.5's own census (measurement 17) is keyed by
//! address. Reusing `net_census`'s own "nothing heard yet" wording here
//! would still claim a kind of listening this panel does not do - the same
//! "refused, not silent" shape `net_ble_packets` uses for its own wiring
//! gaps. See `state::NetState::bt_refused`'s own doc for the receiver's
//! current, narrower meaning.

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

        // B15's own receiver clears `bt_refused` once it is running - see
        // that field's own doc - but does not feed this panel: hits go to
        // `net_bt_hops` instead. "no devices yet" would claim this panel is
        // listening for one; it is not, yet.
        let mut lines = vec![Line::from(Span::styled(
            "not decoding".to_string(),
            Style::default().fg(theme.stale),
        ))];
        for chunk in crate::ui::chrome::wrap(
            "classic Bluetooth hits are not yet folded into a census by LAP - see the hop scatter (net_bt_hops)",
            width,
            4,
        ) {
            lines.push(Line::from(Span::styled(
                chunk,
                Style::default().fg(theme.label),
            )));
        }
        f.render_widget(Paragraph::new(lines), inner);
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
            Some("no classic Bluetooth channel fits inside the current 2.0 MHz view".to_string());
        let out = draw(NetBtCensusPanel, 60, 10, &m).join("\n");
        assert!(out.contains("not decoding"), "{out}");
        assert!(out.contains("2.0"), "{out}");
        assert!(out.contains("MHz"), "{out}");
    }

    /// Without a refusal set - B15's receiver is running - this panel still
    /// says it is not decoding, because it genuinely is not: hits go to
    /// `net_bt_hops`, not here. Distinct from `bt_refused`'s own message,
    /// which would wrongly claim there is no receiver at all.
    #[test]
    fn no_refusal_set_still_says_not_decoding_but_for_a_different_reason() {
        let out = draw(NetBtCensusPanel, 60, 10, &SdrMetrics::fixture().streaming()).join("\n");
        assert!(out.contains("not decoding"), "{out}");
        assert!(out.contains("net_bt_hops"), "{out}");
    }

    #[test]
    fn it_fits_every_size_the_layout_can_hand_it() {
        let mut refused = SdrMetrics::fixture().streaming();
        refused.net.bt_refused =
            Some("no classic Bluetooth channel fits inside the current 2.0 MHz view".to_string());
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
