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

/// Like [`deck_block`] but with an explicit border set - used when two panels
/// bond into one instrument and the facing edge is dropped (e.g. the spectrum
/// renders `TOP | LEFT | RIGHT`, letting the waterfall's top border below it act
/// as the shared frequency ruler).
pub fn deck_block_borders<'a>(border_color: Color, borders: Borders) -> Block<'a> {
    Block::default()
        .borders(borders)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(border_color))
}

/// Overlay only the top reinforced corners (`┏┓`) - for a panel bonded below a
/// neighbour, which has no bottom border to anchor `┗┛`.
pub fn corner_accents_top(f: &mut Frame, area: Rect, color: Color) {
    if area.width < 2 || area.height < 1 {
        return;
    }
    let style = Style::default().fg(color);
    let r = area.x + area.width - 1;
    f.render_widget(
        Paragraph::new(Span::styled("\u{250F}", style)),
        Rect {
            x: area.x,
            y: area.y,
            width: 1,
            height: 1,
        },
    ); // ┏
    f.render_widget(
        Paragraph::new(Span::styled("\u{2513}", style)),
        Rect {
            x: r,
            y: area.y,
            width: 1,
            height: 1,
        },
    ); // ┓
}

/// Overlay `├` / `┤` T-junctions on the top corners of `area`, in `color`. Used
/// at a bonded boundary so the shared-ruler row ties into the continuous side
/// borders of the panel above instead of reading as a separate box's `┌`/`┐`.
pub fn junction_caps(f: &mut Frame, area: Rect, color: Color) {
    if area.width < 2 {
        return;
    }
    let style = Style::default().fg(color);
    let r = area.x + area.width - 1;
    f.render_widget(
        Paragraph::new(Span::styled("\u{251C}", style)),
        Rect {
            x: area.x,
            y: area.y,
            width: 1,
            height: 1,
        },
    ); // ├
    f.render_widget(
        Paragraph::new(Span::styled("\u{2524}", style)),
        Rect {
            x: r,
            y: area.y,
            width: 1,
            height: 1,
        },
    ); // ┤
}

/// Overlay reinforced "bracket" corners on an already-rendered panel frame, in
/// the panel's own border colour. The heavier corner glyphs (`┏┓┗┛`) against the
/// light edges read as fastened instrument-panel corners - a schematic-deck
/// detail that adds structure without touching the colour palette. Call right
/// after rendering the block. No-op for frames too small to have real corners.
pub fn corner_accents(f: &mut Frame, area: Rect, color: Color) {
    if area.width < 2 || area.height < 2 {
        return;
    }
    let style = Style::default().fg(color);
    let (l, t) = (area.x, area.y);
    let (r, b) = (area.x + area.width - 1, area.y + area.height - 1);
    for (x, y, ch) in [
        (l, t, "\u{250F}"), // ┏
        (r, t, "\u{2513}"), // ┓
        (l, b, "\u{2517}"), // ┗
        (r, b, "\u{251B}"), // ┛
    ] {
        f.render_widget(
            Paragraph::new(Span::styled(ch, style)),
            Rect {
                x,
                y,
                width: 1,
                height: 1,
            },
        );
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
    // Borderless: nothing to draw, the panel gets the whole rect.
    if chrome.frame == FrameStyle::Borderless {
        return (area.width > 0 && area.height > 0).then_some(area);
    }

    let color = border_color(chrome, stale, focused, theme);
    let deck = chrome.frame == FrameStyle::Deck;
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(if deck {
            BorderType::Plain
        } else {
            BorderType::Rounded
        })
        .border_style(Style::default().fg(color));

    let spans = title_spans(chrome, focus_key, stale, theme);
    if !spans.is_empty() {
        block = block.title(Line::from(spans));
    }

    let inner = block.inner(area);
    f.render_widget(block, area);
    // The reinforced corners are an overlay, so they go on after the block.
    if deck {
        corner_accents(f, area, color);
    }
    (inner.width > 0 && inner.height > 0).then_some(inner)
}

/// The outer rect an all-borders frame carved `inner` out of.
///
/// For the rare panel that deliberately draws *over* its own side borders: the
/// header's tuning dial runs the full width so its `├` and `┤` end caps tie into
/// the frame instead of stopping one column short of it. Everything else should
/// stay inside the rect it was handed.
pub fn outer_of(inner: Rect) -> Rect {
    Rect {
        x: inner.x.saturating_sub(1),
        y: inner.y.saturating_sub(1),
        width: inner.width + 2,
        height: inner.height + 2,
    }
}

/// The border colour for a frame in a given state: focus outranks staleness,
/// which outranks the panel's declared tone.
///
/// [`Tag::Paused`] counts as staleness here. A paused panel's readings are as
/// frozen as an aged-out one's - the border only says "this is not advancing",
/// and the plate says which of the two it is.
fn border_color(chrome: &PanelChrome, stale: bool, focused: bool, theme: &crate::Theme) -> Color {
    if focused {
        theme.border_focused
    } else if stale || chrome.tags.contains(&Tag::Paused) {
        theme.stale
    } else {
        chrome.tone.color(theme)
    }
}

/// The colour the engine draws this panel's frame in.
///
/// For the two plots whose axes are tinted to match their own border: the engine
/// hands them the *inner* rect, so they cannot read the colour off the frame and
/// would otherwise re-derive the rule. Asking for it keeps "focus outranks
/// staleness outranks tone" stated exactly once.
pub fn frame_color(
    chrome: &PanelChrome,
    state: &crate::state::SdrMetrics,
    focused: bool,
    theme: &crate::Theme,
) -> Color {
    border_color(chrome, chrome.staleness.resolve(state), focused, theme)
}

/// Build the nameplate spans: `␣Name [K]suffix [STALE] [FRZ]␣`, or on a deck
/// frame the engraved tick-tab form `╴NAME╶`.
///
/// An untitled panel gets no nameplate at all, tags and suffix included - a box
/// with nothing but `[STALE]` on its rule reads as debris rather than a title.
pub fn title_spans(
    chrome: &PanelChrome,
    focus_key: Option<char>,
    stale: bool,
    theme: &crate::Theme,
) -> Vec<Span<'static>> {
    if chrome.title.is_empty() {
        return Vec::new();
    }

    // A deck nameplate is an engraved plate: uppercase, between tick end caps in
    // the border's own colour. An instrument nameplate is a label: mixed case,
    // held off the corners by a plain space.
    let deck = chrome.frame == FrameStyle::Deck;
    let name = Style::default()
        .fg(theme.label)
        .add_modifier(Modifier::BOLD);
    let key = Style::default()
        .fg(theme.value_hi)
        .add_modifier(Modifier::BOLD);
    let tick = Style::default().fg(border_color(chrome, stale, false, theme));
    let plate = |s: &str| {
        if deck {
            s.to_uppercase()
        } else {
            s.to_string()
        }
    };

    let mut spans = vec![if deck {
        Span::styled("\u{2574}", tick)
    } else {
        Span::raw(" ")
    }];

    match split_mnemonic(chrome.title) {
        // The key is a letter of the name: light it where it stands.
        Some((before, letter, after)) => {
            debug_assert!(
                focus_key.is_some_and(|k| k.eq_ignore_ascii_case(&letter)),
                "{}: nameplate marks '{letter}' but focus_key() is {focus_key:?}",
                chrome.title,
            );
            if !before.is_empty() {
                spans.push(Span::styled(plate(before), name));
            }
            spans.push(Span::styled(plate(&letter.to_string()), key));
            if !after.is_empty() {
                spans.push(Span::styled(plate(after), name));
            }
        }
        // No marker: the name as written, and the key bracketed after it if the
        // panel has one. Never silently absent - that was the bug.
        None => {
            spans.push(Span::styled(plate(chrome.title), name));
            if let Some(k) = focus_key {
                spans.push(Span::styled(" [", name));
                spans.push(Span::styled(k.to_uppercase().to_string(), key));
                spans.push(Span::styled("]", name));
            }
        }
    }

    if deck {
        spans.push(Span::styled("\u{2576}", tick));
    }

    // The suffix belongs to the name (it says what the panel *is*); the tags say
    // what it is *doing right now*, so they go last, together, at the edge where
    // the eye looks for status.
    if let Some(suffix) = &chrome.suffix {
        spans.push(Span::styled(
            suffix.clone(),
            Style::default().fg(theme.label),
        ));
    }
    // A paused panel is not stale, it is held. Both tags say "not advancing", so
    // printing them together would be two answers to one question - the more
    // specific one wins.
    if stale && !chrome.tags.contains(&Tag::Paused) {
        spans.push(Span::styled(" [STALE]", Style::default().fg(theme.stale)));
    }
    for tag in &chrome.tags {
        let (text, color) = match tag {
            Tag::Frozen => ("FRZ".to_string(), theme.status_warn),
            Tag::Hold => ("HOLD".to_string(), theme.status_warn),
            Tag::Paused => ("PAUSED".to_string(), theme.status_warn),
            Tag::Stride(n) => (format!("\u{00D7}{n}"), theme.label),
            Tag::Scroll(n) => (format!("\u{2191}{n}"), theme.value_hi),
        };
        spans.push(Span::styled(
            format!(" [{text}]"),
            Style::default().fg(color),
        ));
    }

    // The deck's closing tick already caps the plate; only a label nameplate
    // needs a space to keep it off the corner.
    if !deck {
        spans.push(Span::raw(" "));
    }
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
    use crate::ui::panel::{FrameTone, PanelChrome, Staleness};

    /// The rendered nameplate as plain text, for asserting on shape.
    fn text(spans: &[Span<'_>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// The single span drawn in the focus-key colour, if any.
    fn key_span<'a>(spans: &'a [Span<'a>], theme: &crate::Theme) -> Option<&'a str> {
        spans
            .iter()
            .find(|s| s.style.fg == Some(theme.value_hi))
            .map(|s| s.content.as_ref())
    }

    #[test]
    fn a_marked_letter_is_lit_where_it_stands() {
        let t = crate::theme::Theme::sdr();
        let spans = title_spans(&PanelChrome::new("Sig_nal Metrics"), Some('n'), false, &t);
        assert_eq!(
            text(&spans),
            " Signal Metrics ",
            "the marker itself never prints"
        );
        assert_eq!(
            key_span(&spans, &t),
            Some("n"),
            "the marked letter carries the key colour"
        );
    }

    #[test]
    fn a_key_that_is_not_in_the_name_is_bracketed_after_it() {
        let t = crate::theme::Theme::sdr();
        let spans = title_spans(
            &PanelChrome::new("Signal Characterization"),
            Some('x'),
            false,
            &t,
        );
        assert_eq!(text(&spans), " Signal Characterization [X] ");
        assert_eq!(
            key_span(&spans, &t),
            Some("X"),
            "bracketed keys are uppercased"
        );
    }

    #[test]
    fn a_panel_with_a_focus_key_always_advertises_it() {
        // The regression this guards: a panel that claims a key but never shows
        // it. Neither spelling can drop the key, because both branches emit it.
        let t = crate::theme::Theme::sdr();
        for (title, k) in [("_Timing Diagnostics", 't'), ("ADC Loading", 't')] {
            let spans = title_spans(&PanelChrome::new(title), Some(k), false, &t);
            assert!(
                key_span(&spans, &t).is_some(),
                "{title} hides its focus key"
            );
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
        assert_eq!(
            text(&title_spans(&chrome, Some('d'), true, &t)),
            " RF Diagnostics [STALE] [FRZ] "
        );
        // Live, but still holding a snapshot: only the tag.
        assert_eq!(
            text(&title_spans(&chrome, Some('d'), false, &t)),
            " RF Diagnostics [FRZ] "
        );
    }

    #[test]
    fn the_stale_tag_is_drawn_in_the_stale_colour() {
        let t = crate::theme::Theme::sdr();
        let spans = title_spans(&PanelChrome::new("Hardware _Vitals"), Some('v'), true, &t);
        let tag = spans
            .iter()
            .find(|s| s.content.contains("STALE"))
            .expect("no [STALE] tag");
        assert_eq!(tag.style.fg, Some(t.stale));
    }

    #[test]
    fn the_suffix_is_appended_verbatim_and_the_tags_follow_it() {
        // No space is injected: the sweep panel wants two before its band, the
        // strip chart wants one before its `·`. Each writes its own.
        let t = crate::theme::Theme::sdr();
        let chrome = PanelChrome::new("Sweep").suffix("  400.0-500.0 MHz");
        assert_eq!(
            text(&title_spans(&chrome, Some('g'), false, &t)),
            " Sweep [G]  400.0-500.0 MHz "
        );
        // Status sits at the edge, past the descriptive suffix, rather than
        // splitting the name from the rest of its own phrase.
        let chart = PanelChrome::new("Callback Interval").suffix(" · Strip Chart");
        assert_eq!(
            text(&title_spans(&chart, None, true, &t)),
            " Callback Interval · Strip Chart [STALE] "
        );
    }

    #[test]
    fn an_untitled_panel_gets_no_nameplate() {
        let t = crate::theme::Theme::sdr();
        let chrome = PanelChrome::untitled().tag_if(true, Tag::Frozen);
        assert!(
            title_spans(&chrome, None, true, &t).is_empty(),
            "an untitled box stays untitled, tags and staleness included"
        );
    }

    #[test]
    fn focus_outranks_staleness_which_outranks_the_declared_tone() {
        let t = crate::theme::Theme::sdr();
        let c = PanelChrome::new("Any");
        assert_eq!(
            border_color(&c, false, false, &t),
            t.border_default,
            "the tone is the base"
        );
        assert_eq!(border_color(&c, true, false, &t), t.stale);
        assert_eq!(border_color(&c, true, true, &t), t.border_focused);

        // The tone only shows through when neither override applies, so a panel
        // that is never focusable and never stale simply keeps its own slot.
        let dim = PanelChrome::new("Supporting").tone(FrameTone::Dim);
        assert_eq!(border_color(&dim, false, false, &t), t.border_dim);
        for tone in [FrameTone::Focused, FrameTone::Warn, FrameTone::Observer] {
            let c = PanelChrome::untitled().tone(tone);
            assert_eq!(border_color(&c, false, false, &t), tone.color(&t));
        }
    }

    #[test]
    fn a_deck_nameplate_is_engraved_and_a_label_one_is_not() {
        let t = crate::theme::Theme::sdr();
        // Deck: uppercase between tick caps, no padding spaces.
        let deck = PanelChrome::deck("_Command");
        assert_eq!(text(&title_spans(&deck, Some('c'), false, &t)), "╴COMMAND╶");
        assert_eq!(
            key_span(&title_spans(&deck, Some('c'), false, &t)[..], &t),
            Some("C"),
            "the mnemonic survives the uppercasing"
        );
        // Instrument: as written, held off the corners by a space.
        let inst = PanelChrome::new("_Command");
        assert_eq!(text(&title_spans(&inst, Some('c'), false, &t)), " Command ");
        // Tags land outside the plate, not inside it.
        assert_eq!(
            text(&title_spans(&deck, Some('c'), true, &t)),
            "╴COMMAND╶ [STALE]"
        );
    }

    /// The waterfall's plate, built exactly as its `chrome()` builds it. It moved
    /// here from the panel when the frame did: the ordering it pins is now the
    /// engine's rule, not the panel's, and `chrome()` needs a whole metrics
    /// snapshot to call.
    fn waterfall_plate(paused: bool, stride: usize, scroll: usize) -> PanelChrome {
        PanelChrome::deck("WATERFA_LL")
            .stale_when(Staleness::FftAge)
            .tone(FrameTone::Accent)
            .tag_if(paused, Tag::Paused)
            .tag_if(stride > 1, Tag::Stride(stride))
            .tag_if(scroll > 0, Tag::Scroll(scroll))
    }

    #[test]
    fn the_waterfall_plate_reports_state_in_a_fixed_order() {
        let t = crate::theme::Theme::sdr();
        assert_eq!(
            text(&title_spans(
                &waterfall_plate(false, 1, 0),
                Some('l'),
                false,
                &t
            )),
            "\u{2574}WATERFALL\u{2576}"
        );

        assert_eq!(
            text(&title_spans(
                &waterfall_plate(true, 4, 12),
                Some('l'),
                true,
                &t
            )),
            "\u{2574}WATERFALL\u{2576} [PAUSED] [\u{00D7}4] [\u{2191}12]",
            "paused outranks stale: a paused panel is not stale, it is held"
        );

        assert_eq!(
            text(&title_spans(
                &waterfall_plate(false, 1, 0),
                Some('l'),
                true,
                &t
            )),
            "\u{2574}WATERFALL\u{2576} [STALE]"
        );
    }

    #[test]
    fn a_paused_frame_is_cooled_even_while_its_data_is_fresh() {
        // Pausing stops the panel advancing, which is what the cool border means.
        // Without this the plate would say [PAUSED] inside a lit accent frame.
        let t = crate::theme::Theme::sdr();
        assert_eq!(
            border_color(&waterfall_plate(true, 1, 0), false, false, &t),
            t.stale
        );
        assert_eq!(
            border_color(&waterfall_plate(false, 1, 0), false, false, &t),
            t.border_accent
        );
        // Focus still outranks it, as for staleness.
        assert_eq!(
            border_color(&waterfall_plate(true, 1, 0), false, true, &t),
            t.border_focused
        );
    }

    #[test]
    fn the_spectrum_plate_lights_its_key_and_carries_hold() {
        let t = crate::theme::Theme::sdr();
        let held = PanelChrome::deck("SP_ECTRUM")
            .stale_when(Staleness::FftAge)
            .tone(FrameTone::Accent)
            .tag_if(true, Tag::Hold);
        assert_eq!(
            text(&title_spans(&held, Some('e'), false, &t)),
            "\u{2574}SPECTRUM\u{2576} [HOLD]"
        );
        assert_eq!(
            key_span(&title_spans(&held, Some('e'), false, &t)[..], &t),
            Some("E")
        );
        // Held *and* stale: status reads outward from the plate, staleness first.
        assert_eq!(
            text(&title_spans(&held, Some('e'), true, &t)),
            "\u{2574}SPECTRUM\u{2576} [STALE] [HOLD]"
        );
    }

    #[test]
    fn split_mnemonic_handles_the_edges() {
        assert_eq!(
            split_mnemonic("_IQ Diagnostics"),
            Some(("", 'I', "Q Diagnostics"))
        );
        assert_eq!(split_mnemonic("WATERFA_LL"), Some(("WATERFA", 'L', "L")));
        assert_eq!(split_mnemonic("ADC Loading"), None);
        assert_eq!(
            split_mnemonic("Trailing_"),
            None,
            "a marker marking nothing is no marker"
        );
    }
}
