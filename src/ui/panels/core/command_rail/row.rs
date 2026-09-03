// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The rail's row vocabulary: the one label column every section lines its
//! values up against.

use ratatui::{style::Style, text::Span};

/// Width of the label column. Six, so the five-character `TOTAL` still keeps a
/// gap before its value; every section pads to the same width so the values form
/// a single column down the whole rail.
const LABEL_W: usize = 6;

/// A label cell: the name, left-aligned in the shared column, in the label
/// colour. Pair with whatever span the section wants after it.
pub(super) fn label_cell(text: &str, theme: &crate::Theme) -> Span<'static> {
    Span::styled(
        format!("{text:<LABEL_W$}"),
        Style::default().fg(theme.label),
    )
}

/// The same cell, marked when the rail's focus mode is pointed at this stage.
///
/// The marker replaces the label's leading column rather than being added to it,
/// so the bars stay aligned: a row that shifted right when selected would make
/// the whole block twitch.
pub(super) fn stage_label_cell(text: &str, selected: bool, theme: &crate::Theme) -> Span<'static> {
    if !selected {
        return label_cell(text, theme);
    }
    let inner = LABEL_W.saturating_sub(1);
    Span::styled(
        format!("\u{25B8}{text:<inner$}"),
        Style::default()
            .fg(theme.value_hi)
            .add_modifier(ratatui::style::Modifier::BOLD),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_label_occupies_the_same_column() {
        let t = crate::theme::Theme::sdr();
        // Short and long names alike pad to the shared width, so DROP, BUF, USB
        // and TOTAL all start their values at the same x.
        for name in ["BUF", "DROP", "TOTAL", "LNA"] {
            assert_eq!(
                label_cell(name, &t).content.chars().count(),
                LABEL_W,
                "{name}"
            );
        }
        assert!(label_cell("BUF", &t).content.starts_with("BUF"));
        // A name longer than the column is not truncated - it pushes its own
        // value along rather than losing a character.
        assert_eq!(label_cell("HEADROOM", &t).content, "HEADROOM");
    }
}
