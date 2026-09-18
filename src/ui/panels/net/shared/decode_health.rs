// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! `NetDecodeHealthPanel` - what the receiver missed.
//!
//! Design section 13.2: **this is not a debug panel, it is testimony.** Without
//! it, every count in the section is a lower bound presented as a total. A frame
//! at 6 Mbps carrying 1500 bytes is two milliseconds, which at 20 Msps spans
//! several driver blocks, and there are three separate ways for one of those
//! blocks to go missing before anything has had a chance to look at it: the
//! driver loses samples before the block is stamped, the bounded feed refuses a
//! whole block under load, and a run broken in the middle takes with it whatever
//! was being assembled. None of the three is visible from downstream.
//!
//! **Nothing here is decoded yet, and the panel says so rather than printing
//! zeroes.** A row reading `bursts 0` is a claim that we looked and found none.
//! At this point nothing looks. The counters that exist are the ones something
//! actually maintains, and the decode section is one line saying what is not
//! running - which is rule 2 in the small: what cannot be asked is refused,
//! never invented.

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::{NetDecodeHealth, SdrMetrics};
use crate::ui::panel::{Panel, PanelChrome, Staleness};

pub struct NetDecodeHealthPanel;

/// Label and value column widths, so the counts line up under each other.
const LABEL: usize = 15;
const VALUE: usize = 10;

/// One row: a label, a right-aligned count, and a note that earns its place or
/// is not drawn.
///
/// **The note is dropped rather than truncated.** These notes are the
/// qualifiers - "a floor", "no detector is running yet" - and half a qualifier
/// is worse than none: `blocks lost   7   a flo` reads as a number with
/// something unexplained attached, which is the impression the note exists to
/// prevent. The panel's minimum width does not fit all of them.
fn count<'a>(
    label: &str,
    value: String,
    ink: ratatui::style::Color,
    note: Option<&str>,
    theme: &crate::Theme,
    width: usize,
) -> Line<'a> {
    let mut spans = vec![
        Span::styled(
            format!("{:<LABEL$}", label),
            Style::default().fg(theme.label),
        ),
        Span::styled(format!("{:>VALUE$}", value), Style::default().fg(ink)),
    ];
    if let Some(note) = note.filter(|n| LABEL + VALUE + 3 + n.chars().count() <= width) {
        spans.push(Span::styled(
            format!("   {note}"),
            Style::default().fg(theme.label),
        ));
    }
    Line::from(spans)
}

/// A heading, in the same ink the other section headings in this deck use.
fn heading<'a>(text: &str, theme: &crate::Theme) -> Line<'a> {
    Line::from(Span::styled(
        text.to_string(),
        Style::default().fg(theme.label),
    ))
}

/// `1 234 567` - grouped, because these run to seven digits inside a minute and
/// an ungrouped one cannot be read at a glance, which is the only way anybody
/// reads this panel.
fn grouped(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// `1.63 Gsamp` - the pair count in units a person can hold.
///
/// The only figure here not shown in full, and the reason is that it is the only
/// one where the full figure says nothing: a receiver running for an hour at
/// 20 Msps has seen seventy-two billion pairs, and no digit past the third is a
/// fact anybody wants. Grouped, it would also be the one column wide enough to
/// set the width of every other.
fn samples(pairs: u64) -> String {
    const UNITS: [(u64, &str); 3] = [(1_000_000_000, "G"), (1_000_000, "M"), (1_000, "k")];
    for (scale, suffix) in UNITS {
        if pairs >= scale {
            return format!("{:.2} {suffix}samp", pairs as f64 / scale as f64);
        }
    }
    format!("{pairs} samp")
}

/// The whole panel body, as a function of the counts and the width alone.
///
/// Split out because nothing on this panel is a function of anything else: no
/// device, no clock, no lock. The same split `signal::fft` makes, for the same
/// reason.
fn lines(h: &NetDecodeHealth, theme: &crate::Theme, width: usize) -> Vec<Line<'static>> {
    let row = |label, value, note| count(label, value, theme.value, note, theme, width);
    vec![
        heading("WHAT ARRIVED", theme),
        row("blocks", grouped(h.blocks_in), None),
        row("I/Q pairs", samples(h.pairs_in), None),
        row(
            "current run",
            grouped(h.run_blocks),
            Some("since the last break"),
        ),
        Line::from(""),
        heading("WHAT DID NOT", theme),
        row("interruptions", grouped(h.gaps), Some("runs broken")),
        // The floor is stated on the line it qualifies rather than in a footnote
        // somewhere else: the driver reports that samples went, never how many,
        // so one is the smallest number that is certainly not an overstatement.
        row("blocks lost", grouped(h.blocks_lost), Some("a floor")),
        row(
            "feed refused",
            grouped(h.refused_session),
            Some("never reached a decoder"),
        ),
        row(
            "queue peak",
            grouped(h.peak_depth),
            Some("of 4, last window"),
        ),
        Line::from(""),
        heading("WHAT WAS DECODED", theme),
        // Not a zero. A zero on this line would say we looked and found nothing,
        // and nothing looks yet. See the module header. Drawn through the same
        // row builder as everything else, so its note obeys the same rule: this
        // one was hand-built and was the only note on the panel that truncated.
        count(
            "bursts",
            "—".to_string(),
            theme.stale,
            Some("no detector yet"),
            theme,
            width,
        ),
    ]
}

impl Panel for NetDecodeHealthPanel {
    fn name(&self) -> &'static str {
        "net_decode_health"
    }

    fn min_size(&self) -> (u16, u16) {
        (44, 12)
    }

    fn chrome(&self, state: &SdrMetrics) -> PanelChrome {
        // Every number here is a count of something that arrived, so it goes out
        // of date the moment the samples stop. The engine tags and cools it; the
        // panel only declares which rule it lives under.
        PanelChrome::new("Feed Health")
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
        let lines = lines(&state.net.health, theme, inner.width as usize);
        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use crate::state::fixture::draw;
    use crate::state::SdrMetrics;

    use super::NetDecodeHealthPanel;

    fn streaming() -> SdrMetrics {
        let mut m = SdrMetrics::fixture().streaming();
        m.net.health.blocks_in = 12_403;
        m.net.health.pairs_in = 1_626_505_216;
        m.net.health.run_blocks = 940;
        m.net.health.gaps = 3;
        m.net.health.blocks_lost = 7;
        m.net.health.refused_session = 2;
        m.net.health.refused = 1;
        m.net.health.peak_depth = 4;
        m
    }

    #[test]
    fn the_counts_reach_the_screen_grouped_and_labelled() {
        let out = draw(NetDecodeHealthPanel, 60, 18, &streaming()).join("\n");
        assert!(out.contains("12 403"), "{out}");
        assert!(out.contains("interruptions"), "{out}");
        // The pair count is the one figure shown in units rather than in full,
        // because past the third digit it stops being a fact anybody wants.
        assert!(out.contains("1.63 Gsamp"), "{out}");
        assert!(!out.contains("1 626 505 216"), "{out}");
        // The floor is stated on the line it qualifies, not in a footnote.
        assert!(out.contains("a floor"), "{out}");
    }

    /// The one line here that must never become a zero.
    ///
    /// `bursts 0` is a claim that the receiver looked and found nothing, which
    /// is a different statement from "nothing is looking" and the more
    /// flattering of the two. Rule 2: what cannot be asked is refused.
    #[test]
    fn nothing_is_decoded_and_the_panel_says_so_rather_than_showing_zero() {
        let out = draw(NetDecodeHealthPanel, 60, 18, &streaming()).join("\n");
        let bursts = out
            .lines()
            .find(|l| l.contains("bursts"))
            .expect("a bursts line");
        assert!(bursts.contains('—'), "{bursts}");
        assert!(!bursts.contains('0'), "{bursts}");
        assert!(bursts.contains("no detector yet"), "{bursts}");
    }

    /// A radio that has delivered nothing says nothing, and the chrome carries
    /// the reason.
    #[test]
    fn a_feed_that_has_delivered_nothing_reads_as_nothing() {
        let out = draw(NetDecodeHealthPanel, 60, 18, &SdrMetrics::fixture()).join("\n");
        assert!(out.contains("blocks"), "{out}");
        assert!(out.contains("0 samp"), "{out}");
        assert!(out.contains("[STALE]"), "{out}");
    }

    /// A note is dropped whole or drawn whole, and no row ever runs past the
    /// frame.
    ///
    /// The first version of this checked for `"   " + prefix + "\n"`, which the
    /// row padding means can never match: every assertion in it passed on a
    /// panel that was visibly truncating "no detector is running yet" to "no
    /// detector is r". Anchored to the end of the line's content instead.
    #[test]
    fn it_fits_every_size_the_layout_can_hand_it() {
        const NOTES: [&str; 5] = [
            "since the last break",
            "runs broken",
            "a floor",
            "never reached a decoder",
            "no detector yet",
        ];
        for w in 20..90u16 {
            for h in 4..24u16 {
                for line in draw(NetDecodeHealthPanel, w, h, &streaming()) {
                    assert!(
                        line.chars().count() <= w as usize,
                        "{w}x{h} overran: {line:?}"
                    );
                    let content =
                        line.trim_matches(|c: char| c == ' ' || c == '│' || "╭╮╰╯─".contains(c));
                    for note in NOTES {
                        for cut in 1..note.chars().count() {
                            // Anchored on the three-space separator, because a
                            // label ending in "s" is not a truncated "since".
                            let prefix: String = note.chars().take(cut).collect();
                            assert!(
                                !content.ends_with(&format!("   {prefix}")),
                                "{w}x{h} truncated {note:?}: {line:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}
