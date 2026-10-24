// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The page on the display, as it was received: what the link was, and what
//! reading it took (POLICY.md, rule 3: a payload is not testimony).
//!
//! Each row is a reading and what it is against: the rate measured against
//! the rate locked to and this radio's clock, the shift against the
//! standard's ±4.5 kHz, the level with how far the bits stood out from their
//! own spread, the preamble and how clean the sync word was, the codewords
//! and the bits repaired, the polarity as the frequency that carried a 1,
//! the address with its function and frame, the content with what decided
//! its kind (and whether the content overruled the function), how the page
//! ended, when and where it was received. Under them one codeword's 32 bits,
//! the repaired ones marked.
//!
//! The carrier offset is against this radio's tuning, and so carries the
//! engine's word on what such an offset is worth ([`PanelChrome::shows_offsets`]).

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::{link_line, mhz};
use crate::signal::pocsag::page::{Content, Decided, End};
use crate::state::SdrMetrics;
use crate::ui::panel::{Panel, PanelChrome, Staleness, Tag};

pub struct PagerCardPanel;

/// The rows the card takes in its column, when the column has them.
const ROWS: u16 = 19;

/// The standard's shift, for the shift row to be read against.
const STANDARD_SHIFT_KHZ: f64 = 4.5;

impl Panel for PagerCardPanel {
    fn name(&self) -> &'static str {
        "pager_card"
    }

    fn reads_spectrum(&self) -> bool {
        false
    }

    fn min_size(&self) -> (u16, u16) {
        (40, 8)
    }

    fn column_height(&self, column: u16) -> Option<u16> {
        Some(ROWS.min(column.saturating_sub(6)))
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        PanelChrome::new("This page")
            .stale_when(Staleness::Never)
            .shows_offsets()
            .tag_if(state.pager.view.masked, Tag::Masked)
    }

    fn render(
        &self,
        f: &mut Frame,
        inner: Rect,
        state: &SdrMetrics,
        theme: &crate::Theme,
        _focused: bool,
    ) {
        let label = Style::default().fg(theme.label);
        let value = Style::default().fg(theme.value);
        let lead = Style::default()
            .fg(theme.value_hi)
            .add_modifier(Modifier::BOLD);
        let Some(e) = state.pager.current() else {
            f.render_widget(
                Paragraph::new(Span::styled(" No page on the display yet.", label)),
                inner,
            );
            return;
        };
        let l = &e.link;
        let page = &e.page;
        let w = inner.width as usize;
        let row = |name: &str, val: String, why: String, strong: bool| -> Line<'static> {
            let why: String = why.chars().take(w.saturating_sub(26)).collect();
            Line::from(vec![
                Span::styled(format!(" {name:<10}"), label),
                Span::styled(format!("{val:<13}"), if strong { lead } else { value }),
                Span::styled(why, label),
            ])
        };
        let (kind, len) = match &page.content {
            Content::Tone => ("tone", 0),
            Content::Numeric(c) => ("numeric", c.len()),
            Content::Alpha(c) => ("alpha", c.len()),
        };
        let decided = match page.decided {
            Decided::ByFunction => format!("as function {} says", page.function.letter()),
            Decided::OverFunction => format!(
                "function {} says {}; reads as {}",
                page.function.letter(),
                if page.function.letter() == 'A' {
                    "digits"
                } else {
                    "text"
                },
                kind
            ),
            Decided::ByContent => format!("function {} names no format", page.function.letter()),
            Decided::Ambiguous => "either format fits; the likelier".to_string(),
            Decided::NoMessage => "an address alone".to_string(),
        };
        let ended = match page.end {
            End::Idle => "at the idle codeword",
            End::Address => "at the next address",
            End::Undecipherable => "at two unreadable codewords",
            End::Cut => "cut: the stream broke or retuned",
        };
        let clock = crate::ui::panels::core::log::fmt_clock(e.unix_s as u64);
        let mut lines = vec![
            row(
                "Rate",
                format!("{} baud", l.baud),
                format!("{:+.0} ppm, by this radio's clock", l.clock_ppm),
                true,
            ),
            row(
                "Shift",
                format!("\u{b1}{:.2} kHz", l.shift_hz / 1e3),
                format!("the standard is \u{b1}{STANDARD_SHIFT_KHZ}"),
                true,
            ),
            row(
                "Offset",
                format!("{:+.0} Hz", l.offset_hz),
                "the carrier from the tuned frequency".to_string(),
                false,
            ),
            row(
                "Level",
                format!("{:.0} dBFS", l.level_dbfs),
                l.decision_snr_db
                    .map_or("too few bits for their spread".to_string(), |s| {
                        format!("decision SNR {s:.0} dB")
                    }),
                true,
            ),
            row(
                "Preamble",
                format!("{} bits", l.preamble_bits),
                format!("sync word with {} wrong bits", l.sync_errors),
                false,
            ),
            row(
                "Codewords",
                format!("{}", page.codewords.len()),
                format!(
                    "{} bits repaired, {} refused",
                    page.repairs(),
                    page.refused()
                ),
                false,
            ),
            row(
                "Polarity",
                (if l.one_is_higher {
                    "1 is high"
                } else {
                    "1 is low"
                })
                .to_string(),
                "the frequency that carried a 1".to_string(),
                false,
            ),
            row(
                "Address",
                format!("{:07}", page.ric),
                format!(
                    "function {}, frame {}",
                    page.function.letter(),
                    page.ric & 7
                ),
                false,
            ),
            row(
                "Content",
                if len > 0 {
                    format!("{kind}, {len}")
                } else {
                    kind.to_string()
                },
                decided,
                false,
            ),
            row("Ended", String::new(), ended.to_string(), false),
            row(
                "Received",
                clock,
                format!(
                    "at {} MHz, {} batch{}",
                    mhz(e.centre_hz),
                    l.batches,
                    if l.batches == 1 { "" } else { "es" }
                ),
                false,
            ),
            Line::from(Span::styled(format!(" {}", link_line(l)), value)),
        ];

        // One codeword's bits, the repaired ones marked: the one that took
        // the most repairing, else the last.
        if let Some(cw) = page
            .codewords
            .iter()
            .filter(|c| c.checked.is_some())
            .max_by_key(|c| c.checked.map_or(0, |k| k.repairs()))
        {
            let flipped = cw.checked.map_or(0, |k| k.flipped);
            let word = cw.checked.map_or(cw.raw, |k| k.word);
            let mut spans = vec![Span::styled(" ", label)];
            for b in (0..32).rev() {
                let bit = if word >> b & 1 == 1 { "1" } else { "0" };
                let style = if flipped >> b & 1 == 1 {
                    Style::default()
                        .fg(theme.status_warn)
                        .add_modifier(Modifier::BOLD)
                } else if bit == "1" {
                    value
                } else {
                    label
                };
                spans.push(Span::styled(bit, style));
            }
            if flipped != 0 {
                spans.push(Span::styled(
                    " repaired bits lit",
                    Style::default().fg(theme.status_warn),
                ));
            }
            lines.push(Line::raw(""));
            lines.push(Line::from(Span::styled(" A codeword, as corrected", label)));
            lines.push(Line::from(spans));
        }
        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::super::sample::with_pages;
    use super::*;
    use crate::state::fixture::draw;

    /// Every reading, each against what it is measured by.
    #[test]
    fn the_card_says_the_link_and_the_reading() {
        let m = with_pages(&[(200_001, "WX BUDAPEST")]);
        let all = draw(PagerCardPanel, 70, 22, &m).join("\n");
        for want in [
            "Rate",
            "1200 baud",
            "by this radio's clock",
            "Shift",
            "\u{b1}4.50 kHz",
            "the standard is \u{b1}4.5",
            "Offset",
            "Level",
            "-38 dBFS",
            "decision SNR 30 dB",
            "Preamble",
            "576 bits",
            "Codewords",
            "Polarity",
            "1 is low",
            "Address",
            "0200001",
            "function D, frame 1",
            "alpha, 11",
            "as function D says",
            "Ended",
            "at the idle codeword",
            "Received",
            "439.9875 MHz",
        ] {
            assert!(all.contains(want), "'{want}' missing:\n{all}");
        }
    }

    /// Text under function A: the card says the content overruled it.
    #[test]
    fn it_says_when_the_content_overruled_the_function() {
        let mut m = with_pages(&[(0, "sdrtop is awesome")]);
        let e = m.pager.inbox.pop_back().unwrap();
        let mut entry = (*e).clone();
        entry.page.function = crate::signal::pocsag::page::Function::A;
        entry.page.decided = Decided::OverFunction;
        m.pager.push(
            entry.page,
            entry.link,
            entry.centre_hz,
            entry.at,
            entry.unix_s,
        );
        let all = draw(PagerCardPanel, 80, 22, &m).join("\n");
        assert!(
            all.contains("function A says digits; reads as alpha"),
            "{all}"
        );
    }

    /// At the width the card is given beside the face, every explanation
    /// is whole: a cut-off line reads as a value.
    #[test]
    fn every_line_is_whole_at_its_width() {
        let mut m = with_pages(&[(200_001, "WX")]);
        let e = m.pager.inbox.pop_back().unwrap();
        let mut entry = (*e).clone();
        entry.page.end = End::Cut;
        entry.page.function = crate::signal::pocsag::page::Function::A;
        entry.page.decided = Decided::OverFunction;
        entry.link.decision_snr_db = None;
        entry.link.clock_ppm = -330.0;
        m.pager.push(
            entry.page,
            entry.link,
            entry.centre_hz,
            entry.at,
            entry.unix_s,
        );
        let all = draw(PagerCardPanel, 66, 22, &m).join("\n");
        for want in [
            "-330 ppm, by this radio's clock",
            "the carrier from the tuned frequency",
            "too few bits for their spread",
            "the frequency that carried a 1",
            "function A says digits; reads as alpha",
            "cut: the stream broke or retuned",
            "at 439.9875 MHz, 1 batch",
        ] {
            assert!(all.contains(want), "'{want}' cut or missing:\n{all}");
        }
    }

    #[test]
    fn with_no_page_it_says_so() {
        let m = with_pages(&[]);
        let all = draw(PagerCardPanel, 70, 10, &m).join("\n");
        assert!(all.contains("No page on the display yet."), "{all}");
    }
}
