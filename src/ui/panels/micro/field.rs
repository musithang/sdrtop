//! The vocabulary the field views share.
//!
//! All of these panels open the same way - a status badge and the tuned
//! frequency - and all of them use the same two conventions inside: a dim label,
//! and `---` where a number is not being measured. That header was written out
//! three times, byte for byte, in `gain`, `health` and `signal`.
//!
//! Named `field` rather than `header` because it is the field view's whole
//! shared language, not just its first row.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::state::SdrMetrics;
use crate::ui::widgets::micro_common::{fmt_freq_mhz, status_badge};

/// The row every field view opens with: what the radio is doing, and where it is
/// pointed. The two things you look at first on arrival.
pub(super) fn header(state: &SdrMetrics, theme: &crate::Theme) -> Line<'static> {
    let [dot, word] = status_badge(state, theme);
    Line::from(vec![
        Span::raw(" "),
        dot,
        word,
        Span::raw("   "),
        Span::styled(
            fmt_freq_mhz(state.radio.frequency),
            Style::default()
                .fg(theme.value_hi)
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

/// The label / no-reading conventions, carried together with whether the radio
/// is streaming so a caller does not have to thread `stale` separately.
#[derive(Clone, Copy)]
pub(super) struct Field<'a> {
    pub theme: &'a crate::Theme,
    pub stale: bool,
}

impl<'a> Field<'a> {
    pub(super) fn new(state: &SdrMetrics, theme: &'a crate::Theme) -> Self {
        Self {
            theme,
            stale: !state.radio.hw_streaming,
        }
    }

    pub(super) fn label(&self, text: impl Into<String>) -> Span<'static> {
        Span::styled(text.into(), Style::default().fg(self.theme.label))
    }

    /// A label padded to a fixed column, so the values below it line up.
    pub(super) fn padded(&self, text: &str, w: usize) -> Span<'static> {
        self.label(format!("{text:<w$}"))
    }

    /// The stand-in for a number that is not being measured.
    pub(super) fn dash(&self) -> Span<'static> {
        Span::styled("---".to_string(), Style::default().fg(self.theme.stale))
    }

    pub(super) fn value(&self, text: impl Into<String>) -> Span<'static> {
        Span::styled(text.into(), Style::default().fg(self.theme.value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(l: &Line<'static>) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// The header says both things, and says which state the radio is in.
    #[test]
    fn the_header_carries_the_state_and_the_frequency() {
        let t = crate::Theme::sdr();
        let idle = header(&SdrMetrics::fixture(), &t);
        assert!(text(&idle).contains("IDLE"), "{:?}", text(&idle));
        assert!(text(&idle).contains("100.000"), "{:?}", text(&idle));

        let live = header(&SdrMetrics::fixture().streaming(), &t);
        assert!(text(&live).contains("RX"), "{:?}", text(&live));
        assert!(!text(&live).contains("IDLE"));
    }

    /// A padded label is exactly its column, however long the name is - a name
    /// wider than the column pushes rather than truncating, because a clipped
    /// label is worse than a nudged one.
    #[test]
    fn a_padded_label_fills_its_column() {
        let t = crate::Theme::sdr();
        let f = Field {
            theme: &t,
            stale: false,
        };
        assert_eq!(f.padded("SAT", 6).content.chars().count(), 6);
        assert_eq!(f.padded("", 6).content.chars().count(), 6);
        assert_eq!(f.padded("ADC util", 6).content.chars().count(), 8);
    }
}
