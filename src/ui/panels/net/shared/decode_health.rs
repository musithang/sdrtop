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
//! **What was decoded, and what nothing decodes.** Two decoders run now, and
//! their funnels are counted where they happen: every BLE trigger ends as a
//! packet whose CRC passed, one whose CRC failed, or a capture nothing could be
//! decoded from (`signal::ble::receive::Funnel`); classic Bluetooth counts
//! access-code hits and the piconets whose UAP is resolved. A decoder that has
//! not run this session shows `—` and "not decoding", never a zero: a row
//! reading `0` is a claim that it looked and found none. There is still no
//! protocol-agnostic burst detector (the foundation plan's N14 gap), and its
//! row says so the same way - rule 2 in the small: what cannot be asked is
//! refused, never invented.

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::SdrMetrics;
use crate::ui::chrome::{fit_spacers, section};
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

/// The whole panel body, as a function of the state and the width alone.
///
/// Split out because nothing on this panel is a function of anything else: no
/// device, no clock, no lock. The same split `signal::fft` makes, for the same
/// reason.
fn lines(state: &SdrMetrics, theme: &crate::Theme, width: usize) -> Vec<Line<'static>> {
    let h = &state.net.health;
    let row = |label, value, note| count(label, value, theme.value, note, theme, width);
    let dash = |label, note| {
        count(
            label,
            "—".to_string(),
            theme.stale,
            Some(note),
            theme,
            width,
        )
    };
    let mut out = vec![
        section("what arrived", "", width, theme),
        row("blocks", grouped(h.blocks_in), None),
        row("I/Q pairs", samples(h.pairs_in), None),
        row(
            "current run",
            grouped(h.run_blocks),
            Some("since the last break"),
        ),
        Line::from(""),
        section("what did not", "", width, theme),
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
        section("what was decoded", "", width, theme),
    ];

    // BLE: every trigger, and how it ended. Shown once the decoder has run this
    // session; before that, a dash, because a zero would say it looked.
    let f = h.ble;
    if state.net.ble_channel.is_some() || f.triggered > 0 {
        out.push(row(
            "BLE triggers",
            grouped(f.triggered),
            Some("the detector fired"),
        ));
        out.push(row("CRC good", grouped(f.decoded), None));
        out.push(row(
            "CRC failed",
            grouped(f.crc_failed),
            Some("length matches the signal"),
        ));
        out.push(row("gave up", grouped(f.gave_up), Some("nothing matched")));
    } else {
        out.push(dash("BLE", "not decoding"));
    }

    // Classic: hits, and how many of the piconets they came from have a UAP.
    if !state.net.bt_channels_watched.is_empty() || h.bt_hits > 0 {
        let resolved = state.net.bt_uap.values().filter(|c| c.len() == 1).count();
        out.push(row("BT hits", grouped(h.bt_hits), Some("access codes")));
        out.push(row(
            "UAPs resolved",
            format!("{resolved} of {}", state.net.bt_uap.len()),
            Some("piconets named"),
        ));
    } else {
        out.push(dash("classic BT", "not decoding"));
    }

    // What it cost: the same figure the header band shows (Stop 1.3), red
    // above the stream's own pace, where the worker is falling behind.
    out.push(match h.decode_load {
        Some(load) => count(
            "decode load",
            format!("{:.0} %", load * 100.0),
            if load > 1.0 {
                theme.status_crit
            } else {
                theme.value
            },
            (load > 1.0).then_some("falling behind"),
            theme,
            width,
        ),
        None => dash("decode load", "not measured yet"),
    });

    // Not a zero. A zero on this line would say we looked and found nothing,
    // and nothing looks yet. See the module header.
    out.push(dash("bursts", "no detector yet"));
    out
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
        let mut lines = lines(state, theme, inner.width as usize);
        // Breathe like the Lab panels: the blank rows between the three
        // accounts grow on a tall panel and go first on a short one.
        fit_spacers(&mut lines, inner.height as usize);
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

    /// **The funnels, once the decoders have run.** BLE: triggers and how
    /// each ended; classic: hits and the piconets whose UAP is resolved; and
    /// what it all cost, red above the stream's own pace.
    #[test]
    fn the_decode_funnels_reach_the_screen() {
        let mut m = streaming();
        m.net.ble_channel = Some(37);
        m.net.health.ble = crate::signal::ble::receive::Funnel {
            triggered: 1_204,
            decoded: 951,
            crc_failed: 3,
            gave_up: 250,
        };
        m.net.bt_channels_watched = vec![38, 39, 40];
        m.net.health.bt_hits = 312;
        m.net.bt_uap.insert(0x9e8b33, vec![0x47]);
        m.net.bt_uap.insert(0x123456, vec![0x10, 0x90]);
        m.net.health.decode_load = Some(1.07);
        let out = draw(NetDecodeHealthPanel, 64, 30, &m).join("\n");
        let line = |label: &str| {
            out.lines()
                .find(|l| l.contains(label))
                .unwrap_or_else(|| panic!("{label}:\n{out}"))
                .to_string()
        };
        assert!(line("BLE triggers").contains("1 204"), "{out}");
        assert!(line("CRC good").contains("951"), "{out}");
        assert!(line("gave up").contains("250"), "{out}");
        assert!(line("BT hits").contains("312"), "{out}");
        assert!(line("UAPs resolved").contains("1 of 2"), "{out}");
        assert!(line("decode load").contains("107 %"), "{out}");
        assert!(line("decode load").contains("falling behind"), "{out}");
        // Still true, still said.
        assert!(line("bursts").contains("no detector yet"), "{out}");
    }

    /// A decoder that has not run this session is a dash, never a zero.
    #[test]
    fn a_decoder_that_never_ran_is_a_dash_not_a_zero() {
        let out = draw(NetDecodeHealthPanel, 64, 30, &streaming()).join("\n");
        for label in ["BLE", "classic BT", "decode load"] {
            let row = out
                .lines()
                .find(|l| l.trim_start_matches(['│', ' ']).starts_with(label))
                .unwrap_or_else(|| panic!("{label}:\n{out}"));
            assert!(row.contains('—'), "{row}");
            assert!(!row.contains('0'), "{row}");
        }
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
        const NOTES: [&str; 12] = [
            "since the last break",
            "runs broken",
            "a floor",
            "never reached a decoder",
            "no detector yet",
            "the detector fired",
            "length matches the signal",
            "nothing matched",
            "access codes",
            "piconets named",
            "not decoding",
            "not measured yet",
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
