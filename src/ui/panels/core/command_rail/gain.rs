//! The GAIN section of the rail: front-end boost, the per-stage bars, and the
//! TOTAL readout with its clip headroom.

use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

use crate::state::SdrMetrics;
use crate::ui::panels::core::header::gain_bar;
use crate::ui::widgets::charts::gain_bar_colored;

use super::row::label_cell;

/// Combined front-end gain for the TOTAL readout: primary + secondary stage when
/// the device has two (HackRF LNA+VGA), else just the primary (RTL-SDR tuner).
fn total_gain(lna: u32, vga: u32, has_second_stage: bool) -> u32 {
    if has_second_stage {
        lna + vga
    } else {
        lna
    }
}

/// Width of the gain bar given the rail's inner width - leaves room for the
/// `LNA ` label, a space, and a 2-col value. Clamped so it neither vanishes on a
/// narrow rail nor sprawls on a wide one.
fn gain_bar_width(inner_w: usize) -> usize {
    inner_w.saturating_sub(10).clamp(4, 12)
}

/// The GAIN section rows: front-end boost, one bar per stage, then TOTAL with
/// its clip headroom. `active` is "streaming and we own the radio" - idle, every
/// value drops to the label colour and the bars go flat.
pub(super) fn lines(
    state: &SdrMetrics,
    active: bool,
    observer: bool,
    iw: usize,
    theme: &crate::Theme,
) -> Vec<Line<'static>> {
    let gm = &state.caps.gain;
    let bar_w = gain_bar_width(iw);
    let val_col = if active { theme.value } else { theme.label };
    let mut out: Vec<Line<'static>> = Vec::new();

    // Front-end boost (HackRF RF amp / RTL-SDR tuner AGC - one flag, two names).
    let (boost_val, boost_col) = if observer {
        ("\u{2014}".to_string(), theme.label)
    } else if state.radio.amp_enabled {
        ("ON".to_string(), theme.value_hi)
    } else {
        ("OFF".to_string(), theme.label)
    };
    out.push(Line::from(vec![
        Span::raw(" "),
        label_cell(gm.boost_label(), theme),
        Span::styled(boost_val, Style::default().fg(boost_col)),
    ]));
    out.push(Line::raw(""));

    // Primary stage (LNA / Tuner): green → yellow.
    out.push(gain_row(
        label_cell(gm.primary_label(), theme),
        state.radio.lna_gain,
        gm.primary_max_db(),
        theme.status_ok,
        theme.value_hi,
        bar_w,
        active,
        val_col,
        theme,
    ));
    out.push(Line::raw(""));
    // Secondary stage (HackRF VGA only): cyan → orange.
    if gm.has_second_stage() {
        out.push(gain_row(
            label_cell("VGA", theme),
            state.radio.vga_gain,
            62,
            theme.border_accent,
            theme.status_warn,
            bar_w,
            active,
            val_col,
            theme,
        ));
        out.push(Line::raw(""));
    }

    let total = total_gain(
        state.radio.lna_gain,
        state.radio.vga_gain,
        gm.has_second_stage(),
    );
    let mut total_spans = vec![
        Span::raw(" "),
        label_cell("TOTAL", theme),
        Span::styled(format!("{total} dB"), Style::default().fg(val_col)),
    ];
    if active {
        let headroom = (-state.signal.adc_peak_dbfs).max(0.0);
        total_spans.push(Span::styled(
            "  \u{00b7}  ".to_string(),
            Style::default().fg(theme.border_dim),
        ));
        total_spans.push(Span::styled(
            format!("{headroom:.0} dB headroom"),
            Style::default().fg(theme.label),
        ));
    }
    out.push(Line::from(total_spans));
    out.push(Line::raw(""));
    out
}

/// One gain row: ` LABEL [⅛-block bar] value`.
///
/// Streaming, the bar shades along a meaning gradient (`lo` → `hi`); idle it is a
/// flat dim ⅛-block. The header draws its own flat bar through a separate path,
/// so the two never have to agree on colour.
#[allow(clippy::too_many_arguments)]
fn gain_row(
    label: Span<'static>,
    gain: u32,
    max: u32,
    lo: Color,
    hi: Color,
    bar_w: usize,
    active: bool,
    val_col: Color,
    theme: &crate::Theme,
) -> Line<'static> {
    let bar: Vec<Span<'static>> = if active {
        gain_bar_colored(gain, max, bar_w, lo, hi, theme.border_dim)
    } else {
        let (filled, empty) = gain_bar(gain, max, bar_w);
        vec![
            Span::styled(filled, Style::default().fg(theme.label)),
            Span::styled(empty, Style::default().fg(theme.border_dim)),
        ]
    };
    let mut spans = vec![Span::raw(" "), label];
    spans.extend(bar);
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
        format!("{gain:2}"),
        Style::default().fg(val_col),
    ));
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn total_gain_sums_only_with_second_stage() {
        assert_eq!(total_gain(32, 30, true), 62); // HackRF LNA+VGA
        assert_eq!(total_gain(40, 99, false), 40); // RTL-SDR tuner only
    }

    #[test]
    fn gain_bar_width_clamps() {
        assert_eq!(gain_bar_width(10), 4); // tiny rail → floor
        assert_eq!(gain_bar_width(0), 4);
        assert_eq!(gain_bar_width(22), 12); // wide rail → ceiling
        assert_eq!(gain_bar_width(18), 8); // mid → 18-10
    }
}
