// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The bonded seam: what the waterfall draws instead of its own nameplate when
//! it sits directly under the spectrum.
//!
//! Bonded, the two panels are one instrument. The waterfall's **top border
//! becomes the shared frequency ruler** - a `┬`-ticked MHz scale aligned to the
//! plot, with `─` fill so it reads as a continuous engraved rule rather than a
//! box edge. The panel keeps its identity without a second frame: the
//! `╴WATERFALL╶` plate rides the rule's left cap and the live status tags ride a
//! right cap on the bottom border.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::ui::chrome;
use crate::ui::panels::core::spectrum::freq_scale_spans;

use super::Status;

/// The frequency span on screen: the left edge and the width, in hertz.
#[derive(Clone, Copy)]
pub(super) struct Window {
    pub left_hz: f64,
    pub bw: f64,
}

/// Draw the ruler, the name cap and the status cap. `plot` is the grid area the
/// ticks must line up with; `area` is the whole panel.
pub(super) fn seam(
    f: &mut Frame,
    area: Rect,
    plot: Rect,
    window: Window,
    status: &Status,
    border: Color,
    theme: &crate::Theme,
) {
    // The rule, aligned to the plot rather than the panel, so a tick sits over
    // the column it names.
    let ruler = freq_scale_spans(
        window.left_hz,
        window.bw,
        plot.width as usize,
        border,
        theme.value,
        '\u{2500}',
    );
    f.render_widget(
        Paragraph::new(Line::from(ruler)),
        Rect {
            x: plot.x,
            y: area.y,
            width: plot.width,
            height: 1,
        },
    );

    // Left cap: the nameplate, overlaid on the rule's left end so the panel name
    // stays where the eye expects it while the ticks stay aligned beneath.
    let name = nameplate(border, theme);
    let name_w: u16 = name
        .iter()
        .map(|s| s.width() as u16)
        .sum::<u16>()
        .min(area.width.saturating_sub(2));
    f.render_widget(
        Paragraph::new(Line::from(name)),
        Rect {
            x: area.x + 1,
            y: area.y,
            width: name_w,
            height: 1,
        },
    );

    // Right cap on the BOTTOM border: the live status the nameplate used to
    // carry, right-aligned. Only when there is something to say.
    let tags = status_tags(status, theme);
    if tags.is_empty() || area.height < 2 {
        return;
    }
    let tick = Style::default().fg(border);
    let mut cap = vec![Span::styled("\u{2574}", tick)];
    cap.extend(tags);
    cap.push(Span::styled("\u{2576}", tick));
    let w: u16 = cap.iter().map(|s| s.width() as u16).sum();
    f.render_widget(
        Paragraph::new(Line::from(cap)),
        Rect {
            x: (area.x + area.width).saturating_sub(2 + w),
            y: area.y + area.height - 1,
            width: w,
            height: 1,
        },
    );
}

/// `╴WATERFALL╶` with the **second** `L` lit: that is the focus key, and the
/// first `L` is not.
pub(super) fn nameplate(border: Color, theme: &crate::Theme) -> Vec<Span<'static>> {
    let key = Style::default()
        .fg(theme.value_hi)
        .add_modifier(Modifier::BOLD);
    let name = Style::default()
        .fg(theme.label)
        .add_modifier(Modifier::BOLD);
    chrome::nameplate(
        vec![
            Span::styled("WATERFA", name),
            Span::styled("L", key),
            Span::styled("L", name),
        ],
        border,
    )
}

/// The compact glyph form of the status tags, for the bottom-border cap. The
/// unbonded nameplate spells the same state out in brackets instead.
fn status_tags(status: &Status, theme: &crate::Theme) -> Vec<Span<'static>> {
    let mut tags = Vec::new();
    if status.paused {
        tags.push(Span::styled(
            "\u{23F8} ",
            Style::default().fg(theme.status_warn),
        ));
    } else if status.stale {
        tags.push(Span::styled("STALE ", Style::default().fg(theme.stale)));
    }
    if status.stride > 1 {
        tags.push(Span::styled(
            format!("\u{00D7}{} ", status.stride),
            Style::default().fg(theme.label),
        ));
    }
    if status.scroll > 0 {
        tags.push(Span::styled(
            format!("\u{2191}{} ", status.scroll),
            Style::default().fg(theme.value_hi),
        ));
    }
    tags
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(spans: &[Span<'_>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn the_focus_key_is_the_second_l() {
        let t = crate::theme::Theme::sdr();
        let plate = nameplate(Color::Reset, &t);
        assert_eq!(text(&plate), "\u{2574}WATERFALL\u{2576}");
        // Exactly one span carries the key colour, and it is the L at index 8.
        let lit: Vec<&str> = plate
            .iter()
            .filter(|s| s.style.fg == Some(t.value_hi))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(lit, vec!["L"]);
        let before: usize = plate
            .iter()
            .take_while(|s| s.style.fg != Some(t.value_hi))
            .map(|s| s.content.chars().count())
            .sum();
        assert_eq!(
            before, 8,
            "the tick plus WATERFA, so the lit L is the second one"
        );
    }

    #[test]
    fn the_status_cap_says_only_what_is_true() {
        let t = crate::theme::Theme::sdr();
        let quiet = Status {
            paused: false,
            stale: false,
            stride: 1,
            scroll: 0,
        };
        assert!(
            status_tags(&quiet, &t).is_empty(),
            "nothing to report, no cap"
        );

        let busy = Status {
            paused: true,
            stale: true,
            stride: 4,
            scroll: 12,
        };
        assert_eq!(
            text(&status_tags(&busy, &t)),
            "\u{23F8} \u{00D7}4 \u{2191}12 ",
            "paused outranks stale, then stride, then scroll-back"
        );

        let drifting = Status {
            paused: false,
            stale: true,
            stride: 1,
            scroll: 0,
        };
        assert_eq!(text(&status_tags(&drifting, &t)), "STALE ");
    }
}
