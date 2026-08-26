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
        // A name longer than the column is not truncated — it pushes its own
        // value along rather than losing a character.
        assert_eq!(label_cell("HEADROOM", &t).content, "HEADROOM");
    }
}
