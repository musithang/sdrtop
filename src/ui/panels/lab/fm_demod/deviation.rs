//! `DEVIATION` — how far the carrier is being swung, against the nominal limit
//! for the mode, plus where the carrier actually sits inside the channel.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::state::{deviation_limit_hz, FmMeasure, Modulation};
use crate::ui::chrome;
use crate::ui::widgets::micro_common::bar_spans;

use super::fmt::{fmt_hz, fmt_offset, lbl, val};
use super::stack::Stack;

pub(super) fn lines(
    stack: &mut Stack<'static>, measure: Option<FmMeasure>, modulation: Modulation,
    iw: usize, theme: &crate::Theme,
) {
    let limit = deviation_limit_hz(modulation);
    let hint = format!("{:.0} kHz max", limit / 1000.0);
    stack.heading(chrome::section("DEVIATION", &hint, iw, theme));
    match measure {
        Some(FmMeasure { peak_dev_hz, rms_dev_hz, carrier_offset_hz }) => {
            let ratio = if limit > 0.0 { peak_dev_hz / limit } else { 0.0 };
            let color = deviation_color(ratio, theme);
            stack.push(Line::from(vec![
                chrome::field("Peak", 8, theme),
                Span::styled(fmt_hz(peak_dev_hz), Style::default().fg(color).add_modifier(Modifier::BOLD)),
            ]));
            // Bar width leaves room for the leading space and the trailing "%".
            let bar_w = iw.saturating_sub(9).min(14);
            if bar_w >= 4 {
                let mut row = vec![Span::raw(" ")];
                row.extend(bar_spans(ratio.clamp(0.0, 1.0) as f64, bar_w, color, theme));
                row.push(Span::styled(format!(" {:.0}%", (ratio * 100.0).min(999.0)), lbl(theme)));
                stack.ornament(Line::from(row));
            }
            stack.detail(Line::from(vec![
                chrome::field("RMS", 8, theme),
                Span::styled(fmt_hz(rms_dev_hz), val(theme)),
            ]));
            // Not "Offset": the headline already uses that word for the
            // channel's offset from the tuned centre, and this is the opposite
            // measurement with the opposite sign — where the carrier sits inside
            // the channel, i.e. the tuning error the demodulator is seeing.
            stack.detail(Line::from(vec![
                chrome::field("Carrier", 8, theme),
                Span::styled(fmt_offset(carrier_offset_hz), val(theme)),
            ]));
        }
        None => stack.gap(),
    }
}

/// Colour for a deviation reading against its nominal limit: over the limit is
/// a real transmitter fault, and just under it is worth flagging.
fn deviation_color(ratio: f32, theme: &crate::Theme) -> ratatui::style::Color {
    if ratio >= 1.0      { theme.status_crit }
    else if ratio >= 0.9 { theme.status_warn }
    else                 { theme.status_ok }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deviation_color_escalates_at_the_limit() {
        let t = crate::Theme::sdr();
        assert_eq!(deviation_color(0.5, &t), t.status_ok);
        assert_eq!(deviation_color(0.95, &t), t.status_warn);
        // At and above the nominal limit this is over-deviation — a real fault.
        assert_eq!(deviation_color(1.0, &t), t.status_crit);
        assert_eq!(deviation_color(1.4, &t), t.status_crit);
    }
}
