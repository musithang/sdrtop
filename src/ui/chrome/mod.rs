//! Shared chrome — one frame and nameplate vocabulary for every panel.
//!
//! - [`frame`]: the panel frame itself. [`frame::render_frame`] turns a panel's
//!   [`PanelChrome`](crate::ui::panel::PanelChrome) into a border, a nameplate
//!   and the inner rect, so no panel draws its own box. The schematic-deck
//!   pieces ([`deck_block`], [`corner_accents`], [`junction_caps`]) live there
//!   too and are re-exported here.
//! - This module: the vocabulary panels use *inside* that frame — nameplate
//!   labels, the `├╴ SECTION ╶──── hint` subheading, `label : value` fields,
//!   text wrapping, and the density helpers that let an airy line stack breathe
//!   or compress to any terminal height.
//!
//! The deck's frame language is square (Plain) borders with a tick-tab
//! nameplate on the top rule, `┌╴LABEL╶─────┐`, which reads as precision field
//! instrument rather than a soft rounded window, without touching the palette.

pub mod frame;

pub use frame::{
    corner_accents, corner_accents_top, deck_block, deck_block_borders, junction_caps,
};

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

/// Wrap nameplate label spans with tick end-caps: `╴…╶`. The caller may append
/// live state tags after the returned spans before building the title `Line`.
pub fn nameplate<'a>(label_spans: Vec<Span<'a>>, tick_color: Color) -> Vec<Span<'a>> {
    let mut spans = Vec::with_capacity(label_spans.len() + 2);
    spans.push(Span::styled("╴", Style::default().fg(tick_color)));
    spans.extend(label_spans);
    spans.push(Span::styled("╶", Style::default().fg(tick_color)));
    spans
}

/// `├╴ SECTION ╶──── hint` — the shared lab side-panel subheading, spanning the
/// full inner width `iw`: a `├╴` tick tab, the uppercased bold label, a `╶` cap,
/// a dim dashed rule filling the middle, and an optional right-aligned `hint`.
///
/// This is the one nameplate every lab side panel (`iq_diagnostics`, `rf_chain`,
/// `timing_diagnostics`, `signal_characterization`, `fm_demod`) groups its zones
/// with, so they read as one instrument family. `hint` is owned so it can carry a
/// live value.
pub fn section(name: &str, hint: &str, iw: usize, theme: &crate::Theme) -> Line<'static> {
    let dim = theme.border_dim;
    let label_txt = name.to_uppercase();
    let left = label_txt.chars().count() + 5;
    let hint_w = if hint.is_empty() { 0 } else { hint.chars().count() + 1 };
    let dashes = iw.saturating_sub(left + hint_w);
    let mut spans = vec![
        Span::styled("\u{251c}\u{2574} ".to_string(), Style::default().fg(dim)),
        Span::styled(label_txt, Style::default().fg(theme.label).add_modifier(Modifier::BOLD)),
        Span::styled(" \u{2576}".to_string(), Style::default().fg(dim)),
        Span::styled("\u{2500}".repeat(dashes), Style::default().fg(dim)),
    ];
    if !hint.is_empty() {
        spans.push(Span::styled(format!(" {hint}"), Style::default().fg(dim)));
    }
    Line::from(spans)
}

/// A label cell for a side panel's `label : value` rows: a leading space then the
/// name left-padded to `width`, in `theme.label`, so values line up down the zone.
/// Pair with the value span the caller appends. Shared so every lab panel uses the
/// same column rhythm; each panel picks the `width` that clears its longest label.
pub fn field(name: &str, width: usize, theme: &crate::Theme) -> Span<'static> {
    Span::styled(format!(" {name:<width$}"), Style::default().fg(theme.label))
}

/// Break `text` into at most `max_rows` rows of `width` columns, preferring a
/// space to break on but never collapsing the spaces *inside* a row.
///
/// The shared alternative to letting a `Paragraph` clip: a chopped line reads as
/// a value (`Strong carrier (47 dB), 1.25`), so anything long enough to overrun a
/// lab panel's narrow column — RadioText, the verdict copy — comes through here
/// instead.
///
/// Interior gaps survive on purpose. RadioText arrives character by character and
/// unconfirmed positions read as blanks, so a half-decoded message is full of gaps
/// — and squeezing them out glues the surviving fragments into words the station
/// never sent. `"Now Play  Exam      Track"` is visibly a message still filling in;
/// `"Now Play Exam Track"` looks like a decoder that is simply wrong.
pub fn wrap(text: &str, width: usize, max_rows: usize) -> Vec<String> {
    if width == 0 || max_rows == 0 { return Vec::new(); }
    let chars: Vec<char> = text.chars().collect();
    let mut rows = Vec::new();
    let mut i = 0;
    while i < chars.len() && rows.len() < max_rows {
        let hard = (i + width).min(chars.len());
        // Break on a space when one falls in the back half of the row; breaking
        // any earlier wastes more space than the tidier edge is worth.
        let mut cut = hard;
        if hard < chars.len() && !chars[hard].is_whitespace() {
            if let Some(p) = chars[i..hard].iter().rposition(|c| *c == ' ') {
                if p > width / 2 { cut = i + p; }
            }
        }
        rows.push(chars[i..cut].iter().collect::<String>().trim_end().to_string());
        // Consume only the single space we broke on, so interior gaps survive.
        i = cut + usize::from(chars.get(cut) == Some(&' '));
    }
    rows
}

/// Which blank-spacer indices to drop so an airy stack of `total` lines fits
/// `avail` rows. `blank_idx` lists the indices (into the full line list) of the
/// droppable spacer rows, in order.
///
/// When the overflow meets or exceeds the whole spacer budget every spacer goes
/// (true dense). Otherwise only as many as needed are removed, picked evenly
/// across the spacer list so the surviving breathing room stays balanced —
/// instead of the all-or-nothing cliff that, at in-between heights, collapsed a
/// panel to fully dense and stranded a block of blank rows at its foot.
pub fn spacers_to_drop(total: usize, blank_idx: &[usize], avail: usize) -> Vec<usize> {
    if total <= avail { return Vec::new(); }
    let excess = total - avail;
    if excess >= blank_idx.len() { return blank_idx.to_vec(); }
    (0..excess).map(|k| blank_idx[k * blank_idx.len() / excess]).collect()
}

/// Fit an airy `lines` stack into `avail` rows in place: drop only as many blank
/// spacer rows as needed — evenly across the stack — so a panel keeps as much
/// breathing room as fits rather than snapping to fully dense and stranding empty
/// rows. A blank row is one whose spans are all whitespace. No-op when it already
/// fits. The shared self-adjusting-density routine for every airy-stack panel.
pub fn collapse_spacers(lines: &mut Vec<Line<'_>>, avail: usize) {
    if lines.len() <= avail { return; }
    let blank_idx: Vec<usize> = lines.iter().enumerate()
        .filter(|(_, l)| l.spans.iter().all(|s| s.content.trim().is_empty()))
        .map(|(i, _)| i)
        .collect();
    let drop: std::collections::HashSet<usize> =
        spacers_to_drop(lines.len(), &blank_idx, avail).into_iter().collect();
    if drop.is_empty() { return; }
    let mut i = 0usize;
    lines.retain(|_| { let keep = !drop.contains(&i); i += 1; keep });
}

/// The filling counterpart to [`collapse_spacers`]: when an airy stack is *shorter*
/// than `avail`, grow its existing blank spacers so the content spreads to use the
/// whole panel instead of bunching at the top and stranding empty rows at the foot.
/// The leftover rows are distributed evenly across the current blank positions, so
/// every gap opens up by a balanced amount. No-op when the stack already fills (or
/// overflows) `avail`, or when it has no blank spacers to grow.
pub fn pad_to_fill(lines: &mut Vec<Line<'_>>, avail: usize) {
    if lines.len() >= avail { return; }
    let blank_idx: Vec<usize> = lines.iter().enumerate()
        .filter(|(_, l)| l.spans.iter().all(|s| s.content.trim().is_empty()))
        .map(|(i, _)| i)
        .collect();
    if blank_idx.is_empty() { return; }
    let extra = avail - lines.len();
    // How many blank rows to add at each existing spacer, spread evenly.
    let mut add = vec![0usize; blank_idx.len()];
    for k in 0..extra { add[k * blank_idx.len() / extra] += 1; }
    // Insert from the back so earlier indices stay valid.
    for (&bi, &n) in blank_idx.iter().zip(add.iter()).rev() {
        for _ in 0..n { lines.insert(bi, Line::raw("")); }
    }
}

/// Fit an airy `lines` stack to exactly `avail` rows: drop spacers when it
/// overflows, grow them when it underflows. The one call a panel makes to both
/// breathe and fill across every terminal height.
pub fn fit_spacers(lines: &mut Vec<Line<'_>>, avail: usize) {
    collapse_spacers(lines, avail);
    pad_to_fill(lines, avail);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_fills_full_inner_width() {
        let t = crate::theme::Theme::sdr();
        // With and without a hint, the dashed rule stretches the line to exactly iw.
        let plain = super::section("CALLBACK TIMING", "", 40, &t);
        let w: usize = plain.spans.iter().map(|s| s.content.chars().count()).sum();
        assert_eq!(w, 40, "section must span the full inner width");
        let hinted = super::section("DEADLINE BUDGET", "\u{250a} = \u{00b1}600 \u{00b5}s", 44, &t);
        let w: usize = hinted.spans.iter().map(|s| s.content.chars().count()).sum();
        assert_eq!(w, 44, "hinted section still spans the full inner width");
        // Label is uppercased.
        assert!(hinted.spans.iter().any(|s| s.content == "DEADLINE BUDGET"));
    }

    #[test]
    fn field_leading_space_and_left_padded() {
        let t = crate::theme::Theme::sdr();
        let s = super::field("Rate", 11, &t);
        assert_eq!(s.content.chars().count(), 12, "1 lead space + 11-wide label");
        assert!(s.content.starts_with(" Rate"));
        assert!(s.content.ends_with("       "), "short label is right-padded with spaces");
    }

    #[test]
    fn spacers_to_drop_keeps_all_when_it_fits() {
        let blanks = vec![3, 5, 7, 9];
        assert!(spacers_to_drop(20, &blanks, 20).is_empty(), "exact fit drops nothing");
        assert!(spacers_to_drop(18, &blanks, 20).is_empty(), "room to spare drops nothing");
    }

    #[test]
    fn spacers_to_drop_drops_all_when_overflow_exceeds_budget() {
        let blanks = vec![3, 5, 7, 9];
        // Overflow of 6 but only 4 spacers — every spacer must go (true dense).
        assert_eq!(spacers_to_drop(30, &blanks, 24), blanks);
        // Overflow exactly equal to the spacer count also clears them all.
        assert_eq!(spacers_to_drop(28, &blanks, 24), blanks);
    }

    #[test]
    fn spacers_to_drop_removes_only_excess_spread_evenly() {
        let blanks = vec![3, 5, 7, 9, 11, 13]; // 6 spacers
        // Overflow of 2 → drop 2 spacers, spread across the list (not the first two).
        let drop = spacers_to_drop(20, &blanks, 18);
        assert_eq!(drop, vec![3, 9], "evenly spaced: 1st and 4th spacer");
    }

    #[test]
    fn pad_to_fill_grows_spacers_to_use_height() {
        use ratatui::text::Span;
        // content rows at 0,2,4 with blank spacers at 1,3 → 5 lines into 9 rows.
        let mut lines = vec![
            Line::from(Span::raw("a")), Line::raw(""),
            Line::from(Span::raw("b")), Line::raw(""),
            Line::from(Span::raw("c")),
        ];
        pad_to_fill(&mut lines, 9);
        assert_eq!(lines.len(), 9, "stack grows to fill the panel");
        // Extra rows land in the two gaps, evenly (2 each here).
        assert!(lines[1].spans.iter().all(|s| s.content.trim().is_empty()));
    }

    #[test]
    fn pad_to_fill_noop_when_full_or_no_spacers() {
        use ratatui::text::Span;
        let mut full = vec![Line::from(Span::raw("a")), Line::raw(""), Line::from(Span::raw("b"))];
        pad_to_fill(&mut full, 3);
        assert_eq!(full.len(), 3, "already fills → unchanged");
        let mut no_blanks = vec![Line::from(Span::raw("a")), Line::from(Span::raw("b"))];
        pad_to_fill(&mut no_blanks, 10);
        assert_eq!(no_blanks.len(), 2, "no spacers to grow → unchanged");
    }

    #[test]
    fn spacers_to_drop_indices_distinct_and_sized() {
        let blanks: Vec<usize> = (0..13).collect();
        for excess in 1..13 {
            let drop = spacers_to_drop(40, &blanks, 40 - excess);
            let unique: std::collections::HashSet<_> = drop.iter().collect();
            assert_eq!(drop.len(), unique.len(), "excess={excess}: no repeated index");
            assert_eq!(drop.len(), excess, "excess={excess}: drops exactly `excess`");
        }
    }

    #[test]
    fn collapse_spacers_drops_only_excess_in_place() {
        // 3 content rows interleaved with 3 spacers (6 lines); fit into 5 → drop 1.
        let mk = |s: &str| Line::from(Span::raw(s.to_string()));
        let blank = || Line::from(Span::raw("   ".to_string()));
        let mut lines = vec![mk("a"), blank(), mk("b"), blank(), mk("c"), blank()];
        collapse_spacers(&mut lines, 5);
        assert_eq!(lines.len(), 5, "exactly one spacer removed");
        // All three content rows survive.
        let content: Vec<String> = lines.iter()
            .filter(|l| !l.spans.iter().all(|s| s.content.trim().is_empty()))
            .map(|l| l.spans[0].content.to_string())
            .collect();
        assert_eq!(content, vec!["a", "b", "c"]);
    }

    #[test]
    fn wrap_breaks_radiotext_on_words() {
        let rows = wrap("Now playing something long", 12, 3);
        assert_eq!(rows, vec!["Now playing", "something", "long"]);
        for r in &rows { assert!(r.chars().count() <= 12); }
    }

    #[test]
    fn wrap_keeps_the_gaps_in_a_half_decoded_message() {
        // The bug this guards, seen live at 92 groups: RadioText fills in
        // character by character, and collapsing the unconfirmed blanks turned
        // "Now Playing - Example Artist - Example Track Title" into
        // "Now Play Exam Track" — fragments glued into words the
        // station never sent.
        let partial = "Now Play  Exam      Track";
        let rows = wrap(partial, 40, 2);
        assert_eq!(rows, vec![partial]);
    }

    #[test]
    fn wrap_respects_the_row_budget_and_the_column() {
        // More text than rows: the surplus is dropped, never overflowed.
        let rows = wrap("one two three four five six", 9, 2);
        assert_eq!(rows.len(), 2);
        for r in &rows { assert!(r.chars().count() <= 9); }
        // A word longer than the column is cut rather than allowed to overhang.
        assert_eq!(wrap("supercalifragilistic", 8, 1), vec!["supercal"]);
        // Degenerate geometry yields nothing rather than panicking.
        assert!(wrap("text", 0, 2).is_empty());
        assert!(wrap("text", 10, 0).is_empty());
    }

    #[test]
    fn wrap_keeps_a_verdict_headline_whole() {
        // The B2 case: 120-column terminal → 29 inner → 28 for the copy. The
        // headline must break between words, not mid-word as the clip did.
        let rows = wrap("\u{26a0} WFM CARRIER \u{2014} ADJACENT SPLATTER", 28, 2);
        assert_eq!(rows, vec!["\u{26a0} WFM CARRIER \u{2014} ADJACENT", "SPLATTER"]);
    }

    #[test]
    fn collapse_spacers_noop_when_it_fits() {
        let mk = |s: &str| Line::from(Span::raw(s.to_string()));
        let mut lines = vec![mk("a"), mk("b")];
        collapse_spacers(&mut lines, 10);
        assert_eq!(lines.len(), 2);
    }
}
