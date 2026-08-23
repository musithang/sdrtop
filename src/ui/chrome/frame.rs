//! The panel frame: one place that turns a [`PanelChrome`] into a border, a
//! nameplate and the inner rect the panel draws into.
//!
//! Before this existed every panel hand-rolled the same twelve lines, which is
//! how the focus-key highlight went missing three separate times and how five
//! different title styles crept into one deck. A panel now says *what it is*;
//! the spelling, the colours and the `[STALE]` rule live here.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

use crate::ui::panel::{FrameStyle, PanelChrome, Tag};

/// A panel frame in the schematic deck language: square corners, single rule.
pub fn deck_block<'a>(border_color: Color) -> Block<'a> {
    deck_block_borders(border_color, Borders::ALL)
}

/// Like [`deck_block`] but with an explicit border set — used when two panels
/// bond into one instrument and the facing edge is dropped (e.g. the spectrum
/// renders `TOP | LEFT | RIGHT`, letting the waterfall's top border below it act
/// as the shared frequency ruler).
pub fn deck_block_borders<'a>(border_color: Color, borders: Borders) -> Block<'a> {
    Block::default()
        .borders(borders)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(border_color))
}

/// Overlay only the top reinforced corners (`┏┓`) — for a panel bonded below a
/// neighbour, which has no bottom border to anchor `┗┛`.
pub fn corner_accents_top(f: &mut Frame, area: Rect, color: Color) {
    if area.width < 2 || area.height < 1 { return; }
    let style = Style::default().fg(color);
    let r = area.x + area.width - 1;
    f.render_widget(Paragraph::new(Span::styled("\u{250F}", style)),
                    Rect { x: area.x, y: area.y, width: 1, height: 1 }); // ┏
    f.render_widget(Paragraph::new(Span::styled("\u{2513}", style)),
                    Rect { x: r, y: area.y, width: 1, height: 1 });      // ┓
}

/// Overlay `├` / `┤` T-junctions on the top corners of `area`, in `color`. Used
/// at a bonded boundary so the shared-ruler row ties into the continuous side
/// borders of the panel above instead of reading as a separate box's `┌`/`┐`.
pub fn junction_caps(f: &mut Frame, area: Rect, color: Color) {
    if area.width < 2 { return; }
    let style = Style::default().fg(color);
    let r = area.x + area.width - 1;
    f.render_widget(Paragraph::new(Span::styled("\u{251C}", style)),
                    Rect { x: area.x, y: area.y, width: 1, height: 1 }); // ├
    f.render_widget(Paragraph::new(Span::styled("\u{2524}", style)),
                    Rect { x: r, y: area.y, width: 1, height: 1 });      // ┤
}

/// Overlay reinforced "bracket" corners on an already-rendered panel frame, in
/// the panel's own border colour. The heavier corner glyphs (`┏┓┗┛`) against the
/// light edges read as fastened instrument-panel corners — a schematic-deck
/// detail that adds structure without touching the colour palette. Call right
/// after rendering the block. No-op for frames too small to have real corners.
pub fn corner_accents(f: &mut Frame, area: Rect, color: Color) {
    if area.width < 2 || area.height < 2 { return; }
    let style = Style::default().fg(color);
    let (l, t) = (area.x, area.y);
    let (r, b) = (area.x + area.width - 1, area.y + area.height - 1);
    for (x, y, ch) in [
        (l, t, "\u{250F}"), // ┏
        (r, t, "\u{2513}"), // ┓
        (l, b, "\u{2517}"), // ┗
        (r, b, "\u{251B}"), // ┛
    ] {
        f.render_widget(Paragraph::new(Span::styled(ch, style)),
                        Rect { x, y, width: 1, height: 1 });
    }
}

/// Draw `chrome`'s frame into `area` and return the inner rect to render into,
/// or `None` when the frame leaves nothing to draw in.
///
/// This is the only call site of the border rule, so "focused wins over stale
/// wins over default" is stated once for the whole deck.
pub fn render_frame(
    f: &mut Frame,
    area: Rect,
    chrome: &PanelChrome,
    focus_key: Option<char>,
    stale: bool,
    focused: bool,
    theme: &crate::Theme,
) -> Option<Rect> {
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color(chrome.frame, stale, focused, theme)));

    let spans = title_spans(chrome, focus_key, stale, theme);
    if !spans.is_empty() {
        block = block.title(Line::from(spans));
    }

    let inner = block.inner(area);
    f.render_widget(block, area);
    (inner.width > 0 && inner.height > 0).then_some(inner)
}

/// The border colour for a frame in a given state.
fn border_color(style: FrameStyle, stale: bool, focused: bool, theme: &crate::Theme) -> Color {
    match style {
        // A muted frame never takes focus and never goes stale, so it never moves.
        FrameStyle::Muted => theme.border_dim,
        FrameStyle::Instrument | FrameStyle::SelfFramed => {
            if focused { theme.border_focused }
            else if stale { theme.stale }
            else { theme.border_default }
        }
    }
}

/// Build the nameplate spans: `␣Name [K] [STALE] [FRZ]suffix␣`.
///
/// An untitled panel gets no nameplate at all, tags and suffix included — a box
/// with nothing but `[STALE]` on its rule reads as debris rather than a title.
pub fn title_spans(
    chrome: &PanelChrome,
    focus_key: Option<char>,
    stale: bool,
    theme: &crate::Theme,
) -> Vec<Span<'static>> {
    if chrome.title.is_empty() { return Vec::new(); }

    let name = Style::default().fg(theme.label).add_modifier(Modifier::BOLD);
    let key  = Style::default().fg(theme.value_hi).add_modifier(Modifier::BOLD);
    let mut spans = vec![Span::raw(" ")];

    match split_mnemonic(chrome.title) {
        // The key is a letter of the name: light it where it stands.
        Some((before, letter, after)) => {
            debug_assert!(
                focus_key.is_some_and(|k| k.eq_ignore_ascii_case(&letter)),
                "{}: nameplate marks '{letter}' but focus_key() is {focus_key:?}",
                chrome.title,
            );
            if !before.is_empty() { spans.push(Span::styled(before.to_string(), name)); }
            spans.push(Span::styled(letter.to_string(), key));
            if !after.is_empty() { spans.push(Span::styled(after.to_string(), name)); }
        }
        // No marker: the name as written, and the key bracketed after it if the
        // panel has one. Never silently absent — that was the bug.
        None => {
            spans.push(Span::styled(chrome.title.to_string(), name));
            if let Some(k) = focus_key {
                spans.push(Span::styled(" [", name));
                spans.push(Span::styled(k.to_uppercase().to_string(), key));
                spans.push(Span::styled("]", name));
            }
        }
    }

    // The suffix belongs to the name (it says what the panel *is*); the tags say
    // what it is *doing right now*, so they go last, together, at the edge where
    // the eye looks for status.
    if let Some(suffix) = &chrome.suffix {
        spans.push(Span::styled(suffix.clone(), Style::default().fg(theme.label)));
    }
    if stale {
        spans.push(Span::styled(" [STALE]", Style::default().fg(theme.stale)));
    }
    for tag in &chrome.tags {
        let (text, color) = match tag {
            Tag::Frozen => ("FRZ", theme.status_warn),
        };
        spans.push(Span::styled(format!(" [{text}]"), Style::default().fg(color)));
    }

    spans.push(Span::raw(" "));
    spans
}

/// Split a nameplate on its `_` mnemonic marker into (before, key letter, after).
/// `None` when there is no marker, or the title ends with one and so marks
/// nothing.
fn split_mnemonic(title: &str) -> Option<(&str, char, &str)> {
    let (before, rest) = title.split_once('_')?;
    let letter = rest.chars().next()?;
    Some((before, letter, &rest[letter.len_utf8()..]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::panel::{PanelChrome, Staleness};

    /// The rendered nameplate as plain text, for asserting on shape.
    fn text(spans: &[Span<'_>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// The single span drawn in the focus-key colour, if any.
    fn key_span<'a>(spans: &'a [Span<'a>], theme: &crate::Theme) -> Option<&'a str> {
        spans.iter()
            .find(|s| s.style.fg == Some(theme.value_hi))
            .map(|s| s.content.as_ref())
    }

    #[test]
    fn a_marked_letter_is_lit_where_it_stands() {
        let t = crate::theme::Theme::sdr();
        let spans = title_spans(&PanelChrome::new("Sig_nal Metrics"), Some('n'), false, &t);
        assert_eq!(text(&spans), " Signal Metrics ", "the marker itself never prints");
        assert_eq!(key_span(&spans, &t), Some("n"), "the marked letter carries the key colour");
    }

    #[test]
    fn a_key_that_is_not_in_the_name_is_bracketed_after_it() {
        let t = crate::theme::Theme::sdr();
        let spans = title_spans(&PanelChrome::new("Signal Characterization"), Some('x'), false, &t);
        assert_eq!(text(&spans), " Signal Characterization [X] ");
        assert_eq!(key_span(&spans, &t), Some("X"), "bracketed keys are uppercased");
    }

    #[test]
    fn a_panel_with_a_focus_key_always_advertises_it() {
        // The regression this guards: a panel that claims a key but never shows
        // it. Neither spelling can drop the key, because both branches emit it.
        let t = crate::theme::Theme::sdr();
        for (title, k) in [("_Timing Diagnostics", 't'), ("ADC Loading", 't')] {
            let spans = title_spans(&PanelChrome::new(title), Some(k), false, &t);
            assert!(key_span(&spans, &t).is_some(), "{title} hides its focus key");
        }
        // And a panel with no key gets a plain name, not an empty bracket pair.
        let spans = title_spans(&PanelChrome::new("ADC Loading"), None, false, &t);
        assert_eq!(text(&spans), " ADC Loading ");
    }

    #[test]
    fn stale_comes_before_the_live_tags() {
        let t = crate::theme::Theme::sdr();
        let chrome = PanelChrome::new("RF _Diagnostics")
            .stale_when(Staleness::NotStreaming)
            .tag_if(true, Tag::Frozen);
        assert_eq!(text(&title_spans(&chrome, Some('d'), true, &t)),
                   " RF Diagnostics [STALE] [FRZ] ");
        // Live, but still holding a snapshot: only the tag.
        assert_eq!(text(&title_spans(&chrome, Some('d'), false, &t)),
                   " RF Diagnostics [FRZ] ");
    }

    #[test]
    fn the_stale_tag_is_drawn_in_the_stale_colour() {
        let t = crate::theme::Theme::sdr();
        let spans = title_spans(&PanelChrome::new("Hardware _Vitals"), Some('v'), true, &t);
        let tag = spans.iter().find(|s| s.content.contains("STALE")).expect("no [STALE] tag");
        assert_eq!(tag.style.fg, Some(t.stale));
    }

    #[test]
    fn the_suffix_is_appended_verbatim_and_the_tags_follow_it() {
        // No space is injected: the sweep panel wants two before its band, the
        // strip chart wants one before its `·`. Each writes its own.
        let t = crate::theme::Theme::sdr();
        let chrome = PanelChrome::new("Sweep").suffix("  400.0-500.0 MHz");
        assert_eq!(text(&title_spans(&chrome, Some('g'), false, &t)),
                   " Sweep [G]  400.0-500.0 MHz ");
        // Status sits at the edge, past the descriptive suffix, rather than
        // splitting the name from the rest of its own phrase.
        let chart = PanelChrome::new("Callback Interval").suffix(" · Strip Chart");
        assert_eq!(text(&title_spans(&chart, None, true, &t)),
                   " Callback Interval · Strip Chart [STALE] ");
    }

    #[test]
    fn an_untitled_panel_gets_no_nameplate() {
        let t = crate::theme::Theme::sdr();
        let chrome = PanelChrome::untitled().tag_if(true, Tag::Frozen);
        assert!(title_spans(&chrome, None, true, &t).is_empty(),
                "an untitled box stays untitled, tags and staleness included");
    }

    #[test]
    fn a_muted_frame_never_reacts_to_focus_or_staleness() {
        let t = crate::theme::Theme::sdr();
        for (stale, focused) in [(false, false), (true, false), (false, true), (true, true)] {
            assert_eq!(border_color(FrameStyle::Muted, stale, focused, &t), t.border_dim);
        }
        // An instrument frame does, and focus outranks staleness.
        assert_eq!(border_color(FrameStyle::Instrument, false, false, &t), t.border_default);
        assert_eq!(border_color(FrameStyle::Instrument, true,  false, &t), t.stale);
        assert_eq!(border_color(FrameStyle::Instrument, true,  true,  &t), t.border_focused);
    }

    #[test]
    fn split_mnemonic_handles_the_edges() {
        assert_eq!(split_mnemonic("_IQ Diagnostics"), Some(("", 'I', "Q Diagnostics")));
        assert_eq!(split_mnemonic("WATERFA_LL"), Some(("WATERFA", 'L', "L")));
        assert_eq!(split_mnemonic("ADC Loading"), None);
        assert_eq!(split_mnemonic("Trailing_"), None, "a marker marking nothing is no marker");
    }
}
