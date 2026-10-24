// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Every page received, newest first: a row of what it was and how it came,
//! and its text under it.
//!
//! The page on the face's display is the marked row, whichever moved it; the
//! list scrolls to keep it in view. In "my pager" mode only that address's
//! pages are listed, and masked, the text row gives the kind and length.
//!
//! A record of the session, dated row by row, so it does not go stale; the
//! face and the card say whether the radio is listening now.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::mhz;
use crate::signal::pocsag::page::Content;
use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness, Tag};

pub struct PagerInboxPanel;

/// Below this body width the cards give their column up, and the face takes
/// the whole width: the face's own strip still says the link.
pub const CARDS_MIN_BODY: u16 = 150;

/// The cards' column.
const CARDS_W: u16 = 66;

impl Panel for PagerInboxPanel {
    fn name(&self) -> &'static str {
        "pager_inbox"
    }

    fn reads_spectrum(&self) -> bool {
        false
    }

    fn min_size(&self) -> (u16, u16) {
        (40, 6)
    }

    fn column_width(&self, body: u16) -> Option<u16> {
        Some(if body < CARDS_MIN_BODY { 0 } else { CARDS_W })
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        let p = &state.pager;
        PanelChrome::new("Inbox")
            .stale_when(Staleness::Never)
            .tag_if(p.view.masked, Tag::Masked)
            .tag_if(p.view.mine.is_some(), Tag::Filtered)
            .suffix(format!(" {} received", p.received))
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        let p = &state.pager;
        let w = inner.width as usize;
        let quiet = Style::default().fg(theme.label);
        let shown = p.shown();
        if shown.is_empty() {
            let line = if state.radio.hw_streaming {
                format!(
                    " No pages yet. Listening on {} MHz.",
                    mhz(state.radio.frequency)
                )
            } else {
                " No pages yet. RX is stopped.".to_string()
            };
            f.render_widget(Paragraph::new(Span::styled(line, quiet)), inner);
            return;
        }
        let current = p.current().map(|e| e.n);
        let head = Line::from(Span::styled(
            "    #  TIME      RIC      F KIND     LEN  FIX  LEVEL",
            Style::default()
                .fg(theme.border_dim)
                .add_modifier(Modifier::BOLD),
        ));
        let mut rows: Vec<Line> = Vec::new();
        let mut at = 0;
        for e in shown.iter().rev() {
            let on = Some(e.n) == current;
            if on {
                at = rows.len();
            }
            let unread = !p.view.read.contains(&e.n);
            let clock = crate::ui::panels::core::log::fmt_clock(e.unix_s as u64);
            let (kind, len) = match &e.page.content {
                Content::Tone => ("tone", "-".to_string()),
                Content::Numeric(c) => ("numeric", c.len().to_string()),
                Content::Alpha(c) => ("alpha", c.len().to_string()),
            };
            let fix = e.page.repairs();
            let ink = if on { theme.value_hi } else { theme.value };
            let mut style = Style::default().fg(ink);
            if on {
                style = style.add_modifier(Modifier::BOLD);
            }
            rows.push(Line::from(vec![
                Span::styled(
                    if on { "\u{258c}" } else { " " },
                    Style::default().fg(theme.border_accent),
                ),
                Span::styled(
                    if unread { "\u{25cf}" } else { " " },
                    Style::default().fg(theme.status_ok),
                ),
                Span::styled(
                    format!(
                        "{:>3}  {clock}  {:07}  {} {kind:<8} {len:>3}  {fix:>3}  {:.0} dBFS",
                        e.n,
                        e.page.ric,
                        e.page.function.letter(),
                        e.link.level_dbfs
                    ),
                    style,
                ),
            ]));
            let text = match (&e.page.content, p.view.masked) {
                (Content::Tone, _) => None,
                (Content::Numeric(_), true) => Some(format!("{kind}, {len} digits")),
                (_, true) => Some(format!("{kind}, {len} characters")),
                (_, false) => Some(e.page.text()),
            };
            if let Some(text) = text {
                let text: String = text.chars().take(w.saturating_sub(8)).collect();
                rows.push(Line::from(Span::styled(
                    format!("        {text}"),
                    Style::default().fg(if p.view.masked {
                        theme.border_dim
                    } else {
                        theme.label
                    }),
                )));
            }
        }
        // Keep the page on the display in view: the list scrolls under its
        // heading.
        let body = inner.height.saturating_sub(1) as usize;
        let offset = (at + 2).saturating_sub(body);
        let mut lines = vec![head];
        lines.extend(rows.into_iter().skip(offset));
        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::super::sample::with_pages;
    use super::*;
    use crate::state::fixture::draw;

    /// Newest first, the page on the display marked, its text under it.
    #[test]
    fn newest_first_with_the_shown_page_marked() {
        let mut m = with_pages(&[(8, "FIRST"), (16, "SECOND"), (24, "THIRD")]);
        m.pager.step(false);
        let rows = draw(PagerInboxPanel, 70, 12, &m);
        let all = rows.join("\n");
        let third = all.find("THIRD").unwrap();
        let first = all.find("FIRST").unwrap();
        assert!(third < first, "{all}");
        let marked = rows.iter().find(|r| r.contains('\u{258c}')).unwrap();
        assert!(marked.contains("0000016"), "{marked}");
        assert!(all.contains("3 received"), "{all}");
    }

    /// Masked, the text row gives the kind and length and nothing else.
    #[test]
    fn masked_text_is_its_kind_and_length() {
        let mut m = with_pages(&[(8, "SECRET WORDS")]);
        m.pager.view.masked = true;
        let all = draw(PagerInboxPanel, 70, 8, &m).join("\n");
        assert!(!all.contains("SECRET"), "{all}");
        assert!(
            all.contains("alpha, 12 characters") && all.contains("[MASKED]"),
            "{all}"
        );
    }

    /// Empty, it says where it is listening.
    #[test]
    fn empty_it_says_where_it_listens() {
        let mut m = with_pages(&[]);
        m.radio.frequency = 439_987_500;
        let all = draw(PagerInboxPanel, 70, 6, &m).join("\n");
        assert!(all.contains("Listening on 439.9875 MHz"), "{all}");
    }

    /// The marked page stays in view however long the list.
    #[test]
    fn the_shown_page_stays_in_view() {
        let pages: Vec<(u32, String)> = (0..30).map(|i| (8, format!("PAGE {i}"))).collect();
        let refs: Vec<(u32, &str)> = pages.iter().map(|(r, t)| (*r, t.as_str())).collect();
        let mut m = with_pages(&refs);
        for _ in 0..25 {
            m.pager.step(false);
        }
        let rows = draw(PagerInboxPanel, 70, 10, &m);
        assert!(rows.iter().any(|r| r.contains('\u{258c}')), "{rows:#?}");
    }

    #[test]
    fn it_gives_way_on_a_narrow_screen() {
        assert_eq!(PagerInboxPanel.column_width(149), Some(0));
        assert_eq!(PagerInboxPanel.column_width(189), Some(CARDS_W));
    }
}
