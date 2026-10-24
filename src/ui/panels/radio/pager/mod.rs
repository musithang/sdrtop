// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The pager view's three panels, and what they share.
//!
//! - [`face`]: the pager itself, the page on its display.
//! - [`inbox`]: every page received, a row each, the one on the display
//!   marked.
//! - [`card`]: the page on the display, as received: the physics of the
//!   link it came on and what decoding it took (POLICY.md, rule 3).
//!
//! All three read the same page ([`crate::state::PagerState::current`]) and
//! the same view settings, so the face, the list and the card cannot be about
//! different pages, or mask one and not another.

pub mod card;
pub mod face;
pub mod inbox;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
};

use crate::signal::pocsag::demod::Link;

/// Write `text` at (`x`, `y`), clipped to `area`.
pub(super) fn put(buf: &mut Buffer, area: Rect, x: u16, y: u16, text: &str, style: Style) {
    if y < area.top() || y >= area.bottom() {
        return;
    }
    for (x, ch) in (x..).zip(text.chars()) {
        if x >= area.left() && x < area.right() {
            buf.get_mut(x, y).set_char(ch).set_style(style);
        }
    }
}

pub(super) fn fill(buf: &mut Buffer, area: Rect, r: Rect, bg: Color) {
    let r = r.intersection(area);
    for y in r.top()..r.bottom() {
        for x in r.left()..r.right() {
            buf.get_mut(x, y).set_symbol(" ").set_bg(bg);
        }
    }
}

/// A frequency in MHz to the hertz it has, three decimals at least: a
/// paging channel at 439.9875 MHz rounded to 439.988 would name another.
pub(super) fn mhz(hz: u64) -> String {
    let s = format!("{:.6}", hz as f64 / 1e6);
    let trimmed = s.trim_end_matches('0');
    let decimals = trimmed.split('.').nth(1).map_or(0, str::len);
    if decimals < 3 {
        format!("{:.3}", hz as f64 / 1e6)
    } else {
        trimmed.to_string()
    }
}

/// A content's kind, as the display's status row says it.
pub(super) fn kind(c: &crate::signal::pocsag::page::Content) -> &'static str {
    use crate::signal::pocsag::page::Content;
    match c {
        Content::Tone => "TONE",
        Content::Numeric(_) => "NUMERIC",
        Content::Alpha(_) => "ALPHA",
    }
}

/// The link in one line, as the face's strip and the card both say it: the
/// rate and its clock error, the shift, the decision SNR.
pub(super) fn link_line(l: &Link) -> String {
    let mut s = format!(
        "{} baud {:+.0} ppm  \u{b1}{:.2} kHz",
        l.baud,
        l.clock_ppm,
        l.shift_hz / 1e3
    );
    if let Some(snr) = l.decision_snr_db {
        s.push_str(&format!("  SNR {snr:.0} dB"));
    }
    s
}

/// Pages to draw the panels with, for their tests.
#[cfg(test)]
pub(super) mod sample {
    use crate::signal::pocsag::demod::Link;
    use crate::signal::pocsag::page::{Content, Decided, End, Function, Page};
    use crate::state::SdrMetrics;
    use std::time::Instant;

    pub fn link(level: f64) -> Link {
        Link {
            baud: 1200,
            measured_baud: 1200.0,
            clock_ppm: 0.0,
            shift_hz: 4500.0,
            offset_hz: 0.0,
            decision_snr_db: Some(30.0),
            level_dbfs: level,
            preamble_bits: 576,
            sync_errors: 0,
            one_is_higher: false,
            batches: 1,
        }
    }

    pub fn alpha(ric: u32, text: &str) -> Page {
        Page {
            ric,
            function: Function::D,
            content: Content::Alpha(text.bytes().map(Some).collect()),
            decided: Decided::ByFunction,
            end: End::Idle,
            codewords: Vec::new(),
        }
    }

    pub fn with_pages(pages: &[(u32, &str)]) -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.ui.section = crate::signal::pocsag::SECTION.to_string();
        for (ric, text) in pages {
            m.pager.push(
                alpha(*ric, text),
                link(-38.0),
                439_987_500,
                Instant::now(),
                0.0,
            );
        }
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frequency_keeps_its_hertz() {
        assert_eq!(mhz(439_987_500), "439.9875");
        assert_eq!(mhz(100_000_000), "100.000");
        assert_eq!(mhz(145_600_000), "145.600");
        assert_eq!(mhz(466_206_250), "466.20625");
    }
}
